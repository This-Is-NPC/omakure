use crate::node_identity::NodeIdentityError;
use crate::node_registry::RegistryError;
use crate::run_executor::ExecutionTerminal;
use crate::runs::{self, ClaimFilters, RunCompletion, RunRow};
use crate::workspace::Workspace;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Internal poll interval for an idle worker thread (no eligible jobs).
const WORKER_IDLE_POLL_MS: u64 = 250;

/// One worker thread's main loop. Claim, execute, finalize, repeat.
/// Exits when `cancel_flag` flips, or after one cycle when `once = true`.
pub(crate) fn worker_loop(
    workspace: Workspace,
    worker_id: String,
    cancel_flag: Arc<AtomicBool>,
    actor_filter: Option<String>,
    script_filter: Option<String>,
    once: bool,
) {
    worker_loop_inner(
        workspace,
        worker_id,
        cancel_flag,
        actor_filter,
        script_filter,
        once,
        None,
    );
}

/// Worker entry point used by `node serve`, which can re-check local trust
/// immediately before executing a Cue. Standalone queue workers have no node
/// identity context and therefore fail closed for Cue rows through the wrapper
/// above.
pub(crate) fn worker_loop_with_context(
    workspace: Workspace,
    worker_id: String,
    cancel_flag: Arc<AtomicBool>,
    actor_filter: Option<String>,
    script_filter: Option<String>,
    once: bool,
    context: crate::node::NodeContext,
) {
    worker_loop_inner(
        workspace,
        worker_id,
        cancel_flag,
        actor_filter,
        script_filter,
        once,
        Some(context),
    );
}

fn worker_loop_inner(
    workspace: Workspace,
    worker_id: String,
    cancel_flag: Arc<AtomicBool>,
    actor_filter: Option<String>,
    script_filter: Option<String>,
    once: bool,
    trust_context: Option<crate::node::NodeContext>,
) {
    let filters = ClaimFilters {
        actor: actor_filter,
        script: script_filter,
        exclude_cues: trust_context.is_none(),
    };

    // Resolve remote runs abandoned by a previous worker, before claiming any
    // new work.
    //
    // A Cue-origin row is deliberately not lease-stealable, so a crash leaves it
    // `running` with nothing willing to touch it. Without this it would stay
    // that way forever and the Conductor would wait on a result that can never
    // arrive. Recovery marks it terminal; it never re-runs the script.
    //
    // Best effort on purpose: a worker that cannot open the database has bigger
    // problems than an unresolved row, and failing to start over it would take
    // out the queue as well.
    if let Ok(conn) = runs::open(&workspace) {
        if let Ok(recovered) = runs::recover_abandoned_cue_runs(&conn) {
            for run_id in recovered {
                eprintln!("omakure: resolved abandoned remote run {run_id} without re-running it");
            }
        }
    }

    loop {
        if cancel_flag.load(Ordering::SeqCst) {
            return;
        }
        let conn = match runs::open(&workspace) {
            Ok(c) => c,
            Err(_) => {
                thread::sleep(Duration::from_millis(WORKER_IDLE_POLL_MS));
                continue;
            }
        };
        let claimed = match runs::claim_next(&conn, &worker_id, &filters) {
            Ok(opt) => opt,
            Err(_) => {
                drop(conn);
                thread::sleep(Duration::from_millis(WORKER_IDLE_POLL_MS));
                continue;
            }
        };
        drop(conn);
        let Some(row) = claimed else {
            if once {
                return;
            }
            thread::sleep(Duration::from_millis(WORKER_IDLE_POLL_MS));
            continue;
        };
        execute_and_finalize(
            &workspace,
            &row,
            Arc::clone(&cancel_flag),
            trust_context.as_ref(),
        );
        if once {
            return;
        }
    }
}

/// Execute one claimed row through the shared executor and write the
/// terminal transition.
fn execute_and_finalize(
    workspace: &Workspace,
    row: &RunRow,
    cancel_flag: Arc<AtomicBool>,
    trust_context: Option<&crate::node::NodeContext>,
) {
    if row.trigger == runs::RunTrigger::Cue {
        let context = trust_context.expect("generic workers cannot claim Cue rows");
        let guard = match crate::remote_cue::ExecutionGuard::acquire(context, &row.actor) {
            Ok(guard) => guard,
            Err(error) => {
                cancel_without_execution(workspace, row, error);
                return;
            }
        };
        if let Err(error) = cue_worker_preflight(context, workspace, row) {
            cancel_without_execution(workspace, row, error.to_string());
            return;
        }
        execute_and_finalize_inner(workspace, row, cancel_flag, Some(guard));
        return;
    }
    execute_and_finalize_inner(workspace, row, cancel_flag, None);
}

fn execute_and_finalize_inner(
    workspace: &Workspace,
    row: &RunRow,
    cancel_flag: Arc<AtomicBool>,
    spawn_guard: Option<crate::remote_cue::ExecutionGuard>,
) {
    // Layer 2 of the env-injection precedence table
    // (`docs/internal/env-injection-spec.md` §1): the active managed env. Reserved
    // vars (layer 4) are pushed after this inside `execute_with_heartbeat`
    // and remain non-overridable.
    let run_env_name = runs::open(workspace)
        .ok()
        .and_then(|conn| runs::get_run_env(&conn, &row.run_id).ok().flatten());
    let extra_env = match run_env_name.as_deref() {
        Some(name) => {
            let path = match crate::operations::envs::env_file_path(workspace, name) {
                Ok(path) => path,
                Err(err) => {
                    fail_without_execution(
                        workspace,
                        row,
                        format!("queued env resolution failed: {}", err.message),
                    );
                    return;
                }
            };
            match crate::adapters::environments::resolve_run_env(workspace.envs_dir(), Some(&path))
            {
                Ok(env) => env,
                Err(err) => {
                    fail_without_execution(
                        workspace,
                        row,
                        format!("queued env resolution failed: {err}"),
                    );
                    return;
                }
            }
        }
        None => crate::adapters::environments::resolve_active_env(workspace.envs_dir()),
    };
    let result = crate::run_executor::execute_with_heartbeat_guarded(
        workspace,
        row,
        extra_env,
        Some(cancel_flag),
        spawn_guard,
    );
    let conn = match runs::open(workspace) {
        Ok(c) => c,
        Err(_) => return,
    };
    match result.terminal {
        ExecutionTerminal::Completed => {
            let _ = runs::complete(&conn, &row.run_id, result.completion);
        }
        ExecutionTerminal::Failed | ExecutionTerminal::Errored => {
            let _ = runs::fail(&conn, &row.run_id, result.completion);
        }
        ExecutionTerminal::TimedOut => {
            let _ = runs::time_out(&conn, &row.run_id, result.completion);
        }
        ExecutionTerminal::Cancelled => {
            // The cancel transition was already written by the
            // heartbeat-detection path (or is being written now). Either
            // way, record the captured stdout/stderr on the cancelled
            // row.
            let _ = runs::record_cancelled_output(&conn, &row.run_id, result.completion);
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum CuePreflightError {
    #[error("Cue trust preflight could not load identity: {0}")]
    Identity(#[source] NodeIdentityError),
    #[error("Cue trust preflight could not open registry: {0}")]
    RegistryOpen(#[source] RegistryError),
    #[error("Cue trust preflight could not read peer trust: {0}")]
    TrustLookup(#[source] RegistryError),
    #[error("Cue sender is no longer an active trusted peer")]
    SenderNotTrusted,
    #[error("Cue sender is no longer an active conductor")]
    SenderNotConductor,
    #[error("Cue sender no longer passes local authorization policy")]
    LocalPolicyDenied,
    #[error("Cue run has no recorded script name")]
    MissingScriptName,
    #[error("Cue script is no longer declared by local policy")]
    ScriptNotDeclared,
}

fn cue_worker_preflight(
    context: &crate::node::NodeContext,
    workspace: &Workspace,
    row: &RunRow,
) -> Result<(), CuePreflightError> {
    let identity = crate::node_identity::NodeIdentity::load_existing(context)
        .map_err(CuePreflightError::Identity)?;
    let registry =
        crate::node_registry::NodeRegistry::open_existing(context, identity.public_status())
            .map_err(CuePreflightError::RegistryOpen)?;
    let authorization = registry
        .health_authorization(&row.actor)
        .map_err(CuePreflightError::TrustLookup)?;
    let Some(authorization) = authorization else {
        return Err(CuePreflightError::SenderNotTrusted);
    };
    if authorization.state != crate::node_registry::PeerState::Active
        || authorization.role != crate::node_registry::PeerRole::Conductor
    {
        return Err(CuePreflightError::SenderNotConductor);
    }
    let policy = crate::remote_cue::read_policy(context);
    let authority = crate::remote_cue::LocalAuthority {
        remote_cues_enabled: policy.enabled,
        authorization: Some(authorization),
        declared_scripts: policy.declared_scripts.clone(),
        declared_batteries: policy.declared_batteries.clone(),
    };
    if crate::remote_cue::evaluate_gates(&authority) != crate::remote_cue::GateDecision::Accepted {
        return Err(CuePreflightError::LocalPolicyDenied);
    }
    let script_name = row
        .script_name
        .as_deref()
        .ok_or(CuePreflightError::MissingScriptName)?;
    crate::remote_cue::is_declared_or_from_declared_battery(
        script_name,
        std::path::Path::new(&row.script_path),
        &policy,
        workspace,
    )
    .map_err(|_| CuePreflightError::ScriptNotDeclared)?;
    Ok(())
}

fn cancel_without_execution(workspace: &Workspace, row: &RunRow, error: String) {
    eprintln!(
        "omakure: cancelled remote run {} before execution: {error}",
        row.run_id
    );
    let Ok(conn) = runs::open(workspace) else {
        return;
    };
    let _ = runs::cancel(&conn, &row.run_id, Some(error), None);
}

fn fail_without_execution(workspace: &Workspace, row: &RunRow, error: String) {
    let Ok(conn) = runs::open(workspace) else {
        return;
    };
    let _ = runs::fail(
        &conn,
        &row.run_id,
        RunCompletion {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            success: false,
            error: Some(error),
        },
    );
}

#[cfg(test)]
mod cue_preflight_tests {
    use super::*;
    use crate::node_identity::NodeIdentity;
    use crate::node_registry::NodeRegistry;
    use crate::runs::EnqueueOptions;
    use crate::test_support::{node_context, workspace_in, write_bash_script};

    #[test]
    fn cue_preflight_distinguishes_identity_failure_from_untrusted_sender() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace_in(&temp);
        let context = node_context(temp.path());
        let script = write_bash_script(&workspace, "cue.sh", "true");
        let connection = runs::open(&workspace).unwrap();
        let row = runs::enqueue(
            &connection,
            script.to_str().unwrap(),
            &[],
            EnqueueOptions {
                actor: format!("omk1_{}", "a".repeat(64)),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let error = cue_worker_preflight(&context, &workspace, &row).unwrap_err();
        assert!(matches!(error, CuePreflightError::Identity(_)));
        assert!(error
            .to_string()
            .starts_with("Cue trust preflight could not load identity: "));

        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        drop(NodeRegistry::open(&context, identity.public_status()).unwrap());
        let error = cue_worker_preflight(&context, &workspace, &row).unwrap_err();
        assert!(
            matches!(error, CuePreflightError::SenderNotTrusted),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            "Cue sender is no longer an active trusted peer"
        );
    }
}

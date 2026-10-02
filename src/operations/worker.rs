use crate::node_identity::NodeIdentityError;
use crate::node_registry::RegistryError;
use crate::run_executor::ExecutionTerminal;
use crate::runs::{self, ClaimFilters, RunCompletion, RunRow, RunStore, RunsError};
use crate::workspace::Workspace;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

pub(crate) struct StandaloneWorkerOptions {
    pub(crate) concurrency: u32,
    pub(crate) actor_filter: Option<String>,
    pub(crate) script_filter: Option<String>,
    pub(crate) once: bool,
}

/// Run standalone queue workers until their work completes or a shutdown signal arrives.
pub(crate) fn run_standalone_workers(workspace: &Workspace, options: StandaloneWorkerOptions) {
    let cancel_flag = Arc::new(AtomicBool::new(false));
    crate::adapters::signals::install_signal_handlers(Arc::clone(&cancel_flag));

    let concurrency = options.concurrency.max(1);
    let mut handles = Vec::with_capacity(concurrency as usize);
    for thread_idx in 0..concurrency {
        let workspace = workspace.clone_for_executor();
        let cancel_flag = Arc::clone(&cancel_flag);
        let actor_filter = options.actor_filter.clone();
        let script_filter = options.script_filter.clone();
        let once = options.once;
        let worker_id = format!("worker:{}-t{}", std::process::id(), thread_idx);
        handles.push(thread::spawn(move || {
            worker_loop(
                workspace,
                worker_id,
                cancel_flag,
                actor_filter,
                script_filter,
                once,
            );
        }));
    }
    for handle in handles {
        let _ = handle.join();
    }
}

/// Internal poll interval for an idle worker thread (no eligible jobs).
const WORKER_IDLE_POLL_MS: u64 = 250;

pub(crate) fn recover_abandoned_remote_runs(workspace: &Workspace) -> Result<(), RunsError> {
    for run_id in RunStore::open(workspace)?.recover_abandoned_cue_runs()? {
        eprintln!("omakure: resolved abandoned remote run {run_id} without re-running it");
    }
    Ok(())
}

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
    let _ = recover_abandoned_remote_runs(&workspace);
    let mut workflow_recovery_failed = false;

    loop {
        if cancel_flag.load(Ordering::SeqCst) {
            return;
        }
        let row = match claim_after_workflow_recovery(
            &workspace,
            &worker_id,
            &filters,
            &mut workflow_recovery_failed,
        ) {
            Ok(Some(row)) => row,
            Ok(None) if once => return,
            _ => {
                thread::sleep(Duration::from_millis(WORKER_IDLE_POLL_MS));
                continue;
            }
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

fn claim_after_workflow_recovery(
    workspace: &Workspace,
    worker_id: &str,
    filters: &ClaimFilters,
    recovery_failed: &mut bool,
) -> Result<Option<RunRow>, RunsError> {
    let store = RunStore::open(workspace)?;
    match store.recover_workflows() {
        Ok(_) => *recovery_failed = false,
        Err(error) if !*recovery_failed => {
            eprintln!("omakure: workflow recovery remains pending: {error}");
            *recovery_failed = true;
        }
        Err(_) => {}
    }
    store.claim_next(worker_id, filters)
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
                cancel_without_execution(workspace, row, error.to_string());
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
    let extra_env = match resolve_worker_env(workspace, &row.run_id) {
        Ok(env) => env,
        Err(error) => {
            fail_without_execution(workspace, row, error);
            return;
        }
    };
    let result = crate::run_executor::execute_with_heartbeat_guarded(
        workspace,
        row,
        extra_env,
        Some(cancel_flag),
        spawn_guard,
    );
    let store = match RunStore::open(workspace) {
        Ok(store) => store,
        Err(_) => return,
    };
    let transition = match result.terminal {
        ExecutionTerminal::Completed => store.complete(&row.run_id, result.completion),
        ExecutionTerminal::Failed | ExecutionTerminal::Errored => {
            store.fail(&row.run_id, result.completion)
        }
        ExecutionTerminal::TimedOut => store.time_out(&row.run_id, result.completion),
        ExecutionTerminal::Cancelled => {
            // The cancel transition was already written by the
            // heartbeat-detection path (or is being written now). Either
            // way, record the captured stdout/stderr on the cancelled
            // row.
            store.record_cancelled_output(&row.run_id, result.completion)
        }
    };
    if transition.is_ok() {
        advance_workflow_after_run(&store, &row.run_id);
    }
}

fn resolve_worker_env(
    workspace: &Workspace,
    run_id: &str,
) -> Result<Vec<(String, String)>, String> {
    let run_env_name = RunStore::open(workspace)
        .ok()
        .and_then(|store| store.get_run_env(run_id).ok().flatten());
    let Some(name) = run_env_name else {
        return Ok(crate::adapters::environments::resolve_active_env(
            workspace.envs_dir(),
        ));
    };
    let path = crate::operations::envs::env_file_path(workspace, &name)
        .map_err(|err| format!("queued env resolution failed: {}", err.message))?;
    crate::adapters::environments::resolve_run_env(workspace.envs_dir(), Some(&path))
        .map_err(|err| format!("queued env resolution failed: {err}"))
}

fn advance_workflow_after_run(store: &RunStore, run_id: &str) {
    if let Err(error) = store.advance_workflow_for_run(run_id) {
        eprintln!("omakure: workflow advancement remains pending for run {run_id}: {error}");
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
    let Ok(store) = RunStore::open(workspace) else {
        return;
    };
    let _ = store.cancel(&row.run_id, Some(error));
}

fn fail_without_execution(workspace: &Workspace, row: &RunRow, error: String) {
    let Ok(store) = RunStore::open(workspace) else {
        return;
    };
    if store
        .fail(
            &row.run_id,
            RunCompletion {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                success: false,
                error: Some(error),
            },
        )
        .is_ok()
    {
        advance_workflow_after_run(&store, &row.run_id);
    }
}

#[cfg(all(test, unix))]
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
        let store = RunStore::open(&workspace).unwrap();
        let row = store
            .enqueue(
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
        assert!(
            error
                .to_string()
                .starts_with("Cue trust preflight could not load identity: ")
        );

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

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::test_support::workspace_in;

    #[test]
    fn abandoned_cue_recovery_retries_after_store_returns() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace_in(&temp);
        let history = workspace.history_dir();
        let backup = temp.path().join("history-backup");
        std::fs::rename(history, &backup).unwrap();
        std::fs::write(history, "blocked").unwrap();

        let error = recover_abandoned_remote_runs(&workspace).unwrap_err();
        assert!(error.to_string().starts_with("Create history dir failed: "));

        std::fs::remove_file(history).unwrap();
        std::fs::rename(&backup, history).unwrap();
        recover_abandoned_remote_runs(&workspace).unwrap();
    }
}

#[cfg(all(test, unix))]
mod workflow_tests {
    use super::*;
    use crate::runs::{RunState, WorkflowSnapshot, WorkflowState, WorkflowStepSnapshot};
    use crate::test_support::{workspace_in, write_bash_script};

    #[test]
    fn completing_a_workflow_run_queues_only_the_next_step() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = workspace_in(&temp);
        let first = write_bash_script(&workspace, "first.sh", "true");
        let second = write_bash_script(&workspace, "second.sh", "true");
        let steps = [first, second]
            .into_iter()
            .enumerate()
            .map(|(index, path)| WorkflowStepSnapshot {
                name: format!("step-{index}"),
                content_hash: crate::remote_cue::content_hash(&path).unwrap(),
                script_path: path.to_string_lossy().into_owned(),
            })
            .collect();
        let started = RunStore::open(&workspace)
            .unwrap()
            .start_workflow(
                WorkflowSnapshot {
                    battery_id: "local".into(),
                    battery_version: "1.0.0".into(),
                    battery_commit: "commit".into(),
                    workflow_name: "sequential".into(),
                    steps,
                },
                "human",
            )
            .unwrap();

        worker_loop(
            workspace.clone_for_executor(),
            "worker:workflow".into(),
            Arc::new(AtomicBool::new(false)),
            None,
            None,
            true,
        );

        let workflow = RunStore::open(&workspace)
            .unwrap()
            .get_workflow(&started.workflow_id)
            .unwrap()
            .unwrap();
        assert_eq!(workflow.state, WorkflowState::Running);
        assert_eq!(workflow.current_step, 1);
        assert_eq!(workflow.steps[0].state, Some(RunState::Completed));
        assert_eq!(workflow.steps[1].state, Some(RunState::Queued));
        assert!(workflow.steps[1].run_id.is_some());
    }
}

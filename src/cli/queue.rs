//! `omakure queue` — push, cancel, drain, and inspect the run queue.
//!
//! The `worker` subcommand is the long-running daemon that drains the
//! queue. Producers (`add`, `cancel`, `dead-letter`, `stats`) operate on
//! the same `runs.sqlite` and are short-lived CLI commands.
//!
//! Both producers and the worker write through the same state machine
//! defined in [`crate::runs`]. The worker shares its execution code path
//! with the synchronous `omakure run` fast path via
//! [`crate::run_executor::execute_with_heartbeat`].

use crate::cli::args::{
    QueueAddArgs, QueueArgs, QueueCancelArgs, QueueCommand, QueueDeadLetterArgs, QueueWorkerArgs,
};
use crate::cli::json::{self, codes};
use crate::operations::core::{self, CancelRunRequest, DeadLetterRunRequest, EnqueueRunRequest};
use crate::operations::{OperationError, OperationErrorCode};
use crate::run_executor::{execute_with_heartbeat, ExecutionTerminal};
use crate::runs::{self, ClaimFilters, RunCompletion, RunRow};
use crate::workspace::Workspace;
use serde_json::json;
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Internal poll interval for an idle worker thread (no eligible jobs).
const WORKER_IDLE_POLL_MS: u64 = 250;

pub fn run(scripts_dir: PathBuf, args: QueueArgs, json_output: bool) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;
    match args.command {
        QueueCommand::Add(opts) => add(&workspace, opts, json_output),
        QueueCommand::Cancel(opts) => cancel(&workspace, opts, json_output),
        QueueCommand::DeadLetter(opts) => dead_letter(&workspace, opts, json_output),
        QueueCommand::Worker(opts) => worker(&workspace, opts, json_output),
        QueueCommand::Stats => stats(&workspace, json_output),
    }
}

// ---------------------------------------------------------------------------
// Producers
// ---------------------------------------------------------------------------

fn add(workspace: &Workspace, opts: QueueAddArgs, json_output: bool) -> Result<(), Box<dyn Error>> {
    let timeout_ms = match opts.timeout.as_deref() {
        None => None,
        Some(s) => match parse_duration_ms(s) {
            Ok(ms) => Some(ms),
            Err(err) => return emit_error(json_output, codes::INVALID_ARGUMENT, err),
        },
    };

    let row = match core::enqueue_run(
        workspace,
        EnqueueRunRequest {
            script: opts.script,
            args: opts.args,
            env: None,
            secret_fields: Vec::new(),
            run_id: opts.run_id,
            actor: opts.actor,
            reason: opts.reason,
            priority: opts.priority,
            timeout_ms,
            parent_run_id: opts.parent_run_id,
            cron_schedule_id: opts.cron_schedule_id,
        },
    ) {
        Ok(row) => row,
        Err(err) => return emit_operation_error(json_output, err),
    };
    if json_output {
        json::print_ok(row);
    } else {
        println!("queued: {}", row.run_id);
    }
    Ok(())
}

fn cancel(
    workspace: &Workspace,
    opts: QueueCancelArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    match core::cancel_run(
        workspace,
        CancelRunRequest {
            run_id: opts.run_id,
            reason: opts.reason,
        },
    ) {
        Ok(row) => {
            if json_output {
                json::print_ok(row);
            } else {
                println!("cancelled: {}", row.run_id);
            }
            Ok(())
        }
        Err(err) => emit_operation_error(json_output, err),
    }
}

fn dead_letter(
    workspace: &Workspace,
    opts: QueueDeadLetterArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    match core::dead_letter_run(
        workspace,
        DeadLetterRunRequest {
            run_id: opts.run_id,
            reason: opts.reason,
        },
    ) {
        Ok(row) => {
            if json_output {
                json::print_ok(row);
            } else {
                println!("dead_letter: {}", row.run_id);
            }
            Ok(())
        }
        Err(err) => emit_operation_error(json_output, err),
    }
}

fn stats(workspace: &Workspace, json_output: bool) -> Result<(), Box<dyn Error>> {
    let stats = match core::queue_stats(workspace) {
        Ok(s) => s,
        Err(err) => return emit_operation_error(json_output, err),
    };
    if json_output {
        json::print_ok(stats);
    } else {
        println!("Total: {}", stats.total);
        let mut keys: Vec<_> = stats.counts_by_state.iter().collect();
        keys.sort_by(|a, b| a.0.cmp(b.0));
        for (state, count) in keys {
            println!("  {:<12} {}", state, count);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Worker daemon
// ---------------------------------------------------------------------------

fn worker(
    workspace: &Workspace,
    opts: QueueWorkerArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let cancel_flag = Arc::new(AtomicBool::new(false));
    install_signal_handlers(Arc::clone(&cancel_flag));

    let concurrency = opts.concurrency.max(1);
    let mut handles = Vec::with_capacity(concurrency as usize);
    for thread_idx in 0..concurrency {
        let workspace = workspace.clone_for_executor();
        let cancel_flag = Arc::clone(&cancel_flag);
        let actor_filter = opts.actor_filter.clone();
        let script_filter = opts.script_filter.clone();
        let once = opts.once;
        let pid = std::process::id();
        let worker_id = format!("worker:{}-t{}", pid, thread_idx);
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
    for h in handles {
        let _ = h.join();
    }

    if json_output {
        json::print_ok(json!({"status": "stopped"}));
    } else {
        println!("worker stopped");
    }
    Ok(())
}

pub(crate) fn install_signal_handlers(flag: Arc<AtomicBool>) {
    use signal_hook::consts::{SIGINT, SIGTERM};
    let _ = signal_hook::flag::register(SIGINT, Arc::clone(&flag));
    let _ = signal_hook::flag::register(SIGTERM, flag);
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
    let filters = ClaimFilters {
        actor: actor_filter,
        script: script_filter,
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
        execute_and_finalize(&workspace, &row, Arc::clone(&cancel_flag));
        if once {
            return;
        }
    }
}

/// Execute one claimed row through the shared executor and write the
/// terminal transition.
fn execute_and_finalize(workspace: &Workspace, row: &RunRow, cancel_flag: Arc<AtomicBool>) {
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
    let result = execute_with_heartbeat(workspace, row, extra_env, Some(cancel_flag));
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_duration_ms(s: &str) -> Result<i64, String> {
    let trimmed = s.trim();
    let dur = humantime::parse_duration(trimmed)
        .map_err(|err| format!("invalid duration `{}`: {}", trimmed, err))?;
    let ms = dur.as_millis();
    if ms > i64::MAX as u128 {
        return Err(format!("duration too large: {}", trimmed));
    }
    Ok(ms as i64)
}

fn emit_error(json_output: bool, code: &str, message: String) -> Result<(), Box<dyn Error>> {
    if json_output {
        json::print_err(code, message);
        std::process::exit(1);
    }
    Err(message.into())
}

fn emit_operation_error(json_output: bool, err: OperationError) -> Result<(), Box<dyn Error>> {
    let code = match err.code {
        OperationErrorCode::InvalidInput
        | OperationErrorCode::UnsafePath
        | OperationErrorCode::Conflict => codes::INVALID_ARGUMENT,
        OperationErrorCode::NotFound => codes::NOT_FOUND,
        _ => codes::INTERNAL,
    };
    emit_error(json_output, code, err.message)
}

// Used by tests to make captured-output assertions.
#[allow(dead_code)]
pub(crate) fn make_completion(
    stdout: &str,
    stderr: &str,
    exit: Option<i32>,
    ok: bool,
) -> RunCompletion {
    RunCompletion {
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        exit_code: exit,
        success: ok,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::{enqueue, EnqueueOptions, RunState};
    use std::fs;
    use std::path::PathBuf;

    fn make_workspace(label: &str) -> (Workspace, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "omakure_queue_test_{}_{}_{}",
            label,
            std::process::id(),
            runs::current_unix_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let ws = Workspace::new(dir.clone());
        ws.ensure_layout().unwrap();
        (ws, dir)
    }

    #[cfg(unix)]
    fn write_bash_stub(workspace: &Workspace, name: &str, body: &str) -> PathBuf {
        let p = workspace.root().join(name);
        fs::write(&p, format!("#!/usr/bin/env bash\n{}\n", body)).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&p, perms).unwrap();
        p
    }

    #[cfg(unix)]
    fn write_schema_bash_stub(
        workspace: &Workspace,
        name: &str,
        schema_json: &str,
        body: &str,
    ) -> PathBuf {
        write_bash_stub(
            workspace,
            name,
            &format!(
                "# OMAKURE_SCHEMA_START\n# {}\n# OMAKURE_SCHEMA_END\n{}",
                schema_json, body
            ),
        )
    }

    #[test]
    fn parse_duration_ms_recognizes_humantime_units() {
        assert_eq!(parse_duration_ms("30s").unwrap(), 30_000);
        assert_eq!(parse_duration_ms("30m").unwrap(), 1_800_000);
        assert_eq!(parse_duration_ms("1h").unwrap(), 3_600_000);
    }

    #[test]
    fn parse_duration_ms_rejects_garbage() {
        assert!(parse_duration_ms("not a duration").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn add_success_enqueues_row_with_timeout() {
        let (ws, _dir) = make_workspace("add_success");
        let script = write_bash_stub(&ws, "ok.sh", "echo done");

        add(
            &ws,
            QueueAddArgs {
                script: "ok.sh".into(),
                actor: "ai".into(),
                reason: Some("ship it".into()),
                priority: 7,
                timeout: Some("30s".into()),
                parent_run_id: Some("parent-1".into()),
                run_id: Some("rid-add".into()),
                cron_schedule_id: Some("cron-1".into()),
                args: vec!["--target".into(), "prod".into()],
            },
            false,
        )
        .unwrap();

        let conn = runs::open(&ws).unwrap();
        let row = runs::get_run(&conn, "rid-add").unwrap().unwrap();
        assert_eq!(row.state, RunState::Queued);
        assert_eq!(row.actor, "ai");
        assert_eq!(row.reason.as_deref(), Some("ship it"));
        assert_eq!(row.priority, 7);
        assert_eq!(row.timeout_ms, Some(30_000));
        assert_eq!(row.parent_run_id.as_deref(), Some("parent-1"));
        assert_eq!(row.cron_schedule_id.as_deref(), Some("cron-1"));
        assert_eq!(
            row.script_path,
            std::fs::canonicalize(script).unwrap().to_string_lossy()
        );
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn add_invalid_timeout_returns_error() {
        let (ws, _dir) = make_workspace("add_bad_timeout");
        let err = add(
            &ws,
            QueueAddArgs {
                script: "missing.sh".into(),
                actor: "human".into(),
                reason: None,
                priority: 0,
                timeout: Some("definitely-not-a-duration".into()),
                parent_run_id: None,
                run_id: None,
                cron_schedule_id: None,
                args: vec![],
            },
            false,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("not found") || err.to_string().contains("invalid duration")
        );
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn cancel_missing_run_returns_not_found() {
        let (ws, _dir) = make_workspace("cancel_missing");
        let err = cancel(
            &ws,
            QueueCancelArgs {
                run_id: "missing-run".into(),
                reason: None,
            },
            false,
        )
        .unwrap_err();

        assert!(err.to_string().contains("run not found"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn cancel_terminal_row_returns_invalid_argument() {
        let (ws, _dir) = make_workspace("cancel_terminal");
        let script = write_bash_stub(&ws, "ok.sh", "echo done");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "worker:test",
            EnqueueOptions {
                actor: "test".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        runs::complete(&conn, &row.run_id, make_completion("", "", Some(0), true)).unwrap();
        drop(conn);

        let err = cancel(
            &ws,
            QueueCancelArgs {
                run_id: row.run_id,
                reason: Some("too late".into()),
            },
            false,
        )
        .unwrap_err();

        assert!(err.to_string().contains("terminal state"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn dead_letter_failed_row_promotes_state() {
        let (ws, _dir) = make_workspace("dead_letter_failed");
        let script = write_bash_stub(&ws, "boom.sh", "exit 1");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "worker:test",
            EnqueueOptions {
                actor: "test".into(),
                reason: Some("first".into()),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        runs::fail(
            &conn,
            &row.run_id,
            make_completion("", "boom", Some(1), false),
        )
        .unwrap();
        drop(conn);

        dead_letter(
            &ws,
            QueueDeadLetterArgs {
                run_id: row.run_id.clone(),
                reason: Some("retry exhausted".into()),
            },
            false,
        )
        .unwrap();

        let conn = runs::open(&ws).unwrap();
        let updated = runs::get_run(&conn, &row.run_id).unwrap().unwrap();
        assert_eq!(updated.state, RunState::DeadLetter);
        assert!(updated
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("first"));
        assert!(updated
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("retry exhausted"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn dead_letter_completed_row_returns_invalid_argument() {
        let (ws, _dir) = make_workspace("dead_letter_completed");
        let script = write_bash_stub(&ws, "ok.sh", "echo done");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "worker:test",
            EnqueueOptions {
                actor: "test".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        runs::complete(&conn, &row.run_id, make_completion("", "", Some(0), true)).unwrap();
        drop(conn);

        let err = dead_letter(
            &ws,
            QueueDeadLetterArgs {
                run_id: row.run_id,
                reason: None,
            },
            false,
        )
        .unwrap_err();

        assert!(err.to_string().contains("only failed or timed_out"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn worker_actor_filter_claims_only_matching_jobs() {
        let (ws, _dir) = make_workspace("actor_filter");
        let script = write_bash_stub(&ws, "ok.sh", "echo done");
        let conn = runs::open(&ws).unwrap();
        let ai = enqueue(
            &conn,
            script.to_str().unwrap(),
            &[],
            EnqueueOptions {
                actor: "ai".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let human = enqueue(
            &conn,
            script.to_str().unwrap(),
            &[],
            EnqueueOptions {
                actor: "human".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        worker_loop(
            ws.clone_for_executor(),
            "worker:filter".into(),
            Arc::new(AtomicBool::new(false)),
            Some("ai".into()),
            None,
            true,
        );

        let conn = runs::open(&ws).unwrap();
        let ai_after = runs::get_run(&conn, &ai.run_id).unwrap().unwrap();
        let human_after = runs::get_run(&conn, &human.run_id).unwrap().unwrap();
        assert_eq!(ai_after.state, RunState::Completed);
        assert_eq!(human_after.state, RunState::Queued);
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn worker_drains_one_queued_job_then_exits_with_once() {
        let (ws, _dir) = make_workspace("once_drain");
        let script = write_bash_stub(&ws, "ok.sh", "echo done");
        let conn = runs::open(&ws).unwrap();
        let row = enqueue(
            &conn,
            script.to_str().unwrap(),
            &[],
            EnqueueOptions {
                actor: "test".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        let cancel_flag = Arc::new(AtomicBool::new(false));
        worker_loop(
            ws.clone_for_executor(),
            "worker:test".into(),
            cancel_flag,
            None,
            None,
            true,
        );

        let conn = runs::open(&ws).unwrap();
        let after = runs::get_run(&conn, &row.run_id).unwrap().unwrap();
        assert_eq!(after.state, RunState::Completed);
        assert_eq!(after.success, Some(true));
        let _ = fs::remove_dir_all(ws.root());
    }

    // CALL SITE: queue worker (cli/queue.rs `execute_and_finalize`). The
    // active managed env must reach the worker-spawned script; the injected
    // var must appear in the persisted run record's stdout.
    #[test]
    #[cfg(unix)]
    fn worker_injects_active_env_into_script() {
        let (ws, _dir) = make_workspace("inject_env");
        let envs = ws.envs_dir();
        fs::write(envs.join("dev.conf"), "INJECTED_VAR=queue_injected_9").unwrap();
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let script = write_bash_stub(&ws, "echo.sh", "echo \"$INJECTED_VAR\"");
        let conn = runs::open(&ws).unwrap();
        let row = enqueue(
            &conn,
            script.to_str().unwrap(),
            &[],
            EnqueueOptions {
                actor: "test".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        let cancel_flag = Arc::new(AtomicBool::new(false));
        worker_loop(
            ws.clone_for_executor(),
            "worker:test".into(),
            cancel_flag,
            None,
            None,
            true,
        );

        let conn = runs::open(&ws).unwrap();
        let after = runs::get_run(&conn, &row.run_id).unwrap().unwrap();
        assert_eq!(after.state, RunState::Completed);
        assert!(
            after.stdout.contains("queue_injected_9"),
            "expected injected var in stdout, got: {:?}",
            after.stdout
        );
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn worker_fails_queued_run_when_stored_env_disappears_before_execution() {
        let (ws, _dir) = make_workspace("queued_env_missing");
        let marker = ws.root().join("should_not_exist");
        let script = write_bash_stub(
            &ws,
            "must_not_run.sh",
            &format!("touch {}\necho should-not-run", marker.display()),
        );
        fs::write(ws.envs_dir().join("prod.conf"), "TARGET=prod\n").unwrap();
        let conn = runs::open(&ws).unwrap();
        let row = enqueue(
            &conn,
            script.to_str().unwrap(),
            &[],
            EnqueueOptions {
                actor: "test".into(),
                omakure_version: "test".into(),
                env_name: Some("prod".into()),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);
        fs::remove_file(ws.envs_dir().join("prod.conf")).unwrap();

        worker_loop(
            ws.clone_for_executor(),
            "worker:test".into(),
            Arc::new(AtomicBool::new(false)),
            None,
            None,
            true,
        );

        let conn = runs::open(&ws).unwrap();
        let after = runs::get_run(&conn, &row.run_id).unwrap().unwrap();
        assert_eq!(after.state, RunState::Failed);
        assert_eq!(after.success, Some(false));
        assert!(after
            .error
            .unwrap_or_default()
            .contains("queued env resolution failed"));
        assert!(!marker.exists());
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn worker_resolves_secret_from_active_env_and_assembles_arg() {
        let (ws, _dir) = make_workspace("secret_arg");
        let envs = ws.envs_dir();
        fs::write(envs.join("dev.conf"), "TOKEN=worker_secret_value").unwrap();
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let script = write_schema_bash_stub(
            &ws,
            "secret.sh",
            r#"{"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}"#,
            r#"if [ "$2" = "worker_secret_value" ]; then echo matched; else echo "leaked:$2"; exit 7; fi"#,
        );
        let conn = runs::open(&ws).unwrap();
        let row = enqueue(
            &conn,
            script.to_str().unwrap(),
            &[],
            EnqueueOptions {
                actor: "test".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        worker_loop(
            ws.clone_for_executor(),
            "worker:test".into(),
            Arc::new(AtomicBool::new(false)),
            None,
            None,
            true,
        );

        let conn = runs::open(&ws).unwrap();
        let after = runs::get_run(&conn, &row.run_id).unwrap().unwrap();
        assert_eq!(after.state, RunState::Completed, "stderr: {}", after.stderr);
        assert!(after.stdout.contains("matched"));
        assert!(!after.stdout.contains("worker_secret_value"));
        assert!(!after.args_json.contains("worker_secret_value"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn worker_uses_queued_env_metadata_instead_of_later_active_env() {
        let (ws, _dir) = make_workspace("queued_env_metadata");
        let envs = ws.envs_dir();
        fs::write(envs.join("prod.conf"), "TOKEN=prod_secret_value").unwrap();
        fs::write(envs.join("dev.conf"), "TOKEN=dev_secret_value").unwrap();
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let script = write_schema_bash_stub(
            &ws,
            "secret_env.sh",
            r#"{"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}"#,
            r#"if [ "$2" = "prod_secret_value" ]; then echo matched-prod; else echo "wrong:$2"; exit 9; fi"#,
        );
        let conn = runs::open(&ws).unwrap();
        let row = enqueue(
            &conn,
            script.to_str().unwrap(),
            &[],
            EnqueueOptions {
                actor: "test".into(),
                omakure_version: "test".into(),
                env_name: Some("prod".into()),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        worker_loop(
            ws.clone_for_executor(),
            "worker:test".into(),
            Arc::new(AtomicBool::new(false)),
            None,
            None,
            true,
        );

        let conn = runs::open(&ws).unwrap();
        let after = runs::get_run(&conn, &row.run_id).unwrap().unwrap();
        assert_eq!(after.state, RunState::Completed, "stderr: {}", after.stderr);
        assert!(after.stdout.contains("matched-prod"));
        assert!(!after.stdout.contains("prod_secret_value"));
        assert!(!after.stdout.contains("dev_secret_value"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn worker_denies_secret_ref_added_to_env_after_enqueue() {
        let (ws, _dir) = make_workspace("queued_env_ref_toctou");
        let envs = ws.envs_dir();
        fs::write(envs.join("prod.conf"), "TOKEN=initial_plaintext").unwrap();
        fs::write(envs.join("evil.conf"), "token=evil_secret").unwrap();

        write_schema_bash_stub(
            &ws,
            "secret_env.sh",
            r#"{"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}"#,
            r#"echo "$2""#,
        );
        let row = crate::operations::core::enqueue_run(
            &ws,
            crate::operations::core::EnqueueRunRequest {
                script: "secret_env.sh".into(),
                args: Vec::new(),
                env: Some("prod".into()),
                secret_fields: Vec::new(),
                run_id: Some("rid-toctou".into()),
                actor: "test".into(),
                reason: None,
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
            },
        )
        .unwrap();
        fs::write(envs.join("prod.conf"), "TOKEN=secret://evil/token").unwrap();

        worker_loop(
            ws.clone_for_executor(),
            "worker:test".into(),
            Arc::new(AtomicBool::new(false)),
            None,
            None,
            true,
        );

        let conn = runs::open(&ws).unwrap();
        let after = runs::get_run(&conn, &row.run_id).unwrap().unwrap();
        assert_eq!(after.state, RunState::Failed);
        assert!(!after.stdout.contains("evil_secret"));
        assert!(!after.error.unwrap_or_default().contains("evil_secret"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn worker_fails_provider_ref_run_when_secret_policy_is_missing() {
        let (ws, _dir) = make_workspace("missing_secret_policy");
        let envs = ws.envs_dir();
        fs::write(envs.join("prod.conf"), "TOKEN=policy_secret").unwrap();
        let marker = ws.root().join("policy_missing_marker");
        let script = write_schema_bash_stub(
            &ws,
            "secret_arg.sh",
            r#"{"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}"#,
            &format!("touch {}\necho \"$2\"", marker.display()),
        );
        let conn = runs::open(&ws).unwrap();
        let row = enqueue(
            &conn,
            script.to_str().unwrap(),
            &["--token".into(), "secret://prod/token".into()],
            EnqueueOptions {
                actor: "test".into(),
                omakure_version: "test".into(),
                allowed_secret_refs: Some(vec!["secret://prod/token".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        conn.execute(
            "DELETE FROM run_secret_refs WHERE run_id = ?",
            [&row.run_id],
        )
        .unwrap();
        drop(conn);

        worker_loop(
            ws.clone_for_executor(),
            "worker:test".into(),
            Arc::new(AtomicBool::new(false)),
            None,
            None,
            true,
        );

        let conn = runs::open(&ws).unwrap();
        let after = runs::get_run(&conn, &row.run_id).unwrap().unwrap();
        assert_eq!(after.state, RunState::Failed);
        assert!(after
            .error
            .unwrap_or_default()
            .contains("secret provider policy missing"));
        assert!(!after.stdout.contains("policy_secret"));
        assert!(!marker.exists());
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn worker_timeout_marks_row_timed_out() {
        let (ws, _dir) = make_workspace("once_timeout");
        let script = write_bash_stub(&ws, "sleep.sh", "sleep 5");
        let conn = runs::open(&ws).unwrap();
        let row = enqueue(
            &conn,
            script.to_str().unwrap(),
            &[],
            EnqueueOptions {
                timeout_ms: Some(500),
                actor: "test".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        let cancel_flag = Arc::new(AtomicBool::new(false));
        worker_loop(
            ws.clone_for_executor(),
            "worker:test".into(),
            cancel_flag,
            None,
            None,
            true,
        );

        let conn = runs::open(&ws).unwrap();
        let after = runs::get_run(&conn, &row.run_id).unwrap().unwrap();
        assert_eq!(after.state, RunState::TimedOut);
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn worker_runs_script_that_calls_omakure_trace_via_subprocess() {
        // End-to-end: a script launched by the worker calls
        // `omakure trace` in a subprocess. The trace verb reads
        // OMAKURE_RUN_ID + OMAKURE_SCRIPTS_DIR from its environment
        // and writes a row in run_traces under the same workspace.
        //
        // This test only runs when we can locate a built omakure
        // binary on disk; otherwise it skips silently. This avoids
        // requiring `cargo install` in CI.
        let omakure_bin = locate_omakure_binary();
        let Some(bin) = omakure_bin else { return };

        let (ws, _dir) = make_workspace("e2e_trace");
        let script_body = format!(
            "{} trace 'first' --level info\nsleep 0.2\n{} trace 'second' --level warn --data '{{\"k\":1}}'",
            bin.display(),
            bin.display()
        );
        let script = write_bash_stub(&ws, "trace_script.sh", &script_body);
        let conn = runs::open(&ws).unwrap();
        let row = enqueue(
            &conn,
            script.to_str().unwrap(),
            &[],
            EnqueueOptions {
                actor: "test".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        worker_loop(
            ws.clone_for_executor(),
            "worker:trace_e2e".into(),
            Arc::new(AtomicBool::new(false)),
            None,
            None,
            true,
        );

        let conn = runs::open(&ws).unwrap();
        let after = runs::get_run(&conn, &row.run_id).unwrap().unwrap();
        assert_eq!(after.state, RunState::Completed, "stderr: {}", after.stderr);
        let traces = runs::query_traces(&conn, &row.run_id, None, None).unwrap();
        assert_eq!(traces.len(), 2, "expected two traces, got {:?}", traces);
        assert_eq!(traces[0].sequence, 1);
        assert_eq!(traces[0].level, "info");
        assert_eq!(traces[1].sequence, 2);
        assert_eq!(traces[1].level, "warn");
        let _ = fs::remove_dir_all(ws.root());
    }

    #[cfg(unix)]
    fn locate_omakure_binary() -> Option<PathBuf> {
        // The test binary is at target/<profile>/deps/omakure-XXXX. The
        // omakure CLI binary lives one directory up at
        // target/<profile>/omakure.
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?.parent()?;
        let candidate = dir.join("omakure");
        if candidate.exists() {
            Some(candidate)
        } else {
            None
        }
    }

    #[test]
    #[cfg(unix)]
    fn worker_concurrency_two_drains_two_jobs_in_parallel() {
        let (ws, _dir) = make_workspace("once_two");
        let script = write_bash_stub(&ws, "ok.sh", "echo done");
        let conn = runs::open(&ws).unwrap();
        let r1 = enqueue(
            &conn,
            script.to_str().unwrap(),
            &[],
            EnqueueOptions {
                actor: "test".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let r2 = enqueue(
            &conn,
            script.to_str().unwrap(),
            &[],
            EnqueueOptions {
                actor: "test".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        // Two threads, each drains one job and exits.
        let mut handles = Vec::new();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        for i in 0..2 {
            let ws = ws.clone_for_executor();
            let cancel_flag = Arc::clone(&cancel_flag);
            handles.push(thread::spawn(move || {
                worker_loop(ws, format!("worker:t{}", i), cancel_flag, None, None, true);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let conn = runs::open(&ws).unwrap();
        let a1 = runs::get_run(&conn, &r1.run_id).unwrap().unwrap();
        let a2 = runs::get_run(&conn, &r2.run_id).unwrap().unwrap();
        assert_eq!(a1.state, RunState::Completed);
        assert_eq!(a2.state, RunState::Completed);
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn stats_prints_total_and_state_breakdown() {
        let (ws, _dir) = make_workspace("stats_human");
        // Stats works on an empty queue too.
        stats(&ws, false).unwrap();
        stats(&ws, true).unwrap();
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn cancel_queued_run_prints_cancelled_line() {
        let (ws, _dir) = make_workspace("cancel_queued");
        let script = write_bash_stub(&ws, "ok.sh", "echo done");
        let conn = runs::open(&ws).unwrap();
        let row = runs::enqueue(
            &conn,
            script.to_str().unwrap(),
            &[],
            EnqueueOptions {
                actor: "human".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);
        cancel(
            &ws,
            QueueCancelArgs {
                run_id: row.run_id.clone(),
                reason: Some("never mind".into()),
            },
            false,
        )
        .unwrap();
        let conn = runs::open(&ws).unwrap();
        let updated = runs::get_run(&conn, &row.run_id).unwrap().unwrap();
        assert_eq!(updated.state, RunState::Cancelled);
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn dead_letter_missing_run_returns_not_found() {
        let (ws, _dir) = make_workspace("dl_missing");
        let err = dead_letter(
            &ws,
            QueueDeadLetterArgs {
                run_id: "ghost".into(),
                reason: None,
            },
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("run not found"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn run_dispatches_subcommands() {
        let (ws, _dir) = make_workspace("run_dispatch");
        run(
            ws.root().to_path_buf(),
            QueueArgs {
                command: QueueCommand::Stats,
            },
            false,
        )
        .unwrap();
        run(
            ws.root().to_path_buf(),
            QueueArgs {
                command: QueueCommand::Stats,
            },
            true,
        )
        .unwrap();
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn worker_with_concurrency_and_once_drains_queue() {
        let (ws, _dir) = make_workspace("worker_dispatch");
        let script = write_bash_stub(&ws, "ok.sh", "echo done");
        let conn = runs::open(&ws).unwrap();
        for _ in 0..2 {
            runs::enqueue(
                &conn,
                script.to_str().unwrap(),
                &[],
                EnqueueOptions {
                    actor: "human".into(),
                    omakure_version: "test".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        drop(conn);
        worker(
            &ws,
            QueueWorkerArgs {
                concurrency: 2,
                actor_filter: None,
                script_filter: None,
                once: true,
            },
            false,
        )
        .unwrap();
        let _ = fs::remove_dir_all(ws.root());
    }
}

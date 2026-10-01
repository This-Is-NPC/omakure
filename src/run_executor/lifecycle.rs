use super::admission::{execution_script_path, parse_args_json, secret_access_for_row};
use super::environment::{push_reserved_run_env, write_redaction_file};
use super::pipes::{
    drain_channel, spawn_pipe_reader_to_channel, HEARTBEAT_TICK_MS, PIPE_DRAIN_BUDGET_MS,
};
use super::{CancelFlag, ExecutionResult, ExecutionTerminal};
use crate::adapters::script_runner::MultiScriptRunner;
use crate::runs::{self, RunCompletion, RunRow, RunState};
use crate::workspace::Workspace;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Drive a single script through the state machine: spawn it, heartbeat,
/// timeout, and react to external cancel. Caller is responsible for
/// having already inserted the row in `state='running'` (via
/// [`runs::start_inline`] or [`runs::claim_next`]).
///
/// On return, the row is **not yet** transitioned to its terminal state —
/// the caller maps the [`ExecutionResult::terminal`] into one of
/// [`runs::complete`], [`runs::fail`], [`runs::time_out`], or the cancel
/// finalization path.
pub fn execute_with_heartbeat(
    workspace: &Workspace,
    row: &RunRow,
    extra_env: Vec<(String, String)>,
    cancel: Option<CancelFlag>,
) -> ExecutionResult {
    execute_with_heartbeat_guarded(workspace, row, extra_env, cancel, None)
}

pub fn execute_with_heartbeat_guarded(
    workspace: &Workspace,
    row: &RunRow,
    extra_env: Vec<(String, String)>,
    cancel: Option<CancelFlag>,
    spawn_guard: Option<crate::remote_cue::ExecutionGuard>,
) -> ExecutionResult {
    // Resolve the script path. The row stores an absolute path; if the
    // file does not exist (e.g. it was deleted between enqueue and
    // claim), record an Errored result so the worker marks the row
    // failed instead of crashing the daemon.
    let script_path = match execution_script_path(workspace, row) {
        Ok(path) => path,
        Err(result) => return result,
    };

    // Validate the schema's required fields are satisfied (mirrors the
    // pre-PR-#8 `--no-prompt` behavior). The worker is always non-
    // interactive, so missing-required is a hard fail.
    let row_args = parse_args_json(&row.args_json);
    let secret_access = match secret_access_for_row(workspace, row, &row_args) {
        Ok(access) => access,
        Err(err) => {
            return ExecutionResult {
                terminal: ExecutionTerminal::Failed,
                completion: RunCompletion {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    success: false,
                    error: Some(err),
                },
            };
        }
    };
    let resolved_args = match crate::secrets::resolve_args_with_access(
        workspace,
        &script_path,
        &row_args,
        &extra_env,
        &[],
        &secret_access,
    ) {
        Ok(resolved) => resolved,
        Err((field, message)) => {
            return ExecutionResult {
                terminal: ExecutionTerminal::Failed,
                completion: RunCompletion {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    success: false,
                    error: Some(format!("required field `{}` missing: {}", field, message)),
                },
            };
        }
    };
    if let Err((field, message)) = crate::operations::core::check_required_fields(
        workspace,
        &script_path,
        &resolved_args.persisted_args,
    ) {
        return ExecutionResult {
            terminal: ExecutionTerminal::Failed,
            completion: RunCompletion {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                success: false,
                error: Some(format!("required field `{}` missing: {}", field, message)),
            },
        };
    }

    let args = resolved_args.execution_args.clone();
    // Env-injection precedence (`docs/internal/env-injection-spec.md` §1): the
    // caller-supplied `extra_env` (parent shell env is inherited by the
    // child; layer 2 active managed env; future layer 3 `--env-file`) is
    // seeded FIRST, then the reserved layer-4 vars are pushed AFTER it.
    // Because reserved vars are applied here, after env-file resolution, they
    // are not visible to `$VAR` expansion inside `.conf` / `--env-file` values.
    // `build_command` applies pairs in order via `cmd.env`, so the last
    // write of a key wins — the reserved keys below are therefore
    // NON-OVERRIDABLE: a user var of the same name in `extra_env` cannot
    // clobber them.
    let mut env = extra_env;
    let redaction_file = match write_redaction_file(workspace, &row.run_id, &resolved_args.secrets)
    {
        Ok(file) => file,
        Err(err) => {
            return ExecutionResult {
                terminal: ExecutionTerminal::Errored,
                completion: RunCompletion {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    success: false,
                    error: Some(err),
                },
            };
        }
    };
    if let Some(file) = &redaction_file {
        env.push((
            crate::secrets::REDACT_FILE_ENV.to_string(),
            file.path.to_string_lossy().to_string(),
        ));
    }
    push_reserved_run_env(&mut env, workspace, &row.run_id);

    let mut command = match MultiScriptRunner::build_command(&script_path, &args, &env) {
        Ok(cmd) => cmd,
        Err(err) => {
            return ExecutionResult {
                terminal: ExecutionTerminal::Errored,
                completion: RunCompletion {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    success: false,
                    error: Some(format!("build command failed: {}", err)),
                },
            };
        }
    };

    let child_result = command.spawn();
    drop(spawn_guard);
    let mut child = match child_result {
        Ok(c) => c,
        Err(err) => {
            return ExecutionResult {
                terminal: ExecutionTerminal::Errored,
                completion: RunCompletion {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    success: false,
                    error: Some(format!("spawn failed: {}", err)),
                },
            };
        }
    };

    // Pull stdout/stderr off threads so a child that prints a lot does
    // not block on its own pipe buffer. We use a channel + timed drain
    // (instead of join()) so a killed child whose orphaned grandchildren
    // keep its pipe open does not deadlock the executor.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (stdout_tx, stdout_rx) = channel::<String>();
    let (stderr_tx, stderr_rx) = channel::<String>();
    if let Some(h) = stdout {
        spawn_pipe_reader_to_channel(h, stdout_tx);
    }
    if let Some(h) = stderr {
        spawn_pipe_reader_to_channel(h, stderr_tx);
    }

    // Heartbeat thread: refresh the lease and check for external cancel
    // every HEARTBEAT_TICK milliseconds. The thread exits when the main
    // thread flips the local `done` flag.
    let done = Arc::new(AtomicBool::new(false));
    let cancelled_externally = Arc::new(AtomicBool::new(false));
    let stop_heartbeat = Arc::clone(&done);
    let cancelled_signal = Arc::clone(&cancelled_externally);
    let workspace_clone = workspace.clone_for_executor();
    let run_id_clone = row.run_id.clone();
    let worker_id_clone = row
        .worker_id
        .clone()
        .unwrap_or_else(|| "inline".to_string());
    let cancel_for_thread = cancel.clone();
    let heartbeat_handle = thread::spawn(move || {
        // The heartbeat tick is intentionally short relative to
        // HEARTBEAT_MS (60_000) so we react to cancel quickly. The
        // tick controls cancel-detection latency, not lease validity.
        let tick = Duration::from_millis(HEARTBEAT_TICK_MS);
        while !stop_heartbeat.load(Ordering::SeqCst) {
            if let Some(flag) = &cancel_for_thread {
                if flag.load(Ordering::SeqCst) {
                    cancelled_signal.store(true, Ordering::SeqCst);
                    break;
                }
            }
            if let Ok(conn) = runs::open(&workspace_clone) {
                match runs::heartbeat(&conn, &run_id_clone, &worker_id_clone) {
                    Ok(Some(RunState::Running)) => {}
                    Ok(_) => {
                        // Row is no longer ours (cancelled, or stolen,
                        // or already terminal). Tell the main thread to
                        // kill the child.
                        cancelled_signal.store(true, Ordering::SeqCst);
                        break;
                    }
                    Err(_) => {
                        // Transient SQLite error: keep going. The lease
                        // will eventually expire and another worker will
                        // pick up the row.
                    }
                }
            }
            thread::sleep(tick);
        }
    });

    // Per-job execution timeout watcher. Independent from the heartbeat
    // because the user-facing `--timeout` governs business-time, not
    // crash recovery.
    let started = Instant::now();
    let timeout = row
        .timeout_ms
        .map(|ms| Duration::from_millis(ms.max(0) as u64));
    let timed_out = Arc::new(AtomicBool::new(false));
    let mut killed = false;

    // Poll the child periodically. Cannot use `child.wait()` directly
    // because we need to interleave with the timeout / cancel checks.
    let outcome_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if cancelled_externally.load(Ordering::SeqCst) {
                    let _ = child.kill();
                    killed = true;
                    let status = child.wait();
                    break status.map_err(|e| e.to_string());
                }
                if let Some(t) = timeout {
                    if started.elapsed() >= t {
                        timed_out.store(true, Ordering::SeqCst);
                        let _ = child.kill();
                        killed = true;
                        let status = child.wait();
                        break status.map_err(|e| e.to_string());
                    }
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(err) => break Err(format!("wait failed: {}", err)),
        }
    };

    // Stop the heartbeat thread before transitioning state. This
    // ensures no straggler heartbeat overwrites the terminal state.
    done.store(true, Ordering::SeqCst);
    let _ = heartbeat_handle.join();

    // Drain pipe readers with a hard deadline. The reader threads
    // themselves are not joined: an orphaned grandchild process can
    // keep the pipe write end open indefinitely after the script's
    // direct child exits, which would otherwise deadlock the executor.
    let stdout_text = drain_channel(&stdout_rx, Duration::from_millis(PIPE_DRAIN_BUDGET_MS));
    let stderr_text = drain_channel(&stderr_rx, Duration::from_millis(PIPE_DRAIN_BUDGET_MS));

    let cancelled = cancelled_externally.load(Ordering::SeqCst);
    let timed_out = timed_out.load(Ordering::SeqCst);

    let (terminal, completion) = match outcome_status {
        Ok(status) => {
            let exit_code = status.code();
            let success = status.success();
            let terminal = if timed_out {
                ExecutionTerminal::TimedOut
            } else if cancelled {
                ExecutionTerminal::Cancelled
            } else if success {
                ExecutionTerminal::Completed
            } else {
                ExecutionTerminal::Failed
            };
            (
                terminal,
                RunCompletion {
                    stdout: crate::secrets::redact_text(&stdout_text, &resolved_args.secrets),
                    stderr: crate::secrets::redact_text(&stderr_text, &resolved_args.secrets),
                    exit_code,
                    success,
                    error: None,
                },
            )
        }
        Err(err) => (
            ExecutionTerminal::Errored,
            RunCompletion {
                stdout: crate::secrets::redact_text(&stdout_text, &resolved_args.secrets),
                stderr: crate::secrets::redact_text(&stderr_text, &resolved_args.secrets),
                exit_code: None,
                success: false,
                error: Some(crate::secrets::redact_text(&err, &resolved_args.secrets)),
            },
        ),
    };

    // Suppress unused warning when killed branch had no other side
    // effect besides forcing the wait above.
    let _ = killed;

    ExecutionResult {
        terminal,
        completion,
    }
}

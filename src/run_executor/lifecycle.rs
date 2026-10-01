use super::admission::{execution_script_path, parse_args_json, secret_access_for_row};
use super::environment::{push_reserved_run_env, write_redaction_file};
use super::pipes::{
    drain_channel, spawn_pipe_reader_to_channel, HEARTBEAT_TICK_MS, PIPE_DRAIN_BUDGET_MS,
};
use super::{CancelFlag, ExecutionResult, ExecutionTerminal};
use crate::adapters::script_runner::MultiScriptRunner;
use crate::runs::{self, RunCompletion, RunRow, RunState};
use crate::secrets::ResolvedArgs;
use crate::workspace::Workspace;
use std::io;
use std::path::PathBuf;
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

fn execution_error(terminal: ExecutionTerminal, error: String) -> ExecutionResult {
    ExecutionResult {
        terminal,
        completion: RunCompletion {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            success: false,
            error: Some(error),
        },
    }
}

fn resolve_run_args(
    workspace: &Workspace,
    row: &RunRow,
    extra_env: &[(String, String)],
) -> Result<(PathBuf, ResolvedArgs), ExecutionResult> {
    let script_path = execution_script_path(workspace, row)?;
    let row_args = parse_args_json(&row.args_json);
    let secret_access = secret_access_for_row(workspace, row, &row_args)
        .map_err(|error| execution_error(ExecutionTerminal::Failed, error))?;
    let resolved_args = crate::secrets::resolve_args_with_access(
        workspace,
        &script_path,
        &row_args,
        extra_env,
        &[],
        &secret_access,
    )
    .map_err(|(field, message)| {
        execution_error(
            ExecutionTerminal::Failed,
            format!("required field `{}` missing: {}", field, message),
        )
    })?;
    crate::operations::core::check_required_fields(
        workspace,
        &script_path,
        &resolved_args.persisted_args,
    )
    .map_err(|(field, message)| {
        execution_error(
            ExecutionTerminal::Failed,
            format!("required field `{}` missing: {}", field, message),
        )
    })?;
    Ok((script_path, resolved_args))
}

struct HeartbeatWatcher {
    done: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl HeartbeatWatcher {
    fn start(workspace: &Workspace, row: &RunRow, cancel: Option<CancelFlag>) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop_heartbeat = Arc::clone(&done);
        let cancelled_signal = Arc::clone(&cancelled);
        let workspace_clone = workspace.clone_for_executor();
        let run_id_clone = row.run_id.clone();
        let worker_id_clone = row
            .worker_id
            .clone()
            .unwrap_or_else(|| "inline".to_string());
        let handle = thread::spawn(move || {
            let tick = Duration::from_millis(HEARTBEAT_TICK_MS);
            while !stop_heartbeat.load(Ordering::SeqCst) {
                if let Some(flag) = &cancel {
                    if flag.load(Ordering::SeqCst) {
                        cancelled_signal.store(true, Ordering::SeqCst);
                        break;
                    }
                }
                if let Ok(conn) = runs::open(&workspace_clone) {
                    match runs::heartbeat(&conn, &run_id_clone, &worker_id_clone) {
                        Ok(Some(RunState::Running)) => {}
                        Ok(_) => {
                            cancelled_signal.store(true, Ordering::SeqCst);
                            break;
                        }
                        Err(_) => {}
                    }
                }
                thread::sleep(tick);
            }
        });
        Self {
            done,
            cancelled,
            handle,
        }
    }

    fn stop(self) -> bool {
        self.done.store(true, Ordering::SeqCst);
        let _ = self.handle.join();
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[derive(Debug, thiserror::Error)]
enum ChildWaitError {
    #[error("wait failed: {0}")]
    Poll(#[source] io::Error),
    #[error("{0}")]
    Reap(#[source] io::Error),
}

fn kill_and_wait(child: &mut Child) -> Result<ExitStatus, ChildWaitError> {
    let _ = child.kill();
    child.wait().map_err(ChildWaitError::Reap)
}

fn wait_for_child(
    child: &mut Child,
    cancelled: &AtomicBool,
    timeout_ms: Option<i64>,
) -> (Result<ExitStatus, ChildWaitError>, bool) {
    let started = Instant::now();
    let timeout = timeout_ms.map(|ms| Duration::from_millis(ms.max(0) as u64));
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return (Ok(status), false),
            Ok(None) => {
                if cancelled.load(Ordering::SeqCst) {
                    return (kill_and_wait(child), false);
                }
                if timeout.is_some_and(|limit| started.elapsed() >= limit) {
                    return (kill_and_wait(child), true);
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return (Err(ChildWaitError::Poll(error)), false),
        }
    }
}

fn classify_outcome(
    outcome: Result<ExitStatus, ChildWaitError>,
    cancelled: bool,
    timed_out: bool,
    stdout: &str,
    stderr: &str,
    secrets: &[String],
) -> ExecutionResult {
    let stdout = crate::secrets::redact_text(stdout, secrets);
    let stderr = crate::secrets::redact_text(stderr, secrets);
    match outcome {
        Ok(status) => {
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
            ExecutionResult {
                terminal,
                completion: RunCompletion {
                    stdout,
                    stderr,
                    exit_code: status.code(),
                    success,
                    error: None,
                },
            }
        }
        Err(error) => ExecutionResult {
            terminal: ExecutionTerminal::Errored,
            completion: RunCompletion {
                stdout,
                stderr,
                exit_code: None,
                success: false,
                error: Some(crate::secrets::redact_text(&error.to_string(), secrets)),
            },
        },
    }
}

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
    let (script_path, resolved_args) = match resolve_run_args(workspace, row, &extra_env) {
        Ok(prepared) => prepared,
        Err(result) => return result,
    };

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
        Err(err) => return execution_error(ExecutionTerminal::Errored, err.to_string()),
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
            return execution_error(
                ExecutionTerminal::Errored,
                format!("build command failed: {}", err),
            )
        }
    };

    let child_result = command.spawn();
    drop(spawn_guard);
    let mut child = match child_result {
        Ok(c) => c,
        Err(err) => {
            return execution_error(ExecutionTerminal::Errored, format!("spawn failed: {}", err))
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

    let heartbeat = HeartbeatWatcher::start(workspace, row, cancel);
    let (outcome_status, timed_out) =
        wait_for_child(&mut child, &heartbeat.cancelled, row.timeout_ms);
    let cancelled = heartbeat.stop();

    // Drain pipe readers with a hard deadline. The reader threads
    // themselves are not joined: an orphaned grandchild process can
    // keep the pipe write end open indefinitely after the script's
    // direct child exits, which would otherwise deadlock the executor.
    let stdout_text = drain_channel(&stdout_rx, Duration::from_millis(PIPE_DRAIN_BUDGET_MS));
    let stderr_text = drain_channel(&stderr_rx, Duration::from_millis(PIPE_DRAIN_BUDGET_MS));

    classify_outcome(
        outcome_status,
        cancelled,
        timed_out,
        &stdout_text,
        &stderr_text,
        &resolved_args.secrets,
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Command;

    fn externally_reaped_child() -> Child {
        let child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let mut status = 0;
        // SAFETY: this process owns the child PID, and status points to valid storage.
        let reaped = unsafe { libc::waitpid(child.id() as libc::pid_t, &mut status, 0) };
        assert_eq!(reaped, child.id() as libc::pid_t);
        child
    }

    #[test]
    fn child_wait_errors_preserve_text_and_override_timeout_or_cancellation() {
        let mut child = externally_reaped_child();
        let cancelled = AtomicBool::new(false);
        let (outcome, timed_out) = wait_for_child(&mut child, &cancelled, None);
        assert!(!timed_out);
        let error = outcome.unwrap_err();
        assert!(matches!(error, ChildWaitError::Poll(_)));
        let expected = error.to_string();
        assert!(expected.starts_with("wait failed: "));
        let result = classify_outcome(Err(error), true, true, "", "", &[]);
        assert_eq!(result.terminal, ExecutionTerminal::Errored);
        assert_eq!(result.completion.error.as_deref(), Some(expected.as_str()));

        let mut child = externally_reaped_child();
        let error = kill_and_wait(&mut child).unwrap_err();
        assert!(matches!(error, ChildWaitError::Reap(_)));
        let expected = error.to_string();
        assert!(!expected.starts_with("wait failed: "));
        let result = classify_outcome(Err(error), true, true, "", "", &[]);
        assert_eq!(result.terminal, ExecutionTerminal::Errored);
        assert_eq!(result.completion.error.as_deref(), Some(expected.as_str()));
    }
}

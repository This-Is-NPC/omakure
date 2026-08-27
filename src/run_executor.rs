//! Shared execution helper used by both `omakure run` (synchronous fast
//! path) and `omakure queue worker` (daemon draining the queue).
//!
//! `execute_with_heartbeat` owns the entire lifecycle of a single child
//! process: spawning it with the supplied environment, refreshing the
//! lease in `runs.sqlite` periodically, optionally killing it after a
//! per-job timeout, and reacting to mid-execution cancel by polling the
//! heartbeat call's return value.
//!
//! There is exactly one execution code path; the worker's loop and
//! `omakure run` both call this function so the two surfaces never drift.

use crate::adapters::script_runner::MultiScriptRunner;
use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::ports::ScriptRepository;
use crate::runs::{self, RunCompletion, RunRow, RunState, RunTrigger};
use crate::workspace::Workspace;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Outcome of one [`execute_with_heartbeat`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionTerminal {
    /// The script exited cleanly with `success = true`.
    Completed,
    /// The script exited with a non-zero code or `success = false`.
    Failed,
    /// The watcher killed the script for exceeding `timeout_ms`.
    TimedOut,
    /// The script was killed because the row was cancelled externally,
    /// or the worker was asked to shut down before the script finished.
    Cancelled,
    /// The runner failed to spawn the child or hit an unrecoverable error.
    Errored,
}

/// Captured output and the terminal classification produced by one run.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub terminal: ExecutionTerminal,
    pub completion: RunCompletion,
}

/// Optional cancel flag shared by `omakure queue worker`'s SIGINT handler.
/// `omakure run` does not use it (it passes `None`).
pub type CancelFlag = Arc<AtomicBool>;

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
    // Resolve the script path. The row stores an absolute path; if the
    // file does not exist (e.g. it was deleted between enqueue and
    // claim), record an Errored result so the worker marks the row
    // failed instead of crashing the daemon.
    let script_path = PathBuf::from(&row.script_path);
    if !script_path.exists() {
        return ExecutionResult {
            terminal: ExecutionTerminal::Errored,
            completion: RunCompletion {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                success: false,
                error: Some(format!("script not found: {}", row.script_path)),
            },
        };
    }

    if let Err(error) = check_cue_script_unchanged(workspace, row, &script_path) {
        return ExecutionResult {
            terminal: ExecutionTerminal::Failed,
            completion: RunCompletion {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                success: false,
                error: Some(error),
            },
        };
    }

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
    let persisted_args_json = serde_json::to_string(&resolved_args.persisted_args)
        .unwrap_or_else(|_| row.args_json.clone());
    if let Err((field, message)) =
        check_required_fields(workspace, &script_path, &persisted_args_json)
    {
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
    // Env-injection precedence (`.docs/env-injection-spec.md` §1): the
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
    env.push(("OMAKURE_RUN_ID".to_string(), row.run_id.clone()));
    // Pin the workspace so nested `omakure trace` invocations write to
    // the same `runs.sqlite` even when the worker was launched against
    // a non-default scripts dir (or a temp dir under `--scripts-dir`).
    env.push((
        "OMAKURE_SCRIPTS_DIR".to_string(),
        workspace.root().to_string_lossy().to_string(),
    ));

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

    let mut child = match command.spawn() {
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

/// Refuse a Cue-origin run whose script is no longer the script it was
/// authorized against.
///
/// The Remote Cue contract declined this third check on the grounds that it
/// only defended against an attacker who could already write to the workspace.
/// A baseline push makes that premise false: a signed baseline replaces scripts
/// legitimately, so a Cue accepted against version N can reach the executor
/// with version N+1 on disk and nobody hostile anywhere in the story.
///
/// Fail-closed in all three directions. A missing hash row, a failed lookup,
/// and an unreadable script each refuse, because none of them is evidence that
/// the bytes are the authorized ones — and the run_secret_refs precedent, where
/// "missing" and "lookup failed" both meant allow-all, is exactly the shape of
/// mistake this must not repeat.
///
/// Scoped to `RunTrigger::Cue`. A manual or scheduled run is started by someone
/// on this machine against whatever is on this machine; there is no earlier
/// authorization for it to have drifted from.
fn check_cue_script_unchanged(
    workspace: &Workspace,
    row: &RunRow,
    script_path: &Path,
) -> Result<(), String> {
    if row.trigger != RunTrigger::Cue {
        return Ok(());
    }
    let recorded = runs::open(workspace)
        .and_then(|conn| runs::get_run_script_hash(&conn, &row.run_id))
        .map_err(|err| format!("authorized script content lookup failed: {err}"))?
        .ok_or_else(|| {
            "no authorized script content was recorded for this remote run".to_string()
        })?;
    match crate::remote_cue::content_hash(script_path) {
        Some(current) if current == recorded => Ok(()),
        Some(_) => Err(
            "the script changed after this remote run was authorized; it was not executed"
                .to_string(),
        ),
        None => Err("the authorized script could not be read at execution time".to_string()),
    }
}

fn secret_access_for_row(
    workspace: &Workspace,
    row: &RunRow,
    args: &[String],
) -> Result<crate::secrets::SecretAccess, String> {
    let has_provider_ref = args.iter().any(|arg| {
        arg.starts_with("secret://")
            || arg
                .split_once('=')
                .map(|(_, value)| value.starts_with("secret://"))
                .unwrap_or(false)
    });
    let refs = match runs::open(workspace)
        .and_then(|conn| runs::get_run_secret_refs(&conn, &row.run_id))
    {
        Ok(Some(refs)) => refs,
        Ok(None) if has_provider_ref => {
            return Err("secret provider policy missing for queued run".to_string())
        }
        Ok(None) => return Ok(crate::secrets::SecretAccess::allow_all()),
        Err(err) if has_provider_ref => {
            return Err(format!("secret provider policy lookup failed: {err}"))
        }
        Err(_) => return Ok(crate::secrets::SecretAccess::allow_all()),
    };
    if refs
        .iter()
        .any(|secret_ref| secret_ref == runs::ALLOW_ALL_SECRET_REFS_POLICY)
    {
        Ok(crate::secrets::SecretAccess::allow_all())
    } else {
        Ok(crate::secrets::SecretAccess::new(["secrets:use"], refs))
    }
}

struct RedactionFile {
    path: PathBuf,
}

impl Drop for RedactionFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn write_redaction_file(
    workspace: &Workspace,
    run_id: &str,
    secrets: &[String],
) -> Result<Option<RedactionFile>, String> {
    let Some(value) = crate::secrets::secrets_env_value(secrets) else {
        return Ok(None);
    };
    fs::create_dir_all(workspace.history_dir())
        .map_err(|err| format!("create redaction dir failed: {err}"))?;
    let path = workspace.history_dir().join(format!(
        ".redact.{}.{}.tmp",
        sanitize_run_id_for_filename(run_id),
        runs::current_unix_ms()
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(&path)
        .map_err(|err| format!("open redaction file failed: {err}"))?;
    file.write_all(value.as_bytes())
        .map_err(|err| format!("write redaction file failed: {err}"))?;
    Ok(Some(RedactionFile { path }))
}

fn sanitize_run_id_for_filename(run_id: &str) -> String {
    run_id
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

/// Heartbeat tick interval. Short enough to detect external cancel
/// quickly, but long enough not to thrash SQLite.
const HEARTBEAT_TICK_MS: u64 = 250;

/// Maximum time we wait for a pipe to flush after the child has exited.
/// Bounded so a leaked grandchild process holding the pipe write end
/// open cannot deadlock the executor.
const PIPE_DRAIN_BUDGET_MS: u64 = 200;

fn spawn_pipe_reader_to_channel<R: Read + Send + 'static>(handle: R, tx: Sender<String>) {
    thread::spawn(move || {
        let reader = BufReader::new(handle);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                // Receiver dropped — main path moved on; abandon the
                // reader. The thread terminates when the pipe closes,
                // which on a clean exit is immediate and on an orphan
                // happens whenever the orphan eventually exits.
                break;
            }
        }
    });
}

fn drain_channel(rx: &std::sync::mpsc::Receiver<String>, budget: Duration) -> String {
    let deadline = Instant::now() + budget;
    let mut out = String::new();
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                out.push_str(&line);
                out.push('\n');
            }
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    out
}

fn parse_args_json(args_json: &str) -> Vec<String> {
    serde_json::from_str(args_json).unwrap_or_else(|_| Vec::new())
}

fn check_required_fields(
    workspace: &Workspace,
    script: &Path,
    args_json: &str,
) -> Result<(), (String, String)> {
    let repo = FsWorkspaceRepository::new(workspace.root().to_path_buf());
    let schema = match repo.read_schema(script) {
        Ok(s) => s,
        Err(_) => return Ok(()), // permissive when no schema
    };
    let args: Vec<String> = serde_json::from_str(args_json).unwrap_or_default();
    for field in &schema.fields {
        if !field.required.unwrap_or(false) {
            continue;
        }
        let arg_flag = field
            .arg
            .clone()
            .unwrap_or_else(|| format!("--{}", field.name));
        let present = args
            .iter()
            .any(|a| a == &arg_flag || a.starts_with(&format!("{}=", arg_flag)));
        if !present {
            return Err((
                field.name.clone(),
                format!("expected `{}` on the command line", arg_flag),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::environments::resolve_run_env;
    use crate::runs::EnqueueOptions;
    use std::fs;

    fn make_workspace(label: &str) -> Workspace {
        let dir = std::env::temp_dir().join(format!(
            "omakure_executor_test_{}_{}_{}",
            label,
            std::process::id(),
            runs::current_unix_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let ws = Workspace::new(dir);
        ws.ensure_layout().unwrap();
        ws
    }

    fn write_bash_stub(workspace: &Workspace, name: &str, body: &str) -> PathBuf {
        let p = workspace.root().join(name);
        fs::write(&p, format!("#!/usr/bin/env bash\n{}\n", body)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&p).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&p, perms).unwrap();
        }
        p
    }

    /// A Cue authorized one script; a baseline may legitimately replace it
    /// before the worker claims the row. The bytes that run must be the bytes
    /// that were authorized.
    #[test]
    #[cfg(unix)]
    fn a_cue_run_refuses_a_script_that_changed_after_it_was_authorized() {
        let ws = make_workspace("cue_swapped_script");
        let script = write_bash_stub(&ws, "deploy.sh", "echo authorized");
        let authorized = crate::remote_cue::content_hash(&script).unwrap();
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                actor: "conductor".into(),
                omakure_version: "test".into(),
                trigger: RunTrigger::Cue,
                script_content_hash: Some(authorized),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        // The control: unchanged bytes still run, so the refusal below is
        // about the swap and not about Cue-origin runs being blocked outright.
        let unchanged = execute_with_heartbeat(&ws, &row, vec![], None);
        assert_eq!(unchanged.terminal, ExecutionTerminal::Completed);
        assert!(unchanged.completion.stdout.contains("authorized"));

        write_bash_stub(&ws, "deploy.sh", "echo substituted");
        let swapped = execute_with_heartbeat(&ws, &row, vec![], None);

        assert_eq!(swapped.terminal, ExecutionTerminal::Failed);
        assert!(
            !swapped.completion.stdout.contains("substituted"),
            "the substituted script must not have run at all, got: {:?}",
            swapped.completion.stdout
        );
        assert!(swapped
            .completion
            .error
            .unwrap_or_default()
            .contains("changed after this remote run was authorized"));
        let _ = fs::remove_dir_all(ws.root());
    }

    /// The `run_secret_refs` lesson: "no record" must not read as "no
    /// constraint". A Cue-origin row without a recorded hash is a row whose
    /// authorization cannot be checked, and running it would make the whole
    /// binding optional for anyone who could delete one table row.
    #[test]
    #[cfg(unix)]
    fn a_cue_run_with_no_recorded_hash_does_not_execute() {
        let ws = make_workspace("cue_missing_hash");
        let script = write_bash_stub(&ws, "deploy.sh", "echo unconstrained");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                actor: "conductor".into(),
                omakure_version: "test".into(),
                trigger: RunTrigger::Cue,
                script_content_hash: None,
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        let result = execute_with_heartbeat(&ws, &row, vec![], None);

        assert_eq!(result.terminal, ExecutionTerminal::Failed);
        assert!(
            !result.completion.stdout.contains("unconstrained"),
            "an unconstrained remote run must not reach the child process"
        );
        let _ = fs::remove_dir_all(ws.root());
    }

    /// The check is scoped to remote runs. A run someone started on this
    /// machine has no earlier authorization to have drifted from, and
    /// requiring a hash for it would break every local path.
    #[test]
    #[cfg(unix)]
    fn a_manual_run_is_unaffected_by_the_authorized_content_check() {
        let ws = make_workspace("manual_unaffected");
        let script = write_bash_stub(&ws, "local.sh", "echo local");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                actor: "human".into(),
                omakure_version: "test".into(),
                trigger: RunTrigger::Manual,
                script_content_hash: None,
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        write_bash_stub(&ws, "local.sh", "echo edited");
        let result = execute_with_heartbeat(&ws, &row, vec![], None);

        assert_eq!(result.terminal, ExecutionTerminal::Completed);
        assert!(result.completion.stdout.contains("edited"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn execute_completes_simple_script() {
        let ws = make_workspace("complete_simple");
        let script = write_bash_stub(&ws, "ok.sh", "echo hello");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                actor: "human".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        let result = execute_with_heartbeat(&ws, &row, vec![], None);
        assert_eq!(result.terminal, ExecutionTerminal::Completed);
        assert!(result.completion.stdout.contains("hello"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn execute_injects_omakure_scripts_dir_env_var() {
        let ws = make_workspace("scripts_dir_env");
        let script = write_bash_stub(&ws, "echodir.sh", "echo $OMAKURE_SCRIPTS_DIR");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                actor: "human".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);
        let result = execute_with_heartbeat(&ws, &row, vec![], None);
        assert_eq!(result.terminal, ExecutionTerminal::Completed);
        assert!(
            result
                .completion
                .stdout
                .trim()
                .ends_with(ws.root().to_string_lossy().as_ref()),
            "expected stdout to end with workspace root, got: {:?}",
            result.completion.stdout
        );
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn execute_uses_redaction_file_instead_of_plaintext_secret_env() {
        let ws = make_workspace("redaction_file_env");
        let script = write_bash_stub(
            &ws,
            "redact-env.sh",
            r#"# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
if [ -n "$OMAKURE_REDACT_SECRETS" ]; then
  echo raw-redaction-env-present
  exit 2
fi
test -n "$OMAKURE_REDACT_SECRETS_FILE"
test -f "$OMAKURE_REDACT_SECRETS_FILE"
printf '%s\n' "$OMAKURE_REDACT_SECRETS_FILE"
"#,
        );
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                actor: "human".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        let result = execute_with_heartbeat(
            &ws,
            &row,
            vec![("TOKEN".into(), "redaction-file-secret".into())],
            None,
        );

        assert_eq!(result.terminal, ExecutionTerminal::Completed);
        assert!(!result.completion.stdout.contains("redaction-file-secret"));
        let redaction_file = result.completion.stdout.trim();
        assert!(!redaction_file.is_empty());
        assert!(!PathBuf::from(redaction_file).exists());
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn execute_injects_omakure_run_id_env_var() {
        let ws = make_workspace("env_var");
        let script = write_bash_stub(&ws, "echoid.sh", "echo $OMAKURE_RUN_ID");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                actor: "human".into(),
                omakure_version: "test".into(),
                run_id: Some("rid-fixed".into()),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);
        let result = execute_with_heartbeat(&ws, &row, vec![], None);
        assert_eq!(result.terminal, ExecutionTerminal::Completed);
        assert!(
            result.completion.stdout.contains("rid-fixed"),
            "expected stdout to contain rid-fixed, got: {:?}",
            result.completion.stdout
        );
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn execute_reserved_vars_win_over_injected_extra_env() {
        // Precedence spec §1 layer 4: reserved vars are pushed AFTER
        // extra_env, so a user attempt to override OMAKURE_RUN_ID via the
        // injected env must lose (non-overridable).
        let ws = make_workspace("reserved_wins");
        let script = write_bash_stub(&ws, "echoid.sh", "echo $OMAKURE_RUN_ID");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                omakure_version: "test".into(),
                run_id: Some("rid-reserved".into()),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        let extra_env = vec![("OMAKURE_RUN_ID".to_string(), "HIJACKED".to_string())];
        let result = execute_with_heartbeat(&ws, &row, extra_env, None);

        assert_eq!(result.terminal, ExecutionTerminal::Completed);
        assert!(
            result.completion.stdout.contains("rid-reserved"),
            "reserved run id should survive, got: {:?}",
            result.completion.stdout
        );
        assert!(
            !result.completion.stdout.contains("HIJACKED"),
            "injected value must not override reserved var"
        );
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn execute_reserved_scripts_dir_wins_over_injected_extra_env() {
        // Precedence spec §1 layer 4: OMAKURE_SCRIPTS_DIR is reserved just
        // like OMAKURE_RUN_ID and must be the final value observed by the
        // child, even if extra_env tries to hijack it.
        let ws = make_workspace("reserved_scripts_dir_wins");
        let script = write_bash_stub(&ws, "echodir.sh", "echo $OMAKURE_SCRIPTS_DIR");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        let extra_env = vec![(
            "OMAKURE_SCRIPTS_DIR".to_string(),
            "/tmp/hijacked-omakure".to_string(),
        )];
        let result = execute_with_heartbeat(&ws, &row, extra_env, None);

        assert_eq!(result.terminal, ExecutionTerminal::Completed);
        assert_eq!(result.completion.stdout.trim(), ws.root().to_string_lossy());
        assert!(!result.completion.stdout.contains("hijacked"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn execute_reserved_vars_are_not_expandable_in_resolved_env() {
        // Reserved vars are applied by execute_with_heartbeat after
        // resolve_run_env has expanded user layers, so references to them in
        // active env files expand as undefined. The reserved var itself is
        // still injected afterward and visible to the child.
        let ws = make_workspace("reserved_not_expandable");
        let envs = ws.envs_dir();
        fs::create_dir_all(envs).unwrap();
        fs::write(envs.join("dev.conf"), "PLAIN=$OMAKURE_RUN_ID\n").unwrap();
        fs::write(envs.join("active"), "dev.conf\n").unwrap();
        let script = write_bash_stub(&ws, "echoenv.sh", "echo \"${PLAIN}|${OMAKURE_RUN_ID}\"");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                omakure_version: "test".into(),
                run_id: Some("rid-real-pipeline".into()),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        let extra_env = resolve_run_env(envs, None).unwrap();
        let result = execute_with_heartbeat(&ws, &row, extra_env, None);

        assert_eq!(result.terminal, ExecutionTerminal::Completed);
        assert_eq!(result.completion.stdout.trim(), "|rid-real-pipeline");
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn execute_failed_script_marked_failed() {
        let ws = make_workspace("failed");
        let script = write_bash_stub(&ws, "bad.sh", "exit 7");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);
        let result = execute_with_heartbeat(&ws, &row, vec![], None);
        assert_eq!(result.terminal, ExecutionTerminal::Failed);
        assert_eq!(result.completion.exit_code, Some(7));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn execute_timeout_kills_long_script() {
        let ws = make_workspace("timeout");
        let script = write_bash_stub(&ws, "sleep.sh", "sleep 5");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                timeout_ms: Some(500),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);
        let started = Instant::now();
        let result = execute_with_heartbeat(&ws, &row, vec![], None);
        assert_eq!(result.terminal, ExecutionTerminal::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn execute_returns_errored_when_script_missing() {
        let ws = make_workspace("missing_script");
        let conn = runs::open(&ws).unwrap();
        // Enqueue a row pointing at a path that does not exist.
        let bogus = ws.root().join("does_not_exist.sh");
        let row = runs::start_inline(
            &conn,
            bogus.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        let result = execute_with_heartbeat(&ws, &row, vec![], None);
        assert_eq!(result.terminal, ExecutionTerminal::Errored);
        assert!(result
            .completion
            .error
            .as_deref()
            .unwrap_or("")
            .contains("script not found"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn execute_returns_errored_for_unsupported_extension() {
        let ws = make_workspace("unsupported_ext");
        let script = ws.root().join("plain.txt");
        fs::write(&script, "not a script").unwrap();
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        let result = execute_with_heartbeat(&ws, &row, vec![], None);
        assert_eq!(result.terminal, ExecutionTerminal::Errored);
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn execute_fails_when_required_field_missing() {
        let ws = make_workspace("missing_required");
        let script = write_bash_stub(
            &ws,
            "needs.sh",
            r#"# placeholder
echo done"#,
        );
        // Inject a schema block at the top of the script declaring a
        // required `--name` field.
        let body = "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {\"Name\": \"x\", \"Fields\": [{\"Name\": \"name\", \"Type\": \"string\", \"Order\": 1, \"Required\": true}]}\n# OMAKURE_SCHEMA_END\necho done\n";
        fs::write(&script, body).unwrap();
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        let result = execute_with_heartbeat(&ws, &row, vec![], None);
        assert_eq!(result.terminal, ExecutionTerminal::Failed);
        assert!(result
            .completion
            .error
            .as_deref()
            .unwrap_or("")
            .contains("required field"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn check_required_fields_passes_when_arg_present() {
        let ws = make_workspace("required_present");
        let script = ws.root().join("ok.sh");
        let body = "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {\"Name\": \"x\", \"Fields\": [{\"Name\": \"name\", \"Type\": \"string\", \"Order\": 1, \"Required\": true}]}\n# OMAKURE_SCHEMA_END\necho done\n";
        fs::write(&script, body).unwrap();

        let args_json = serde_json::to_string(&vec!["--name=alice"]).unwrap();
        let res = check_required_fields(&ws, &script, &args_json);
        assert!(res.is_ok());

        // Optional field absent — also OK.
        let opt_body = "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {\"Name\": \"x\", \"Fields\": [{\"Name\": \"opt\", \"Type\": \"string\", \"Order\": 1}]}\n# OMAKURE_SCHEMA_END\n";
        fs::write(&script, opt_body).unwrap();
        assert!(check_required_fields(&ws, &script, "[]").is_ok());

        // Schema absent — permissive.
        let bare = "#!/usr/bin/env bash\necho hi\n";
        fs::write(&script, bare).unwrap();
        assert!(check_required_fields(&ws, &script, "[]").is_ok());

        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    #[cfg(unix)]
    fn execute_resolves_persisted_secret_ref_at_runtime_and_redacts_output() {
        let ws = make_workspace("secret_ref_runtime");
        fs::write(
            ws.envs_dir().join("prod.conf"),
            "TOKEN=from_file_provider\n",
        )
        .unwrap();
        let script = write_bash_stub(&ws, "secret_ref.sh", "");
        let body = r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"SecretRef","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
if [ "$1" = "--token=from_file_provider" ]; then echo "matched from_file_provider"; else echo "leaked:$1"; exit 7; fi
"#;
        fs::write(&script, body).unwrap();
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &["--token=secret://prod/token".to_string()],
            "inline:test",
            EnqueueOptions {
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        let result = execute_with_heartbeat(&ws, &row, vec![], None);

        assert_eq!(result.terminal, ExecutionTerminal::Completed);
        assert!(result.completion.stdout.contains("matched <redacted>"));
        assert!(!result.completion.stdout.contains("from_file_provider"));
        assert!(!result.completion.stderr.contains("from_file_provider"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn parse_args_json_handles_invalid_input() {
        assert!(parse_args_json("not valid json").is_empty());
        assert_eq!(parse_args_json("[\"a\",\"b\"]"), vec!["a", "b"]);
    }

    #[test]
    #[cfg(unix)]
    fn execute_external_cancel_kills_running_script() {
        let ws = make_workspace("cancel");
        let script = write_bash_stub(&ws, "sleep.sh", "sleep 10");
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            script.to_str().unwrap(),
            &[],
            "inline:test",
            EnqueueOptions {
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        drop(conn);

        // Spawn a thread that flips the row to cancelled after a delay.
        let ws_thread = ws.clone_for_executor();
        let id = row.run_id.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(400));
            let conn = runs::open(&ws_thread).unwrap();
            runs::cancel(&conn, &id, Some("user".into()), None).unwrap();
        });

        let started = Instant::now();
        let result = execute_with_heartbeat(&ws, &row, vec![], None);
        canceller.join().unwrap();
        assert_eq!(result.terminal, ExecutionTerminal::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(8));
        let _ = fs::remove_dir_all(ws.root());
    }
}

//! `omakure serve` — cron scheduler daemon.
//!
//! Scans the scripts workspace for scripts whose embedded schema declares a
//! `Schedule` block, computes next fire times, and enqueues runs through
//! the shared `runs::enqueue` state machine with `trigger = Scheduled`.
//!
//! Execution is delegated to the queue worker. By default `serve` also
//! spawns an in-process worker so a single invocation is self-sufficient;
//! pass `--no-worker` when a dedicated `omakure queue worker` is already
//! running.
//!
//! A single PID file at `<workspace>/.omakure/daemon.pid` guards against
//! concurrent daemons. Structured events are appended to
//! `<workspace>/.omakure/daemon.log`.

use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::app_meta;
use crate::cli::args::ServeArgs;
use crate::cli::json::{self, codes};
#[cfg(windows)]
use crate::cli::serve_windows::{self, OpenEventError, ProcessProbe, StopEvent};
use crate::domain::{next_fire_after, parse_cron};
use crate::ports::ScriptRepository;
use crate::runs::{self, EnqueueOptions, RunTrigger};
use crate::secrets;
use crate::workspace::Workspace;
use chrono::Utc;
use cron::Schedule as CronSchedule;
use serde_json::json;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const SCAN_INTERVAL: Duration = Duration::from_secs(5);
const STOP_GRACE: Duration = Duration::from_secs(5);

pub fn run(scripts_dir: PathBuf, args: ServeArgs, json_output: bool) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;

    if args.install {
        return crate::cli::serve_autostart::install(&workspace, json_output);
    }
    if args.uninstall {
        return crate::cli::serve_autostart::uninstall(&workspace, json_output);
    }
    if args.status {
        return crate::cli::serve_autostart::status(&workspace, json_output);
    }

    if args.stop {
        return stop(&workspace, json_output);
    }

    if args.detach {
        return detach_and_run(workspace, args, json_output);
    }

    run_foreground(workspace, args, json_output)
}

fn pid_file(workspace: &Workspace) -> PathBuf {
    workspace.omakure_dir().join("daemon.pid")
}

fn log_file(workspace: &Workspace) -> PathBuf {
    workspace.omakure_dir().join("daemon.log")
}

// ---------------------------------------------------------------------------
// Lock file
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn acquire_lock(workspace: &Workspace) -> Result<(), String> {
    let path = pid_file(workspace);
    if path.exists() {
        match read_pid(&path) {
            Some(pid) if process_alive(pid) => {
                return Err(format!(
                    "daemon already running (pid {pid}, lock file {})",
                    path.display()
                ));
            }
            _ => {
                let _ = fs::remove_file(&path);
            }
        }
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("create {}: {e}", path.display()))?;
    writeln!(file, "{}", std::process::id())
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsPidFile {
    pid: u32,
    stop_event: String,
}

#[cfg(windows)]
struct WindowsLock {
    identity: WindowsPidFile,
    stop_event: StopEvent,
}

#[cfg(windows)]
fn read_windows_pid_file(path: &Path) -> Result<WindowsPidFile, String> {
    let contents =
        fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let mut lines = contents.lines();
    let pid = lines
        .next()
        .ok_or_else(|| format!("{} is empty", path.display()))?
        .trim()
        .parse::<u32>()
        .map_err(|error| format!("invalid PID in {}: {error}", path.display()))?;
    let stop_event = lines
        .next()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("{} has no stop-event identity", path.display()))?
        .to_string();
    if !serve_windows::is_stop_event_name(&stop_event) {
        return Err(format!("invalid stop-event identity in {}", path.display()));
    }
    Ok(WindowsPidFile { pid, stop_event })
}

#[cfg(windows)]
fn remove_windows_pid_file_if_current(path: &Path, expected: &WindowsPidFile) {
    if let Ok(current) = read_windows_pid_file(path) {
        if &current != expected {
            return;
        }
        let _ = fs::remove_file(path);
    }
}

#[cfg(windows)]
fn publish_windows_pid_file(path: &Path, identity: &WindowsPidFile) -> Result<(), String> {
    let token = identity
        .stop_event
        .rsplit('-')
        .next()
        .ok_or_else(|| "stop-event identity has no publication token".to_string())?;
    let temp_path = path.with_file_name(format!("daemon.pid.{token}.tmp"));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| format!("create {}: {error}", temp_path.display()))?;
        writeln!(file, "{}\n{}", identity.pid, identity.stop_event)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("flush {}: {error}", temp_path.display()))?;
        drop(file);
        serve_windows::publish_exclusive(&temp_path, path)
            .map_err(|error| format!("publish {}: {error}", path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[cfg(windows)]
fn acquire_lock(workspace: &Workspace) -> Result<WindowsLock, String> {
    let path = pid_file(workspace);
    if path.exists() {
        let existing = read_windows_pid_file(&path)?;
        match serve_windows::probe_process(existing.pid) {
            ProcessProbe::Live(_process) => {
                match serve_windows::open_stop_event(&existing.stop_event) {
                    Ok(_event) => {
                        return Err(format!(
                            "daemon already running (pid {}, lock file {})",
                            existing.pid,
                            path.display()
                        ));
                    }
                    Err(OpenEventError::NotFound) => {
                        return Err(format!(
                            "daemon pid {} is live but its stop event is unavailable; \
                             refusing to reclaim {}",
                            existing.pid,
                            path.display()
                        ));
                    }
                    Err(OpenEventError::Indeterminate(error)) => {
                        return Err(format!(
                            "cannot verify daemon pid {}: {error}; refusing to reclaim {}",
                            existing.pid,
                            path.display()
                        ));
                    }
                }
            }
            ProcessProbe::Dead => {
                remove_windows_pid_file_if_current(&path, &existing);
            }
            ProcessProbe::Indeterminate(error) => {
                return Err(format!(
                    "cannot determine whether daemon pid {} is live: {error}; refusing to reclaim {}",
                    existing.pid,
                    path.display()
                ));
            }
        }
    }

    let (stop_event_name, stop_event) = serve_windows::create_stop_event()?;
    let identity = WindowsPidFile {
        pid: std::process::id(),
        stop_event: stop_event_name,
    };
    if let Err(error) = publish_windows_pid_file(&path, &identity) {
        return Err(error);
    }
    Ok(WindowsLock {
        identity,
        stop_event,
    })
}

#[cfg(unix)]
fn release_lock(workspace: &Workspace) {
    let _ = fs::remove_file(pid_file(workspace));
}

#[cfg(windows)]
fn release_lock(workspace: &Workspace, expected: &WindowsPidFile) {
    remove_windows_pid_file_if_current(&pid_file(workspace), expected);
}

#[cfg(unix)]
fn read_pid(path: &Path) -> Option<u32> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // `kill -0` never delivers a signal; it only checks existence + permission.
    unsafe { libc_kill(pid as i32, 0) == 0 }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(unix)]
fn send_sigterm(pid: u32) -> bool {
    unsafe { libc_kill(pid as i32, 15) == 0 }
}

// ---------------------------------------------------------------------------
// Stop
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn stop(workspace: &Workspace, json_output: bool) -> Result<(), Box<dyn Error>> {
    let path = pid_file(workspace);
    let Some(pid) = read_pid(&path) else {
        return emit_error(
            json_output,
            codes::DAEMON_NOT_RUNNING,
            format!("no daemon pid file at {}", path.display()),
        );
    };
    if !process_alive(pid) {
        let _ = fs::remove_file(&path);
        return emit_error(
            json_output,
            codes::DAEMON_NOT_RUNNING,
            format!(
                "stale pid file at {} (process {pid} is gone)",
                path.display()
            ),
        );
    }
    if !send_sigterm(pid) {
        return emit_error(
            json_output,
            codes::INTERNAL,
            format!("failed to signal daemon pid {pid}"),
        );
    }
    let deadline = std::time::Instant::now() + STOP_GRACE;
    while std::time::Instant::now() < deadline {
        if !process_alive(pid) {
            let _ = fs::remove_file(&path);
            if json_output {
                json::print_ok(json!({ "stopped": pid }));
            } else {
                println!("stopped daemon pid {pid}");
            }
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    emit_error(
        json_output,
        codes::INTERNAL,
        format!("daemon pid {pid} did not exit within {:?}", STOP_GRACE),
    )
}

#[cfg(windows)]
fn stop(workspace: &Workspace, json_output: bool) -> Result<(), Box<dyn Error>> {
    let path = pid_file(workspace);
    let pid_file = match read_windows_pid_file(&path) {
        Ok(pid_file) => pid_file,
        Err(_error) if !path.exists() => {
            return emit_error(
                json_output,
                codes::DAEMON_NOT_RUNNING,
                format!("no daemon pid file at {}", path.display()),
            );
        }
        Err(error) => {
            return emit_error(
                json_output,
                codes::INTERNAL,
                format!(
                    "cannot determine daemon identity from {}: {error}",
                    path.display()
                ),
            );
        }
    };

    let process = match serve_windows::probe_process(pid_file.pid) {
        ProcessProbe::Live(process) => process,
        ProcessProbe::Dead => {
            remove_windows_pid_file_if_current(&path, &pid_file);
            return emit_error(
                json_output,
                codes::DAEMON_NOT_RUNNING,
                format!(
                    "stale pid file at {} (process {} is gone)",
                    path.display(),
                    pid_file.pid
                ),
            );
        }
        ProcessProbe::Indeterminate(error) => {
            return emit_error(
                json_output,
                codes::INTERNAL,
                format!(
                    "cannot determine whether daemon pid {} is live: {error}",
                    pid_file.pid
                ),
            );
        }
    };

    if let Err(error) = serve_windows::signal_stop(&pid_file.stop_event) {
        return emit_error(
            json_output,
            codes::INTERNAL,
            format!("failed to signal daemon pid {}: {error}", pid_file.pid),
        );
    }
    match process.wait(STOP_GRACE) {
        Ok(true) => {
            remove_windows_pid_file_if_current(&path, &pid_file);
            if json_output {
                json::print_ok(json!({ "stopped": pid_file.pid }));
            } else {
                println!("stopped daemon pid {}", pid_file.pid);
            }
            Ok(())
        }
        Ok(false) => emit_error(
            json_output,
            codes::INTERNAL,
            format!(
                "daemon pid {} did not exit within {:?}",
                pid_file.pid, STOP_GRACE
            ),
        ),
        Err(error) => emit_error(
            json_output,
            codes::INTERNAL,
            format!("failed waiting for daemon pid {}: {error}", pid_file.pid),
        ),
    }
}

// ---------------------------------------------------------------------------
// Detach (Unix) / not_implemented (Windows)
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn detach_and_run(
    workspace: Workspace,
    args: ServeArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    use daemonize::Daemonize;
    let log_path = log_file(&workspace);
    let pid_path = pid_file(&workspace);
    // We want daemonize to own the pid file so it is cleaned up on crash.
    // But we keep our own double-check inside run_foreground to catch a
    // competing daemon, since daemonize's pid file is best-effort.
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let stderr = stdout.try_clone()?;
    let daemon = Daemonize::new()
        .pid_file(&pid_path)
        .chown_pid_file(false)
        .working_directory(workspace.root())
        .stdout(stdout)
        .stderr(stderr);
    if let Err(err) = daemon.start() {
        return emit_error(
            json_output,
            codes::DAEMON_ALREADY_RUNNING,
            format!("daemonize failed: {err}"),
        );
    }
    // Inside the daemon: daemonize wrote the pid file already, so skip our
    // own acquire_lock (it would refuse — file exists with live pid == us).
    run_scheduler(workspace, args, /* locked_by_daemonize = */ true)?;
    Ok(())
}

#[cfg(windows)]
fn detach_and_run(
    _workspace: Workspace,
    _args: ServeArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    emit_error(
        json_output,
        codes::NOT_IMPLEMENTED,
        "--detach is not supported on Windows; run in the foreground",
    )
}

fn run_foreground(
    workspace: Workspace,
    args: ServeArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    #[cfg(unix)]
    if let Err(err) = acquire_lock(&workspace) {
        return emit_error(json_output, codes::DAEMON_ALREADY_RUNNING, err);
    }
    #[cfg(windows)]
    let lock = match acquire_lock(&workspace) {
        Ok(lock) => lock,
        Err(err) => {
            return emit_error(json_output, codes::DAEMON_ALREADY_RUNNING, err);
        }
    };
    #[cfg(windows)]
    let expected = lock.identity.clone();
    #[cfg(unix)]
    {
        let result = run_scheduler(workspace.clone_for_executor(), args, false);
        release_lock(&workspace);
        result
    }
    #[cfg(windows)]
    {
        let result = run_scheduler(workspace.clone_for_executor(), args, false, lock.stop_event);
        release_lock(&workspace, &expected);
        result
    }
}

// ---------------------------------------------------------------------------
// Scheduler loop
// ---------------------------------------------------------------------------

fn run_scheduler(
    workspace: Workspace,
    args: ServeArgs,
    locked_by_daemonize: bool,
    #[cfg(windows)] stop_event: StopEvent,
) -> Result<(), Box<dyn Error>> {
    let cancel_flag = Arc::new(AtomicBool::new(false));
    crate::cli::queue::install_signal_handlers(Arc::clone(&cancel_flag));

    let log_path = log_file(&workspace);
    log_line(
        &log_path,
        "INFO",
        &format!("serve started pid={}", std::process::id()),
    );

    let mut worker_handles = Vec::new();
    if !args.no_worker {
        for thread_idx in 0..args.concurrency.max(1) {
            let ws = workspace.clone_for_executor();
            let flag = Arc::clone(&cancel_flag);
            let worker_id = format!("serve-worker:{}-t{}", std::process::id(), thread_idx);
            worker_handles.push(thread::spawn(move || {
                crate::cli::queue::worker_loop(ws, worker_id, flag, None, None, false);
            }));
        }
    }

    loop {
        if cancel_flag.load(Ordering::SeqCst) {
            break;
        }
        #[cfg(windows)]
        if stop_event
            .is_signaled()
            .map_err(|error| std::io::Error::other(format!("check stop event: {error}")))?
        {
            cancel_flag.store(true, Ordering::SeqCst);
            break;
        }
        let tick_start = Utc::now();
        match scheduler_tick(&workspace, tick_start) {
            Ok(fired) => {
                if fired > 0 {
                    log_line(&log_path, "INFO", &format!("tick fired={fired}"));
                }
            }
            Err(err) => log_line(&log_path, "ERROR", &format!("tick failed: {err}")),
        }

        if args.once {
            break;
        }

        // Sleep in small slices so cancel_flag is observed promptly.
        let deadline = std::time::Instant::now() + SCAN_INTERVAL;
        while std::time::Instant::now() < deadline {
            if cancel_flag.load(Ordering::SeqCst) {
                break;
            }
            #[cfg(windows)]
            if stop_event
                .is_signaled()
                .map_err(|error| std::io::Error::other(format!("check stop event: {error}")))?
            {
                cancel_flag.store(true, Ordering::SeqCst);
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
    }

    log_line(&log_path, "INFO", "serve stopping, waiting for workers");
    for h in worker_handles {
        let _ = h.join();
    }
    log_line(&log_path, "INFO", "serve stopped");

    if locked_by_daemonize {
        // Daemonize owns the pid file; clean it up explicitly.
        let _ = fs::remove_file(pid_file(&workspace));
    }
    Ok(())
}

/// Enumerate scripts, find due schedules, enqueue runs.
/// Returns the number of rows enqueued.
pub(crate) fn scheduler_tick(
    workspace: &Workspace,
    now: chrono::DateTime<Utc>,
) -> Result<usize, String> {
    let repo = FsWorkspaceRepository::new(workspace.root().to_path_buf());
    let scripts = repo
        .list_scripts_recursive()
        .map_err(|e| format!("list scripts: {e}"))?;
    let conn = runs::open(workspace).map_err(|e| format!("open runs.sqlite: {e}"))?;
    let mut fired = 0usize;
    let log_path = log_file(workspace);
    for script in scripts {
        let schema = match repo.read_schema(&script) {
            Ok(s) => s,
            Err(err) => {
                log_line(
                    &log_path,
                    "ERROR",
                    &format!("{}: unreadable schema: {err}", script.display()),
                );
                continue;
            }
        };
        let Some(schedule) = schema.schedule.as_ref() else {
            continue;
        };
        if !schedule.enabled {
            continue;
        }

        let cron_expr = &schedule.cron;
        let cron = match parse_cron(cron_expr) {
            Ok(c) => c,
            Err(err) => {
                log_line(
                    &log_path,
                    "ERROR",
                    &format!("{}: invalid cron `{cron_expr}`: {err}", script.display()),
                );
                continue;
            }
        };

        let canonical = fs::canonicalize(&script).unwrap_or_else(|_| script.clone());
        let canonical_str = canonical.to_string_lossy().to_string();
        let schedule_id = format!("{}@{}", canonical_str, cron_expr);

        let last_fire = match runs::last_scheduled_fire_ms(&conn, &schedule_id) {
            Ok(last_fire) => last_fire,
            Err(err) => {
                log_line(
                    &log_path,
                    "ERROR",
                    &format!("{schedule_id}: schedule state unreadable: {err}; skipping fire"),
                );
                continue;
            }
        };
        // First-ever fire: look back ~2 minutes so crons that fire at
        // least once a minute (and sub-minute 6-field crons like
        // `*/10 * * * * *`) are recognised as due on the first tick
        // after daemon start. Longer periods (`@daily`, `@hourly`
        // off-boundary) are NOT triggered at start — they wait for
        // their next natural firing time.
        let reference = last_fire
            .and_then(chrono::DateTime::<Utc>::from_timestamp_millis)
            .unwrap_or(now - chrono::Duration::minutes(2));

        if !is_due(&cron, reference, now) {
            continue;
        }

        let raw_args = build_args_from_defaults(&schema);
        // Secret-safe enqueue: reject plaintext secret-field defaults and
        // persist `secret://` refs (not plaintext), matching the manual and
        // HTTP enqueue contract. Without this, a secret field carrying a
        // plaintext `Default` would land raw in runs.sqlite `args_json` and
        // leak through `history` / `GET /v1/runs/:id` / traces, since read
        // paths do not redact. Fail closed: skip the fire and log on any
        // unresolvable or non-reconstructable secret.
        let resolved = match secrets::validate_queued_secret_args_reconstructable(
            workspace, &canonical, &raw_args,
        )
        .and_then(|()| {
            secrets::resolve_args_with_access(
                workspace,
                &canonical,
                &raw_args,
                &[],
                &[],
                &secrets::SecretAccess::allow_all(),
            )
        }) {
            Ok(resolved) => resolved,
            Err((field, message)) => {
                log_line(
                    &log_path,
                    "ERROR",
                    &format!(
                        "{schedule_id}: secret field `{field}` not enqueue-safe: {message}; skipping fire"
                    ),
                );
                continue;
            }
        };
        let opts = EnqueueOptions {
            actor: "scheduler".to_string(),
            reason: Some(format!("cron: {cron_expr}")),
            cron_schedule_id: Some(schedule_id.clone()),
            script_name: Some(schema.name.clone()),
            omakure_version: app_meta::APP_VERSION.to_string(),
            trigger: RunTrigger::Scheduled,
            allowed_secret_refs: Some(resolved.provider_refs),
            ..Default::default()
        };
        match runs::enqueue_scheduled(&conn, &canonical_str, &resolved.persisted_args, opts) {
            Ok(Some(row)) => {
                fired += 1;
                log_line(
                    &log_path,
                    "INFO",
                    &format!(
                        "enqueued run_id={} script={} schedule_id={}",
                        row.run_id, canonical_str, schedule_id
                    ),
                );
            }
            Ok(None) => log_line(
                &log_path,
                "WARN",
                &format!("{schedule_id}: previous run still in flight, skipping fire"),
            ),
            Err(err) => log_line(
                &log_path,
                "ERROR",
                &format!("enqueue {schedule_id} failed: {err}"),
            ),
        }
    }
    Ok(fired)
}

fn is_due(
    cron: &CronSchedule,
    reference: chrono::DateTime<Utc>,
    now: chrono::DateTime<Utc>,
) -> bool {
    match next_fire_after(cron, reference) {
        Some(next) => next <= now,
        None => false,
    }
}

fn build_args_from_defaults(schema: &crate::domain::Schema) -> Vec<String> {
    let mut out = Vec::new();
    for field in &schema.fields {
        let Some(default) = field.default.as_deref() else {
            continue;
        };
        if default.is_empty() {
            continue;
        }
        let flag = field
            .arg
            .clone()
            .unwrap_or_else(|| format!("--{}", field.name));
        out.push(flag);
        out.push(default.to_string());
    }
    out
}

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

fn log_line(path: &Path, level: &str, message: &str) {
    let line = format!("{} [{}] {}\n", Utc::now().to_rfc3339(), level, message);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    } else {
        eprintln!("{line}");
    }
}

// ---------------------------------------------------------------------------
// Error helper
// ---------------------------------------------------------------------------

fn emit_error(
    json_output: bool,
    code: &str,
    message: impl Into<String>,
) -> Result<(), Box<dyn Error>> {
    let msg = message.into();
    if json_output {
        json::print_err(code, msg.clone());
    } else {
        eprintln!("error: {msg}");
    }
    std::process::exit(1);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_script(dir: &Path, name: &str, schedule: Option<&str>) -> PathBuf {
        let mut json = String::from(
            "{ \"Name\": \"demo\", \"Fields\": [ {\"Name\":\"env\",\"Type\":\"string\",\"Arg\":\"--env\",\"Default\":\"prod\"} ]",
        );
        if let Some(cron) = schedule {
            json.push_str(&format!(
                ", \"Schedule\": {{ \"Cron\": \"{cron}\", \"Enabled\": true }}"
            ));
        }
        json.push_str(" }");
        let script = format!(
            "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {}\n# OMAKURE_SCHEMA_END\necho ok\n",
            json
        );
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(script.as_bytes()).unwrap();
        path
    }

    #[test]
    fn build_args_emits_flag_and_default() {
        let schema = crate::domain::parse_schema(
            r#"{"Name":"s","Fields":[{"Name":"env","Type":"string","Arg":"--env","Default":"prod"}]}"#,
        )
        .unwrap();
        let args = build_args_from_defaults(&schema);
        assert_eq!(args, vec!["--env", "prod"]);
    }

    #[test]
    fn build_args_skips_fields_without_default() {
        let schema = crate::domain::parse_schema(
            r#"{"Name":"s","Fields":[{"Name":"env","Type":"string","Arg":"--env"}]}"#,
        )
        .unwrap();
        let args = build_args_from_defaults(&schema);
        assert!(args.is_empty());
    }

    #[test]
    fn tick_enqueues_scheduled_run_on_first_fire() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        // Use a schedule that fires every minute; reference starts before now
        // so the very first tick will always be due.
        write_script(tmp.path(), "scheduled.sh", Some("* * * * *"));
        // Also a non-scheduled script to confirm we ignore it.
        write_script(tmp.path(), "manual.sh", None);

        let now = Utc::now();
        let fired = scheduler_tick(&ws, now).unwrap();
        assert_eq!(fired, 1, "exactly one scheduled script should have fired");

        let conn = runs::open(&ws).unwrap();
        let rows = runs::query_runs(
            &conn,
            &runs::RunFilters {
                states: runs::RunStateSet::All.to_states(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].trigger, RunTrigger::Scheduled);
        assert!(rows[0]
            .cron_schedule_id
            .as_deref()
            .unwrap()
            .contains("@* * * * *"));
    }

    #[test]
    fn tick_skips_disabled_schedule() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        // Write schedule with Enabled=false manually.
        let json =
            r#"{ "Name":"s", "Fields":[], "Schedule": { "Cron": "* * * * *", "Enabled": false } }"#;
        let script = format!(
            "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {}\n# OMAKURE_SCHEMA_END\n",
            json
        );
        fs::write(tmp.path().join("off.sh"), script).unwrap();

        let fired = scheduler_tick(&ws, Utc::now()).unwrap();
        assert_eq!(fired, 0);
    }

    #[test]
    fn concurrent_ticks_enqueue_one_scheduled_run() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        write_script(tmp.path(), "concurrent.sh", Some("* * * * *"));
        let now = Utc::now();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let first_ws = ws.clone_for_executor();
        let second_ws = ws.clone_for_executor();
        let first_barrier = Arc::clone(&barrier);
        let second_barrier = Arc::clone(&barrier);
        let first = thread::spawn(move || {
            first_barrier.wait();
            scheduler_tick(&first_ws, now)
        });
        let second = thread::spawn(move || {
            second_barrier.wait();
            scheduler_tick(&second_ws, now)
        });
        barrier.wait();
        let first_fired = first.join().unwrap().unwrap();
        let second_fired = second.join().unwrap().unwrap();
        assert_eq!(
            first_fired + second_fired,
            1,
            "concurrent scheduler ticks must claim one fire"
        );

        let conn = runs::open(&ws).unwrap();
        let rows = runs::query_runs(
            &conn,
            &runs::RunFilters {
                states: runs::RunStateSet::All.to_states(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
    }
    #[test]
    fn tick_logs_malformed_schema_and_continues() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        fs::write(
            tmp.path().join("broken.sh"),
            "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {not-json}\n# OMAKURE_SCHEMA_END\n",
        )
        .unwrap();
        write_script(tmp.path(), "healthy.sh", Some("* * * * *"));

        let fired = scheduler_tick(&ws, Utc::now()).unwrap();
        assert_eq!(fired, 1, "a malformed script must not stop other schedules");
        let log = fs::read_to_string(log_file(&ws)).unwrap();
        assert!(log.contains("broken.sh"));
        assert!(log.contains("unreadable schema"));
    }
    #[test]
    fn tick_skips_when_previous_run_still_in_flight() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        let script = write_script(tmp.path(), "s.sh", Some("* * * * *"));
        let fired = scheduler_tick(&ws, Utc::now()).unwrap();
        assert_eq!(fired, 1);
        // Second tick immediately after should not enqueue again because the
        // previous row is still queued.
        let fired_again = scheduler_tick(&ws, Utc::now()).unwrap();
        assert_eq!(fired_again, 0);
        let _ = script;
    }

    #[test]
    fn tick_persists_secret_ref_default_not_plaintext() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        std::env::set_var("OMAKURE_CRON_SECRET_REF", "cron_plaintext_value");
        let json = r#"{ "Name":"s", "Fields":[{"Name":"TOKEN","Type":"secret","Arg":"--token","Default":"secret://env/OMAKURE_CRON_SECRET_REF"}], "Schedule": { "Cron": "* * * * *", "Enabled": true } }"#;
        let script = format!(
            "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {json}\n# OMAKURE_SCHEMA_END\n"
        );
        fs::write(tmp.path().join("sched.sh"), script).unwrap();

        let fired = scheduler_tick(&ws, Utc::now()).unwrap();
        assert_eq!(fired, 1);

        let conn = runs::open(&ws).unwrap();
        let rows = runs::query_runs(
            &conn,
            &runs::RunFilters {
                states: runs::RunStateSet::All.to_states(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        // Regression (audit #1936 finding 1): the cron path must persist the
        // secret:// ref, never the resolved plaintext, into args_json at rest.
        assert!(rows[0]
            .args_json
            .contains("secret://env/OMAKURE_CRON_SECRET_REF"));
        assert!(
            !rows[0].args_json.contains("cron_plaintext_value"),
            "scheduler leaked resolved secret plaintext into args_json: {}",
            rows[0].args_json
        );
        std::env::remove_var("OMAKURE_CRON_SECRET_REF");
    }

    #[test]
    fn tick_skips_fire_on_plaintext_secret_default() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        let json = r#"{ "Name":"s", "Fields":[{"Name":"TOKEN","Type":"secret","Arg":"--token","Default":"plaintext_secret_default"}], "Schedule": { "Cron": "* * * * *", "Enabled": true } }"#;
        let script = format!(
            "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {json}\n# OMAKURE_SCHEMA_END\n"
        );
        fs::write(tmp.path().join("bad.sh"), script).unwrap();

        // Regression (audit #1936 finding 1): a plaintext secret default is not
        // reconstructable, so the fire is rejected (fail-closed) rather than
        // persisting plaintext at rest.
        let fired = scheduler_tick(&ws, Utc::now()).unwrap();
        assert_eq!(fired, 0, "plaintext secret default must not enqueue");

        let conn = runs::open(&ws).unwrap();
        let rows = runs::query_runs(
            &conn,
            &runs::RunFilters {
                states: runs::RunStateSet::All.to_states(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            rows.is_empty(),
            "no run should be enqueued for a plaintext secret default"
        );
    }

    #[cfg(unix)]
    #[test]
    fn acquire_lock_rejects_when_live_pid_present() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        // Write our own PID — it is by definition alive.
        fs::write(pid_file(&ws), std::process::id().to_string()).unwrap();
        let err = acquire_lock(&ws).unwrap_err();
        assert!(err.contains("daemon already running"), "was: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn acquire_lock_reclaims_stale_pid() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        // A PID that is effectively guaranteed not to exist.
        fs::write(pid_file(&ws), "999999999").unwrap();
        acquire_lock(&ws).expect("stale PID should be reclaimed");
        release_lock(&ws);
    }

    #[cfg(windows)]
    #[test]
    fn windows_acquire_lock_reclaims_dead_pid_with_event_identity() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        fs::write(
            pid_file(&ws),
            "4294967295\nLocal\\OmakureServeStop-00000000000000000000000000000000\n",
        )
        .unwrap();

        let event = acquire_lock(&ws).expect("dead PID should be reclaimed");
        release_lock(&ws, &event.identity);
        drop(event);
    }

    #[cfg(windows)]
    #[test]
    fn windows_malformed_or_partial_pid_files_are_preserved() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        let path = pid_file(&ws);

        for contents in [
            "",
            "1234\n",
            "not-a-pid\nLocal\\OmakureServeStop-00000000000000000000000000000000\n",
            "1234\nnot-an-event\n",
        ] {
            fs::write(&path, contents).unwrap();
            assert!(
                acquire_lock(&ws).is_err(),
                "invalid PID file must not be accepted: {contents:?}"
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_release_does_not_delete_a_replacement_identity() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        let old = WindowsPidFile {
            pid: 100,
            stop_event: "Local\\OmakureServeStop-00000000000000000000000000000001".to_string(),
        };
        let replacement = WindowsPidFile {
            pid: 200,
            stop_event: "Local\\OmakureServeStop-00000000000000000000000000000002".to_string(),
        };
        fs::write(
            pid_file(&ws),
            format!("{}\n{}\n", replacement.pid, replacement.stop_event),
        )
        .unwrap();

        release_lock(&ws, &old);

        assert_eq!(read_windows_pid_file(&pid_file(&ws)).unwrap(), replacement);
    }

    #[cfg(windows)]
    #[test]
    fn windows_release_deletes_only_the_published_identity() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        let identity = WindowsPidFile {
            pid: 300,
            stop_event: "Local\\OmakureServeStop-00000000000000000000000000000003".to_string(),
        };
        fs::write(
            pid_file(&ws),
            format!("{}\n{}\n", identity.pid, identity.stop_event),
        )
        .unwrap();

        release_lock(&ws, &identity);

        assert!(!pid_file(&ws).exists());
    }

    #[cfg(windows)]
    #[test]
    fn windows_pid_publication_is_complete_and_exclusive() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        let identity = WindowsPidFile {
            pid: 400,
            stop_event: "Local\\OmakureServeStop-00000000000000000000000000000004".to_string(),
        };

        publish_windows_pid_file(&pid_file(&ws), &identity).unwrap();

        assert_eq!(read_windows_pid_file(&pid_file(&ws)).unwrap(), identity);
        assert!(!pid_file(&ws)
            .with_file_name("daemon.pid.00000000000000000000000000000004.tmp")
            .exists());

        let replacement = WindowsPidFile {
            pid: 401,
            stop_event: "Local\\OmakureServeStop-00000000000000000000000000000005".to_string(),
        };
        let result = publish_windows_pid_file(&pid_file(&ws), &replacement);
        assert!(result.is_err(), "publication must retain exclusive startup");
        assert_eq!(read_windows_pid_file(&pid_file(&ws)).unwrap(), identity);
        assert!(!pid_file(&ws)
            .with_file_name("daemon.pid.00000000000000000000000000000005.tmp")
            .exists());
    }
}

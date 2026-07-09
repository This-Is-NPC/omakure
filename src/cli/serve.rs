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
use crate::domain::{next_fire_after, parse_cron};
use crate::ports::ScriptRepository;
use crate::runs::{self, EnqueueOptions, RunState, RunTrigger};
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

fn release_lock(workspace: &Workspace) {
    let _ = fs::remove_file(pid_file(workspace));
}

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

#[cfg(windows)]
fn process_alive(_pid: u32) -> bool {
    // On Windows we conservatively assume the PID file is stale if we ever
    // fail to open it; the user can delete it manually. Good enough for v1.
    true
}

#[cfg(unix)]
fn send_sigterm(pid: u32) -> bool {
    unsafe { libc_kill(pid as i32, 15) == 0 }
}

#[cfg(windows)]
fn send_sigterm(_pid: u32) -> bool {
    false
}

// ---------------------------------------------------------------------------
// Stop
// ---------------------------------------------------------------------------

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
    if let Err(err) = acquire_lock(&workspace) {
        return emit_error(json_output, codes::DAEMON_ALREADY_RUNNING, err);
    }
    let result = run_scheduler(workspace.clone_for_executor(), args, false);
    release_lock(&workspace);
    result
}

// ---------------------------------------------------------------------------
// Scheduler loop
// ---------------------------------------------------------------------------

fn run_scheduler(
    workspace: Workspace,
    args: ServeArgs,
    locked_by_daemonize: bool,
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
            Err(_) => continue,
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

        let last_fire = last_fire_ms(&conn, &schedule_id).unwrap_or(None);
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

        if has_live_run(&conn, &schedule_id).unwrap_or(false) {
            log_line(
                &log_path,
                "WARN",
                &format!("{schedule_id}: previous run still in flight, skipping fire"),
            );
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
        match runs::enqueue(&conn, &canonical_str, &resolved.persisted_args, opts) {
            Ok(row) => {
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

fn last_fire_ms(conn: &rusqlite::Connection, schedule_id: &str) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT MAX(enqueued_at) FROM runs WHERE cron_schedule_id = ?",
        [schedule_id],
        |row| row.get::<_, Option<i64>>(0),
    )
    .map_err(|e| format!("last_fire query: {e}"))
}

fn has_live_run(conn: &rusqlite::Connection, schedule_id: &str) -> Result<bool, String> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM runs WHERE cron_schedule_id = ? AND state IN (?, ?)",
            rusqlite::params![
                schedule_id,
                RunState::Queued.as_str(),
                RunState::Running.as_str()
            ],
            |row| row.get(0),
        )
        .map_err(|e| format!("live_run query: {e}"))?;
    Ok(count > 0)
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
}

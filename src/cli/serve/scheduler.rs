use super::lifecycle::pid_file;
use super::logging::{log_file, log_line};
use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::app_meta;
use crate::cli::args::ServeArgs;
#[cfg(windows)]
use crate::cli::serve_windows::StopEvent;
use crate::domain::{next_fire_after, parse_cron};
use crate::ports::ScriptRepository;
use crate::runs::{self, EnqueueOptions, RunTrigger};
use crate::secrets;
use crate::workspace::Workspace;
use chrono::Utc;
use cron::Schedule as CronSchedule;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const SCAN_INTERVAL: Duration = Duration::from_secs(5);

pub(super) fn run_scheduler(
    workspace: Workspace,
    args: ServeArgs,
    locked_by_daemonize: bool,
    #[cfg(windows)] stop_event: StopEvent,
) -> Result<(), Box<dyn Error>> {
    let cancel_flag = Arc::new(AtomicBool::new(false));
    crate::adapters::signals::install_signal_handlers(Arc::clone(&cancel_flag));

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
                crate::operations::worker::worker_loop(ws, worker_id, flag, None, None, false);
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

/// Discovery visibility is not execution authority: file symlinks can point
/// into uninstalled Battery cache even when hidden directories are not listed.
fn scheduled_subjects(
    workspace: &Workspace,
    repo: &FsWorkspaceRepository,
) -> Result<Vec<PathBuf>, String> {
    let scripts = repo
        .list_scripts_recursive()
        .map_err(|e| format!("list scripts: {e}"))?;
    let log_path = log_file(workspace);
    let mut subjects = Vec::new();
    for script in scripts {
        match crate::operations::core::canonical_script_path(&script, workspace.scripts_root()) {
            Ok(script) => subjects.push(script),
            Err(error) => log_line(&log_path, "ERROR", &format!("{error}; skipping schedule")),
        }
    }
    Ok(subjects)
}

/// Enumerate scripts, find due schedules, enqueue runs.
/// Returns the number of rows enqueued.
pub(crate) fn scheduler_tick(
    workspace: &Workspace,
    now: chrono::DateTime<Utc>,
) -> Result<usize, String> {
    let repo = FsWorkspaceRepository::new(workspace.root().to_path_buf());
    let scripts = scheduled_subjects(workspace, &repo)?;
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

        let canonical = script;
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

pub(super) fn build_args_from_defaults(schema: &crate::domain::Schema) -> Vec<String> {
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

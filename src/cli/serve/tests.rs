#[cfg(unix)]
use super::lifecycle::LockError;
#[cfg(windows)]
use super::lifecycle::{
    WindowsLockError, WindowsPidFile, WindowsPidFileError, WindowsPidPublicationError,
    publish_windows_pid_file, read_windows_pid_file,
};
use super::lifecycle::{acquire_lock, pid_file, release_lock};
use super::logging::log_file;
use super::scheduler::{SchedulerTickError, build_args_from_defaults, scheduler_tick};
use crate::runs::{self, RunStore, RunTrigger};
use crate::test_support::workspace_in;
use crate::workspace::Workspace;
use chrono::Utc;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
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

fn all_runs(workspace: &Workspace) -> Vec<runs::RunRow> {
    RunStore::open(workspace)
        .unwrap()
        .query_runs(&runs::RunFilters {
            states: runs::RunStateSet::All.to_states(),
            ..Default::default()
        })
        .unwrap()
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
    let ws = workspace_in(&tmp);
    // Use a schedule that fires every minute; reference starts before now
    // so the very first tick will always be due.
    write_script(tmp.path(), "scheduled.sh", Some("* * * * *"));
    // Also a non-scheduled script to confirm we ignore it.
    write_script(tmp.path(), "manual.sh", None);

    let now = Utc::now();
    let fired = scheduler_tick(&ws, now).unwrap();
    assert_eq!(fired, 1, "exactly one scheduled script should have fired");

    let rows = all_runs(&ws);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].trigger, RunTrigger::Scheduled);
    assert!(
        rows[0]
            .cron_schedule_id
            .as_deref()
            .unwrap()
            .contains("@* * * * *")
    );
}

#[test]
fn tick_reports_run_store_open_failure_without_changing_error_text() {
    let temp = TempDir::new().unwrap();
    let workspace = workspace_in(&temp);
    write_script(temp.path(), "scheduled.sh", Some("* * * * *"));
    let history = workspace.history_dir();
    fs::remove_dir_all(history).unwrap();
    fs::write(history, "blocked").unwrap();

    let error = scheduler_tick(&workspace, Utc::now()).unwrap_err();
    assert!(matches!(error, SchedulerTickError::OpenRuns(_)));
    assert!(
        error
            .to_string()
            .starts_with("open runs.sqlite: Create history dir failed: ")
    );
}

#[test]
#[cfg(unix)]
fn scheduler_rejects_reserved_metadata_aliases() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    let cache = ws.root().join(".omakure/batteries/cache");
    fs::create_dir_all(&cache).unwrap();
    let cached = write_script(&cache, "cached.sh", Some("* * * * *"));
    std::os::unix::fs::symlink(cached, ws.root().join("alias.sh")).unwrap();
    assert_eq!(scheduler_tick(&ws, Utc::now()).unwrap(), 0);
    write_script(ws.root(), "installed.sh", Some("* * * * *"));
    assert_eq!(scheduler_tick(&ws, Utc::now()).unwrap(), 1);
}

#[test]
fn tick_skips_disabled_schedule() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
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
    let ws = workspace_in(&tmp);
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

    let rows = all_runs(&ws);
    assert_eq!(rows.len(), 1);
}
#[test]
fn tick_logs_malformed_schema_and_continues() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
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
    let ws = workspace_in(&tmp);
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
fn tick_skips_fire_on_plaintext_secret_default() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    let json = r#"{ "Name":"s", "Fields":[{"Name":"TOKEN","Type":"secret","Arg":"--token","Default":"plaintext_secret_default"}], "Schedule": { "Cron": "* * * * *", "Enabled": true } }"#;
    let script =
        format!("#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {json}\n# OMAKURE_SCHEMA_END\n");
    fs::write(tmp.path().join("bad.sh"), script).unwrap();

    // Regression (audit #1936 finding 1): a plaintext secret default is not
    // reconstructable, so the fire is rejected (fail-closed) rather than
    // persisting plaintext at rest.
    let fired = scheduler_tick(&ws, Utc::now()).unwrap();
    assert_eq!(fired, 0, "plaintext secret default must not enqueue");

    let rows = all_runs(&ws);
    assert!(
        rows.is_empty(),
        "no run should be enqueued for a plaintext secret default"
    );
}

#[cfg(unix)]
#[test]
fn acquire_lock_rejects_when_live_pid_present() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    // Write our own PID — it is by definition alive.
    fs::write(pid_file(&ws), std::process::id().to_string()).unwrap();
    let err = acquire_lock(&ws).unwrap_err();
    assert!(matches!(&err, LockError::AlreadyRunning { .. }));
    assert_eq!(
        err.to_string(),
        format!(
            "daemon already running (pid {}, lock file {})",
            std::process::id(),
            pid_file(&ws).display()
        )
    );
}

#[cfg(unix)]
#[test]
fn acquire_lock_reports_create_failure_with_path() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    fs::remove_dir_all(ws.omakure_dir()).unwrap();
    let err = acquire_lock(&ws).unwrap_err();
    assert!(matches!(
        &err,
        LockError::Create { path, source }
            if path == &pid_file(&ws) && source.kind() == std::io::ErrorKind::NotFound
    ));
    assert!(
        err.to_string()
            .starts_with(&format!("create {}: ", pid_file(&ws).display()))
    );
}

#[cfg(unix)]
#[test]
fn acquire_lock_reclaims_stale_pid() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    // A PID that is effectively guaranteed not to exist.
    fs::write(pid_file(&ws), "999999999").unwrap();
    acquire_lock(&ws).expect("stale PID should be reclaimed");
    release_lock(&ws);
}

#[cfg(windows)]
#[test]
fn windows_acquire_lock_reclaims_dead_pid_with_event_identity() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
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
fn windows_acquire_lock_preserves_live_identity_and_typed_error() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    let path = pid_file(&ws);
    let (stop_event, _event) = crate::cli::serve_windows::create_stop_event().unwrap();
    let identity = WindowsPidFile {
        pid: std::process::id(),
        stop_event,
    };
    fs::write(
        &path,
        format!("{}\n{}\n", identity.pid, identity.stop_event),
    )
    .unwrap();

    let error = acquire_lock(&ws)
        .err()
        .expect("live daemon must retain lock");
    assert!(matches!(
        error,
        WindowsLockError::AlreadyRunning { pid, path: lock_path }
            if pid == identity.pid && lock_path == path
    ));
    assert_eq!(read_windows_pid_file(&path).unwrap(), identity);
}

#[cfg(windows)]
#[test]
fn windows_malformed_or_partial_pid_files_are_preserved() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    let path = pid_file(&ws);

    for (contents, expected) in [
        ("", format!("{} is empty", path.display())),
        (
            "1234\n",
            format!("{} has no stop-event identity", path.display()),
        ),
        (
            "not-a-pid\nLocal\\OmakureServeStop-00000000000000000000000000000000\n",
            format!(
                "invalid PID in {}: invalid digit found in string",
                path.display()
            ),
        ),
        (
            "1234\nnot-an-event\n",
            format!("invalid stop-event identity in {}", path.display()),
        ),
    ] {
        fs::write(&path, contents).unwrap();
        assert_eq!(
            read_windows_pid_file(&path).unwrap_err().to_string(),
            expected
        );
        let error = acquire_lock(&ws).err().expect("invalid PID file must fail");
        assert!(matches!(&error, WindowsLockError::PidFile(_)));
        assert_eq!(error.to_string(), expected);
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
    }
}

#[cfg(windows)]
#[test]
fn windows_pid_file_read_error_keeps_path_and_source() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    let path = pid_file(&ws);
    let error = read_windows_pid_file(&path).unwrap_err();
    assert!(matches!(
        &error,
        WindowsPidFileError::Read { path: failed_path, source }
            if failed_path == &path && source.kind() == std::io::ErrorKind::NotFound
    ));
    assert!(
        error
            .to_string()
            .starts_with(&format!("read {}: ", path.display()))
    );
}

#[cfg(windows)]
#[test]
fn windows_release_does_not_delete_a_replacement_identity() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
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
    let ws = workspace_in(&tmp);
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
    let ws = workspace_in(&tmp);
    let identity = WindowsPidFile {
        pid: 400,
        stop_event: "Local\\OmakureServeStop-00000000000000000000000000000004".to_string(),
    };

    publish_windows_pid_file(&pid_file(&ws), &identity).unwrap();

    assert_eq!(read_windows_pid_file(&pid_file(&ws)).unwrap(), identity);
    assert!(
        !pid_file(&ws)
            .with_file_name("daemon.pid.00000000000000000000000000000004.tmp")
            .exists()
    );

    let replacement = WindowsPidFile {
        pid: 401,
        stop_event: "Local\\OmakureServeStop-00000000000000000000000000000005".to_string(),
    };
    let error = publish_windows_pid_file(&pid_file(&ws), &replacement).unwrap_err();
    assert!(matches!(&error, WindowsPidPublicationError::Publish { .. }));
    assert!(error.to_string().starts_with(&format!(
        "publish {}: MoveFileExW failed with Windows error ",
        pid_file(&ws).display()
    )));
    assert_eq!(read_windows_pid_file(&pid_file(&ws)).unwrap(), identity);
    assert!(
        !pid_file(&ws)
            .with_file_name("daemon.pid.00000000000000000000000000000005.tmp")
            .exists()
    );
}

#[cfg(windows)]
#[test]
fn windows_pid_publication_reports_create_failure() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("missing").join("daemon.pid");
    let identity = WindowsPidFile {
        pid: 402,
        stop_event: "Local\\OmakureServeStop-00000000000000000000000000000006".to_string(),
    };
    let temp_path = path.with_file_name("daemon.pid.00000000000000000000000000000006.tmp");
    let error = publish_windows_pid_file(&path, &identity).unwrap_err();
    assert!(matches!(
        &error,
        WindowsPidPublicationError::Create { path: failed_path, source }
            if failed_path == &temp_path
                && source.kind() == std::io::ErrorKind::NotFound
    ));
    assert!(
        error
            .to_string()
            .starts_with(&format!("create {}: ", temp_path.display()))
    );
}

#[cfg(unix)]
use super::logging::log_file;
use super::scheduler::run_scheduler;
use crate::cli::args::ServeArgs;
use crate::cli::emit::exit_with_error;
use crate::cli::json::{self, codes};
#[cfg(windows)]
use crate::cli::serve_windows::{self, OpenEventError, ProcessProbe, StopEvent};
use crate::workspace::Workspace;
use serde_json::json;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::thread;
use std::time::Duration;

const STOP_GRACE: Duration = Duration::from_secs(5);

#[cfg(unix)]
#[derive(Debug, thiserror::Error)]
pub(super) enum LockError {
    #[error("daemon already running (pid {pid}, lock file {})", path.display())]
    AlreadyRunning { pid: u32, path: PathBuf },
    #[error("create {}: {source}", path.display())]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

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

pub(super) fn pid_file(workspace: &Workspace) -> PathBuf {
    workspace.omakure_dir().join("daemon.pid")
}

// ---------------------------------------------------------------------------
// Lock file
// ---------------------------------------------------------------------------

#[cfg(unix)]
pub(super) fn acquire_lock(workspace: &Workspace) -> Result<(), LockError> {
    let path = pid_file(workspace);
    if path.exists() {
        match read_pid(&path) {
            Some(pid) if process_alive(pid) => {
                return Err(LockError::AlreadyRunning { pid, path });
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
        .map_err(|source| LockError::Create {
            path: path.clone(),
            source,
        })?;
    writeln!(file, "{}", std::process::id()).map_err(|source| LockError::Write { path, source })?;
    Ok(())
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WindowsPidFile {
    pub(super) pid: u32,
    pub(super) stop_event: String,
}

#[cfg(windows)]
pub(super) struct WindowsLock {
    pub(super) identity: WindowsPidFile,
    stop_event: StopEvent,
}

#[cfg(windows)]
#[derive(Debug, thiserror::Error)]
pub(super) enum WindowsPidFileError {
    #[error("read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} is empty", path.display())]
    Empty { path: PathBuf },
    #[error("invalid PID in {}: {source}", path.display())]
    InvalidPid {
        path: PathBuf,
        #[source]
        source: std::num::ParseIntError,
    },
    #[error("{} has no stop-event identity", path.display())]
    MissingStopEvent { path: PathBuf },
    #[error("invalid stop-event identity in {}", path.display())]
    InvalidStopEvent { path: PathBuf },
}

#[cfg(windows)]
pub(super) fn read_windows_pid_file(path: &Path) -> Result<WindowsPidFile, WindowsPidFileError> {
    let contents = fs::read_to_string(path).map_err(|source| WindowsPidFileError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut lines = contents.lines();
    let pid = lines
        .next()
        .ok_or_else(|| WindowsPidFileError::Empty {
            path: path.to_path_buf(),
        })?
        .trim()
        .parse::<u32>()
        .map_err(|source| WindowsPidFileError::InvalidPid {
            path: path.to_path_buf(),
            source,
        })?;
    let stop_event = lines
        .next()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| WindowsPidFileError::MissingStopEvent {
            path: path.to_path_buf(),
        })?
        .to_string();
    if !serve_windows::is_stop_event_name(&stop_event) {
        return Err(WindowsPidFileError::InvalidStopEvent {
            path: path.to_path_buf(),
        });
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
pub(super) fn publish_windows_pid_file(
    path: &Path,
    identity: &WindowsPidFile,
) -> Result<(), String> {
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
pub(super) fn acquire_lock(workspace: &Workspace) -> Result<WindowsLock, String> {
    let path = pid_file(workspace);
    if path.exists() {
        let existing = read_windows_pid_file(&path).map_err(|error| error.to_string())?;
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
    publish_windows_pid_file(&path, &identity)?;
    Ok(WindowsLock {
        identity,
        stop_event,
    })
}

#[cfg(unix)]
pub(super) fn release_lock(workspace: &Workspace) {
    let _ = fs::remove_file(pid_file(workspace));
}

#[cfg(windows)]
pub(super) fn release_lock(workspace: &Workspace, expected: &WindowsPidFile) {
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
        exit_with_error(
            json_output,
            codes::DAEMON_NOT_RUNNING,
            format!("no daemon pid file at {}", path.display()),
        );
    };
    if !process_alive(pid) {
        let _ = fs::remove_file(&path);
        exit_with_error(
            json_output,
            codes::DAEMON_NOT_RUNNING,
            format!(
                "stale pid file at {} (process {pid} is gone)",
                path.display()
            ),
        );
    }
    if !send_sigterm(pid) {
        exit_with_error(
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
    exit_with_error(
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
            exit_with_error(
                json_output,
                codes::DAEMON_NOT_RUNNING,
                format!("no daemon pid file at {}", path.display()),
            );
        }
        Err(error) => {
            exit_with_error(
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
            exit_with_error(
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
            exit_with_error(
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
        exit_with_error(
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
        Ok(false) => exit_with_error(
            json_output,
            codes::INTERNAL,
            format!(
                "daemon pid {} did not exit within {:?}",
                pid_file.pid, STOP_GRACE
            ),
        ),
        Err(error) => exit_with_error(
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
        exit_with_error(
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
    exit_with_error(
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
        exit_with_error(json_output, codes::DAEMON_ALREADY_RUNNING, err.to_string());
    }
    #[cfg(windows)]
    let lock = match acquire_lock(&workspace) {
        Ok(lock) => lock,
        Err(err) => {
            exit_with_error(json_output, codes::DAEMON_ALREADY_RUNNING, err);
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

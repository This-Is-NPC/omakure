//! `omakure serve --install` / `--uninstall` / `--status`.
//!
//! Installs a per-workspace systemd user service so the scheduler
//! daemon survives reboots without requiring the user to wire up
//! init scripts by hand. Each workspace gets its own unit, uniquely
//! named by a stable hash of the canonical workspace path.
//!
//! Linux-only. Other platforms return `not_implemented`; platform-specific
//! service-manager integration is unsupported.

use crate::cli::emit::exit_with_error;
use crate::cli::json::codes;
use crate::workspace::Workspace;
use std::error::Error;
#[cfg(any(target_os = "linux", test))]
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use crate::cli::json;
#[cfg(target_os = "linux")]
use serde_json::json;
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::process::Command;

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, thiserror::Error)]
enum AutostartError {
    #[cfg(target_os = "linux")]
    #[error("HOME is not set")]
    HomeMissing,
    #[error("current_exe: {0}")]
    CurrentExe(#[source] std::io::Error),
    #[error("canonicalize workspace {path}: {source}")]
    CanonicalizeWorkspace {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[cfg(target_os = "linux")]
    #[error("systemctl --user {args:?}: {source}")]
    SystemctlIo {
        args: Vec<String>,
        #[source]
        source: std::io::Error,
    },
    #[cfg(target_os = "linux")]
    #[error("systemctl --user {args:?} failed: {stderr}")]
    SystemctlFailed { args: Vec<String>, stderr: String },
}

/// Public entry points mirror the three CLI flags.
pub fn install(workspace: &Workspace, json_output: bool) -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "linux")]
    {
        install_linux(workspace, json_output)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = workspace;
        unsupported(json_output)
    }
}

pub fn uninstall(workspace: &Workspace, json_output: bool) -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "linux")]
    {
        uninstall_linux(workspace, json_output)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = workspace;
        unsupported(json_output)
    }
}

pub fn status(workspace: &Workspace, json_output: bool) -> Result<(), Box<dyn Error>> {
    #[cfg(target_os = "linux")]
    {
        status_linux(workspace, json_output)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = workspace;
        unsupported(json_output)
    }
}

#[cfg(not(target_os = "linux"))]
fn unsupported(json_output: bool) -> Result<(), Box<dyn Error>> {
    exit_with_error(
        json_output,
        codes::NOT_IMPLEMENTED,
        "serve --install is only supported on Linux (systemd user units). \
         Platform-specific service-manager integration is unsupported on \
         macOS and Windows.",
    )
}

// ---------------------------------------------------------------------------
// Naming + paths (pure; platform-independent)
// ---------------------------------------------------------------------------

/// Stable, deterministic 64-bit FNV-1a hash of the canonical workspace
/// path. Used to derive a unique systemd unit name per workspace so
/// multiple workspaces can each have their own service. Not
/// cryptographic — we only need collision-resistance across a single
/// user's machine.
#[cfg(any(target_os = "linux", test))]
fn path_hash(path: &Path) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn unit_name(workspace: &Workspace) -> String {
    let canonical =
        std::fs::canonicalize(workspace.root()).unwrap_or_else(|_| workspace.root().to_path_buf());
    format!("omakure-{:016x}.service", path_hash(&canonical))
}

#[cfg(target_os = "linux")]
fn unit_dir() -> Result<PathBuf, AutostartError> {
    let home = std::env::var_os("HOME").ok_or(AutostartError::HomeMissing)?;
    Ok(PathBuf::from(home).join(".config/systemd/user"))
}

#[cfg(target_os = "linux")]
fn unit_path(workspace: &Workspace) -> Result<PathBuf, AutostartError> {
    Ok(unit_dir()?.join(unit_name(workspace)))
}

#[cfg(any(target_os = "linux", test))]
fn current_binary() -> Result<PathBuf, AutostartError> {
    std::env::current_exe().map_err(AutostartError::CurrentExe)
}

#[cfg(any(target_os = "linux", test))]
fn render_unit(workspace: &Workspace) -> Result<String, AutostartError> {
    let canonical = std::fs::canonicalize(workspace.root()).map_err(|source| {
        AutostartError::CanonicalizeWorkspace {
            path: workspace.root().display().to_string(),
            source,
        }
    })?;
    let bin = current_binary()?;
    Ok(format!(
        "[Unit]\n\
         Description=Omakure scheduler for {workspace}\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         WorkingDirectory={workspace}\n\
         ExecStart={bin} serve\n\
         Restart=on-failure\n\
         RestartSec=5s\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        workspace = canonical.display(),
        bin = bin.display(),
    ))
}

// ---------------------------------------------------------------------------
// Linux implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn install_linux(workspace: &Workspace, json_output: bool) -> Result<(), Box<dyn Error>> {
    let dir = match unit_dir() {
        Ok(d) => d,
        Err(e) => exit_with_error(json_output, codes::INTERNAL, e.to_string()),
    };
    if let Err(e) = fs::create_dir_all(&dir) {
        exit_with_error(
            json_output,
            codes::INTERNAL,
            format!("create {}: {e}", dir.display()),
        );
    }

    let path = match unit_path(workspace) {
        Ok(p) => p,
        Err(e) => exit_with_error(json_output, codes::INTERNAL, e.to_string()),
    };
    let body = match render_unit(workspace) {
        Ok(b) => b,
        Err(e) => exit_with_error(json_output, codes::INTERNAL, e.to_string()),
    };
    if let Err(e) = fs::write(&path, &body) {
        exit_with_error(
            json_output,
            codes::INTERNAL,
            format!("write {}: {e}", path.display()),
        );
    }

    let name = unit_name(workspace);
    // daemon-reload picks up the new unit; enable --now both enables it
    // for future boots and starts it immediately. We treat systemctl
    // failures as fatal — an installed-but-unstarted unit would be
    // worse UX than a loud error.
    if let Err(e) = systemctl(&["daemon-reload"]) {
        exit_with_error(json_output, codes::INTERNAL, e.to_string());
    }
    if let Err(e) = systemctl(&["enable", "--now", &name]) {
        exit_with_error(json_output, codes::INTERNAL, e.to_string());
    }

    if json_output {
        json::print_ok(json!({
            "unit": name,
            "unit_path": path.to_string_lossy(),
            "enabled": true,
            "active": true,
        }));
    } else {
        println!("installed systemd user service: {name}");
        println!("  {}", path.display());
        println!("  tail with: journalctl --user -u {name} -f");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_linux(workspace: &Workspace, json_output: bool) -> Result<(), Box<dyn Error>> {
    let path = match unit_path(workspace) {
        Ok(p) => p,
        Err(e) => exit_with_error(json_output, codes::INTERNAL, e.to_string()),
    };
    let name = unit_name(workspace);

    if !path.exists() {
        exit_with_error(
            json_output,
            codes::DAEMON_NOT_RUNNING,
            format!("no systemd user unit installed for this workspace ({name})"),
        );
    }

    // `disable --now` stops and disables; we ignore its exit status so
    // that a half-installed unit (file present, never enabled) can still
    // be cleaned up by the subsequent remove + reload.
    let _ = systemctl(&["disable", "--now", &name]);
    if let Err(e) = fs::remove_file(&path) {
        exit_with_error(
            json_output,
            codes::INTERNAL,
            format!("remove {}: {e}", path.display()),
        );
    }
    let _ = systemctl(&["daemon-reload"]);

    if json_output {
        json::print_ok(json!({ "unit": name, "removed": true }));
    } else {
        println!("removed systemd user service: {name}");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn status_linux(workspace: &Workspace, json_output: bool) -> Result<(), Box<dyn Error>> {
    let name = unit_name(workspace);
    let path = match unit_path(workspace) {
        Ok(p) => p,
        Err(e) => exit_with_error(json_output, codes::INTERNAL, e.to_string()),
    };
    let installed = path.exists();
    let active = installed && systemctl_is_active(&name);
    let enabled = installed && systemctl_is_enabled(&name);

    if json_output {
        json::print_ok(json!({
            "unit": name,
            "unit_path": path.to_string_lossy(),
            "installed": installed,
            "active": active,
            "enabled": enabled,
        }));
    } else {
        println!("unit:      {name}");
        println!("path:      {}", path.display());
        println!("installed: {installed}");
        println!("active:    {active}");
        println!("enabled:   {enabled}");
        if installed {
            println!("tail with: journalctl --user -u {name} -f");
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn systemctl(args: &[&str]) -> Result<(), AutostartError> {
    let owned_args = || args.iter().map(|arg| (*arg).to_string()).collect();
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|source| AutostartError::SystemctlIo {
            args: owned_args(),
            source,
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AutostartError::SystemctlFailed {
            args: owned_args(),
            stderr: stderr.trim().to_string(),
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn systemctl_is_active(name: &str) -> bool {
    Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", name])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn systemctl_is_enabled(name: &str) -> bool {
    Command::new("systemctl")
        .args(["--user", "is-enabled", "--quiet", name])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Error helper
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn unit_name_is_stable_for_same_path() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        let a = unit_name(&ws);
        let b = unit_name(&ws);
        assert_eq!(a, b);
        assert!(a.starts_with("omakure-"));
        assert!(a.ends_with(".service"));
    }

    #[test]
    fn unit_name_differs_per_workspace() {
        let tmp_a = TempDir::new().unwrap();
        let tmp_b = TempDir::new().unwrap();
        let a = unit_name(&Workspace::new(tmp_a.path().to_path_buf()));
        let b = unit_name(&Workspace::new(tmp_b.path().to_path_buf()));
        assert_ne!(a, b, "distinct workspace paths must map to distinct units");
    }

    #[test]
    fn render_unit_contains_workspace_and_binary() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::new(tmp.path().to_path_buf());
        let body = render_unit(&ws).unwrap();
        let canonical = std::fs::canonicalize(tmp.path()).unwrap();
        assert!(body.contains(&format!("WorkingDirectory={}", canonical.display())));
        assert!(body.contains("ExecStart="));
        assert!(body.contains(" serve\n"));
        assert!(body.contains("[Install]"));
        assert!(body.contains("WantedBy=default.target"));
    }

    #[test]
    fn render_unit_preserves_canonicalization_error_context() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("missing");
        let workspace = Workspace::new(missing.clone());
        let source = std::fs::canonicalize(&missing).unwrap_err();
        let error = render_unit(&workspace).unwrap_err();
        assert!(matches!(
            error,
            AutostartError::CanonicalizeWorkspace { .. }
        ));
        assert_eq!(
            error.to_string(),
            format!("canonicalize workspace {}: {source}", missing.display())
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn systemctl_errors_preserve_command_and_stderr_text() {
        let args = vec!["enable".to_string(), "--now".to_string()];
        let io_error = AutostartError::SystemctlIo {
            args: args.clone(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing executable"),
        };
        assert_eq!(
            io_error.to_string(),
            "systemctl --user [\"enable\", \"--now\"]: missing executable"
        );
        let failure = AutostartError::SystemctlFailed {
            args,
            stderr: "unit missing".to_string(),
        };
        assert_eq!(
            failure.to_string(),
            "systemctl --user [\"enable\", \"--now\"] failed: unit missing"
        );
    }
}

//! `omakure serve --install` / `--uninstall` / `--status`.
//!
//! Installs a per-workspace systemd user service so the scheduler
//! daemon survives reboots without requiring the user to wire up
//! init scripts by hand. Each workspace gets its own unit, uniquely
//! named by a stable hash of the canonical workspace path.
//!
//! Linux-only. Other platforms return `not_implemented`; we avoid
//! half-working stubs (e.g. a macOS launchd plist that silently
//! leaks on uninstall) until there is a real need.

use crate::cli::json::{self, codes};
use crate::workspace::Workspace;
use serde_json::json;
use std::error::Error;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::process::Command;

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
    emit_error(
        json_output,
        codes::NOT_IMPLEMENTED,
        "serve --install is only supported on Linux (systemd user units). \
         On macOS/Windows, wire the daemon up with your platform's service manager.",
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
fn path_hash(path: &Path) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub(crate) fn unit_name(workspace: &Workspace) -> String {
    let canonical = std::fs::canonicalize(workspace.root())
        .unwrap_or_else(|_| workspace.root().to_path_buf());
    format!("omakure-{:016x}.service", path_hash(&canonical))
}

fn unit_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    Ok(PathBuf::from(home).join(".config/systemd/user"))
}

fn unit_path(workspace: &Workspace) -> Result<PathBuf, String> {
    Ok(unit_dir()?.join(unit_name(workspace)))
}

fn current_binary() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("current_exe: {e}"))
}

fn render_unit(workspace: &Workspace) -> Result<String, String> {
    let canonical = std::fs::canonicalize(workspace.root())
        .map_err(|e| format!("canonicalize workspace {}: {e}", workspace.root().display()))?;
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
        Err(e) => return emit_error(json_output, codes::INTERNAL, e),
    };
    if let Err(e) = fs::create_dir_all(&dir) {
        return emit_error(
            json_output,
            codes::INTERNAL,
            format!("create {}: {e}", dir.display()),
        );
    }

    let path = match unit_path(workspace) {
        Ok(p) => p,
        Err(e) => return emit_error(json_output, codes::INTERNAL, e),
    };
    let body = match render_unit(workspace) {
        Ok(b) => b,
        Err(e) => return emit_error(json_output, codes::INTERNAL, e),
    };
    if let Err(e) = fs::write(&path, &body) {
        return emit_error(
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
        return emit_error(json_output, codes::INTERNAL, e);
    }
    if let Err(e) = systemctl(&["enable", "--now", &name]) {
        return emit_error(json_output, codes::INTERNAL, e);
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
        Err(e) => return emit_error(json_output, codes::INTERNAL, e),
    };
    let name = unit_name(workspace);

    if !path.exists() {
        return emit_error(
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
        return emit_error(
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
        Err(e) => return emit_error(json_output, codes::INTERNAL, e),
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
fn systemctl(args: &[&str]) -> Result<(), String> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|e| format!("systemctl --user {args:?}: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "systemctl --user {args:?} failed: {}",
            stderr.trim()
        ));
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
}

use crate::cli::args::UninstallArgs;
use crate::util::ps_quote;
use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use winreg::enums::HKEY_CURRENT_USER;
#[cfg(windows)]
use winreg::RegKey;

pub fn run(scripts_dir: PathBuf, options: UninstallArgs) -> Result<(), Box<dyn Error>> {
    let exe = env::current_exe()?;

    if cfg!(windows) {
        uninstall_windows(&exe)?;
    } else {
        uninstall_unix(&exe)?;
    }

    if options.scripts {
        if scripts_dir.exists() {
            std::fs::remove_dir_all(&scripts_dir)?;
            println!("Removed scripts folder: {}", scripts_dir.display());
        } else {
            println!("Scripts folder not found: {}", scripts_dir.display());
        }
    }

    Ok(())
}

fn uninstall_unix(exe: &Path) -> Result<(), Box<dyn Error>> {
    match std::fs::remove_file(exe) {
        Ok(()) => println!("Removed {}", exe.display()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            println!("Binary already removed: {}", exe.display())
        }
        Err(err) => return Err(err.into()),
    }

    Ok(())
}

fn uninstall_windows(exe: &Path) -> Result<(), Box<dyn Error>> {
    let install_dir = exe
        .parent()
        .ok_or("Unable to determine install directory")?;

    #[cfg(windows)]
    {
        remove_from_user_path(install_dir)?;
    }

    #[cfg(not(windows))]
    {
        let _ = install_dir;
    }

    let script = build_windows_uninstall_script(exe, install_dir, std::process::id());

    Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .spawn()?;

    println!("Uninstall will finish after this process exits.");
    Ok(())
}

fn build_windows_uninstall_script(exe: &Path, install_dir: &Path, pid: u32) -> String {
    format!(
        r#"$processId = {pid}
try {{
  $p = Get-Process -Id $processId -ErrorAction SilentlyContinue
  if ($p) {{ $p.WaitForExit() }}
}} catch {{}}

$target = {target}
if (Test-Path -LiteralPath $target) {{
  Remove-Item -LiteralPath $target -Force
}}

$installDir = {install_dir}
if (Test-Path -LiteralPath $installDir) {{
  $items = Get-ChildItem -LiteralPath $installDir -Force -ErrorAction SilentlyContinue
  if (-not $items) {{ Remove-Item -LiteralPath $installDir -Force }}
}}

$rootDir = Split-Path -Parent $installDir
if (Test-Path -LiteralPath $rootDir) {{
  $items = Get-ChildItem -LiteralPath $rootDir -Force -ErrorAction SilentlyContinue
  if (-not $items) {{ Remove-Item -LiteralPath $rootDir -Force }}
}}
"#,
        pid = pid,
        target = ps_quote(&exe.display().to_string()),
        install_dir = ps_quote(&install_dir.display().to_string())
    )
}

#[cfg(windows)]
fn remove_from_user_path(install_dir: &Path) -> Result<bool, Box<dyn Error>> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (env_key, _) = hkcu.create_subkey("Environment")?;
    let current: String = env_key.get_value("Path").unwrap_or_default();
    if current.trim().is_empty() {
        return Ok(false);
    }

    let mut remove_candidates = Vec::new();
    remove_candidates.push(normalize_path(&install_dir.to_string_lossy()));
    if let Ok(local) = env::var("LOCALAPPDATA") {
        remove_candidates.push(normalize_path(
            &PathBuf::from(local)
                .join("omakure")
                .join("bin")
                .to_string_lossy(),
        ));
    } else if let Ok(profile) = env::var("USERPROFILE") {
        remove_candidates.push(normalize_path(
            &PathBuf::from(profile)
                .join("AppData")
                .join("Local")
                .join("omakure")
                .join("bin")
                .to_string_lossy(),
        ));
    }

    remove_candidates.retain(|value| !value.is_empty());
    remove_candidates.sort();
    remove_candidates.dedup();

    let mut kept = Vec::new();
    for entry in current.split(';') {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = normalize_path(trimmed);
        let remove =
            remove_candidates.contains(&normalized) || normalized.ends_with("\\omakure\\bin");
        if !remove {
            kept.push(trimmed.to_string());
        }
    }

    let new_value = kept.join(";");
    if new_value != current {
        env_key.set_value("Path", &new_value)?;
        return Ok(true);
    }

    Ok(false)
}

#[cfg(windows)]
fn normalize_path(path: &str) -> String {
    let trimmed = path.trim().trim_matches('"');
    let trimmed = trimmed.trim_end_matches('\\').trim_end_matches('/');
    let replaced = trimmed.replace('/', "\\");
    collapse_backslashes(&replaced).to_lowercase()
}

#[cfg(windows)]
fn collapse_backslashes(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }

    let mut output = String::with_capacity(path.len());
    let mut chars = path.chars();
    let mut last_was_backslash = false;

    if path.starts_with("\\\\") {
        output.push('\\');
        output.push('\\');
        chars.next();
        chars.next();
        last_was_backslash = true;
    }

    for ch in chars {
        if ch == '\\' {
            if !last_was_backslash {
                output.push('\\');
            }
            last_was_backslash = true;
        } else {
            output.push(ch);
            last_was_backslash = false;
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn uninstall_unix_removes_existing_binary() {
        let tmp = TempDir::new().unwrap();
        let exe = tmp.path().join("omakure");
        std::fs::write(&exe, "bin").unwrap();

        uninstall_unix(&exe).unwrap();

        assert!(!exe.exists());
    }

    #[test]
    fn uninstall_unix_allows_missing_binary() {
        let tmp = TempDir::new().unwrap();
        let exe = tmp.path().join("missing-omakure");

        uninstall_unix(&exe).unwrap();

        assert!(!exe.exists());
    }

    #[test]
    fn build_windows_uninstall_script_contains_expected_targets() {
        let exe = PathBuf::from("/tmp/omakure/bin/omakure.exe");
        let install_dir = exe.parent().unwrap();

        let script = build_windows_uninstall_script(&exe, install_dir, 4242);

        assert!(script.contains("$processId = 4242"));
        assert!(script.contains("Remove-Item -LiteralPath $target -Force"));
        assert!(script.contains("$rootDir = Split-Path -Parent $installDir"));
        assert!(script.contains("omakure.exe"));
        assert!(script.contains("/tmp/omakure/bin"));
    }
}

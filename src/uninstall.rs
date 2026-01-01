use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use winreg::enums::HKEY_CURRENT_USER;
#[cfg(windows)]
use winreg::RegKey;

pub struct UninstallOptions {
    pub scripts_dir: PathBuf,
    pub remove_scripts: bool,
}

pub fn print_uninstall_help() {
    println!(
        "Usage: omakure uninstall [--scripts]\n\n\
Options:\n\
  --scripts   Remove the scripts directory as well\n\n\
Environment:\n\
  OMAKURE_SCRIPTS_DIR  Scripts directory override\n\
  OVERTURE_SCRIPTS_DIR  Legacy scripts directory override\n\
  CLOUD_MGMT_SCRIPTS_DIR  Legacy scripts directory override"
    );
}

pub fn parse_uninstall_args(
    args: &[String],
    scripts_dir: PathBuf,
) -> Result<UninstallOptions, Box<dyn Error>> {
    let mut options = UninstallOptions {
        scripts_dir,
        remove_scripts: false,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--scripts" => {
                options.remove_scripts = true;
                i += 1;
            }
            unknown => {
                return Err(format!("Unknown uninstall arg: {}", unknown).into());
            }
        }
    }

    Ok(options)
}

pub fn run_uninstall(options: UninstallOptions) -> Result<(), Box<dyn Error>> {
    let exe = env::current_exe()?;

    if cfg!(windows) {
        uninstall_windows(&exe)?;
    } else {
        uninstall_unix(&exe)?;
    }

    if options.remove_scripts {
        if options.scripts_dir.exists() {
            std::fs::remove_dir_all(&options.scripts_dir)?;
            println!("Removed scripts folder: {}", options.scripts_dir.display());
        } else {
            println!("Scripts folder not found: {}", options.scripts_dir.display());
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

    let script = format!(
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
        pid = std::process::id(),
        target = ps_quote(&exe.display().to_string()),
        install_dir = ps_quote(&install_dir.display().to_string())
    );

    Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .spawn()?;

    println!("Uninstall will finish after this process exits.");
    Ok(())
}

fn ps_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "''"))
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
            &PathBuf::from(local).join("omakure").join("bin").to_string_lossy(),
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
        let remove = remove_candidates.contains(&normalized)
            || normalized.ends_with("\\omakure\\bin");
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
    trimmed.replace('/', "\\").to_lowercase()
}

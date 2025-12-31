use serde_json::Value;
use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_REPO: &str = "This-Is-NPC/omakure";

pub struct UpdateOptions {
    pub repo: String,
    pub version: Option<String>,
    pub scripts_dir: PathBuf,
}

pub fn print_update_help() {
    println!(
        "Usage: omakure update [--repo owner/name] [--version vX.Y.Z]\n\n\
Options:\n\
  --repo     GitHub repository (default: This-Is-NPC/omakure)\n\
  --version  Release tag (defaults to latest)\n\n\
Environment:\n\
  REPO     GitHub repository (same as --repo)\n\
  VERSION  Release tag (same as --version)\n\
  OMAKURE_REPO  Override repo without clobbering REPO\n\
  OMAKURE_SCRIPTS_DIR  Scripts directory override\n\
  OVERTURE_REPO  Legacy repo override\n\
  OVERTURE_SCRIPTS_DIR  Legacy scripts directory override\n\
  CLOUD_MGMT_REPO  Legacy repo override\n\
  CLOUD_MGMT_SCRIPTS_DIR  Legacy scripts directory override"
    );
}

pub fn parse_update_args(
    args: &[String],
    scripts_dir: PathBuf,
) -> Result<UpdateOptions, Box<dyn Error>> {
    let repo = env::var("OMAKURE_REPO")
        .or_else(|_| env::var("OVERTURE_REPO"))
        .or_else(|_| env::var("CLOUD_MGMT_REPO"))
        .or_else(|_| env::var("REPO"))
        .unwrap_or_else(|_| DEFAULT_REPO.to_string());
    let mut opts = UpdateOptions {
        repo,
        version: env::var("VERSION").ok(),
        scripts_dir,
    };

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--repo" => {
                let value = args.get(i + 1).ok_or("Missing value for --repo")?;
                opts.repo = value.to_string();
                i += 2;
            }
            "--version" => {
                let value = args.get(i + 1).ok_or("Missing value for --version")?;
                opts.version = Some(value.to_string());
                i += 2;
            }
            unknown => {
                return Err(format!("Unknown update arg: {}", unknown).into());
            }
        }
    }

    Ok(opts)
}

pub fn run_update(options: UpdateOptions) -> Result<(), Box<dyn Error>> {
    let repo = options.repo;
    let version = match options.version {
        Some(version) => normalize_version_tag(&version),
        None => fetch_latest_version(&repo)?,
    };

    fs::create_dir_all(&options.scripts_dir)?;

    let temp_dir = env::temp_dir().join(format!("omakure-update-{}", std::process::id()));
    fs::create_dir_all(&temp_dir)?;
    let _temp_guard = TempDirGuard::new(temp_dir.clone());

    let current_version = env!("CARGO_PKG_VERSION");
    let target_version = version.trim_start_matches('v');
    let should_update = target_version != current_version;

    if should_update {
        let asset = release_asset(&version)?;
        let url = format!(
            "https://github.com/{}/releases/download/{}/{}",
            repo, version, asset
        );
        let archive_path = temp_dir.join(&asset);
        download_to_path(&url, &archive_path)?;

        let extract_dir = temp_dir.join("release");
        fs::create_dir_all(&extract_dir)?;
        extract_archive(&archive_path, &extract_dir)?;

        let bin_name = if cfg!(windows) {
            "omakure.exe"
        } else {
            "omakure"
        };
        let new_bin = find_file(&extract_dir, bin_name)?;
        install_binary(&new_bin)?;
        println!("Updated omakure to {}", version);
    } else {
        println!("omakure already on {}", version);
    }

    if let Err(err) = sync_repo_scripts(&repo, &version, &options.scripts_dir, &temp_dir) {
        eprintln!("Warning: failed to sync scripts: {}", err);
    }

    Ok(())
}

fn normalize_version_tag(version: &str) -> String {
    if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{}", version)
    }
}

fn fetch_latest_version(repo: &str) -> Result<String, Box<dyn Error>> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo);
    let json = download_string(&url)?;
    let value: Value = serde_json::from_str(&json)?;
    let tag = value
        .get("tag_name")
        .and_then(|value| value.as_str())
        .ok_or("tag_name not found in release JSON")?;
    Ok(normalize_version_tag(tag))
}

fn release_asset(version: &str) -> Result<String, Box<dyn Error>> {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        return Err("Unsupported OS for update".into());
    };

    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        return Err("Unsupported architecture for update".into());
    };

    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    Ok(format!("omakure-{}-{}-{}.{}", version, os, arch, ext))
}

fn download_string(url: &str) -> Result<String, Box<dyn Error>> {
    if cfg!(windows) {
        let script = format!("(Invoke-WebRequest -Uri {}).Content", ps_quote(url));
        let output = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()?;
        if !output.status.success() {
            return Err(format!("Failed to download {}", url).into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else if command_exists("curl") {
        let output = Command::new("curl").args(["-fsSL", url]).output()?;
        if !output.status.success() {
            return Err(format!("Failed to download {}", url).into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else if command_exists("wget") {
        let output = Command::new("wget").args(["-qO-", url]).output()?;
        if !output.status.success() {
            return Err(format!("Failed to download {}", url).into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err("Missing curl or wget for update".into())
    }
}

fn download_to_path(url: &str, dest: &Path) -> Result<(), Box<dyn Error>> {
    if cfg!(windows) {
        let script = format!(
            "Invoke-WebRequest -Uri {} -OutFile {}",
            ps_quote(url),
            ps_quote(&dest.display().to_string())
        );
        let status = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()?;
        if !status.success() {
            return Err(format!("Failed to download {}", url).into());
        }
    } else if command_exists("curl") {
        let status = Command::new("curl")
            .args(["-fL", "-o", &dest.display().to_string(), url])
            .status()?;
        if !status.success() {
            return Err(format!("Failed to download {}", url).into());
        }
    } else if command_exists("wget") {
        let status = Command::new("wget")
            .args(["-q", "-O", &dest.display().to_string(), url])
            .status()?;
        if !status.success() {
            return Err(format!("Failed to download {}", url).into());
        }
    } else {
        return Err("Missing curl or wget for update".into());
    }

    Ok(())
}

fn extract_archive(archive: &Path, dest: &Path) -> Result<(), Box<dyn Error>> {
    if cfg!(windows) {
        let script = format!(
            "Expand-Archive -Path {} -DestinationPath {} -Force",
            ps_quote(&archive.display().to_string()),
            ps_quote(&dest.display().to_string())
        );
        let status = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()?;
        if !status.success() {
            return Err("Failed to extract update archive".into());
        }
    } else {
        if !command_exists("tar") {
            return Err("Missing tar for update".into());
        }
        let status = Command::new("tar")
            .args([
                "-xzf",
                &archive.display().to_string(),
                "-C",
                &dest.display().to_string(),
            ])
            .status()?;
        if !status.success() {
            return Err("Failed to extract update archive".into());
        }
    }

    Ok(())
}

fn install_binary(new_bin: &Path) -> Result<(), Box<dyn Error>> {
    let target = env::current_exe()?;
    if cfg!(windows) {
        install_binary_windows(new_bin, &target)?;
    } else {
        install_binary_unix(new_bin, &target)?;
    }
    Ok(())
}

fn install_binary_unix(new_bin: &Path, target: &Path) -> Result<(), Box<dyn Error>> {
    let target_dir = target
        .parent()
        .ok_or("Unable to determine install directory")?;
    let file_name = target
        .file_name()
        .ok_or("Unable to determine binary name")?
        .to_string_lossy()
        .to_string();
    let temp_target = target_dir.join(format!("{}.new", file_name));

    fs::copy(new_bin, &temp_target)?;
    set_executable_permissions(&temp_target)?;

    match fs::rename(&temp_target, target) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(&temp_target, target)?;
            set_executable_permissions(target)?;
            let _ = fs::remove_file(&temp_target);
            Ok(())
        }
    }
}

fn install_binary_windows(new_bin: &Path, target: &Path) -> Result<(), Box<dyn Error>> {
    let target_dir = target
        .parent()
        .ok_or("Unable to determine install directory")?;
    let stem = target
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or("Unable to determine binary name")?;
    let ext = target.extension().and_then(OsStr::to_str).unwrap_or("exe");
    let new_path = target_dir.join(format!("{}.new.{}", stem, ext));
    let backup_path = target_dir.join(format!("{}.old.{}", stem, ext));

    if new_path.exists() {
        let _ = fs::remove_file(&new_path);
    }
    fs::copy(new_bin, &new_path)?;

    let script = format!(
        "$pid = {pid}; \
         try {{ $p = Get-Process -Id $pid -ErrorAction SilentlyContinue; if ($p) {{ $p.WaitForExit(); }} }} catch {{}}; \
         if (Test-Path {target}) {{ Move-Item -Force {target} {backup}; }} \
         Move-Item -Force {new_path} {target}; \
         if (Test-Path {backup}) {{ Remove-Item -Force {backup}; }}",
        pid = std::process::id(),
        target = ps_quote(&target.display().to_string()),
        new_path = ps_quote(&new_path.display().to_string()),
        backup = ps_quote(&backup_path.display().to_string())
    );

    Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .spawn()?;

    println!("Update will finish after this process exits.");
    Ok(())
}

fn sync_repo_scripts(
    repo: &str,
    version: &str,
    scripts_dir: &Path,
    work_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    let source_url = if cfg!(windows) {
        format!(
            "https://github.com/{}/archive/refs/tags/{}.zip",
            repo, version
        )
    } else {
        format!(
            "https://github.com/{}/archive/refs/tags/{}.tar.gz",
            repo, version
        )
    };

    let source_archive = if cfg!(windows) {
        work_dir.join("omakure-source.zip")
    } else {
        work_dir.join("omakure-source.tar.gz")
    };

    download_to_path(&source_url, &source_archive)?;

    let source_root = work_dir.join("source");
    fs::create_dir_all(&source_root)?;
    extract_archive(&source_archive, &source_root)?;

    let scripts_src = find_dir_named(&source_root, "scripts")
        .ok_or("scripts folder not found in source archive")?;
    let (copied, skipped) = copy_missing_files(&scripts_src, scripts_dir)?;

    if copied > 0 {
        println!("Copied {} script(s) to {}", copied, scripts_dir.display());
    } else if skipped > 0 {
        println!("Scripts already up to date in {}", scripts_dir.display());
    }

    Ok(())
}

fn copy_missing_files(src_dir: &Path, dest_dir: &Path) -> Result<(usize, usize), Box<dyn Error>> {
    let mut stack = vec![src_dir.to_path_buf()];
    let mut copied = 0;
    let mut skipped = 0;

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(src_dir)
                .map_err(|_| "Failed to compute script path")?;
            let target = dest_dir.join(rel);
            if target.exists() {
                skipped += 1;
                continue;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&path, &target)?;
            copied += 1;
        }
    }

    Ok((copied, skipped))
}

fn find_file(root: &Path, name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.file_name() == Some(OsStr::new(name)) {
                return Ok(path);
            }
        }
    }
    Err(format!("{} not found in archive", name).into())
}

fn find_dir_named(root: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).ok()?;
        for entry in entries {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() {
                if path.file_name() == Some(OsStr::new(name)) {
                    return Some(path);
                }
                stack.push(path);
            }
        }
    }
    None
}

fn command_exists(cmd: &str) -> bool {
    Command::new(cmd).arg("--version").output().is_ok()
}

#[cfg(not(windows))]
fn set_executable_permissions(path: &Path) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(windows)]
fn set_executable_permissions(_path: &Path) -> Result<(), Box<dyn Error>> {
    Ok(())
}

fn ps_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "''"))
}

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

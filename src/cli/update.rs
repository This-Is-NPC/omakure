use crate::cli::args::UpdateArgs;
use crate::util::{ps_quote, set_executable_permissions, TempDirGuard};
use serde_json::Value;
use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_REPO: &str = "This-Is-NPC/omakure";

pub fn run(scripts_dir: PathBuf, args: UpdateArgs) -> Result<(), Box<dyn Error>> {
    let repo = resolve_repo(args.repo);
    let version = match resolve_version(args.version) {
        Some(version) => normalize_version_tag(&version),
        None => fetch_latest_version(&repo)?,
    };

    fs::create_dir_all(&scripts_dir)?;

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

    if let Err(err) = sync_repo_scripts(&repo, &version, &scripts_dir, &temp_dir) {
        eprintln!("Warning: failed to sync scripts: {}", err);
    }

    Ok(())
}

fn resolve_repo(repo: Option<String>) -> String {
    repo.or_else(|| env::var("OMAKURE_REPO").ok())
        .or_else(|| env::var("OVERTURE_REPO").ok())
        .or_else(|| env::var("CLOUD_MGMT_REPO").ok())
        .or_else(|| env::var("REPO").ok())
        .unwrap_or_else(|| DEFAULT_REPO.to_string())
}

fn resolve_version(version: Option<String>) -> Option<String> {
    version.or_else(|| env::var("VERSION").ok())
}

pub(crate) fn normalize_version_tag(version: &str) -> String {
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

pub(crate) fn release_asset(version: &str) -> Result<String, Box<dyn Error>> {
    let os = if cfg!(target_os = "linux") {
        if cfg!(target_env = "musl") {
            "linux-musl"
        } else {
            "linux"
        }
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

    release_asset_for(version, os, arch)
}

fn release_asset_for(version: &str, os: &str, arch: &str) -> Result<String, Box<dyn Error>> {
    if !matches!(arch, "x86_64" | "aarch64") {
        return Err(format!("Unsupported architecture for update: {arch}").into());
    }

    let ext = match os {
        "linux" | "linux-musl" | "darwin" => "tar.gz",
        "windows" => "zip",
        _ => return Err(format!("Unsupported OS for update: {os}").into()),
    };

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
        "$processId = {pid}; \
         try {{ $p = Get-Process -Id $processId -ErrorAction SilentlyContinue; if ($p) {{ $p.WaitForExit(); }} }} catch {{}}; \
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use tempfile::TempDir;

    #[rstest]
    #[case::with_prefix("v1.2.3", "v1.2.3")]
    #[case::without_prefix("1.2.3", "v1.2.3")]
    #[case::already_prefixed("v0.1.8", "v0.1.8")]
    fn test_normalize_version_tag(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(normalize_version_tag(input), expected);
    }

    #[rstest]
    #[case("linux", "x86_64", "omakure-v0.1.8-linux-x86_64.tar.gz")]
    #[case("linux", "aarch64", "omakure-v0.1.8-linux-aarch64.tar.gz")]
    #[case("linux-musl", "x86_64", "omakure-v0.1.8-linux-musl-x86_64.tar.gz")]
    #[case("linux-musl", "aarch64", "omakure-v0.1.8-linux-musl-aarch64.tar.gz")]
    #[case("darwin", "x86_64", "omakure-v0.1.8-darwin-x86_64.tar.gz")]
    #[case("darwin", "aarch64", "omakure-v0.1.8-darwin-aarch64.tar.gz")]
    #[case("windows", "x86_64", "omakure-v0.1.8-windows-x86_64.zip")]
    #[case("windows", "aarch64", "omakure-v0.1.8-windows-aarch64.zip")]
    fn test_release_asset_selection(#[case] os: &str, #[case] arch: &str, #[case] expected: &str) {
        assert_eq!(release_asset_for("v0.1.8", os, arch).unwrap(), expected);
    }

    #[rstest]
    #[case("linux", "riscv64", "Unsupported architecture")]
    #[case("linux", "armv7", "Unsupported architecture")]
    #[case("freebsd", "x86_64", "Unsupported OS")]
    fn test_release_asset_rejects_unknown_platform_values(
        #[case] os: &str,
        #[case] arch: &str,
        #[case] expected_error: &str,
    ) {
        let error = release_asset_for("v0.1.8", os, arch).unwrap_err();
        assert!(error.to_string().contains(expected_error));
    }

    #[test]
    fn test_release_asset_uses_this_binarys_platform() {
        let asset = release_asset("v0.1.8").unwrap();
        assert!(asset.starts_with("omakure-v0.1.8-"));
        if cfg!(target_os = "linux") && cfg!(target_env = "musl") {
            assert!(asset.contains("-linux-musl-"));
        } else if cfg!(target_os = "linux") {
            assert!(asset.contains("-linux-"));
        } else if cfg!(target_os = "macos") {
            assert!(asset.contains("-darwin-"));
        } else if cfg!(target_os = "windows") {
            assert!(asset.contains("-windows-"));
        }
        if cfg!(target_arch = "x86_64") {
            assert!(asset.contains("-x86_64."));
        } else if cfg!(target_arch = "aarch64") {
            assert!(asset.contains("-aarch64."));
        }
    }

    #[test]
    fn test_resolve_repo_default() {
        env::remove_var("OMAKURE_REPO");
        env::remove_var("OVERTURE_REPO");
        env::remove_var("CLOUD_MGMT_REPO");
        env::remove_var("REPO");
        assert_eq!(resolve_repo(None), DEFAULT_REPO);
    }

    #[test]
    fn test_resolve_repo_explicit() {
        assert_eq!(resolve_repo(Some("user/repo".to_string())), "user/repo");
    }

    #[test]
    fn test_resolve_version_none() {
        env::remove_var("VERSION");
        assert_eq!(resolve_version(None), None);
    }

    #[test]
    fn test_resolve_version_explicit() {
        assert_eq!(
            resolve_version(Some("1.0.0".to_string())),
            Some("1.0.0".to_string())
        );
    }

    #[test]
    fn test_find_file_recursive() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("target.txt"), "found").unwrap();

        let result = find_file(tmp.path(), "target.txt").unwrap();
        assert_eq!(result, sub.join("target.txt"));
    }

    #[test]
    fn test_find_file_not_found() {
        let tmp = TempDir::new().unwrap();
        let result = find_file(tmp.path(), "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_find_dir_named() {
        let tmp = TempDir::new().unwrap();
        let scripts = tmp.path().join("archive/scripts");
        fs::create_dir_all(&scripts).unwrap();

        let result = find_dir_named(tmp.path(), "scripts");
        assert_eq!(result, Some(scripts));
    }

    #[test]
    fn test_copy_missing_files() {
        let src = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();

        fs::write(src.path().join("a.sh"), "script a").unwrap();
        fs::write(src.path().join("b.sh"), "script b").unwrap();
        fs::write(dest.path().join("a.sh"), "existing").unwrap();

        let (copied, skipped) = copy_missing_files(src.path(), dest.path()).unwrap();
        assert_eq!(copied, 1);
        assert_eq!(skipped, 1);
        // existing file not overwritten
        assert_eq!(
            fs::read_to_string(dest.path().join("a.sh")).unwrap(),
            "existing"
        );
        assert_eq!(
            fs::read_to_string(dest.path().join("b.sh")).unwrap(),
            "script b"
        );
    }
}

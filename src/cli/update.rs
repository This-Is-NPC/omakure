use crate::cli::args::UpdateArgs;
use crate::util::ps_quote;
use serde_json::Value;
use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(not(windows))]
use tempfile::TempDir;

const DEFAULT_REPO: &str = "This-Is-NPC/omakure";

pub fn run(_scripts_dir: PathBuf, args: UpdateArgs) -> Result<(), Box<dyn Error>> {
    let repo = resolve_repo(args.repo);
    validate_repo(&repo)?;
    let version = match resolve_version(args.version) {
        Some(version) => normalize_version_tag(&version),
        None => fetch_latest_version(&repo)?,
    };

    validate_version(&version)?;

    let current_version = env!("CARGO_PKG_VERSION");
    let target_version = version.trim_start_matches('v');
    let should_update = target_version != current_version;

    if should_update {
        let staging = update_staging_in(&env::temp_dir())?;
        let asset = release_asset(&version)?;
        let url = format!(
            "https://github.com/{}/releases/download/{}/{}",
            repo, version, asset
        );
        // Never derive a local path from a remote tag or archive entry.
        let archive_path = staging.path().join(if cfg!(windows) {
            "payload.zip"
        } else {
            "payload.tar.gz"
        });
        download_to_path(&url, &archive_path)?;

        let bin_name = if cfg!(windows) {
            "omakure.exe"
        } else {
            "omakure"
        };
        let new_bin = extract_release_binary(&archive_path, staging.path(), bin_name)?;
        install_binary(&new_bin)?;
        println!("Updated omakure to {}", version);
    } else {
        println!("omakure already on {}", version);
    }

    Ok(())
}

#[cfg(not(windows))]
type UpdateStaging = TempDir;

#[cfg(windows)]
struct UpdateStaging {
    path: PathBuf,
    keep: bool,
}

#[cfg(windows)]
impl UpdateStaging {
    fn path(&self) -> &Path {
        &self.path
    }
    fn keep(mut self) -> PathBuf {
        self.keep = true;
        self.path.clone()
    }
}

#[cfg(windows)]
impl Drop for UpdateStaging {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(windows)]
fn update_staging_in(parent: &Path) -> io::Result<UpdateStaging> {
    use rand::RngCore;
    for _ in 0..16 {
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let suffix = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = parent.join(format!(".omakure-update-{suffix}"));
        match create_private_windows_directory(&path) {
            Ok(()) => return Ok(UpdateStaging { path, keep: false }),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Unable to allocate update staging",
    ))
}

#[cfg(windows)]
fn create_private_windows_directory(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

    // Protected DACL: owner, SYSTEM and administrators only, inherited by files.
    // Apply at creation, not after exposing a directory with inherited access.
    let sddl = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let name = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: strings are NUL-terminated; Windows allocates the descriptor,
    // which remains live until CreateDirectoryW completes.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let result = CreateDirectoryW(name.as_ptr(), &attributes);
        let error = (result == 0).then(io::Error::last_os_error);
        LocalFree(descriptor);
        error.map_or(Ok(()), Err)
    }
}

#[cfg(not(windows))]
fn update_staging_in(parent: &Path) -> io::Result<UpdateStaging> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(".omakure-update-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    builder.tempdir_in(parent)
}

fn validate_repo(repo: &str) -> Result<(), Box<dyn Error>> {
    let parts: Vec<_> = repo.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || matches!(*part, "." | "..")
                || !part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        })
    {
        return Err("Update repository must be an owner/name pair".into());
    }
    Ok(())
}

fn validate_version(version: &str) -> Result<(), Box<dyn Error>> {
    if version.len() < 2
        || version.len() > 128
        || !version.starts_with('v')
        || !version
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-+".contains(&c))
    {
        return Err("Update version must be a simple version tag".into());
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

fn extract_release_binary(
    archive: &Path,
    staging: &Path,
    bin_name: &str,
) -> Result<PathBuf, Box<dyn Error>> {
    let destination = staging.join(bin_name);
    if cfg!(windows) {
        let script = format!(
            "$ErrorActionPreference = 'Stop'; \
             Add-Type -AssemblyName System.IO.Compression.FileSystem; \
             $zip = [IO.Compression.ZipFile]::OpenRead({archive}); \
             try {{ \
               if ($zip.Entries.Count -ne 1) {{ throw 'Expected one release binary' }}; \
               $entry = $zip.Entries[0]; \
               $kind = ($entry.ExternalAttributes -shr 16) -band 61440; \
               if ($entry.FullName -cne {name} -or ($kind -ne 0 -and $kind -ne 32768) \
                   -or ($entry.ExternalAttributes -band 1024) -ne 0) \
                 {{ throw 'Invalid release entry' }}; \
               $inputStream = $entry.Open(); \
               try {{ \
                 $outputStream = [IO.File]::Open({destination}, [IO.FileMode]::CreateNew); \
                 try {{ $inputStream.CopyTo($outputStream); $outputStream.Flush($true) }} \
                 finally {{ $outputStream.Dispose() }} \
               }} finally {{ $inputStream.Dispose() }} \
             }} finally {{ $zip.Dispose() }}",
            archive = ps_quote(&archive.display().to_string()),
            name = ps_quote(bin_name),
            destination = ps_quote(&destination.display().to_string()),
        );
        if !Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()?
            .success()
        {
            return Err("Failed to extract release binary".into());
        }
    } else {
        // Inspect before streaming a single member into our own file descriptor.
        // tar never receives a destination directory or writes archive paths.
        let names = Command::new("tar").arg("-tzf").arg(archive).output()?;
        let types = Command::new("tar").arg("-tvzf").arg(archive).output()?;
        if !names.status.success() || !types.status.success() {
            return Err("Failed to inspect release archive".into());
        }
        validate_tar_listing(&names.stdout, &types.stdout, bin_name)?;
        let output = create_binary_file(&destination)?;
        let status = Command::new("tar")
            .arg("-xOzf")
            .arg(archive)
            .args(["--", bin_name])
            .stdout(Stdio::from(output.try_clone()?))
            .status()?;
        if !status.success() {
            return Err("Failed to extract release binary".into());
        }
        output.sync_all()?;
    }
    if regular_file_metadata(&destination)?.len() == 0 {
        return Err("Release binary is empty".into());
    }
    Ok(destination)
}

fn validate_tar_listing(names: &[u8], types: &[u8], expected: &str) -> Result<(), Box<dyn Error>> {
    let names = std::str::from_utf8(names)?;
    let types = std::str::from_utf8(types)?;
    let mut entries = names.lines();
    let mut details = types.lines();
    if entries.next() != Some(expected)
        || entries.next().is_some()
        || !details.next().is_some_and(|line| line.starts_with('-'))
        || details.next().is_some()
    {
        return Err("Release archive must contain only the regular release binary".into());
    }
    Ok(())
}

fn regular_file_metadata(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected a regular binary file",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Reparse points are not binaries",
            ));
        }
    }
    Ok(metadata)
}

fn create_binary_file(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o755);
    }
    options.open(path)
}

fn stage_install_binary(
    new_bin: &Path,
    target: &Path,
) -> Result<(UpdateStaging, PathBuf), Box<dyn Error>> {
    regular_file_metadata(target)?;
    regular_file_metadata(new_bin)?;
    let mut source_options = fs::OpenOptions::new();
    source_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        source_options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut source = source_options.open(new_bin)?;
    if !source.metadata()?.is_file() || source.metadata()?.len() == 0 {
        return Err("Expected a nonempty regular release binary".into());
    }
    let staging = update_staging_in(
        target
            .parent()
            .ok_or("Unable to determine install directory")?,
    )?;
    let staged = staging.path().join("replacement");
    let mut destination = create_binary_file(&staged)?;
    io::copy(&mut source, &mut destination)?;
    destination.flush()?;
    destination.sync_all()?;
    // Close all writable descriptors before rename/exec (also on overlayfs).
    drop(destination);
    Ok((staging, staged))
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
    let (_staging, staged) = stage_install_binary(new_bin, target)?;
    // Same-filesystem atomic replacement. Never fall back to truncating the
    // live executable; on failure the old binary must remain usable.
    fs::rename(staged, target)?;
    Ok(())
}

fn windows_replace_script(staging: &Path, staged: &Path, target: &Path, pid: u32) -> String {
    format!(
        "$ErrorActionPreference = 'Stop'; \
         $processId = {pid}; \
         $p = Get-Process -Id $processId -ErrorAction SilentlyContinue; \
         if ($p) {{ $p.WaitForExit() }}; \
         try {{ [IO.File]::Replace({staged}, {target}, {backup}) }} \
         catch {{ Write-Error ('Update failed; recovery files preserved in ' + {staging}); exit 1 }}; \
         Remove-Item -LiteralPath {staging} -Recurse -Force",
        staged = ps_quote(&staged.display().to_string()),
        target = ps_quote(&target.display().to_string()),
        backup = ps_quote(&staging.join("backup").display().to_string()),
        staging = ps_quote(&staging.display().to_string()),
    )
}

fn install_binary_windows(new_bin: &Path, target: &Path) -> Result<(), Box<dyn Error>> {
    let (staging, staged) = stage_install_binary(new_bin, target)?;
    let script = windows_replace_script(staging.path(), &staged, target, std::process::id());
    Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .spawn()?;
    // Ownership transfers only after successful spawn. The child waits for
    // this process to exit before replacing the executable and cleaning up.
    let _ = staging.keep();
    println!("Update will finish after this process exits.");
    Ok(())
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
    fn security_update_validates_remote_identifiers() {
        assert!(validate_repo("This-Is-NPC/omakure").is_ok());
        for repo in ["../omakure", "/omakure", "a/b/c", "a/b?x", "a/b\n"] {
            assert!(validate_repo(repo).is_err(), "{repo:?}");
        }
        assert!(validate_version("v0.4.4-rc.1+build").is_ok());
        for version in ["v", "v../../escape", "v1?x", "v1\n", "1.0"] {
            assert!(validate_version(version).is_err(), "{version:?}");
        }
    }

    #[test]
    fn security_update_same_version_never_creates_or_syncs_workspace() {
        let root = TempDir::new().unwrap();
        let workspace = root.path().join("not-created");
        run(
            workspace.clone(),
            UpdateArgs {
                repo: Some(DEFAULT_REPO.into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            },
        )
        .unwrap();
        assert!(!workspace.exists());
    }

    #[test]
    fn security_update_private_staging_ignores_predictable_directory() {
        let parent = TempDir::new().unwrap();
        let planted = parent
            .path()
            .join(format!("omakure-update-{}", std::process::id()));
        fs::create_dir(&planted).unwrap();
        fs::write(planted.join("marker"), "untouched").unwrap();
        let first = update_staging_in(parent.path()).unwrap();
        let second = update_staging_in(parent.path()).unwrap();
        assert_ne!(first.path(), second.path());
        assert_ne!(first.path(), planted);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(first.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let owned = first.path().to_owned();
        drop(first);
        assert!(!owned.exists());
        assert_eq!(
            fs::read_to_string(planted.join("marker")).unwrap(),
            "untouched"
        );
    }

    #[test]
    fn security_update_tar_requires_exactly_one_regular_binary() {
        assert!(
            validate_tar_listing(b"omakure\n", b"-rwxr-xr-x owner 4 omakure\n", "omakure").is_ok()
        );
        for names in [
            &b"../omakure\n"[..],
            &b"nested/omakure\n"[..],
            &b"/omakure\n"[..],
            &b"omakure\nextra\n"[..],
            &b"omakure\nomakure\n"[..],
            &b""[..],
        ] {
            assert!(validate_tar_listing(names, b"-rwxr-xr-x\n", "omakure").is_err());
        }
        for kind in *b"lhdbcp" {
            assert!(validate_tar_listing(b"omakure\n", &[kind, b'\n'], "omakure").is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn security_update_extracts_only_regular_release_member() {
        use std::os::unix::fs::symlink;
        let root = TempDir::new().unwrap();
        let source = root.path().join("source");
        fs::create_dir(&source).unwrap();
        let binary = source.join("omakure");
        fs::write(&binary, b"release payload").unwrap();
        let archive = root.path().join("payload.tar.gz");
        let pack = || {
            assert!(Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(&source)
                .arg("omakure")
                .status()
                .unwrap()
                .success());
        };
        pack();
        let staging = update_staging_in(root.path()).unwrap();
        let extracted = extract_release_binary(&archive, staging.path(), "omakure").unwrap();
        assert_eq!(fs::read(extracted).unwrap(), b"release payload");
        fs::remove_file(&binary).unwrap();
        symlink("../outside", &binary).unwrap();
        pack();
        let staging = update_staging_in(root.path()).unwrap();
        assert!(extract_release_binary(&archive, staging.path(), "omakure").is_err());
        assert!(!staging.path().join("omakure").exists());
    }

    #[cfg(unix)]
    #[test]
    fn security_update_install_ignores_fixed_new_symlink_and_executes() {
        use std::os::unix::fs::symlink;
        let root = TempDir::new().unwrap();
        let target = root.path().join("omakure");
        let source = root.path().join("release");
        let victim = root.path().join("victim");
        fs::write(&target, "old binary").unwrap();
        fs::write(&source, "#!/bin/sh\nexit 0\n").unwrap();
        fs::write(&victim, "untouched").unwrap();
        symlink(&victim, root.path().join("omakure.new")).unwrap();
        install_binary_unix(&source, &target).unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "untouched");
        assert!(fs::symlink_metadata(root.path().join("omakure.new"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_updated_binary_executes(&target);
        assert!(!fs::read_dir(root.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".omakure-update-")));
    }

    #[cfg(unix)]
    fn assert_updated_binary_executes(target: &Path) {
        // Parallel tests may fork while the staging descriptor is open and
        // temporarily inherit it until their exec. As in dependency fixtures,
        // tolerate only that bounded kernel ETXTBSY window, never other errors.
        // A descriptor leaked by installation still fails at the deadline.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match Command::new(target).status() {
                Ok(status) => {
                    assert!(status.success());
                    return;
                }
                Err(error)
                    if error.kind() == io::ErrorKind::ExecutableFileBusy
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("updated binary cannot execute: {error}"),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn security_update_rejects_symlink_sources_and_targets_without_mutation() {
        use std::os::unix::fs::symlink;
        let root = TempDir::new().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        let link = root.path().join("link");
        fs::write(&source, "new").unwrap();
        fs::write(&target, "old").unwrap();
        symlink(&target, &link).unwrap();
        assert!(install_binary_unix(&source, &link).is_err());
        assert!(install_binary_unix(&link, &target).is_err());
        fs::write(&source, "").unwrap();
        assert!(install_binary_unix(&source, &target).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "old");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 3);
    }

    // Native Windows checks run in the platform test suite.
    #[cfg(windows)]
    #[test]
    fn security_update_windows_extracts_only_single_release_binary() {
        let root = TempDir::new().unwrap();
        let archive = root.path().join("payload.zip");
        let script = format!(
            "$ErrorActionPreference = 'Stop'; \
             Add-Type -AssemblyName System.IO.Compression.FileSystem; \
             $zip = [IO.Compression.ZipFile]::Open({}, [IO.Compression.ZipArchiveMode]::Create); \
             try {{ $entry = $zip.CreateEntry('omakure.exe'); $writer = [IO.StreamWriter]::new($entry.Open()); \
               try {{ $writer.Write('release payload') }} finally {{ $writer.Dispose() }} \
             }} finally {{ $zip.Dispose() }}",
            ps_quote(&archive.display().to_string()),
        );
        assert!(Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()
            .unwrap()
            .success());
        let staging = update_staging_in(root.path()).unwrap();
        let extracted = extract_release_binary(&archive, staging.path(), "omakure.exe").unwrap();
        assert_eq!(fs::read_to_string(extracted).unwrap(), "release payload");
        let script = format!(
            "$ErrorActionPreference = 'Stop'; \
             Add-Type -AssemblyName System.IO.Compression.FileSystem; \
             $zip = [IO.Compression.ZipFile]::Open({}, [IO.Compression.ZipArchiveMode]::Update); \
             try {{ $null = $zip.CreateEntry('../escape') }} finally {{ $zip.Dispose() }}",
            ps_quote(&archive.display().to_string()),
        );
        assert!(Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()
            .unwrap()
            .success());
        let rejected = update_staging_in(root.path()).unwrap();
        assert!(extract_release_binary(&archive, rejected.path(), "omakure.exe").is_err());
        assert!(!root.path().join("escape").exists());
        assert!(!rejected.path().join("omakure.exe").exists());
    }

    #[cfg(windows)]
    #[test]
    fn security_update_windows_native_replace_retains_recovery_on_failure() {
        let root = TempDir::new().unwrap();
        let staging = update_staging_in(root.path()).unwrap();
        let target = root.path().join("omakure.exe");
        let staged = staging.path().join("replacement");
        let backup = staging.path().join("backup");
        fs::write(&staged, "new").unwrap();
        fs::write(&backup, "recovery marker").unwrap();
        // Missing target forces a real File.Replace failure. Use a PID outside
        // the normal process range without overflowing PowerShell's Int32.
        let script = windows_replace_script(staging.path(), &staged, &target, i32::MAX as u32);
        assert!(!Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()
            .unwrap()
            .success());
        assert_eq!(fs::read_to_string(&backup).unwrap(), "recovery marker");
        assert_eq!(fs::read_to_string(&staged).unwrap(), "new");
        fs::remove_file(&backup).unwrap();
        fs::write(&target, "old").unwrap();
        assert!(Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()
            .unwrap()
            .success());
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert!(!staging.path().exists());
    }

    #[cfg(windows)]
    #[test]
    fn security_update_windows_staging_acl_is_protected_and_collision_safe() {
        let root = TempDir::new().unwrap();
        let staging = update_staging_in(root.path()).unwrap();
        fs::write(staging.path().join("marker"), "owned").unwrap();
        assert_eq!(
            create_private_windows_directory(staging.path())
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        let script = format!(
            "$ErrorActionPreference = 'Stop'; $acl = Get-Acl -LiteralPath {}; \
             if (-not $acl.AreAccessRulesProtected) {{ exit 1 }}; \
             foreach ($rule in $acl.Access) {{ \
               $sid = $rule.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value; \
               if ($sid -notin @('S-1-3-4', 'S-1-5-18', 'S-1-5-32-544')) {{ exit 2 }} \
             }}",
            ps_quote(&staging.path().display().to_string()),
        );
        assert!(Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()
            .unwrap()
            .success());
        assert_eq!(
            fs::read_to_string(staging.path().join("marker")).unwrap(),
            "owned"
        );
    }

    #[test]
    fn security_update_windows_replacement_uses_private_backup_and_literal_cleanup() {
        let script = windows_replace_script(
            Path::new("C:/owned's staging"),
            Path::new("C:/owned's staging/replacement"),
            Path::new("C:/bin/omakure.exe"),
            123,
        );
        assert!(script.contains("[IO.File]::Replace("));
        assert!(script.contains("'C:/owned''s staging/replacement'"));
        assert!(script.contains(&ps_quote(
            &Path::new("C:/owned's staging")
                .join("backup")
                .display()
                .to_string()
        )));
        assert!(script.contains("Remove-Item -LiteralPath 'C:/owned''s staging'"));
        assert!(!script.contains("Move-Item"));
        assert!(!script.contains("finally"));
        assert!(script.contains("recovery files preserved"));
        assert!(script.find("WaitForExit").unwrap() < script.find("[IO.File]::Replace").unwrap());
    }
}

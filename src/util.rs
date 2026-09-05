use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "linux")]
use std::sync::OnceLock;

/// Set executable permissions on Unix systems (no-op on Windows).
#[cfg(not(windows))]
pub fn set_executable_permissions(path: &Path) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(windows)]
pub fn set_executable_permissions(_path: &Path) -> Result<(), Box<dyn Error>> {
    Ok(())
}

/// Quote a string for use in PowerShell commands.
pub fn ps_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "''"))
}

/// Parent directory for generated executables (askpass scripts, test shims).
///
/// On Linux, uses the first *executable* staging root (`XDG_RUNTIME_DIR`, `/dev/shm`,
/// `TMPDIR`, then the host temp directory). A root may still be overlay-backed if every
/// tmpfs is `noexec` (fail-open vs `EACCES`).
/// On other platforms, uses the host temp directory.
pub fn generated_executable_tempdir() -> io::Result<tempfile::TempDir> {
    #[cfg(target_os = "linux")]
    {
        tempfile::TempDir::new_in(linux_executable_staging_root()?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        tempfile::tempdir()
    }
}

#[cfg(target_os = "linux")]
const ETXTBSY: i32 = 26;

#[cfg(target_os = "linux")]
fn staging_root_allows_exec(dir: &Path) -> bool {
    let probe = dir.join(format!(
        ".omakure-exec-probe-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let ok = match write_generated_executable(&probe, b"#!/bin/sh\nexit 0\n") {
        Ok(()) => match Command::new(&probe).status() {
            Ok(status) => status.success(),
            Err(err) if err.raw_os_error() == Some(ETXTBSY) => true,
            Err(_) => false,
        },
        Err(_) => false,
    };
    let _ = fs::remove_file(&probe);
    ok
}

#[cfg(target_os = "linux")]
fn linux_staging_roots() -> Vec<PathBuf> {
    let candidates = [
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        Some(PathBuf::from("/dev/shm")),
        std::env::var_os("TMPDIR").map(PathBuf::from),
        Some(std::env::temp_dir()),
    ];
    let mut roots = Vec::new();
    for candidate in candidates.into_iter().flatten() {
        if !candidate.is_dir() {
            continue;
        }
        let duplicate = roots.iter().any(|existing| {
            fs::canonicalize(existing).ok() == fs::canonicalize(&candidate).ok()
                || existing == &candidate
        });
        if !duplicate {
            roots.push(candidate);
        }
    }
    roots
}

#[cfg(target_os = "linux")]
static LINUX_EXECUTABLE_STAGING_ROOT: OnceLock<PathBuf> = OnceLock::new();

#[cfg(target_os = "linux")]
fn linux_executable_staging_root() -> io::Result<PathBuf> {
    if let Some(root) = LINUX_EXECUTABLE_STAGING_ROOT.get() {
        return Ok(root.clone());
    }

    let roots = linux_staging_roots();
    let mut tried = Vec::new();
    for root in roots {
        tried.push(root.display().to_string());
        if staging_root_allows_exec(&root) {
            let cached = LINUX_EXECUTABLE_STAGING_ROOT.get_or_init(|| root);
            return Ok(cached.clone());
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "no executable staging root among tried paths: {}",
            tried.join(", ")
        ),
    ))
}

/// Write an executable file using sibling-install on Unix (mode at open, no post-close chmod).
pub fn write_generated_executable(path: &Path, contents: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::fs::{File, OpenOptions};
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let file_name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
        let sibling = parent.join(format!(".{}.install", file_name.to_string_lossy()));

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o755)
            .open(&sibling)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        if let Ok(dir_file) = File::open(parent) {
            let _ = dir_file.sync_all();
        }

        match fs::rename(&sibling, path) {
            Ok(()) => Ok(()),
            Err(err) => {
                let _ = fs::remove_file(&sibling);
                Err(err)
            }
        }
    }
    #[cfg(not(unix))]
    {
        fs::write(path, contents)
    }
}

/// Read a directory, returning an empty list if missing.
pub fn read_dir_or_empty(dir: &Path) -> io::Result<Vec<fs::DirEntry>> {
    match fs::read_dir(dir) {
        Ok(entries) => entries.collect(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err),
    }
}

/// Read a file, returning None if missing.
pub fn read_file_if_exists(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// RAII guard that removes a temporary directory when dropped.
pub struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ps_quote_simple() {
        assert_eq!(ps_quote("hello"), "'hello'");
    }

    #[test]
    fn test_ps_quote_with_single_quote() {
        assert_eq!(ps_quote("it's"), "'it''s'");
    }

    #[test]
    fn test_ps_quote_empty() {
        assert_eq!(ps_quote(""), "''");
    }

    #[test]
    #[cfg(not(windows))]
    fn test_set_executable_permissions_marks_user_exec_bit() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("script.sh");
        fs::write(&file, "#!/bin/sh\n").unwrap();

        set_executable_permissions(&file).unwrap();

        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn test_temp_dir_guard_removes_dir_on_drop() {
        let base = std::env::temp_dir().join(format!(
            "omakure_temp_guard_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        assert!(base.exists());
        {
            let _guard = TempDirGuard::new(base.clone());
        }
        assert!(!base.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn generated_executable_tempdir_stages_under_executable_root() {
        let dir = generated_executable_tempdir().unwrap();
        let parent = dir.path().parent().expect("tempdir has parent");
        let candidate_roots = linux_staging_roots();
        assert!(
            candidate_roots.iter().any(|root| parent.starts_with(root)),
            "expected staging under one of {:?}, got {}",
            candidate_roots,
            dir.path().display()
        );

        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(
            !dir.path().starts_with(workspace),
            "must not stage in workspace: {}",
            dir.path().display()
        );

        let shim = dir.path().join("shim");
        write_generated_executable(&shim, b"#!/bin/sh\nexit 0\n").unwrap();
        assert!(Command::new(&shim).status().unwrap().success());
    }

    #[cfg(unix)]
    #[test]
    fn write_generated_executable_sets_mode_at_open_without_set_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = generated_executable_tempdir().unwrap();
        let path = dir.path().join("probe");
        write_generated_executable(&path, b"#!/bin/sh\nexit 0\n").unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn test_read_dir_or_empty_returns_empty_when_missing() {
        let path = std::env::temp_dir().join("omakure_definitely_not_a_real_dir_xyz_42");
        let _ = fs::remove_dir_all(&path);
        let entries = read_dir_or_empty(&path).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_read_file_if_exists_returns_none_for_missing() {
        let path = std::env::temp_dir().join("omakure_definitely_not_a_real_file_xyz_42");
        let _ = fs::remove_file(&path);
        assert!(read_file_if_exists(&path).unwrap().is_none());
    }

    #[test]
    fn test_read_file_if_exists_returns_some_when_present() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("f.txt");
        fs::write(&path, "hi").unwrap();
        assert_eq!(read_file_if_exists(&path).unwrap(), Some("hi".to_string()));
    }
}

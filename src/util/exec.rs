use std::fs;
use std::io;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::process::Command;
#[cfg(target_os = "linux")]
use std::sync::OnceLock;

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
}

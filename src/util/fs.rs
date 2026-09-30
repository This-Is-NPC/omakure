use std::error::Error;
use std::fs;
use std::io;
use std::path::Path;

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

/// Atomically install a staged file over an existing destination on Windows.
/// ReplaceFileW preserves the destination's metadata and security descriptor;
/// unlike remove-then-rename there is no observable delete gap.
#[cfg(windows)]
pub fn replace_existing_windows(tmp: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    let replacement = tmp
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let replaced = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: Both paths are NUL-terminated UTF-16 strings that remain alive
    // for the duration of the synchronous API call. The null backup and
    // exclusion/preserve pointers request no backup and default behavior.
    let result = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Existing files must be replaced in place on Windows rather than removed
    /// before the staged file is installed.
    #[cfg(windows)]
    #[test]
    fn windows_replaces_existing_file_atomically() {
        let dir = tempfile::TempDir::new().unwrap();
        let destination = dir.path().join("target.toml");
        let replacement = dir.path().join("target.toml.tmp");
        fs::write(&destination, "old").unwrap();
        fs::write(&replacement, "new").unwrap();

        replace_existing_windows(&replacement, &destination).unwrap();

        assert_eq!(fs::read_to_string(&destination).unwrap(), "new");
        assert!(!replacement.exists(), "staged file must be consumed");
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

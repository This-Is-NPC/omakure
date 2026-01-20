use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

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
}

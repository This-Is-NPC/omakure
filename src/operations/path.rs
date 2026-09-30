//! Script-path helpers shared by the listing, describe, content, and search
//! operations.

use super::{OperationError, OperationErrorCode, OperationResult};
use std::path::{Path, PathBuf};

pub(crate) fn canonical_scripts_root(scripts_root: &Path) -> OperationResult<PathBuf> {
    scripts_root.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to canonicalize scripts root: {err}"),
        )
    })
}

/// `path` relative to `root` with `/` separators, compared as text; the whole
/// path when it is not under `root`.
pub(crate) fn logical_relative_path(path: &Path, root: &Path) -> String {
    let path_text = path.to_string_lossy().replace('\\', "/");
    let root_text = root
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    path_text
        .strip_prefix(&root_text)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(&path_text)
        .to_string()
}

/// [`logical_relative_path`] after resolving both paths, so two spellings of
/// the same directory still match; a path that cannot be resolved is compared
/// as given.
pub(crate) fn canonical_relative_path(path: &Path, root: &Path) -> String {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    logical_relative_path(&canonical_path, &canonical_root)
}

/// Whether `path` starts with a UNC (`\\`) or drive-letter (`C:`) prefix.
pub(crate) fn has_windows_prefix(path: &str) -> bool {
    path.starts_with("\\\\")
        || (path.as_bytes().get(1).is_some_and(|colon| *colon == b':')
            && path.as_bytes()[0].is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_use_forward_slashes_for_windows_fixtures() {
        for (root, path) in [
            (
                r"C:\workspace\scripts",
                r"C:\workspace\scripts\tools\deploy.cmd",
            ),
            (
                r"\\?\C:\workspace\scripts",
                r"\\?\C:\workspace\scripts\tools\deploy.cmd",
            ),
            (
                r"C:\PROGRA~1\OMAKURE\scripts",
                r"C:\PROGRA~1\OMAKURE\scripts\tools\deploy.cmd",
            ),
        ] {
            let (root, path) = (Path::new(root), Path::new(path));
            assert_eq!(logical_relative_path(path, root), "tools/deploy.cmd");
            assert_eq!(canonical_relative_path(path, root), "tools/deploy.cmd");
        }
    }

    #[test]
    fn a_path_outside_the_root_is_returned_whole() {
        let root = Path::new("/workspace/scripts");
        assert_eq!(
            logical_relative_path(Path::new("/elsewhere/deploy.sh"), root),
            "/elsewhere/deploy.sh"
        );
        assert_eq!(
            logical_relative_path(Path::new("/workspace/scripts-old/deploy.sh"), root),
            "/workspace/scripts-old/deploy.sh"
        );
    }

    #[test]
    fn windows_prefixes_are_unc_or_drive_letters() {
        assert!(has_windows_prefix(r"\\server\share"));
        assert!(has_windows_prefix("C:deploy.sh"));
        assert!(!has_windows_prefix("tools/deploy.sh"));
        assert!(!has_windows_prefix("1:deploy.sh"));
    }
}

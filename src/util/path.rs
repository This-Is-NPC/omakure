use std::path::Path;

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
            assert_eq!(
                logical_relative_path(Path::new(path), Path::new(root)),
                "tools/deploy.cmd"
            );
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
}

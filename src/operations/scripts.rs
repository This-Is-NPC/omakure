use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::ports::{ScriptRepository, WorkspaceEntryKind};
use crate::runtime::script_kind;
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use super::{OperationError, OperationErrorCode, OperationResult};

pub const MAX_SCRIPT_CONTENT_BYTES: u64 = 1024 * 1024;
pub const MAX_TREE_ENTRIES: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListTreeRequest {
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadScriptContentRequest {
    pub script: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub name: String,
    pub relative_path: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptContent {
    pub absolute_path: String,
    pub relative_path: String,
    pub content: String,
    pub size_bytes: u64,
}

pub fn list_tree(
    workspace: &Workspace,
    request: ListTreeRequest,
) -> OperationResult<Vec<TreeEntry>> {
    list_tree_limited(workspace, request, MAX_TREE_ENTRIES)
}

pub fn list_tree_limited(
    workspace: &Workspace,
    request: ListTreeRequest,
    max_entries: usize,
) -> OperationResult<Vec<TreeEntry>> {
    let root = canonical_scripts_root(workspace)?;
    let dir = resolve_workspace_path(request.path.as_deref().unwrap_or(""), &root)?;
    if !dir.is_dir() {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "tree path is not a directory",
        ));
    }

    let repo = FsWorkspaceRepository::new(root.clone());
    let entries = repo.list_entries(&dir).map_err(io_error)?;
    let limit = max_entries.max(1);
    if entries.len() > limit {
        return Err(OperationError::new(
            OperationErrorCode::PayloadTooLarge,
            format!("tree listing exceeds {limit} entries"),
        ));
    }
    Ok(entries
        .into_iter()
        .map(|entry| {
            let relative_path = logical_relative_path(&entry.path, &root);
            let name = entry
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_string();
            let kind = match entry.kind {
                WorkspaceEntryKind::Directory => "directory",
                WorkspaceEntryKind::Script => "script",
            }
            .to_string();
            TreeEntry {
                name,
                relative_path,
                kind,
            }
        })
        .collect())
}

pub fn read_script_content(
    workspace: &Workspace,
    request: ReadScriptContentRequest,
) -> OperationResult<ScriptContent> {
    read_script_content_limited(workspace, request, MAX_SCRIPT_CONTENT_BYTES)
}

pub fn read_script_content_limited(
    workspace: &Workspace,
    request: ReadScriptContentRequest,
    max_bytes: u64,
) -> OperationResult<ScriptContent> {
    let root = canonical_scripts_root(workspace)?;
    let path = resolve_workspace_path(&request.script, &root)?;
    if !path.is_file() {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "script is not a file",
        ));
    }
    if script_kind(&path).is_none() {
        return Err(OperationError::new(
            OperationErrorCode::UnsupportedScript,
            "unsupported script type",
        ));
    }
    let mut file = open_script_file(&path, &root)?;
    let metadata = file.metadata().map_err(io_error)?;
    let limit = max_bytes.max(1);
    if metadata.len() > limit {
        return Err(OperationError::new(
            OperationErrorCode::PayloadTooLarge,
            format!("script content exceeds {limit} bytes"),
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes).map_err(io_error)?;
    if bytes.contains(&0) {
        return Err(OperationError::new(
            OperationErrorCode::UnsupportedScript,
            "script content is binary",
        ));
    }
    let content = String::from_utf8(bytes).map_err(|_| {
        OperationError::new(
            OperationErrorCode::UnsupportedScript,
            "script content is not valid UTF-8",
        )
    })?;
    let relative_path = logical_relative_path(&path, &root);
    Ok(ScriptContent {
        absolute_path: path.to_string_lossy().to_string(),
        relative_path,
        size_bytes: metadata.len(),
        content,
    })
}

fn canonical_scripts_root(workspace: &Workspace) -> OperationResult<PathBuf> {
    workspace.scripts_root().canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to canonicalize scripts root: {err}"),
        )
    })
}
fn logical_relative_path(path: &Path, root: &Path) -> String {
    let canonical_root = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf());
    let canonical_path = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf());
    let path_text = canonical_path.to_string_lossy().replace('\\', "/");
    let root_text = canonical_root
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

fn has_windows_prefix(path: &str) -> bool {
    path.starts_with("\\\\")
        || (path.as_bytes().get(1).is_some_and(|colon| *colon == b':')
            && path.as_bytes()[0].is_ascii_alphabetic())
}


fn resolve_workspace_path(path: &str, root: &Path) -> OperationResult<PathBuf> {
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            "path escapes scripts root",
        ));
    }
    let normalized = path.replace('\\', "/");
    let raw = normalized.trim_start_matches('/');
    let path = PathBuf::from(raw);
    if has_windows_prefix(&normalized)
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            "path escapes scripts root",
        ));
    }
    if path.components().any(is_hidden_metadata_component) {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            "path targets hidden Omakure metadata",
        ));
    }
    let candidate = root.join(path);
    let canonical = candidate.canonicalize().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            OperationError::new(
                OperationErrorCode::NotFound,
                format!("path not found: {raw}"),
            )
        } else {
            io_error(err)
        }
    })?;
    if canonical.starts_with(root) {
        Ok(canonical)
    } else {
        Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            "path escapes scripts root",
        ))
    }
}

fn open_script_file(path: &Path, root: &Path) -> OperationResult<std::fs::File> {
    let file = open_no_follow(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd_path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
        if let Ok(opened) = fd_path.canonicalize() {
            if !opened.starts_with(root) {
                return Err(OperationError::new(
                    OperationErrorCode::UnsafePath,
                    "path escapes scripts root",
                ));
            }
        }
    }
    Ok(file)
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> OperationResult<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(0o400000)
        .open(path)
        .map_err(io_error)
}

#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> OperationResult<std::fs::File> {
    std::fs::File::open(path).map_err(io_error)
}

fn is_hidden_metadata_component(component: Component<'_>) -> bool {
    matches!(
        component,
        Component::Normal(name)
            if name == ".omakure" || name == ".history" || name == ".git"
    )
}

fn io_error(err: impl std::error::Error) -> OperationError {
    OperationError::new(OperationErrorCode::IoFailed, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn workspace_in(dir: &TempDir) -> Workspace {
        let workspace = Workspace::new(dir.path().to_path_buf());
        workspace.ensure_layout().unwrap();
        workspace
    }

    #[test]
    fn list_tree_honors_nested_omakureignore() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::create_dir_all(workspace.scripts_root().join("scripts/hidden")).unwrap();
        std::fs::write(
            workspace.scripts_root().join("scripts/.omakureignore"),
            "hidden/\n",
        )
        .unwrap();
        std::fs::write(
            workspace.scripts_root().join("scripts/show.sh"),
            "#!/bin/sh\n",
        )
        .unwrap();
        std::fs::write(
            workspace.scripts_root().join("scripts/hidden/nope.sh"),
            "#!/bin/sh\n",
        )
        .unwrap();

        let entries = list_tree(
            &workspace,
            ListTreeRequest {
                path: Some("scripts".into()),
            },
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, "scripts/show.sh");
    }

    #[test]
    fn read_script_content_rejects_parent_traversal() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let err = read_script_content(
            &workspace,
            ReadScriptContentRequest {
                script: "../outside.sh".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[test]
    fn read_script_content_rejects_absolute_paths_before_trimming() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let err = read_script_content(
            &workspace,
            ReadScriptContentRequest {
                script: "/tmp/outside.sh".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[test]
    fn list_tree_rejects_too_many_entries() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        for idx in 0..=MAX_TREE_ENTRIES {
            std::fs::write(
                workspace.scripts_root().join(format!("script-{idx}.sh")),
                "#!/bin/sh\n",
            )
            .unwrap();
        }

        let err = list_tree(&workspace, ListTreeRequest { path: None }).unwrap_err();

        assert_eq!(err.code, OperationErrorCode::PayloadTooLarge);
    }

    #[test]
    fn tree_and_content_reject_hidden_metadata_paths() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        for path in [".omakure", ".history", ".git"] {
            let tree_err = list_tree(
                &workspace,
                ListTreeRequest {
                    path: Some(path.into()),
                },
            )
            .unwrap_err();
            assert_eq!(tree_err.code, OperationErrorCode::UnsafePath);

            let content_err = read_script_content(
                &workspace,
                ReadScriptContentRequest {
                    script: format!("{path}/secret.sh"),
                },
            )
            .unwrap_err();
            assert_eq!(content_err.code, OperationErrorCode::UnsafePath);
        }
    }

    #[test]
    fn read_script_content_rejects_oversized_scripts() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(
            workspace.scripts_root().join("big.sh"),
            vec![b'a'; MAX_SCRIPT_CONTENT_BYTES as usize + 1],
        )
        .unwrap();

        let err = read_script_content(
            &workspace,
            ReadScriptContentRequest {
                script: "big.sh".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::PayloadTooLarge);
    }

    #[test]
    fn read_script_content_rejects_binary_and_invalid_utf8() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(
            workspace.scripts_root().join("binary.sh"),
            b"#!/bin/sh\n\0\n",
        )
        .unwrap();
        std::fs::write(workspace.scripts_root().join("bad-utf8.sh"), [0xff, 0xfe]).unwrap();

        let binary_err = read_script_content(
            &workspace,
            ReadScriptContentRequest {
                script: "binary.sh".into(),
            },
        )
        .unwrap_err();
        assert_eq!(binary_err.code, OperationErrorCode::UnsupportedScript);

        let utf8_err = read_script_content(
            &workspace,
            ReadScriptContentRequest {
                script: "bad-utf8.sh".into(),
            },
        )
        .unwrap_err();
        assert_eq!(utf8_err.code, OperationErrorCode::UnsupportedScript);
    }

    #[cfg(unix)]
    #[test]
    fn read_script_content_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(outside.path().join("outside.sh"), "#!/bin/sh\n").unwrap();
        symlink(
            outside.path().join("outside.sh"),
            workspace.scripts_root().join("escape.sh"),
        )
        .unwrap();

        let err = read_script_content(
            &workspace,
            ReadScriptContentRequest {
                script: "escape.sh".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[test]
    fn windows_relative_fixtures_resolve_and_render_with_slashes() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::create_dir_all(workspace.scripts_root().join("tools")).unwrap();
        let script = workspace.scripts_root().join("tools/deploy.sh");
        std::fs::write(&script, "#!/bin/sh\necho ok\n").unwrap();

        let resolved =
            resolve_workspace_path(r"tools\deploy.sh", workspace.scripts_root()).unwrap();
        assert_eq!(resolved, script.canonicalize().unwrap());
        assert_eq!(
            logical_relative_path(&resolved, workspace.scripts_root()),
            "tools/deploy.sh"
        );
    }

    #[test]
    fn read_script_content_returns_text_script() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(
            workspace.scripts_root().join("ok.sh"),
            "#!/bin/sh\necho ok\n",
        )
        .unwrap();

        let content = read_script_content(
            &workspace,
            ReadScriptContentRequest {
                script: "ok.sh".into(),
            },
        )
        .unwrap();

        assert_eq!(content.relative_path, "ok.sh");
        assert!(content.content.contains("echo ok"));
    }
}

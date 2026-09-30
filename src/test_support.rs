//! Fixtures shared by the crate's inline unit tests.

use crate::node::{NodeContext, NodePathOverrides, NodePlatform};
use crate::workspace::Workspace;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::TempDir;

/// A test-mode node context whose state directory and `node.toml` live under
/// `root`.
pub(crate) fn node_context(root: &Path) -> NodeContext {
    NodeContext::resolve_for(
        NodePlatform::current(),
        NodePathOverrides::new(Some(root.join("state")), Some(root.join("node.toml"))),
        true,
        None,
        None,
        None,
    )
    .expect("resolve the node context")
}

/// [`node_context`] with a minimal `node.toml` already written.
pub(crate) fn configured_node_context(root: &Path) -> NodeContext {
    fs::write(root.join("node.toml"), "version = 1\n").expect("write config");
    node_context(root)
}

/// A workspace rooted at `dir` with its layout created.
pub(crate) fn workspace_in(dir: &TempDir) -> Workspace {
    let workspace = Workspace::new(dir.path().to_path_buf());
    workspace.ensure_layout().unwrap();
    workspace
}

/// A fresh workspace under the system temporary directory, unique per call,
/// for tests that remove it themselves.
pub(crate) fn scratch_workspace(label: &str) -> Workspace {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "omakure_test_{label}_{}_{}_{}",
        std::process::id(),
        crate::util::time::unix_millis(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let workspace = Workspace::new(dir);
    workspace.ensure_layout().unwrap();
    workspace
}

/// An executable Bash script at the workspace root running `body`.
#[cfg(unix)]
pub(crate) fn write_bash_script(
    workspace: &Workspace,
    name: &str,
    body: &str,
) -> std::path::PathBuf {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let path = workspace.root().join(name);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(&path)
        .unwrap();
    write!(file, "#!/usr/bin/env bash\n{body}\n").unwrap();
    path
}

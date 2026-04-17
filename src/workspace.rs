use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Workspace layout used by the TUI and CLI.
///
/// The layout tracks two distinct path anchors:
///
/// - The **global root** owns `.history/`, `.omaken/`, `.omaken/envs/`,
///   the SQLite search index, and `omakure.toml`. This is the only path
///   that [`ensure_layout`](Self::ensure_layout) ever creates files in.
/// - The **scripts root** is the directory the user navigates inside the
///   TUI. It is also the source of `<scripts-root>/index.lua` and the
///   optional `<scripts-root>/omakure.conf` session env override.
///
/// When the TUI is launched without a positional path, both anchors point
/// at the same directory and behavior is identical to before this split.
/// When a positional path is supplied, only the scripts root is overridden;
/// `ensure_layout` is still strictly bound to the global root and never
/// touches the scripts root.
pub struct Workspace {
    root: PathBuf,
    scripts_root: PathBuf,
    scripts_root_override: bool,
    omaken_dir: PathBuf,
    history_dir: PathBuf,
    config_path: PathBuf,
    envs_dir: PathBuf,
    envs_active_path: PathBuf,
}

impl Workspace {
    /// Build a workspace whose global root and scripts root are the same.
    pub fn new(root: PathBuf) -> Self {
        let scripts_root = root.clone();
        Self::with_scripts_root(root, scripts_root, false)
    }

    /// Clone the workspace for use inside a background thread (the
    /// run executor's heartbeat / cancel watcher). The clone preserves
    /// every path anchor; it does not re-run any I/O.
    pub fn clone_for_executor(&self) -> Self {
        Self {
            root: self.root.clone(),
            scripts_root: self.scripts_root.clone(),
            scripts_root_override: self.scripts_root_override,
            omaken_dir: self.omaken_dir.clone(),
            history_dir: self.history_dir.clone(),
            config_path: self.config_path.clone(),
            envs_dir: self.envs_dir.clone(),
            envs_active_path: self.envs_active_path.clone(),
        }
    }

    /// Build a workspace where the scripts root may differ from the global
    /// root. `scripts_root_override` is `true` only when the scripts root
    /// was supplied via the positional CLI argument; this controls whether
    /// `<scripts-root>/omakure.conf` is interpreted as a session env.
    pub fn with_scripts_root(
        root: PathBuf,
        scripts_root: PathBuf,
        scripts_root_override: bool,
    ) -> Self {
        let omaken_dir = root.join(".omaken");
        let history_dir = root.join(".history");
        let config_path = root.join("omakure.toml");
        let envs_dir = omaken_dir.join("envs");
        let envs_active_path = envs_dir.join("active");
        Self {
            root,
            scripts_root,
            scripts_root_override,
            omaken_dir,
            history_dir,
            config_path,
            envs_dir,
            envs_active_path,
        }
    }

    /// Global workspace root — the owner of all persisted Omakure state.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Scripts root — the directory the TUI browses for the current session.
    pub fn scripts_root(&self) -> &Path {
        &self.scripts_root
    }

    /// Whether the scripts root was supplied via a positional CLI argument.
    pub fn has_scripts_root_override(&self) -> bool {
        self.scripts_root_override
    }

    pub fn omaken_dir(&self) -> &Path {
        &self.omaken_dir
    }

    pub fn history_dir(&self) -> &Path {
        &self.history_dir
    }

    pub fn search_db_path(&self) -> PathBuf {
        self.history_dir.join("search-index.sqlite")
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn envs_dir(&self) -> &Path {
        &self.envs_dir
    }

    pub fn envs_active_path(&self) -> &Path {
        &self.envs_active_path
    }

    /// Create all global workspace directories and the default config file
    /// if they do not yet exist.
    ///
    /// This method is **strictly bound to the global root**. It never
    /// reads or writes anything inside the scripts root, even when the
    /// scripts root has been overridden via a positional CLI argument.
    pub fn ensure_layout(&self) -> io::Result<()> {
        // Invariant: every path created here must live under `self.root`.
        // Adding any path that derives from `self.scripts_root` would
        // leak Omakure metadata into a directory the user did not intend
        // to make a workspace.
        debug_assert!(self.omaken_dir.starts_with(&self.root));
        debug_assert!(self.history_dir.starts_with(&self.root));
        debug_assert!(self.envs_dir.starts_with(&self.root));
        debug_assert!(self.config_path.starts_with(&self.root));

        fs::create_dir_all(&self.root)?;
        fs::create_dir_all(&self.omaken_dir)?;
        fs::create_dir_all(&self.history_dir)?;
        fs::create_dir_all(&self.envs_dir)?;
        if !self.config_path.exists() {
            fs::write(&self.config_path, default_config())?;
        }
        Ok(())
    }
}

fn default_config() -> String {
    format!(
        "# Omakure workspace configuration\n[workspace]\nversion = \"{}\"\n",
        crate::app_meta::APP_VERSION
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_new_uses_same_root_for_scripts() {
        let root = PathBuf::from("/tmp/omakure-ws-test");
        let ws = Workspace::new(root.clone());
        assert_eq!(ws.root(), root.as_path());
        assert_eq!(ws.scripts_root(), root.as_path());
        assert!(!ws.has_scripts_root_override());
    }

    #[test]
    fn workspace_with_scripts_root_decouples_paths() {
        let global = PathBuf::from("/tmp/omakure-global");
        let scripts = PathBuf::from("/tmp/some-other-dir");
        let ws = Workspace::with_scripts_root(global.clone(), scripts.clone(), true);
        assert_eq!(ws.root(), global.as_path());
        assert_eq!(ws.scripts_root(), scripts.as_path());
        assert!(ws.has_scripts_root_override());
        // All metadata paths must derive from the global root, not the scripts root.
        assert!(ws.omaken_dir().starts_with(&global));
        assert!(ws.history_dir().starts_with(&global));
        assert!(ws.envs_dir().starts_with(&global));
        assert!(ws.config_path().starts_with(&global));
        assert!(!ws.omaken_dir().starts_with(&scripts));
        assert!(!ws.history_dir().starts_with(&scripts));
    }

    #[test]
    fn workspace_path_accessors_derive_from_global_root() {
        let root = PathBuf::from("/tmp/omakure-paths");
        let ws = Workspace::new(root.clone());
        assert_eq!(
            ws.search_db_path(),
            root.join(".history").join("search-index.sqlite")
        );
        assert_eq!(
            ws.envs_active_path(),
            root.join(".omaken").join("envs").join("active")
        );
    }

    #[test]
    fn workspace_clone_for_executor_preserves_paths() {
        let root = PathBuf::from("/tmp/omakure-clone");
        let original = Workspace::with_scripts_root(root.clone(), root.clone(), false);
        let cloned = original.clone_for_executor();
        assert_eq!(cloned.root(), original.root());
        assert_eq!(cloned.scripts_root(), original.scripts_root());
        assert_eq!(
            cloned.has_scripts_root_override(),
            original.has_scripts_root_override()
        );
    }

    #[test]
    fn ensure_layout_creates_only_global_root_metadata() {
        let dir = std::env::temp_dir().join("__omakure_ensure_layout_split_test__");
        let global = dir.join("global");
        let scripts = dir.join("scripts");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&scripts).expect("create scripts dir");

        let ws = Workspace::with_scripts_root(global.clone(), scripts.clone(), true);
        ws.ensure_layout().expect("ensure_layout succeeds");

        assert!(global.join(".omaken").exists());
        assert!(global.join(".history").exists());
        assert!(global.join("omakure.toml").exists());
        assert!(global.join(".omaken").join("envs").exists());

        assert!(!scripts.join(".omaken").exists());
        assert!(!scripts.join(".history").exists());
        assert!(!scripts.join("omakure.toml").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}

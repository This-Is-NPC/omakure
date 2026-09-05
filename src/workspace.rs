use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Workspace layout used by the headless CLI.
pub struct Workspace {
    root: PathBuf,
    omakure_dir: PathBuf,
    history_dir: PathBuf,
    config_path: PathBuf,
    envs_dir: PathBuf,
    envs_active_path: PathBuf,
}

impl Workspace {
    /// Build a workspace rooted at `root`.
    pub fn new(root: PathBuf) -> Self {
        let omakure_dir = root.join(".omakure");
        let history_dir = root.join(".history");
        let config_path = root.join("omakure.toml");
        let envs_dir = omakure_dir.join("envs");
        let envs_active_path = envs_dir.join("active");
        Self {
            root,
            omakure_dir,
            history_dir,
            config_path,
            envs_dir,
            envs_active_path,
        }
    }

    /// Clone the workspace for use inside a background thread (the
    /// run executor's heartbeat / cancel watcher). The clone preserves
    /// every path anchor; it does not re-run any I/O.
    pub fn clone_for_executor(&self) -> Self {
        Self {
            root: self.root.clone(),
            omakure_dir: self.omakure_dir.clone(),
            history_dir: self.history_dir.clone(),
            config_path: self.config_path.clone(),
            envs_dir: self.envs_dir.clone(),
            envs_active_path: self.envs_active_path.clone(),
        }
    }

    /// Global workspace root — the owner of all persisted Omakure state.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Scripts root, which is the workspace root for the headless CLI.
    pub fn scripts_root(&self) -> &Path {
        &self.root
    }

    pub fn omakure_dir(&self) -> &Path {
        &self.omakure_dir
    }

    /// Where a running node service records the address it actually bound.
    ///
    /// Deliberately here and not in the node state directory: that directory
    /// holds identity and trust material behind a closed allow-list of entries,
    /// and a service address is neither. It is runtime information, and this is
    /// where runtime information lives.
    ///
    /// It exists because `api.bind` in the config is only a request —
    /// `node serve --bind` wins over it — so a separate process reading the
    /// config alone would look in the wrong place.
    pub fn service_endpoint_path(&self) -> PathBuf {
        self.omakure_dir.join("service.json")
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
    /// This method creates metadata only below the workspace root.
    pub fn ensure_layout(&self) -> io::Result<()> {
        // Invariant: every path created here must live under `self.root`.
        debug_assert!(self.omakure_dir.starts_with(&self.root));
        debug_assert!(self.history_dir.starts_with(&self.root));
        debug_assert!(self.envs_dir.starts_with(&self.root));
        debug_assert!(self.config_path.starts_with(&self.root));

        fs::create_dir_all(&self.root)?;
        fs::create_dir_all(&self.omakure_dir)?;
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
    fn workspace_new_uses_root_for_metadata() {
        let root = PathBuf::from("/tmp/omakure-ws-test");
        let ws = Workspace::new(root.clone());
        assert_eq!(ws.root(), root.as_path());
        assert!(ws.omakure_dir().starts_with(&root));
        assert!(ws.history_dir().starts_with(&root));
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
            root.join(".omakure").join("envs").join("active")
        );
    }

    #[test]
    fn workspace_clone_for_executor_preserves_paths() {
        let root = PathBuf::from("/tmp/omakure-clone");
        let original = Workspace::new(root.clone());
        let cloned = original.clone_for_executor();
        assert_eq!(cloned.root(), original.root());
    }

    #[test]
    fn ensure_layout_creates_workspace_metadata() {
        let dir = std::env::temp_dir().join("__omakure_ensure_layout_test__");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create workspace dir");

        let ws = Workspace::new(dir.clone());
        ws.ensure_layout().expect("ensure_layout succeeds");

        assert!(dir.join(".omakure").exists());
        assert!(dir.join(".history").exists());
        assert!(dir.join("omakure.toml").exists());
        assert!(dir.join(".omakure").join("envs").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}

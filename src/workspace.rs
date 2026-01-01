use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub struct Workspace {
    root: PathBuf,
    omaken_dir: PathBuf,
    history_dir: PathBuf,
    config_path: PathBuf,
}

impl Workspace {
    pub fn new(root: PathBuf) -> Self {
        let omaken_dir = root.join(".omaken");
        let history_dir = root.join(".history");
        let config_path = root.join("omakure.toml");
        Self {
            root,
            omaken_dir,
            history_dir,
            config_path,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
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

    pub fn ensure_layout(&self) -> io::Result<()> {
        fs::create_dir_all(&self.root)?;
        fs::create_dir_all(&self.omaken_dir)?;
        fs::create_dir_all(&self.history_dir)?;
        if !self.config_path.exists() {
            fs::write(&self.config_path, default_config())?;
        }
        Ok(())
    }
}

fn default_config() -> &'static str {
    r#"# Omakure workspace configuration
[workspace]
version = 1
"#
}

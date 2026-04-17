use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::AppResult;

#[derive(Debug, Clone)]
pub struct EnvironmentConfig {
    pub envs_dir: PathBuf,
    pub active: Option<String>,
    pub defaults: HashMap<String, String>,
    /// When `Some`, the active environment for the current session is the
    /// `omakure.conf` file at this path (set by the TUI when launched with
    /// a positional scripts-root override). Repository implementations
    /// always leave this field `None`; only the application layer fills it.
    #[allow(dead_code)]
    pub session_conf_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct EnvFile {
    pub name: String,
}

pub type EnvPreview = Vec<(String, String)>;

pub trait EnvironmentRepository {
    fn list_env_files(&self) -> AppResult<Vec<EnvFile>>;
    fn load_environment_config(&self) -> AppResult<EnvironmentConfig>;
    fn set_active_env(&self, name: Option<&str>) -> AppResult<()>;
    fn load_env_preview(&self, path: &Path) -> AppResult<EnvPreview>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_environment_config_construction() {
        let config = EnvironmentConfig {
            envs_dir: PathBuf::from("/tmp/envs"),
            active: Some("dev.conf".to_string()),
            defaults: HashMap::from([("host".to_string(), "localhost".to_string())]),
            session_conf_path: None,
        };
        assert_eq!(config.active, Some("dev.conf".to_string()));
        assert_eq!(config.defaults.get("host").unwrap(), "localhost");
        assert!(config.session_conf_path.is_none());
    }

    #[test]
    fn test_environment_repository_is_object_safe() {
        fn _assert_object_safe(_: &dyn EnvironmentRepository) {}
    }

    #[test]
    fn test_env_file_clone() {
        let file = EnvFile {
            name: "dev.conf".to_string(),
        };
        let cloned = file.clone();
        assert_eq!(cloned.name, "dev.conf");
    }
}

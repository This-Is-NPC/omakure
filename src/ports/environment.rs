use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::AppResult;

#[derive(Debug, Clone)]
#[allow(dead_code)] // `envs_dir`/`defaults` are read by the adapter tests only
pub struct EnvironmentConfig {
    pub envs_dir: PathBuf,
    pub active: Option<String>,
    pub defaults: HashMap<String, String>,
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
    fn create_env(&self, name: &str, params: &[(&str, &str)]) -> AppResult<()>;
    fn load_env_preview_by_name(&self, name: &str) -> AppResult<EnvPreview>;
    fn replace_env(&self, name: &str, params: &[(&str, &str)]) -> AppResult<()>;
    fn set_env_param(&self, name: &str, key: &str, value: &str) -> AppResult<()>;
    fn remove_env_param(&self, name: &str, key: &str) -> AppResult<()>;
    fn activate_env(&self, name: &str) -> AppResult<()>;
    fn deactivate_env(&self) -> AppResult<()>;
    fn delete_env(&self, name: &str) -> AppResult<()>;
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
        };
        assert_eq!(config.active, Some("dev.conf".to_string()));
        assert_eq!(config.defaults.get("host").unwrap(), "localhost");
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

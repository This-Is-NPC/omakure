use std::path::Path;

use crate::error::AppResult;
use crate::ports::{EnvFile, EnvPreview, EnvironmentConfig, EnvironmentRepository};

pub struct EnvironmentService {
    repo: Box<dyn EnvironmentRepository>,
}

impl EnvironmentService {
    pub fn new(repo: Box<dyn EnvironmentRepository>) -> Self {
        Self { repo }
    }

    pub fn list_env_files(&self) -> AppResult<Vec<EnvFile>> {
        self.repo.list_env_files()
    }

    pub fn load_environment_config(&self) -> AppResult<EnvironmentConfig> {
        self.repo.load_environment_config()
    }

    pub fn load_env_preview(&self, path: &Path) -> AppResult<EnvPreview> {
        self.repo.load_env_preview(path)
    }

    pub fn create_env(&self, name: &str, params: &[(&str, &str)]) -> AppResult<()> {
        self.repo.create_env(name, params)
    }

    pub fn load_env_preview_by_name(&self, name: &str) -> AppResult<EnvPreview> {
        self.repo.load_env_preview_by_name(name)
    }

    pub fn replace_env(&self, name: &str, params: &[(&str, &str)]) -> AppResult<()> {
        self.repo.replace_env(name, params)
    }

    pub fn set_env_param(&self, name: &str, key: &str, value: &str) -> AppResult<()> {
        self.repo.set_env_param(name, key, value)
    }

    pub fn remove_env_param(&self, name: &str, key: &str) -> AppResult<()> {
        self.repo.remove_env_param(name, key)
    }

    pub fn activate_env(&self, name: &str) -> AppResult<()> {
        self.repo.activate_env(name)
    }

    pub fn deactivate_env(&self) -> AppResult<()> {
        self.repo.deactivate_env()
    }

    pub fn delete_env(&self, name: &str) -> AppResult<()> {
        self.repo.delete_env(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::environments::FsEnvironmentRepository;
    use pretty_assertions::assert_eq;
    use rstest::{fixture, rstest};
    use std::fs;
    use tempfile::TempDir;

    #[fixture]
    fn env_service() -> (TempDir, EnvironmentService) {
        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();
        fs::write(envs.join("dev.conf"), "HOST=localhost\nPORT=3000").unwrap();
        fs::write(envs.join("staging.conf"), "HOST=staging.example.com").unwrap();

        let repo = FsEnvironmentRepository::new(&envs);
        let service = EnvironmentService::new(Box::new(repo));
        (tmp, service)
    }

    #[rstest]
    fn test_list_env_files(env_service: (TempDir, EnvironmentService)) {
        let (_tmp, service) = env_service;
        let files = service.list_env_files().unwrap();
        assert_eq!(files.len(), 2);
    }

    #[rstest]
    fn test_load_environment_config(env_service: (TempDir, EnvironmentService)) {
        let (_tmp, service) = env_service;
        let config = service.load_environment_config().unwrap();
        assert!(config.active.is_none());
    }

    #[rstest]
    fn test_activate_env(env_service: (TempDir, EnvironmentService)) {
        let (_tmp, service) = env_service;
        service.activate_env("dev").unwrap();
        let config = service.load_environment_config().unwrap();
        assert_eq!(config.active, Some("dev.conf".to_string()));
    }
}

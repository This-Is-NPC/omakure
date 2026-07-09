use crate::adapters::environments::FsEnvironmentRepository;
use crate::error::{AppError, EnvironmentError};
use crate::use_cases::EnvironmentService;
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::{OperationError, OperationErrorCode, OperationResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvParam {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvSummary {
    pub name: String,
    pub file: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvPreviewEntry {
    pub key: String,
    pub value: String,
}

pub fn list_envs(workspace: &Workspace) -> OperationResult<Vec<EnvSummary>> {
    let service = env_service(workspace);
    let active = service
        .load_environment_config()
        .map_err(map_env_error)?
        .active;
    let envs = service.list_env_files().map_err(map_env_error)?;
    Ok(envs
        .into_iter()
        .map(|env| EnvSummary {
            name: env.name.trim_end_matches(".conf").to_string(),
            active: active.as_deref() == Some(env.name.as_str()),
            file: env.name,
        })
        .collect())
}

pub fn create_env(workspace: &Workspace, name: &str, params: &[EnvParam]) -> OperationResult<()> {
    let refs = param_refs(params);
    env_service(workspace)
        .create_env(name, &refs)
        .map_err(map_env_error)
}

pub fn show_env(workspace: &Workspace, name: &str) -> OperationResult<Vec<EnvPreviewEntry>> {
    env_service(workspace)
        .load_env_preview_by_name(name)
        .map(|entries| {
            entries
                .into_iter()
                .map(|(key, value)| EnvPreviewEntry { key, value })
                .collect()
        })
        .map_err(map_env_error)
}

pub fn env_file_path(workspace: &Workspace, name: &str) -> OperationResult<PathBuf> {
    FsEnvironmentRepository::new(workspace.envs_dir())
        .env_path_for_name(name, true)
        .map_err(map_env_error)
}

pub fn replace_env(workspace: &Workspace, name: &str, params: &[EnvParam]) -> OperationResult<()> {
    let refs = param_refs(params);
    env_service(workspace)
        .replace_env(name, &refs)
        .map_err(map_env_error)
}

pub fn set_param(workspace: &Workspace, name: &str, key: &str, value: &str) -> OperationResult<()> {
    env_service(workspace)
        .set_env_param(name, key, value)
        .map_err(map_env_error)
}

pub fn remove_param(workspace: &Workspace, name: &str, key: &str) -> OperationResult<()> {
    env_service(workspace)
        .remove_env_param(name, key)
        .map_err(map_env_error)
}

pub fn activate_env(workspace: &Workspace, name: &str) -> OperationResult<()> {
    env_service(workspace)
        .activate_env(name)
        .map_err(map_env_error)
}

pub fn deactivate_env(workspace: &Workspace) -> OperationResult<()> {
    env_service(workspace)
        .deactivate_env()
        .map_err(map_env_error)
}

pub fn delete_env(workspace: &Workspace, name: &str) -> OperationResult<()> {
    env_service(workspace)
        .delete_env(name)
        .map_err(map_env_error)
}

fn env_service(workspace: &Workspace) -> EnvironmentService {
    EnvironmentService::new(Box::new(FsEnvironmentRepository::new(workspace.envs_dir())))
}

fn param_refs(params: &[EnvParam]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|param| (param.key.as_str(), param.value.as_str()))
        .collect()
}

fn map_env_error(err: AppError) -> OperationError {
    match err {
        AppError::Environment(EnvironmentError::NotFound { name }) => {
            OperationError::new(OperationErrorCode::NotFound, name)
        }
        AppError::Environment(EnvironmentError::InvalidName { name }) => OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("Invalid environment name: {name}"),
        ),
        AppError::Environment(EnvironmentError::UnsafePath { path }) => OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("Unsafe environment path: {path}"),
        ),
        AppError::Environment(EnvironmentError::ReadFailed(message)) => {
            OperationError::new(OperationErrorCode::IoFailed, message)
        }
        AppError::Environment(EnvironmentError::WriteFailed(message)) => {
            OperationError::new(OperationErrorCode::IoFailed, message)
        }
        other => OperationError::new(OperationErrorCode::IoFailed, other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use pretty_assertions::assert_eq;
    use std::fs;
    use tempfile::TempDir;

    fn workspace() -> (TempDir, Workspace) {
        let tmp = TempDir::new().unwrap();
        let scripts = tmp.path().join("scripts");
        let workspace = Workspace::new(scripts);
        workspace.ensure_layout().unwrap();
        (tmp, workspace)
    }

    #[test]
    fn env_operations_round_trip_and_mask_preview() {
        let (_tmp, workspace) = workspace();

        create_env(
            &workspace,
            "prod",
            &[
                EnvParam {
                    key: "HOST".to_string(),
                    value: "prod.example.com".to_string(),
                },
                EnvParam {
                    key: "API_KEY".to_string(),
                    value: "secret".to_string(),
                },
            ],
        )
        .unwrap();

        assert!(workspace.envs_dir().join("prod.conf").is_file());
        assert_eq!(
            show_env(&workspace, "prod").unwrap(),
            vec![
                EnvPreviewEntry {
                    key: "HOST".to_string(),
                    value: "prod.example.com".to_string(),
                },
                EnvPreviewEntry {
                    key: "API_KEY".to_string(),
                    value: "****".to_string(),
                },
            ]
        );

        set_param(&workspace, "prod", "PORT", "443").unwrap();
        activate_env(&workspace, "prod").unwrap();
        assert_eq!(
            list_envs(&workspace).unwrap(),
            vec![EnvSummary {
                name: "prod".to_string(),
                file: "prod.conf".to_string(),
                active: true,
            }]
        );

        remove_param(&workspace, "prod", "API_KEY").unwrap();
        assert_eq!(
            fs::read_to_string(workspace.envs_dir().join("prod.conf")).unwrap(),
            "HOST=prod.example.com\nPORT=443\n"
        );

        deactivate_env(&workspace).unwrap();
        delete_env(&workspace, "prod").unwrap();
        assert!(list_envs(&workspace).unwrap().is_empty());
    }
}

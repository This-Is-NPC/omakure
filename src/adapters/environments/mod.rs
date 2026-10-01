use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{AppResult, EnvironmentError};
use crate::util::fs::{read_dir_or_empty, read_file_if_exists};
use files::{ensure_env_path_safe, write_active_atomic, write_env_params_atomic};
use layers::{parse_env_defaults, parse_env_pairs_raw, parse_env_preview};
use values::is_valid_var_name;

mod files;
mod layers;
mod values;

pub(crate) use layers::{read_managed_env_defaults, resolve_active_env, resolve_run_env};
pub(crate) use values::{is_sensitive_key, should_mask_env_value};

pub(crate) const MASKED_ENV_VALUE: &str = "****";

#[derive(Debug, Clone)]
pub(crate) struct EnvironmentConfig {
    pub active: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct EnvFile {
    pub name: String,
}

type EnvPreview = Vec<(String, String)>;

pub struct FsEnvironmentRepository {
    envs_dir: PathBuf,
}

impl FsEnvironmentRepository {
    pub fn new<P: Into<PathBuf>>(envs_dir: P) -> Self {
        Self {
            envs_dir: envs_dir.into(),
        }
    }

    fn read_env_defaults(&self, path: &Path) -> AppResult<HashMap<String, String>> {
        let contents = fs::read_to_string(path).map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to read environment file {}: {}",
                path.display(),
                err
            ))
        })?;
        Ok(parse_env_defaults(&contents))
    }

    pub(crate) fn env_path_for_name(&self, name: &str, must_exist: bool) -> AppResult<PathBuf> {
        validate_env_name(name)?;
        fs::create_dir_all(&self.envs_dir).map_err(|err| {
            EnvironmentError::WriteFailed(format!(
                "Failed to create environments dir {}: {}",
                self.envs_dir.display(),
                err
            ))
        })?;

        let path = self.envs_dir.join(format!("{name}.conf"));
        ensure_env_path_safe(&self.envs_dir, &path, must_exist)?;
        Ok(path)
    }

    pub(crate) fn list_env_files(&self) -> AppResult<Vec<EnvFile>> {
        let mut entries = Vec::new();
        let dir = read_dir_or_empty(&self.envs_dir).map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to read environments dir {}: {}",
                self.envs_dir.display(),
                err
            ))
        })?;

        for entry in dir {
            let path = entry.path();
            if path
                .symlink_metadata()
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(true)
            {
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let name = match path.file_name().and_then(|name| name.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };
            if name == "active" {
                continue;
            }
            entries.push(EnvFile { name });
        }

        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    pub(crate) fn load_environment_config(&self) -> AppResult<EnvironmentConfig> {
        let active = load_active_env_name(&self.envs_dir)?;
        if let Some(name) = &active {
            let path = self.envs_dir.join(name);
            if !path.is_file() {
                return Err(EnvironmentError::NotFound {
                    name: path.display().to_string(),
                }
                .into());
            }
            self.read_env_defaults(&path)?;
        }

        Ok(EnvironmentConfig { active })
    }

    pub(crate) fn set_active_env(&self, name: Option<&str>) -> AppResult<()> {
        fs::create_dir_all(&self.envs_dir).map_err(|err| {
            EnvironmentError::WriteFailed(format!(
                "Failed to create environments dir {}: {}",
                self.envs_dir.display(),
                err
            ))
        })?;
        let active_path = self.envs_dir.join("active");

        match name {
            Some(name) => {
                let candidate = self.envs_dir.join(name);
                if !candidate.is_file() {
                    return Err(EnvironmentError::NotFound {
                        name: candidate.display().to_string(),
                    }
                    .into());
                }
                write_active_atomic(&active_path, name)?;
            }
            None => {
                if active_path.exists() {
                    fs::remove_file(&active_path).map_err(|err| {
                        EnvironmentError::WriteFailed(format!(
                            "Failed to clear active environment {}: {}",
                            active_path.display(),
                            err
                        ))
                    })?;
                }
            }
        }

        Ok(())
    }

    pub(crate) fn load_env_preview(&self, path: &Path) -> AppResult<EnvPreview> {
        let contents = fs::read_to_string(path).map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to read environment file {}: {}",
                path.display(),
                err
            ))
        })?;
        Ok(parse_env_preview(&contents))
    }

    pub(crate) fn create_env(&self, name: &str, params: &[(&str, &str)]) -> AppResult<()> {
        let path = self.env_path_for_name(name, false)?;
        if path.exists() {
            return Err(EnvironmentError::WriteFailed(format!(
                "Environment already exists: {}",
                path.display()
            ))
            .into());
        }
        write_env_params_atomic(&path, params)
    }

    pub(crate) fn load_env_preview_by_name(&self, name: &str) -> AppResult<EnvPreview> {
        let path = self.env_path_for_name(name, true)?;
        self.load_env_preview(&path)
    }

    pub(crate) fn replace_env(&self, name: &str, params: &[(&str, &str)]) -> AppResult<()> {
        let path = self.env_path_for_name(name, true)?;
        write_env_params_atomic(&path, params)
    }

    pub(crate) fn set_env_param(&self, name: &str, key: &str, value: &str) -> AppResult<()> {
        validate_env_key(key)?;
        let path = self.env_path_for_name(name, true)?;
        let mut params = parse_env_pairs_raw(&fs::read_to_string(&path).map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to read environment file {}: {}",
                path.display(),
                err
            ))
        })?);
        match params.iter_mut().find(|(existing, _)| existing == key) {
            Some((_, existing_value)) => *existing_value = value.to_string(),
            None => params.push((key.to_string(), value.to_string())),
        }
        let refs: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        write_env_params_atomic(&path, &refs)
    }

    pub(crate) fn remove_env_param(&self, name: &str, key: &str) -> AppResult<()> {
        validate_env_key(key)?;
        let path = self.env_path_for_name(name, true)?;
        let mut params = parse_env_pairs_raw(&fs::read_to_string(&path).map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to read environment file {}: {}",
                path.display(),
                err
            ))
        })?);
        params.retain(|(existing, _)| existing != key);
        let refs: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        write_env_params_atomic(&path, &refs)
    }

    pub(crate) fn activate_env(&self, name: &str) -> AppResult<()> {
        let path = self.env_path_for_name(name, true)?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| EnvironmentError::UnsafePath {
                path: path.display().to_string(),
            })?;
        write_active_atomic(&self.envs_dir.join("active"), file_name)
    }

    pub(crate) fn deactivate_env(&self) -> AppResult<()> {
        self.set_active_env(None)
    }

    pub(crate) fn delete_env(&self, name: &str) -> AppResult<()> {
        let path = self.env_path_for_name(name, true)?;
        fs::remove_file(&path).map_err(|err| {
            EnvironmentError::WriteFailed(format!(
                "Failed to delete environment file {}: {}",
                path.display(),
                err
            ))
        })?;

        if load_active_env_name(&self.envs_dir)? == Some(format!("{name}.conf")) {
            self.set_active_env(None)?;
        }
        Ok(())
    }
}

fn load_active_env_name(envs_dir: &Path) -> AppResult<Option<String>> {
    let active_path = envs_dir.join("active");
    let contents = read_file_if_exists(&active_path)
        .map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to read active environment {}: {}",
                active_path.display(),
                err
            ))
        })?
        .unwrap_or_default();

    if contents.is_empty() {
        return Ok(None);
    }

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        return Ok(Some(trimmed.to_string()));
    }

    Ok(None)
}

fn validate_env_name(name: &str) -> Result<(), EnvironmentError> {
    let invalid = name.is_empty()
        || name == "active"
        || name.starts_with('.')
        || name.ends_with(".conf")
        || name.contains("..")
        || name.contains('/')
        || name.contains('\\')
        || Path::new(name).components().count() != 1;
    if invalid {
        return Err(EnvironmentError::InvalidName {
            name: name.to_string(),
        });
    }
    Ok(())
}

fn validate_env_key(key: &str) -> Result<(), EnvironmentError> {
    if is_valid_var_name(key) {
        Ok(())
    } else {
        Err(EnvironmentError::WriteFailed(format!(
            "Invalid environment variable name: {key}"
        )))
    }
}

#[cfg(test)]
mod tests;

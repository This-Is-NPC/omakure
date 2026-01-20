use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct EnvironmentConfig {
    pub envs_dir: PathBuf,
    pub active: Option<String>,
    pub defaults: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct EnvFile {
    pub name: String,
}

pub fn load_env_preview(path: &Path) -> Result<Vec<(String, String)>, String> {
    let contents = fs::read_to_string(path).map_err(|err| {
        format!(
            "Failed to read environment file {}: {}",
            path.display(),
            err
        )
    })?;
    let mut entries = Vec::new();

    for line in contents.lines() {
        let mut trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if let Some(stripped) = trimmed.strip_prefix("export ") {
            trimmed = stripped.trim();
        }

        let mut parts = trimmed.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim();
        let raw_value = parts.next().unwrap_or("").trim();
        if key.is_empty() {
            continue;
        }
        let mut value = strip_quotes(raw_value).trim().to_string();
        if is_sensitive_key(key) && !value.is_empty() {
            value = "***".to_string();
        }
        entries.push((key.to_string(), value));
    }

    Ok(entries)
}

pub fn list_env_files(envs_dir: &Path) -> Result<Vec<EnvFile>, String> {
    let mut entries = Vec::new();
    let dir = match fs::read_dir(envs_dir) {
        Ok(dir) => dir,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(entries);
        }
        Err(err) => {
            return Err(format!(
                "Failed to read environments dir {}: {}",
                envs_dir.display(),
                err
            ));
        }
    };

    for entry in dir {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
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

pub fn load_environment_config(envs_dir: &Path) -> Result<EnvironmentConfig, String> {
    let active = load_active_env_name(envs_dir)?;
    let defaults = if let Some(name) = &active {
        let path = envs_dir.join(name);
        if !path.is_file() {
            return Err(format!("Active environment not found: {}", path.display()));
        }
        load_env_defaults(&path)?
    } else {
        HashMap::new()
    };

    Ok(EnvironmentConfig {
        envs_dir: envs_dir.to_path_buf(),
        active,
        defaults,
    })
}

pub fn set_active_env(envs_dir: &Path, name: Option<&str>) -> Result<(), String> {
    fs::create_dir_all(envs_dir).map_err(|err| {
        format!(
            "Failed to create environments dir {}: {}",
            envs_dir.display(),
            err
        )
    })?;
    let active_path = envs_dir.join("active");

    match name {
        Some(name) => {
            let candidate = envs_dir.join(name);
            if !candidate.is_file() {
                return Err(format!(
                    "Environment file not found: {}",
                    candidate.display()
                ));
            }
            fs::write(&active_path, format!("{}\n", name)).map_err(|err| {
                format!(
                    "Failed to write active environment {}: {}",
                    active_path.display(),
                    err
                )
            })?;
        }
        None => {
            if active_path.exists() {
                fs::remove_file(&active_path).map_err(|err| {
                    format!(
                        "Failed to clear active environment {}: {}",
                        active_path.display(),
                        err
                    )
                })?;
            }
        }
    }

    Ok(())
}

fn load_active_env_name(envs_dir: &Path) -> Result<Option<String>, String> {
    let active_path = envs_dir.join("active");
    let contents = match fs::read_to_string(&active_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(format!(
                "Failed to read active environment {}: {}",
                active_path.display(),
                err
            ));
        }
    };

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        return Ok(Some(trimmed.to_string()));
    }

    Ok(None)
}

fn load_env_defaults(path: &Path) -> Result<HashMap<String, String>, String> {
    let contents = fs::read_to_string(path).map_err(|err| {
        format!(
            "Failed to read environment file {}: {}",
            path.display(),
            err
        )
    })?;
    let mut defaults = HashMap::new();

    for line in contents.lines() {
        let mut trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if let Some(stripped) = trimmed.strip_prefix("export ") {
            trimmed = stripped.trim();
        }

        let mut parts = trimmed.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim();
        let raw_value = parts.next().unwrap_or("").trim();
        if key.is_empty() {
            continue;
        }
        let value = strip_quotes(raw_value).trim();
        if value.is_empty() {
            continue;
        }
        defaults.insert(key.to_ascii_lowercase(), value.to_string());
    }

    Ok(defaults)
}

fn strip_quotes(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.as_bytes()[0] as char;
        let last = trimmed.as_bytes()[trimmed.len() - 1] as char;
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    trimmed
}

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    let tokens = [
        "password", "secret", "token", "key", "api", "private", "cred",
    ];
    tokens.iter().any(|token| lower.contains(token))
}

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{AppResult, EnvironmentError};
pub use crate::ports::{EnvFile, EnvironmentConfig};
use crate::ports::{EnvPreview, EnvironmentRepository};
use crate::util::{read_dir_or_empty, read_file_if_exists};

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
}

impl EnvironmentRepository for FsEnvironmentRepository {
    fn list_env_files(&self) -> AppResult<Vec<EnvFile>> {
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

    fn load_environment_config(&self) -> AppResult<EnvironmentConfig> {
        let active = load_active_env_name(&self.envs_dir)?;
        let defaults = if let Some(name) = &active {
            let path = self.envs_dir.join(name);
            if !path.is_file() {
                return Err(EnvironmentError::NotFound {
                    name: path.display().to_string(),
                }
                .into());
            }
            self.read_env_defaults(&path)?
        } else {
            HashMap::new()
        };

        Ok(EnvironmentConfig {
            envs_dir: self.envs_dir.clone(),
            active,
            defaults,
            session_conf_path: None,
        })
    }

    fn set_active_env(&self, name: Option<&str>) -> AppResult<()> {
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
                fs::write(&active_path, format!("{}\n", name)).map_err(|err| {
                    EnvironmentError::WriteFailed(format!(
                        "Failed to write active environment {}: {}",
                        active_path.display(),
                        err
                    ))
                })?;
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

    fn load_env_preview(&self, path: &Path) -> AppResult<EnvPreview> {
        let contents = fs::read_to_string(path).map_err(|err| {
            EnvironmentError::ReadFailed(format!(
                "Failed to read environment file {}: {}",
                path.display(),
                err
            ))
        })?;
        Ok(parse_env_preview(&contents))
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

pub(crate) fn parse_env_preview(contents: &str) -> Vec<(String, String)> {
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

    entries
}

pub(crate) fn parse_env_defaults(contents: &str) -> HashMap<String, String> {
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

    defaults
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

pub(crate) fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    let tokens = [
        "password", "secret", "token", "key", "api", "private", "cred",
    ];
    tokens.iter().any(|token| lower.contains(token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::{fixture, rstest};
    use std::fs;
    use tempfile::TempDir;

    // --- Pure function tests ---

    #[rstest]
    #[case::password("DB_PASSWORD", true)]
    #[case::secret("SECRET_KEY", true)]
    #[case::token("AUTH_TOKEN", true)]
    #[case::api_key("API_KEY", true)]
    #[case::private("PRIVATE_KEY", true)]
    #[case::cred("CREDENTIALS", true)]
    #[case::key("SSH_KEY", true)]
    #[case::url_not_sensitive("DATABASE_URL", false)]
    #[case::name_not_sensitive("APP_NAME", false)]
    #[case::port_not_sensitive("PORT", false)]
    #[case::debug_not_sensitive("DEBUG", false)]
    fn test_is_sensitive_key(#[case] key: &str, #[case] expected: bool) {
        assert_eq!(is_sensitive_key(key), expected);
    }

    #[rstest]
    #[case::simple_pair("KEY=value", vec![("key", "value")])]
    #[case::export_prefix("export KEY=value", vec![("key", "value")])]
    #[case::double_quotes("KEY=\"quoted value\"", vec![("key", "quoted value")])]
    #[case::single_quotes("KEY='single'", vec![("key", "single")])]
    #[case::comment_skipped("# comment", vec![])]
    #[case::semicolon_comment_skipped("; comment", vec![])]
    #[case::empty_line_skipped("", vec![])]
    #[case::empty_value_skipped("KEY=", vec![])]
    #[case::whitespace_trimmed("  KEY = value  ", vec![("key", "value")])]
    fn test_parse_env_defaults(#[case] input: &str, #[case] expected: Vec<(&str, &str)>) {
        let result = parse_env_defaults(input);
        let expected_map: HashMap<String, String> = expected
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(result, expected_map);
    }

    #[test]
    fn test_parse_env_defaults_multiline() {
        let input = "HOST=localhost\nPORT=8080\n# comment\nDEBUG=true";
        let result = parse_env_defaults(input);
        assert_eq!(result.len(), 3);
        assert_eq!(result.get("host").unwrap(), "localhost");
        assert_eq!(result.get("port").unwrap(), "8080");
        assert_eq!(result.get("debug").unwrap(), "true");
    }

    #[rstest]
    #[case::simple_pair("HOST=localhost", vec![("HOST", "localhost")])]
    #[case::sensitive_masked("DB_PASSWORD=secret123", vec![("DB_PASSWORD", "***")])]
    #[case::api_key_masked("API_KEY=abc", vec![("API_KEY", "***")])]
    #[case::comment_skipped("# comment\nNAME=test", vec![("NAME", "test")])]
    fn test_parse_env_preview(#[case] input: &str, #[case] expected: Vec<(&str, &str)>) {
        let result = parse_env_preview(input);
        let expected_vec: Vec<(String, String)> = expected
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(result, expected_vec);
    }

    // --- Filesystem-based tests ---

    #[fixture]
    fn envs_dir() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        fs::create_dir_all(&envs).unwrap();

        fs::write(envs.join("dev.conf"), "HOST=localhost\nPORT=3000").unwrap();
        fs::write(
            envs.join("prod.conf"),
            "HOST=prod.example.com\nAPI_KEY=secret",
        )
        .unwrap();

        (tmp, envs)
    }

    #[rstest]
    fn test_list_env_files(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        let repo = FsEnvironmentRepository::new(&envs);
        let files = repo.list_env_files().unwrap();

        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["dev.conf", "prod.conf"]);
    }

    #[rstest]
    fn test_list_env_files_skips_active(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let repo = FsEnvironmentRepository::new(&envs);
        let files = repo.list_env_files().unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert!(!names.contains(&"active"));
    }

    #[rstest]
    fn test_load_environment_config_no_active(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        let repo = FsEnvironmentRepository::new(&envs);
        let config = repo.load_environment_config().unwrap();

        assert!(config.active.is_none());
        assert!(config.defaults.is_empty());
    }

    #[rstest]
    fn test_load_environment_config_with_active(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let repo = FsEnvironmentRepository::new(&envs);
        let config = repo.load_environment_config().unwrap();

        assert_eq!(config.active, Some("dev.conf".to_string()));
        assert_eq!(config.defaults.get("host").unwrap(), "localhost");
        assert_eq!(config.defaults.get("port").unwrap(), "3000");
    }

    #[rstest]
    fn test_set_active_env(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        let repo = FsEnvironmentRepository::new(&envs);

        repo.set_active_env(Some("dev.conf")).unwrap();
        let active = fs::read_to_string(envs.join("active")).unwrap();
        assert_eq!(active.trim(), "dev.conf");
    }

    #[rstest]
    fn test_set_active_env_clear(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        fs::write(envs.join("active"), "dev.conf\n").unwrap();

        let repo = FsEnvironmentRepository::new(&envs);
        repo.set_active_env(None).unwrap();
        assert!(!envs.join("active").exists());
    }

    #[rstest]
    fn test_set_active_env_nonexistent(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        let repo = FsEnvironmentRepository::new(&envs);

        let result = repo.set_active_env(Some("nonexistent.conf"));
        assert!(result.is_err());
    }

    #[rstest]
    fn test_load_env_preview(envs_dir: (TempDir, PathBuf)) {
        let (_tmp, envs) = envs_dir;
        let repo = FsEnvironmentRepository::new(&envs);
        let preview = repo.load_env_preview(&envs.join("prod.conf")).unwrap();

        assert_eq!(
            preview[0],
            ("HOST".to_string(), "prod.example.com".to_string())
        );
        assert_eq!(preview[1], ("API_KEY".to_string(), "***".to_string()));
    }
}

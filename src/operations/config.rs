use crate::adapters::environments::{
    is_sensitive_key, resolve_active_env, should_mask_env_value, FsEnvironmentRepository,
    MASKED_ENV_VALUE,
};
use crate::app_meta;
use crate::ports::EnvironmentRepository;
use crate::runtime::{python_program, resolve_interpreter};
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::path::Path;

use super::{OperationError, OperationErrorCode, OperationResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSummary {
    pub version: String,
    pub binary: String,
    pub workspace_root: String,
    pub scripts_root: String,
    pub omakure_dir: String,
    pub history_dir: String,
    pub workspace_config: String,
    pub envs_dir: String,
    pub envs_active_path: String,
    pub active_env: Option<String>,
    pub bootstrap_mode: String,
    pub env_overrides: BTreeMap<String, String>,
    pub active_env_keys: Vec<EnvVarView>,
    pub interpreter: InterpreterView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVarView {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpreterView {
    pub program: String,
    pub path: Option<String>,
}

pub fn config_summary(workspace: &Workspace) -> OperationResult<ConfigSummary> {
    let exe = env::current_exe()
        .map_err(|err| OperationError::new(OperationErrorCode::IoFailed, err.to_string()))?;
    let active_env = read_active_env(workspace);
    let (active_env_keys, interpreter) = resolve_env_diagnostics(workspace.envs_dir());

    Ok(ConfigSummary {
        version: app_meta::APP_VERSION.to_string(),
        binary: exe.display().to_string(),
        workspace_root: workspace.root().display().to_string(),
        scripts_root: workspace.scripts_root().display().to_string(),
        omakure_dir: workspace.omakure_dir().display().to_string(),
        history_dir: workspace.history_dir().display().to_string(),
        workspace_config: workspace.config_path().display().to_string(),
        envs_dir: workspace.envs_dir().display().to_string(),
        envs_active_path: workspace.envs_active_path().display().to_string(),
        active_env,
        bootstrap_mode: "plain".to_string(),
        env_overrides: collect_env_overrides(),
        active_env_keys,
        interpreter,
    })
}

pub fn redacted_config_summary(workspace: &Workspace) -> OperationResult<ConfigSummary> {
    let mut summary = config_summary(workspace)?;
    for key in &mut summary.active_env_keys {
        key.value = MASKED_ENV_VALUE.to_string();
    }
    Ok(summary)
}

fn read_active_env(workspace: &Workspace) -> Option<String> {
    let repo = FsEnvironmentRepository::new(workspace.envs_dir());
    repo.load_environment_config().ok().and_then(|c| c.active)
}

pub fn resolve_env_diagnostics(envs_dir: &Path) -> (Vec<EnvVarView>, InterpreterView) {
    let pairs = resolve_active_env(envs_dir);
    let sensitive_parent_values = sensitive_parent_values();
    let program = python_program();
    let path = resolve_interpreter(program, &pairs).map(|p| p.display().to_string());

    let keys = pairs
        .into_iter()
        .map(|(key, value)| {
            let value = if should_mask_resolved_env_value(&key, &value, &sensitive_parent_values) {
                MASKED_ENV_VALUE.to_string()
            } else {
                value
            };
            EnvVarView { key, value }
        })
        .collect();

    (
        keys,
        InterpreterView {
            program: program.to_string(),
            path,
        },
    )
}

pub fn mask_env_display_value(key: &str, value: String) -> String {
    if should_mask_env_value(key, &value) {
        MASKED_ENV_VALUE.to_string()
    } else {
        value
    }
}

fn should_mask_resolved_env_value(
    key: &str,
    value: &str,
    sensitive_parent_values: &[String],
) -> bool {
    should_mask_env_value(key, value)
        || (!value.is_empty() && sensitive_parent_values.iter().any(|secret| secret == value))
}

fn sensitive_parent_values() -> Vec<String> {
    env::vars()
        .filter_map(|(key, value)| {
            if is_sensitive_key(&key) && !value.is_empty() {
                Some(value)
            } else {
                None
            }
        })
        .collect()
}

pub fn collect_env_overrides() -> BTreeMap<String, String> {
    let names = env_override_names();
    let mut out = BTreeMap::new();
    for name in names {
        if let Ok(value) = env::var(name) {
            out.insert(name.to_string(), mask_env_display_value(name, value));
        }
    }
    out
}

pub fn env_override_names() -> [&'static str; 8] {
    [
        "OMAKURE_SCRIPTS_DIR",
        "OMAKURE_REPO",
        "REPO",
        "VERSION",
        "OVERTURE_SCRIPTS_DIR",
        "OVERTURE_REPO",
        "CLOUD_MGMT_SCRIPTS_DIR",
        "CLOUD_MGMT_REPO",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    fn write_active_env(dir: &Path, conf_name: &str, contents: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(conf_name), contents).unwrap();
        fs::write(dir.join("active"), format!("{}\n", conf_name)).unwrap();
    }

    #[test]
    fn config_summary_serializes_full_contract() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = Workspace::new(tmp.path().to_path_buf());
        workspace.ensure_layout().unwrap();

        let payload = config_summary(&workspace).unwrap();

        assert_eq!(payload.version, app_meta::APP_VERSION);
        assert_eq!(
            payload.workspace_root,
            workspace.root().display().to_string()
        );
        assert_eq!(payload.bootstrap_mode, "plain");
        assert_eq!(payload.interpreter.program, python_program());
    }

    #[test]
    fn redacted_config_summary_masks_all_active_env_values() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = Workspace::new(tmp.path().to_path_buf());
        workspace.ensure_layout().unwrap();
        write_active_env(workspace.envs_dir(), "dev.conf", "HOST=localhost\n");

        let payload = redacted_config_summary(&workspace).unwrap();

        assert_eq!(payload.active_env_keys[0].key, "HOST");
        assert_eq!(payload.active_env_keys[0].value, MASKED_ENV_VALUE);
    }

    #[test]
    fn resolve_env_diagnostics_masks_sensitive_values() {
        let tmp = tempfile::TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        write_active_env(
            &envs,
            "dev.conf",
            "HOST=localhost\nAPI_KEY=supersecret123\n",
        );

        let (keys, _interp) = resolve_env_diagnostics(&envs);

        assert_eq!(keys[0].value, "localhost");
        assert_eq!(keys[1].value, MASKED_ENV_VALUE);
        assert!(keys.iter().all(|kv| kv.value != "supersecret123"));
    }

    #[test]
    fn resolve_env_diagnostics_masks_parent_sourced_secret_value() {
        let _guard = env_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        env::set_var("AWS_SECRET_ACCESS_KEY", "parent-secret-value");
        write_active_env(&envs, "dev.conf", "PLAIN=$AWS_SECRET_ACCESS_KEY\n");

        let (keys, _interp) = resolve_env_diagnostics(&envs);

        env::remove_var("AWS_SECRET_ACCESS_KEY");
        assert_eq!(keys[0].key, "PLAIN");
        assert_eq!(keys[0].value, MASKED_ENV_VALUE);
    }

    #[test]
    fn collect_env_overrides_masks_credential_values() {
        let _guard = env_lock();
        env::set_var(
            "OMAKURE_REPO",
            "https://user:secret@example.invalid/repo.git",
        );
        let overrides = collect_env_overrides();
        env::remove_var("OMAKURE_REPO");

        assert_eq!(
            overrides.get("OMAKURE_REPO"),
            Some(&MASKED_ENV_VALUE.to_string())
        );
    }
}

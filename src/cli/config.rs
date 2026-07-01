use crate::adapters::environments::{
    is_sensitive_key, resolve_active_env, should_mask_env_value, FsEnvironmentRepository,
    MASKED_ENV_VALUE,
};
use crate::app_meta;
use crate::cli::json;
use crate::ports::EnvironmentRepository;
use crate::runtime::{python_program, resolve_interpreter};
use crate::workspace::Workspace;
use serde::Serialize;
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};

/// JSON shape for `omakure config --json`.
#[derive(Debug, Serialize)]
pub struct ConfigPayload {
    pub version: String,
    pub binary: String,
    pub workspace_root: String,
    pub scripts_root: String,
    pub omaken_dir: String,
    pub history_dir: String,
    pub workspace_config: String,
    pub envs_dir: String,
    pub envs_active_path: String,
    pub active_env: Option<String>,
    pub bootstrap_mode: String,
    pub env_overrides: BTreeMap<String, String>,
    /// Resolved active-env `KEY=value` pairs, with sensitive values masked.
    /// Order and key case mirror the injected env (see `resolve_active_env`).
    pub active_env_keys: Vec<EnvVarView>,
    /// The interpreter omakure would spawn for `.py` scripts, resolved
    /// against the active env's `PATH` (falls back to name-based lookup).
    pub interpreter: InterpreterView,
}

/// A single resolved active-env entry. `value` is already masked for
/// sensitive keys (see [`is_sensitive_key`]); the raw value is never surfaced.
#[derive(Debug, Serialize)]
pub struct EnvVarView {
    pub key: String,
    pub value: String,
}

/// The interpreter that would run `.py` scripts. `path` is the absolute
/// binary resolved against the active env's `PATH`; `None` means omakure
/// falls back to name-based resolution of `program`.
#[derive(Debug, Serialize)]
pub struct InterpreterView {
    pub program: String,
    pub path: Option<String>,
}

pub fn run(scripts_dir: PathBuf, json_output: bool) -> Result<(), Box<dyn Error>> {
    let exe = env::current_exe()?;
    let workspace = Workspace::new(scripts_dir);
    let active_env = read_active_env(&workspace);
    let (active_env_keys, interpreter) = resolve_env_diagnostics(workspace.envs_dir());

    if json_output {
        let payload = ConfigPayload {
            version: app_meta::APP_VERSION.to_string(),
            binary: exe.display().to_string(),
            workspace_root: workspace.root().display().to_string(),
            scripts_root: workspace.scripts_root().display().to_string(),
            omaken_dir: workspace.omaken_dir().display().to_string(),
            history_dir: workspace.history_dir().display().to_string(),
            workspace_config: workspace.config_path().display().to_string(),
            envs_dir: workspace.envs_dir().display().to_string(),
            envs_active_path: workspace.envs_active_path().display().to_string(),
            active_env,
            // The CLI dispatch never opens the TUI, so `omakure config` is
            // always the global "plain" mode. The TUI bootstrap label is
            // a separate concern handled inside the TUI itself.
            bootstrap_mode: "plain".to_string(),
            env_overrides: collect_env_overrides(),
            active_env_keys,
            interpreter,
        };
        json::print_ok(payload);
        return Ok(());
    }

    println!("Version: {}", app_meta::APP_VERSION);
    println!("Binary: {}", exe.display());
    println!("Workspace root: {}", workspace.root().display());
    println!("Omaken dir: {}", workspace.omaken_dir().display());
    println!("History dir: {}", workspace.history_dir().display());
    println!("Workspace config: {}", workspace.config_path().display());
    println!("Environments dir: {}", workspace.envs_dir().display());
    println!(
        "Active environment file: {}",
        workspace.envs_active_path().display()
    );
    if let Some(env) = &active_env {
        println!("Active environment: {}", env);
    }
    print!("{}", render_env_diagnostics(&active_env_keys, &interpreter));

    print_env_if_set("OMAKURE_SCRIPTS_DIR");
    print_env_if_set("OMAKURE_REPO");
    print_env_if_set("REPO");
    print_env_if_set("VERSION");
    print_env_if_set("OVERTURE_SCRIPTS_DIR");
    print_env_if_set("OVERTURE_REPO");
    print_env_if_set("CLOUD_MGMT_SCRIPTS_DIR");
    print_env_if_set("CLOUD_MGMT_REPO");

    Ok(())
}

fn print_env_if_set(name: &str) {
    if let Ok(value) = env::var(name) {
        println!("{}: {}", name, mask_env_display_value(name, value));
    }
}

fn read_active_env(workspace: &Workspace) -> Option<String> {
    let repo = FsEnvironmentRepository::new(workspace.envs_dir());
    repo.load_environment_config().ok().and_then(|c| c.active)
}

/// Resolve the active env into masked `KEY=value` views plus the interpreter
/// that would actually run `.py` scripts.
///
/// Values for sensitive keys, credential-bearing URLs, and values that match a
/// sensitive parent-env value are masked with `****`; the raw value never
/// leaves [`resolve_active_env`]. The interpreter
/// path is resolved against the active env's `PATH` exactly as a real run
/// would (see [`resolve_interpreter`]), so users can confirm *which* python
/// omakure spawns and debug env/interpreter collisions.
pub(crate) fn resolve_env_diagnostics(envs_dir: &Path) -> (Vec<EnvVarView>, InterpreterView) {
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

fn mask_env_display_value(key: &str, value: String) -> String {
    if should_mask_env_value(key, &value) {
        MASKED_ENV_VALUE.to_string()
    } else {
        value
    }
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

fn should_mask_resolved_env_value(
    key: &str,
    value: &str,
    sensitive_parent_values: &[String],
) -> bool {
    should_mask_env_value(key, value)
        || (!value.is_empty() && sensitive_parent_values.iter().any(|secret| secret == value))
}

/// Render the human-readable env/interpreter diagnostics block for
/// `omakure config`. Kept pure (returns a `String`) so it is unit-testable.
fn render_env_diagnostics(keys: &[EnvVarView], interpreter: &InterpreterView) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    if keys.is_empty() {
        out.push_str("Active env keys: (none)\n");
    } else {
        out.push_str("Active env keys:\n");
        for kv in keys {
            let _ = writeln!(out, "  {} = {}", kv.key, kv.value);
        }
    }
    match &interpreter.path {
        Some(path) => {
            let _ = writeln!(
                out,
                "Resolved {} interpreter: {}",
                interpreter.program, path
            );
        }
        None => {
            let _ = writeln!(
                out,
                "Resolved {0} interpreter: {0} (name-based fallback; not found on active env PATH)",
                interpreter.program
            );
        }
    }
    out
}

pub(crate) fn collect_env_overrides() -> BTreeMap<String, String> {
    let names = [
        "OMAKURE_SCRIPTS_DIR",
        "OMAKURE_REPO",
        "REPO",
        "VERSION",
        "OVERTURE_SCRIPTS_DIR",
        "OVERTURE_REPO",
        "CLOUD_MGMT_SCRIPTS_DIR",
        "CLOUD_MGMT_REPO",
    ];
    let mut out = BTreeMap::new();
    for name in names {
        if let Ok(value) = env::var(name) {
            out.insert(name.to_string(), value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_env_overrides_only_known_names() {
        let overrides = collect_env_overrides();
        let known = [
            "OMAKURE_SCRIPTS_DIR",
            "OMAKURE_REPO",
            "REPO",
            "VERSION",
            "OVERTURE_SCRIPTS_DIR",
            "OVERTURE_REPO",
            "CLOUD_MGMT_SCRIPTS_DIR",
            "CLOUD_MGMT_REPO",
        ];
        for key in overrides.keys() {
            assert!(known.contains(&key.as_str()), "unexpected key: {}", key);
        }
    }

    #[test]
    fn test_collect_env_overrides_picks_up_set_vars() {
        env::set_var("OMAKURE_SCRIPTS_DIR", "/test/scripts");
        let overrides = collect_env_overrides();
        assert_eq!(
            overrides.get("OMAKURE_SCRIPTS_DIR"),
            Some(&"/test/scripts".to_string())
        );
        env::remove_var("OMAKURE_SCRIPTS_DIR");
    }

    #[test]
    fn test_run_human_and_json_modes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let scripts = tmp.path().to_path_buf();
        // Set one known env var so the print branch is exercised.
        env::set_var("REPO", "owner/repo");
        run(scripts.clone(), false).unwrap();
        run(scripts, true).unwrap();
        env::remove_var("REPO");
    }

    #[test]
    fn test_print_env_if_set_does_not_panic() {
        env::set_var("VERSION", "1.2.3");
        print_env_if_set("VERSION");
        env::remove_var("VERSION");
        // Missing var hits the silent branch.
        print_env_if_set("__omakure_no_such_env_var__");
    }

    #[test]
    fn test_read_active_env_returns_none_for_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = Workspace::new(tmp.path().to_path_buf());
        assert!(read_active_env(&workspace).is_none());
    }

    #[test]
    fn test_config_payload_serializes() {
        let payload = ConfigPayload {
            version: "0.1.8".to_string(),
            binary: "/usr/bin/omakure".to_string(),
            workspace_root: "/home/user/scripts".to_string(),
            scripts_root: "/home/user/scripts".to_string(),
            omaken_dir: "/home/user/scripts/.omaken".to_string(),
            history_dir: "/home/user/scripts/.history".to_string(),
            workspace_config: "/home/user/scripts/omakure.toml".to_string(),
            envs_dir: "/home/user/scripts/.omaken/envs".to_string(),
            envs_active_path: "/home/user/scripts/.omaken/envs/active".to_string(),
            active_env: None,
            bootstrap_mode: "plain".to_string(),
            env_overrides: BTreeMap::new(),
            active_env_keys: Vec::new(),
            interpreter: InterpreterView {
                program: "python3".to_string(),
                path: None,
            },
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"version\":\"0.1.8\""));
    }

    // --- Task 1757: env/interpreter surfacing in `omakure config` ---

    use std::fs;

    fn write_active_env(dir: &Path, conf_name: &str, contents: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(conf_name), contents).unwrap();
        fs::write(dir.join("active"), format!("{}\n", conf_name)).unwrap();
    }

    #[test]
    fn test_resolve_env_diagnostics_masks_sensitive_values() {
        let tmp = tempfile::TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        write_active_env(
            &envs,
            "dev.conf",
            "HOST=localhost\nAPI_KEY=supersecret123\n",
        );

        let (keys, _interp) = resolve_env_diagnostics(&envs);

        // Non-sensitive value is shown plainly, order preserved.
        assert_eq!(keys[0].key, "HOST");
        assert_eq!(keys[0].value, "localhost");
        // Sensitive value is masked — the raw secret must not appear.
        assert_eq!(keys[1].key, "API_KEY");
        assert_eq!(keys[1].value, "****");
        assert!(keys.iter().all(|kv| kv.value != "supersecret123"));
    }

    #[test]
    fn test_resolve_env_diagnostics_masks_parent_sourced_secret_value() {
        let tmp = tempfile::TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        env::set_var("AWS_SECRET_ACCESS_KEY", "parent-secret-value");
        write_active_env(&envs, "dev.conf", "PLAIN=$AWS_SECRET_ACCESS_KEY\n");

        let (keys, _interp) = resolve_env_diagnostics(&envs);

        env::remove_var("AWS_SECRET_ACCESS_KEY");
        assert_eq!(keys[0].key, "PLAIN");
        assert_eq!(keys[0].value, "****");
    }

    #[test]
    fn test_resolve_env_diagnostics_masks_credential_url_value() {
        let tmp = tempfile::TempDir::new().unwrap();
        let envs = tmp.path().join("envs");
        write_active_env(
            &envs,
            "dev.conf",
            "DATABASE_URL=postgres://user:pass@localhost/db\n",
        );

        let (keys, _interp) = resolve_env_diagnostics(&envs);

        assert_eq!(keys[0].key, "DATABASE_URL");
        assert_eq!(keys[0].value, "****");
    }

    #[test]
    fn test_mask_env_display_value_masks_sensitive_and_credential_values() {
        assert_eq!(
            mask_env_display_value("MYSQL_PWD", "secret".to_string()),
            "****"
        );
        assert_eq!(
            mask_env_display_value(
                "DATABASE_URL",
                "postgres://user:pass@localhost/db".to_string()
            ),
            "****"
        );
        assert_eq!(
            mask_env_display_value("VERSION", "1.2.3".to_string()),
            "1.2.3"
        );
    }

    #[test]
    fn test_render_env_diagnostics_includes_keys_and_masks_secret() {
        let keys = vec![
            EnvVarView {
                key: "HOST".to_string(),
                value: "localhost".to_string(),
            },
            EnvVarView {
                key: "API_KEY".to_string(),
                value: "****".to_string(),
            },
        ];
        let interpreter = InterpreterView {
            program: "python3".to_string(),
            path: Some("/proj/.venv/bin/python3".to_string()),
        };

        let rendered = render_env_diagnostics(&keys, &interpreter);

        // Keys are surfaced.
        assert!(rendered.contains("HOST = localhost"));
        assert!(rendered.contains("API_KEY = ****"));
        // The resolved interpreter absolute path is surfaced.
        assert!(rendered.contains("Resolved python3 interpreter: /proj/.venv/bin/python3"));
        // The raw secret never leaks (defense in depth).
        assert!(!rendered.contains("supersecret123"));
    }

    #[test]
    fn test_render_env_diagnostics_notes_name_based_fallback() {
        let interpreter = InterpreterView {
            program: "python3".to_string(),
            path: None,
        };
        let rendered = render_env_diagnostics(&[], &interpreter);
        assert!(rendered.contains("Active env keys: (none)"));
        assert!(rendered.contains("name-based fallback"));
    }

    /// With an injected `PATH` prepending a shim `python3`, the resolved
    /// interpreter must be the absolute shim path — proving `config` reports
    /// which interpreter a real run would actually spawn.
    #[cfg(unix)]
    #[test]
    fn test_resolve_env_diagnostics_resolves_interpreter_absolute_path() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let bin = tmp.path().join("venv-bin");
        fs::create_dir_all(&bin).unwrap();
        let shim = bin.join("python3");
        fs::write(&shim, "#!/bin/sh\necho shim\n").unwrap();
        let mut perms = fs::metadata(&shim).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&shim, perms).unwrap();

        let envs = tmp.path().join("envs");
        write_active_env(&envs, "dev.conf", &format!("PATH={}\n", bin.display()));

        let (_keys, interp) = resolve_env_diagnostics(&envs);
        assert_eq!(interp.path.as_deref(), Some(shim.to_str().unwrap()));
    }
}

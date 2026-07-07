use crate::cli::json;
use crate::operations::config::{self, ConfigSummary, EnvVarView, InterpreterView};
use crate::workspace::Workspace;
use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;

pub type ConfigPayload = ConfigSummary;

pub fn run(scripts_dir: PathBuf, json_output: bool) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    let payload = config::config_summary(&workspace)?;

    if json_output {
        json::print_ok(payload);
        return Ok(());
    }

    print_human(&payload);
    Ok(())
}

fn print_human(payload: &ConfigPayload) {
    println!("Version: {}", payload.version);
    println!("Binary: {}", payload.binary);
    println!("Workspace root: {}", payload.workspace_root);
    println!("Omakure dir: {}", payload.omakure_dir);
    println!("History dir: {}", payload.history_dir);
    println!("Workspace config: {}", payload.workspace_config);
    println!("Environments dir: {}", payload.envs_dir);
    println!("Active environment file: {}", payload.envs_active_path);
    if let Some(env) = &payload.active_env {
        println!("Active environment: {}", env);
    }
    print!(
        "{}",
        render_env_diagnostics(&payload.active_env_keys, &payload.interpreter)
    );
    print_env_overrides(&payload.env_overrides);
}

fn print_env_overrides(overrides: &BTreeMap<String, String>) {
    for name in config::env_override_names() {
        if let Some(value) = overrides.get(name) {
            println!("{}: {}", name, value);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_meta;

    #[test]
    fn test_config_payload_serializes() {
        let payload = ConfigPayload {
            version: "0.1.8".to_string(),
            binary: "/usr/bin/omakure".to_string(),
            workspace_root: "/home/user/scripts".to_string(),
            scripts_root: "/home/user/scripts".to_string(),
            omakure_dir: "/home/user/scripts/.omakure".to_string(),
            history_dir: "/home/user/scripts/.history".to_string(),
            workspace_config: "/home/user/scripts/omakure.toml".to_string(),
            envs_dir: "/home/user/scripts/.omakure/envs".to_string(),
            envs_active_path: "/home/user/scripts/.omakure/envs/active".to_string(),
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

    #[test]
    fn test_run_human_and_json_modes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let scripts = tmp.path().to_path_buf();
        run(scripts.clone(), false).unwrap();
        run(scripts, true).unwrap();
    }

    #[test]
    fn test_config_summary_returns_current_version() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workspace = Workspace::new(tmp.path().to_path_buf());
        let payload = config::config_summary(&workspace).unwrap();
        assert_eq!(payload.version, app_meta::APP_VERSION);
        assert!(payload.active_env.is_none());
    }

    #[test]
    fn test_print_env_overrides_uses_stable_order() {
        let mut overrides = BTreeMap::new();
        overrides.insert("VERSION".to_string(), "1.2.3".to_string());
        overrides.insert("REPO".to_string(), "owner/repo".to_string());
        print_env_overrides(&overrides);
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

        assert!(rendered.contains("HOST = localhost"));
        assert!(rendered.contains("API_KEY = ****"));
        assert!(rendered.contains("Resolved python3 interpreter: /proj/.venv/bin/python3"));
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
}

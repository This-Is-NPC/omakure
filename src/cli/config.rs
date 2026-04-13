use crate::adapters::environments::FsEnvironmentRepository;
use crate::app_meta;
use crate::cli::json;
use crate::ports::EnvironmentRepository;
use crate::workspace::Workspace;
use serde::Serialize;
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::path::PathBuf;

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
}

pub fn run(scripts_dir: PathBuf, json_output: bool) -> Result<(), Box<dyn Error>> {
    let exe = env::current_exe()?;
    let workspace = Workspace::new(scripts_dir);
    let active_env = read_active_env(&workspace);

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
        println!("{}: {}", name, value);
    }
}

fn read_active_env(workspace: &Workspace) -> Option<String> {
    let repo = FsEnvironmentRepository::new(workspace.envs_dir());
    repo.load_environment_config().ok().and_then(|c| c.active)
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
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"version\":\"0.1.8\""));
    }
}

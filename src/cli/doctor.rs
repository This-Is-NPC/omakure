use crate::adapters::system_checks::{
    ensure_bash_installed, ensure_git_installed, ensure_jq_installed, ensure_powershell_installed,
    ensure_python_installed,
};
use crate::workspace::Workspace;
use std::error::Error;
use std::path::PathBuf;

pub fn run(scripts_dir: PathBuf) -> Result<(), Box<dyn Error>> {
    let mut ok = true;
    let workspace = Workspace::new(scripts_dir);

    println!("Checks:");
    ok &= print_required("git", ensure_git_installed());
    ok &= print_required("bash", ensure_bash_installed());
    ok &= print_required("jq", ensure_jq_installed());
    print_optional("powershell", ensure_powershell_installed());
    print_optional("python", ensure_python_installed());

    print_workspace_path("workspace_root", workspace.root());
    print_workspace_path("omaken_dir", workspace.omaken_dir());
    print_workspace_path("history_dir", workspace.history_dir());
    print_workspace_path("workspace_config", workspace.config_path());

    if !ok {
        println!("One or more checks failed.");
        std::process::exit(1);
    }

    println!("All checks passed.");
    Ok(())
}

fn print_required<E: std::fmt::Display>(label: &str, result: Result<(), E>) -> bool {
    match result {
        Ok(()) => {
            println!("  {}: OK", label);
            true
        }
        Err(err) => {
            println!("  {}: ERROR - {}", label, err);
            false
        }
    }
}

fn print_optional<E: std::fmt::Display>(label: &str, result: Result<(), E>) {
    match result {
        Ok(()) => {
            println!("  {}: OK", label);
        }
        Err(err) => {
            println!("  {}: WARN - {}", label, err);
        }
    }
}

fn print_workspace_path(label: &str, path: &std::path::Path) {
    if path.exists() {
        println!("  {}: OK - {}", label, path.display());
    } else {
        println!("  {}: WARN - {} (not created yet)", label, path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_required_ok() {
        let result: Result<(), String> = Ok(());
        assert!(print_required("test", result));
    }

    #[test]
    fn test_print_required_err() {
        let result: Result<(), String> = Err("fail".to_string());
        assert!(!print_required("test", result));
    }
}

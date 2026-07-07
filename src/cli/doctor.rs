use crate::adapters::system_checks::{
    ensure_bash_installed, ensure_git_installed, ensure_jq_installed, ensure_powershell_installed,
    ensure_python_installed,
};
use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::ports::ScriptRepository;
use crate::workspace::Workspace;
use std::error::Error;
use std::path::{Path, PathBuf};

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
    print_workspace_path("omakure_dir", workspace.omakure_dir());
    print_workspace_path("history_dir", workspace.history_dir());
    print_workspace_path("workspace_config", workspace.config_path());

    print_schemas_check(workspace.root());

    if !ok {
        println!("One or more checks failed.");
        std::process::exit(1);
    }

    println!("All checks passed.");
    Ok(())
}

/// Walk the workspace and try to parse each script's schema. Reports
/// unparseable scripts as a non-blocking WARN so users discover broken
/// schemas without opening the TUI.
pub(crate) fn check_schemas(root: &Path) -> (usize, Vec<(PathBuf, String)>) {
    let repo = FsWorkspaceRepository::new(root.to_path_buf());
    let scripts = match repo.list_scripts_recursive() {
        Ok(s) => s,
        Err(_) => return (0, Vec::new()),
    };
    let total = scripts.len();
    let mut failures = Vec::new();
    for script in scripts {
        if let Err(err) = repo.read_schema(&script) {
            let rel = script.strip_prefix(root).unwrap_or(&script).to_path_buf();
            failures.push((rel, err.to_string()));
        }
    }
    (total, failures)
}

fn print_schemas_check(root: &Path) {
    let (total, failures) = check_schemas(root);
    if total == 0 {
        println!("  schemas: WARN - no scripts found");
        return;
    }
    let parsed = total - failures.len();
    if failures.is_empty() {
        println!("  schemas: OK - {}/{} parseable", parsed, total);
        return;
    }
    println!("  schemas: WARN - {}/{} invalid", failures.len(), total);
    for (path, err) in failures {
        println!("    {}: {}", path.display(), err);
    }
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

    #[test]
    fn test_check_schemas_reports_parseable_and_failures() {
        use std::fs;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Good script — parses cleanly.
        fs::write(
            root.join("good.sh"),
            "#!/bin/bash\n# OMAKURE_SCHEMA_START\n# {\"Name\": \"Good\", \"Fields\": []}\n# OMAKURE_SCHEMA_END\necho ok\n",
        )
        .unwrap();

        // Good ps1 without Order — would have failed before the normalize fix;
        // doctor must now report it as parseable.
        fs::write(
            root.join("teams.ps1"),
            "# OMAKURE_SCHEMA_START\n# {\"Name\": \"T\", \"Fields\": [{\"Name\": \"x\", \"Type\": \"string\"}]}\n# OMAKURE_SCHEMA_END\n",
        )
        .unwrap();

        // Broken script — SCHEMA_START without SCHEMA_END.
        fs::write(
            root.join("broken.sh"),
            "#!/bin/bash\n# OMAKURE_SCHEMA_START\n# {\"Name\": \"Broken\"}\necho still open\n",
        )
        .unwrap();

        let (total, failures) = check_schemas(root);
        assert_eq!(total, 3);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, PathBuf::from("broken.sh"));
    }

    #[test]
    fn test_check_schemas_empty_workspace_reports_zero() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let (total, failures) = check_schemas(tmp.path());
        assert_eq!(total, 0);
        assert!(failures.is_empty());
    }
}

use crate::adapters::system_checks::{
    ensure_bash_installed, ensure_git_installed, ensure_jq_installed, ensure_powershell_installed,
    ensure_python_installed,
};
use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::ports::ScriptRepository;
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::OperationResult;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub dependencies: Vec<DoctorCheck>,
    pub workspace_paths: Vec<WorkspacePathCheck>,
    pub schemas: SchemaCheckReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub label: String,
    pub required: bool,
    pub ok: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePathCheck {
    pub label: String,
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaCheckReport {
    pub total: usize,
    pub parsed: usize,
    pub failures: Vec<SchemaFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaFailure {
    pub path: PathBuf,
    pub error: String,
}

pub fn doctor_report(workspace: &Workspace) -> OperationResult<DoctorReport> {
    let dependencies = vec![
        required_check("git", ensure_git_installed()),
        required_check("bash", ensure_bash_installed()),
        required_check("jq", ensure_jq_installed()),
        optional_check("powershell", ensure_powershell_installed()),
        optional_check("python", ensure_python_installed()),
        // Lua is deliberately absent. This list reports *host* dependencies
        // that can be missing; the Lua runtime is compiled into this binary, so
        // an entry here would always pass and would read as a dependency the
        // operator has to satisfy. Do not add one.
    ];
    let workspace_paths = vec![
        workspace_path("workspace_root", workspace.root()),
        workspace_path("omakure_dir", workspace.omakure_dir()),
        workspace_path("history_dir", workspace.history_dir()),
        workspace_path("workspace_config", workspace.config_path()),
    ];
    let schemas = check_schemas(workspace.scripts_root());
    let ok = dependencies
        .iter()
        .filter(|check| check.required)
        .all(|check| check.ok);
    Ok(DoctorReport {
        ok,
        dependencies,
        workspace_paths,
        schemas,
    })
}

pub fn check_schemas(root: &Path) -> SchemaCheckReport {
    let repo = FsWorkspaceRepository::new(root.to_path_buf());
    let scripts = match repo.list_scripts_recursive() {
        Ok(s) => s,
        Err(_) => {
            return SchemaCheckReport {
                total: 0,
                parsed: 0,
                failures: Vec::new(),
            }
        }
    };
    let total = scripts.len();
    let mut failures = Vec::new();
    for script in scripts {
        if let Err(err) = repo.read_schema(&script) {
            let rel = script.strip_prefix(root).unwrap_or(&script).to_path_buf();
            failures.push(SchemaFailure {
                path: rel,
                error: err.to_string(),
            });
        }
    }
    SchemaCheckReport {
        total,
        parsed: total - failures.len(),
        failures,
    }
}

fn required_check<E: std::fmt::Display>(label: &str, result: Result<(), E>) -> DoctorCheck {
    dependency_check(label, true, result)
}

fn optional_check<E: std::fmt::Display>(label: &str, result: Result<(), E>) -> DoctorCheck {
    dependency_check(label, false, result)
}

fn dependency_check<E: std::fmt::Display>(
    label: &str,
    required: bool,
    result: Result<(), E>,
) -> DoctorCheck {
    match result {
        Ok(()) => DoctorCheck {
            label: label.to_string(),
            required,
            ok: true,
            message: None,
        },
        Err(err) => DoctorCheck {
            label: label.to_string(),
            required,
            ok: false,
            message: Some(err.to_string()),
        },
    }
}

fn workspace_path(label: &str, path: &Path) -> WorkspacePathCheck {
    WorkspacePathCheck {
        label: label.to_string(),
        path: path.display().to_string(),
        exists: path.exists(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn check_schemas_reports_parseable_and_failures() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("good.sh"),
            "#!/bin/bash\n# OMAKURE_SCHEMA_START\n# {\"Name\": \"Good\", \"Fields\": []}\n# OMAKURE_SCHEMA_END\necho ok\n",
        )
        .unwrap();
        fs::write(
            root.join("broken.sh"),
            "#!/bin/bash\n# OMAKURE_SCHEMA_START\n# {\"Name\": \"Broken\"}\necho still open\n",
        )
        .unwrap();

        let report = check_schemas(root);

        assert_eq!(report.total, 2);
        assert_eq!(report.parsed, 1);
        assert_eq!(report.failures[0].path, PathBuf::from("broken.sh"));
    }

    #[test]
    fn check_schemas_empty_workspace_reports_zero() {
        let tmp = TempDir::new().unwrap();
        let report = check_schemas(tmp.path());
        assert_eq!(report.total, 0);
        assert_eq!(report.parsed, 0);
        assert!(report.failures.is_empty());
    }

    #[test]
    fn doctor_report_contains_workspace_and_schema_sections() {
        let tmp = TempDir::new().unwrap();
        let workspace = Workspace::new(tmp.path().to_path_buf());
        workspace.ensure_layout().unwrap();

        let report = doctor_report(&workspace).unwrap();

        assert!(report
            .workspace_paths
            .iter()
            .any(|path| path.label == "workspace_root"));
        assert_eq!(report.schemas.total, 0);
    }
}

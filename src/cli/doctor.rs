use crate::operations::doctor::{self, DoctorCheck, DoctorReport, SchemaCheckReport};
use crate::workspace::Workspace;
use std::error::Error;
use std::path::PathBuf;

pub fn run(scripts_dir: PathBuf) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    let report = doctor::doctor_report(&workspace)?;

    print_report(&report);
    if !report.ok {
        println!("One or more checks failed.");
        std::process::exit(1);
    }

    println!("All checks passed.");
    Ok(())
}

fn print_report(report: &DoctorReport) {
    println!("Checks:");
    for check in &report.dependencies {
        if check.required {
            print_required(check);
        } else {
            print_optional(check);
        }
    }
    for path in &report.workspace_paths {
        if path.exists {
            println!("  {}: OK - {}", path.label, path.path);
        } else {
            println!("  {}: WARN - {} (not created yet)", path.label, path.path);
        }
    }
    print_schemas_report(&report.schemas);
}

#[cfg(test)]
pub(crate) fn check_schemas(root: &std::path::Path) -> (usize, Vec<(PathBuf, String)>) {
    let report = doctor::check_schemas(root);
    (
        report.total,
        report
            .failures
            .into_iter()
            .map(|failure| (failure.path, failure.error))
            .collect(),
    )
}

fn print_schemas_report(report: &SchemaCheckReport) {
    if report.total == 0 {
        println!("  schemas: WARN - no scripts found");
        return;
    }
    if report.failures.is_empty() {
        println!(
            "  schemas: OK - {}/{} parseable",
            report.parsed, report.total
        );
        return;
    }
    println!(
        "  schemas: WARN - {}/{} invalid",
        report.failures.len(),
        report.total
    );
    for failure in &report.failures {
        println!("    {}: {}", failure.path.display(), failure.error);
    }
}

fn print_required(check: &DoctorCheck) -> bool {
    if check.ok {
        println!("  {}: OK", check.label);
        true
    } else {
        println!(
            "  {}: ERROR - {}",
            check.label,
            check.message.as_deref().unwrap_or("unknown error")
        );
        false
    }
}

fn print_optional(check: &DoctorCheck) {
    if check.ok {
        println!("  {}: OK", check.label);
    } else {
        println!(
            "  {}: WARN - {}",
            check.label,
            check.message.as_deref().unwrap_or("unknown error")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::doctor::{SchemaFailure, WorkspacePathCheck};

    #[test]
    fn test_print_required_ok() {
        let check = DoctorCheck {
            label: "test".into(),
            required: true,
            ok: true,
            message: None,
        };
        assert!(print_required(&check));
    }

    #[test]
    fn test_print_required_err() {
        let check = DoctorCheck {
            label: "test".into(),
            required: true,
            ok: false,
            message: Some("fail".into()),
        };
        assert!(!print_required(&check));
    }

    #[test]
    fn test_check_schemas_reports_parseable_and_failures() {
        use std::fs;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(
            root.join("good.sh"),
            "#!/bin/bash\n# OMAKURE_SCHEMA_START\n# {\"Name\": \"Good\", \"Fields\": []}\n# OMAKURE_SCHEMA_END\necho ok\n",
        )
        .unwrap();
        fs::write(
            root.join("teams.ps1"),
            "# OMAKURE_SCHEMA_START\n# {\"Name\": \"T\", \"Fields\": [{\"Name\": \"x\", \"Type\": \"string\"}]}\n# OMAKURE_SCHEMA_END\n",
        )
        .unwrap();
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
        let tmp = tempfile::TempDir::new().unwrap();
        let (total, failures) = check_schemas(tmp.path());
        assert_eq!(total, 0);
        assert!(failures.is_empty());
    }

    #[test]
    fn print_report_handles_all_sections() {
        let report = DoctorReport {
            ok: true,
            dependencies: vec![DoctorCheck {
                label: "git".into(),
                required: true,
                ok: true,
                message: None,
            }],
            workspace_paths: vec![WorkspacePathCheck {
                label: "workspace_root".into(),
                path: "/tmp".into(),
                exists: true,
            }],
            schemas: SchemaCheckReport {
                total: 1,
                parsed: 0,
                failures: vec![SchemaFailure {
                    path: PathBuf::from("broken.sh"),
                    error: "bad".into(),
                }],
            },
        };

        print_report(&report);
    }
}

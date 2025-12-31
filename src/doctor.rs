use crate::adapters::system_checks::{
    ensure_bash_installed, ensure_git_installed, ensure_jq_installed, ensure_powershell_installed,
    ensure_python_installed,
};
use crate::workspace::Workspace;
use std::error::Error;
use std::path::PathBuf;

pub struct DoctorOptions {
    pub scripts_dir: PathBuf,
}

pub fn print_doctor_help() {
    println!(
        "Usage: omakure doctor\n\n\
Aliases:\n\
  check\n\n\
Notes:\n\
  Validates runtimes and workspace paths (PowerShell/Python are optional).\n\n\
Environment:\n\
  OMAKURE_SCRIPTS_DIR  Scripts directory override\n\
  OVERTURE_SCRIPTS_DIR  Legacy scripts directory override\n\
  CLOUD_MGMT_SCRIPTS_DIR  Legacy scripts directory override"
    );
}

pub fn parse_doctor_args(
    args: &[String],
    scripts_dir: PathBuf,
) -> Result<DoctorOptions, Box<dyn Error>> {
    if !args.is_empty() {
        return Err("doctor does not accept arguments".into());
    }
    Ok(DoctorOptions { scripts_dir })
}

pub fn run_doctor(options: DoctorOptions) -> Result<(), Box<dyn Error>> {
    let mut ok = true;
    let workspace = Workspace::new(options.scripts_dir);

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

fn print_required(label: &str, result: Result<(), Box<dyn Error>>) -> bool {
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

fn print_optional(label: &str, result: Result<(), Box<dyn Error>>) {
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

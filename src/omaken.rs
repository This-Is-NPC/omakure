use crate::adapters::system_checks::ensure_git_installed;
use crate::workspace::Workspace;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

pub struct OmakenListOptions {
    pub workspace_root: PathBuf,
}

pub struct OmakenInstallOptions {
    pub workspace_root: PathBuf,
    pub url: String,
    pub name: Option<String>,
}

pub fn print_list_help() {
    println!(
        "Usage: omakure list\n\n\
Notes:\n\
  Lists installed Omaken flavors in .omaken.\n\n\
Environment:\n\
  OMAKURE_SCRIPTS_DIR  Workspace root override"
    );
}

pub fn print_install_help() {
    println!(
        "Usage: omakure install <git-url> [--name <name>]\n\n\
Notes:\n\
  Installs a flavor into .omaken from a Git repository.\n\n\
Environment:\n\
  OMAKURE_SCRIPTS_DIR  Workspace root override"
    );
}

pub fn parse_list_args(
    args: &[String],
    workspace_root: PathBuf,
) -> Result<OmakenListOptions, Box<dyn Error>> {
    if !args.is_empty() {
        return Err("list does not accept arguments".into());
    }
    Ok(OmakenListOptions { workspace_root })
}

pub fn parse_install_args(
    args: &[String],
    workspace_root: PathBuf,
) -> Result<OmakenInstallOptions, Box<dyn Error>> {
    if args.is_empty() {
        return Err("Missing git URL. Use `omakure install <git-url>`.".into());
    }

    let mut url = None;
    let mut name = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--name" => {
                let value = args.get(i + 1).ok_or("Missing value for --name")?;
                name = Some(value.to_string());
                i += 2;
            }
            value if url.is_none() => {
                url = Some(value.to_string());
                i += 1;
            }
            unknown => {
                return Err(format!("Unknown install arg: {}", unknown).into());
            }
        }
    }

    let url = url.ok_or("Missing git URL. Use `omakure install <git-url>`.")?;
    Ok(OmakenInstallOptions {
        workspace_root,
        url,
        name,
    })
}

pub fn run_list(options: OmakenListOptions) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(options.workspace_root);
    workspace.ensure_layout()?;
    list_omaken(&workspace)
}

pub fn run_install(options: OmakenInstallOptions) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(options.workspace_root);
    workspace.ensure_layout()?;
    install_omaken(&workspace, &options.url, options.name.as_deref())
}

fn list_omaken(workspace: &Workspace) -> Result<(), Box<dyn Error>> {
    let mut flavors = Vec::new();
    for entry in fs::read_dir(workspace.omaken_dir())? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                flavors.push(name.to_string());
            }
        }
    }
    flavors.sort();
    if flavors.is_empty() {
        println!("No Omaken flavors installed.");
    } else {
        println!("Omaken flavors:");
        for name in flavors {
            println!(" - {}", name);
        }
    }
    Ok(())
}

fn install_omaken(
    workspace: &Workspace,
    url: &str,
    override_name: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    ensure_git_installed()?;
    let name = override_name
        .map(|name| name.to_string())
        .unwrap_or_else(|| infer_name_from_url(url));
    if name.trim().is_empty() {
        return Err("Could not infer a folder name from the URL".into());
    }
    let target_dir = workspace.omaken_dir().join(&name);
    if target_dir.exists() {
        return Err(format!(
            "Omaken already exists: {}",
            target_dir.display()
        )
        .into());
    }

    let status = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg(url)
        .arg(&target_dir)
        .status()?;
    if !status.success() {
        return Err("git clone failed".into());
    }

    println!("Installed Omaken flavor to {}", target_dir.display());
    Ok(())
}

fn infer_name_from_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    last.trim_end_matches(".git").to_string()
}

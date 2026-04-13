use crate::adapters::system_checks::ensure_git_installed;
use crate::cli::args::OmakenInstallArgs;
use crate::workspace::Workspace;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

pub fn run_list(workspace_root: PathBuf) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(workspace_root);
    workspace.ensure_layout()?;
    list_omaken(&workspace)
}

pub fn run_install(
    workspace_root: PathBuf,
    options: OmakenInstallArgs,
) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(workspace_root);
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
        return Err(format!("Omaken already exists: {}", target_dir.display()).into());
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use std::fs;
    use tempfile::TempDir;

    #[rstest]
    #[case::https_url("https://github.com/user/my-scripts.git", "my-scripts")]
    #[case::https_no_git("https://github.com/user/my-scripts", "my-scripts")]
    #[case::trailing_slash("https://github.com/user/repo/", "repo")]
    #[case::ssh_url("git@github.com:user/scripts.git", "scripts")]
    fn test_infer_name_from_url(#[case] url: &str, #[case] expected: &str) {
        assert_eq!(infer_name_from_url(url), expected);
    }

    #[test]
    fn test_list_omaken_empty() {
        let tmp = TempDir::new().unwrap();
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        // Should not error, just print "No Omaken flavors installed"
        assert!(list_omaken(&ws).is_ok());
    }

    #[test]
    fn test_list_omaken_with_flavors() {
        let tmp = TempDir::new().unwrap();
        let ws = crate::workspace::Workspace::new(tmp.path().to_path_buf());
        ws.ensure_layout().unwrap();
        fs::create_dir_all(ws.omaken_dir().join("my-flavor")).unwrap();

        assert!(list_omaken(&ws).is_ok());
    }
}

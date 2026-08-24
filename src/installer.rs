#[cfg(windows)]
use std::error::Error;
#[cfg(windows)]
use std::path::Path;

#[cfg(windows)]
fn main() -> Result<(), Box<dyn Error>> {
    use std::fs;
    use winreg::enums::*;
    use winreg::RegKey;

    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let install_service = args.iter().any(|arg| arg == "--install-node-service");
    let uninstall_service = args.iter().any(|arg| arg == "--uninstall-node-service");
    let reset_state = args.iter().any(|arg| arg == "--uninstall-node-state");
    let confirmed = args.iter().any(|arg| arg == "--confirmed");
    if install_service && uninstall_service {
        return Err(
            "--install-node-service and --uninstall-node-service cannot be combined".into(),
        );
    }
    if reset_state && (!uninstall_service || !confirmed) {
        return Err("--uninstall-node-state requires --uninstall-node-service --confirmed".into());
    }
    if uninstall_service {
        require_administrator()?;
        uninstall_windows_node_service(reset_state)?;
        return Ok(());
    }

    let installer_path = std::env::current_exe()?;
    let installer_dir = installer_path
        .parent()
        .ok_or("Unable to determine installer directory")?;
    let source_exe = installer_dir.join("omakure.exe");
    if !source_exe.exists() {
        return Err("omakure.exe not found next to the installer".into());
    }

    let install_dir = if install_service {
        require_administrator()?;
        default_service_install_dir()?
    } else {
        default_install_dir()?
    };
    fs::create_dir_all(&install_dir)?;
    let target_exe = install_dir.join("omakure.exe");
    fs::copy(&source_exe, &target_exe)?;

    if install_service {
        let tokens = argument_value(&args, "--node-tokens-file")
            .ok_or("--install-node-service requires --node-tokens-file")?;
        let tokens = Path::new(tokens);
        if !tokens.is_file() {
            return Err("--node-tokens-file must name an existing hashed tokens TOML".into());
        }
        install_windows_node_service(&target_exe, tokens)?;
    }

    add_to_user_path(&install_dir)?;

    println!("Installed to {}", target_exe.display());
    println!("Open a new terminal and run `omakure`.");
    Ok(())
}

#[cfg(windows)]
fn argument_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

#[cfg(windows)]
fn require_administrator() -> Result<(), Box<dyn Error>> {
    let status = std::process::Command::new("net")
        .args(["session"])
        .status()?;
    if !status.success() {
        return Err(
            "machine node-service provisioning requires an elevated Administrator shell".into(),
        );
    }
    Ok(())
}

#[cfg(windows)]
fn default_service_install_dir() -> Result<std::path::PathBuf, Box<dyn Error>> {
    std::env::var_os("ProgramFiles")
        .map(std::path::PathBuf::from)
        .map(|path| path.join("Omakure"))
        .ok_or_else(|| "ProgramFiles is not set".into())
}

#[cfg(windows)]
fn install_windows_node_service(binary: &Path, tokens_source: &Path) -> Result<(), Box<dyn Error>> {
    use std::fs;
    use std::process::Command;

    let program_data = std::env::var_os("ProgramData")
        .map(std::path::PathBuf::from)
        .ok_or("ProgramData is not set")?;
    let root = program_data.join("Omakure");
    let workspace = program_data.join("Omakure-Workspace");
    let config = root.join("node.toml");
    let tokens = root.join("tokens.toml");
    prepare_windows_node_acl(&root)?;
    let result = (|| {
        fs::create_dir_all(&root)?;
        fs::create_dir_all(&workspace)?;
        if !config.exists() {
            fs::write(
                &config,
                "version = 1\n\n[node]\ndisplay_name = \"\"\n\n[api]\nbind = \"127.0.0.1:7878\"\n\n[network]\nmode = \"direct\"\nrelays = []\nstatic_peers = []\nmax_message_bytes = 1048576\n\n[trust]\nenrollment = \"disabled\"\nallow_remote_cues = false\nallow_baseline_push = false\n\n[organization]\nid = \"\"\ndiscovery_secret_ref = \"\"\n",
            )?;
        }
        fs::copy(tokens_source, &tokens)?;
        let binary = binary.to_string_lossy();
        let workspace = workspace.to_string_lossy();
        let tokens = tokens.to_string_lossy();
        let bin_path = format!(
            "\"{}\" --scripts-dir \"{}\" node serve --tokens-file \"{}\"",
            binary, workspace, tokens
        );
        let action = if Command::new("sc.exe")
            .args(["query", "OmakureNode"])
            .status()?
            .success()
        {
            "config"
        } else {
            "create"
        };
        let status = Command::new("sc.exe")
            .args([
                action,
                "OmakureNode",
                "binPath=",
                &bin_path,
                "obj=",
                "NT AUTHORITY\\LocalService",
                "start=",
                "auto",
                "DisplayName=",
                "Omakure Machine Node Service",
            ])
            .status()?;
        if !status.success() {
            return Err("sc.exe could not register OmakureNode".into());
        }
        Ok::<(), Box<dyn Error>>(())
    })();
    let acl_result = restore_windows_node_acl(&root, &workspace, &config, &tokens);
    result?;
    acl_result
}

#[cfg(windows)]
fn prepare_windows_node_acl(root: &std::path::Path) -> Result<(), Box<dyn Error>> {
    use std::process::Command;
    if !root.exists() {
        return Ok(());
    }
    let takeown = Command::new("takeown")
        .args(["/F", root.to_string_lossy().as_ref(), "/R", "/D", "Y"])
        .status()?;
    if !takeown.success() {
        return Err("could not take temporary ownership of node state".into());
    }
    let grant = Command::new("icacls")
        .args([
            root.to_string_lossy().as_ref(),
            "/grant:r",
            "BUILTIN\\Administrators:(OI)(CI)F",
            "/T",
            "/C",
        ])
        .status()?;
    if !grant.success() {
        return Err("could not prepare elevated access to node state".into());
    }
    Ok(())
}

#[cfg(windows)]
fn restore_windows_node_acl(
    root: &std::path::Path,
    workspace: &std::path::Path,
    config: &std::path::Path,
    tokens: &std::path::Path,
) -> Result<(), Box<dyn Error>> {
    use std::fs;

    let files = fs::read_dir(root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    for path in files {
        set_windows_node_acl(&path, false, false)?;
    }
    set_windows_node_acl(root, true, true)?;
    set_windows_node_acl(workspace, true, true)?;
    set_windows_node_acl(config, false, false)?;
    set_windows_node_acl(tokens, false, false)?;
    Ok(())
}

#[cfg(windows)]
fn set_windows_node_acl(
    path: &std::path::Path,
    directory: bool,
    service_modify: bool,
) -> Result<(), Box<dyn Error>> {
    use std::process::Command;
    if !path.exists() {
        return Ok(());
    }
    let system = if directory {
        "SYSTEM:(OI)(CI)F"
    } else {
        "SYSTEM:F"
    };
    let service = if directory {
        "NT AUTHORITY\\LocalService:(OI)(CI)M"
    } else if service_modify {
        "NT AUTHORITY\\LocalService:M"
    } else {
        "NT AUTHORITY\\LocalService:R"
    };
    let status = Command::new("icacls")
        .args([
            path.to_string_lossy().as_ref(),
            "/inheritance:r",
            "/setowner",
            "NT AUTHORITY\\SYSTEM",
            "/remove:g",
            "BUILTIN\\Administrators",
            "NT SERVICE\\OmakureNode",
            "Users",
            "Authenticated Users",
            "Everyone",
            "/grant:r",
            system,
            service,
        ])
        .status()?;
    if !status.success() {
        return Err(format!("could not set exact node ACL for {}", path.display()).into());
    }
    Ok(())
}

#[cfg(windows)]
fn uninstall_windows_node_service(reset_state: bool) -> Result<(), Box<dyn Error>> {
    use std::fs;
    use std::process::Command;
    let _ = Command::new("sc.exe")
        .args(["stop", "OmakureNode"])
        .status()?;
    let _ = Command::new("sc.exe")
        .args(["delete", "OmakureNode"])
        .status()?;
    if reset_state {
        let program_data = std::env::var_os("ProgramData")
            .map(std::path::PathBuf::from)
            .ok_or("ProgramData is not set")?;
        let _ = fs::remove_dir_all(program_data.join("Omakure"));
        let _ = fs::remove_dir_all(program_data.join("Omakure-Workspace"));
    }
    Ok(())
}

#[cfg(windows)]
fn default_install_dir() -> Result<std::path::PathBuf, Box<dyn Error>> {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        Ok(std::path::PathBuf::from(local).join("omakure").join("bin"))
    } else if let Ok(profile) = std::env::var("USERPROFILE") {
        Ok(std::path::PathBuf::from(profile)
            .join("AppData")
            .join("Local")
            .join("omakure")
            .join("bin"))
    } else {
        Err("LOCALAPPDATA/USERPROFILE not found".into())
    }
}

#[cfg(windows)]
fn add_to_user_path(dir: &Path) -> Result<(), Box<dyn Error>> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (env, _) = hkcu.create_subkey("Environment")?;
    let current: String = env.get_value("Path").unwrap_or_default();
    let dir_str = dir.to_string_lossy().to_string();
    let normalized_dir = normalize_path(&dir_str);

    let mut exists = false;
    for entry in current.split(';').filter(|entry| !entry.is_empty()) {
        if normalize_path(entry) == normalized_dir {
            exists = true;
            break;
        }
    }

    if !exists {
        let new_value = if current.trim().is_empty() {
            dir_str.clone()
        } else {
            format!("{};{}", current, dir_str)
        };
        env.set_value("Path", &new_value)?;
        println!("Added to PATH: {}", dir_str);
    } else {
        println!("PATH already contains: {}", dir_str);
    }

    Ok(())
}

#[cfg(windows)]
fn normalize_path(input: &str) -> String {
    input
        .trim_matches('"')
        .trim_end_matches('\\')
        .to_lowercase()
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This installer is for Windows only.");
    std::process::exit(1);
}

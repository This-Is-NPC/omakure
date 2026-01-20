#[cfg(windows)]
use std::error::Error;
#[cfg(windows)]
use std::path::Path;

#[cfg(windows)]
fn main() -> Result<(), Box<dyn Error>> {
    use std::fs;
    use winreg::enums::*;
    use winreg::RegKey;

    let installer_path = std::env::current_exe()?;
    let installer_dir = installer_path
        .parent()
        .ok_or("Unable to determine installer directory")?;
    let source_exe = installer_dir.join("omakure.exe");
    if !source_exe.exists() {
        return Err("omakure.exe not found next to the installer".into());
    }

    let install_dir = default_install_dir()?;
    fs::create_dir_all(&install_dir)?;
    let target_exe = install_dir.join("omakure.exe");
    fs::copy(&source_exe, &target_exe)?;

    add_to_user_path(&install_dir)?;

    println!("Installed to {}", target_exe.display());
    println!("Open a new terminal and run `omakure`.");
    Ok(())
}

#[cfg(windows)]
fn default_install_dir() -> Result<std::path::PathBuf, Box<dyn Error>> {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        Ok(PathBuf::from(local).join("omakure").join("bin"))
    } else if let Ok(profile) = std::env::var("USERPROFILE") {
        Ok(PathBuf::from(profile)
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

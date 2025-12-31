use std::env;
use std::error::Error;
use std::path::PathBuf;
use crate::workspace::Workspace;

pub struct ConfigOptions {
    pub scripts_dir: PathBuf,
}

pub fn print_config_help() {
    println!(
        "Usage: omakure config\n\n\
Aliases:\n\
  env\n\n\
Notes:\n\
  Prints resolved workspace paths and environment overrides.\n\n\
Environment:\n\
  OMAKURE_SCRIPTS_DIR  Scripts directory override\n\
  OMAKURE_REPO         Default repo for update\n\
  REPO                 Repo override for update\n\
  VERSION              Version override for update\n\
  OVERTURE_SCRIPTS_DIR  Legacy scripts directory override\n\
  OVERTURE_REPO         Legacy repo override\n\
  CLOUD_MGMT_SCRIPTS_DIR  Legacy scripts directory override\n\
  CLOUD_MGMT_REPO         Legacy repo override"
    );
}

pub fn parse_config_args(
    args: &[String],
    scripts_dir: PathBuf,
) -> Result<ConfigOptions, Box<dyn Error>> {
    if !args.is_empty() {
        return Err("config does not accept arguments".into());
    }
    Ok(ConfigOptions { scripts_dir })
}

pub fn run_config(options: ConfigOptions) -> Result<(), Box<dyn Error>> {
    let exe = env::current_exe()?;
    let workspace = Workspace::new(options.scripts_dir.clone());
    println!("Version: {}", env!("CARGO_PKG_VERSION"));
    println!("Binary: {}", exe.display());
    println!("Workspace root: {}", workspace.root().display());
    println!("Omaken dir: {}", workspace.omaken_dir().display());
    println!("History dir: {}", workspace.history_dir().display());
    println!(
        "Workspace config: {}",
        workspace.config_path().display()
    );

    if let Ok(value) = env::var("OMAKURE_SCRIPTS_DIR") {
        println!("OMAKURE_SCRIPTS_DIR: {}", value);
    }
    if let Ok(value) = env::var("OMAKURE_REPO") {
        println!("OMAKURE_REPO: {}", value);
    }
    if let Ok(value) = env::var("REPO") {
        println!("REPO: {}", value);
    }
    if let Ok(value) = env::var("VERSION") {
        println!("VERSION: {}", value);
    }
    if let Ok(value) = env::var("OVERTURE_SCRIPTS_DIR") {
        println!("OVERTURE_SCRIPTS_DIR: {}", value);
    }
    if let Ok(value) = env::var("OVERTURE_REPO") {
        println!("OVERTURE_REPO: {}", value);
    }
    if let Ok(value) = env::var("CLOUD_MGMT_SCRIPTS_DIR") {
        println!("CLOUD_MGMT_SCRIPTS_DIR: {}", value);
    }
    if let Ok(value) = env::var("CLOUD_MGMT_REPO") {
        println!("CLOUD_MGMT_REPO: {}", value);
    }

    Ok(())
}

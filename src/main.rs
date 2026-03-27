mod adapters;
mod app_meta;
mod cli;
mod domain;
mod error;
mod history;
mod lua_widget;
mod ports;
mod runtime;
mod search_index;
mod theme_config;
mod use_cases;
mod util;
mod workspace;

use adapters::script_runner::MultiScriptRunner;
use adapters::tui;
use adapters::workspace_repository::FsWorkspaceRepository;
use clap::Parser;
use cli::args::{Cli, Commands, Shell};
use std::env;
use std::error::Error;
use std::path::PathBuf;
use use_cases::ScriptService;
use workspace::Workspace;

fn scripts_dir_for(name: &str) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(documents) = windows_documents_dir() {
            return documents.join(name);
        }

        if let Ok(user_profile) = env::var("USERPROFILE") {
            return PathBuf::from(user_profile).join("Documents").join(name);
        }
    }

    #[cfg(not(windows))]
    {
        if let Ok(home) = env::var("HOME") {
            return PathBuf::from(home).join("Documents").join(name);
        }
    }

    PathBuf::from("scripts")
}

#[cfg(windows)]
fn windows_documents_dir() -> Option<PathBuf> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let subkeys = [
        "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Shell Folders",
        "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\User Shell Folders",
    ];

    for subkey in subkeys {
        if let Ok(key) = hkcu.open_subkey(subkey) {
            if let Ok(value) = key.get_value::<String, _>("Personal") {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some(PathBuf::from(expand_windows_env_vars(trimmed)));
                }
            }
        }
    }

    None
}

#[cfg(windows)]
fn expand_windows_env_vars(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }

        let mut name = String::new();
        let mut found_end = false;
        while let Some(next) = chars.next() {
            if next == '%' {
                found_end = true;
                break;
            }
            name.push(next);
        }

        if !found_end {
            output.push('%');
            output.push_str(&name);
            break;
        }

        if name.is_empty() {
            output.push('%');
            continue;
        }

        if let Ok(value) = env::var(&name) {
            output.push_str(&value);
        } else {
            output.push('%');
            output.push_str(&name);
            output.push('%');
        }
    }

    output
}

fn default_scripts_dir() -> PathBuf {
    scripts_dir_for("omakure-scripts")
}

fn scripts_dir() -> PathBuf {
    if let Ok(dir) = env::var("OMAKURE_SCRIPTS_DIR") {
        return PathBuf::from(dir);
    }

    if let Ok(dir) = env::var("OVERTURE_SCRIPTS_DIR") {
        return PathBuf::from(dir);
    }

    if let Ok(dir) = env::var("CLOUD_MGMT_SCRIPTS_DIR") {
        return PathBuf::from(dir);
    }

    if cfg!(debug_assertions) {
        let dev_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts");
        if dev_dir.is_dir() {
            return dev_dir;
        }
    }

    let default_dir = default_scripts_dir();
    if default_dir.is_dir() {
        return default_dir;
    }

    for legacy_dir in [
        scripts_dir_for("overture-scripts"),
        scripts_dir_for("cloud-mgmt-scripts"),
    ] {
        if legacy_dir.is_dir() {
            return legacy_dir;
        }
    }

    default_dir
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let scripts_dir = cli.scripts_dir.unwrap_or_else(scripts_dir);

    match cli.command {
        Some(Commands::Update(args)) => cli::update::run(scripts_dir, args)?,
        Some(Commands::Uninstall(args)) => cli::uninstall::run(scripts_dir, args)?,
        Some(Commands::Doctor) => cli::doctor::run(scripts_dir)?,
        Some(Commands::List) => cli::omaken::run_list(scripts_dir)?,
        Some(Commands::Install(args)) => cli::omaken::run_install(scripts_dir, args)?,
        Some(Commands::Scripts) => cli::list::run(scripts_dir)?,
        Some(Commands::Run(args)) => cli::run::run(scripts_dir, args)?,
        Some(Commands::Init(args)) => cli::init::run(scripts_dir, args)?,
        Some(Commands::Config) => cli::config::run(scripts_dir)?,
        Some(Commands::Theme(args)) => cli::theme::run(scripts_dir, args)?,
        Some(Commands::Completion(args)) => generate_completions(args.shell),
        None => run_tui(scripts_dir)?,
    }

    Ok(())
}

fn run_tui(scripts_dir: PathBuf) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir.clone());
    workspace.ensure_layout()?;

    let repo = Box::new(FsWorkspaceRepository::new(scripts_dir));
    let runner = Box::new(MultiScriptRunner::new());
    let service = ScriptService::new(repo, runner);

    let mut terminal = tui::setup_terminal()?;
    let app_result = tui::run_app(&mut terminal, &service, workspace);
    tui::restore_terminal(&mut terminal)?;
    app_result?;

    Ok(())
}

fn generate_completions(shell: Shell) {
    use clap::CommandFactory;
    use clap_complete::{generate, Shell as ClapShell};

    let mut cmd = Cli::command();
    let shell = match shell {
        Shell::Bash => ClapShell::Bash,
        Shell::Zsh => ClapShell::Zsh,
        Shell::Fish => ClapShell::Fish,
        Shell::Pwsh => ClapShell::PowerShell,
    };

    generate(shell, &mut cmd, "omakure", &mut std::io::stdout());
}

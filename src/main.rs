mod adapters;
mod app_meta;
mod auth;
mod cli;
pub mod domain;
mod error;
pub mod node;
pub mod node_identity;
pub mod operations;
mod policy;
mod ports;
pub mod redaction;
mod run_executor;
mod runs;
mod runtime;
mod search_index;
pub mod secrets;
mod use_cases;
mod util;
mod workspace;

use clap::Parser;
use cli::args::{Cli, Commands, Shell};
use std::env;
use std::error::Error;
use std::path::PathBuf;

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

fn main() {
    if let Err(err) = run() {
        // Top-level errors are rendered via Display so users see the
        // configured error messages instead of Rust's `Debug` rendering.
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let json_output = cli.json;

    let Some(command) = cli.command else {
        if json_output {
            cli::json::print_err(
                cli::json::codes::INVALID_ARGUMENT,
                "a subcommand is required; use `omakure --help` for usage",
            );
            std::process::exit(1);
        }

        use clap::CommandFactory;
        let mut command = Cli::command();
        command.print_long_help()?;
        println!();
        return Ok(());
    };

    let global_root = cli.scripts_dir.clone().unwrap_or_else(scripts_dir);

    match command {
        Commands::Update(args) => cli::update::run(global_root, args)?,
        Commands::Uninstall(args) => cli::uninstall::run(global_root, args)?,
        Commands::Doctor => cli::doctor::run(global_root)?,
        Commands::Scripts(args) => cli::list::run(global_root, args, json_output)?,
        Commands::Describe(args) => cli::describe::run(global_root, args, json_output)?,
        Commands::Search(args) => cli::search::run(global_root, args, json_output)?,
        Commands::History(args) => cli::history::run(global_root, args, json_output)?,
        Commands::Queue(args) => cli::queue::run(global_root, args, json_output)?,
        Commands::Battery(args) => cli::battery::run(global_root, args, json_output)?,
        Commands::Env(args) => cli::env::run(global_root, args, json_output)?,
        Commands::Token(args) => cli::token::run(args, json_output)?,
        Commands::Api(args) => cli::api::run(global_root, args)?,
        Commands::Engine(args) => cli::engine::run(global_root, args)?,
        Commands::Trace(args) => cli::trace::run(global_root, args, json_output)?,
        Commands::HelpAi => cli::help_ai::run()?,
        Commands::Run(args) => cli::run::run(global_root, args, json_output)?,
        Commands::Init(args) => cli::init::run_with_format(global_root, args, json_output)?,
        Commands::Config => cli::config::run(global_root, json_output)?,
        Commands::Serve(args) => cli::serve::run(global_root, args, json_output)?,
        Commands::Completion(args) => generate_completions(args.shell),
    }

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

mod adapters;
mod app_meta;
mod cli;
mod domain;
mod error;
mod lua_widget;
mod ports;
mod runs;
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
use error::AppError;
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
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
    let global_root = cli.scripts_dir.clone().unwrap_or_else(scripts_dir);
    let json_output = cli.json;

    match cli.command {
        Some(Commands::Update(args)) => cli::update::run(global_root, args)?,
        Some(Commands::Uninstall(args)) => cli::uninstall::run(global_root, args)?,
        Some(Commands::Doctor) => cli::doctor::run(global_root)?,
        Some(Commands::List) => cli::omaken::run_list(global_root)?,
        Some(Commands::Install(args)) => cli::omaken::run_install(global_root, args)?,
        Some(Commands::Scripts) => cli::list::run(global_root, json_output)?,
        Some(Commands::Describe(args)) => cli::describe::run(global_root, args, json_output)?,
        Some(Commands::Search(args)) => cli::search::run(global_root, args, json_output)?,
        Some(Commands::History(args)) => cli::history::run(global_root, args, json_output)?,
        Some(Commands::HelpAi) => cli::help_ai::run()?,
        Some(Commands::Run(args)) => cli::run::run(global_root, args, json_output)?,
        Some(Commands::Init(args)) => cli::init::run_with_format(global_root, args, json_output)?,
        Some(Commands::Config) => cli::config::run(global_root, json_output)?,
        Some(Commands::Theme(args)) => cli::theme::run(global_root, args)?,
        Some(Commands::Completion(args)) => generate_completions(args.shell),
        None => {
            let scripts_root = resolve_scripts_root(cli.path.as_deref(), &global_root)?;
            let scripts_root_override = cli.path.is_some();
            run_tui(global_root, scripts_root, scripts_root_override)?;
        }
    }

    Ok(())
}

/// Resolve the scripts root the TUI should browse.
///
/// When `positional` is `None`, returns `default_root` unchanged so the
/// existing precedence chain in [`scripts_dir`] continues to drive the
/// global workspace location. When `positional` is `Some`, validates that
/// the path exists, is a directory, and canonicalizes it (following
/// symlinks) so the same physical directory always produces the same key.
fn resolve_scripts_root(
    positional: Option<&Path>,
    default_root: &Path,
) -> Result<PathBuf, AppError> {
    let Some(path) = positional else {
        return Ok(default_root.to_path_buf());
    };

    if !path.exists() {
        return Err(AppError::ScriptsDirNotFound {
            path: path.to_path_buf(),
        });
    }

    if !path.is_dir() {
        // Surface the canonical absolute form when possible so the user
        // sees a deterministic path in the error.
        let display_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        return Err(AppError::ScriptsDirNotADirectory { path: display_path });
    }

    fs::canonicalize(path).map_err(|err| AppError::ScriptsDirResolveFailed {
        path: path.to_path_buf(),
        message: err.to_string(),
    })
}

fn run_tui(
    global_root: PathBuf,
    scripts_root: PathBuf,
    scripts_root_override: bool,
) -> Result<(), Box<dyn Error>> {
    let workspace =
        Workspace::with_scripts_root(global_root, scripts_root.clone(), scripts_root_override);
    workspace.ensure_layout()?;

    let repo = Box::new(FsWorkspaceRepository::new(scripts_root));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_scripts_root_returns_default_when_none() {
        let default = PathBuf::from("/tmp/default-omakure-root");
        let resolved = resolve_scripts_root(None, &default).expect("default should pass through");
        assert_eq!(resolved, default);
    }

    #[test]
    fn resolve_scripts_root_errors_on_nonexistent_path() {
        let default = PathBuf::from("/tmp");
        let missing = PathBuf::from("/tmp/__omakure_definitely_missing_path__");
        let err = resolve_scripts_root(Some(&missing), &default)
            .expect_err("missing path must fail");
        assert!(matches!(err, AppError::ScriptsDirNotFound { .. }));
        let msg = format!("{}", err);
        assert!(
            msg.contains("scripts directory not found"),
            "message was: {msg}"
        );
    }

    #[test]
    fn resolve_scripts_root_errors_when_path_is_a_file() {
        let tmp = std::env::temp_dir().join("__omakure_resolve_root_file_test__");
        let _ = fs::remove_file(&tmp);
        fs::write(&tmp, "not a directory").expect("create temp file");
        let default = std::env::temp_dir();
        let err = resolve_scripts_root(Some(&tmp), &default)
            .expect_err("file path must fail");
        assert!(matches!(err, AppError::ScriptsDirNotADirectory { .. }));
        let msg = format!("{}", err);
        assert!(msg.contains("expected a directory"), "message was: {msg}");
        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn resolve_scripts_root_canonicalizes_relative_and_absolute_to_same_path() {
        let tmp = std::env::temp_dir().join("__omakure_resolve_root_canon_test__");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).expect("create temp dir");

        // Sanity: from /tmp the relative form `__omakure_resolve_root_canon_test__`
        // and the absolute form should canonicalize to the same path.
        let abs = resolve_scripts_root(Some(&tmp), &PathBuf::from("/"))
            .expect("absolute path resolves");

        let prev = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(std::env::temp_dir()).expect("chdir tmp");
        let rel = PathBuf::from("__omakure_resolve_root_canon_test__");
        let rel_resolved = resolve_scripts_root(Some(&rel), &PathBuf::from("/"))
            .expect("relative path resolves");
        std::env::set_current_dir(&prev).expect("restore cwd");

        assert_eq!(abs, rel_resolved);
        let _ = fs::remove_dir_all(&tmp);
    }
}

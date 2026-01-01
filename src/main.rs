mod adapters;
mod completion;
mod config;
mod domain;
mod doctor;
mod history;
mod init;
mod list;
mod lua_widget;
mod omaken;
mod run;
mod uninstall;
mod ports;
mod update;
mod use_cases;
mod runtime;
mod workspace;
mod search_index;

use adapters::script_runner::MultiScriptRunner;
use adapters::workspace_repository::FsWorkspaceRepository;
use adapters::tui;
use std::env;
use std::error::Error;
use std::path::PathBuf;
use use_cases::ScriptService;
use workspace::Workspace;

fn scripts_dir_for(name: &str) -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(user_profile) = env::var("USERPROFILE") {
            return PathBuf::from(user_profile)
                .join("Documents")
                .join(name);
        }
    }

    #[cfg(not(windows))]
    {
        if let Ok(home) = env::var("HOME") {
            return PathBuf::from(home)
                .join("Documents")
                .join(name);
        }
    }

    PathBuf::from("scripts")
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

fn print_help() {
    println!(
        "Usage: omakure [command]\n\n\
Commands:\n\
  update    Update omakure from GitHub Releases\n\
  uninstall Remove the omakure binary\n\
  doctor    Check runtime dependencies and workspace\n\
  check     Alias for doctor\n\
  list      List Omaken flavors\n\
  install   Install an Omaken flavor\n\
  scripts   List available scripts\n\
  run       Run a script without the TUI\n\
  init      Create a new script template\n\
  config    Show resolved paths and env\n\
  env       Alias for config\n\
  completion Generate shell completion\n\
\n\
Options:\n\
  -h, --help     Show this help\n\
  -V, --version  Show version"
    );
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    if let Some(command) = args.next() {
        match command.as_str() {
            "update" => {
                let update_args: Vec<String> = args.collect();
                if update_args
                    .iter()
                    .any(|arg| arg == "-h" || arg == "--help")
                {
                    update::print_update_help();
                    return Ok(());
                }
                let options = update::parse_update_args(&update_args, scripts_dir())?;
                update::run_update(options)?;
                return Ok(());
            }
            "uninstall" => {
                let uninstall_args: Vec<String> = args.collect();
                if uninstall_args
                    .iter()
                    .any(|arg| arg == "-h" || arg == "--help")
                {
                    uninstall::print_uninstall_help();
                    return Ok(());
                }
                let options = uninstall::parse_uninstall_args(&uninstall_args, scripts_dir())?;
                uninstall::run_uninstall(options)?;
                return Ok(());
            }
            "doctor" | "check" => {
                let doctor_args: Vec<String> = args.collect();
                if doctor_args
                    .iter()
                    .any(|arg| arg == "-h" || arg == "--help")
                {
                    doctor::print_doctor_help();
                    return Ok(());
                }
                let options = doctor::parse_doctor_args(&doctor_args, scripts_dir())?;
                doctor::run_doctor(options)?;
                return Ok(());
            }
            "list" => {
                let list_args: Vec<String> = args.collect();
                if list_args
                    .iter()
                    .any(|arg| arg == "-h" || arg == "--help")
                {
                    omaken::print_list_help();
                    return Ok(());
                }
                let options = omaken::parse_list_args(&list_args, scripts_dir())?;
                omaken::run_list(options)?;
                return Ok(());
            }
            "install" => {
                let install_args: Vec<String> = args.collect();
                if install_args
                    .iter()
                    .any(|arg| arg == "-h" || arg == "--help")
                {
                    omaken::print_install_help();
                    return Ok(());
                }
                let options = omaken::parse_install_args(&install_args, scripts_dir())?;
                omaken::run_install(options)?;
                return Ok(());
            }
            "scripts" => {
                let list_args: Vec<String> = args.collect();
                if list_args
                    .iter()
                    .any(|arg| arg == "-h" || arg == "--help")
                {
                    list::print_list_help();
                    return Ok(());
                }
                let options = list::parse_list_args(&list_args, scripts_dir())?;
                list::run_list(options)?;
                return Ok(());
            }
            "run" => {
                let run_args: Vec<String> = args.collect();
                if run::wants_help(&run_args) {
                    run::print_run_help();
                    return Ok(());
                }
                let options = run::parse_run_args(&run_args, scripts_dir())?;
                run::run_script(options)?;
                return Ok(());
            }
            "init" => {
                let init_args: Vec<String> = args.collect();
                if init_args
                    .iter()
                    .any(|arg| arg == "-h" || arg == "--help")
                {
                    init::print_init_help();
                    return Ok(());
                }
                let options = init::parse_init_args(&init_args, scripts_dir())?;
                init::run_init(options)?;
                return Ok(());
            }
            "config" | "env" => {
                let config_args: Vec<String> = args.collect();
                if config_args
                    .iter()
                    .any(|arg| arg == "-h" || arg == "--help")
                {
                    config::print_config_help();
                    return Ok(());
                }
                let options = config::parse_config_args(&config_args, scripts_dir())?;
                config::run_config(options)?;
                return Ok(());
            }
            "completion" => {
                let completion_args: Vec<String> = args.collect();
                if completion_args
                    .iter()
                    .any(|arg| arg == "-h" || arg == "--help")
                {
                    completion::print_completion_help();
                    return Ok(());
                }
                let options = completion::parse_completion_args(&completion_args)?;
                completion::run_completion(options)?;
                return Ok(());
            }
            "help" | "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            "version" | "-V" | "--version" => {
                println!("omakure {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => {}
        }
    }

    let scripts_dir = scripts_dir();
    let workspace = Workspace::new(scripts_dir.clone());
    workspace.ensure_layout()?;

    let repo = Box::new(FsWorkspaceRepository::new(scripts_dir.clone()));
    let runner = Box::new(MultiScriptRunner::new());
    let service = ScriptService::new(repo, runner);

    let mut terminal = tui::setup_terminal()?;
    let app_result = tui::run_app(&mut terminal, &service, workspace);
    tui::restore_terminal(&mut terminal)?;
    app_result?;

    Ok(())
}

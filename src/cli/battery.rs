use crate::cli::args::{BatteryArgs, BatteryCommand};
use crate::cli::json;
use crate::operations::battery::{
    add_battery, inspect_battery, install_battery_script, list_batteries, list_battery_scripts,
    remove_battery, sync_battery, AddBatteryRequest, InspectBatteryRequest,
    InstallBatteryScriptRequest, RemoveBatteryRequest, SyncBatteryRequest,
};
use crate::operations::OperationError;
use crate::workspace::Workspace;
use std::error::Error;
use std::path::PathBuf;

pub fn run(
    scripts_dir: PathBuf,
    args: BatteryArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;

    match args.command {
        BatteryCommand::List => render(
            json_output,
            || list_batteries(&workspace),
            |items| {
                if items.is_empty() {
                    println!("No Batteries registered");
                } else {
                    for battery in items {
                        println!("{} {}", battery.name, battery.requested_ref);
                    }
                }
            },
        ),
        BatteryCommand::Add(args) => render(
            json_output,
            || {
                add_battery(
                    &workspace,
                    AddBatteryRequest {
                        name: args.name,
                        git_url: args.git_url,
                        requested_ref: args.requested_ref,
                    },
                )
            },
            |summary| println!("Registered Battery {}", summary.name),
        ),
        BatteryCommand::Sync(args) => render(
            json_output,
            || sync_battery(&workspace, SyncBatteryRequest { name: args.name }),
            |summary| {
                println!(
                    "Synced Battery {} at {}",
                    summary.name,
                    summary.resolved_commit.as_deref().unwrap_or("unknown")
                );
            },
        ),
        BatteryCommand::Inspect(args) => render(
            json_output,
            || inspect_battery(&workspace, InspectBatteryRequest { name: args.name }),
            |response| {
                println!("Battery {}", response.summary.name);
                println!("Scripts: {}", response.manifest.scripts.len());
            },
        ),
        BatteryCommand::Scripts(args) => render(
            json_output,
            || list_battery_scripts(&workspace, InspectBatteryRequest { name: args.name }),
            |scripts| {
                if scripts.is_empty() {
                    println!("No scripts found");
                } else {
                    for script in scripts {
                        println!("{} {}", script.id, script.path.display());
                    }
                }
            },
        ),
        BatteryCommand::Install(args) => render(
            json_output,
            || {
                install_battery_script(
                    &workspace,
                    InstallBatteryScriptRequest {
                        battery_name: args.name,
                        script_id: args.script_id,
                        force: args.force,
                    },
                )
            },
            |response| println!("Installed {}", response.installed_path.display()),
        ),
        BatteryCommand::Remove(args) => render(
            json_output,
            || {
                remove_battery(
                    &workspace,
                    RemoveBatteryRequest {
                        name: args.name,
                        remove_cache: args.remove_cache,
                    },
                )
            },
            |response| println!("Removed Battery {}", response.name),
        ),
    }
}

fn render<T, F, H>(json_output: bool, operation: F, human: H) -> Result<(), Box<dyn Error>>
where
    T: serde::Serialize,
    F: FnOnce() -> Result<T, OperationError>,
    H: FnOnce(T),
{
    match operation() {
        Ok(data) => {
            if json_output {
                json::print_ok(data);
            } else {
                human(data);
            }
            Ok(())
        }
        Err(err) => {
            if json_output {
                json::print_err(err.code.as_str(), err.message.clone());
                std::process::exit(1);
            }
            Err(Box::new(err))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{BatteryArgs, BatteryCommand};

    #[test]
    fn list_command_runs_against_empty_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        run(
            tmp.path().to_path_buf(),
            BatteryArgs {
                command: BatteryCommand::List,
            },
            false,
        )
        .unwrap();
    }

    #[test]
    fn render_non_json_errors_return_err_for_top_level_stderr() {
        let err = render::<(), _, _>(
            false,
            || {
                Err(OperationError::new(
                    crate::operations::OperationErrorCode::NotFound,
                    "missing",
                ))
            },
            |_| {},
        )
        .unwrap_err();

        assert!(err.to_string().contains("not_found"));
    }
}

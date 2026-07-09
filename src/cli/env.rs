use crate::cli::args::{EnvArgs, EnvCommand, EnvCreateArgs, EnvRemoveArgs, EnvSetArgs};
use crate::cli::json::{self, codes};
use crate::operations::envs::{self, EnvParam};
use crate::operations::{OperationError, OperationErrorCode};
use crate::workspace::Workspace;
use serde::Serialize;
use std::error::Error;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
struct EnvMutation<'a> {
    name: &'a str,
}

#[derive(Debug, Serialize)]
struct EnvDeactivateMutation {
    active: Option<String>,
}

pub fn run(scripts_dir: PathBuf, args: EnvArgs, json_output: bool) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;

    match args.command {
        EnvCommand::List => list(&workspace, json_output),
        EnvCommand::Create(opts) => create(&workspace, opts, json_output),
        EnvCommand::Show(opts) => show(&workspace, &opts.name, json_output),
        EnvCommand::Set(opts) => set(&workspace, opts, json_output),
        EnvCommand::Remove(opts) => remove(&workspace, opts, json_output),
        EnvCommand::Replace(opts) => replace(&workspace, opts, json_output),
        EnvCommand::Activate(opts) => activate(&workspace, &opts.name, json_output),
        EnvCommand::Deactivate => deactivate(&workspace, json_output),
        EnvCommand::Delete(opts) => delete(&workspace, &opts.name, json_output),
    }
}

fn list(workspace: &Workspace, json_output: bool) -> Result<(), Box<dyn Error>> {
    let envs = envs::list_envs(workspace).map_err(Box::<dyn Error>::from)?;
    if json_output {
        json::print_ok(envs);
    } else if envs.is_empty() {
        println!("No environments found");
    } else {
        for env in envs {
            let marker = if env.active { "*" } else { " " };
            println!("{} {} ({})", marker, env.name, env.file);
        }
    }
    Ok(())
}

fn create(
    workspace: &Workspace,
    opts: EnvCreateArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let params = match parse_params(&opts.params) {
        Ok(params) => params,
        Err(err) => return emit_operation_error(json_output, err),
    };
    match envs::create_env(workspace, &opts.name, &params) {
        Ok(()) => emit_mutation(json_output, &opts.name, "created"),
        Err(err) => emit_operation_error(json_output, err),
    }
}

fn show(workspace: &Workspace, name: &str, json_output: bool) -> Result<(), Box<dyn Error>> {
    match envs::show_env(workspace, name) {
        Ok(entries) => {
            if json_output {
                json::print_ok(entries);
            } else {
                for entry in entries {
                    println!("{}={}", entry.key, entry.value);
                }
            }
            Ok(())
        }
        Err(err) => emit_operation_error(json_output, err),
    }
}

fn set(workspace: &Workspace, opts: EnvSetArgs, json_output: bool) -> Result<(), Box<dyn Error>> {
    let param = match parse_param(&opts.param) {
        Ok(param) => param,
        Err(err) => return emit_operation_error(json_output, err),
    };
    match envs::set_param(workspace, &opts.name, &param.key, &param.value) {
        Ok(()) => emit_mutation(json_output, &opts.name, "set"),
        Err(err) => emit_operation_error(json_output, err),
    }
}

fn remove(
    workspace: &Workspace,
    opts: EnvRemoveArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    match envs::remove_param(workspace, &opts.name, &opts.key) {
        Ok(()) => emit_mutation(json_output, &opts.name, "removed"),
        Err(err) => emit_operation_error(json_output, err),
    }
}

fn replace(
    workspace: &Workspace,
    opts: EnvCreateArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let params = match parse_params(&opts.params) {
        Ok(params) => params,
        Err(err) => return emit_operation_error(json_output, err),
    };
    match envs::replace_env(workspace, &opts.name, &params) {
        Ok(()) => emit_mutation(json_output, &opts.name, "replaced"),
        Err(err) => emit_operation_error(json_output, err),
    }
}

fn activate(workspace: &Workspace, name: &str, json_output: bool) -> Result<(), Box<dyn Error>> {
    match envs::activate_env(workspace, name) {
        Ok(()) => emit_mutation(json_output, name, "activated"),
        Err(err) => emit_operation_error(json_output, err),
    }
}

fn deactivate(workspace: &Workspace, json_output: bool) -> Result<(), Box<dyn Error>> {
    match envs::deactivate_env(workspace) {
        Ok(()) => {
            if json_output {
                json::print_ok(EnvDeactivateMutation { active: None });
            } else {
                println!("deactivated");
            }
            Ok(())
        }
        Err(err) => emit_operation_error(json_output, err),
    }
}

fn delete(workspace: &Workspace, name: &str, json_output: bool) -> Result<(), Box<dyn Error>> {
    match envs::delete_env(workspace, name) {
        Ok(()) => emit_mutation(json_output, name, "deleted"),
        Err(err) => emit_operation_error(json_output, err),
    }
}

fn emit_mutation(json_output: bool, name: &str, verb: &str) -> Result<(), Box<dyn Error>> {
    if json_output {
        json::print_ok(EnvMutation { name });
    } else {
        println!("{}: {}", verb, name);
    }
    Ok(())
}

fn parse_params(values: &[String]) -> Result<Vec<EnvParam>, OperationError> {
    values.iter().map(|value| parse_param(value)).collect()
}

fn parse_param(value: &str) -> Result<EnvParam, OperationError> {
    let Some((key, param_value)) = value.split_once('=') else {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "expected KEY=VALUE",
        ));
    };
    if key.trim().is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "environment key cannot be empty",
        ));
    }
    Ok(EnvParam {
        key: key.to_string(),
        value: param_value.to_string(),
    })
}

fn emit_operation_error(json_output: bool, err: OperationError) -> Result<(), Box<dyn Error>> {
    let code = match err.code {
        OperationErrorCode::InvalidInput | OperationErrorCode::UnsafePath => {
            codes::INVALID_ARGUMENT
        }
        OperationErrorCode::NotFound => codes::NOT_FOUND,
        _ => codes::INTERNAL,
    };
    if json_output {
        json::print_err(code, err.message);
        std::process::exit(1);
    }
    Err(err.message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn env_cli_create_set_remove_activate_show_round_trip() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        run(
            root.clone(),
            EnvArgs {
                command: EnvCommand::Create(EnvCreateArgs {
                    name: "prod".into(),
                    params: vec!["HOST=prod.example.com".into(), "API_KEY=supersecret".into()],
                }),
            },
            true,
        )
        .unwrap();
        run(
            root.clone(),
            EnvArgs {
                command: EnvCommand::Set(EnvSetArgs {
                    name: "prod".into(),
                    param: "PORT=443".into(),
                }),
            },
            true,
        )
        .unwrap();
        run(
            root.clone(),
            EnvArgs {
                command: EnvCommand::Remove(EnvRemoveArgs {
                    name: "prod".into(),
                    key: "API_KEY".into(),
                }),
            },
            true,
        )
        .unwrap();
        run(
            root.clone(),
            EnvArgs {
                command: EnvCommand::Activate(crate::cli::args::EnvNameArgs {
                    name: "prod".into(),
                }),
            },
            true,
        )
        .unwrap();
        run(
            root.clone(),
            EnvArgs {
                command: EnvCommand::Show(crate::cli::args::EnvNameArgs {
                    name: "prod".into(),
                }),
            },
            true,
        )
        .unwrap();

        let env_dir = root.join(".omakure/envs");
        assert_eq!(
            fs::read_to_string(env_dir.join("prod.conf")).unwrap(),
            "HOST=prod.example.com\nPORT=443\n"
        );
        assert_eq!(
            fs::read_to_string(env_dir.join("active")).unwrap(),
            "prod.conf\n"
        );
    }

    #[test]
    fn parse_param_rejects_missing_equals() {
        let err = parse_param("TOKEN").unwrap_err();
        assert_eq!(err.code, OperationErrorCode::InvalidInput);
    }
}

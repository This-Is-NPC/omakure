use super::*;
use clap::{CommandFactory, Parser};

mod api;
mod node;

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("omakure").chain(args.iter().copied()))
}

#[test]
fn test_parse_no_args_leaves_command_empty_for_help() {
    let cli = parse(&[]).unwrap();
    assert!(cli.command.is_none());
}

#[test]
fn test_parse_run_subcommand() {
    let cli = parse(&["run", "deploy.sh"]).unwrap();
    match cli.command.unwrap() {
        Commands::Run(args) => {
            assert_eq!(args.script, "deploy.sh");
            assert_eq!(args.actor, "human");
            assert!(!args.no_prompt);
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_parse_run_with_flags() {
    let cli = parse(&["run", "deploy.sh", "--json", "--no-prompt", "--actor", "ai"]).unwrap();
    assert!(cli.json);
    match cli.command.unwrap() {
        Commands::Run(args) => {
            assert_eq!(args.actor, "ai");
            assert!(args.no_prompt);
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_parse_run_secret_input() {
    let cli = parse(&["run", "deploy.sh", "--secret", "TOKEN=direct"]);
    let cli = cli.unwrap();
    match cli.command.unwrap() {
        Commands::Run(args) => assert_eq!(args.secrets, vec!["TOKEN=direct"]),
        _ => panic!("expected Run command"),
    }
}

#[test]
fn test_parse_env_namespace() {
    let cli = parse(&["env", "set", "prod", "API_KEY=secret"]);
    let cli = cli.unwrap();
    match cli.command.unwrap() {
        Commands::Env(args) => match args.command {
            EnvCommand::Set(set) => {
                assert_eq!(set.name, "prod");
                assert_eq!(set.param, "API_KEY=secret");
            }
            _ => panic!("expected env set"),
        },
        _ => panic!("expected Env command"),
    }
}

#[test]
fn test_parse_global_json_flag() {
    let cli = parse(&["--json", "scripts"]).unwrap();
    assert!(cli.json);
}

#[test]
fn test_unknown_top_level_command_is_rejected() {
    assert!(parse(&["list"]).is_err());
}

#[test]
fn test_legacy_install_url_is_rejected() {
    let result = parse(&["install", "https://example.com/scripts.git"]);
    assert!(result.is_err());
}

#[test]
fn test_parse_queue_add() {
    let cli = parse(&["queue", "add", "deploy.sh", "--priority", "10"]).unwrap();
    match cli.command.unwrap() {
        Commands::Queue(q) => match q.command {
            QueueCommand::Add(args) => {
                assert_eq!(args.script, "deploy.sh");
                assert_eq!(args.priority, 10);
            }
            _ => panic!("expected Add"),
        },
        _ => panic!("expected Queue"),
    }
}

#[test]
fn test_parse_battery_add() {
    let cli = parse(&[
        "battery",
        "add",
        "https://example.invalid/azure.git",
        "--name",
        "azure",
        "--ref",
        "stable",
    ])
    .unwrap();
    match cli.command.unwrap() {
        Commands::Battery(args) => match args.command {
            BatteryCommand::Add(add) => {
                assert_eq!(add.git_url, "https://example.invalid/azure.git");
                assert_eq!(add.name, "azure");
                assert_eq!(add.requested_ref, "stable");
            }
            _ => panic!("expected Battery Add"),
        },
        _ => panic!("expected Battery"),
    }
}

#[test]
fn test_parse_battery_install_force() {
    let cli = parse(&["battery", "install", "azure", "azure.list", "--force"]).unwrap();
    match cli.command.unwrap() {
        Commands::Battery(args) => match args.command {
            BatteryCommand::Install(install) => {
                assert_eq!(install.name, "azure");
                assert_eq!(install.script_id, "azure.list");
                assert!(install.force);
            }
            _ => panic!("expected Battery Install"),
        },
        _ => panic!("expected Battery"),
    }
}

#[test]
fn test_parse_battery_remove_cache() {
    let cli = parse(&["battery", "remove", "azure", "--remove-cache"]).unwrap();
    match cli.command.unwrap() {
        Commands::Battery(args) => match args.command {
            BatteryCommand::Remove(remove) => {
                assert_eq!(remove.name, "azure");
                assert!(remove.remove_cache);
            }
            _ => panic!("expected Battery Remove"),
        },
        _ => panic!("expected Battery"),
    }
}

#[test]
fn test_parse_battery_workflow_start() {
    let cli = parse(&["--json", "battery", "workflow", "start", "local", "deploy"]).unwrap();
    assert!(cli.json);
    match cli.command.unwrap() {
        Commands::Battery(args) => match args.command {
            BatteryCommand::Workflow(workflow) => match workflow.command {
                BatteryWorkflowCommand::Start(start) => {
                    assert_eq!(start.battery_name, "local");
                    assert_eq!(start.workflow_name, "deploy");
                }
                _ => panic!("expected Battery workflow start"),
            },
            _ => panic!("expected Battery workflow"),
        },
        _ => panic!("expected Battery"),
    }
}

#[test]
fn test_parse_battery_workflow_status() {
    let cli = parse(&["battery", "workflow", "status", "workflow-123"]).unwrap();
    match cli.command.unwrap() {
        Commands::Battery(args) => match args.command {
            BatteryCommand::Workflow(workflow) => match workflow.command {
                BatteryWorkflowCommand::Status(status) => {
                    assert_eq!(status.workflow_run_id, "workflow-123");
                }
                _ => panic!("expected Battery workflow status"),
            },
            _ => panic!("expected Battery workflow"),
        },
        _ => panic!("expected Battery"),
    }
}

#[test]
fn test_parse_queue_worker() {
    let cli = parse(&["queue", "worker", "--concurrency", "4"]).unwrap();
    match cli.command.unwrap() {
        Commands::Queue(q) => match q.command {
            QueueCommand::Worker(args) => assert_eq!(args.concurrency, 4),
            _ => panic!("expected Worker"),
        },
        _ => panic!("expected Queue"),
    }
}

#[test]
fn test_parse_history_list_with_state() {
    let cli = parse(&["history", "list", "--state", "completed", "--since", "1h"]).unwrap();
    match cli.command.unwrap() {
        Commands::History(h) => match h.command {
            HistoryCommand::List(args) => {
                assert_eq!(args.state, vec!["completed"]);
                assert_eq!(args.since, Some("1h".to_string()));
            }
            _ => panic!("expected List"),
        },
        _ => panic!("expected History"),
    }
}

#[test]
fn test_parse_history_show() {
    let cli = parse(&["history", "show", "abc123"]).unwrap();
    match cli.command.unwrap() {
        Commands::History(h) => match h.command {
            HistoryCommand::Show(args) => assert_eq!(args.run_id, "abc123"),
            _ => panic!("expected Show"),
        },
        _ => panic!("expected History"),
    }
}

#[test]
fn test_conflicting_scripts_dir_and_path() {
    let result = parse(&["--scripts-dir", "/a", "/b"]);
    assert!(result.is_err());
}

#[test]
fn test_parse_describe() {
    let cli = parse(&["describe", "deploy.sh"]).unwrap();
    match cli.command.unwrap() {
        Commands::Describe(args) => assert_eq!(args.script, "deploy.sh"),
        _ => panic!("expected Describe"),
    }
}

#[test]
fn test_parse_search() {
    let cli = parse(&["search", "deploy", "--tag", "infra"]).unwrap();
    match cli.command.unwrap() {
        Commands::Search(args) => {
            assert_eq!(args.query, "deploy");
            assert_eq!(args.tag, vec!["infra"]);
        }
        _ => panic!("expected Search"),
    }
}

#[test]
fn test_parse_init_with_schema() {
    let cli = parse(&["init", "new.sh", "--schema-json", "{}"]).unwrap();
    match cli.command.unwrap() {
        Commands::Init(args) => {
            assert_eq!(args.script, "new.sh");
            assert_eq!(args.schema_json, Some("{}".to_string()));
        }
        _ => panic!("expected Init"),
    }
}

#[test]
fn test_parse_trace() {
    let cli = parse(&["trace", "hello", "--level", "warn", "--data", "{\"k\":1}"]).unwrap();
    match cli.command.unwrap() {
        Commands::Trace(args) => {
            assert_eq!(args.message, "hello");
            assert_eq!(args.level, "warn");
            assert_eq!(args.data, Some("{\"k\":1}".to_string()));
        }
        _ => panic!("expected Trace"),
    }
}

#[test]
fn test_history_success_failure_conflict() {
    let result = parse(&["history", "list", "--success", "--failure"]);
    assert!(result.is_err());
}

#[test]
fn test_history_state_and_state_set_conflict() {
    let result = parse(&[
        "history",
        "list",
        "--state",
        "completed",
        "--state-set",
        "all",
    ]);
    assert!(result.is_err());
}

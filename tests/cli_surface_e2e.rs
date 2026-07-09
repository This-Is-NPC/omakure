mod support;

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Coverage {
    Covered(&'static str),
    Excluded(&'static str),
}

#[derive(Clone, Copy, Debug)]
struct CommandCoverage {
    command: &'static str,
    coverage: Coverage,
}

const TOP_LEVEL_COVERAGE: &[CommandCoverage] = &[
    CommandCoverage {
        command: "api",
        coverage: Coverage::Covered("tests/http_api_e2e.rs"),
    },
    CommandCoverage {
        command: "battery",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "completion",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "config",
        coverage: Coverage::Covered(
            "tests/cli_positional_path.rs + tests/cli_surface_e2e.rs --json",
        ),
    },
    CommandCoverage {
        command: "describe",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "doctor",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "env",
        coverage: Coverage::Covered("tests/secret_cli_e2e.rs + tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "help-ai",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "history",
        coverage: Coverage::Covered("tests/secret_cli_e2e.rs + tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "init",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "queue",
        coverage: Coverage::Covered("tests/secret_cli_e2e.rs + tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "run",
        coverage: Coverage::Covered("tests/secret_cli_e2e.rs"),
    },
    CommandCoverage {
        command: "scripts",
        coverage: Coverage::Covered("tests/cli_positional_path.rs"),
    },
    CommandCoverage {
        command: "search",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "serve",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs --once"),
    },
    CommandCoverage {
        command: "theme",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs list/path/preview"),
    },
    CommandCoverage {
        command: "trace",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs via real run"),
    },
    CommandCoverage {
        command: "uninstall",
        coverage: Coverage::Excluded("destructive; help surface only"),
    },
    CommandCoverage {
        command: "update",
        coverage: Coverage::Excluded("network/self-update; help surface only"),
    },
];

const NESTED_COVERAGE: &[CommandCoverage] = &[
    CommandCoverage {
        command: "battery add",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "battery inspect",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "battery install",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "battery list",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "battery remove",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "battery scripts",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "battery sync",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "env activate",
        coverage: Coverage::Covered("tests/secret_cli_e2e.rs"),
    },
    CommandCoverage {
        command: "env create",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs + tests/secret_cli_e2e.rs"),
    },
    CommandCoverage {
        command: "env deactivate",
        coverage: Coverage::Covered("tests/secret_cli_e2e.rs"),
    },
    CommandCoverage {
        command: "env delete",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "env list",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "env remove",
        coverage: Coverage::Covered("tests/secret_cli_e2e.rs"),
    },
    CommandCoverage {
        command: "env replace",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "env set",
        coverage: Coverage::Covered("tests/secret_cli_e2e.rs"),
    },
    CommandCoverage {
        command: "env show",
        coverage: Coverage::Covered("tests/secret_cli_e2e.rs"),
    },
    CommandCoverage {
        command: "history list",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "history show",
        coverage: Coverage::Covered("tests/secret_cli_e2e.rs"),
    },
    CommandCoverage {
        command: "history stats",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "history tail",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "history traces",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "queue add",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs + tests/secret_cli_e2e.rs"),
    },
    CommandCoverage {
        command: "queue cancel",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "queue dead-letter",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "queue stats",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "queue worker",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs --once + tests/secret_cli_e2e.rs"),
    },
    CommandCoverage {
        command: "serve --detach/--stop/--install/--uninstall",
        coverage: Coverage::Excluded(
            "daemon/systemd host mutation; --once and --status cover safe boundaries",
        ),
    },
    CommandCoverage {
        command: "serve --status",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs host-safe status probe"),
    },
    CommandCoverage {
        command: "theme list",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "theme path",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "theme preview",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "theme set",
        coverage: Coverage::Excluded(
            "mutates global theme config; list/path/preview cover safe theme surface",
        ),
    },
];

#[test]
fn command_surface_inventory_maps_all_current_commands() {
    // Derive top-level + nested subcommands from clap `--help` so inventory
    // drift against `src/cli/args.rs` fails this suite.
    let clap_top = clap_top_level_commands();
    let mut inventory_top: Vec<&str> = TOP_LEVEL_COVERAGE.iter().map(|e| e.command).collect();
    inventory_top.sort_unstable();
    assert_eq!(
        inventory_top, clap_top,
        "TOP_LEVEL_COVERAGE must match `omakure --help` Commands list"
    );

    let expected_nested = [
        "battery add",
        "battery inspect",
        "battery install",
        "battery list",
        "battery remove",
        "battery scripts",
        "battery sync",
        "env activate",
        "env create",
        "env deactivate",
        "env delete",
        "env list",
        "env remove",
        "env replace",
        "env set",
        "env show",
        "history list",
        "history show",
        "history stats",
        "history tail",
        "history traces",
        "queue add",
        "queue cancel",
        "queue dead-letter",
        "queue stats",
        "queue worker",
        "theme list",
        "theme path",
        "theme preview",
        "theme set",
    ];
    let clap_nested = clap_nested_commands(&["battery", "env", "history", "queue", "theme"]);
    assert_eq!(
        clap_nested, expected_nested,
        "nested clap subcommands drifted from expected set"
    );

    let inventory_nested: Vec<&str> = NESTED_COVERAGE
        .iter()
        .filter(|e| !e.command.starts_with("serve "))
        .map(|e| e.command)
        .collect();
    assert_eq!(
        inventory_nested, expected_nested,
        "NESTED_COVERAGE (excluding serve flag rows) must cover every clap nested subcommand"
    );

    assert!(TOP_LEVEL_COVERAGE.iter().all(|entry| match entry.coverage {
        Coverage::Covered(path) | Coverage::Excluded(path) => !path.trim().is_empty(),
    }));
    assert!(NESTED_COVERAGE.iter().all(|entry| match entry.coverage {
        Coverage::Covered(path) | Coverage::Excluded(path) => !path.trim().is_empty(),
    }));
}

fn clap_top_level_commands() -> Vec<String> {
    let output = support::omakure_command()
        .arg("--help")
        .output()
        .expect("omakure --help");
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    let mut names = Vec::new();
    let mut in_commands = false;
    for line in text.lines() {
        if line.starts_with("Commands:") {
            in_commands = true;
            continue;
        }
        if in_commands {
            if line.starts_with("Arguments:") || line.starts_with("Options:") || line.is_empty() {
                if !names.is_empty()
                    && (line.starts_with("Arguments:") || line.starts_with("Options:"))
                {
                    break;
                }
                continue;
            }
            let name = line.split_whitespace().next().unwrap_or("");
            if name == "help" {
                continue;
            }
            if !name.is_empty() {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    names
}

fn clap_nested_commands(parents: &[&str]) -> Vec<String> {
    let mut names = Vec::new();
    for parent in parents {
        let output = support::omakure_command()
            .args([parent, "--help"])
            .output()
            .unwrap_or_else(|_| panic!("omakure {parent} --help"));
        assert!(output.status.success(), "{parent} --help failed");
        let text = String::from_utf8_lossy(&output.stdout);
        let mut in_commands = false;
        for line in text.lines() {
            if line.starts_with("Commands:") {
                in_commands = true;
                continue;
            }
            if in_commands {
                if line.starts_with("Arguments:") || line.starts_with("Options:") || line.is_empty()
                {
                    if !line.is_empty()
                        && (line.starts_with("Arguments:") || line.starts_with("Options:"))
                    {
                        break;
                    }
                    continue;
                }
                let name = line.split_whitespace().next().unwrap_or("");
                if name == "help" || name.is_empty() {
                    continue;
                }
                names.push(format!("{parent} {name}"));
            }
        }
    }
    names.sort();
    names
}

#[test]
fn local_info_commands_cover_init_describe_search_doctor_help_completion_theme_and_serve() {
    let workspace = support::TestWorkspace::new("cli_surface_info");

    let init = omakure(workspace.path(), &["--json", "init", "tools/info.sh"]);
    assert_success(&init);
    assert_eq!(json(&init)["data"]["relative_path"], "tools/info.sh");

    let describe = omakure(workspace.path(), &["--json", "describe", "tools/info.sh"]);
    assert_success(&describe);
    assert_eq!(json(&describe)["data"]["relative_path"], "tools/info.sh");

    let search = omakure(workspace.path(), &["--json", "search", "info"]);
    assert_success(&search);
    assert!(json(&search)["data"]
        .as_array()
        .expect("search data")
        .iter()
        .any(|entry| { entry["relative_path"] == "tools/info.sh" }));

    let doctor = omakure(workspace.path(), &["--json", "doctor"]);
    assert_success(&doctor);
    assert!(String::from_utf8_lossy(&doctor.stdout).contains("All checks passed"));

    let help_ai = omakure(workspace.path(), &["help-ai"]);
    assert_success(&help_ai);
    assert!(json(&help_ai)["data"]["verbs"]
        .as_array()
        .expect("verbs")
        .iter()
        .any(|verb| { verb["name"] == "trace" }));

    let completion = omakure_large_output(workspace.path(), &["completion", "bash"]);
    assert_success(&completion);
    assert!(String::from_utf8_lossy(&completion.stdout).contains("omakure"));

    let theme_list = omakure(workspace.path(), &["theme", "list"]);
    assert_success(&theme_list);
    assert!(String::from_utf8_lossy(&theme_list.stdout).contains("Built-in themes"));

    let theme_path = omakure(workspace.path(), &["theme", "path"]);
    assert_success(&theme_path);
    assert!(String::from_utf8_lossy(&theme_path.stdout).contains("Config dir"));

    let theme_preview = omakure(workspace.path(), &["theme", "preview", "default"]);
    assert_success(&theme_preview);
    assert!(String::from_utf8_lossy(&theme_preview.stdout).contains("Theme:"));

    let serve_once = omakure(workspace.path(), &["serve", "--once", "--no-worker"]);
    assert_success(&serve_once);

    // Host-safe boundary: status probe must not mutate systemd units.
    let serve_status = omakure(workspace.path(), &["serve", "--status"]);
    assert_eq!(
        serve_status.status.code(),
        Some(0),
        "serve --status must exit 0 (stdout_len={}, stderr_len={})",
        serve_status.stdout.len(),
        serve_status.stderr.len()
    );
    let status_out = String::from_utf8_lossy(&serve_status.stdout);
    assert!(
        status_out.contains("unit:") && status_out.contains("installed:"),
        "serve --status missing expected markers (stdout_len={})",
        serve_status.stdout.len()
    );

    let config_json = omakure(workspace.path(), &["--json", "config"]);
    assert_success(&config_json);
    let config_env = json(&config_json);
    assert_eq!(config_env["ok"], true);
    assert!(config_env["schema_version"].is_string());
    assert!(config_env["data"].is_object());

    for args in [
        ["update", "--help"].as_slice(),
        ["uninstall", "--help"].as_slice(),
        ["theme", "set", "--help"].as_slice(),
    ] {
        let output = omakure(workspace.path(), args);
        assert_success(&output);
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage"));
    }
}

#[test]
fn behavioral_flags_cover_tags_history_filters_init_force_and_queue_priority_timeout() {
    let workspace = support::TestWorkspace::new("cli_surface_flags");

    let schema =
        r#"{"Name":"tagged","Description":"flag fixture","Tags":["alpha","beta"],"Fields":[]}"#;
    let init = omakure(
        workspace.path(),
        &["--json", "init", "tools/tagged.sh", "--schema-json", schema],
    );
    assert_success(&init);
    assert_eq!(json(&init)["data"]["relative_path"], "tools/tagged.sh");

    let force = omakure(
        workspace.path(),
        &[
            "--json",
            "init",
            "tools/tagged.sh",
            "--schema-json",
            schema,
            "--force",
        ],
    );
    assert_success(&force);

    let mut body_stdin = support::omakure_command();
    body_stdin
        .arg("--scripts-dir")
        .arg(workspace.path())
        .args([
            "--json",
            "init",
            "tools/body.sh",
            "--schema-json",
            r#"{"Name":"body","Description":"body stdin","Fields":[]}"#,
            "--body-stdin",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = body_stdin.spawn().expect("spawn init --body-stdin");
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin
            .write_all(b"echo body-stdin-ok\n")
            .expect("write body");
    }
    let body_out = child.wait_with_output().expect("wait body-stdin");
    assert_success(&body_out);
    let body_path = workspace.path().join("tools/body.sh");
    let body_text = fs::read_to_string(&body_path).expect("read body script");
    assert!(
        body_text.contains("OMAKURE_SCHEMA_START") && body_text.contains("echo body-stdin-ok"),
        "body-stdin with schema-json must write schema header then stdin body (len={})",
        body_text.len()
    );

    let scripts = omakure(
        workspace.path(),
        &["--json", "scripts", "--tag", "alpha", "--tag", "beta"],
    );
    assert_success(&scripts);
    assert!(json(&scripts)["data"]
        .as_array()
        .expect("scripts data")
        .iter()
        .any(|entry| entry["relative_path"] == "tools/tagged.sh"
            || entry["path"] == "tools/tagged.sh"
            || entry.as_str() == Some("tools/tagged.sh")));

    let search = omakure(
        workspace.path(),
        &["--json", "search", "tagged", "--tag", "alpha"],
    );
    assert_success(&search);
    assert!(json(&search)["data"]
        .as_array()
        .expect("search data")
        .iter()
        .any(|entry| entry["relative_path"] == "tools/tagged.sh"));

    let run = omakure(workspace.path(), &["--json", "run", "tools/tagged.sh"]);
    assert_success(&run);
    let run_id = json(&run)["data"]["run_id"]
        .as_str()
        .expect("run id")
        .to_string();

    let history_state = omakure(
        workspace.path(),
        &["--json", "history", "list", "--state", "completed"],
    );
    assert_success(&history_state);
    assert!(json(&history_state)["data"]
        .as_array()
        .expect("history data")
        .iter()
        .any(|row| row["run_id"] == run_id));

    let history_set = omakure(
        workspace.path(),
        &["--json", "history", "list", "--state-set", "terminal"],
    );
    assert_success(&history_set);
    assert_eq!(json(&history_set)["ok"], true);

    let show = omakure(workspace.path(), &["--json", "history", "show", &run_id]);
    assert_success(&show);
    assert_eq!(json(&show)["data"]["run_id"], run_id);

    let queued = omakure(
        workspace.path(),
        &[
            "--json",
            "queue",
            "add",
            "tools/tagged.sh",
            "--run-id",
            "prio-timeout",
            "--priority",
            "9",
            "--timeout",
            "30s",
        ],
    );
    assert_success(&queued);
    assert_eq!(json(&queued)["data"]["run_id"], "prio-timeout");
    assert_eq!(json(&queued)["data"]["priority"], 9);

    let cancel = omakure(
        workspace.path(),
        &["--json", "queue", "cancel", "prio-timeout"],
    );
    assert_success(&cancel);
}

#[test]
fn env_history_queue_and_trace_subcommands_have_black_box_coverage() {
    let workspace = support::TestWorkspace::new("cli_surface_stateful");
    let script = workspace.write_schema_script(
        "trace.sh",
        "trace_fixture",
        r##""$OMAKURE_BIN" --scripts-dir "$OMAKURE_SCRIPTS_DIR" trace "trace message" --data '{"ok":true}'
echo traced"##,
    );
    support::set_executable(&script);

    for args in [
        vec!["--json", "env", "create", "prod", "HOST=old"],
        vec!["--json", "env", "replace", "prod", "HOST=new", "PORT=443"],
        vec!["--json", "env", "list"],
    ] {
        assert_success(&omakure(workspace.path(), &args));
    }

    let omakure_bin = support::omakure_bin();
    let omakure_bin = omakure_bin.to_string_lossy().to_string();
    let run = omakure_with_env(
        workspace.path(),
        &["--json", "run", "trace.sh"],
        &[("OMAKURE_BIN", omakure_bin.as_str())],
    );
    assert_success(&run);
    let run_id = json(&run)["data"]["run_id"]
        .as_str()
        .expect("run id")
        .to_string();

    for args in [
        vec!["--json", "history", "list"],
        vec!["--json", "history", "tail", "--limit", "1"],
        vec!["--json", "history", "stats"],
        vec!["--json", "history", "traces", &run_id],
    ] {
        let output = omakure(workspace.path(), &args);
        assert_success(&output);
        assert_eq!(json(&output)["ok"], true);
    }

    let traces = omakure(workspace.path(), &["--json", "history", "traces", &run_id]);
    assert!(json(&traces)["data"]
        .as_array()
        .expect("traces")
        .iter()
        .any(|trace| { trace["message"] == "trace message" }));

    let queued = omakure(
        workspace.path(),
        &[
            "--json",
            "queue",
            "add",
            "trace.sh",
            "--run-id",
            "queued-cancel",
        ],
    );
    assert_success(&queued);
    let cancel = omakure(
        workspace.path(),
        &["--json", "queue", "cancel", "queued-cancel"],
    );
    assert_success(&cancel);
    assert_eq!(json(&cancel)["data"]["state"], "cancelled");

    let failing = workspace.write_schema_script("fail.sh", "fail_fixture", "exit 7");
    support::set_executable(&failing);
    let enqueue_fail = omakure(
        workspace.path(),
        &[
            "--json",
            "queue",
            "add",
            "fail.sh",
            "--run-id",
            "queued-dead",
        ],
    );
    assert_success(&enqueue_fail);
    let worker = omakure(workspace.path(), &["--json", "queue", "worker", "--once"]);
    assert_success(&worker);
    let dead_letter = omakure(
        workspace.path(),
        &["--json", "queue", "dead-letter", "queued-dead"],
    );
    assert_success(&dead_letter);
    assert_eq!(json(&dead_letter)["data"]["state"], "dead_letter");

    let stats = omakure(workspace.path(), &["--json", "queue", "stats"]);
    assert_success(&stats);
    assert!(json(&stats)["data"]["counts_by_state"].is_object());

    let delete = omakure(workspace.path(), &["--json", "env", "delete", "prod"]);
    assert_success(&delete);
}

#[test]
fn battery_lifecycle_subcommands_work_against_local_repo() {
    let workspace = support::TestWorkspace::new("cli_surface_battery");
    let repo = support::TestWorkspace::new("cli_surface_battery_repo");
    write_battery_repo(repo.path());

    let add = omakure(
        workspace.path(),
        &[
            "--json",
            "battery",
            "add",
            repo.path().to_str().expect("repo path"),
            "--name",
            "local",
        ],
    );
    assert_success(&add);

    for args in [
        vec!["--json", "battery", "list"],
        vec!["--json", "battery", "sync", "local"],
        vec!["--json", "battery", "inspect", "local"],
        vec!["--json", "battery", "scripts", "local"],
        vec!["--json", "battery", "install", "local", "local.echo"],
    ] {
        let output = omakure(workspace.path(), &args);
        assert_success(&output);
        assert_eq!(json(&output)["ok"], true);
    }
    assert!(workspace.path().join("scripts/echo.sh").exists());

    // Second install without --force should fail; --force overwrites.
    let conflict = omakure(
        workspace.path(),
        &["--json", "battery", "install", "local", "local.echo"],
    );
    assert!(
        !conflict.status.success(),
        "second install without --force must fail"
    );
    let forced = omakure(
        workspace.path(),
        &[
            "--json",
            "battery",
            "install",
            "local",
            "local.echo",
            "--force",
        ],
    );
    assert_success(&forced);
    assert_eq!(json(&forced)["ok"], true);

    let remove = omakure(
        workspace.path(),
        &["--json", "battery", "remove", "local", "--remove-cache"],
    );
    assert_success(&remove);
    assert_eq!(json(&remove)["ok"], true);
}

fn write_battery_repo(root: &Path) {
    fs::create_dir_all(root.join("scripts")).expect("create battery scripts dir");
    fs::write(
        root.join("omakure-battery.toml"),
        r#"[battery]
name = "local"
version = "0.1.0"
description = "Local test battery"

[[scripts]]
id = "local.echo"
path = "scripts/echo.sh"
description = "Echo fixture"
tags = ["test"]
"#,
    )
    .expect("write manifest");
    fs::write(
        root.join("scripts/echo.sh"),
        r#"#!/bin/sh
# OMAKURE_SCHEMA_START
# {"Name":"Battery Echo","Description":"Echo fixture","Fields":[]}
# OMAKURE_SCHEMA_END
echo battery
"#,
    )
    .expect("write battery script");
    support::set_executable(&root.join("scripts/echo.sh"));
    run_git(root, &["init", "-b", "main"]);
    run_git(root, &["add", "."]);
    run_git(
        root,
        &[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Test User",
            "commit",
            "-m",
            "battery fixture",
        ],
    );
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn omakure(workspace: &Path, args: &[&str]) -> Output {
    omakure_with_env(workspace, args, &[])
}

fn omakure_large_output(workspace: &Path, args: &[&str]) -> Output {
    support::omakure_command()
        .arg("--scripts-dir")
        .arg(workspace)
        .args(args)
        .output()
        .expect("run omakure command")
}

fn omakure_with_env(workspace: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = support::omakure_command();
    command.arg("--scripts-dir").arg(workspace).args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    support::command_with_timeout(&mut command, Duration::from_secs(20))
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected success, status: {:?}, stdout_len: {}, stderr_len: {}",
        output.status.code(),
        output.stdout.len(),
        output.stderr.len()
    );
}

fn json(output: &Output) -> Value {
    support::json_envelope(&output.stdout)
}

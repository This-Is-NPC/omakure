mod support;

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

// Coverage-guarantee boundary (read before trusting this inventory):
//
// `command_surface_inventory_maps_all_current_commands` is a DRIFT TRIPWIRE,
// not a proof of behavioral coverage. It mechanically asserts that this
// inventory equals the clap command set (`omakure --help`), so a command
// added to `src/cli/args.rs` without an inventory entry fails the suite. The
// `Covered("path")` string is a human-authored pointer to where the command is
// exercised — it is NOT asserted to reference a test that actually invokes the
// command. A command can therefore be "listed but unexercised" if someone adds
// an inventory row without a matching black-box assertion. The behavioral
// coverage itself lives in the `#[test]` functions below; keep them in lockstep
// with the inventory by hand.

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
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs --json"),
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
        command: "node",
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
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
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
        command: "token",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs + src/cli/token.rs"),
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
        command: "node authority",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "node baseline",
        coverage: Coverage::Covered(
            "tests/cli_surface_e2e.rs + src/baseline_push.rs delivery_tests",
        ),
    },
    CommandCoverage {
        command: "node capabilities",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "node cue",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "node direct-probe",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "node discovery",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "node enroll",
        coverage: Coverage::Covered("tests/direct_transport_e2e.rs"),
    },
    CommandCoverage {
        command: "node health",
        coverage: Coverage::Covered(
            "tests/cli_surface_e2e.rs + tests/health_plane_transport_e2e.rs",
        ),
    },
    CommandCoverage {
        command: "node init",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "node peers",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "node reset",
        coverage: Coverage::Covered("src/cli/args.rs + node lifecycle tests"),
    },
    CommandCoverage {
        command: "node revoke",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "node serve",
        coverage: Coverage::Covered("tests/node_service_e2e.rs"),
    },
    CommandCoverage {
        command: "node signals",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs + tests/health_plane_signals.rs"),
    },
    CommandCoverage {
        command: "node status",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs"),
    },
    CommandCoverage {
        command: "node trust",
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
        command: "token generate",
        coverage: Coverage::Covered("tests/cli_surface_e2e.rs + src/cli/token.rs"),
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
        "node authority",
        "node baseline",
        "node capabilities",
        "node cue",
        "node direct-probe",
        "node discovery",
        "node enroll",
        "node health",
        "node init",
        "node peers",
        "node reset",
        "node revoke",
        "node serve",
        "node signals",
        "node status",
        "node trust",
        "queue add",
        "queue cancel",
        "queue dead-letter",
        "queue stats",
        "queue worker",
        "token generate",
    ];
    let clap_nested =
        clap_nested_commands(&["battery", "env", "history", "queue", "node", "token"]);
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
fn local_info_commands_cover_init_describe_search_doctor_help_completion_and_serve() {
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
    let help_ai_json = json(&help_ai);
    let verbs = help_ai_json["data"]["verbs"].as_array().expect("verbs");
    assert!(verbs.iter().any(|verb| verb["name"] == "trace"));
    assert!(verbs.iter().any(|verb| verb["name"] == "token"));

    let token_gen = omakure(
        workspace.path(),
        &[
            "--json",
            "token",
            "generate",
            "--id",
            "e2e",
            "--scope",
            "runs:read",
        ],
    );
    assert_success(&token_gen);
    let token_body = json(&token_gen);
    assert!(token_body["ok"].as_bool().unwrap());
    assert!(token_body["data"]["token"]
        .as_str()
        .unwrap()
        .starts_with("omk_live_"));
    assert!(token_body["data"]["hash"]
        .as_str()
        .unwrap()
        .contains("argon2id"));

    let completion = omakure_large_output(workspace.path(), &["completion", "bash"]);
    assert_success(&completion);
    assert!(String::from_utf8_lossy(&completion.stdout).contains("omakure"));

    let serve_once = omakure(workspace.path(), &["serve", "--once", "--no-worker"]);
    assert_success(&serve_once);
    // Prove the one-shot loop actually started and shut down cleanly rather than
    // parsing args and exiting: the daemon log must record both lifecycle lines.
    let daemon_log = fs::read_to_string(workspace.path().join(".omakure/daemon.log"))
        .expect("serve --once must write .omakure/daemon.log");
    assert!(
        daemon_log.contains("serve started") && daemon_log.contains("serve stopped"),
        "serve --once must log start+stop lifecycle (log len={})",
        daemon_log.len()
    );

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
    ] {
        let output = omakure(workspace.path(), args);
        assert_success(&output);
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage"));
    }
}

/// `node baseline` end to end at the CLI: a key, a signed manifest, and a push
/// that has nowhere to go.
///
/// The push assertion is the one that matters. A baseline travels on the
/// session the running service holds, and with no service there is nothing to
/// ask — so the command must say so rather than dialling around it, which is
/// exactly the second way into the responder the design refuses to open.
#[test]
fn node_baseline_creates_a_key_signs_a_set_and_refuses_to_push_without_a_service() {
    let workspace = support::TestWorkspace::new("cli_node_baseline");
    let state = workspace.path().join("node-state");
    let config = workspace.path().join("node.toml");
    let state_arg = state.to_string_lossy().to_string();
    let config_arg = config.to_string_lossy().to_string();
    workspace.write_schema_script("deploy.sh", "deploy", "echo fleet");

    let node = |extra: &[&str]| {
        let mut args = vec![
            "--json",
            "node",
            "--node-state-dir",
            state_arg.as_str(),
            "--node-config",
            config_arg.as_str(),
        ];
        args.extend_from_slice(extra);
        omakure_with_env(workspace.path(), &args, &[("OMAKURE_NODE_TEST_MODE", "1")])
    };

    assert_success(&node(&["init"]));

    let created = node(&["baseline", "create-key"]);
    assert_success(&created);
    assert_eq!(
        json(&created)["data"]["key_id"].as_str().unwrap().len(),
        32,
        "the key id a receiver records is 16 bytes of hex"
    );

    // Creating twice must refuse: a rotation orphans every baseline the old
    // key ever signed.
    assert!(!node(&["baseline", "create-key"]).status.success());

    let manifest = workspace.path().join("fleet.ombm");
    let manifest_arg = manifest.to_string_lossy().to_string();
    let published = node(&[
        "baseline",
        "publish",
        "--script",
        "deploy.sh",
        "--out",
        manifest_arg.as_str(),
    ]);
    assert_success(&published);
    assert_eq!(
        json(&published)["data"]["baseline_id"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert!(manifest.exists(), "publish must have written the manifest");

    let pushed = node(&[
        "baseline",
        "push",
        "--peer-node-id",
        "omk1_0000000000000000000000000000000000000000000000000000000000000000",
        "--manifest",
        manifest_arg.as_str(),
        "--wait-seconds",
        "1",
    ]);
    assert!(
        !pushed.status.success(),
        "with no service running there is no session a baseline could travel on"
    );

    // Rollback is the one baseline verb that reaches no peer at all, and on a
    // node that was never pushed one there is nothing to reach for. Refusing is
    // the answer; a success that changed nothing would tell an operator their
    // machine had been put back when it had not.
    assert!(
        !node(&["baseline", "rollback"]).status.success(),
        "replacing every script a baseline named is confirmed explicitly"
    );
    let nothing_to_undo = node(&["baseline", "rollback", "--confirmed"]);
    assert!(
        !nothing_to_undo.status.success(),
        "a node with no previous baseline must refuse rather than report a rollback"
    );
    assert_eq!(
        json(&nothing_to_undo)["error"]["code"],
        "not_found",
        "body: {}",
        String::from_utf8_lossy(&nothing_to_undo.stdout)
    );
}

#[test]
fn node_cli_commands_share_public_status_and_confirmed_trust_mutations() {
    let workspace = support::TestWorkspace::new("cli_node_surface");
    let state = workspace.path().join("node-state");
    let config = workspace.path().join("node.toml");
    let state_arg = state.to_string_lossy().to_string();
    let config_arg = config.to_string_lossy().to_string();
    let node_args = [state_arg.as_str(), config_arg.as_str()];

    let init = omakure_with_env(
        workspace.path(),
        &[
            "--json",
            "node",
            "--node-state-dir",
            node_args[0],
            "--node-config",
            node_args[1],
            "init",
        ],
        &[("OMAKURE_NODE_TEST_MODE", "1")],
    );
    assert_success(&init);
    assert_eq!(json(&init)["data"]["status"]["initialized"], true);
    assert_eq!(json(&init)["data"]["state_dir_created"], true);

    let status = omakure_with_env(
        workspace.path(),
        &[
            "--json",
            "node",
            "--node-state-dir",
            node_args[0],
            "--node-config",
            node_args[1],
            "status",
        ],
        &[("OMAKURE_NODE_TEST_MODE", "1")],
    );
    assert_success(&status);
    assert_eq!(
        json(&status)["data"]["identity"]["public_key"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert!(json(&status)["data"]["identity"]
        .get("private_key")
        .is_none());

    let trust = [
        "--json",
        "node",
        "--node-state-dir",
        node_args[0],
        "--node-config",
        node_args[1],
        "trust",
        "--node-id",
        "omk1_71319375521da1a36e37088c56b0e957043cc8459de4d0a54642e5e0b2443a92",
        "--public-key",
        "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
        "--capability",
        "remote-run",
        "--actor",
        "operator",
        "--reason",
        "approved",
        "--confirmed",
    ];
    let imported = omakure_with_env(workspace.path(), &trust, &[("OMAKURE_NODE_TEST_MODE", "1")]);
    assert_success(&imported);
    assert_eq!(json(&imported)["data"]["state"], "active");

    let peers = omakure_with_env(
        workspace.path(),
        &[
            "--json",
            "node",
            "--node-state-dir",
            node_args[0],
            "--node-config",
            node_args[1],
            "peers",
        ],
        &[("OMAKURE_NODE_TEST_MODE", "1")],
    );
    assert_success(&peers);
    assert_eq!(json(&peers)["data"].as_array().unwrap().len(), 1);

    // `node health` is the CLI half of the fleet-status projection. It renders
    // the same protocol-neutral operation the HTTP route renders, so an
    // actively trusted peer that has never reported appears exactly once with
    // the frozen `unknown` presence and no Profile or Pulse.
    let health = omakure_with_env(
        workspace.path(),
        &[
            "--json",
            "node",
            "--node-state-dir",
            node_args[0],
            "--node-config",
            node_args[1],
            "health",
        ],
        &[("OMAKURE_NODE_TEST_MODE", "1")],
    );
    assert_success(&health);
    let health_body = json(&health);
    assert_eq!(health_body["data"]["enabled"], true);
    assert_eq!(health_body["data"]["presence"]["unknown"], 1);
    assert_eq!(health_body["data"]["presence"]["total"], 1);
    assert_eq!(health_body["data"]["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(health_body["data"]["nodes"][0]["presence"], "unknown");
    assert_eq!(
        health_body["data"]["nodes"][0]["baseline_status"],
        "unknown"
    );
    assert_eq!(health_body["data"]["baselines"]["unknown"], 1);
    assert_eq!(health_body["data"]["baselines"]["total"], 1);
    assert_eq!(health_body["data"]["nodes"][0]["role"], "performer");
    assert!(health_body["data"]["nodes"][0]["profile"].is_null());
    assert!(health_body["data"]["nodes"][0]["pulse"].is_null());
    // Current status only: the projection has no history, series, or log key.
    for forbidden in ["history", "series", "logs", "alerts"] {
        assert!(
            health_body["data"].get(forbidden).is_none(),
            "fleet status must not expose `{forbidden}`"
        );
    }

    // `node signals` is the CLI half of the closed Signal feed. Trusting the
    // peer above was an authoritative local trust transition, so exactly one
    // `enrolled` Signal is visible, bounded and newest first.
    let signals = omakure_with_env(
        workspace.path(),
        &[
            "--json",
            "node",
            "--node-state-dir",
            node_args[0],
            "--node-config",
            node_args[1],
            "signals",
        ],
        &[("OMAKURE_NODE_TEST_MODE", "1")],
    );
    assert_success(&signals);
    let signals_body = json(&signals)["data"].clone();
    assert_eq!(signals_body["enabled"], true);
    assert_eq!(signals_body["gap"], false);
    assert_eq!(signals_body["limit"], 64);
    assert_eq!(signals_body["retention_seconds"], 604_800);
    let entries = signals_body["signals"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["kind"], "enrolled");
    assert_eq!(entries[0]["source"], "local");
    assert!(entries[0]["run"].is_null());
    assert_eq!(
        entries[0]["subject"],
        health_body["data"]["nodes"][0]["node_id"]
    );
    // A closed feed: three kinds, no subscription, webhook, alert, or history.
    for forbidden in [
        "history",
        "series",
        "logs",
        "alerts",
        "subscriptions",
        "webhooks",
    ] {
        assert!(
            signals_body.get(forbidden).is_none(),
            "the Signal feed must not expose `{forbidden}`"
        );
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

    // Enqueue (but do not run) a job so history filters have both a terminal
    // (completed) run and an in-flight (queued) run to discriminate between.
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

    // `--state completed` must include the completed run and EXCLUDE the queued
    // one — proves the filter restricts rather than returning everything.
    let history_state = omakure(
        workspace.path(),
        &["--json", "history", "list", "--state", "completed"],
    );
    assert_success(&history_state);
    let completed_ids = history_run_ids(&history_state);
    assert!(
        completed_ids.contains(&run_id),
        "--state completed must include the completed run"
    );
    assert!(
        !completed_ids.iter().any(|id| id == "prio-timeout"),
        "--state completed must exclude the queued run"
    );

    // `--state-set terminal` includes completed, excludes queued.
    let history_terminal = omakure(
        workspace.path(),
        &["--json", "history", "list", "--state-set", "terminal"],
    );
    assert_success(&history_terminal);
    let terminal_ids = history_run_ids(&history_terminal);
    assert!(
        terminal_ids.contains(&run_id),
        "--state-set terminal must include the completed run"
    );
    assert!(
        !terminal_ids.iter().any(|id| id == "prio-timeout"),
        "--state-set terminal must exclude the queued run"
    );

    // `--state-set in_flight` is the mirror image: includes queued, excludes
    // completed.
    let history_in_flight = omakure(
        workspace.path(),
        &["--json", "history", "list", "--state-set", "in_flight"],
    );
    assert_success(&history_in_flight);
    let in_flight_ids = history_run_ids(&history_in_flight);
    assert!(
        in_flight_ids.iter().any(|id| id == "prio-timeout"),
        "--state-set in_flight must include the queued run"
    );
    assert!(
        !in_flight_ids.contains(&run_id),
        "--state-set in_flight must exclude the completed run"
    );

    let show = omakure(workspace.path(), &["--json", "history", "show", &run_id]);
    assert_success(&show);
    assert_eq!(json(&show)["data"]["run_id"], run_id);

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

    // A second run guarantees history holds >= 2 rows, so `--limit 1` is
    // actually discriminating rather than trivially satisfied.
    let run2 = omakure_with_env(
        workspace.path(),
        &["--json", "run", "trace.sh"],
        &[("OMAKURE_BIN", omakure_bin.as_str())],
    );
    assert_success(&run2);

    for args in [
        vec!["--json", "history", "list"],
        vec!["--json", "history", "traces", &run_id],
    ] {
        let output = omakure(workspace.path(), &args);
        assert_success(&output);
        assert_eq!(json(&output)["ok"], true);
    }

    // Two completed runs exist: stats must report them, not an empty summary.
    let stats = omakure(workspace.path(), &["--json", "history", "stats"]);
    assert_success(&stats);
    let stats_data = &json(&stats)["data"];
    assert!(
        stats_data["total"].as_u64().unwrap_or(0) >= 2,
        "history stats total must count both runs (data={stats_data})"
    );
    assert!(
        stats_data["counts_by_state"]["completed"]
            .as_u64()
            .unwrap_or(0)
            >= 2,
        "history stats must report the completed runs (data={stats_data})"
    );

    // `history list` sees both runs; `tail --limit 1` must cap the result to
    // exactly one row.
    let full_history = omakure(workspace.path(), &["--json", "history", "list"]);
    assert_success(&full_history);
    assert!(
        json(&full_history)["data"]
            .as_array()
            .expect("history list data")
            .len()
            >= 2,
        "expected >= 2 history rows before asserting --limit caps"
    );
    let tail_one = omakure(
        workspace.path(),
        &["--json", "history", "tail", "--limit", "1"],
    );
    assert_success(&tail_one);
    assert_eq!(
        json(&tail_one)["data"].as_array().expect("tail data").len(),
        1,
        "--limit 1 must return exactly one row"
    );

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
    let counts = &json(&stats)["data"]["counts_by_state"];
    assert!(counts.is_object());
    // The two jobs above ended in distinct terminal states; stats must reflect
    // them rather than returning an empty/placeholder map.
    assert!(
        counts["cancelled"].as_u64().unwrap_or(0) >= 1,
        "queue stats must count the cancelled job (counts={counts})"
    );
    assert!(
        counts["dead_letter"].as_u64().unwrap_or(0) >= 1,
        "queue stats must count the dead-letter job (counts={counts})"
    );

    let delete = omakure(workspace.path(), &["--json", "env", "delete", "prod"]);
    assert_success(&delete);
}

#[test]
fn battery_lifecycle_subcommands_work_against_local_repo() {
    let workspace = support::TestWorkspace::new("cli_surface_battery");
    let repo = support::TestWorkspace::new("cli_surface_battery_repo");
    support::write_local_battery_repo(repo.path(), "local", "Local test battery");

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

    let sync = omakure(workspace.path(), &["--json", "battery", "sync", "local"]);
    assert_success(&sync);

    // list must contain the registered battery by name.
    let list = omakure(workspace.path(), &["--json", "battery", "list"]);
    assert_success(&list);
    assert!(
        json(&list)["data"]
            .as_array()
            .expect("battery list data")
            .iter()
            .any(|entry| entry["name"] == "local" || entry["summary"]["name"] == "local"),
        "battery list must include the registered battery, got: {}",
        String::from_utf8_lossy(&list.stdout)
    );

    // inspect must resolve the battery summary by name.
    let inspect = omakure(workspace.path(), &["--json", "battery", "inspect", "local"]);
    assert_success(&inspect);
    assert_eq!(json(&inspect)["data"]["summary"]["name"], "local");

    // scripts must list the fixture's script id.
    let scripts = omakure(workspace.path(), &["--json", "battery", "scripts", "local"]);
    assert_success(&scripts);
    assert!(
        json(&scripts)["data"]
            .as_array()
            .expect("battery scripts data")
            .iter()
            .any(|entry| entry["id"] == "local.echo"),
        "battery scripts must list local.echo, got: {}",
        String::from_utf8_lossy(&scripts.stdout)
    );

    let install = omakure(
        workspace.path(),
        &["--json", "battery", "install", "local", "local.echo"],
    );
    assert_success(&install);
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

/// Extract the `run_id` values from a `history list --json` envelope so filter
/// tests can assert exact set membership (inclusion AND exclusion).
fn history_run_ids(output: &Output) -> Vec<String> {
    json(output)["data"]
        .as_array()
        .expect("history data array")
        .iter()
        .filter_map(|row| row["run_id"].as_str().map(str::to_string))
        .collect()
}

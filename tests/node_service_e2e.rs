mod support;

use serde_json::json;
use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const API_TOKEN: &str = "node-service-e2e-token-with-enough-entropy-00001";

fn write_echo_script(root: &Path, name: &str) {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create script parent");
    }
    let script = r#"#!/bin/sh
# OMAKURE_SCHEMA_START
# {
#   "Name": "echo-job",
#   "Description": "node service e2e fixture",
#   "Fields": []
# }
# OMAKURE_SCHEMA_END
echo node-service-ok
"#;
    std::fs::write(&path, script).expect("write echo script");
    support::set_executable(&path);
}

#[test]
fn node_service_workers_zero_no_scheduler_serves_http_like_api() {
    let workspace = support::TestWorkspace::new("node_service_api_only");
    write_echo_script(workspace.path(), "echo.sh");
    let server = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--capability",
            "scripts:read",
            "--capability",
            "runs:read",
            "--capability",
            "runs:write",
        ],
        &[],
        Duration::from_secs(15),
    );

    let health = server.get_unauthenticated("/v1/health");
    assert_eq!(health.status, 200, "body: {}", health.safe_body());

    let ready = server.await_ready(Duration::from_secs(10));
    assert_eq!(ready.status, 200, "body: {}", ready.safe_body());
    assert_eq!(ready.json()["data"]["status"], "ready");
    let ready_body = ready.body.clone();
    assert!(!ready_body.contains(API_TOKEN));
    assert!(!ready_body.contains(workspace.path().to_string_lossy().as_ref()));

    let scripts = server.get("/v1/scripts");
    assert_eq!(scripts.status, 200, "body: {}", scripts.safe_body());
    assert_eq!(scripts.json()["ok"], true);

    let enqueue = server.post_json(
        "/v1/runs",
        &json!({
            "script": "echo.sh",
            "run_id": "node-service-api-only-run"
        }),
    );
    assert_eq!(enqueue.status, 200, "body: {}", enqueue.safe_body());
    assert_eq!(enqueue.json()["data"]["state"], "queued");

    // No in-process worker: run stays queued.
    let show = server.get("/v1/runs/node-service-api-only-run");
    assert_eq!(show.status, 200, "body: {}", show.safe_body());
    assert_eq!(show.json()["data"]["state"], "queued");
}

#[test]
fn node_service_workers_complete_enqueued_run_in_process() {
    let workspace = support::TestWorkspace::new("node_service_workers");
    write_echo_script(workspace.path(), "echo.sh");
    let server = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &[
            "--workers",
            "1",
            "--no-scheduler",
            "--capability",
            "runs:read",
            "--capability",
            "runs:write",
        ],
        &[],
        Duration::from_secs(15),
    );

    let enqueue = server.post_json(
        "/v1/runs",
        &json!({
            "script": "echo.sh",
            "run_id": "node-service-worker-run"
        }),
    );
    assert_eq!(enqueue.status, 200, "body: {}", enqueue.safe_body());

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let show = server.get("/v1/runs/node-service-worker-run");
        assert_eq!(show.status, 200, "body: {}", show.safe_body());
        let show_json = show.json();
        let state = show_json["data"]["state"].as_str().unwrap_or("");
        if state == "completed" {
            assert!(
                show_json["data"]["stdout"]
                    .as_str()
                    .unwrap_or("")
                    .contains("node-service-ok"),
                "stdout: {}",
                show.safe_body()
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run did not complete in time; last={}",
            show.safe_body()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn node_service_ready_unauthenticated_and_minimal() {
    let workspace = support::TestWorkspace::new("node_service_ready");
    let server = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &["--workers", "0", "--no-scheduler"],
        &[],
        Duration::from_secs(15),
    );

    let ready = server.await_ready(Duration::from_secs(10));
    assert_eq!(ready.status, 200, "body: {}", ready.safe_body());
    let json = ready.json();
    assert_eq!(json["ok"], true);
    assert_eq!(json["data"]["status"], "ready");
    let keys: Vec<_> = json["data"]
        .as_object()
        .expect("data object")
        .keys()
        .cloned()
        .collect();
    assert_eq!(keys, vec!["status".to_string()]);
}

#[test]
fn node_service_ready_with_readiness_requires_flags_when_loops_alive() {
    let workspace = support::TestWorkspace::new("node_service_ready_flags");
    let server = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &[
            "--workers",
            "1",
            "--scheduler",
            "--readiness-requires-worker",
            "--readiness-requires-scheduler",
        ],
        &[],
        Duration::from_secs(15),
    );

    let ready = server.await_ready(Duration::from_secs(10));
    assert_eq!(ready.status, 200, "body: {}", ready.safe_body());
    assert_eq!(ready.json()["data"]["status"], "ready");
}

#[cfg(unix)]
#[test]
fn node_service_sigterm_stops_cleanly() {
    let workspace = support::TestWorkspace::new("node_service_sigterm");
    let mut server = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &["--workers", "1", "--no-scheduler"],
        &[],
        Duration::from_secs(15),
    );

    let pid = server.child_id();
    let ready = server.await_ready(Duration::from_secs(10));
    assert_eq!(ready.status, 200, "body: {}", ready.safe_body());

    // SAFETY: libc::kill is the standard way to deliver SIGTERM to a child.
    let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    assert_eq!(rc, 0, "kill(SIGTERM) failed");

    let status = server.wait_exit(Duration::from_secs(15));
    assert!(
        status.success() || status.code() == Some(0) || status.code().is_none(),
        "node service exit status: {status:?}"
    );
}

#[test]
fn harness_kill_exit_is_expected_on_this_platform() {
    assert!(support::terminated_exit_is_expected_for_parts(
        true,
        Some(0)
    ));
    assert!(support::terminated_exit_is_expected_for_parts(false, None));
    assert!(!support::terminated_exit_is_expected_for_parts(
        false,
        Some(2)
    ));
    #[cfg(windows)]
    assert!(support::terminated_exit_is_expected_for_parts(
        false,
        Some(1)
    ));
    #[cfg(not(windows))]
    assert!(!support::terminated_exit_is_expected_for_parts(
        false,
        Some(1)
    ));
}

#[test]
fn node_service_can_be_terminated_and_restarted_portably() {
    let workspace = support::TestWorkspace::new("node_service_restart");
    let server = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &["--workers", "0", "--no-scheduler"],
        &[],
        Duration::from_secs(15),
    );
    let identity_before = fs::read(workspace.path().join(".node-state/identity.key")).unwrap();
    assert_eq!(server.await_ready(Duration::from_secs(10)).status, 200);
    let status = server.terminate();
    support::assert_terminated(status);

    let restarted = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &["--workers", "0", "--no-scheduler"],
        &[],
        Duration::from_secs(15),
    );
    assert_eq!(restarted.await_ready(Duration::from_secs(10)).status, 200);
    assert_eq!(
        fs::read(workspace.path().join(".node-state/identity.key")).unwrap(),
        identity_before
    );
}

#[test]
fn first_start_creates_one_stable_identity_and_empty_node_registry() {
    let workspace = support::TestWorkspace::new("node_service_identity");
    let args = [
        "--workers",
        "0",
        "--no-scheduler",
        "--capability",
        "node:read",
    ];
    let first = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &args,
        &[],
        Duration::from_secs(15),
    );
    let first_status = first.get("/v1/node/status");
    assert_eq!(
        first_status.status,
        200,
        "body: {}",
        first_status.safe_body()
    );
    let first_identity = first_status.json()["data"]["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();
    drop(first);

    let second = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &args,
        &[],
        Duration::from_secs(15),
    );
    let second_status = second.get("/v1/node/status");
    assert_eq!(
        second_status.status,
        200,
        "body: {}",
        second_status.safe_body()
    );
    assert_eq!(
        second_status.json()["data"]["identity"]["node_id"],
        first_identity
    );
    assert_eq!(second_status.json()["data"]["trust"]["peer_count"], 0);
    assert_eq!(
        second_status.json()["data"]["config"]["enrollment"],
        "disabled"
    );
    assert_eq!(
        second_status.json()["data"]["trust"]["active_peer_count"],
        0
    );

    let state = workspace.path().join(".node-state");
    let names = fs::read_dir(state)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        names.iter().filter(|name| *name == "identity.key").count(),
        1
    );
    assert_eq!(
        names.iter().filter(|name| *name == "node.sqlite").count(),
        1
    );
}

#[test]
fn corrupt_node_state_fails_before_readiness_without_replacing_identity() {
    let workspace = support::TestWorkspace::new("node_service_corrupt");
    let state = workspace.path().join("state");
    let config = workspace.path().join("node.toml");
    let state_arg = state.to_string_lossy().to_string();
    let config_arg = config.to_string_lossy().to_string();
    let envs = [("OMAKURE_NODE_TEST_MODE", "1")];
    let init = support::command_with_timeout(
        support::omakure_command()
            .arg("--scripts-dir")
            .arg(workspace.path())
            .args([
                "node",
                "--node-state-dir",
                &state_arg,
                "--node-config",
                &config_arg,
                "init",
            ])
            .envs(envs),
        Duration::from_secs(10),
    );
    assert!(init.status.success(), "init failed: {:?}", init.status);
    let identity_before = fs::read(state.join("identity.key")).unwrap();
    fs::write(state.join("node.sqlite"), b"corrupt node registry").unwrap();

    let output = support::command_with_timeout(
        support::omakure_command()
            .arg("--scripts-dir")
            .arg(workspace.path())
            .args([
                "node",
                "--node-state-dir",
                &state_arg,
                "--node-config",
                &config_arg,
                "serve",
                "--workers",
                "0",
                "--no-scheduler",
            ])
            .env("OMAKURE_NODE_TEST_MODE", "1")
            .env("OMAKURE_API_TOKEN", API_TOKEN),
        Duration::from_secs(5),
    );
    assert!(!output.status.success());
    assert_eq!(
        fs::read(state.join("identity.key")).unwrap(),
        identity_before
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("node trust registry is invalid or corrupt"));
    assert!(!stderr.contains("corrupt node registry"));
}

#[cfg(unix)]
#[test]
fn bootstrap_token_recovery_failure_aborts_before_listener_readiness() {
    let workspace = support::TestWorkspace::new("node_service_token_recovery_failure");
    let state = workspace.path().join("state");
    let config = workspace.path().join("node.toml");
    let state_arg = state.to_string_lossy().to_string();
    let config_arg = config.to_string_lossy().to_string();
    let init = support::command_with_timeout(
        support::omakure_command()
            .arg("--scripts-dir")
            .arg(workspace.path())
            .args([
                "node",
                "--node-state-dir",
                &state_arg,
                "--node-config",
                &config_arg,
                "init",
            ])
            .env("OMAKURE_NODE_TEST_MODE", "1"),
        Duration::from_secs(10),
    );
    assert!(init.status.success(), "init failed: {:?}", init.status);

    let token = "startup-recovery-token-with-enough-entropy-00001";
    let nonce = [7_u8; 16];
    let config_text = format!(
        r#"version = 1

[node]
display_name = "recovery-failure"

[api]
bind = "127.0.0.1:7878"

[network]
mode = "direct"
relays = []
static_peers = []
max_message_bytes = 1048576

[trust]
enrollment = "signed-bundle"
allow_remote_cues = false
allow_baseline_push = false
authorities = [{{ key_id = "00000000000000000000000000000000", public_key = "0000000000000000000000000000000000000000000000000000000000000000", revoked = false }}]
bootstrap_token_hash = "{}"
bootstrap_nonce_hash = "{}"

[organization]
id = "omakure"
discovery_secret_ref = ""
"#,
        omakure::enrollment::hex_bytes(&omakure::enrollment::hash_bootstrap_token(
            token.as_bytes(),
        )),
        omakure::enrollment::hex_bytes(&omakure::enrollment::hash_bootstrap_nonce(&nonce)),
    );
    fs::write(&config, config_text).unwrap();

    let token_path = workspace.path().join("bootstrap.token");
    let token_target = workspace.path().join("token-target");
    fs::write(&token_target, token).unwrap();
    let tombstone = workspace
        .path()
        .join(".omakure-bootstrap-token-00000000000000000000000000000000-bootstrap.token");
    std::os::unix::fs::symlink(&token_target, &tombstone).unwrap();

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let output = support::command_with_timeout(
        Command::new(support::omakure_bin())
            .arg("--scripts-dir")
            .arg(workspace.path())
            .args([
                "node",
                "--node-state-dir",
                &state_arg,
                "--node-config",
                &config_arg,
                "serve",
                "--bind",
                &address.to_string(),
                "--workers",
                "0",
                "--no-scheduler",
                "--bootstrap-token-file",
            ])
            .arg(&token_path)
            .env("OMAKURE_NODE_TEST_MODE", "1")
            .env("OMAKURE_API_TOKEN", API_TOKEN),
        Duration::from_secs(5),
    );
    assert!(!output.status.success());
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bootstrap token cleanup recovery failed"));
    assert!(!stderr.contains(token));
    assert!(!stderr.contains(workspace.path().to_string_lossy().as_ref()));
}

#[test]
fn missing_node_registry_blocks_start_without_replacing_identity() {
    let workspace = support::TestWorkspace::new("node_service_missing_registry");
    let server = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &["--workers", "0", "--no-scheduler"],
        &[],
        Duration::from_secs(15),
    );
    drop(server);

    let identity_path = workspace.path().join(".node-state/identity.key");
    let database_path = workspace.path().join(".node-state/node.sqlite");
    let identity_before = fs::read(&identity_path).unwrap();
    fs::remove_file(&database_path).unwrap();
    let output = support::command_with_timeout(
        support::omakure_command()
            .arg("--scripts-dir")
            .arg(workspace.path())
            .args([
                "node",
                "serve",
                "--bind",
                "127.0.0.1:0",
                "--workers",
                "0",
                "--no-scheduler",
            ])
            .env("OMAKURE_NODE_TEST_MODE", "1")
            .env(
                "OMAKURE_NODE_STATE_DIR",
                workspace.path().join(".node-state"),
            )
            .env("OMAKURE_NODE_CONFIG", workspace.path().join("node.toml"))
            .env("OMAKURE_API_TOKEN", API_TOKEN),
        Duration::from_secs(10),
    );
    assert!(!output.status.success());
    assert_eq!(fs::read(identity_path).unwrap(), identity_before);
    assert!(!database_path.exists());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("node identity state is invalid or insecure"));
}

#[test]
fn reset_is_confirmed_and_workspace_independent() {
    let workspace = support::TestWorkspace::new("node_service_reset");
    let replacement_workspace = support::TestWorkspace::new("node_service_replacement");
    let state = workspace.path().join("state");
    let config = workspace.path().join("node.toml");
    let state_arg = state.to_string_lossy().to_string();
    let config_arg = config.to_string_lossy().to_string();
    let env = ("OMAKURE_NODE_TEST_MODE", "1");

    let init = |scripts_dir: &Path| {
        support::command_with_timeout(
            support::omakure_command()
                .arg("--scripts-dir")
                .arg(scripts_dir)
                .args([
                    "node",
                    "--node-state-dir",
                    &state_arg,
                    "--node-config",
                    &config_arg,
                    "init",
                ])
                .env(env.0, env.1),
            Duration::from_secs(10),
        )
    };
    assert!(init(workspace.path()).status.success());
    let before = support::command_with_timeout(
        support::omakure_command()
            .arg("--scripts-dir")
            .arg(replacement_workspace.path())
            .args([
                "--json",
                "node",
                "--node-state-dir",
                &state_arg,
                "--node-config",
                &config_arg,
                "status",
            ])
            .env(env.0, env.1),
        Duration::from_secs(10),
    );
    assert!(before.status.success());
    let before_id = support::json_envelope(&before.stdout)["data"]["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();

    let denied = support::command_with_timeout(
        support::omakure_command()
            .arg("--scripts-dir")
            .arg(workspace.path())
            .args([
                "--json",
                "node",
                "--node-state-dir",
                &state_arg,
                "--node-config",
                &config_arg,
                "reset",
            ])
            .env(env.0, env.1),
        Duration::from_secs(10),
    );
    assert!(!denied.status.success());
    assert!(state.exists());

    let reset = support::command_with_timeout(
        support::omakure_command()
            .arg("--scripts-dir")
            .arg(workspace.path())
            .args([
                "--json",
                "node",
                "--node-state-dir",
                &state_arg,
                "--node-config",
                &config_arg,
                "reset",
                "--confirmed",
            ])
            .env(env.0, env.1),
        Duration::from_secs(10),
    );
    assert!(reset.status.success());
    assert!(state.is_dir());
    assert!(!state.join("identity.key").exists());
    assert!(!state.join("node.sqlite").exists());
    assert!(config.exists());

    let restarted = init(replacement_workspace.path());
    assert!(restarted.status.success());
    let after = support::command_with_timeout(
        support::omakure_command()
            .arg("--json")
            .arg("node")
            .arg("--node-state-dir")
            .arg(&state_arg)
            .arg("--node-config")
            .arg(&config_arg)
            .arg("status")
            .env(env.0, env.1),
        Duration::from_secs(10),
    );
    assert!(after.status.success());
    let after_id = support::json_envelope(&after.stdout)["data"]["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(before_id, after_id);
}

#[test]
fn reset_while_node_service_is_active_refuses_without_mutation() {
    let workspace = support::TestWorkspace::new("node_service_reset_active");
    let server = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &["--workers", "0", "--no-scheduler"],
        &[],
        Duration::from_secs(15),
    );
    let state = workspace.path().join(".node-state");
    let identity_before = fs::read(state.join("identity.key")).unwrap();
    let database_before = fs::read(state.join("node.sqlite")).unwrap();
    let reset = support::command_with_timeout(
        support::omakure_command()
            .arg("--scripts-dir")
            .arg(workspace.path())
            .args(["--json", "node", "reset", "--confirmed"])
            .env("OMAKURE_NODE_TEST_MODE", "1")
            .env("OMAKURE_NODE_STATE_DIR", &state)
            .env("OMAKURE_NODE_CONFIG", workspace.path().join("node.toml")),
        Duration::from_secs(10),
    );
    assert!(!reset.status.success());
    assert!(String::from_utf8_lossy(&reset.stdout)
        .contains("node service is active; stop it before changing node state"));
    assert_eq!(
        fs::read(state.join("identity.key")).unwrap(),
        identity_before
    );
    assert_eq!(
        fs::read(state.join("node.sqlite")).unwrap(),
        database_before
    );
    assert_eq!(server.await_ready(Duration::from_secs(10)).status, 200);
}

#[test]
fn init_while_node_service_is_active_conflicts_without_hanging_or_mutating_identity() {
    let workspace = support::TestWorkspace::new("node_service_init_active");
    let server = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--capability",
            "node:write",
        ],
        &[],
        Duration::from_secs(15),
    );
    let state = workspace.path().join(".node-state");
    let config = workspace.path().join("node.toml");
    let identity_before = fs::read(state.join("identity.key")).unwrap();

    let api_init = server.post_json("/v1/node/init", &json!({}));
    assert_eq!(api_init.status, 409, "body: {}", api_init.safe_body());
    assert!(api_init
        .body
        .contains("node service is active; stop it before changing node state"));

    let cli_init = support::command_with_timeout(
        support::omakure_command()
            .args(["--scripts-dir", workspace.path().to_str().unwrap()])
            .args(["--json", "node", "init"])
            .env("OMAKURE_NODE_TEST_MODE", "1")
            .env("OMAKURE_NODE_STATE_DIR", &state)
            .env("OMAKURE_NODE_CONFIG", &config),
        Duration::from_secs(5),
    );
    assert!(!cli_init.status.success());
    assert!(String::from_utf8_lossy(&cli_init.stdout)
        .contains("node service is active; stop it before changing node state"));
    assert_eq!(
        fs::read(state.join("identity.key")).unwrap(),
        identity_before
    );
    assert_eq!(server.await_ready(Duration::from_secs(10)).status, 200);
}

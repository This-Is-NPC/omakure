mod support;

use serde_json::json;
use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

const API_TOKEN: &str = "policy-e2e-token-with-enough-entropy-00001";

fn write_policy(path: &Path, body: &str) {
    fs::write(path, body).expect("write policy.toml");
}

fn write_wildcard_tokens(path: &Path) -> String {
    // Use CLI token generate for a real Argon2id entry.
    let output = support::omakure_command()
        .args([
            "token", "generate", "--id", "admin", "--scope", "*", "--json",
        ])
        .output()
        .expect("token generate");
    assert!(
        output.status.success(),
        "token generate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = support::json_envelope(&output.stdout);
    let plaintext = envelope["data"]["token"].as_str().unwrap().to_string();
    let entry = envelope["data"]["tokens_file_entry"].as_str().unwrap();
    fs::write(path, format!("version = 1\n{entry}\n")).expect("write tokens");
    plaintext
}

#[test]
fn policy_read_only_blocks_writes_with_wildcard_token() {
    let workspace = support::TestWorkspace::new("policy_ro");
    workspace.write_schema_script("echo.sh", "echo", "echo ok");
    let policy_path = workspace.path().join("policy.toml");
    write_policy(
        &policy_path,
        r#"
version = 1
[routes]
writes = false
battery = true
[auth]
legacy_env_token = true
"#,
    );
    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &[
            "--policy",
            policy_path.to_str().unwrap(),
            "--capability",
            "all",
        ],
        &[],
        Duration::from_secs(20),
    );

    let scripts = server.get("/v1/scripts");
    assert_eq!(scripts.status, 200, "body: {}", scripts.safe_body());

    let enqueue = server.post_json(
        "/v1/runs",
        &json!({ "script": "echo.sh", "run_id": "policy-ro-run" }),
    );
    assert_eq!(enqueue.status, 403, "body: {}", enqueue.safe_body());
    assert_eq!(enqueue.json()["error"]["code"], "forbidden");
}

#[test]
fn policy_battery_disabled_blocks_battery_routes() {
    let workspace = support::TestWorkspace::new("policy_batt");
    let policy_path = workspace.path().join("policy.toml");
    write_policy(
        &policy_path,
        r#"
version = 1
[routes]
battery = false
[auth]
legacy_env_token = true
"#,
    );
    let server = support::HttpServer::start_with_args(
        workspace.path(),
        API_TOKEN,
        &[
            "--policy",
            policy_path.to_str().unwrap(),
            "--capability",
            "all",
        ],
        &[],
        Duration::from_secs(20),
    );

    let list = server.get("/v1/batteries");
    assert_eq!(list.status, 403, "body: {}", list.safe_body());
}

#[test]
fn policy_legacy_disabled_fails_startup_before_bind() {
    let workspace = support::TestWorkspace::new("policy_legacy");
    let policy_path = workspace.path().join("policy.toml");
    write_policy(
        &policy_path,
        r#"
version = 1
[auth]
legacy_env_token = false
"#,
    );

    let port = support::unique_loopback_port();
    let addr = format!("127.0.0.1:{port}");

    let mut child = support::omakure_command()
        .arg("--scripts-dir")
        .arg(workspace.path())
        .arg("api")
        .arg("--bind")
        .arg(&addr)
        .arg("--policy")
        .arg(&policy_path)
        .env("OMAKURE_API_TOKEN", API_TOKEN)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn api");

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("api should have exited on legacy_env_token=false");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(!status.success(), "expected non-zero exit");

    // Port must still be free (never bound).
    let probe = std::net::TcpListener::bind(("127.0.0.1", port));
    assert!(
        probe.is_ok(),
        "bind should still be free after policy failure"
    );
}

#[test]
fn policy_parse_error_fails_before_bind() {
    let workspace = support::TestWorkspace::new("policy_parse");
    let policy_path = workspace.path().join("policy.toml");
    write_policy(&policy_path, "version = 1\nbroken = [");

    let port = support::unique_loopback_port();
    let addr = format!("127.0.0.1:{port}");

    let output = support::command_with_timeout(
        support::omakure_command()
            .arg("--scripts-dir")
            .arg(workspace.path())
            .arg("api")
            .arg("--bind")
            .arg(&addr)
            .arg("--policy")
            .arg(&policy_path)
            .env("OMAKURE_API_TOKEN", API_TOKEN),
        Duration::from_secs(10),
    );
    assert!(!output.status.success());
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        err.contains("policy") || err.contains("parse"),
        "stderr/stdout: {err}"
    );
    assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
}

#[test]
fn policy_non_loopback_from_file_allows_zero_bind_without_cli_flag() {
    let workspace = support::TestWorkspace::new("policy_nlb");
    let tokens_path = workspace.path().join("tokens.toml");
    let plaintext = write_wildcard_tokens(&tokens_path);
    let policy_path = workspace.path().join("policy.toml");
    write_policy(
        &policy_path,
        r#"
version = 1
[http]
allow_non_loopback = true
[auth]
legacy_env_token = false
"#,
    );

    // Without policy allow_non_loopback, this would fail before bind.
    let mut command = support::omakure_command();
    // Use a non-loopback address that still works in CI: 0.0.0.0 with reserved port.
    let port = support::unique_loopback_port();
    let bind = format!("0.0.0.0:{port}");

    command
        .arg("--scripts-dir")
        .arg(workspace.path())
        .arg("api")
        .arg("--bind")
        .arg(&bind)
        .arg("--policy")
        .arg(&policy_path)
        .arg("--tokens-file")
        .arg(&tokens_path)
        .env_remove("OMAKURE_API_TOKEN")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = support::spawn_guard(&mut command);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut ready = false;
    while Instant::now() < deadline {
        if let Ok(mut stream) = std::net::TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(200),
        ) {
            use std::io::{Read, Write};
            let req = format!(
                "GET /v1/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            );
            if stream.write_all(req.as_bytes()).is_ok() {
                let mut raw = String::new();
                if stream.read_to_string(&mut raw).is_ok() && raw.contains(" 200 ") {
                    ready = true;
                    break;
                }
            }
        }
        if child.child_mut().try_wait().ok().flatten().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        ready,
        "api with policy allow_non_loopback should serve on {bind}"
    );
    let _ = plaintext;
    let _ = child.kill_and_wait();
}

#[test]
fn policy_non_loopback_denied_without_opt_in() {
    let workspace = support::TestWorkspace::new("policy_nlb_deny");
    let policy_path = workspace.path().join("policy.toml");
    write_policy(
        &policy_path,
        r#"
version = 1
[http]
allow_non_loopback = false
[auth]
legacy_env_token = true
"#,
    );
    let output = support::command_with_timeout(
        support::omakure_command()
            .arg("--scripts-dir")
            .arg(workspace.path())
            .arg("api")
            .arg("--bind")
            .arg("0.0.0.0:17999")
            .arg("--policy")
            .arg(&policy_path)
            .env("OMAKURE_API_TOKEN", API_TOKEN),
        Duration::from_secs(10),
    );
    assert!(!output.status.success());
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        err.contains("non-loopback") || err.contains("allow-non-loopback"),
        "got: {err}"
    );
}

#[test]
fn node_service_takes_workers_default_from_policy() {
    let workspace = support::TestWorkspace::new("policy_node_service");
    workspace.write_schema_script("echo.sh", "echo", "echo ok");
    let policy_path = workspace.path().join("policy.toml");
    write_policy(
        &policy_path,
        r#"
version = 1
[node]
workers = 0
scheduler = false
[auth]
legacy_env_token = true
"#,
    );
    let server = support::HttpServer::start_node_service(
        workspace.path(),
        API_TOKEN,
        &[
            "--policy",
            policy_path.to_str().unwrap(),
            // omit --workers / --no-scheduler → policy defaults
            "--capability",
            "runs:read",
            "--capability",
            "runs:write",
        ],
        &[],
        Duration::from_secs(20),
    );

    let enqueue = server.post_json(
        "/v1/runs",
        &json!({ "script": "echo.sh", "run_id": "policy-node-service-run" }),
    );
    assert_eq!(enqueue.status, 200, "body: {}", enqueue.safe_body());
    // workers=0 from policy → stays queued
    let show = server.get("/v1/runs/policy-node-service-run");
    assert_eq!(show.status, 200);
    assert_eq!(show.json()["data"]["state"], "queued");
}

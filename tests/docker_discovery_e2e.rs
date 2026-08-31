//! Opt-in Linux/Docker acceptance test for trust-neutral discovery.
//!
//! Run with:
//! `cargo test --test docker_discovery_e2e -- --ignored --nocapture`

use rusqlite::Connection;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const TARGET_API: &str = "http://127.0.0.1:17878";

fn compose_project() -> &'static str {
    static PROJECT: OnceLock<String> = OnceLock::new();
    PROJECT
        .get_or_init(|| format!("omakure-discovery-{}", std::process::id()))
        .as_str()
}

/// Every Docker call is bounded, so a wedged daemon cannot hang the suite.
///
/// Two different budgets, because two different things are being bounded. An
/// operation on a stack that is already up is fast, and 120s is a generous
/// ceiling for one. A call carrying `--build` may compile this crate inside the
/// container from a cold layer cache, which on an ordinary machine does not fit
/// in two minutes -- and when it did not, `timeout` killed the build and the
/// test reported `compose up failed`, which reads exactly like the product
/// refusing to start. One budget for both made a slow machine indistinguishable
/// from a broken node.
const COMPOSE_OPERATION_TIMEOUT: &str = "120s";
const COMPOSE_BUILD_TIMEOUT: &str = "1800s";

fn bounded_command_within(program: &str, budget: &str) -> Command {
    let mut command = Command::new("timeout");
    command.args(["--foreground", "--kill-after=10s", budget, program]);
    command
}

fn bounded_command(program: &str) -> Command {
    bounded_command_within(program, COMPOSE_OPERATION_TIMEOUT)
}

/// The budget a Compose invocation gets, decided by whether it can build.
fn compose_timeout(args: &[&str]) -> &'static str {
    if args.contains(&"--build") {
        COMPOSE_BUILD_TIMEOUT
    } else {
        COMPOSE_OPERATION_TIMEOUT
    }
}

struct ComposeGuard {
    root: PathBuf,
    _tokens_dir: TempDir,
    finalized: bool,
}

impl ComposeGuard {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let tokens_dir = TempDir::new().expect("create discovery token directory");
        let (target_tokens, target_client) = generate_auth(tokens_dir.path(), "discovery-target");
        let (candidate_tokens, candidate_client) =
            generate_auth(tokens_dir.path(), "discovery-candidate");
        std::env::set_var("OMAKURE_ENROLLMENT_TARGET_TOKENS_FILE", &target_tokens);
        std::env::set_var("OMAKURE_ENROLLMENT_TARGET_CLIENT_FILE", &target_client);
        std::env::set_var(
            "OMAKURE_ENROLLMENT_CANDIDATE_TOKENS_FILE",
            &candidate_tokens,
        );
        std::env::set_var(
            "OMAKURE_ENROLLMENT_CANDIDATE_CLIENT_FILE",
            &candidate_client,
        );
        let guard = Self {
            root,
            _tokens_dir: tokens_dir,
            finalized: false,
        };
        guard.start();
        guard
    }

    fn start(&self) {
        if std::env::var_os("OMAKURE_E2E_INDUCE_PARTIAL_UP").is_some() {
            let partial = compose(
                &self.root,
                &[
                    "-p",
                    compose_project(),
                    "up",
                    "--build",
                    "-d",
                    "enrollment-target",
                ],
            );
            assert!(
                partial.status.success(),
                "partial Compose setup failed: {}\n{}",
                output_text(&partial),
                compose_diagnostics(&self.root)
            );
            let failed = compose(
                &self.root,
                &[
                    "-p",
                    compose_project(),
                    "up",
                    "--build",
                    "-d",
                    "enrollment-target",
                    "missing-induced-failure-service",
                ],
            );
            assert!(
                !failed.status.success(),
                "induced Compose failure unexpectedly passed: {}\n{}",
                output_text(&failed),
                compose_diagnostics(&self.root)
            );
            panic!(
                "induced partial-up failure\n{}",
                compose_diagnostics(&self.root)
            );
        }
        let output = compose(
            &self.root,
            &[
                "-p",
                compose_project(),
                "up",
                "--build",
                "-d",
                "enrollment-target",
                "enrollment-candidate",
            ],
        );
        assert!(
            output.status.success(),
            "docker compose up failed: {}\n{}",
            output_text(&output),
            compose_diagnostics(&self.root)
        );
    }

    fn finalize(mut self) {
        if let Err(error) = cleanup(&self.root) {
            panic!("discovery Docker cleanup failed: {error}");
        }
        self.finalized = true;
    }
}

fn generate_auth(directory: &Path, id: &str) -> (PathBuf, PathBuf) {
    let generated = bounded_command(env!("CARGO_BIN_EXE_omakure"))
        .args([
            "--json",
            "token",
            "generate",
            "--id",
            id,
            "--scope",
            "node:read",
            "--scope",
            "discovery:read",
            "--scope",
            "enrollment:read",
            "--scope",
            "enrollment:write",
        ])
        .output()
        .expect("generate discovery token");
    if !generated.status.success() {
        panic!(
            "token generation failed: status={} stderr={}",
            generated.status,
            safe_generation_stderr(&generated)
        );
    }
    let generated: Value = serde_json::from_slice(&generated.stdout).expect("token JSON");
    let token = generated["data"]["token"]
        .as_str()
        .expect("generated plaintext token")
        .to_string();
    let entry = generated["data"]["tokens_file_entry"]
        .as_str()
        .expect("generated token entry");
    let tokens_path = directory.join(format!("{id}.tokens.toml"));
    let client_path = directory.join(format!("{id}.client.token"));
    fs::write(&tokens_path, format!("version = 1\n\n{entry}")).expect("write discovery token file");
    fs::write(&client_path, token).expect("write discovery client token");
    (tokens_path, client_path)
}

impl Drop for ComposeGuard {
    fn drop(&mut self) {
        if !self.finalized {
            eprintln!(
                "discovery Docker failure diagnostics:\n{}",
                compose_diagnostics(&self.root)
            );
            if let Err(error) = cleanup(&self.root) {
                eprintln!("discovery Docker cleanup after panic failed: {error}");
            }
        }
    }
}

fn cleanup(root: &Path) -> Result<(), String> {
    let mut failures = Vec::new();
    let down = compose(
        root,
        &[
            "-p",
            compose_project(),
            "down",
            "--volumes",
            "--remove-orphans",
        ],
    );
    if !down.status.success() {
        failures.push(format!(
            "compose down status={} stderr={}",
            down.status,
            safe_stderr(&down)
        ));
    }
    for resource in ["container", "network", "volume"] {
        let output = bounded_command("docker")
            .args([
                resource,
                "ls",
                "-q",
                "--filter",
                &format!("label=com.docker.compose.project={}", compose_project()),
            ])
            .output();
        match output {
            Ok(output) if !output.status.success() => failures.push(format!(
                "inspect {resource} status={} stderr={}",
                output.status,
                safe_stderr(&output)
            )),
            Ok(output) if !String::from_utf8_lossy(&output.stdout).trim().is_empty() => {
                failures.push(format!("project-labeled {resource} remains"));
            }
            Ok(_) => {}
            Err(error) => failures.push(format!("inspect {resource}: {error}")),
        }
    }
    cleanup_result(failures)
}

fn cleanup_result(failures: Vec<String>) -> Result<(), String> {
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::cleanup_result;

    #[test]
    fn cleanup_reports_all_failures() {
        let error = cleanup_result(vec!["down failed".into(), "volume remains".into()])
            .expect_err("cleanup failure should be returned");
        assert!(error.contains("down failed"));
        assert!(error.contains("volume remains"));
    }
}

#[test]
#[ignore]
fn cleanup_after_induced_partial_up() {
    std::env::set_var("OMAKURE_E2E_INDUCE_PARTIAL_UP", "1");
    let result = std::panic::catch_unwind(ComposeGuard::new);
    std::env::remove_var("OMAKURE_E2E_INDUCE_PARTIAL_UP");
    assert!(result.is_err(), "induced partial-up should fail");
    cleanup(&PathBuf::from(env!("CARGO_MANIFEST_DIR")))
        .expect("induced partial-up cleanup should leave no resources");
}

fn safe_stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

fn safe_generation_stderr(output: &Output) -> String {
    let stderr = safe_stderr(output);
    let lower = stderr.to_ascii_lowercase();
    assert!(
        !lower.contains("bearer ") && !lower.contains("$argon2") && !lower.contains("token ="),
        "token generation stderr contained sensitive material"
    );
    stderr
}

fn compose(root: &Path, args: &[&str]) -> Output {
    bounded_command_within("docker", compose_timeout(args))
        .current_dir(root)
        .args(["compose"])
        .args(args)
        .output()
        .expect("run docker compose")
}
fn compose_diagnostics(root: &Path) -> String {
    let ps = compose(root, &["-p", compose_project(), "ps", "-a"]);
    let logs = compose(
        root,
        &[
            "-p",
            compose_project(),
            "logs",
            "--no-color",
            "--tail",
            "200",
            "enrollment-target",
            "enrollment-candidate",
        ],
    );
    format!(
        "compose ps:\n{}\ncompose logs:\n{}",
        output_text(&ps),
        output_text(&logs)
    )
}

fn exec(service: &str, args: &[&str]) -> Output {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    bounded_command("docker")
        .current_dir(root)
        .args(["compose", "-p", compose_project(), "exec", "-T", service])
        .args(args)
        .output()
        .expect("run docker compose exec")
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json_output(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "expected successful command: {}",
        output_text(output)
    );
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("invalid JSON output ({error}): {}", output_text(output)))
}

fn container_ip(service: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let id = compose(&root, &["-p", compose_project(), "ps", "-q", service]);
    assert!(
        id.status.success(),
        "cannot locate {service}: {}",
        output_text(&id)
    );
    let id = String::from_utf8_lossy(&id.stdout).trim().to_string();
    let output = bounded_command("docker")
        .args([
            "inspect",
            "-f",
            "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
            &id,
        ])
        .output()
        .expect("inspect discovery container");
    assert!(
        output.status.success(),
        "inspect failed: {}",
        output_text(&output)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn compose_service_healthy(service: &str) -> bool {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let id = compose(&root, &["-p", compose_project(), "ps", "-q", service]);
    if !id.status.success() {
        return false;
    }
    let id = String::from_utf8_lossy(&id.stdout).trim().to_string();
    if id.is_empty() {
        return false;
    }
    let health = bounded_command("docker")
        .args([
            "inspect",
            "-f",
            "{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}",
            &id,
        ])
        .output();
    health
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "healthy")
}

fn readiness_http_ready(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/v1/ready");
    request_json("GET", &url, false, None)
        .is_ok_and(|value| value["ok"] == true && value["data"]["status"] == "ready")
}

fn authenticated_node_status_ready(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/v1/node/status");
    request_json("GET", &url, true, None).is_ok_and(|value| {
        value["ok"] == true
            && value["data"]["identity"]["node_id"].is_string()
            && value["data"]["trust"]["active_peer_count"].is_number()
    })
}

fn service_http_ready(service: &str, port: u16) -> bool {
    compose_service_healthy(service)
        && readiness_http_ready(port)
        && authenticated_node_status_ready(port)
}

fn wait_for_service_ready(service: &str, port: u16) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if service_http_ready(service, port) {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    panic!(
        "Docker service {service} did not become semantically ready:\n{}",
        compose_diagnostics(&root)
    );
}

fn wait_for_health() {
    wait_for_service_ready("enrollment-target", 17878);
    wait_for_service_ready("enrollment-candidate", 17879);
}

fn wait_for_stopped(service: &str) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let output = compose(
            &root,
            &[
                "-p",
                compose_project(),
                "ps",
                "--status",
                "running",
                "-q",
                service,
            ],
        );
        if output.status.success() && String::from_utf8_lossy(&output.stdout).trim().is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "Docker service did not stop: {service}\n{}",
        compose_diagnostics(&root)
    );
}

fn curl(method: &str, url: &str, include_addresses: bool) -> Value {
    curl_json(method, url, include_addresses, None)
}

fn request_json(
    method: &str,
    url: &str,
    authenticated: bool,
    body: Option<&str>,
) -> Result<Value, String> {
    let header_file = if authenticated {
        let client_file = if url.contains(":17879/") {
            std::env::var_os("OMAKURE_ENROLLMENT_CANDIDATE_CLIENT_FILE")
                .ok_or_else(|| "candidate client file is unset".to_string())?
        } else {
            std::env::var_os("OMAKURE_ENROLLMENT_TARGET_CLIENT_FILE")
                .ok_or_else(|| "target client file is unset".to_string())?
        };
        let token = fs::read_to_string(client_file).map_err(|error| error.to_string())?;
        let file = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
        fs::write(file.path(), format!("Authorization: Bearer {token}\n"))
            .map_err(|error| error.to_string())?;
        Some(file)
    } else {
        None
    };
    let mut command = bounded_command("curl");
    command.args([
        "--fail",
        "--silent",
        "--show-error",
        "--connect-timeout",
        "3",
        "--max-time",
        "10",
        "-X",
        method,
        url,
    ]);
    if let Some(header_file) = &header_file {
        let header_arg = format!("@{}", header_file.path().display());
        command.args(["-H", &header_arg]);
    }
    if let Some(body) = body {
        command.args(["-H", "Content-Type: application/json", "--data", body]);
    }
    let output = command.output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(output_text(&output));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid JSON ({error}): {}", output_text(&output)))
}

fn curl_json(method: &str, url: &str, include_addresses: bool, body: Option<&str>) -> Value {
    let value = request_json(method, url, true, body)
        .unwrap_or_else(|error| panic!("curl failed: {error}"));
    if include_addresses {
        assert!(value["data"]["candidates"]
            .as_array()
            .is_some_and(|candidates| candidates
                .iter()
                .all(|candidate| candidate["address"].is_string())));
    }
    value
}

fn copy_state(service: &str, destination: &Path) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let destination = destination.to_str().expect("temporary path is UTF-8");
    let output = bounded_command("docker")
        .current_dir(root)
        .args([
            "compose",
            "-p",
            compose_project(),
            "cp",
            &format!("{service}:/var/lib/omakure/node.sqlite"),
            destination,
        ])
        .output()
        .expect("copy discovery state");
    assert!(
        output.status.success(),
        "docker cp failed: {}",
        output_text(&output)
    );
}

fn active_registry_counts(path: &Path) -> (i64, i64, i64, i64, i64) {
    let connection = Connection::open(path).expect("open copied discovery registry");
    let count = |sql: &str| {
        connection
            .query_row(sql, [], |row| row.get::<_, i64>(0))
            .expect("query discovery registry count")
    };
    (
        count("SELECT COUNT(*) FROM remote_identities WHERE state = 'active'"),
        count("SELECT COUNT(*) FROM trusted_peers WHERE state = 'active'"),
        count("SELECT COUNT(*) FROM transport_key_epochs WHERE state = 'active'"),
        count("SELECT COUNT(*) FROM manual_enrollment_requests WHERE state = 'pending'"),
        count("SELECT COUNT(*) FROM channel_sessions WHERE state = 'active'"),
    )
}

fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap())
        .collect()
}

fn copy_from_container(service: &str, source: &str, destination: &Path) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let destination = destination.to_str().expect("temporary path is UTF-8");
    let output = bounded_command("docker")
        .current_dir(root)
        .args([
            "compose",
            "-p",
            compose_project(),
            "cp",
            &format!("{service}:{source}"),
            destination,
        ])
        .output()
        .expect("copy discovery state");
    assert!(
        output.status.success(),
        "docker cp failed: {}",
        output_text(&output)
    );
}

#[test]
#[ignore = "requires Docker/Linux bridge broadcast or multicast"]
fn docker_discovery_finds_nodes_without_creating_trust_or_sessions() {
    let compose_guard = ComposeGuard::new();
    wait_for_health();
    let candidate_status = json_output(&exec(
        "enrollment-candidate",
        &["omakure", "--json", "node", "status"],
    ));
    let candidate_node_id = candidate_status["data"]["identity"]["node_id"]
        .as_str()
        .expect("candidate node id")
        .to_string();
    let target_status = json_output(&exec(
        "enrollment-target",
        &["omakure", "--json", "node", "status"],
    ));
    let target_node_id = target_status["data"]["identity"]["node_id"]
        .as_str()
        .expect("target node id")
        .to_string();
    assert_eq!(target_status["data"]["trust"]["active_peer_count"], 0);

    let deadline = Instant::now() + Duration::from_secs(15);
    let discovered = loop {
        let output = curl(
            "GET",
            &format!("{TARGET_API}/v1/node/discovery?include_addresses=true"),
            true,
        );
        if output["data"]["candidates"]
            .as_array()
            .is_some_and(|candidates| candidates.iter().any(|c| c["node_id"] == candidate_node_id))
        {
            break output;
        }
        assert!(
            Instant::now() < deadline,
            "candidate was not discovered: {output}"
        );
        std::thread::sleep(Duration::from_millis(250));
    };
    assert_eq!(discovered["data"]["secret_configured"], false);

    let status = curl("GET", &format!("{TARGET_API}/v1/node/status"), false);
    assert!(status["data"]["discovery"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .all(|candidate| candidate["address"].is_null()));

    let target_ip = container_ip("enrollment-target");
    let blocked = exec(
        "enrollment-candidate",
        &[
            "omakure",
            "--json",
            "node",
            "direct-probe",
            "--endpoint",
            &format!("{target_ip}:7988"),
            "--peer-node-id",
            &target_node_id,
        ],
    );
    assert!(
        !blocked.status.success(),
        "discovery authorized a direct probe"
    );

    for service in ["enrollment-target", "enrollment-candidate"] {
        let dir = TempDir::new().expect("discovery state snapshot directory");
        let path = dir.path().join("node.sqlite");
        copy_state(service, &path);
        assert_eq!(
            active_registry_counts(&path),
            (0, 0, 0, 0, 0),
            "service: {service}"
        );
    }

    let target_endpoint = format!("{}:7988", container_ip("enrollment-target"));
    let request_output = exec(
        "enrollment-candidate",
        &[
            "omakure",
            "--json",
            "node",
            "enroll",
            "request",
            "--endpoint",
            &target_endpoint,
            "--capability",
            "remote-run",
            "--lifetime-seconds",
            "300",
        ],
    );
    assert!(
        request_output.status.success(),
        "manual enrollment request failed: {}",
        output_text(&request_output)
    );
    let request = json_output(&request_output);
    let pending = curl("GET", &format!("{TARGET_API}/v1/node/enrollments"), false);
    assert_eq!(pending["data"].as_array().unwrap().len(), 1);
    let pending_node_id = pending["data"][0]["node_id"].as_str().unwrap();
    let request_hex = request["data"]["request_hex"].as_str().unwrap();
    let request_bytes = decode_hex(request_hex);
    assert_eq!(request_bytes[4], 2);
    let certificate_dir = TempDir::new().expect("candidate certificate directory");
    let certificate_file = certificate_dir.path().join("transport.cert");
    copy_from_container(
        "enrollment-candidate",
        "/var/lib/omakure/transport.cert",
        &certificate_file,
    );
    let approval_body = serde_json::json!({
        "request_hex": request_hex,
        "transport_certificate": lower_hex(&fs::read(&certificate_file).unwrap()),
        "code": request["data"]["code"].as_str().unwrap(),
        "actor": "discovery-e2e",
        "reason": "explicit discovery follow-up enrollment",
        "confirmed": true
    });
    let approved = curl_json(
        "POST",
        &format!("{TARGET_API}/v1/node/enrollments/{pending_node_id}/approve"),
        false,
        Some(&approval_body.to_string()),
    );
    assert_eq!(approved["data"]["state"], "active");

    let target_certificate_dir = TempDir::new().expect("target certificate directory");
    let target_certificate_file = target_certificate_dir.path().join("transport.cert");
    copy_from_container(
        "enrollment-target",
        "/var/lib/omakure/transport.cert",
        &target_certificate_file,
    );
    let reciprocal_body = serde_json::json!({
        "request_hex": request["data"]["reciprocal_request_hex"].as_str().unwrap(),
        "transport_certificate": lower_hex(&fs::read(&target_certificate_file).unwrap()),
        "code": request["data"]["reciprocal_code"].as_str().unwrap(),
        "actor": "discovery-e2e",
        "reason": "explicit reciprocal discovery follow-up enrollment",
        "confirmed": true
    });
    let reciprocal = curl_json(
        "POST",
        &format!("http://127.0.0.1:17879/v1/node/enrollments/{target_node_id}/approve"),
        false,
        Some(&reciprocal_body.to_string()),
    );
    assert_eq!(reciprocal["data"]["state"], "active");
    let accepted = exec(
        "enrollment-candidate",
        &[
            "omakure",
            "--json",
            "node",
            "direct-probe",
            "--endpoint",
            &target_endpoint,
            "--peer-node-id",
            &target_node_id,
        ],
    );
    assert_eq!(json_output(&accepted)["data"]["accepted"], true);

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert!(compose(
        &root,
        &["-p", compose_project(), "stop", "enrollment-candidate"]
    )
    .status
    .success());
    wait_for_stopped("enrollment-candidate");
    assert!(compose(
        &root,
        &["-p", compose_project(), "stop", "enrollment-target"]
    )
    .status
    .success());
    wait_for_stopped("enrollment-target");
    assert!(compose(
        &root,
        &[
            "-p",
            compose_project(),
            "up",
            "-d",
            "--no-deps",
            "enrollment-target"
        ]
    )
    .status
    .success());
    wait_for_service_ready("enrollment-target", 17878);
    let empty_after_restart = curl("GET", &format!("{TARGET_API}/v1/node/discovery"), false);
    assert!(
        empty_after_restart["data"]["candidates"]
            .as_array()
            .unwrap()
            .is_empty(),
        "observer retained or rediscovered candidates before sender restart: {empty_after_restart}"
    );

    assert!(compose(
        &root,
        &["-p", compose_project(), "start", "enrollment-candidate"]
    )
    .status
    .success());
    wait_for_health();
    let restart_deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let output = curl("GET", &format!("{TARGET_API}/v1/node/discovery"), false);
        if output["data"]["candidates"]
            .as_array()
            .is_some_and(|candidates| {
                candidates
                    .iter()
                    .any(|candidate| candidate["node_id"] == candidate_node_id)
            })
        {
            break;
        }
        assert!(
            Instant::now() < restart_deadline,
            "candidate was not rediscovered: {output}"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
    let accepted_after_restart = exec(
        "enrollment-candidate",
        &[
            "omakure",
            "--json",
            "node",
            "direct-probe",
            "--endpoint",
            &target_endpoint,
            "--peer-node-id",
            &target_node_id,
        ],
    );
    assert_eq!(
        json_output(&accepted_after_restart)["data"]["accepted"],
        true
    );
    compose_guard.finalize();
}

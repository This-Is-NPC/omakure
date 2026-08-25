//! Opt-in Docker acceptance test for the complete manual enrollment path.
//!
//! Run with:
//! `cargo test --test docker_enrollment_e2e -- --ignored --nocapture`

use rusqlite::Connection;
use serde_json::Value;
use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const TARGET_API: &str = "http://127.0.0.1:17878";

fn compose_project() -> &'static str {
    static PROJECT: OnceLock<String> = OnceLock::new();
    PROJECT
        .get_or_init(|| format!("omakure-enrollment-{}", std::process::id()))
        .as_str()
}

fn bounded_command(program: &str) -> Command {
    let mut command = Command::new("timeout");
    command.args(["--foreground", "--kill-after=10s", "120s", program]);
    command
}

struct ComposeGuard {
    root: PathBuf,
    _tokens_dir: TempDir,
    finalized: bool,
}

impl ComposeGuard {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let tokens_dir = TempDir::new().expect("create ephemeral enrollment token directory");
        let (target_tokens, target_client) = generate_auth(tokens_dir.path(), "enrollment-target");
        let (candidate_tokens, candidate_client) =
            generate_auth(tokens_dir.path(), "enrollment-candidate");
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
            let partial = compose(&self.root, &["up", "--build", "-d", "enrollment-target"]);
            assert!(partial.status.success(), "partial Compose setup failed");
            let failed = compose(
                &self.root,
                &[
                    "up",
                    "--build",
                    "-d",
                    "enrollment-target",
                    "missing-induced-failure-service",
                ],
            );
            assert!(
                !failed.status.success(),
                "induced Compose failure unexpectedly passed"
            );
            panic!("induced partial-up failure");
        }
        let output = compose(
            &self.root,
            &[
                "up",
                "--build",
                "-d",
                "enrollment-target",
                "enrollment-candidate",
            ],
        );
        assert!(
            output.status.success(),
            "docker compose up failed: {}",
            output_text(&output)
        );
    }

    fn finalize(mut self) {
        if let Err(error) = cleanup(&self.root) {
            panic!("enrollment Docker cleanup failed: {error}");
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
            "enrollment:read",
            "--scope",
            "enrollment:write",
            "--scope",
            "discovery:read",
        ])
        .output()
        .expect("generate ephemeral enrollment token");
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
        .expect("generated plaintext token");
    let entry = generated["data"]["tokens_file_entry"]
        .as_str()
        .expect("generated tokens file entry");
    let tokens_path = directory.join(format!("{id}.tokens.toml"));
    let client_path = directory.join(format!("{id}.client.token"));
    fs::write(&tokens_path, format!("version = 1\n\n{entry}"))
        .expect("write ephemeral enrollment tokens file");
    fs::write(&client_path, token).expect("write ephemeral enrollment client token");
    (tokens_path, client_path)
}

impl Drop for ComposeGuard {
    fn drop(&mut self) {
        if !self.finalized {
            if let Err(error) = cleanup(&self.root) {
                eprintln!("enrollment Docker cleanup after panic failed: {error}");
            }
        }
    }
}

fn cleanup(root: &Path) -> Result<(), String> {
    let mut failures = Vec::new();
    let down = compose(root, &["down", "--volumes", "--remove-orphans"]);
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
        let error = cleanup_result(vec!["down failed".into(), "network remains".into()])
            .expect_err("cleanup failure should be returned");
        assert!(error.contains("down failed"));
        assert!(error.contains("network remains"));
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
    bounded_command("docker")
        .current_dir(root)
        .args(["compose", "-p", compose_project()])
        .args(args)
        .output()
        .expect("run docker compose")
}

fn exec(service: &str, args: &[&str]) -> Output {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut command = bounded_command("docker");
    command
        .current_dir(root)
        .args(["compose", "-p", compose_project(), "exec", "-T", service])
        .args(args);
    command.output().expect("run docker compose exec")
}

fn container_ip(service: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let id = compose(&root, &["ps", "-q", service]);
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
        .expect("inspect Docker container");
    assert!(
        output.status.success(),
        "cannot inspect {service}: {}",
        output_text(&output)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
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

fn health(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).is_ok()
}

fn wait_for_health() {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if health(17878)
            && health(17879)
            && health(17988)
            && health(17989)
            && transport_ready("enrollment-target")
            && transport_ready("enrollment-candidate")
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("Docker enrollment services did not expose their API ports");
}

fn transport_ready(service: &str) -> bool {
    let output = exec(
        service,
        &[
            "test",
            "-s",
            "/var/lib/omakure/transport.key",
            "-a",
            "-s",
            "/var/lib/omakure/transport.cert",
        ],
    );
    output.status.success()
}

fn curl(method: &str, url: &str, body: Option<&str>) -> Value {
    let mut args = vec![
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
    ];
    let body_arg = body.map(str::to_string);
    let client_file = if url.contains(":17879/") {
        std::env::var_os("OMAKURE_ENROLLMENT_CANDIDATE_CLIENT_FILE").expect("candidate client file")
    } else {
        std::env::var_os("OMAKURE_ENROLLMENT_TARGET_CLIENT_FILE").expect("target client file")
    };
    let token = fs::read_to_string(client_file).expect("read protected enrollment client token");
    let header_file = tempfile::NamedTempFile::new().expect("create curl header file");
    fs::write(
        header_file.path(),
        format!("Authorization: Bearer {token}\n"),
    )
    .expect("write curl header file");
    let header_arg = format!("@{}", header_file.path().display());
    if let Some(body) = body_arg.as_deref() {
        args.extend(["-H", "Content-Type: application/json", "-H", &header_arg]);
        args.extend(["--data", body]);
    } else {
        args.extend(["-H", &header_arg]);
    }
    let output = bounded_command("curl")
        .args(args)
        .output()
        .expect("run curl");
    assert!(
        output.status.success(),
        "curl failed: {}",
        output_text(&output)
    );
    serde_json::from_slice(&output.stdout).expect("curl returned JSON")
}

fn lower_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0);
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
        .expect("copy Docker state");
    assert!(
        output.status.success(),
        "docker compose cp failed: {}",
        output_text(&output)
    );
}

struct RegistrySnapshot {
    peers: Vec<String>,
    trusted: Vec<String>,
    audits: Vec<String>,
    enrollment_audits: Vec<String>,
    active_identities: i64,
    active_trusted_peers: i64,
    active_transport_epochs: i64,
    pending_manual_requests: i64,
}

fn registry_snapshot(path: &Path) -> RegistrySnapshot {
    let connection = Connection::open(path).expect("open copied target registry");
    let peers = connection
        .prepare("SELECT node_id || '|' || public_key || '|' || state FROM peers ORDER BY node_id")
        .expect("prepare peers")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query peers")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect peers");
    let trusted = connection
        .prepare(
            "SELECT r.node_id || '|' || hex(r.identity_key) || '|' || r.state || '|' || COALESCE(p.state, '')
             FROM remote_identities r
             LEFT JOIN trusted_peers p ON p.node_id = r.node_id
             ORDER BY r.node_id",
        )
        .expect("prepare v2 identities")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query v2 identities")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect v2 identities");
    let audits = connection
        .prepare(
            "SELECT event_type || '|' || node_id || '|' || COALESCE(actor, '') || '|' || COALESCE(reason, '')
             FROM audit_events ORDER BY id",
        )
        .expect("prepare audit events")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query audit events")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect audit events");
    let enrollment_audits = connection
        .prepare(
            "SELECT event_code || '|' || node_id || '|' || outcome
             FROM enrollment_audits ORDER BY id",
        )
        .expect("prepare enrollment audits")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query enrollment audits")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect enrollment audits");
    let active_identities = connection
        .query_row(
            "SELECT COUNT(*) FROM remote_identities WHERE state = 'active'",
            [],
            |row| row.get(0),
        )
        .expect("count active identities");
    let active_trusted_peers = connection
        .query_row(
            "SELECT COUNT(*) FROM trusted_peers WHERE state = 'active'",
            [],
            |row| row.get(0),
        )
        .expect("count active trusted peers");
    let active_transport_epochs = connection
        .query_row(
            "SELECT COUNT(*) FROM transport_key_epochs WHERE state = 'active'",
            [],
            |row| row.get(0),
        )
        .expect("count active transport epochs");
    let pending_manual_requests = connection
        .query_row(
            "SELECT COUNT(*) FROM manual_enrollment_requests
             WHERE source = 'manual' AND state = 'pending'",
            [],
            |row| row.get(0),
        )
        .expect("count pending manual requests");
    RegistrySnapshot {
        peers,
        trusted,
        audits,
        enrollment_audits,
        active_identities,
        active_trusted_peers,
        active_transport_epochs,
        pending_manual_requests,
    }
}

#[test]
#[ignore = "requires Docker and intentionally runs the full two-container transaction"]
fn docker_manual_enrollment_is_pending_blocked_approved_and_restart_stable() {
    let compose_guard = ComposeGuard::new();
    wait_for_health();
    let target_endpoint = format!("{}:7988", container_ip("enrollment-target"));

    let target_status_output = exec(
        "enrollment-target",
        &["omakure", "--json", "node", "status"],
    );
    assert!(
        target_status_output.status.success(),
        "target status failed: {}",
        output_text(&target_status_output)
    );
    let target_status = json_output(&target_status_output);
    let target_node_id = target_status["data"]["identity"]["node_id"]
        .as_str()
        .expect("target node ID")
        .to_string();

    let blocked = exec(
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
    assert!(
        !blocked.status.success(),
        "untrusted candidate probe succeeded"
    );

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
    if !request_output.status.success() {
        let pending = curl("GET", &format!("{TARGET_API}/v1/node/enrollments"), None);
        panic!(
            "manual enrollment request failed: {}; target pending={pending}",
            output_text(&request_output)
        );
    }
    let request = json_output(&request_output);
    assert_eq!(
        request["ok"],
        true,
        "manual enrollment request returned an error: {}",
        output_text(&request_output)
    );
    assert_eq!(request["data"]["state"], "pending");

    let pending = curl("GET", &format!("{TARGET_API}/v1/node/enrollments"), None);
    assert_eq!(pending["ok"], true);
    assert_eq!(pending["data"].as_array().unwrap().len(), 1);
    let pending_node_id = pending["data"][0]["node_id"].as_str().unwrap();
    assert_eq!(pending_node_id.len(), 69);

    let reciprocal_request = request["data"]["reciprocal_request_hex"]
        .as_str()
        .expect("reciprocal request");
    let reciprocal_code = request["data"]["reciprocal_code"]
        .as_str()
        .expect("reciprocal code");
    let request_bytes = decode_hex(request["data"]["request_hex"].as_str().unwrap());
    let reciprocal_bytes = decode_hex(reciprocal_request);
    assert_eq!(request_bytes[4], 2);
    assert_eq!(reciprocal_bytes[4], 2);
    assert_eq!(&request_bytes[5..21], &reciprocal_bytes[5..21]);
    assert!(request_bytes[5..21].iter().any(|byte| *byte != 0));
    assert_ne!(&request_bytes[21..37], &reciprocal_bytes[21..37]);

    let candidate_pending = curl("GET", "http://127.0.0.1:17879/v1/node/enrollments", None);
    assert_eq!(candidate_pending["ok"], true);
    assert_eq!(candidate_pending["data"].as_array().unwrap().len(), 1);
    assert_eq!(candidate_pending["data"][0]["node_id"], target_node_id);

    let _ = compose(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        &["stop", "enrollment-target", "enrollment-candidate"],
    );
    let preapproval_dir = TempDir::new().unwrap();
    let target_preapproval_db = preapproval_dir.path().join("target-node.sqlite");
    let candidate_preapproval_db = preapproval_dir.path().join("candidate-node.sqlite");
    copy_from_container(
        "enrollment-target",
        "/var/lib/omakure/node.sqlite",
        &target_preapproval_db,
    );
    copy_from_container(
        "enrollment-candidate",
        "/var/lib/omakure/node.sqlite",
        &candidate_preapproval_db,
    );
    for snapshot in [
        registry_snapshot(&target_preapproval_db),
        registry_snapshot(&candidate_preapproval_db),
    ] {
        assert_eq!(snapshot.active_identities, 0);
        assert_eq!(snapshot.active_trusted_peers, 0);
        assert_eq!(snapshot.active_transport_epochs, 0);
        assert_eq!(snapshot.pending_manual_requests, 1);
    }
    let restarted_before_approval = compose(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        &["start", "enrollment-target", "enrollment-candidate"],
    );
    assert!(
        restarted_before_approval.status.success(),
        "restart before approval failed: {}",
        output_text(&restarted_before_approval)
    );
    wait_for_health();
    let target_endpoint = format!("{}:7988", container_ip("enrollment-target"));

    let still_blocked = exec(
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
    assert!(
        !still_blocked.status.success(),
        "pending candidate probe succeeded"
    );

    let certificate_path = TempDir::new().unwrap();
    let certificate_file = certificate_path.path().join("transport.cert");
    copy_from_container(
        "enrollment-candidate",
        "/var/lib/omakure/transport.cert",
        &certificate_file,
    );
    let certificate = lower_hex(&fs::read(&certificate_file).unwrap());
    let approval_body = serde_json::json!({
        "request_hex": request["data"]["request_hex"].as_str().unwrap(),
        "transport_certificate": certificate,
        "code": request["data"]["code"].as_str().unwrap(),
        "actor": "docker-e2e",
        "reason": "verified candidate identity and transport certificate",
        "confirmed": true
    });
    let approved = curl(
        "POST",
        &format!("{TARGET_API}/v1/node/enrollments/{pending_node_id}/approve"),
        Some(&approval_body.to_string()),
    );
    assert_eq!(approved["ok"], true);
    assert_eq!(approved["data"]["state"], "active");

    let candidate_still_pending = curl("GET", "http://127.0.0.1:17879/v1/node/enrollments", None);
    assert_eq!(candidate_still_pending["data"].as_array().unwrap().len(), 1);
    let candidate_one_direction = curl("GET", "http://127.0.0.1:17879/v1/node/peers", None);
    assert_eq!(candidate_one_direction["data"][0]["state"], "pending");
    let target_one_direction = curl("GET", &format!("{TARGET_API}/v1/node/peers"), None);
    assert_eq!(target_one_direction["data"][0]["state"], "active");
    let target_certificate_path = TempDir::new().unwrap();
    let target_certificate_file = target_certificate_path.path().join("transport.cert");
    copy_from_container(
        "enrollment-target",
        "/var/lib/omakure/transport.cert",
        &target_certificate_file,
    );
    let target_certificate = lower_hex(&fs::read(&target_certificate_file).unwrap());
    let candidate_approval_body = serde_json::json!({
        "request_hex": reciprocal_request,
        "transport_certificate": target_certificate,
        "code": reciprocal_code,
        "actor": "docker-e2e",
        "reason": "verified target identity and transport certificate",
        "confirmed": true
    });
    let candidate_approved = curl(
        "POST",
        &format!("http://127.0.0.1:17879/v1/node/enrollments/{target_node_id}/approve"),
        Some(&candidate_approval_body.to_string()),
    );
    assert_eq!(candidate_approved["ok"], true);
    assert_eq!(candidate_approved["data"]["state"], "active");
    let candidate_both_directions = curl("GET", "http://127.0.0.1:17879/v1/node/peers", None);
    assert_eq!(candidate_both_directions["data"][0]["state"], "active");
    let active = curl("GET", &format!("{TARGET_API}/v1/node/peers"), None);
    assert_eq!(active["data"][0]["node_id"], pending_node_id);
    assert_eq!(active["data"][0]["state"], "active");

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

    let before_restart = curl("GET", &format!("{TARGET_API}/v1/node/status"), None);
    let target_db = TempDir::new().unwrap();
    let target_db_path = target_db.path().join("node.sqlite");
    let _ = compose(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        &["stop", "enrollment-target"],
    );
    copy_from_container(
        "enrollment-target",
        "/var/lib/omakure/node.sqlite",
        &target_db_path,
    );
    let before_registry = registry_snapshot(&target_db_path);
    assert!(before_registry
        .peers
        .iter()
        .any(|peer| peer.contains("|active")));
    assert!(before_registry
        .trusted
        .iter()
        .any(|peer| peer.contains("|active|active")));
    assert!(before_registry
        .audits
        .iter()
        .any(|event| event.starts_with("enrollment_pending|")));
    assert!(before_registry
        .audits
        .iter()
        .any(|event| event.starts_with("enrollment_approved|") && event.contains("docker-e2e")));
    assert!(
        before_registry
            .enrollment_audits
            .iter()
            .filter(|event| event.ends_with("|approved"))
            .count()
            >= 1
    );

    let restarted = compose(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        &["start", "enrollment-target", "enrollment-candidate"],
    );
    assert!(
        restarted.status.success(),
        "restart failed: {}",
        output_text(&restarted)
    );
    wait_for_health();
    let after_restart = curl("GET", &format!("{TARGET_API}/v1/node/status"), None);
    assert_eq!(
        after_restart["data"]["identity"]["node_id"],
        before_restart["data"]["identity"]["node_id"]
    );
    let active_after_restart = curl("GET", &format!("{TARGET_API}/v1/node/peers"), None);
    assert_eq!(active_after_restart["data"][0]["state"], "active");
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

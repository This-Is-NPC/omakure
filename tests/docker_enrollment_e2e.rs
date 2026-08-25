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
use std::time::{Duration, Instant};
use tempfile::TempDir;

const COMPOSE_PROJECT: &str = "omakure";
const TARGET_API: &str = "http://127.0.0.1:17878";

struct ComposeGuard {
    root: PathBuf,
    _tokens_dir: TempDir,
}

impl ComposeGuard {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let tokens_dir = TempDir::new().expect("create ephemeral enrollment token directory");
        let tokens_path = tokens_dir.path().join("tokens.toml");
        let generated = Command::new(env!("CARGO_BIN_EXE_omakure"))
            .args([
                "--json",
                "token",
                "generate",
                "--id",
                "enrollment-e2e",
                "--scope",
                "node:read",
                "--scope",
                "enrollment:read",
                "--scope",
                "enrollment:write",
            ])
            .output()
            .expect("generate ephemeral enrollment token");
        assert!(
            generated.status.success(),
            "token generation failed: {}",
            output_text(&generated)
        );
        let generated: Value = serde_json::from_slice(&generated.stdout).expect("token JSON");
        let token = generated["data"]["token"]
            .as_str()
            .expect("generated plaintext token")
            .to_string();
        let entry = generated["data"]["tokens_file_entry"]
            .as_str()
            .expect("generated tokens file entry");
        fs::write(&tokens_path, format!("version = 1\n\n{entry}"))
            .expect("write ephemeral enrollment tokens file");
        std::env::set_var("OMAKURE_ENROLLMENT_TOKENS_FILE", &tokens_path);
        std::env::set_var("OMAKURE_ENROLLMENT_E2E_TOKEN", token);
        compose(&root, &["down", "-v"]);
        let output = compose(
            &root,
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
        Self {
            root,
            _tokens_dir: tokens_dir,
        }
    }
}

impl Drop for ComposeGuard {
    fn drop(&mut self) {
        let _ = compose(&self.root, &["down", "-v"]);
    }
}

fn compose(root: &Path, args: &[&str]) -> Output {
    Command::new("docker")
        .current_dir(root)
        .args(["compose", "-p", COMPOSE_PROJECT])
        .args(args)
        .output()
        .expect("run docker compose")
}

fn exec(service: &str, args: &[&str]) -> Output {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut command = Command::new("docker");
    command
        .current_dir(root)
        .args(["compose", "-p", COMPOSE_PROJECT, "exec", "-T", service])
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
    let output = Command::new("docker")
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
    let mut args = vec!["--fail", "--silent", "--show-error", "-X", method, url];
    let body_arg = body.map(str::to_string);
    let token = std::env::var("OMAKURE_ENROLLMENT_E2E_TOKEN")
        .expect("ephemeral enrollment token is configured");
    let auth_header = format!("Authorization: Bearer {token}");
    if let Some(body) = body_arg.as_deref() {
        args.extend(["-H", "Content-Type: application/json", "-H"]);
        args.push(&auth_header);
        args.extend(["--data", body]);
    } else {
        args.extend(["-H", &auth_header]);
    }
    let output = Command::new("curl").args(args).output().expect("run curl");
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
    let output = Command::new("docker")
        .current_dir(root)
        .args([
            "compose",
            "-p",
            COMPOSE_PROJECT,
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
    let _compose = ComposeGuard::new();
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
}

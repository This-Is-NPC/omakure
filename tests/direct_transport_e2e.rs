mod support;

use rusqlite::Connection;
use serde_json::Value;
use std::io::Write;
use std::net::{Shutdown, TcpStream};
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

const TOKEN: &str = "direct-transport-e2e-token-with-enough-entropy-00001";

fn node_args(workspace: &Path) -> (String, String) {
    (
        workspace.join(".node-state").to_string_lossy().into_owned(),
        workspace.join("node.toml").to_string_lossy().into_owned(),
    )
}

fn run_node(workspace: &Path, args: &[String]) -> Output {
    let (state, config) = node_args(workspace);
    let mut command = Command::new(support::omakure_bin());
    command
        .arg("--scripts-dir")
        .arg(workspace)
        .arg("--json")
        .arg("node")
        .arg("--node-state-dir")
        .arg(state)
        .arg("--node-config")
        .arg(config)
        .args(args)
        .env("OMAKURE_NODE_TEST_MODE", "1")
        .env("OMAKURE_API_TOKEN", TOKEN);
    command.output().expect("run node command")
}

fn json(output: &Output) -> Value {
    support::json_envelope(&output.stdout)
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed: status={:?}, stdout_len={}, stderr_len={}, stdout={:?}, stderr={:?}",
        output.status,
        output.stdout.len(),
        output.stderr.len(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(json(output)["ok"], true);
}

fn init_node(workspace: &Path) {
    let output = run_node(workspace, &["init".to_string()]);
    assert_success(&output);
}

fn status_node(workspace: &Path) -> Value {
    let output = run_node(workspace, &["status".to_string()]);
    assert_success(&output);
    json(&output)["data"].clone()
}

fn trust_node(workspace: &Path, peer_workspace: &Path, peer: &Value) {
    let certificate = std::fs::read(peer_workspace.join(".node-state/transport.cert"))
        .expect("read peer transport certificate")
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let args = vec![
        "trust".to_string(),
        "--node-id".to_string(),
        peer["identity"]["node_id"].as_str().unwrap().to_string(),
        "--public-key".to_string(),
        peer["identity"]["public_key"].as_str().unwrap().to_string(),
        "--transport-certificate".to_string(),
        certificate,
        "--capability".to_string(),
        "remote-run".to_string(),
        "--actor".to_string(),
        "e2e".to_string(),
        "--reason".to_string(),
        "pretrusted transport peer".to_string(),
        "--confirmed".to_string(),
    ];
    let output = run_node(workspace, &args);
    assert_success(&output);
    assert_eq!(json(&output)["data"]["state"], "active");
}

fn configure_static_peer(workspace: &Path, direct_port: &str, peer_node_id: &str, peer_port: &str) {
    let path = workspace.join("node.toml");
    let config = std::fs::read_to_string(&path).expect("read node config");
    let config = config
        .replace(
            "static_peers = []",
            &format!("static_peers = [\"{peer_node_id}@127.0.0.1:{peer_port}\"]"),
        )
        .replace(
            "static_peers = [",
            &format!("direct_bind = \"127.0.0.1:{direct_port}\"\nstatic_peers = ["),
        );
    std::fs::write(path, config).expect("write static peer config");
}

fn wait_for_connected(server: &support::HttpServer, peer_node_id: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    loop {
        let status = server.get("/v1/node/status");
        assert_eq!(status.status, 200, "body: {}", status.safe_body());
        let transport = &status.json()["data"]["transport"];
        if transport["connected_peer_count"] == 1
            && transport["peers"][0]["node_id"] == peer_node_id
            && transport["peers"][0]["state"] == "connected"
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "transport did not connect: {transport}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn free_port() -> String {
    support::unique_loopback_port().to_string()
}

fn probe(workspace: &Path, endpoint: &str, peer_node_id: &str) -> Output {
    run_node(
        workspace,
        &[
            "direct-probe".to_string(),
            "--endpoint".to_string(),
            endpoint.to_string(),
            "--peer-node-id".to_string(),
            peer_node_id.to_string(),
        ],
    )
}

fn audit_count(workspace: &Path, outcome: &str) -> i64 {
    let connection =
        Connection::open(workspace.join(".node-state/node.sqlite")).expect("open node registry");
    connection
        .query_row(
            "SELECT COUNT(*) FROM transport_audit WHERE outcome = ?1",
            [outcome],
            |row| row.get(0),
        )
        .expect("count transport audit rows")
}

fn wait_for_audit(workspace: &Path, outcome: &str, expected: i64) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while audit_count(workspace, outcome) < expected {
        assert!(
            std::time::Instant::now() < deadline,
            "expected {expected} {outcome} transport audit row(s)"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn registry_snapshot(workspace: &Path) -> (Vec<String>, Vec<String>) {
    let connection =
        Connection::open(workspace.join(".node-state/node.sqlite")).expect("open node registry");
    let mut peers = connection
        .prepare(
            "SELECT node_id, public_key, role, state, capabilities_json
             FROM peers ORDER BY node_id",
        )
        .expect("prepare peer snapshot");
    let peers = peers
        .query_map([], |row| {
            Ok(format!(
                "{}|{}|{}|{}|{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .expect("query peer snapshot")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect peer snapshot");
    let mut trusted = connection
        .prepare(
            "SELECT node_id, role, state, hex(capabilities)
             FROM trusted_peers ORDER BY node_id",
        )
        .expect("prepare trusted snapshot");
    let trusted = trusted
        .query_map([], |row| {
            Ok(format!(
                "{}|{}|{}|{}",
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .expect("query trusted snapshot")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect trusted snapshot");
    (peers, trusted)
}

fn rejection_audit_snapshot(workspace: &Path) -> (i64, String, String, Option<i64>) {
    let connection =
        Connection::open(workspace.join(".node-state/node.sqlite")).expect("open node registry");
    let count = connection
        .query_row(
            "SELECT COUNT(*) FROM transport_audit WHERE outcome = 'rejected'",
            [],
            |row| row.get(0),
        )
        .expect("count rejected transport audits");
    let (event_type, outcome, error_code) = connection
        .query_row(
            "SELECT event_type, outcome, error_code
             FROM transport_audit WHERE outcome = 'rejected'
             ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read latest transport rejection audit");
    (count, event_type, outcome, error_code)
}

fn assert_raw_rejection<F>(workspace: &Path, attack: F)
where
    F: FnOnce(),
{
    let before_rows = registry_snapshot(workspace);
    let before_rejected = audit_count(workspace, "rejected");
    std::thread::sleep(Duration::from_millis(150));
    attack();
    wait_for_audit(workspace, "rejected", before_rejected + 1);
    assert_eq!(registry_snapshot(workspace), before_rows);
    let (_, event_type, outcome, error_code) = rejection_audit_snapshot(workspace);
    assert_eq!(event_type, "probe_rejected");
    assert_eq!(outcome, "rejected");
    assert!(error_code.is_some());
}

fn raw_frame(version: u8, kind: u8, flags: u16, body: &[u8]) -> Vec<u8> {
    let length = 4 + body.len();
    let mut frame = Vec::with_capacity(length + 4);
    frame.extend_from_slice(&(length as u32).to_be_bytes());
    frame.extend_from_slice(&[version, kind]);
    frame.extend_from_slice(&flags.to_be_bytes());
    frame.extend_from_slice(body);
    frame
}

fn send_raw(endpoint: &str, bytes: &[u8]) {
    let mut stream = TcpStream::connect(endpoint).expect("connect raw adversary");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set raw read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set raw write timeout");
    stream.write_all(bytes).expect("send raw adversary frame");
    let _ = stream.shutdown(Shutdown::Write);
}

#[test]
fn direct_transport_process_probe_authorizes_audits_rejects_and_restarts() {
    let first = support::TestWorkspace::new("direct_transport_first");
    let second = support::TestWorkspace::new("direct_transport_second");
    let untrusted = support::TestWorkspace::new("direct_transport_untrusted");
    init_node(first.path());
    init_node(second.path());
    init_node(untrusted.path());

    let first_status = status_node(first.path());
    let second_status = status_node(second.path());
    trust_node(first.path(), second.path(), &second_status);
    trust_node(second.path(), first.path(), &first_status);

    let first_port = free_port();
    let first_server = support::HttpServer::start_node_service(
        first.path(),
        TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--direct-bind",
            &format!("127.0.0.1:{first_port}"),
        ],
        &[],
        Duration::from_secs(15),
    );
    let second_port = free_port();
    let second_server = support::HttpServer::start_node_service(
        second.path(),
        TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--direct-bind",
            &format!("127.0.0.1:{second_port}"),
        ],
        &[],
        Duration::from_secs(15),
    );

    let first_probe = probe(
        first.path(),
        &format!("127.0.0.1:{second_port}"),
        second_status["identity"]["node_id"].as_str().unwrap(),
    );
    assert_success(&first_probe);
    assert_eq!(json(&first_probe)["data"]["accepted"], true);
    wait_for_audit(second.path(), "accepted", 1);

    let reverse_probe = probe(
        second.path(),
        &format!("127.0.0.1:{first_port}"),
        first_status["identity"]["node_id"].as_str().unwrap(),
    );
    assert_success(&reverse_probe);
    assert_eq!(json(&reverse_probe)["data"]["accepted"], true);
    wait_for_audit(first.path(), "accepted", 1);

    let untrusted_probe = probe(
        untrusted.path(),
        &format!("127.0.0.1:{second_port}"),
        second_status["identity"]["node_id"].as_str().unwrap(),
    );
    assert!(!untrusted_probe.status.success());
    wait_for_audit(second.path(), "rejected", 1);

    let endpoint = format!("127.0.0.1:{second_port}");
    assert_raw_rejection(second.path(), || {
        send_raw(&endpoint, &raw_frame(9, 1, 0, &[]));
    });
    // The source admission contract is four unauthenticated attempts per
    // minute. The first four malformed attempts above exercise the rejection
    // path; a larger flood must be dropped without making the service fail.
    for _ in 0..32 {
        let _ = TcpStream::connect(&endpoint);
    }
    let exit = second_server.terminate();
    assert!(exit.success() || exit.code().is_none());

    let restarted = support::HttpServer::start_node_service(
        second.path(),
        TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--direct-bind",
            &format!("127.0.0.1:{second_port}"),
        ],
        &[],
        Duration::from_secs(15),
    );
    let restarted_probe = probe(
        first.path(),
        &format!("127.0.0.1:{second_port}"),
        second_status["identity"]["node_id"].as_str().unwrap(),
    );
    assert_success(&restarted_probe);
    wait_for_audit(second.path(), "accepted", 2);

    let _ = restarted.terminate();
    let _ = first_server.terminate();
}

#[test]
fn node_service_static_peers_connect_reconnect_and_report_redacted_status() {
    let first = support::TestWorkspace::new("direct_static_first");
    let second = support::TestWorkspace::new("direct_static_second");
    init_node(first.path());
    init_node(second.path());

    let first_status = status_node(first.path());
    let second_status = status_node(second.path());
    trust_node(first.path(), second.path(), &second_status);
    trust_node(second.path(), first.path(), &first_status);
    let first_direct_port = free_port();
    let second_direct_port = free_port();
    configure_static_peer(
        first.path(),
        &first_direct_port,
        second_status["identity"]["node_id"].as_str().unwrap(),
        &second_direct_port,
    );
    configure_static_peer(
        second.path(),
        &second_direct_port,
        first_status["identity"]["node_id"].as_str().unwrap(),
        &first_direct_port,
    );

    let first_server = support::HttpServer::start_node_service(
        first.path(),
        TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--capability",
            "node:read",
        ],
        &[],
        Duration::from_secs(15),
    );
    let second_server = support::HttpServer::start_node_service(
        second.path(),
        TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--capability",
            "node:read",
        ],
        &[],
        Duration::from_secs(15),
    );
    wait_for_connected(
        &first_server,
        second_status["identity"]["node_id"].as_str().unwrap(),
    );
    wait_for_connected(
        &second_server,
        first_status["identity"]["node_id"].as_str().unwrap(),
    );
    let status_body = first_server.get("/v1/node/status").json();
    let transport = status_body["data"]["transport"].to_string();
    assert!(!transport.contains(&first_direct_port));
    assert!(!transport.contains("127.0.0.1"));

    let exit = second_server.terminate();
    assert!(exit.success() || exit.code().is_none());
    let restarted = support::HttpServer::start_node_service(
        second.path(),
        TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--capability",
            "node:read",
        ],
        &[],
        Duration::from_secs(15),
    );
    wait_for_connected(
        &first_server,
        second_status["identity"]["node_id"].as_str().unwrap(),
    );
    let _ = restarted.terminate();
    let _ = first_server.terminate();
}

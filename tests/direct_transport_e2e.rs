mod support;

use rusqlite::Connection;
use serde_json::Value;
use snow::{params::NoiseParams, Builder};
use std::io::Write;
use std::net::{Shutdown, TcpStream};
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use omakure::direct_transport::{
    sign_probe, unix_seconds, Frame, HandshakeRole, NoiseHandshake, TransportCertificate,
    TransportSession, ENVELOPE_KIND, NOISE_NAME, PROLOGUE,
};
use omakure::node::{NodeContext, NodePathOverrides, NodePlatform};
use omakure::node_identity::NodeIdentity;

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

fn enable_manual_enrollment(workspace: &Path) {
    let path = workspace.join("node.toml");
    let config = std::fs::read_to_string(&path).expect("read node config");
    let config = config.replace("enrollment = \"disabled\"", "enrollment = \"manual\"");
    std::fs::write(path, config).expect("enable manual enrollment");
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
             FROM transport_audit WHERE outcome = 'rejected' AND error_code IS NOT NULL
             ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read latest transport rejection audit");
    (count, event_type, outcome, error_code)
}

fn protocol_rejection_count(workspace: &Path) -> i64 {
    Connection::open(workspace.join(".node-state/node.sqlite"))
        .expect("open node registry")
        .query_row(
            "SELECT COUNT(*) FROM transport_audit
             WHERE outcome = 'rejected' AND error_code IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .expect("count protocol rejection audits")
}

fn assert_raw_rejection<F>(workspace: &Path, attack: F)
where
    F: FnOnce(),
{
    let before_rows = registry_snapshot(workspace);
    let before_rejected = protocol_rejection_count(workspace);
    std::thread::sleep(Duration::from_millis(150));
    attack();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && protocol_rejection_count(workspace) < before_rejected + 1 {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(protocol_rejection_count(workspace) > before_rejected);
    assert_eq!(registry_snapshot(workspace), before_rows);
    let (_, event_type, outcome, error_code) = rejection_audit_snapshot(workspace);
    assert_eq!(event_type, "probe_rejected");
    assert_eq!(outcome, "rejected");
    assert!(
        error_code.is_some(),
        "rejection audit did not record a protocol error: event={event_type} outcome={outcome}"
    );
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

fn node_material(workspace: &Path) -> (NodeIdentity, [u8; 32], TransportCertificate) {
    let context = NodeContext::resolve_for(
        NodePlatform::current(),
        NodePathOverrides::new(
            Some(workspace.join(".node-state")),
            Some(workspace.join("node.toml")),
        ),
        true,
        None,
        None,
        None,
    )
    .expect("resolve node context");
    let identity = NodeIdentity::load_existing(&context).expect("load node identity");
    let private: [u8; 32] = std::fs::read(context.transport_key_path())
        .expect("read transport key")
        .try_into()
        .expect("transport key length");
    let certificate = TransportCertificate::from_bytes(
        &std::fs::read(context.transport_certificate_path()).expect("read transport certificate"),
    )
    .expect("parse transport certificate");
    (identity, private, certificate)
}

fn read_frame(stream: &mut TcpStream) -> Vec<u8> {
    use std::io::Read;

    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).expect("read frame prefix");
    let length = u32::from_be_bytes(prefix) as usize;
    let mut encoded = vec![0_u8; length + 4];
    encoded[..4].copy_from_slice(&prefix);
    stream
        .read_exact(&mut encoded[4..])
        .expect("read frame body");
    encoded
}

fn send_handshake_frame(stream: &mut TcpStream, message_number: u8, message: &[u8]) {
    stream
        .write_all(
            &Frame::handshake(message_number, message)
                .expect("encode handshake frame")
                .encode()
                .expect("serialize handshake frame"),
        )
        .expect("send handshake frame");
}

fn custom_certificate_handshake(endpoint: &str, private: [u8; 32], certificate: &[u8]) {
    let params: NoiseParams = NOISE_NAME.parse().expect("parse Noise parameters");
    let mut handshake = Builder::new(params)
        .prologue(PROLOGUE)
        .expect("set Noise prologue")
        .local_private_key(&private)
        .expect("set Noise private key")
        .build_initiator()
        .expect("build Noise initiator");
    let mut stream = TcpStream::connect(endpoint).expect("connect custom handshake");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set custom handshake read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set custom handshake write timeout");
    let mut message = vec![0_u8; 4096];
    let length = handshake
        .write_message(&[], &mut message)
        .expect("write Noise message 1");
    send_handshake_frame(&mut stream, 1, &message[..length]);

    let response = Frame::parse(&read_frame(&mut stream)).expect("parse Noise message 2");
    assert_eq!(response.message_number().expect("message 2 number"), 2);
    let mut payload = vec![0_u8; 4096];
    handshake
        .read_message(&response.body[1..], &mut payload)
        .expect("read Noise message 2");
    let mut message_three_payload = Vec::with_capacity(1 + certificate.len());
    message_three_payload.push(1);
    message_three_payload.extend_from_slice(certificate);
    let length = handshake
        .write_message(&message_three_payload, &mut message)
        .expect("write custom Noise message 3");
    send_handshake_frame(&mut stream, 3, &message[..length]);
    let _ = stream.shutdown(Shutdown::Both);
}

fn valid_session(
    endpoint: &str,
    private: [u8; 32],
    certificate: TransportCertificate,
) -> (TcpStream, TransportSession) {
    let mut handshake = NoiseHandshake::new(HandshakeRole::Initiator, private, certificate)
        .expect("build production Noise handshake");
    let mut stream = TcpStream::connect(endpoint).expect("connect valid session");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set valid session read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set valid session write timeout");
    stream
        .write_all(&handshake.write_next().expect("write production message 1"))
        .expect("send production message 1");
    let response = read_frame(&mut stream);
    handshake
        .read_next(&response, unix_seconds())
        .expect("read production message 2");
    stream
        .write_all(&handshake.write_next().expect("write production message 3"))
        .expect("send production message 3");
    (
        stream,
        handshake.into_session().expect("finish production session"),
    )
}

fn send_probe_frame(
    stream: &mut TcpStream,
    session: &mut TransportSession,
    identity: &NodeIdentity,
    nonce: [u8; 16],
    mutate_signature: bool,
) -> Vec<u8> {
    let signed = sign_probe(identity, session.session_id(), nonce, unix_seconds())
        .expect("sign production probe");
    let mut body = signed.encoded();
    if mutate_signature {
        let last = body.last_mut().expect("probe signature");
        *last ^= 1;
    }
    let frame = session
        .write(ENVELOPE_KIND, &body)
        .expect("encrypt production probe");
    stream.write_all(&frame).expect("send production probe");
    frame
}

fn protocol_audit_count(workspace: &Path) -> i64 {
    protocol_rejection_count(workspace)
}

fn latest_protocol_audit(workspace: &Path) -> (String, String, i64) {
    Connection::open(workspace.join(".node-state/node.sqlite"))
        .expect("open audit registry")
        .query_row(
            "SELECT event_type, outcome, error_code
             FROM transport_audit
             WHERE outcome = 'rejected' AND error_code IS NOT NULL
             ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read protocol audit")
}

fn wait_for_protocol_audit(workspace: &Path, before: i64, expected_code: i64) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if protocol_audit_count(workspace) > before {
            let (event_type, outcome, error_code) = latest_protocol_audit(workspace);
            assert_eq!(event_type, "probe_rejected");
            assert_eq!(outcome, "rejected");
            assert_eq!(error_code, expected_code);
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("expected protocol audit code {expected_code}");
}

fn full_registry_snapshot(workspace: &Path) -> String {
    full_registry_snapshot_at(&workspace.join(".node-state/node.sqlite"))
}

fn full_registry_snapshot_at(database: &Path) -> String {
    Connection::open(database)
        .expect("open full registry")
        .query_row(
            "SELECT
              COALESCE((SELECT group_concat(value, ';') FROM (SELECT node_id || ':' || state || ':' || hex(identity_key) AS value FROM remote_identities ORDER BY node_id)), '') || '|' ||
              COALESCE((SELECT group_concat(value, ';') FROM (SELECT node_id || ':' || state || ':' || role || ':' || hex(capabilities) AS value FROM trusted_peers ORDER BY node_id)), '') || '|' ||
              COALESCE((SELECT group_concat(value, ';') FROM (SELECT node_id || ':' || key_epoch || ':' || state || ':' || hex(public_key) AS value FROM transport_key_epochs ORDER BY node_id, key_epoch)), '') || '|' ||
              COALESCE((SELECT group_concat(value, ';') FROM (SELECT replay_kind || ':' || hex(replay_id) || ':' || expires_at AS value FROM enrollment_replays ORDER BY replay_kind, replay_id)), '') || '|' ||
              COALESCE((SELECT group_concat(value, ';') FROM (SELECT hex(request_id) || ':' || node_id || ':' || state AS value FROM manual_enrollment_requests ORDER BY request_id)), '') || '|' ||
              COALESCE((SELECT group_concat(value, ';') FROM (SELECT hex(session_id) || ':' || node_id || ':' || state || ':' || send_sequence || ':' || receive_sequence AS value FROM channel_sessions ORDER BY session_id)), '')",
            [],
            |row| row.get(0),
        )
        .expect("read full registry snapshot")
}

fn latest_protocol_audit_at(database: &Path) -> (String, String, i64) {
    Connection::open(database)
        .expect("open protocol audit database")
        .query_row(
            "SELECT event_type, outcome, error_code
             FROM transport_audit
             WHERE outcome = 'rejected' AND error_code IS NOT NULL
             ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read latest protocol audit")
}

fn assert_custom_certificate_case(
    target: &Path,
    endpoint: &str,
    private: [u8; 32],
    certificate: &[u8],
    expected_code: i64,
) {
    let before_state = full_registry_snapshot(target);
    let before_rejections = protocol_audit_count(target);
    custom_certificate_handshake(endpoint, private, certificate);
    wait_for_protocol_audit(target, before_rejections, expected_code);
    assert_eq!(full_registry_snapshot(target), before_state);
}

#[test]
#[ignore = "canonical certification invokes this against the post-reset production listener"]
fn direct_transport_post_reset_old_identity_rejected() {
    let old_state = std::env::var_os("OMAKURE_RESET_OLD_STATE")
        .expect("OMAKURE_RESET_OLD_STATE for canonical reset proof");
    let old_config = std::env::var_os("OMAKURE_RESET_OLD_CONFIG")
        .expect("OMAKURE_RESET_OLD_CONFIG for canonical reset proof");
    let endpoint = std::env::var("OMAKURE_RESET_ENDPOINT")
        .expect("OMAKURE_RESET_ENDPOINT for canonical reset proof");
    let expected_node_id = std::env::var("OMAKURE_RESET_EXPECTED_NODE_ID")
        .expect("OMAKURE_RESET_EXPECTED_NODE_ID for canonical reset proof");
    let database = Path::new(&old_state).join("node.sqlite");
    let before_state = full_registry_snapshot_at(&database);

    let output = Command::new(support::omakure_bin())
        .arg("--json")
        .arg("node")
        .arg("--node-state-dir")
        .arg(&old_state)
        .arg("--node-config")
        .arg(&old_config)
        .arg("direct-probe")
        .arg("--endpoint")
        .arg(endpoint)
        .arg("--peer-node-id")
        .arg(expected_node_id)
        .env("OMAKURE_NODE_TEST_MODE", "1")
        .output()
        .expect("run old identity reset proof");
    assert!(!output.status.success(), "old reset identity was accepted");
    assert_eq!(json(&output)["error"]["code"], "transport_not_enrolled");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let audit = latest_protocol_audit_at(&database);
        if audit == ("probe_rejected".into(), "rejected".into(), 1006) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "old identity proof audit was not 1006: {audit:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(full_registry_snapshot_at(&database), before_state);
}

#[test]
fn direct_transport_production_listener_rejects_adversarial_certificates_envelopes_and_targets() {
    let initiator = support::TestWorkspace::new("direct_adversarial_initiator");
    let target = support::TestWorkspace::new("direct_adversarial_target");
    init_node(initiator.path());
    init_node(target.path());

    let initiator_status = status_node(initiator.path());
    let target_status = status_node(target.path());
    trust_node(target.path(), initiator.path(), &initiator_status);
    let target_port = free_port();
    let target_server = support::HttpServer::start_node_service(
        target.path(),
        TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--direct-bind",
            &format!("127.0.0.1:{target_port}"),
            "--capability",
            "node:read",
        ],
        &[],
        Duration::from_secs(15),
    );
    let endpoint = format!("127.0.0.1:{target_port}");
    let (initiator_identity, private, valid_certificate) = node_material(initiator.path());

    let now = unix_seconds();
    let expired_certificate = TransportCertificate::issue(
        &initiator_identity,
        *valid_certificate.transport_public(),
        valid_certificate.key_epoch() + 1,
        now.saturating_sub(2_000),
        now.saturating_sub(1_000),
        [0x11; 16],
    )
    .expect("issue expired certificate fixture");
    assert_custom_certificate_case(
        target.path(),
        &endpoint,
        private,
        expired_certificate.as_bytes(),
        1008,
    );

    let mut forged_signature = valid_certificate.as_bytes().to_vec();
    *forged_signature.last_mut().expect("certificate signature") ^= 1;
    assert_custom_certificate_case(target.path(), &endpoint, private, &forged_signature, 1004);

    let mut mismatched_identity = valid_certificate.as_bytes().to_vec();
    mismatched_identity[45] = if mismatched_identity[45] == b'0' {
        b'1'
    } else {
        b'0'
    };
    assert_custom_certificate_case(
        target.path(),
        &endpoint,
        private,
        &mismatched_identity,
        1005,
    );

    let exit = target_server.terminate();
    assert!(exit.success() || exit.code().is_none());
    let target_server = support::HttpServer::start_node_service(
        target.path(),
        TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--direct-bind",
            &format!("127.0.0.1:{target_port}"),
            "--capability",
            "node:read",
        ],
        &[],
        Duration::from_secs(15),
    );

    let before_forged_envelope_state = full_registry_snapshot(target.path());
    let before_forged_envelope_rejections = protocol_audit_count(target.path());
    let (mut forged_stream, mut forged_session) =
        valid_session(&endpoint, private, valid_certificate.clone());
    send_probe_frame(
        &mut forged_stream,
        &mut forged_session,
        &initiator_identity,
        [0x21; 16],
        true,
    );
    let _ = forged_stream.shutdown(Shutdown::Both);
    wait_for_protocol_audit(target.path(), before_forged_envelope_rejections, 1004);
    assert_eq!(
        full_registry_snapshot(target.path()),
        before_forged_envelope_state
    );

    let before_replay_state = full_registry_snapshot(target.path());
    let before_replay_rejections = protocol_audit_count(target.path());
    let (mut replay_stream, mut replay_session) =
        valid_session(&endpoint, private, valid_certificate.clone());
    let replay_frame = send_probe_frame(
        &mut replay_stream,
        &mut replay_session,
        &initiator_identity,
        [0x31; 16],
        false,
    );
    let ack = read_frame(&mut replay_stream);
    replay_session
        .read(&ack)
        .expect("read production probe ack");
    replay_stream
        .write_all(&replay_frame)
        .expect("send identical encrypted replay bytes");
    let _ = replay_stream.shutdown(Shutdown::Both);
    wait_for_protocol_audit(target.path(), before_replay_rejections, 1009);
    assert_eq!(full_registry_snapshot(target.path()), before_replay_state);

    let before_wrong_target_state = full_registry_snapshot(target.path());
    let before_wrong_target_rejections = protocol_audit_count(initiator.path());
    let wrong_target = probe(
        initiator.path(),
        &endpoint,
        initiator_status["identity"]["node_id"]
            .as_str()
            .expect("initiator node id as wrong target"),
    );
    assert!(!wrong_target.status.success());
    assert_eq!(
        json(&wrong_target)["error"]["code"],
        "transport_identity_mismatch"
    );
    wait_for_protocol_audit(initiator.path(), before_wrong_target_rejections, 1005);
    assert_eq!(
        full_registry_snapshot(target.path()),
        before_wrong_target_state
    );

    assert_eq!(
        target_server.get("/v1/node/status").json()["data"]["identity"]["node_id"],
        target_status["identity"]["node_id"]
    );
    let exit = target_server.terminate();
    assert!(exit.success() || exit.code().is_none());
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
fn direct_transport_manual_enrollment_stages_then_requires_approval() {
    let target = support::TestWorkspace::new("direct_enrollment_target");
    let candidate = support::TestWorkspace::new("direct_enrollment_candidate");
    init_node(target.path());
    init_node(candidate.path());
    enable_manual_enrollment(target.path());
    enable_manual_enrollment(candidate.path());

    let target_port = free_port();
    let target_server = support::HttpServer::start_node_service(
        target.path(),
        TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--direct-bind",
            &format!("127.0.0.1:{target_port}"),
            "--capability",
            "node:read",
        ],
        &[],
        Duration::from_secs(15),
    );
    let candidate_port = free_port();
    let candidate_server = support::HttpServer::start_node_service(
        candidate.path(),
        TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--direct-bind",
            &format!("127.0.0.1:{candidate_port}"),
        ],
        &[],
        Duration::from_secs(15),
    );

    let request = run_node(
        candidate.path(),
        &[
            "enroll".into(),
            "request".into(),
            "--endpoint".into(),
            format!("127.0.0.1:{target_port}"),
            "--capability".into(),
            "remote-run".into(),
            "--lifetime-seconds".into(),
            "300".into(),
        ],
    );
    assert_success(&request);
    let request_data = json(&request)["data"].clone();
    assert_eq!(request_data["state"], "pending");
    assert_eq!(
        target_server.get("/v1/node/peers").json()["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let certificate = std::fs::read(candidate.path().join(".node-state/transport.cert"))
        .expect("read candidate transport certificate")
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let approve = run_node(
        target.path(),
        &[
            "enroll".into(),
            "approve".into(),
            "--request".into(),
            request_data["request_hex"].as_str().unwrap().into(),
            "--transport-certificate".into(),
            certificate,
            "--code".into(),
            request_data["code"].as_str().unwrap().into(),
            "--actor".into(),
            "e2e".into(),
            "--reason".into(),
            "approved after out-of-band verification".into(),
            "--confirmed".into(),
        ],
    );
    assert_success(&approve);
    assert_eq!(json(&approve)["data"]["state"], "active");
    assert_eq!(
        target_server.get("/v1/node/peers").json()["data"][0]["state"],
        "active"
    );

    let _ = candidate_server.terminate();
    let _ = target_server.terminate();
}

/// A probe into a standing session must say so, not die on a closed stream.
///
/// This is the normal state of a managed fleet: the service holds a session
/// with the peer, so the peer refuses a second connection inside `register` and
/// hangs up without a reply. The dial sees only `UnexpectedEof`, which used to
/// surface as `transport_internal` / "direct transport I/O failed" -- an error
/// that names neither the cause nor anything the operator could do about it.
///
/// The second half is the more important one. A connected peer must not become
/// an excuse for every I/O failure: a probe at an address where nothing is
/// listening is still a wrong address, and must keep saying so.
#[test]
fn direct_probe_names_the_standing_session_and_still_reports_a_dead_endpoint() {
    let first = support::TestWorkspace::new("direct_probe_session_first");
    let second = support::TestWorkspace::new("direct_probe_session_second");
    init_node(first.path());
    init_node(second.path());

    let first_status = status_node(first.path());
    let second_status = status_node(second.path());
    let second_id = second_status["identity"]["node_id"].as_str().unwrap();
    trust_node(first.path(), second.path(), &second_status);
    trust_node(second.path(), first.path(), &first_status);

    let first_direct_port = free_port();
    let second_direct_port = free_port();
    // Both sides name the other because dial ownership is decided by node id
    // order, and the ids are fresh every run: configuring one side only would
    // leave no session at all about half the time.
    configure_static_peer(
        first.path(),
        &first_direct_port,
        second_id,
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
    // Wait on the fact, not on a duration. Probing before the session is up
    // would succeed and prove nothing.
    wait_for_connected(&first_server, second_id);

    let refused = probe(
        first.path(),
        &format!("127.0.0.1:{second_direct_port}"),
        second_id,
    );
    assert!(
        !refused.status.success(),
        "a second connection to a connected peer cannot succeed"
    );
    let envelope = json(&refused);
    assert_eq!(
        envelope["error"]["code"], "already_exists",
        "the operator must be told a session exists, not handed an I/O \
         failure: {envelope}"
    );
    let message = envelope["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(second_id) && message.contains("node status"),
        "the explanation must name the peer and where the answer already is: {message}"
    );

    // Nothing is listening here, and the peer this names is connected. The
    // standing session must not be offered as the explanation for that.
    let dead_port = free_port();
    let unreachable = probe(first.path(), &format!("127.0.0.1:{dead_port}"), second_id);
    assert!(!unreachable.status.success());
    let envelope = json(&unreachable);
    assert_eq!(
        envelope["error"]["code"], "transport_internal",
        "a refused connection is a wrong endpoint, not a duplicate session: {envelope}"
    );

    let _ = second_server.terminate();
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

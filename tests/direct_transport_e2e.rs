mod support;

use rusqlite::Connection;
use serde_json::Value;
use snow::{params::NoiseParams, Builder, HandshakeState, TransportState};
use std::io::{Read, Write};
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

fn noise_initiator(private_key: &[u8; 32]) -> HandshakeState {
    let params: NoiseParams = "Noise_XX_25519_ChaChaPoly_SHA256"
        .parse()
        .expect("parse Noise parameters");
    Builder::new(params)
        .prologue(b"omakure/direct-transport/v1\0")
        .expect("set Noise prologue")
        .local_private_key(private_key)
        .expect("set Noise static key")
        .build_initiator()
        .expect("build Noise initiator")
}

fn write_noise_frame(stream: &mut TcpStream, message_number: u8, message: &[u8]) {
    let mut body = Vec::with_capacity(message.len() + 1);
    body.push(message_number);
    body.extend_from_slice(message);
    stream
        .write_all(&raw_frame(1, 1, 0, &body))
        .expect("send Noise handshake frame");
}

fn read_noise_frame(stream: &mut TcpStream) -> Vec<u8> {
    let mut prefix = [0u8; 4];
    stream
        .read_exact(&mut prefix)
        .expect("read Noise handshake length");
    let length = u32::from_be_bytes(prefix) as usize;
    let mut frame = vec![0u8; length + 4];
    frame[..4].copy_from_slice(&prefix);
    stream
        .read_exact(&mut frame[4..])
        .expect("read Noise handshake frame");
    frame
}

fn raw_msg3_rejection(
    endpoint: &str,
    private_key: &[u8; 32],
    certificate: &[u8],
    mutate: impl FnOnce(&mut [u8]),
) {
    let mut stream = TcpStream::connect(endpoint).expect("connect raw Noise adversary");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set raw Noise read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set raw Noise write timeout");
    let mut handshake = noise_initiator(private_key);
    let mut message = vec![0u8; 4096];
    let length = handshake
        .write_message(&[], &mut message)
        .expect("write Noise message 1");
    write_noise_frame(&mut stream, 1, &message[..length]);
    let response = read_noise_frame(&mut stream);
    let mut payload = vec![0u8; 4096];
    let response_length = handshake
        .read_message(&response[9..], &mut payload)
        .expect("read Noise message 2");
    assert!(response_length > 0);

    let mut certificate = certificate.to_vec();
    mutate(&mut certificate);
    let mut certificate_payload = Vec::with_capacity(1 + certificate.len());
    certificate_payload.push(1);
    certificate_payload.extend_from_slice(&certificate);
    let length = handshake
        .write_message(&certificate_payload, &mut message)
        .expect("write Noise message 3");
    write_noise_frame(&mut stream, 3, &message[..length]);
    let _ = stream.shutdown(Shutdown::Write);
}

fn raw_authenticated_stream(
    endpoint: &str,
    private_key: &[u8; 32],
    certificate: &[u8],
) -> (TcpStream, [u8; 32], TransportState) {
    let mut stream = TcpStream::connect(endpoint).expect("connect raw session adversary");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set raw session read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("set raw session write timeout");
    let mut handshake = noise_initiator(private_key);
    let mut message = vec![0u8; 4096];
    let length = handshake
        .write_message(&[], &mut message)
        .expect("write session Noise message 1");
    write_noise_frame(&mut stream, 1, &message[..length]);
    let response = read_noise_frame(&mut stream);
    let mut payload = vec![0u8; 4096];
    handshake
        .read_message(&response[9..], &mut payload)
        .expect("read session Noise message 2");
    let mut certificate_payload = Vec::with_capacity(1 + certificate.len());
    certificate_payload.push(1);
    certificate_payload.extend_from_slice(certificate);
    let length = handshake
        .write_message(&certificate_payload, &mut message)
        .expect("write session Noise message 3");
    write_noise_frame(&mut stream, 3, &message[..length]);
    let session_id = handshake
        .get_handshake_hash()
        .try_into()
        .expect("Noise handshake hash length");
    let transport = handshake
        .into_transport_mode()
        .expect("enter Noise transport mode");
    (stream, session_id, transport)
}

fn captured_session_frame(endpoint: &str, private_key: &[u8; 32], certificate: &[u8]) -> Vec<u8> {
    let (mut stream, session_id, mut transport) =
        raw_authenticated_stream(endpoint, private_key, certificate);
    let mut plaintext = Vec::new();
    plaintext.extend_from_slice(&0u64.to_be_bytes());
    plaintext.extend_from_slice(&[1, 1]);
    plaintext.extend_from_slice(b"captured-session");
    let mut ciphertext = vec![0u8; plaintext.len() + 16];
    let length = transport
        .write_message(&plaintext, &mut ciphertext)
        .expect("encrypt captured session frame");
    ciphertext.truncate(length);
    let mut body = session_id.to_vec();
    body.extend_from_slice(&ciphertext);
    let frame = raw_frame(1, 2, 0, &body);
    stream
        .write_all(&frame)
        .expect("send captured session frame");
    let _ = stream.shutdown(Shutdown::Write);
    frame
}

fn send_captured_frame(endpoint: &str, private_key: &[u8; 32], certificate: &[u8], frame: &[u8]) {
    let (mut stream, _, _) = raw_authenticated_stream(endpoint, private_key, certificate);
    stream
        .write_all(frame)
        .expect("replay captured session frame");
    let _ = stream.shutdown(Shutdown::Write);
}

fn transport_material(workspace: &Path) -> ([u8; 32], Vec<u8>) {
    let private_key: [u8; 32] = std::fs::read(workspace.join(".node-state/transport.key"))
        .expect("read transport private key")
        .try_into()
        .expect("transport private key length");
    let certificate = std::fs::read(workspace.join(".node-state/transport.cert"))
        .expect("read transport certificate");
    (private_key, certificate)
}

fn hex_key(value: &str) -> [u8; 32] {
    let bytes = (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).unwrap())
        .collect::<Vec<_>>();
    bytes.try_into().unwrap()
}

fn low_order_keys() -> [[u8; 32]; 7] {
    let mut one = [0u8; 32];
    one[0] = 1;
    [
        [0; 32],
        one,
        hex_key("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800"),
        hex_key("5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157"),
        hex_key("ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"),
        hex_key("edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"),
        hex_key("eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"),
    ]
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
    let (untrusted_private, untrusted_certificate) = transport_material(untrusted.path());
    assert_raw_rejection(second.path(), || {
        send_raw(&endpoint, &[0, 0, 0, 3]);
    });
    assert_raw_rejection(second.path(), || {
        send_raw(&endpoint, &raw_frame(9, 1, 0, &[]));
    });
    assert_raw_rejection(second.path(), || {
        raw_msg3_rejection(
            &endpoint,
            &untrusted_private,
            &untrusted_certificate,
            |cert| {
                cert[200] ^= 1;
            },
        );
    });
    for low_order in low_order_keys() {
        assert_raw_rejection(second.path(), || {
            let mut body = Vec::with_capacity(33);
            body.push(1);
            body.extend_from_slice(&low_order);
            send_raw(&endpoint, &raw_frame(1, 1, 0, &body));
        });
    }
    for low_order in low_order_keys() {
        assert_raw_rejection(second.path(), || {
            raw_msg3_rejection(
                &endpoint,
                &untrusted_private,
                &untrusted_certificate,
                |cert| {
                    cert[109..141].copy_from_slice(&low_order);
                },
            );
        });
    }
    let before_rows = registry_snapshot(second.path());
    let before_rejected = audit_count(second.path(), "rejected");
    std::thread::sleep(Duration::from_millis(150));
    let (first_private, first_certificate) = transport_material(first.path());
    let captured = captured_session_frame(&endpoint, &first_private, &first_certificate);
    send_captured_frame(&endpoint, &first_private, &first_certificate, &captured);
    wait_for_audit(second.path(), "rejected", before_rejected + 2);
    assert_eq!(registry_snapshot(second.path()), before_rows);

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

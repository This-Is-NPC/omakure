//! Production-listener compatibility coverage for the Health Plane contract.
//!
//! This test proves three things against the shipped implementation:
//!
//! 1. A Health Plane message reaches the real production Noise listener over the
//!    real handshake, certificate, framing, and session path.
//! 2. Role and capability authorization is decidable from the live `node.sqlite`
//!    of the running service for every one of the five frozen message kinds,
//!    including after `node capabilities` and `node revoke`.
//! 3. Nothing in the frozen direct-transport identity construction has to
//!    change: the same BIP-340 identity key, the same 245-byte transport
//!    certificate, and the same `verify_envelope` accept the Health Plane
//!    envelopes.
//!
//! These assertions provide regression coverage for the production listener and
//! transport compatibility that the shipped Health Plane relies on.

mod support;

use omakure::direct_transport::{
    sign_probe, unix_seconds, verify_envelope, HandshakeRole, NoiseHandshake, TransportCertificate,
    TransportSession, ENVELOPE_KIND,
};
use omakure::node::{NodeContext, NodePathOverrides, NodePlatform};
use omakure::node_identity::NodeIdentity;
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

const TOKEN: &str = "health-plane-feasibility-token-with-enough-entropy-001";
const TRANSPORT_CERTIFICATE_BYTES: usize = 245;

const ROLE_CONDUCTOR: i64 = 1;
const ROLE_PERFORMER: i64 = 2;
const CAPABILITY_PROFILE_PULSE: &str = "inventory-health";
const CAPABILITY_SIGNAL: &str = "notifications";

// ---------------------------------------------------------------------------
// The frozen authorization decision, evaluated only from registry rows.
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Allow,
    Revoked,
    WrongRole,
    MissingCapability,
}

#[derive(Debug, Clone)]
struct TrustRow {
    identity_state: String,
    trust_state: String,
    role: i64,
    capabilities: Vec<String>,
}

fn decide(row: Option<&TrustRow>, kind: &str) -> Decision {
    let Some(row) = row else {
        return Decision::Revoked;
    };
    if row.identity_state != "active" || row.trust_state != "active" {
        return Decision::Revoked;
    }
    let (required_role, required_capability) = match kind {
        "health_profile" | "health_pulse" => (ROLE_PERFORMER, Some(CAPABILITY_PROFILE_PULSE)),
        "health_signal" => (ROLE_PERFORMER, Some(CAPABILITY_SIGNAL)),
        "health_ack" | "health_error" => (ROLE_CONDUCTOR, None),
        other => panic!("unknown health plane kind {other}"),
    };
    if row.role != required_role {
        return Decision::WrongRole;
    }
    if let Some(capability) = required_capability {
        if !row.capabilities.iter().any(|value| value == capability) {
            return Decision::MissingCapability;
        }
    }
    Decision::Allow
}

/// Read role, capabilities, and both trust states straight out of the live
/// `node.sqlite` of the running production service.
fn trust_row(workspace: &Path, node_id: &str) -> Option<TrustRow> {
    let connection = Connection::open(workspace.join(".node-state/node.sqlite"))
        .expect("open live node registry");
    connection
        .query_row(
            "SELECT r.state, p.state, p.role, p.capabilities
             FROM remote_identities r
             JOIN trusted_peers p ON p.node_id = r.node_id
             WHERE r.node_id = ?1",
            [node_id],
            |row| {
                let capabilities: Vec<u8> = row.get(3)?;
                Ok(TrustRow {
                    identity_state: row.get(0)?,
                    trust_state: row.get(1)?,
                    role: row.get(2)?,
                    capabilities: serde_json::from_slice::<Vec<String>>(&capabilities)
                        .expect("canonical capability JSON"),
                })
            },
        )
        .ok()
}

// ---------------------------------------------------------------------------
// Health Plane message construction, mirroring the frozen envelope exactly.
// ---------------------------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn canonical(value: &Value) -> Vec<u8> {
    serde_jcs::to_vec(value).expect("canonical JSON")
}

fn health_payload(kind: &str, target: &str, seed: u8) -> Value {
    let message_id = hex(&[seed; 16]);
    match kind {
        "health_profile" => json!({
            "health_version": 1,
            "message_id": message_id,
            "target": target,
            "profile": {
                "agent_version": "0.3.0",
                "arch": "x86_64",
                "baseline_id": "",
                "baseline_observed_id": "",
                "capabilities": [CAPABILITY_PROFILE_PULSE, CAPABILITY_SIGNAL],
                "display_name": "feasibility-performer",
                "distro_id": "arch",
                "distro_version": "rolling",
                "omarchy_channel": "",
                "omarchy_version": "",
                "platform": "linux",
                "profile_revision": 1,
                "role": "performer",
                "runtimes": [{"available": true, "name": "bash", "version": "5.2.37"}]
            }
        }),
        "health_pulse" => json!({
            "health_version": 1,
            "message_id": message_id,
            "target": target,
            "pulse": {
                "emitted_at": 0,
                "last_run": Value::Null,
                "profile_revision": 1,
                "runner": {
                    "queue_depth": 0,
                    "scheduler": "disabled",
                    "state": "idle",
                    "workers_busy": 0,
                    "workers_configured": 0
                },
                "sequence": 1,
                "uptime_seconds": 1
            }
        }),
        "health_signal" => json!({
            "health_version": 1,
            "message_id": message_id,
            "target": target,
            "signal": {
                "kind": "enrolled",
                "occurred_at": 0,
                "run": Value::Null,
                "sequence": 1,
                "signal_id": hex(&[seed ^ 0xff; 16]),
                "subject": target
            }
        }),
        other => panic!("unsupported feasibility kind {other}"),
    }
}

/// Build and sign a Health Plane envelope using the frozen construction only.
/// Nothing here is production code; the point is that production needs no new
/// signing primitive.
fn sign_health_envelope(
    identity: &NodeIdentity,
    kind: &str,
    session: &TransportSession,
    nonce: [u8; 16],
    target: &str,
    seed: u8,
    now: u64,
) -> Vec<u8> {
    let sender = identity.public_status().node_id.clone();
    let mut payload = health_payload(kind, target, seed);
    if kind == "health_pulse" {
        payload["pulse"]["emitted_at"] = Value::from(now);
    }
    if kind == "health_signal" {
        payload["signal"]["occurred_at"] = Value::from(now);
    }
    let envelope = json!({
        "created_at": now,
        "kind": kind,
        "nonce": hex(&nonce),
        "payload": payload,
        "sender": sender,
        "session_id": hex(session.session_id()),
        "version": 1,
    });
    let canonical_bytes = canonical(&envelope);
    let prehash =
        omakure::node_identity::DirectEnvelopePrehash::from_canonical_bytes(&canonical_bytes);
    let signature = identity
        .sign_direct_envelope(prehash)
        .expect("sign health envelope with the frozen identity key");
    let mut encoded = canonical_bytes;
    encoded.extend_from_slice(&signature.to_bytes());
    encoded
}

// ---------------------------------------------------------------------------
// Node process helpers.
// ---------------------------------------------------------------------------

fn run_node(workspace: &Path, args: &[String]) -> Output {
    let state = workspace.join(".node-state");
    let config = workspace.join("node.toml");
    Command::new(support::omakure_bin())
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
        .env("OMAKURE_API_TOKEN", TOKEN)
        .output()
        .expect("run node command")
}

fn assert_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "node command failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = support::json_envelope(&output.stdout);
    assert_eq!(envelope["ok"], true, "envelope: {envelope}");
    envelope["data"].clone()
}

fn init_node(workspace: &Path) -> Value {
    assert_success(&run_node(workspace, &["init".to_string()]));
    assert_success(&run_node(workspace, &["status".to_string()]))
}

fn trust_peer(
    workspace: &Path,
    peer_workspace: &Path,
    peer_status: &Value,
    role: &str,
    capabilities: &[&str],
) {
    let certificate = hex(
        &std::fs::read(peer_workspace.join(".node-state/transport.cert"))
            .expect("read peer transport certificate"),
    );
    let mut args = vec![
        "trust".to_string(),
        "--node-id".to_string(),
        peer_status["identity"]["node_id"].as_str().unwrap().into(),
        "--public-key".to_string(),
        peer_status["identity"]["public_key"]
            .as_str()
            .unwrap()
            .into(),
        "--transport-certificate".to_string(),
        certificate,
        "--role".to_string(),
        role.to_string(),
        "--actor".to_string(),
        "health-plane-feasibility".to_string(),
        "--reason".to_string(),
        "health plane contract feasibility probe".to_string(),
        "--confirmed".to_string(),
    ];
    for capability in capabilities {
        args.push("--capability".to_string());
        args.push((*capability).to_string());
    }
    let data = assert_success(&run_node(workspace, &args));
    assert_eq!(data["state"], "active");
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

/// Complete a real production handshake and probe/ack round trip against the
/// production listener, then hand back the live session.
fn production_session(
    endpoint: &str,
    private: [u8; 32],
    certificate: TransportCertificate,
    identity: &NodeIdentity,
    remote_node_id: &str,
    remote_identity_key: &[u8; 32],
) -> (TcpStream, TransportSession) {
    let mut handshake = NoiseHandshake::new(HandshakeRole::Initiator, private, certificate)
        .expect("build production Noise handshake");
    let mut stream = TcpStream::connect(endpoint).expect("connect production listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("write timeout");
    stream
        .write_all(&handshake.write_next().expect("message 1"))
        .expect("send message 1");
    let response = read_frame(&mut stream);
    handshake
        .read_next(&response, unix_seconds())
        .expect("read message 2");
    stream
        .write_all(&handshake.write_next().expect("message 3"))
        .expect("send message 3");
    let mut session = handshake.into_session().expect("establish session");

    let nonce = [0x5a_u8; 16];
    let probe = sign_probe(identity, session.session_id(), nonce, unix_seconds())
        .expect("sign production probe");
    let frame = session
        .write(ENVELOPE_KIND, &probe.encoded())
        .expect("encrypt probe");
    stream.write_all(&frame).expect("send probe");

    let ack_frame = read_frame(&mut stream);
    let ack = session.read(&ack_frame).expect("decrypt ack");
    assert_eq!(ack.kind, ENVELOPE_KIND);
    verify_envelope(
        &ack.body,
        remote_node_id,
        remote_identity_key,
        "ack",
        session.session_id(),
        &nonce,
    )
    .expect("the production listener must acknowledge an authorized peer");
    (stream, session)
}

fn identity_key_bytes(status: &Value) -> [u8; 32] {
    let text = status["identity"]["public_key"].as_str().unwrap();
    let bytes: Vec<u8> = (0..text.len() / 2)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
        .collect();
    bytes.try_into().expect("identity key length")
}

fn audit_totals(workspace: &Path) -> (i64, i64) {
    let connection = Connection::open(workspace.join(".node-state/node.sqlite"))
        .expect("open live node registry");
    let accepted: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM transport_audit WHERE outcome = 'accepted'",
            [],
            |row| row.get(0),
        )
        .expect("count accepted audits");
    let rejected: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM transport_audit WHERE outcome = 'rejected'",
            [],
            |row| row.get(0),
        )
        .expect("count rejected audits");
    (accepted, rejected)
}

fn trust_snapshot(workspace: &Path) -> String {
    let connection = Connection::open(workspace.join(".node-state/node.sqlite"))
        .expect("open live node registry");
    let mut statement = connection
        .prepare(
            "SELECT r.node_id, r.state, p.state, p.role, p.capabilities
             FROM remote_identities r
             LEFT JOIN trusted_peers p ON p.node_id = r.node_id
             ORDER BY r.node_id",
        )
        .expect("prepare trust snapshot");
    let rows = statement
        .query_map([], |row| {
            let capabilities: Option<Vec<u8>> = row.get(4)?;
            Ok(format!(
                "{}|{}|{}|{}|{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                row.get::<_, Option<i64>>(3)?.unwrap_or_default(),
                capabilities
                    .map(|value| String::from_utf8_lossy(&value).into_owned())
                    .unwrap_or_default()
            ))
        })
        .expect("query trust snapshot")
        .map(|row| row.expect("trust row"))
        .collect::<Vec<_>>();
    rows.join("\n")
}

// ---------------------------------------------------------------------------
// The probe.
// ---------------------------------------------------------------------------

#[test]
fn health_plane_reaches_the_production_listener_and_authorization_is_enforceable() {
    let conductor = support::TestWorkspace::new("health_plane_conductor");
    let performer = support::TestWorkspace::new("health_plane_performer");
    let conductor_status = init_node(conductor.path());
    let performer_status = init_node(performer.path());
    let conductor_id = conductor_status["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();
    let performer_id = performer_status["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();

    // The frozen identity construction is untouched: the certificate is the
    // same 245-byte record and the node ID still derives from the x-only key.
    let certificate_bytes =
        std::fs::read(performer.path().join(".node-state/transport.cert")).expect("certificate");
    assert_eq!(certificate_bytes.len(), TRANSPORT_CERTIFICATE_BYTES);
    let mut derivation = b"omakure/node-id/v1\0".to_vec();
    derivation.extend_from_slice(&identity_key_bytes(&performer_status));
    assert_eq!(
        performer_id,
        format!("omk1_{}", hex(Sha256::digest(derivation).as_slice()))
    );

    // The Conductor trusts the Performer with both Health Plane capabilities;
    // the Performer trusts the Conductor with none.
    trust_peer(
        conductor.path(),
        performer.path(),
        &performer_status,
        "performer",
        &[CAPABILITY_PROFILE_PULSE, CAPABILITY_SIGNAL],
    );
    trust_peer(
        performer.path(),
        conductor.path(),
        &conductor_status,
        "conductor",
        &[],
    );

    let conductor_port = support::unique_loopback_port().to_string();
    let conductor_server = support::HttpServer::start_node_service(
        conductor.path(),
        TOKEN,
        &[
            "--workers",
            "0",
            "--no-scheduler",
            "--direct-bind",
            &format!("127.0.0.1:{conductor_port}"),
        ],
        &[],
        Duration::from_secs(20),
    );
    let endpoint = format!("127.0.0.1:{conductor_port}");

    let trust_before = trust_snapshot(conductor.path());
    let (accepted_before, rejected_before) = audit_totals(conductor.path());

    // 1. Reach the production listener over the real handshake and probe path.
    let (performer_identity, performer_private, performer_certificate) =
        node_material(performer.path());
    let (mut stream, mut session) = production_session(
        &endpoint,
        performer_private,
        performer_certificate,
        &performer_identity,
        &conductor_id,
        &identity_key_bytes(&conductor_status),
    );

    // 2. Carry every Performer-to-Conductor Health Plane kind over that live
    //    production session. The listener has no Health Plane handler yet, so
    //    the contracted expectation is that the frames traverse the full
    //    production path and are discarded without any state mutation.
    for (seed, kind) in ["health_profile", "health_pulse", "health_signal"]
        .into_iter()
        .enumerate()
    {
        let now = unix_seconds();
        let encoded = sign_health_envelope(
            &performer_identity,
            kind,
            &session,
            [0xa0 + seed as u8; 16],
            &conductor_id,
            0x10 + seed as u8,
            now,
        );
        assert!(
            encoded.len() <= 2_112,
            "{kind} exceeded the frozen encoded cap: {}",
            encoded.len()
        );

        // The frozen verifier accepts the Health Plane envelope unchanged.
        let nonce = omakure::direct_transport::envelope_nonce(&encoded).expect("nonce");
        verify_envelope(
            &encoded,
            &performer_id,
            &identity_key_bytes(&performer_status),
            kind,
            session.session_id(),
            &nonce,
        )
        .unwrap_or_else(|error| {
            panic!("the frozen verifier rejected a canonical {kind} envelope: {error}")
        });

        let frame = session
            .write(ENVELOPE_KIND, &encoded)
            .expect("encrypt health plane envelope");
        stream
            .write_all(&frame)
            .expect("deliver health plane envelope to the production listener");
    }

    // Give the listener a bounded moment to process the delivered frames.
    std::thread::sleep(Duration::from_millis(500));

    // 3. No Health Plane message mutated identity, trust, capability, or
    //    revocation state on the production node.
    assert_eq!(
        trust_snapshot(conductor.path()),
        trust_before,
        "a Health Plane message must never mutate trust state"
    );
    let (accepted_after, rejected_after) = audit_totals(conductor.path());
    assert_eq!(
        accepted_after,
        accepted_before + 1,
        "exactly one probe acceptance should be audited"
    );
    assert_eq!(
        rejected_after, rejected_before,
        "Health Plane frames must not produce a transport rejection on an authorized session"
    );

    // 4. Authorization is decidable from the live registry for every kind.
    let row = trust_row(conductor.path(), &performer_id).expect("live trusted peer row");
    assert_eq!(row.role, ROLE_PERFORMER);
    assert_eq!(
        row.capabilities,
        vec![
            CAPABILITY_PROFILE_PULSE.to_string(),
            CAPABILITY_SIGNAL.to_string()
        ],
        "the production trust path must persist Health Plane capabilities"
    );
    for kind in ["health_profile", "health_pulse", "health_signal"] {
        assert_eq!(decide(Some(&row), kind), Decision::Allow, "{kind}");
    }
    for kind in ["health_ack", "health_error"] {
        assert_eq!(decide(Some(&row), kind), Decision::WrongRole, "{kind}");
    }

    // The Performer side authorizes the Conductor for acknowledgements only.
    let conductor_row = trust_row(performer.path(), &conductor_id).expect("live conductor row");
    assert_eq!(conductor_row.role, ROLE_CONDUCTOR);
    assert!(conductor_row.capabilities.is_empty());
    for kind in ["health_ack", "health_error"] {
        assert_eq!(
            decide(Some(&conductor_row), kind),
            Decision::Allow,
            "{kind}"
        );
    }
    for kind in ["health_profile", "health_pulse", "health_signal"] {
        assert_eq!(
            decide(Some(&conductor_row), kind),
            Decision::WrongRole,
            "{kind}"
        );
    }

    // 5. Removing a capability through the production CLI denies exactly the
    //    kinds that require it, with no other change.
    let _ = stream.shutdown(std::net::Shutdown::Both);
    let exit = conductor_server.terminate();
    assert!(exit.success() || exit.code().is_none());

    assert_success(&run_node(
        conductor.path(),
        &[
            "capabilities".to_string(),
            performer_id.clone(),
            "--capability".to_string(),
            CAPABILITY_PROFILE_PULSE.to_string(),
            "--actor".to_string(),
            "health-plane-feasibility".to_string(),
            "--reason".to_string(),
            "drop notifications".to_string(),
            "--confirmed".to_string(),
        ],
    ));
    let row = trust_row(conductor.path(), &performer_id).expect("row after capability change");
    assert_eq!(row.capabilities, vec![CAPABILITY_PROFILE_PULSE.to_string()]);
    assert_eq!(decide(Some(&row), "health_profile"), Decision::Allow);
    assert_eq!(decide(Some(&row), "health_pulse"), Decision::Allow);
    assert_eq!(
        decide(Some(&row), "health_signal"),
        Decision::MissingCapability
    );

    // 6. Revocation through the production CLI denies every kind.
    assert_success(&run_node(
        conductor.path(),
        &[
            "revoke".to_string(),
            performer_id.clone(),
            "--actor".to_string(),
            "health-plane-feasibility".to_string(),
            "--reason".to_string(),
            "feasibility revocation".to_string(),
            "--confirmed".to_string(),
        ],
    ));
    let row = trust_row(conductor.path(), &performer_id).expect("row after revocation");
    assert_eq!(row.trust_state, "revoked");
    for kind in [
        "health_profile",
        "health_pulse",
        "health_signal",
        "health_ack",
        "health_error",
    ] {
        assert_eq!(decide(Some(&row), kind), Decision::Revoked, "{kind}");
    }

    // 7. An unknown peer is denied before role or capability is considered.
    assert_eq!(decide(None, "health_pulse"), Decision::Revoked);
}

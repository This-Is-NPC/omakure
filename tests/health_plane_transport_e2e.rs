//! Wave 3 certification: Profile and Pulse over the production direct
//! transport, and the bounded fleet-status projection both adapters render.
//!
//! Nothing here simulates the transport. Two independently stateful `node
//! serve` processes complete the shipped Noise handshake against each other,
//! and every Health Plane message crosses a real encrypted session. The
//! adversary half drives raw production sessions from a third node so each
//! contracted rejection is proven end to end.
//!
//! Time is handled two ways, both deliberate:
//!
//! * Everything that can be observed quickly (reaching `online`, adapter
//!   agreement, reconnect recovery, revocation) uses a bounded real wait.
//! * The presence windows, which are 90 and 600 seconds wide, are exercised by
//!   replaying the *real* state the *real* exchange produced through the
//!   production projection over an injected clock, at the exact frozen
//!   boundary seconds. No test sleeps for ten minutes and no window is
//!   approximated.

mod support;

use omakure::direct_transport::{
    sign_health_envelope, sign_probe, unix_seconds, verify_envelope, HandshakeRole, NoiseHandshake,
    TransportCertificate, TransportSession, ENVELOPE_KIND,
};
use omakure::health_plane::bounds::{
    MAX_AGE_SECONDS, MAX_CANONICAL_PROFILE, MAX_FUTURE_SKEW_SECONDS,
    MAX_MESSAGES_PER_PEER_PER_MINUTE, PRESENCE_ONLINE_SECONDS, PRESENCE_STALE_SECONDS,
    RATE_BURST_ALLOWANCE,
};
use omakure::health_plane::model::{HealthCode, Presence};
use omakure::health_plane::{HealthClock, HealthPlane};
use omakure::node::{NodeContext, NodePathOverrides, NodePlatform};
use omakure::node_identity::NodeIdentity;
use omakure::node_registry::NodeRegistry;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

const TOKEN: &str = "health-plane-transport-token-with-enough-entropy-01";
const CAPABILITY_PROFILE_PULSE: &str = "inventory-health";
/// How long a bounded real wait may run before the certification fails.
const REACH_TIMEOUT: Duration = Duration::from_secs(45);

// ---------------------------------------------------------------------------
// Node lifecycle helpers.
// ---------------------------------------------------------------------------

fn run_node(workspace: &Path, args: &[String]) -> Output {
    Command::new(support::omakure_bin())
        .arg("--scripts-dir")
        .arg(workspace)
        .arg("--json")
        .arg("node")
        .arg("--node-state-dir")
        .arg(workspace.join(".node-state"))
        .arg("--node-config")
        .arg(workspace.join("node.toml"))
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
        "health-plane-transport".to_string(),
        "--reason".to_string(),
        "health plane wave 3 certification".to_string(),
        "--confirmed".to_string(),
    ];
    for capability in capabilities {
        args.push("--capability".to_string());
        args.push((*capability).to_string());
    }
    assert_eq!(
        assert_success(&run_node(workspace, &args))["state"],
        "active"
    );
}

fn configure_direct(workspace: &Path, direct_port: u16, peer_node_id: &str, peer_port: u16) {
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
    std::fs::write(path, config).expect("write direct transport config");
}

fn serve(workspace: &Path) -> support::HttpServer {
    support::HttpServer::start_node_service(
        workspace,
        TOKEN,
        &[
            "--workers",
            "1",
            "--no-scheduler",
            "--capability",
            "node:read",
        ],
        &[],
        Duration::from_secs(20),
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn identity_key_bytes(status: &Value) -> [u8; 32] {
    let text = status["identity"]["public_key"].as_str().unwrap();
    (0..32)
        .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
        .collect::<Vec<u8>>()
        .try_into()
        .expect("identity key length")
}

// ---------------------------------------------------------------------------
// Fleet-status readers: the two shipped adapters over one operation.
// ---------------------------------------------------------------------------

fn fleet_status_http(server: &support::HttpServer) -> Value {
    let response = server.get("/v1/node/health");
    assert_eq!(response.status, 200, "body: {}", response.safe_body());
    response.json()["data"].clone()
}

fn fleet_status_cli(workspace: &Path) -> Value {
    assert_success(&run_node(workspace, &["health".to_string()]))
}

fn node_row<'a>(status: &'a Value, node_id: &str) -> Option<&'a Value> {
    status["nodes"]
        .as_array()?
        .iter()
        .find(|node| node["node_id"] == node_id)
}

/// Poll until `node_id` reports a Pulse sequence strictly above `sequence`.
fn wait_for_pulse_after(server: &support::HttpServer, node_id: &str, sequence: u64) -> Value {
    let deadline = Instant::now() + REACH_TIMEOUT;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        last = fleet_status_http(server);
        if let Some(node) = node_row(&last, node_id) {
            if node["pulse"]["sequence"]
                .as_u64()
                .is_some_and(|next| next > sequence)
            {
                return last;
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("node {node_id} never reported a Pulse after sequence {sequence}; last: {last}");
}

/// Poll the Conductor's own HTTP projection until `node_id` reaches `presence`.
fn wait_for_presence(server: &support::HttpServer, node_id: &str, presence: &str) -> Value {
    let deadline = Instant::now() + REACH_TIMEOUT;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        last = fleet_status_http(server);
        if let Some(node) = node_row(&last, node_id) {
            if node["presence"] == presence {
                return last;
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("node {node_id} never reached presence {presence}; last status: {last}");
}

// ---------------------------------------------------------------------------
// Raw production client, for the adversary half.
// ---------------------------------------------------------------------------

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

/// Complete a real production handshake and probe/ack round trip, then hand
/// back the live session.
fn production_session(
    endpoint: &str,
    workspace: &Path,
    remote_node_id: &str,
    remote_identity_key: &[u8; 32],
) -> (TcpStream, TransportSession, NodeIdentity) {
    let (identity, private, certificate) = node_material(workspace);
    let mut handshake = NoiseHandshake::new(HandshakeRole::Initiator, private, certificate)
        .expect("build production Noise handshake");
    let mut stream = TcpStream::connect(endpoint).expect("connect production listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
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
    let probe = sign_probe(&identity, session.session_id(), nonce, unix_seconds())
        .expect("sign production probe");
    let frame = session
        .write(ENVELOPE_KIND, &probe.encoded())
        .expect("encrypt probe");
    stream.write_all(&frame).expect("send probe");
    let ack_frame = read_frame(&mut stream);
    let ack = session.read(&ack_frame).expect("decrypt ack");
    verify_envelope(
        &ack.body,
        remote_node_id,
        remote_identity_key,
        "ack",
        session.session_id(),
        &nonce,
    )
    .expect("the production listener must acknowledge an authorized peer");
    (stream, session, identity)
}

fn message_id(seed: u8) -> String {
    hex(&[seed; 16])
}

fn profile_payload(target: &str, seed: u8, revision: u64) -> Value {
    json!({
        "health_version": 1,
        "message_id": message_id(seed),
        "target": target,
        "profile": {
            "agent_version": "0.3.0",
            "arch": "x86_64",
            "capabilities": [CAPABILITY_PROFILE_PULSE],
            "display_name": "adversary",
            "distro_id": "arch",
            "distro_version": "rolling",
            "omarchy_channel": "stable",
            "omarchy_version": "2.1.0",
            "platform": "linux",
            "profile_revision": revision,
            "role": "performer",
            "runtimes": []
        }
    })
}

/// What one live exchange observed.
struct Exchange {
    /// The `health_ack` or `health_error` addressed to the message just sent.
    reply: Option<(String, Value)>,
    /// Every other Health Plane envelope the peer sent on the same session.
    ///
    /// A node that trusts its peer in the `conductor` role reports its own
    /// Profile and Pulse on the same session it receives on, so a certification
    /// that assumed one frame per send would read the wrong envelope.
    unrelated: Vec<String>,
}

/// Send one Health Plane envelope over a live production session and collect
/// the reply the Conductor chose, if it chose to reply at all.
fn exchange(
    stream: &mut TcpStream,
    session: &mut TransportSession,
    identity: &NodeIdentity,
    kind: &str,
    payload: Value,
    created_at: u64,
    nonce_seed: u8,
) -> Exchange {
    let sent_message_id = payload["message_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let envelope = sign_health_envelope(
        identity,
        kind,
        session.session_id(),
        [nonce_seed; 16],
        payload,
        created_at,
    )
    .expect("sign health envelope");
    let frame = session
        .write(ENVELOPE_KIND, &envelope.encoded())
        .expect("encrypt health envelope");
    stream.write_all(&frame).expect("send health envelope");

    let deadline = Instant::now() + Duration::from_secs(4);
    let mut unrelated = Vec::new();
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        stream
            .set_read_timeout(Some(remaining.max(Duration::from_millis(50))))
            .expect("reply timeout");
        let mut prefix = [0_u8; 4];
        if stream.read_exact(&mut prefix).is_err() {
            break;
        }
        let length = u32::from_be_bytes(prefix) as usize;
        let mut encoded = vec![0_u8; length + 4];
        encoded[..4].copy_from_slice(&prefix);
        stream
            .read_exact(&mut encoded[4..])
            .expect("read reply body");
        let message = session.read(&encoded).expect("decrypt reply");
        let value: Value = serde_json::from_slice(&message.body[..message.body.len() - 64])
            .expect("parse reply envelope");
        let reply_kind = value["kind"].as_str().unwrap_or_default().to_string();
        let payload = value["payload"].clone();
        let acked = match reply_kind.as_str() {
            "health_ack" => payload["ack"]["acked_message_id"].as_str(),
            "health_error" => payload["error"]["acked_message_id"].as_str(),
            _ => None,
        };
        if acked == Some(sent_message_id.as_str()) {
            return Exchange {
                reply: Some((reply_kind, payload)),
                unrelated,
            };
        }
        unrelated.push(reply_kind);
    }
    Exchange {
        reply: None,
        unrelated,
    }
}

/// Every Health Plane audit code the Conductor has recorded so far.
///
/// The rows are redacted by construction: the schema stores only the stable
/// code, the peer node ID, the message kind, the byte count, and the outcome.
fn health_audit_codes(workspace: &Path) -> Vec<i64> {
    let connection = Connection::open(workspace.join(".node-state/node.sqlite"))
        .expect("open live node registry");
    let mut statement = connection
        .prepare("SELECT error_code FROM health_audit WHERE error_code IS NOT NULL")
        .expect("prepare health audit query");
    let codes = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .expect("query health audit")
        .map(|row| row.expect("audit row"))
        .collect();
    codes
}

/// Assert the frozen "drop, audit, and send nothing" outcome.
///
/// The contract emits a `health_error` only once the sender is authenticated,
/// authorized, *and* target-bound. Every rejection before that point - size,
/// schema, unknown field, and wrong target - is a silent drop with an audit
/// row, so that an unauthorized or misaddressed sender learns nothing.
fn assert_dropped(exchange: Exchange, workspace: &Path, code: HealthCode, label: &str) {
    if let Some((kind, payload)) = exchange.reply {
        panic!("{label} must be dropped silently, but got {kind}: {payload}");
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if health_audit_codes(workspace).contains(&i64::from(code.code())) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "{label} was dropped but never audited as {} ({}); codes: {:?}",
        code.name(),
        code.code(),
        health_audit_codes(workspace)
    );
}

fn assert_error(exchange: Exchange, code: HealthCode) {
    let (kind, payload) = exchange.reply.unwrap_or_else(|| {
        panic!(
            "expected a bounded {} rejection, got no reply at all",
            code.name()
        )
    });
    assert_eq!(kind, "health_error", "payload: {payload}");
    assert_eq!(payload["error"]["code"], code.code(), "payload: {payload}");
    assert_eq!(payload["error"]["reason"], code.name());
    assert_eq!(payload["error"]["accepted"], false);
    // The frozen rule: an error carries only the stable code and its name.
    let fields: Vec<&String> = payload["error"]
        .as_object()
        .expect("error object")
        .keys()
        .collect();
    assert_eq!(fields.len(), 4, "error body must stay bounded: {payload}");
}

// ---------------------------------------------------------------------------
// Live trust and state inspection.
// ---------------------------------------------------------------------------

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
    statement
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
        .collect::<Vec<_>>()
        .join("\n")
}

/// Ask a running node service to stop and assert it exits cleanly and promptly.
///
/// On Unix this is a real SIGTERM against the live process, which is the only
/// way to prove the Health Plane tick loop honors cancellation rather than
/// holding the shutdown open until its next cadence. Elsewhere the portable
/// terminate path is used, which still proves the process is reaped.
fn stop_cleanly(server: &mut Option<support::HttpServer>) {
    let Some(mut running) = server.take() else {
        return;
    };
    #[cfg(unix)]
    {
        let pid = running.child_id();
        // SAFETY: libc::kill is the standard way to signal a child process.
        let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        assert_eq!(rc, 0, "kill(SIGTERM) failed");
        let started = Instant::now();
        // The deadline is deliberately longer than the assertion below: if the
        // service ever held shutdown open, `wait_exit` would fall back to a
        // hard kill at 10 s and the elapsed assertion would catch it rather
        // than the run silently passing on a killed process.
        let status = running.wait_exit(Duration::from_secs(10));
        assert!(
            status.success() || status.code() == Some(0) || status.code().is_none(),
            "a reporting node must stop cleanly, got {status:?}"
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(8),
            "a reporting node held shutdown open for {elapsed:?}; the Health Plane \
             tick must not park cancellation behind its cadence"
        );
    }
    #[cfg(not(unix))]
    {
        let _ = running.terminate();
    }
}

/// A settable clock, so the frozen presence windows are exercised exactly.
#[derive(Debug)]
struct FixedClock(AtomicI64);

impl HealthClock for FixedClock {
    fn unix_seconds(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }

    fn monotonic_millis(&self) -> u64 {
        0
    }
}

/// Replay the real state a real exchange produced through the production
/// projection at one chosen second.
fn presence_at(workspace: &Path, node_id: &str, now: i64) -> Presence {
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
    let registry =
        NodeRegistry::open_existing(&context, identity.public_status()).expect("open registry");
    let plane = HealthPlane::with_clock(&registry, Box::new(FixedClock(AtomicI64::new(now))));
    plane
        .node_status(node_id)
        .expect("project node status")
        .unwrap_or_else(|| panic!("no Health Plane state for {node_id}"))
        .presence
}

fn last_pulse_at(workspace: &Path, node_id: &str) -> i64 {
    let connection = Connection::open(workspace.join(".node-state/node.sqlite"))
        .expect("open live node registry");
    connection
        .query_row(
            "SELECT last_pulse_at FROM health_peers WHERE node_id = ?1",
            [node_id],
            |row| row.get(0),
        )
        .expect("read the accepted pulse timestamp")
}

// ---------------------------------------------------------------------------
// The SMART gate.
// ---------------------------------------------------------------------------

#[test]
fn two_real_nodes_exchange_profile_and_pulse_and_both_adapters_agree() {
    let conductor = support::TestWorkspace::new("health_tx_conductor");
    let performer = support::TestWorkspace::new("health_tx_performer");
    let bystander = support::TestWorkspace::new("health_tx_bystander");
    let conductor_status = init_node(conductor.path());
    let performer_status = init_node(performer.path());
    let bystander_status = init_node(bystander.path());
    let conductor_id = conductor_status["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();
    let performer_id = performer_status["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();
    let bystander_id = bystander_status["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();

    // The Conductor manages one Performer with the frozen Profile/Pulse
    // capability, and separately trusts a peer that never runs at all: that
    // peer is the "never seen" row the projection must still report.
    trust_peer(
        conductor.path(),
        performer.path(),
        &performer_status,
        "performer",
        &[CAPABILITY_PROFILE_PULSE],
    );
    trust_peer(
        conductor.path(),
        bystander.path(),
        &bystander_status,
        "performer",
        &[CAPABILITY_PROFILE_PULSE],
    );
    trust_peer(
        performer.path(),
        conductor.path(),
        &conductor_status,
        "conductor",
        &[CAPABILITY_PROFILE_PULSE],
    );

    let conductor_port = support::unique_loopback_port();
    let performer_port = support::unique_loopback_port();
    configure_direct(
        conductor.path(),
        conductor_port,
        &performer_id,
        performer_port,
    );
    configure_direct(
        performer.path(),
        performer_port,
        &conductor_id,
        conductor_port,
    );

    let conductor_server = serve(conductor.path());

    // Before the Performer ever runs, both trusted peers are visible with the
    // frozen `unknown` presence and no Profile or Pulse.
    let cold = fleet_status_http(&conductor_server);
    assert_eq!(cold["enabled"], true);
    assert_eq!(cold["presence"]["unknown"], 2);
    assert_eq!(cold["presence"]["online"], 0);
    for node_id in [&performer_id, &bystander_id] {
        let row = node_row(&cold, node_id).expect("trusted peer row");
        assert_eq!(row["presence"], "unknown");
        assert!(row["profile"].is_null());
        assert!(row["pulse"].is_null());
        assert!(row["last_pulse_at"].is_null());
    }

    let mut performer_server = Some(serve(performer.path()));

    // 1. SMART: the Performer reaches `online` inside the Wave 1 window.
    let started = Instant::now();
    let online = wait_for_presence(&conductor_server, &performer_id, "online");
    let reached = started.elapsed();
    assert!(
        reached.as_secs() as i64 <= PRESENCE_ONLINE_SECONDS,
        "reaching online took {reached:?}, beyond the frozen {PRESENCE_ONLINE_SECONDS}s window"
    );

    let row = node_row(&online, &performer_id).expect("performer row");
    assert_eq!(row["role"], "performer");
    assert_eq!(row["trust_state"], "active");
    assert_eq!(row["capabilities"], json!([CAPABILITY_PROFILE_PULSE]));
    assert_eq!(row["profile"]["role"], "performer");
    assert_eq!(row["profile"]["platform"], "linux");
    assert!(row["profile"]["profile_revision"].as_u64().unwrap() >= 1);
    assert_eq!(row["pulse"]["runner"]["scheduler"], "disabled");
    assert_eq!(row["pulse"]["runner"]["workers_configured"], 1);
    assert!(row["pulse"]["sequence"].as_u64().unwrap() >= 1);
    assert_eq!(
        row["pulse"]["emitted_at"], row["pulse"]["sequence"],
        "the frozen contract binds emitted_at to the pulse sequence"
    );
    // The never-seen peer is unaffected by the other peer reporting.
    assert_eq!(
        node_row(&online, &bystander_id).expect("bystander row")["presence"],
        "unknown"
    );

    // 2. Both shipped adapters render one operation, so they agree exactly.
    let via_cli = fleet_status_cli(conductor.path());
    assert_eq!(via_cli["local_node_id"], conductor_id);
    assert_eq!(via_cli["enabled"], true);
    for node_id in [&performer_id, &bystander_id] {
        let http_row = node_row(&online, node_id).expect("http row");
        let cli_row = node_row(&via_cli, node_id).expect("cli row");
        for field in [
            "node_id",
            "role",
            "capabilities",
            "trust_state",
            "profile",
            "pulse",
            "signal_cursor",
            "stored_signals",
            "held_signals",
            "version_incompatible",
        ] {
            assert_eq!(
                http_row[field], cli_row[field],
                "CLI and HTTP disagreed on `{field}` for {node_id}"
            );
        }
    }

    // 3. Nothing privacy class P1 reached the projection.
    let rendered = via_cli.to_string();
    for forbidden in [
        "127.0.0.1",
        "/home/",
        "secret://",
        conductor.path().to_string_lossy().as_ref(),
    ] {
        assert!(
            !rendered.contains(forbidden),
            "fleet status leaked `{forbidden}`"
        );
    }

    // 4. Cancellation, then isolation. The Performer is asked to stop while a
    //    Health Plane session is live and reporting, and must exit cleanly
    //    inside a bounded window: the steady-state loop now waits on a one
    //    second readability tick, so a stop request cannot be parked behind a
    //    long blocking read. The accepted state stays, and the presence
    //    windows are then exercised at their exact frozen boundaries by
    //    replaying the real state through the production projection.
    stop_cleanly(&mut performer_server);
    let pulse_at = last_pulse_at(conductor.path(), &performer_id);
    for (offset, expected) in [
        (0, Presence::Online),
        (PRESENCE_ONLINE_SECONDS, Presence::Online),
        (PRESENCE_ONLINE_SECONDS + 1, Presence::Stale),
        (PRESENCE_STALE_SECONDS, Presence::Stale),
        (PRESENCE_STALE_SECONDS + 1, Presence::Offline),
    ] {
        assert_eq!(
            presence_at(conductor.path(), &performer_id, pulse_at + offset),
            expected,
            "presence at last_pulse_at + {offset}s must be {expected:?}"
        );
    }

    // 5. Recovery. The Performer restarts and returns to `online`, and its
    //    Pulse sequence strictly advances across the restart, which is what
    //    makes the reporting state survive a process lifetime.
    let previous_sequence = node_row(&online, &performer_id).expect("row")["pulse"]["sequence"]
        .as_u64()
        .expect("pulse sequence");
    let previous_revision = node_row(&online, &performer_id).expect("row")["profile"]
        ["profile_revision"]
        .as_u64()
        .expect("profile revision");
    let mut performer_server = Some(serve(performer.path()));
    let recovered = wait_for_pulse_after(&conductor_server, &performer_id, previous_sequence);
    let row = node_row(&recovered, &performer_id).expect("performer row");
    assert_eq!(
        row["presence"], "online",
        "a reconnected Performer must be online again"
    );
    assert!(
        row["pulse"]["sequence"].as_u64().unwrap() > previous_sequence,
        "a restarted Performer must not replay a sequence the Conductor already accepted"
    );
    assert!(
        row["profile"]["profile_revision"].as_u64().unwrap() >= previous_revision,
        "a restart must never move profile_revision backwards"
    );

    // 6. Revocation is immediate: the peer leaves the projection at once and
    //    its Health Plane state is purged, without touching the other peer.
    assert_success(&run_node(
        conductor.path(),
        &[
            "revoke".to_string(),
            performer_id.clone(),
            "--actor".to_string(),
            "health-plane-transport".to_string(),
            "--reason".to_string(),
            "wave 3 certification".to_string(),
            "--confirmed".to_string(),
        ],
    ));
    let after = fleet_status_http(&conductor_server);
    assert!(
        node_row(&after, &performer_id).is_none(),
        "a revoked peer must leave the fleet projection immediately: {after}"
    );
    assert_eq!(
        node_row(&after, &bystander_id).expect("bystander row")["presence"],
        "unknown",
        "revoking one peer must not disturb another"
    );

    stop_cleanly(&mut performer_server);
    let _ = conductor_server.terminate();
}

// ---------------------------------------------------------------------------
// The adversary half.
// ---------------------------------------------------------------------------

#[test]
fn contracted_adversaries_are_rejected_without_unauthorized_state_mutation() {
    let conductor = support::TestWorkspace::new("health_adv_conductor");
    let performer = support::TestWorkspace::new("health_adv_performer");
    let manager = support::TestWorkspace::new("health_adv_manager");
    let conductor_status = init_node(conductor.path());
    let performer_status = init_node(performer.path());
    let manager_status = init_node(manager.path());
    let conductor_id = conductor_status["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();
    let performer_id = performer_status["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();
    let manager_id = manager_status["identity"]["node_id"]
        .as_str()
        .unwrap()
        .to_string();

    // One authorized Performer, and one peer trusted in the *conductor* role
    // that will try to report health it is not permitted to report.
    trust_peer(
        conductor.path(),
        performer.path(),
        &performer_status,
        "performer",
        &[CAPABILITY_PROFILE_PULSE],
    );
    trust_peer(
        conductor.path(),
        manager.path(),
        &manager_status,
        "conductor",
        &[],
    );

    let conductor_port = support::unique_loopback_port();
    let path = conductor.path().join("node.toml");
    let config = std::fs::read_to_string(&path).expect("read node config");
    std::fs::write(
        &path,
        config.replace(
            "static_peers = [",
            &format!("direct_bind = \"127.0.0.1:{conductor_port}\"\nstatic_peers = ["),
        ),
    )
    .expect("write direct bind");
    let conductor_server = serve(conductor.path());
    let endpoint = format!("127.0.0.1:{conductor_port}");
    let conductor_key = identity_key_bytes(&conductor_status);

    let trust_before = trust_snapshot(conductor.path());

    let (mut stream, mut session, identity) =
        production_session(&endpoint, performer.path(), &conductor_id, &conductor_key);

    // 1. Wrong target: a syntactically valid third-party node ID is rejected
    //    before any state is read or written.
    assert_dropped(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&manager_id, 0x01, 1),
            unix_seconds(),
            0x01,
        ),
        conductor.path(),
        HealthCode::WrongTarget,
        "a third-party target",
    );

    // 2. Future beyond the frozen skew, and stale beyond the frozen age.
    assert_error(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&conductor_id, 0x02, 1),
            unix_seconds() + MAX_FUTURE_SKEW_SECONDS as u64 + 5,
            0x02,
        ),
        HealthCode::Future,
    );
    assert_error(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&conductor_id, 0x03, 1),
            unix_seconds() - MAX_AGE_SECONDS as u64 - 5,
            0x03,
        ),
        HealthCode::Stale,
    );

    // 3. An unknown field anywhere in the closed schema.
    let mut unknown = profile_payload(&conductor_id, 0x04, 1);
    unknown["profile"]["hostname"] = json!("workshop.local");
    assert_dropped(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            unknown,
            unix_seconds(),
            0x04,
        ),
        conductor.path(),
        HealthCode::UnknownField,
        "a smuggled hostname field",
    );

    // 4. A grammar violation that would smuggle a path.
    let mut path_smuggle = profile_payload(&conductor_id, 0x05, 1);
    path_smuggle["profile"]["display_name"] = json!("/etc/shadow");
    assert_dropped(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            path_smuggle,
            unix_seconds(),
            0x05,
        ),
        conductor.path(),
        HealthCode::InvalidMessage,
        "a display name carrying a filesystem path",
    );

    // 5. Oversized: past the frozen per-kind canonical cap.
    let mut oversized = profile_payload(&conductor_id, 0x06, 1);
    oversized["profile"]["runtimes"] = json!((0..64)
        .map(|index| json!({
            "available": true,
            "name": format!("runtime{index}"),
            "version": "9.9.9999999999999999999999"
        }))
        .collect::<Vec<Value>>());
    assert!(
        serde_jcs::to_vec(&oversized).unwrap().len() > MAX_CANONICAL_PROFILE / 2,
        "the oversized fixture must actually be large"
    );
    assert_dropped(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            oversized,
            unix_seconds(),
            0x06,
        ),
        conductor.path(),
        HealthCode::MessageTooLarge,
        "an oversized Profile",
    );

    // 6. An accepted Profile, then the same `message_id` replayed.
    let accepted = exchange(
        &mut stream,
        &mut session,
        &identity,
        "health_profile",
        profile_payload(&conductor_id, 0x07, 1),
        unix_seconds(),
        0x07,
    );
    let (kind, payload) = accepted
        .reply
        .expect("an authorized Profile must be acknowledged");
    assert_eq!(kind, "health_ack", "payload: {payload}");
    assert_eq!(payload["ack"]["accepted"], true);
    assert_eq!(payload["ack"]["acked_message_id"], message_id(0x07));

    assert_error(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&conductor_id, 0x07, 2),
            unix_seconds(),
            0x08,
        ),
        HealthCode::Replay,
    );

    // 7. An out-of-order Profile revision is a replay, not an acceptance.
    assert_error(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&conductor_id, 0x09, 1),
            unix_seconds(),
            0x09,
        ),
        HealthCode::Replay,
    );

    // 8. Flood: past the frozen per-minute allowance plus its burst.
    let mut rate_limited = false;
    for seed in 0..(MAX_MESSAGES_PER_PEER_PER_MINUTE + RATE_BURST_ALLOWANCE + 4) {
        let seed = 0x20 + seed as u8;
        let reply = exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&conductor_id, seed, 100 + seed as u64),
            unix_seconds(),
            seed,
        );
        if let Some((kind, payload)) = reply.reply {
            if kind == "health_error" && payload["error"]["code"] == HealthCode::RateLimited.code()
            {
                rate_limited = true;
                break;
            }
        }
    }
    assert!(
        rate_limited,
        "a flooding peer must hit the frozen per-peer rate limit"
    );
    let _ = stream.shutdown(std::net::Shutdown::Both);

    // 9. Wrong role: a peer trusted as a Conductor cannot report health, and
    //    the rejection is a silent drop rather than an oracle.
    let (mut stream, mut session, identity) =
        production_session(&endpoint, manager.path(), &conductor_id, &conductor_key);
    let wrong_role = exchange(
        &mut stream,
        &mut session,
        &identity,
        "health_profile",
        profile_payload(&conductor_id, 0x60, 1),
        unix_seconds(),
        0x60,
    );
    // The same session proves both halves of the role rule at once: this node
    // reports its own Profile to the peer it trusts as a Conductor, and refuses
    // the Profile that peer sent in the other direction.
    assert!(
        wrong_role
            .unrelated
            .iter()
            .any(|kind| kind == "health_profile"),
        "a node must report to the peer it trusts in the conductor role: {:?}",
        wrong_role.unrelated
    );
    assert_dropped(
        wrong_role,
        conductor.path(),
        HealthCode::WrongRole,
        "a peer trusted as a Conductor reporting health",
    );
    let _ = stream.shutdown(std::net::Shutdown::Both);

    // 10. Revocation: the same authorized Performer, once revoked, is dropped.
    assert_success(&run_node(
        conductor.path(),
        &[
            "revoke".to_string(),
            performer_id.clone(),
            "--actor".to_string(),
            "health-plane-transport".to_string(),
            "--reason".to_string(),
            "adversary certification".to_string(),
            "--confirmed".to_string(),
        ],
    ));
    let revoked_session = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (mut stream, mut session, identity) =
            production_session(&endpoint, performer.path(), &conductor_id, &conductor_key);
        let reply = exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&conductor_id, 0x70, 500),
            unix_seconds(),
            0x70,
        );
        let _ = stream.shutdown(std::net::Shutdown::Both);
        reply.reply
    }));
    // A revoked peer is refused at the transport admission gate or, if it still
    // reaches the Health Plane, dropped without a reply. Both are fail-closed;
    // neither may acknowledge.
    if let Ok(Some((kind, payload))) = revoked_session {
        panic!("a revoked peer received {kind}: {payload}");
    }

    // 11. Nothing an adversary sent changed identity, trust, capability, or
    //     revocation state beyond the one explicit operator revocation.
    let trust_after = trust_snapshot(conductor.path());
    assert_ne!(
        trust_before, trust_after,
        "the explicit operator revocation must be visible"
    );
    assert_eq!(
        trust_after.matches(&performer_id).count(),
        trust_before.matches(&performer_id).count(),
        "no adversary may add or remove a trust row"
    );
    assert!(
        trust_after.contains(&manager_id),
        "the wrong-role peer's trust row must be untouched"
    );
    assert!(
        trust_after.contains("revoked"),
        "only the operator's revocation changed trust"
    );

    let _ = conductor_server.terminate();
}

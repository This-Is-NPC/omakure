//! The contracted Health Plane adversarial matrix, injected over one real
//! production Noise session into a running packaged Conductor.
//!
//! This harness is driven by `.scripts/health-plane-certification.sh` and is
//! ignored by default, because it requires that script's live Compose
//! topology. It never simulates the transport: it completes the shipped
//! `Noise_XX_25519_ChaChaPoly_SHA256` handshake against the Conductor's
//! production direct listener, signs every envelope with the frozen BIP-340
//! construction, and reads the Conductor's durable audit trail out of its real
//! `node.sqlite`.
//!
//! Every expected code below is transcribed from
//! `.docs/health-plane-contract.md`; none of them is chosen here. The frozen
//! reply policy is asserted as well as the code: a `health_error` is emitted
//! only once the sender is authenticated, authorized *and* target-bound, so
//! trust, role, capability, size, schema, and target failures are silent drops
//! with a durable redacted audit row.
//!
//! Run with:
//! `cargo test --test docker_health_plane_adversary -- --ignored --nocapture`

use omakure::direct_transport::{
    sign_health_envelope, sign_probe, unix_seconds, verify_envelope, HandshakeRole, NoiseHandshake,
    TransportCertificate, TransportSession, ENVELOPE_KIND,
};
use omakure::health_plane::bounds::{
    MAX_AGE_SECONDS, MAX_FUTURE_SKEW_SECONDS, MAX_MESSAGES_PER_PEER_PER_MINUTE,
    MIN_PULSE_INTERVAL_SECONDS, RATE_BURST_ALLOWANCE, REORDER_BUFFER_ENTRIES,
    SIGNAL_INBOX_CAPACITY,
};
use omakure::health_plane::model::HealthCode;
use omakure::node::{NodeContext, NodePathOverrides, NodePlatform};
use omakure::node_identity::NodeIdentity;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Read timeout for one production frame. Bounded, never blocking forever.
const FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one exchange waits for the Conductor's reply before concluding the
/// message was dropped. The frozen acknowledgement timeout is 5 seconds.
const REPLY_WINDOW: Duration = Duration::from_secs(6);
/// How long a durable audit row may take to become visible to the host reader.
const AUDIT_WINDOW: Duration = Duration::from_secs(20);
/// How far the simulated backward wall-clock step moves. It stays inside the
/// frozen 120-second freshness window on purpose, so the rejection proves the
/// ordering rule rather than the freshness rule.
const BACKWARD_CLOCK_STEP_SECONDS: u64 = 30;
/// Ceiling on the flood case, so it can never become an unbounded loop.
const FLOOD_CEILING: usize = (MAX_MESSAGES_PER_PEER_PER_MINUTE + RATE_BURST_ALLOWANCE + 8) as usize;

// ---------------------------------------------------------------------------
// Environment plumbing supplied by the certification script.
// ---------------------------------------------------------------------------

fn env(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set by .scripts/health-plane-certification.sh"))
}

fn compose(args: &[&str]) -> std::process::Output {
    let mut command = Command::new("timeout");
    command.args([
        "--foreground",
        "--kill-after=5s",
        "120s",
        "docker",
        "compose",
    ]);
    command.args(["-f", &env("OMAKURE_HP_COMPOSE_FILE")]);
    command.args(["-p", &env("OMAKURE_HP_PROJECT")]);
    command.args(args);
    command.output().expect("run docker compose")
}

/// Perform one explicit, confirmed trust mutation at the Conductor.
///
/// This is the production CLI on the production registry: the harness never
/// edits trust itself, it only asks the Conductor's own operator surface to,
/// so the rejections that follow are the real authorization path.
fn conductor_admin(args: &[&str]) {
    let service = env("OMAKURE_HP_CONDUCTOR_SERVICE");
    let mut full = vec![
        "exec",
        "-T",
        service.as_str(),
        "/usr/local/bin/omakure",
        "--json",
        "node",
    ];
    full.extend_from_slice(args);
    let output = compose(&full);
    assert!(
        output.status.success(),
        "conductor admin command {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A host-side copy of the Conductor's live registry, write-ahead log included.
fn conductor_registry() -> Connection {
    let tmp = PathBuf::from(env("OMAKURE_HP_TMP"));
    let destination = tmp.join("adversary-conductor.sqlite");
    let service = env("OMAKURE_HP_CONDUCTOR_SERVICE");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", destination.display()));
    }
    let source = format!("{service}:/var/lib/omakure/node.sqlite");
    let output = compose(&["cp", &source, destination.to_str().expect("path")]);
    assert!(
        output.status.success(),
        "copying the Conductor registry failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let wal_source = format!("{service}:/var/lib/omakure/node.sqlite-wal");
    let wal_destination = format!("{}-wal", destination.display());
    let _ = compose(&["cp", &wal_source, &wal_destination]);
    Connection::open(&destination).expect("open the copied Conductor registry")
}

fn latest_audit_id() -> i64 {
    conductor_registry()
        .query_row("SELECT COALESCE(MAX(id), 0) FROM health_audit", [], |row| {
            row.get(0)
        })
        .expect("read the Health Plane audit high-water mark")
}

/// One durable, redacted audit row recorded after `after_id` with `code`.
///
/// Bounded: the row must appear inside [`AUDIT_WINDOW`] or the case fails.
fn await_audit(after_id: i64, code: HealthCode, label: &str) -> (String, String, String) {
    let deadline = Instant::now() + AUDIT_WINDOW;
    loop {
        let connection = conductor_registry();
        let row = connection
            .query_row(
                "SELECT event_code, message_kind, outcome FROM health_audit
                 WHERE id > ?1 AND error_code = ?2 ORDER BY id LIMIT 1",
                rusqlite::params![after_id, i64::from(code.code())],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .ok();
        if let Some(row) = row {
            assert!(
                matches!(row.2.as_str(), "rejected" | "dropped"),
                "{label}: audit outcome for {code:?} was {:?}",
                row.2
            );
            return row;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: the Conductor recorded no durable audit row with code {} ({:?}) within {AUDIT_WINDOW:?}",
            code.code(),
            code
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

// ---------------------------------------------------------------------------
// Production Noise client.
// ---------------------------------------------------------------------------

fn node_material(state_dir: &Path) -> (NodeIdentity, [u8; 32], TransportCertificate) {
    // The config must live outside the state directory: the shipped node
    // context refuses overlapping paths.
    let config_path = state_dir.parent().unwrap_or(Path::new(".")).join(format!(
        "{}-harness-node.toml",
        state_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("harness")
    ));
    if !config_path.exists() {
        std::fs::write(&config_path, "version = 1\n").expect("write harness node config");
    }
    let context = NodeContext::resolve_for(
        NodePlatform::current(),
        NodePathOverrides::new(Some(state_dir.to_path_buf()), Some(config_path)),
        true,
        None,
        None,
        None,
    )
    .expect("resolve the harness node context");
    let identity = NodeIdentity::load_existing(&context).expect("load the harness node identity");
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

fn read_frame(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).ok()?;
    let length = u32::from_be_bytes(prefix) as usize;
    let mut encoded = vec![0_u8; length + 4];
    encoded[..4].copy_from_slice(&prefix);
    stream.read_exact(&mut encoded[4..]).ok()?;
    Some(encoded)
}

/// Complete one real handshake and probe/ack round trip against the running
/// Conductor, and hand back the live session.
fn production_session(state_dir: &Path) -> (TcpStream, TransportSession, NodeIdentity) {
    let endpoint = env("OMAKURE_HP_CONDUCTOR_ENDPOINT");
    let conductor_id = env("OMAKURE_HP_CONDUCTOR_ID");
    let conductor_key = decode_key(&env("OMAKURE_HP_CONDUCTOR_KEY"));
    let (identity, private, certificate) = node_material(state_dir);
    let mut handshake = NoiseHandshake::new(HandshakeRole::Initiator, private, certificate)
        .expect("build the production Noise handshake");
    let mut stream = TcpStream::connect(&endpoint).expect("connect the production listener");
    stream
        .set_read_timeout(Some(FRAME_TIMEOUT))
        .expect("read timeout");
    stream
        .set_write_timeout(Some(FRAME_TIMEOUT))
        .expect("write timeout");
    stream
        .write_all(&handshake.write_next().expect("message 1"))
        .expect("send message 1");
    let response = read_frame(&mut stream).expect("read message 2");
    handshake
        .read_next(&response, unix_seconds())
        .expect("read message 2");
    stream
        .write_all(&handshake.write_next().expect("message 3"))
        .expect("send message 3");
    let mut session = handshake.into_session().expect("establish the session");

    let nonce = [0x5a_u8; 16];
    let probe =
        sign_probe(&identity, session.session_id(), nonce, unix_seconds()).expect("sign the probe");
    let frame = session
        .write(ENVELOPE_KIND, &probe.encoded())
        .expect("encrypt the probe");
    stream.write_all(&frame).expect("send the probe");
    let ack_frame = read_frame(&mut stream).expect("read the probe acknowledgement");
    let ack = session
        .read(&ack_frame)
        .expect("decrypt the acknowledgement");
    verify_envelope(
        &ack.body,
        &conductor_id,
        &conductor_key,
        "ack",
        session.session_id(),
        &nonce,
    )
    .expect("the production listener must acknowledge an authorized peer");
    (stream, session, identity)
}

fn decode_key(hex: &str) -> [u8; 32] {
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex byte"))
        .collect();
    bytes.try_into().expect("32-byte key")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn message_id(seed: u8) -> String {
    hex(&[seed; 16])
}

/// What one live exchange observed on the session.
struct Exchange {
    /// The `health_ack` or `health_error` addressed to the message just sent.
    reply: Option<(String, Value)>,
}

/// Send one already-encoded envelope and collect the reply, if any.
fn exchange_encoded(
    stream: &mut TcpStream,
    session: &mut TransportSession,
    encoded: Vec<u8>,
    sent_message_id: &str,
) -> Exchange {
    let frame = session
        .write(ENVELOPE_KIND, &encoded)
        .expect("encrypt the Health Plane envelope");
    stream
        .write_all(&frame)
        .expect("send the Health Plane envelope");

    let deadline = Instant::now() + REPLY_WINDOW;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        stream
            .set_read_timeout(Some(remaining.max(Duration::from_millis(50))))
            .expect("reply timeout");
        let Some(frame) = read_frame(stream) else {
            break;
        };
        let message = session.read(&frame).expect("decrypt the reply");
        let value: Value = serde_json::from_slice(&message.body[..message.body.len() - 64])
            .expect("parse the reply envelope");
        let reply_kind = value["kind"].as_str().unwrap_or_default().to_string();
        let payload = value["payload"].clone();
        let acked = match reply_kind.as_str() {
            "health_ack" => payload["ack"]["acked_message_id"].as_str(),
            "health_error" => payload["error"]["acked_message_id"].as_str(),
            _ => None,
        };
        if acked == Some(sent_message_id) {
            return Exchange {
                reply: Some((reply_kind, payload)),
            };
        }
    }
    Exchange { reply: None }
}

/// Sign and send one Health Plane envelope over the live session.
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
    .expect("sign the Health Plane envelope")
    .encoded();
    exchange_encoded(stream, session, envelope, &sent_message_id)
}

/// Assert the frozen "reply with this stable code" outcome.
fn assert_error(exchange: Exchange, code: HealthCode, label: &str) {
    let (kind, payload) = exchange.reply.unwrap_or_else(|| {
        panic!(
            "{label}: expected a health_error {}, got no reply",
            code.code()
        )
    });
    assert_eq!(kind, "health_error", "{label}: payload {payload}");
    assert_eq!(
        payload["error"]["code"],
        json!(code.code()),
        "{label}: payload {payload}"
    );
    assert_eq!(payload["error"]["accepted"], json!(false), "{label}");
}

/// Assert the frozen "drop silently, audit durably" outcome.
fn assert_dropped(exchange: Exchange, after_id: i64, code: HealthCode, label: &str) {
    if let Some((kind, payload)) = exchange.reply {
        panic!("{label}: must be dropped silently, but the Conductor replied {kind}: {payload}");
    }
    await_audit(after_id, code, label);
}

fn assert_accepted(exchange: Exchange, label: &str) -> u64 {
    let (kind, payload) = exchange
        .reply
        .unwrap_or_else(|| panic!("{label}: an authorized message must be acknowledged"));
    assert_eq!(kind, "health_ack", "{label}: payload {payload}");
    assert_eq!(payload["ack"]["accepted"], json!(true), "{label}");
    payload["ack"]["cursor"].as_u64().expect("cursor")
}

// ---------------------------------------------------------------------------
// Frozen payload shapes.
// ---------------------------------------------------------------------------

fn profile_payload(target: &str, id: &str, revision: u64) -> Value {
    json!({
        "health_version": 1,
        "message_id": id,
        "target": target,
        "profile": {
            "agent_version": "0.3.0",
            "arch": "x86_64",
            "capabilities": ["inventory-health", "notifications"],
            "display_name": "health-adversary",
            "distro_id": "debian",
            "distro_version": "12",
            "omarchy_channel": "",
            "omarchy_version": "",
            "platform": "linux",
            "profile_revision": revision,
            "role": "performer",
            "runtimes": []
        }
    })
}

fn pulse_payload(target: &str, id: &str, sequence: u64, revision: u64, emitted_at: u64) -> Value {
    json!({
        "health_version": 1,
        "message_id": id,
        "target": target,
        "pulse": {
            "emitted_at": emitted_at,
            "last_run": null,
            "profile_revision": revision,
            "runner": {
                "queue_depth": 0,
                "scheduler": "disabled",
                "state": "idle",
                "workers_busy": 0,
                "workers_configured": 1
            },
            "sequence": sequence,
            "uptime_seconds": 60
        }
    })
}

fn signal_payload(
    target: &str,
    id: &str,
    sequence: u64,
    signal_seed: u8,
    occurred_at: u64,
) -> Value {
    json!({
        "health_version": 1,
        "message_id": id,
        "target": target,
        "signal": {
            "kind": "run-completed",
            "occurred_at": occurred_at,
            "run": {
                "exit_code": 0,
                "finished_at": occurred_at,
                "run_id": hex(&[signal_seed; 16]),
                "script": "adversary",
                "state": "completed"
            },
            "sequence": sequence,
            "signal_id": hex(&[signal_seed; 16]),
            "subject": null
        }
    })
}

// ---------------------------------------------------------------------------
// The matrix.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires the live topology started by .scripts/health-plane-certification.sh"]
fn the_contracted_adversarial_matrix_is_rejected_over_production_noise() {
    let state_dir = PathBuf::from(env("OMAKURE_HP_ADVERSARY_STATE"));
    let conductor_id = env("OMAKURE_HP_CONDUCTOR_ID");
    let adversary_id = env("OMAKURE_HP_ADVERSARY_ID");
    let seeded_cursor: u64 = env("OMAKURE_HP_SEEDED_CURSOR")
        .parse()
        .expect("the seeded Signal cursor must be an integer");

    let (mut stream, mut session, identity) = production_session(&state_dir);

    // 1. Wrong target: a syntactically valid third-party node ID is rejected
    //    before any state is read or written, and never answered.
    let mark = latest_audit_id();
    assert_dropped(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&adversary_id, &message_id(0x01), 1),
            unix_seconds(),
            0x01,
        ),
        mark,
        HealthCode::WrongTarget,
        "a Profile addressed to a third party",
    );

    // 2. Future beyond the frozen 60-second skew.
    assert_error(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&conductor_id, &message_id(0x02), 1),
            unix_seconds() + MAX_FUTURE_SKEW_SECONDS as u64 + 5,
            0x02,
        ),
        HealthCode::Future,
        "a Profile beyond the frozen future skew",
    );

    // 3. Stale beyond the frozen 120-second age.
    assert_error(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&conductor_id, &message_id(0x03), 1),
            unix_seconds() - MAX_AGE_SECONDS as u64 - 5,
            0x03,
        ),
        HealthCode::Stale,
        "a Profile beyond the frozen freshness window",
    );

    // 4. An unknown field smuggled into the closed schema.
    let mark = latest_audit_id();
    let mut unknown = profile_payload(&conductor_id, &message_id(0x04), 1);
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
        mark,
        HealthCode::UnknownField,
        "a smuggled hostname field",
    );

    // 5. A grammar violation that would smuggle a filesystem path.
    let mark = latest_audit_id();
    let mut malformed = profile_payload(&conductor_id, &message_id(0x05), 1);
    malformed["profile"]["display_name"] = json!("/etc/shadow");
    assert_dropped(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            malformed,
            unix_seconds(),
            0x05,
        ),
        mark,
        HealthCode::InvalidMessage,
        "a display name carrying a filesystem path",
    );

    // 6. Oversized past the frozen per-kind canonical cap.
    let mark = latest_audit_id();
    let mut oversized = profile_payload(&conductor_id, &message_id(0x06), 1);
    oversized["profile"]["runtimes"] = json!((0..64)
        .map(|index| json!({
            "available": true,
            "name": format!("runtime{index}"),
            "version": "9.9.9999999999999999999999"
        }))
        .collect::<Vec<Value>>());
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
        mark,
        HealthCode::MessageTooLarge,
        "an oversized Profile",
    );

    // 7. A forged signature over an otherwise perfect envelope.
    let mark = latest_audit_id();
    let mut forged = sign_health_envelope(
        &identity,
        "health_profile",
        session.session_id(),
        [0x07; 16],
        profile_payload(&conductor_id, &message_id(0x07), 1),
        unix_seconds(),
    )
    .expect("sign the envelope to forge")
    .encoded();
    let last = forged.len() - 1;
    forged[last] ^= 0x01;
    assert_dropped(
        exchange_encoded(&mut stream, &mut session, forged, &message_id(0x07)),
        mark,
        HealthCode::InvalidMessage,
        "a forged BIP-340 signature",
    );

    // 8. A spoofed sender: the envelope claims to come from the Conductor
    //    itself while riding the adversary's authenticated session.
    let mark = latest_audit_id();
    let spoofed = spoofed_envelope(
        &conductor_id,
        "health_profile",
        session.session_id(),
        [0x08; 16],
        profile_payload(&conductor_id, &message_id(0x08), 1),
        unix_seconds(),
    );
    assert_dropped(
        exchange_encoded(&mut stream, &mut session, spoofed, &message_id(0x08)),
        mark,
        HealthCode::InvalidMessage,
        "an envelope spoofing the Conductor's own identity",
    );

    // 9. An envelope bound to a different session.
    let mark = latest_audit_id();
    let other_session = [0x99_u8; 32];
    let cross_session = sign_health_envelope(
        &identity,
        "health_profile",
        &other_session,
        [0x09; 16],
        profile_payload(&conductor_id, &message_id(0x09), 1),
        unix_seconds(),
    )
    .expect("sign the cross-session envelope")
    .encoded();
    assert_dropped(
        exchange_encoded(&mut stream, &mut session, cross_session, &message_id(0x09)),
        mark,
        HealthCode::Replay,
        "an envelope bound to a different session",
    );

    // 10. An authorized Profile is accepted, and the same `message_id` replayed
    //     immediately afterwards is not.
    let base_revision = unix_seconds();
    assert_accepted(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&conductor_id, &message_id(0x10), base_revision),
            unix_seconds(),
            0x10,
        ),
        "an authorized Profile",
    );
    assert_error(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&conductor_id, &message_id(0x10), base_revision + 1),
            unix_seconds(),
            0x11,
        ),
        HealthCode::Replay,
        "a replayed message_id",
    );

    // 11. A Profile revision that does not strictly increase.
    assert_error(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&conductor_id, &message_id(0x12), base_revision - 1),
            unix_seconds(),
            0x12,
        ),
        HealthCode::Replay,
        "a Profile revision that stepped backwards",
    );

    // 12. An authorized Pulse is accepted, and a Pulse whose sequence stepped
    //     backwards is not. This is the contracted effect of a backward
    //     wall-clock step across a restart: `pulse.sequence` and
    //     `pulse.emitted_at` are both derived from the wall clock, and the
    //     frozen schema requires `emitted_at == created_at`, so a node whose
    //     clock moved backwards emits a well-formed, still-fresh Pulse with a
    //     non-increasing sequence. It fails closed as `health_replay` and
    //     mutates nothing. The step used here is deliberately inside the frozen
    //     120-second freshness window; a larger backward step is simply
    //     rejected earlier as `health_stale`.
    let base_sequence = unix_seconds();
    assert_accepted(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_pulse",
            pulse_payload(
                &conductor_id,
                &message_id(0x13),
                base_sequence,
                base_revision,
                base_sequence,
            ),
            base_sequence,
            0x13,
        ),
        "an authorized Pulse",
    );
    let stored_sequence = stored_pulse_sequence(&adversary_id);
    // The frozen minimum accepted Pulse interval is checked at step 11, before
    // the ordering rules at step 13. Waiting it out is what makes the next
    // rejection prove the ordering rule rather than the rate rule. The wait is
    // the frozen bound itself, not a guess.
    std::thread::sleep(Duration::from_secs(MIN_PULSE_INTERVAL_SECONDS as u64 + 1));
    let stepped_back = base_sequence - BACKWARD_CLOCK_STEP_SECONDS;
    assert_error(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_pulse",
            pulse_payload(
                &conductor_id,
                &message_id(0x14),
                stepped_back,
                base_revision,
                stepped_back,
            ),
            stepped_back,
            0x14,
        ),
        HealthCode::Replay,
        "a Pulse whose wall-clock sequence stepped backwards",
    );
    assert_eq!(
        stored_pulse_sequence(&adversary_id),
        stored_sequence,
        "a backward wall-clock Pulse must not move the stored sequence"
    );

    // 13. A Signal past the frozen reorder buffer is refused outright.
    assert_error(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_signal",
            signal_payload(
                &conductor_id,
                &message_id(0x15),
                seeded_cursor + REORDER_BUFFER_ENTRIES + 1,
                0xa1,
                unix_seconds(),
            ),
            unix_seconds(),
            0x15,
        ),
        HealthCode::Reordered,
        "a Signal past the frozen reorder buffer",
    );

    // 14. A Signal inside the reorder buffer is held, and the acknowledged
    //     cursor does not advance past the gap.
    let cursor = assert_accepted(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_signal",
            signal_payload(
                &conductor_id,
                &message_id(0x16),
                seeded_cursor + 2,
                0xa2,
                unix_seconds(),
            ),
            unix_seconds(),
            0x16,
        ),
        "a Signal held behind a gap",
    );
    assert_eq!(
        cursor, seeded_cursor,
        "the acknowledged cursor must not advance past a gap"
    );

    // 15. The frozen per-Performer inbox is full, so the next in-order Signal
    //     is refused rather than stored.
    assert_eq!(
        stored_signal_count(&adversary_id),
        SIGNAL_INBOX_CAPACITY,
        "the certification script must seed the inbox to its frozen capacity"
    );
    assert_error(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_signal",
            signal_payload(
                &conductor_id,
                &message_id(0x17),
                seeded_cursor + 1,
                0xa3,
                unix_seconds(),
            ),
            unix_seconds(),
            0x17,
        ),
        HealthCode::QueueFull,
        "a Signal arriving at a full inbox",
    );
    assert_eq!(
        stored_signal_count(&adversary_id),
        SIGNAL_INBOX_CAPACITY,
        "a refused Signal must not be stored"
    );

    // 16. Flood: past the frozen per-peer allowance and its burst.
    let mut rate_limited = false;
    for index in 0..FLOOD_CEILING {
        let seed = 0x20_u8.wrapping_add(index as u8);
        let reply = exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(
                &conductor_id,
                &message_id(seed),
                base_revision + 10 + index as u64,
            ),
            unix_seconds(),
            seed,
        );
        if let Some((kind, payload)) = reply.reply {
            if kind == "health_error"
                && payload["error"]["code"] == json!(HealthCode::RateLimited.code())
            {
                rate_limited = true;
                break;
            }
        }
    }
    assert!(
        rate_limited,
        "a flooding peer must hit a frozen Health Plane rate bound within {FLOOD_CEILING} messages"
    );

    // 17. Capability: the Conductor withdraws `notifications`, so this peer may
    //     still report Profile and Pulse but its Signals are refused. Steps 8
    //     and 9 of the frozen receive order run before the rate check, so the
    //     flood above cannot mask this outcome.
    let mark = latest_audit_id();
    conductor_admin(&[
        "capabilities",
        &adversary_id,
        "--capability",
        "inventory-health",
        "--actor",
        "health-certification",
        "--reason",
        "adversarial capability withdrawal",
        "--confirmed",
    ]);
    assert_dropped(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_signal",
            signal_payload(
                &conductor_id,
                &message_id(0x50),
                seeded_cursor + 1,
                0xb1,
                unix_seconds(),
            ),
            unix_seconds(),
            0x50,
        ),
        mark,
        HealthCode::MissingCapability,
        "a Signal from a peer without the frozen notifications capability",
    );

    // 18. Role: a peer the Conductor trusts in the *conductor* role may not
    //     report health at all, and learns nothing from the refusal. It rides
    //     its own real production session, because the shipped `node trust`
    //     refuses to re-register an existing peer and a role is therefore not
    //     something an adversary can flip on a peer it already controls.
    let role_state = PathBuf::from(env("OMAKURE_HP_ROLE_STATE"));
    let mark = latest_audit_id();
    let (mut role_stream, mut role_session, role_identity) = production_session(&role_state);
    assert_eq!(
        role_identity.public_status().node_id,
        env("OMAKURE_HP_ROLE_ID"),
        "the wrong-role session must use the identity the Conductor trusts as a Conductor"
    );
    assert_dropped(
        exchange(
            &mut role_stream,
            &mut role_session,
            &role_identity,
            "health_profile",
            profile_payload(&conductor_id, &message_id(0x51), base_revision + 500),
            unix_seconds(),
            0x51,
        ),
        mark,
        HealthCode::WrongRole,
        "a peer trusted in the conductor role reporting health",
    );
    let _ = role_stream.shutdown(std::net::Shutdown::Both);

    // 19. Revocation on the live session: trust is re-read per message, so an
    //     operator revocation excludes the peer immediately, without waiting
    //     for the session to end.
    let mark = latest_audit_id();
    conductor_admin(&[
        "revoke",
        &adversary_id,
        "--actor",
        "health-certification",
        "--reason",
        "adversarial certification",
        "--confirmed",
    ]);
    assert_dropped(
        exchange(
            &mut stream,
            &mut session,
            &identity,
            "health_profile",
            profile_payload(&conductor_id, &message_id(0x52), base_revision + 600),
            unix_seconds(),
            0x52,
        ),
        mark,
        HealthCode::Revoked,
        "a revoked peer reporting health on an already established session",
    );

    let _ = stream.shutdown(std::net::Shutdown::Both);

    // 20. Nothing an adversary sent produced an unrecognised outcome, and the
    //     audit trail carries only the frozen redacted columns.
    let connection = conductor_registry();
    let unknown_codes: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM health_audit
             WHERE error_code IS NOT NULL AND error_code NOT BETWEEN 1101 AND 1115",
            [],
            |row| row.get(0),
        )
        .expect("scan the audit trail for unstable codes");
    assert_eq!(
        unknown_codes, 0,
        "the Conductor recorded a Health Plane error code outside the frozen 1101-1115 range"
    );
    // No adversarial case may be attributed to an honest Performer. The codes
    // below are the ones an honest Performer can never legitimately produce:
    // malformed, oversized, wrong target, wrong role, missing capability,
    // revoked, queue full, unknown field, and corrupt state. Replay and rate
    // limiting are deliberately excluded, because a legitimate reconnect inside
    // the frozen minimum Pulse interval produces them by design.
    let performer_id = env("OMAKURE_HP_PERFORMER_ID");
    let performer_rejections: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM health_audit
             WHERE node_id = ?1
               AND error_code IN (1102, 1103, 1104, 1105, 1106, 1107, 1113, 1114, 1115)",
            rusqlite::params![performer_id],
            |row| row.get(0),
        )
        .expect("scan the audit trail for collateral rejections");
    assert_eq!(
        performer_rejections, 0,
        "adversarial traffic caused an authorization or validity rejection to be \
         attributed to an honest Performer"
    );
}

// ---------------------------------------------------------------------------
// Registry projections used by the assertions above.
// ---------------------------------------------------------------------------

fn stored_pulse_sequence(node_id: &str) -> i64 {
    conductor_registry()
        .query_row(
            "SELECT COALESCE((SELECT sequence FROM health_pulses WHERE node_id = ?1), 0)",
            rusqlite::params![node_id],
            |row| row.get(0),
        )
        .expect("read the stored Pulse sequence")
}

fn stored_signal_count(node_id: &str) -> i64 {
    conductor_registry()
        .query_row(
            "SELECT COUNT(*) FROM health_signals WHERE node_id = ?1",
            rusqlite::params![node_id],
            |row| row.get(0),
        )
        .expect("read the stored Signal count")
}

/// Build an envelope that claims a `sender` the session did not authenticate.
///
/// The frozen envelope shape is reused verbatim - seven canonical fields, the
/// RFC-8785 encoding - so the rejection can only come from the identity check
/// rather than from a malformed frame. The signature bytes are deliberately
/// meaningless: the sender mismatch is decided before verification.
fn spoofed_envelope(
    sender: &str,
    kind: &str,
    session_id: &[u8; 32],
    nonce: [u8; 16],
    payload: Value,
    created_at: u64,
) -> Vec<u8> {
    let envelope = json!({
        "created_at": created_at,
        "kind": kind,
        "nonce": hex(&nonce),
        "payload": payload,
        "sender": sender,
        "session_id": hex(session_id),
        "version": 1,
    });
    let mut encoded = serde_jcs::to_vec(&envelope).expect("canonicalize the spoofed envelope");
    encoded.extend_from_slice(&[0_u8; 64]);
    encoded
}

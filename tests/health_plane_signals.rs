//! Integration coverage for the closed Health Plane Signal lifecycle
//! (task #2779, wave 4 of plan `health-plane-foundation`).
//!
//! The suite drives a real node: state, identity, and trust are created through
//! the shipped CLI, and every assertion is made against the production registry
//! and the production shared operations reopened over that same `node.sqlite`.
//! Nothing here reimplements storage, authorization, ordering, or retention;
//! those belong to wave 2 and are exercised, not duplicated.
//!
//! Time is injected. No test sleeps, and every window - freshness, the reorder
//! buffer, the rate windows, and the 7-day retention - is exercised at its
//! exact frozen boundary second.

mod support;

use omakure::health_plane::bounds::{
    MAX_AGE_SECONDS, MAX_FUTURE_SKEW_SECONDS, MAX_SIGNALS_PER_PEER_PER_MINUTE,
    RATE_MINUTE_WINDOW_SECONDS, REORDER_BUFFER_ENTRIES, REORDER_BUFFER_SECONDS,
    SIGNAL_INBOX_CAPACITY, SIGNAL_OUTBOX_CAPACITY, SIGNAL_RETENTION_SECONDS, STORAGE_CEILING_BYTES,
};
use omakure::health_plane::model::{HealthCode, HealthDecision, SignalKind};
use omakure::health_plane::{HealthClock, HealthPlane, HealthReply, InboundHealthMessage};
use omakure::node::{NodeContext, NodePathOverrides, NodePlatform};
use omakure::node_identity::NodeIdentity;
use omakure::node_registry::NodeRegistry;
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

const TOKEN: &str = "health-plane-signals-token-with-enough-entropy-01";
const BASE_NOW: i64 = 1_700_000_000;
/// The canonical byte length the suite declares for every ingested message.
const CANONICAL_LEN: usize = 900;

// ---------------------------------------------------------------------------
// A real node, driven the way production drives it.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct FixedClock(AtomicI64);

#[derive(Debug, Clone)]
struct SharedClock(Arc<FixedClock>);

impl HealthClock for SharedClock {
    fn unix_seconds(&self) -> i64 {
        self.0 .0.load(Ordering::SeqCst)
    }

    fn monotonic_millis(&self) -> u64 {
        0
    }
}

struct Node {
    _temp: TempDir,
    workspace: PathBuf,
    context: NodeContext,
    clock: Arc<FixedClock>,
    local_node_id: String,
}

impl Node {
    fn start() -> Self {
        let temp = TempDir::new().expect("temp workspace");
        let workspace = temp.path().to_path_buf();
        assert_success(&run_node(&workspace, &["init".to_string()]));
        let status = assert_success(&run_node(&workspace, &["status".to_string()]));
        let local_node_id = status["identity"]["node_id"]
            .as_str()
            .expect("local node id")
            .to_string();
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
        Self {
            _temp: temp,
            workspace,
            context,
            clock: Arc::new(FixedClock(AtomicI64::new(BASE_NOW))),
            local_node_id,
        }
    }

    /// Reopen the shipped registry exactly the way a restart does.
    fn registry(&self) -> NodeRegistry {
        let identity = NodeIdentity::load_existing(&self.context).expect("load node identity");
        NodeRegistry::open_existing(&self.context, identity.public_status()).expect("open registry")
    }

    fn set_now(&self, seconds: i64) {
        self.clock.0.store(seconds, Ordering::SeqCst);
    }

    fn now(&self) -> i64 {
        self.clock.0.load(Ordering::SeqCst)
    }

    fn advance(&self, seconds: i64) {
        self.set_now(self.now() + seconds);
    }
}

fn open_plane<'a>(node: &Node, registry: &'a NodeRegistry) -> HealthPlane<'a> {
    HealthPlane::with_clock(registry, Box::new(SharedClock(Arc::clone(&node.clock))))
}

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

/// The frozen node identifier construction, reused verbatim.
fn node_id_for_x_only_public_key(public_key: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"omakure/node-id/v1\0");
    digest.update(public_key);
    let hash: [u8; 32] = digest.finalize().into();
    format!(
        "omk1_{}",
        hash.iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn peer_identity(seed: u8) -> (String, String) {
    let key = k256::schnorr::SigningKey::from_slice(&[seed; 32]).expect("test scalar");
    let xonly = key.verifying_key().to_bytes();
    let public_key = xonly.iter().map(|byte| format!("{byte:02x}")).collect();
    (node_id_for_x_only_public_key(&xonly), public_key)
}

fn trust_peer(node: &Node, seed: u8, role: &str, capabilities: &[&str]) -> String {
    let (node_id, public_key) = peer_identity(seed);
    let mut args = vec![
        "trust".to_string(),
        "--node-id".to_string(),
        node_id.clone(),
        "--public-key".to_string(),
        public_key,
        "--role".to_string(),
        role.to_string(),
        "--actor".to_string(),
        // Deliberately hostile evidence: the actor and reason are privacy
        // class P1 free text, and no Signal may ever carry them.
        "/home/operator/keys/id_ed25519".to_string(),
        "--reason".to_string(),
        "secret://vault/enrollment AWS_SECRET_ACCESS_KEY=abc123".to_string(),
        "--confirmed".to_string(),
    ];
    for capability in capabilities {
        args.push("--capability".to_string());
        args.push((*capability).to_string());
    }
    let data = assert_success(&run_node(&node.workspace, &args));
    assert_eq!(data["state"], "active");
    node_id
}

fn revoke_peer(node: &Node, node_id: &str) {
    assert_success(&run_node(
        &node.workspace,
        &[
            "revoke".to_string(),
            node_id.to_string(),
            "--actor".to_string(),
            "/root/.ssh/authorized_keys".to_string(),
            "--reason".to_string(),
            "secret://vault/revocation /etc/shadow".to_string(),
            "--confirmed".to_string(),
        ],
    ));
}

fn hex16(seed: u64) -> String {
    format!("{seed:032x}")
}

fn signal_payload(
    target: &str,
    message_seed: u64,
    sequence: u64,
    signal_seed: u64,
    occurred_at: i64,
) -> Value {
    json!({
        "health_version": 1,
        "message_id": hex16(message_seed),
        "signal": {
            "kind": "run-completed",
            "occurred_at": occurred_at,
            "run": {
                "exit_code": 0,
                "finished_at": occurred_at,
                "run_id": hex16(signal_seed + 900_000),
                "script": "deploy",
                "state": "completed"
            },
            "sequence": sequence,
            "signal_id": hex16(signal_seed),
            "subject": Value::Null
        },
        "target": target,
    })
}

fn ack_payload(target: &str, message_seed: u64, acked: &str, cursor: u64) -> Value {
    json!({
        "health_version": 1,
        "message_id": hex16(message_seed),
        "ack": {
            "accepted": true,
            "acked_message_id": acked,
            "cursor": cursor,
        },
        "target": target,
    })
}

fn ingest(
    plane: &HealthPlane<'_>,
    sender: &str,
    kind: &str,
    created_at: i64,
    payload: &Value,
) -> (HealthDecision, HealthReply) {
    let outcome = plane
        .ingest(InboundHealthMessage {
            sender,
            kind,
            created_at,
            canonical_len: CANONICAL_LEN,
            payload,
        })
        .expect("ingest");
    (outcome.decision, outcome.reply)
}

/// Deliver one `run-completed` Signal and return the decision.
fn deliver_signal(
    node: &Node,
    plane: &HealthPlane<'_>,
    sender: &str,
    message_seed: u64,
    sequence: u64,
    signal_seed: u64,
) -> HealthDecision {
    let now = node.now();
    ingest(
        plane,
        sender,
        "health_signal",
        now,
        &signal_payload(
            &node.local_node_id,
            message_seed,
            sequence,
            signal_seed,
            now,
        ),
    )
    .0
}

/// Every Health Plane audit outcome recorded so far, redacted by construction.
fn health_audit(node: &Node) -> Vec<(String, Option<i64>)> {
    let connection = Connection::open(node.workspace.join(".node-state/node.sqlite"))
        .expect("open live node registry");
    let mut statement = connection
        .prepare("SELECT outcome, error_code FROM health_audit ORDER BY id")
        .expect("prepare health audit query");
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .expect("query health audit")
        .map(|row| row.expect("audit row"))
        .collect();
    rows
}

/// The complete trust and identity state, so a rejection can be proven inert.
fn trust_snapshot(node: &Node) -> String {
    let connection = Connection::open(node.workspace.join(".node-state/node.sqlite"))
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

/// The strings that must never appear in a Signal, a feed, or an audit row.
const FORBIDDEN: [&str; 7] = [
    "secret://",
    "AWS_SECRET_ACCESS_KEY",
    "/home/operator",
    "/root/.ssh",
    "/etc/shadow",
    "id_ed25519",
    "authorized_keys",
];

fn assert_no_leakage(label: &str, encoded: &str) {
    for forbidden in FORBIDDEN {
        assert!(
            !encoded.contains(forbidden),
            "{label} leaked a privacy class P1 value {forbidden}: {encoded}"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 1: exactly one visible Signal.
// ---------------------------------------------------------------------------

#[test]
fn one_terminal_run_stays_exactly_one_signal_across_duplicates_and_restarts() {
    let node = Node::start();
    let performer = trust_peer(
        &node,
        21,
        "performer",
        &["inventory-health", "notifications"],
    );
    let registry = node.registry();
    let plane = open_plane(&node, &registry);

    // First delivery: accepted, and the cursor advances by exactly one.
    let decision = deliver_signal(&node, &plane, &performer, 1, 1, 500);
    assert_eq!(decision, HealthDecision::Accepted { cursor: 1 });
    assert_eq!(plane.signals(&performer, 64).expect("signals").len(), 1);

    // Duplicate delivery of the identical message: replay, still one Signal.
    let decision = deliver_signal(&node, &plane, &performer, 1, 1, 500);
    assert_eq!(decision, HealthDecision::Rejected(HealthCode::Replay));

    // Acknowledgement loss: the Performer resends the same logical Signal with
    // a fresh `message_id`. The `signal_id` is the application idempotency key,
    // so it is refused and the cursor does not move.
    let decision = deliver_signal(&node, &plane, &performer, 2, 1, 500);
    assert_eq!(decision, HealthDecision::Rejected(HealthCode::Replay));

    // A resend that also reuses the sequence but claims a new signal id is a
    // replay too: the sequence is at or below the cursor.
    let decision = deliver_signal(&node, &plane, &performer, 3, 1, 501);
    assert_eq!(decision, HealthDecision::Rejected(HealthCode::Replay));

    assert_eq!(plane.signals(&performer, 64).expect("signals").len(), 1);
    drop(plane);
    drop(registry);

    // Receiver restart: the registry is reopened from disk exactly as a
    // restarted `node serve` reopens it.
    let restarted = node.registry();
    let plane = open_plane(&node, &restarted);
    let signals = plane.signals(&performer, 64).expect("signals");
    assert_eq!(signals.len(), 1, "a restart must not duplicate or lose it");
    assert_eq!(signals[0].signal_id, hex16(500));
    assert_eq!(signals[0].kind, SignalKind::RunCompleted);
    let status = plane
        .node_status(&performer)
        .expect("status")
        .expect("performer row");
    assert_eq!(status.signal_cursor, 1);
    assert_eq!(status.stored_signals, 1);
    assert_eq!(status.held_signals, 0);

    // And the resend after the restart is still exactly one visible Signal.
    node.advance(1);
    let decision = deliver_signal(&node, &plane, &performer, 4, 1, 500);
    assert_eq!(decision, HealthDecision::Rejected(HealthCode::Replay));
    assert_eq!(plane.signals(&performer, 64).expect("signals").len(), 1);
}

#[test]
fn enrollment_and_revocation_each_produce_exactly_one_local_lifecycle_signal() {
    let node = Node::start();
    let performer = trust_peer(
        &node,
        22,
        "performer",
        &["inventory-health", "notifications"],
    );
    let registry = node.registry();
    let plane = open_plane(&node, &registry);

    let enrolled = plane.local_signals(64).expect("local signals");
    assert_eq!(enrolled.len(), 1, "one activation is one Signal");
    assert_eq!(enrolled[0].kind, SignalKind::Enrolled);
    assert_eq!(enrolled[0].subject.as_deref(), Some(performer.as_str()));
    assert!(enrolled[0].run.is_none());

    // Reading twice cannot mint a second Signal, because nothing is written.
    assert_eq!(plane.local_signals(64).expect("local signals"), enrolled);

    revoke_peer(&node, &performer);
    let after = plane.local_signals(64).expect("local signals");
    assert_eq!(after.len(), 2, "one revocation is one more Signal");
    assert_eq!(after[0].kind, SignalKind::Revoked);
    assert_eq!(after[0].subject.as_deref(), Some(performer.as_str()));
    assert_eq!(after[1], enrolled[0], "the enrolled Signal is unchanged");
    assert!(
        after[0].sequence > after[1].sequence,
        "the local feed is ordered newest first"
    );

    // Revocation cleanup deletes every Health Plane row for a peer that is no
    // longer actively trusted. The local revocation Signal must survive it.
    plane.purge_revoked().expect("purge revoked");
    let preserved = plane.local_signals(64).expect("local signals");
    assert_eq!(preserved, after, "the local revocation Signal is preserved");

    // A restart reproduces exactly the same feed, ids included.
    drop(plane);
    drop(registry);
    let restarted = node.registry();
    let plane = open_plane(&node, &restarted);
    assert_eq!(plane.local_signals(64).expect("local signals"), after);
}

// ---------------------------------------------------------------------------
// Acceptance criterion 3: revocation blocks remote Signals.
// ---------------------------------------------------------------------------

#[test]
fn revocation_blocks_later_remote_signals_and_keeps_the_local_revocation_signal() {
    let node = Node::start();
    let performer = trust_peer(
        &node,
        23,
        "performer",
        &["inventory-health", "notifications"],
    );
    let registry = node.registry();
    let plane = open_plane(&node, &registry);
    assert_eq!(
        deliver_signal(&node, &plane, &performer, 10, 1, 600),
        HealthDecision::Accepted { cursor: 1 }
    );

    revoke_peer(&node, &performer);
    let before = trust_snapshot(&node);

    node.advance(1);
    let outcome = plane
        .ingest(InboundHealthMessage {
            sender: &performer,
            kind: "health_signal",
            created_at: node.now(),
            canonical_len: CANONICAL_LEN,
            payload: &signal_payload(&node.local_node_id, 11, 2, 601, node.now()),
        })
        .expect("ingest");
    assert_eq!(
        outcome.decision,
        HealthDecision::Rejected(HealthCode::Revoked)
    );
    assert_eq!(
        outcome.reply,
        HealthReply::None,
        "a revoked peer learns nothing: the message is dropped and audited"
    );
    assert_eq!(
        trust_snapshot(&node),
        before,
        "a rejected Signal must not touch trust"
    );

    let local = plane.local_signals(64).expect("local signals");
    assert_eq!(local.len(), 2);
    assert_eq!(local[0].kind, SignalKind::Revoked);

    // The revoked peer is no longer part of the fleet, so its retained Health
    // Plane rows stop reporting at once.
    let feed = assert_success(&run_node(&node.workspace, &["signals".to_string()]));
    assert_eq!(feed["signals"].as_array().expect("signals").len(), 2);
    for signal in feed["signals"].as_array().expect("signals") {
        assert_eq!(signal["source"], "local");
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 2: every contracted rejection is exact and inert.
// ---------------------------------------------------------------------------

#[test]
fn contracted_signal_rejections_are_audited_and_mutate_neither_health_nor_trust() {
    let node = Node::start();
    let performer = trust_peer(
        &node,
        24,
        "performer",
        &["inventory-health", "notifications"],
    );
    let no_notifications = trust_peer(&node, 25, "performer", &["inventory-health"]);
    let conductor = trust_peer(&node, 26, "conductor", &["notifications"]);
    let (stranger, _) = peer_identity(27);
    let registry = node.registry();
    let plane = open_plane(&node, &registry);
    assert_eq!(
        deliver_signal(&node, &plane, &performer, 20, 1, 700),
        HealthDecision::Accepted { cursor: 1 }
    );
    let trust_before = trust_snapshot(&node);
    let status_before = plane.node_status(&performer).expect("status");
    let now = node.now();
    let target = node.local_node_id.clone();

    // Every rejection the frozen receive order can produce for a Signal, with
    // the exact code and the exact reply posture.
    let cases: Vec<(&str, &str, i64, Value, HealthCode, bool)> = vec![
        (
            "unauthorized: the peer was never granted notifications",
            no_notifications.as_str(),
            now,
            signal_payload(&target, 21, 1, 701, now),
            HealthCode::MissingCapability,
            false,
        ),
        (
            "wrong role: a Conductor cannot report health",
            conductor.as_str(),
            now,
            signal_payload(&target, 22, 1, 702, now),
            HealthCode::WrongRole,
            false,
        ),
        (
            "unauthorized: an untrusted identity",
            stranger.as_str(),
            now,
            signal_payload(&target, 23, 1, 703, now),
            HealthCode::Revoked,
            false,
        ),
        (
            "wrong target: a third party's node id",
            performer.as_str(),
            now,
            signal_payload(&stranger, 24, 2, 704, now),
            HealthCode::WrongTarget,
            false,
        ),
        (
            "replayed message id",
            performer.as_str(),
            now,
            signal_payload(&target, 20, 2, 705, now),
            HealthCode::Replay,
            true,
        ),
        (
            "expired: one second past the frozen freshness window",
            performer.as_str(),
            now - MAX_AGE_SECONDS - 1,
            signal_payload(&target, 25, 2, 706, now - MAX_AGE_SECONDS - 1),
            HealthCode::Stale,
            true,
        ),
        (
            "future: one second past the frozen skew allowance",
            performer.as_str(),
            now + MAX_FUTURE_SKEW_SECONDS + 1,
            signal_payload(&target, 26, 2, 707, now),
            HealthCode::Future,
            true,
        ),
        (
            "spoofed body: a run-completed Signal that also names a subject",
            performer.as_str(),
            now,
            {
                let mut payload = signal_payload(&target, 27, 2, 708, now);
                payload["signal"]["subject"] = json!(stranger);
                payload
            },
            HealthCode::InvalidMessage,
            false,
        ),
        (
            "spoofed kind: an enrolled Signal carrying a run",
            performer.as_str(),
            now,
            {
                let mut payload = signal_payload(&target, 28, 2, 709, now);
                payload["signal"]["kind"] = json!("enrolled");
                payload
            },
            HealthCode::InvalidMessage,
            false,
        ),
        (
            "smuggled field: an unknown key inside the closed schema",
            performer.as_str(),
            now,
            {
                let mut payload = signal_payload(&target, 29, 2, 710, now);
                payload["signal"]["stdout"] = json!("hello");
                payload
            },
            HealthCode::UnknownField,
            false,
        ),
        (
            "smuggled secret reference in an otherwise valid field",
            performer.as_str(),
            now,
            {
                let mut payload = signal_payload(&target, 31, 2, 712, now);
                payload["signal"]["run"]["script"] = json!("secret://vault/token");
                payload
            },
            HealthCode::InvalidMessage,
            false,
        ),
        (
            "far future sequence: beyond the frozen reorder window",
            performer.as_str(),
            now,
            signal_payload(&target, 30, 1 + REORDER_BUFFER_ENTRIES + 1, 711, now),
            HealthCode::Reordered,
            true,
        ),
    ];

    for (label, sender, created_at, payload, code, replies) in cases {
        let outcome = plane
            .ingest(InboundHealthMessage {
                sender,
                kind: "health_signal",
                created_at,
                canonical_len: CANONICAL_LEN,
                payload: &payload,
            })
            .expect("ingest");
        assert_eq!(
            outcome.decision,
            HealthDecision::Rejected(code),
            "{label} must produce {}",
            code.name()
        );
        match (&outcome.reply, replies) {
            (HealthReply::Error { code: replied, .. }, true) => assert_eq!(*replied, code),
            (HealthReply::None, false) => {}
            (reply, _) => panic!("{label} produced an unexpected reply: {reply:?}"),
        }
    }

    // Nothing a rejection touched: not trust, not the fleet snapshot, not the
    // stored Signal feed.
    assert_eq!(trust_snapshot(&node), trust_before);
    assert_eq!(
        plane.node_status(&performer).expect("status"),
        status_before
    );
    assert_eq!(plane.signals(&performer, 64).expect("signals").len(), 1);
    assert!(plane
        .signals(&no_notifications, 64)
        .expect("signals")
        .is_empty());

    // The audit trail records the stable code and nothing else.
    let audit = health_audit(&node);
    let rejected: Vec<i64> = audit
        .iter()
        .filter(|(outcome, _)| outcome == "rejected")
        .filter_map(|(_, code)| *code)
        .collect();
    for code in [
        HealthCode::MissingCapability,
        HealthCode::WrongRole,
        HealthCode::Revoked,
        HealthCode::WrongTarget,
        HealthCode::Replay,
        HealthCode::Stale,
        HealthCode::Future,
        HealthCode::InvalidMessage,
        HealthCode::UnknownField,
        HealthCode::Reordered,
    ] {
        assert!(
            rejected.contains(&i64::from(code.code())),
            "missing audit for {} ({}); recorded: {rejected:?}",
            code.name(),
            code.code()
        );
    }
    assert_no_leakage("health audit", &format!("{audit:?}"));
}

// ---------------------------------------------------------------------------
// Acceptance criterion 4: bounds.
// ---------------------------------------------------------------------------

#[test]
fn cursor_gaps_hold_and_expire_inside_the_frozen_reorder_bounds() {
    let node = Node::start();
    let performer = trust_peer(
        &node,
        28,
        "performer",
        &["inventory-health", "notifications"],
    );
    let registry = node.registry();
    let plane = open_plane(&node, &registry);

    // A gap holds rather than admitting a hole: the cursor stays at zero.
    assert_eq!(
        deliver_signal(&node, &plane, &performer, 40, 3, 800),
        HealthDecision::Held { cursor: 0 }
    );
    assert!(plane.signals(&performer, 64).expect("signals").is_empty());

    // Filling the head advances the cursor by exactly one; the gap at two
    // still stalls the feed.
    assert_eq!(
        deliver_signal(&node, &plane, &performer, 41, 1, 801),
        HealthDecision::Accepted { cursor: 1 }
    );
    let status = plane
        .node_status(&performer)
        .expect("status")
        .expect("performer");
    assert_eq!(status.signal_cursor, 1);
    assert_eq!(status.held_signals, 1);

    // Closing the gap promotes every contiguous held Signal at once.
    assert_eq!(
        deliver_signal(&node, &plane, &performer, 42, 2, 802),
        HealthDecision::Accepted { cursor: 3 }
    );
    assert_eq!(plane.signals(&performer, 64).expect("signals").len(), 3);

    // A held Signal that outlives the frozen reorder lifetime is discarded and
    // the cursor does not advance past the gap.
    assert_eq!(
        deliver_signal(&node, &plane, &performer, 43, 6, 803),
        HealthDecision::Held { cursor: 3 }
    );
    node.advance(REORDER_BUFFER_SECONDS + 1);
    let report = plane.prune().expect("prune");
    assert_eq!(report.expired_held_signals, 1);
    let status = plane
        .node_status(&performer)
        .expect("status")
        .expect("performer");
    assert_eq!(status.signal_cursor, 3, "the cursor never skips a gap");
    assert_eq!(status.held_signals, 0);
    assert_eq!(status.stored_signals, 3);
}

#[test]
fn inbox_capacity_retention_and_storage_stay_inside_every_frozen_bound() {
    let node = Node::start();
    let performer = trust_peer(
        &node,
        29,
        "performer",
        &["inventory-health", "notifications"],
    );
    let registry = node.registry();
    let plane = open_plane(&node, &registry);

    // Fill the frozen 64-entry per-Performer inbox, respecting the frozen
    // per-minute Signal rate on the way in.
    let mut seed = 1_000_u64;
    for index in 0..SIGNAL_INBOX_CAPACITY {
        if index % MAX_SIGNALS_PER_PEER_PER_MINUTE == 0 && index > 0 {
            node.advance(RATE_MINUTE_WINDOW_SECONDS);
        }
        seed += 1;
        let decision = deliver_signal(&node, &plane, &performer, seed, index as u64 + 1, seed);
        assert_eq!(
            decision,
            HealthDecision::Accepted {
                cursor: index as u64 + 1
            },
            "signal {index} was refused"
        );
    }
    assert_eq!(
        plane.signals(&performer, 1_000).expect("signals").len(),
        SIGNAL_INBOX_CAPACITY as usize,
        "the read surface is bounded by the frozen inbox capacity"
    );

    // The next Signal is refused with the frozen queue-full code; the
    // Performer keeps it in its own outbox.
    node.advance(RATE_MINUTE_WINDOW_SECONDS);
    seed += 1;
    assert_eq!(
        deliver_signal(
            &node,
            &plane,
            &performer,
            seed,
            SIGNAL_INBOX_CAPACITY as u64 + 1,
            seed
        ),
        HealthDecision::Rejected(HealthCode::QueueFull)
    );

    let stored = plane.storage_bytes().expect("storage bytes");
    assert!(
        stored <= STORAGE_CEILING_BYTES,
        "stored {stored} bytes exceeds the frozen ceiling {STORAGE_CEILING_BYTES}"
    );

    // Past the frozen retention window every stored Signal expires.
    node.advance(SIGNAL_RETENTION_SECONDS + 1);
    let report = plane.prune().expect("prune");
    assert_eq!(report.expired_signals, SIGNAL_INBOX_CAPACITY as u64);
    assert!(plane
        .signals(&performer, 1_000)
        .expect("signals")
        .is_empty());
}

#[test]
fn an_acknowledgement_retires_exactly_the_outbox_signal_it_names() {
    let node = Node::start();
    // This node is the Performer: its peer is the Conductor it reports to.
    let conductor = trust_peer(&node, 30, "conductor", &["notifications"]);
    let registry = node.registry();
    let plane = open_plane(&node, &registry);

    let run = omakure::health_plane::model::RunFact {
        exit_code: Some(0),
        finished_at: node.now(),
        run_id: hex16(4_242),
        script: "deploy".to_string(),
        started_at: None,
        state: "completed".to_string(),
        trigger: None,
    };
    let first = plane
        .enqueue_signal(
            &conductor,
            &hex16(31),
            SignalKind::RunCompleted,
            node.now(),
            None,
            Some(&run),
            900,
        )
        .expect("enqueue signal");
    assert_eq!(first.sequence, 1);
    assert_eq!(first.attempts, 0);
    assert_eq!(
        first.expires_at - first.enqueued_at,
        SIGNAL_RETENTION_SECONDS
    );

    // The same logical Signal is never queued twice.
    assert!(plane
        .enqueue_signal(
            &conductor,
            &hex16(31),
            SignalKind::RunCompleted,
            node.now(),
            None,
            Some(&run),
            900,
        )
        .is_err());
    assert_eq!(plane.outbox(64).expect("outbox").len(), 1);

    let message_id = hex16(32);
    assert!(plane
        .mark_signal_sent(&hex16(31), &message_id)
        .expect("mark sent"));

    // The Conductor's acknowledgement retires exactly that entry.
    let outcome = plane
        .ingest(InboundHealthMessage {
            sender: &conductor,
            kind: "health_ack",
            created_at: node.now(),
            // Inside the frozen per-kind cap for `health_ack`.
            canonical_len: 510,
            payload: &ack_payload(&node.local_node_id, 33, &message_id, 1),
        })
        .expect("ingest");
    let decision = outcome.decision;
    assert_eq!(decision, HealthDecision::Accepted { cursor: 1 });
    assert!(
        plane.outbox(64).expect("outbox").is_empty(),
        "an acknowledged Signal leaves the outbox"
    );
    assert_eq!(plane.signals_dropped().expect("dropped"), 0);
}

#[test]
fn outbox_overflow_drops_the_oldest_signal_and_the_queue_survives_a_restart() {
    let node = Node::start();
    let conductor = trust_peer(&node, 32, "conductor", &["notifications"]);
    let registry = node.registry();
    let plane = open_plane(&node, &registry);

    let enqueue = |plane: &HealthPlane<'_>, index: u64| {
        let run = omakure::health_plane::model::RunFact {
            exit_code: Some(0),
            finished_at: node.now(),
            run_id: hex16(700_000 + index),
            script: "deploy".to_string(),
            started_at: None,
            state: "completed".to_string(),
            trigger: None,
        };
        plane.enqueue_signal(
            &conductor,
            &hex16(index),
            SignalKind::RunCompleted,
            node.now(),
            None,
            Some(&run),
            900,
        )
    };

    for index in 1..=(SIGNAL_OUTBOX_CAPACITY as u64) {
        enqueue(&plane, index).expect("enqueue inside the frozen capacity");
    }
    assert_eq!(
        plane.outbox(1_000).expect("outbox").len(),
        SIGNAL_OUTBOX_CAPACITY as usize
    );
    assert_eq!(plane.signals_dropped().expect("dropped"), 0);

    // One past the frozen capacity: the oldest undelivered Signal is dropped,
    // the local counter records it, and the queue never grows.
    let overflowed = enqueue(&plane, SIGNAL_OUTBOX_CAPACITY as u64 + 1).expect("overflow enqueue");
    let outbox = plane.outbox(1_000).expect("outbox");
    assert_eq!(outbox.len(), SIGNAL_OUTBOX_CAPACITY as usize);
    assert_eq!(plane.signals_dropped().expect("dropped"), 1);
    assert_eq!(
        outbox[0].signal_id,
        hex16(2),
        "the oldest entry was dropped"
    );
    assert_eq!(
        outbox[outbox.len() - 1].signal_id,
        overflowed.signal_id,
        "the newest entry is queued last"
    );
    assert!(
        outbox
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence),
        "the outbox is strictly ordered by sequence"
    );

    // Sender restart: the durable outbox is exactly what carries a Signal
    // across a process restart, with its sequence and idempotency key intact.
    drop(plane);
    drop(registry);
    let restarted = node.registry();
    let plane = open_plane(&node, &restarted);
    let after = plane.outbox(1_000).expect("outbox");
    assert_eq!(
        after, outbox,
        "a restart neither loses nor renumbers Signals"
    );
    assert_eq!(plane.signals_dropped().expect("dropped"), 1);

    // Past the frozen retention window the queue drains itself.
    node.advance(SIGNAL_RETENTION_SECONDS + 1);
    let report = plane.prune().expect("prune");
    assert_eq!(report.expired_outbox_signals, SIGNAL_OUTBOX_CAPACITY as u64);
    assert!(plane.outbox(1_000).expect("outbox").is_empty());
}

// ---------------------------------------------------------------------------
// Acceptance criterion 5: adapter parity and redaction.
// ---------------------------------------------------------------------------

#[test]
fn the_signal_read_surface_is_bounded_newest_first_and_carries_no_private_field() {
    let node = Node::start();
    let performer = trust_peer(
        &node,
        31,
        "performer",
        &["inventory-health", "notifications"],
    );
    let registry = node.registry();
    let plane = open_plane(&node, &registry);
    assert_eq!(
        deliver_signal(&node, &plane, &performer, 60, 1, 900),
        HealthDecision::Accepted { cursor: 1 }
    );
    node.advance(10);
    assert_eq!(
        deliver_signal(&node, &plane, &performer, 61, 2, 901),
        HealthDecision::Accepted { cursor: 2 }
    );
    drop(plane);
    drop(registry);

    let feed = assert_success(&run_node(&node.workspace, &["signals".to_string()]));
    assert_eq!(feed["enabled"], true);
    assert_eq!(feed["local_node_id"], node.local_node_id);
    assert_eq!(feed["retention_seconds"], SIGNAL_RETENTION_SECONDS);
    assert_eq!(feed["limit"], SIGNAL_INBOX_CAPACITY);
    assert_eq!(feed["gap"], false);

    let signals = feed["signals"].as_array().expect("signals");
    assert_eq!(
        signals.len(),
        3,
        "one enrollment plus two run-completed Signals"
    );
    let occurred: Vec<i64> = signals
        .iter()
        .map(|signal| signal["occurred_at"].as_i64().expect("occurred_at"))
        .collect();
    let mut sorted = occurred.clone();
    sorted.sort_unstable_by(|left, right| right.cmp(left));
    assert_eq!(occurred, sorted, "the feed is newest first");

    let kinds: Vec<&str> = signals
        .iter()
        .map(|signal| signal["kind"].as_str().expect("kind"))
        .collect();
    for kind in &kinds {
        assert!(
            ["enrolled", "revoked", "run-completed"].contains(kind),
            "the Signal vocabulary is closed; found {kind}"
        );
    }
    assert_eq!(kinds.iter().filter(|kind| **kind == "enrolled").count(), 1);

    let cursors = feed["cursors"].as_array().expect("cursors");
    assert_eq!(cursors.len(), 1);
    assert_eq!(cursors[0]["node_id"], performer);
    assert_eq!(cursors[0]["cursor"], 2);
    assert_eq!(cursors[0]["stored"], 2);
    assert_eq!(cursors[0]["held"], 0);
    assert_eq!(cursors[0]["gap"], false);

    // The bounded run facts carry only the frozen five fields.
    let remote = signals
        .iter()
        .find(|signal| signal["kind"] == "run-completed")
        .expect("a run-completed Signal");
    assert_eq!(remote["source"], performer);
    let run = remote["run"].as_object().expect("run object");
    assert_eq!(run.len(), 5);
    for field in ["exit_code", "finished_at", "run_id", "script", "state"] {
        assert!(run.contains_key(field), "missing {field}");
    }
    for forbidden in [
        "args",
        "stdout",
        "stderr",
        "error",
        "actor",
        "worker_id",
        "script_path",
        "reason",
    ] {
        assert!(!run.contains_key(forbidden), "run leaked {forbidden}");
    }

    assert_no_leakage("signal feed", &feed.to_string());

    // The Signal feed is not an event bus: no subscription, webhook, alert, or
    // history surface exists on it.
    for forbidden in [
        "subscriptions",
        "subscribe",
        "webhook",
        "alerts",
        "history",
        "series",
    ] {
        assert!(
            feed.get(forbidden).is_none(),
            "the closed feed must not expose {forbidden}"
        );
    }
}

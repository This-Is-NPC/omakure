//! Integration coverage for the bounded Health Plane state owned by
//! `node.sqlite` and the protocol-neutral shared operations built on top of it
//! (task #2777, wave 2 of plan `health-plane-foundation`).
//!
//! Unlike the in-crate unit tests, this suite drives a node the way production
//! does: it initializes real node state and real trust through the shipped CLI,
//! then reopens that database through the public registry surface. It proves
//! the schema version 7 migration lands on a production node, that the public
//! operations enforce the frozen contract, that state survives a restart, and
//! that the public projection never carries a privacy class P1 field.
//!
//! It deliberately does not exercise transport scheduling, the Noise
//! application dispatcher, or any CLI/HTTP adapter: those are waves 3 and 4.

mod support;

use omakure::health_plane::bounds::{PRESENCE_ONLINE_SECONDS, STORAGE_CEILING_BYTES};
use omakure::health_plane::model::{HealthCode, HealthDecision, Presence};
use omakure::health_plane::{HealthClock, HealthPlane, HealthReply, InboundHealthMessage};
use omakure::node::{NodeContext, NodePathOverrides, NodePlatform};
use omakure::node_identity::NodeIdentity;
use omakure::node_registry::{NodeRegistry, PeerState};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

const TOKEN: &str = "health-plane-state-token-with-enough-entropy-0001";
const BASE_NOW: i64 = 1_700_000_000;

#[derive(Debug)]
struct FixedClock(AtomicI64);

/// A shareable handle so the test can advance the clock the operations layer
/// reads. Time is injected; nothing in this suite reads the wall clock.
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

/// The frozen node identifier construction, reused verbatim so the test can
/// name a synthetic peer without touching the identity module.
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
        "health-plane-state-tests".to_string(),
        "--reason".to_string(),
        "health plane state integration peer".to_string(),
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
            "health-plane-state-tests".to_string(),
            "--reason".to_string(),
            "device reported lost".to_string(),
            "--confirmed".to_string(),
        ],
    ));
}

fn hex16(seed: u64) -> String {
    format!("{seed:032x}")
}

fn profile_payload(target: &str, message_seed: u64, revision: u64) -> Value {
    json!({
        "health_version": 1,
        "message_id": hex16(message_seed),
        "profile": {
            "agent_version": "0.3.0",
            "arch": "x86_64",
            "baseline_id": "",
            "baseline_observed_id": "",
            "capabilities": ["inventory-health", "notifications"],
            "display_name": "workshop-laptop",
            "distro_id": "arch",
            "distro_version": "rolling",
            "omarchy_channel": "stable",
            "omarchy_version": "2.1.0",
            "platform": "linux",
            "profile_revision": revision,
            "role": "performer",
            "runtimes": [{"available": true, "name": "bash", "version": "5.2.37"}]
        },
        "target": target,
    })
}

fn pulse_payload(target: &str, message_seed: u64, sequence: u64, emitted_at: i64) -> Value {
    json!({
        "health_version": 1,
        "message_id": hex16(message_seed),
        "pulse": {
            "emitted_at": emitted_at,
            "last_run": Value::Null,
            "profile_revision": 1,
            "runner": {
                "queue_depth": 0,
                "scheduler": "running",
                "state": "idle",
                "workers_busy": 0,
                "workers_configured": 1
            },
            "sequence": sequence,
            "uptime_seconds": 3600
        },
        "target": target,
    })
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
            canonical_len: 900,
            payload,
        })
        .expect("ingest");
    (outcome.decision, outcome.reply)
}

#[test]
fn a_production_node_migrates_to_schema_seven_and_serves_bounded_health_state() {
    let node = Node::start();
    let performer = trust_peer(
        &node,
        7,
        "performer",
        &["inventory-health", "notifications"],
    );
    let registry = node.registry();
    assert!(
        registry.health_plane_enabled().expect("plane state"),
        "the shipped node must migrate to the Health Plane schema"
    );

    let plane = HealthPlane::with_clock(&registry, Box::new(SharedClock(Arc::clone(&node.clock))));
    let target = node.local_node_id.clone();

    let (decision, reply) = ingest(
        &plane,
        &performer,
        "health_profile",
        BASE_NOW,
        &profile_payload(&target, 1, 1),
    );
    assert_eq!(decision, HealthDecision::Accepted { cursor: 0 });
    assert_eq!(
        reply,
        HealthReply::Ack {
            acked_message_id: hex16(1),
            cursor: 0
        }
    );

    node.set_now(BASE_NOW + 30);
    let (decision, _) = ingest(
        &plane,
        &performer,
        "health_pulse",
        BASE_NOW + 30,
        &pulse_payload(&target, 2, 1, BASE_NOW + 30),
    );
    assert_eq!(decision, HealthDecision::Accepted { cursor: 0 });

    node.set_now(BASE_NOW + 60);
    let (decision, _) = ingest(
        &plane,
        &performer,
        "health_signal",
        BASE_NOW + 60,
        &signal_payload(&target, 3, 1, 101, BASE_NOW + 60),
    );
    assert_eq!(decision, HealthDecision::Accepted { cursor: 1 });

    // A newer Profile replaces the single retained row rather than appending.
    node.set_now(BASE_NOW + 90);
    let (decision, _) = ingest(
        &plane,
        &performer,
        "health_profile",
        BASE_NOW + 90,
        &profile_payload(&target, 4, 2),
    );
    assert_eq!(decision, HealthDecision::Accepted { cursor: 1 });

    let fleet = plane.fleet_status().expect("fleet status");
    assert_eq!(fleet.len(), 1);
    let status = &fleet[0];
    assert_eq!(status.node_id, performer);
    assert_eq!(status.role, "performer");
    assert_eq!(status.trust_state, "active");
    assert_eq!(status.presence, Presence::Online);
    assert_eq!(
        status.profile.as_ref().expect("profile").profile_revision,
        2
    );
    assert_eq!(status.pulse.as_ref().expect("pulse").sequence, 1);
    assert_eq!(status.signal_cursor, 1);
    assert_eq!(status.stored_signals, 1);
    assert_eq!(
        status.capabilities,
        vec!["inventory-health".to_string(), "notifications".to_string()]
    );
    assert!(plane.storage_bytes().expect("storage") < STORAGE_CEILING_BYTES);

    // Presence degrades purely from the injected clock.
    node.set_now(BASE_NOW + 30 + PRESENCE_ONLINE_SECONDS + 1);
    assert_eq!(
        plane
            .node_status(&performer)
            .expect("node status")
            .expect("tracked")
            .presence,
        Presence::Stale
    );
}

#[test]
fn health_state_and_replay_protection_survive_a_restart() {
    let node = Node::start();
    let performer = trust_peer(
        &node,
        8,
        "performer",
        &["inventory-health", "notifications"],
    );
    let target = node.local_node_id.clone();

    {
        let registry = node.registry();
        let plane =
            HealthPlane::with_clock(&registry, Box::new(SharedClock(Arc::clone(&node.clock))));
        ingest(
            &plane,
            &performer,
            "health_profile",
            BASE_NOW,
            &profile_payload(&target, 11, 3),
        );
        node.set_now(BASE_NOW + 30);
        ingest(
            &plane,
            &performer,
            "health_signal",
            BASE_NOW + 30,
            &signal_payload(&target, 12, 1, 201, BASE_NOW + 30),
        );
    }

    // Reopen the database exactly as a service restart would.
    let registry = node.registry();
    let plane = HealthPlane::with_clock(&registry, Box::new(SharedClock(Arc::clone(&node.clock))));
    let status = plane
        .node_status(&performer)
        .expect("node status")
        .expect("tracked");
    assert_eq!(status.profile.expect("profile").profile_revision, 3);
    assert_eq!(status.signal_cursor, 1);
    assert_eq!(plane.signals(&performer, 64).expect("signals").len(), 1);

    node.set_now(BASE_NOW + 60);
    let (decision, _) = ingest(
        &plane,
        &performer,
        "health_profile",
        BASE_NOW + 60,
        &profile_payload(&target, 11, 4),
    );
    assert_eq!(
        decision,
        HealthDecision::Rejected(HealthCode::Replay),
        "a restart must not reopen the replay window"
    );
}

#[test]
fn revocation_stops_reporting_and_purges_derived_state_only() {
    let node = Node::start();
    let performer = trust_peer(
        &node,
        9,
        "performer",
        &["inventory-health", "notifications"],
    );
    let target = node.local_node_id.clone();
    let registry = node.registry();
    let plane = HealthPlane::with_clock(&registry, Box::new(SharedClock(Arc::clone(&node.clock))));

    ingest(
        &plane,
        &performer,
        "health_profile",
        BASE_NOW,
        &profile_payload(&target, 21, 1),
    );
    assert_eq!(plane.fleet_status().expect("fleet").len(), 1);

    revoke_peer(&node, &performer);

    node.set_now(BASE_NOW + 10);
    let (decision, reply) = ingest(
        &plane,
        &performer,
        "health_profile",
        BASE_NOW + 10,
        &profile_payload(&target, 22, 2),
    );
    assert_eq!(decision, HealthDecision::Rejected(HealthCode::Revoked));
    assert_eq!(
        reply,
        HealthReply::None,
        "a revoked peer never receives a health_error"
    );

    let purged = plane.purge_revoked().expect("purge");
    assert_eq!(purged, vec![performer.clone()]);
    assert!(plane.fleet_status().expect("fleet").is_empty());

    // Trust evidence is untouched: the revocation is still on record.
    let authorization = plane
        .authorization(&performer)
        .expect("authorization")
        .expect("retained identity");
    assert_eq!(authorization.state, PeerState::Revoked);
    let peers = assert_success(&run_node(&node.workspace, &["peers".to_string()]));
    assert!(
        peers.to_string().contains("revoked"),
        "the revocation must remain visible to the operator: {peers}"
    );
}

#[test]
fn the_public_projection_never_carries_a_forbidden_field() {
    let node = Node::start();
    let performer = trust_peer(
        &node,
        10,
        "performer",
        &["inventory-health", "notifications"],
    );
    let target = node.local_node_id.clone();
    let registry = node.registry();
    let plane = HealthPlane::with_clock(&registry, Box::new(SharedClock(Arc::clone(&node.clock))));

    // A payload carrying a privacy class P1 field is rejected outright and
    // never reaches storage, so no redaction step can ever be skipped.
    let mut hostile = profile_payload(&target, 31, 1);
    hostile.as_object_mut().unwrap()["profile"]
        .as_object_mut()
        .unwrap()
        .insert("hostname".to_string(), json!("workshop.local"));
    let (decision, reply) = ingest(&plane, &performer, "health_profile", BASE_NOW, &hostile);
    assert_eq!(decision, HealthDecision::Rejected(HealthCode::UnknownField));
    assert_eq!(reply, HealthReply::None);

    ingest(
        &plane,
        &performer,
        "health_profile",
        BASE_NOW,
        &profile_payload(&target, 32, 1),
    );
    let rendered = serde_json::to_string(&plane.fleet_status().expect("fleet")).expect("encode");
    for forbidden in [
        "hostname",
        "workshop.local",
        "username",
        "ip_address",
        "mac_address",
        "secret://",
        "token",
        "cpu",
        "memory",
        "disk",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "the public projection leaked {forbidden:?}"
        );
    }

    // The audit trail records the stable code and metadata, never the payload.
    let audit = plane.audit_events(16).expect("audit");
    assert!(audit
        .iter()
        .any(|event| event.error_code == Some(HealthCode::UnknownField.code())));
    for event in &audit {
        let rendered = format!("{event:?}");
        assert!(!rendered.contains("workshop"), "audit leaked: {rendered}");
        assert!(!rendered.contains("hostname"), "audit leaked: {rendered}");
    }
}

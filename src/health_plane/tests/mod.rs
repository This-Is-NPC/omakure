use super::*;
use crate::test_support::{node_context, opaque_id_hex, peer_identity};

use crate::node_identity::NodeIdentity;
use crate::node_registry::{PeerRegistration, PeerRole, PeerSource};

use serde_json::json;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

mod fleet;
mod receive;

const BASE_NOW: i64 = 1_700_000_000;

/// Signals one Performer reports while the feed is read concurrently.
/// Well inside the frozen 64-entry inbox, so nothing is evicted.
const SIGNALS_UNDER_CONCURRENT_READ: u64 = 40;

#[derive(Debug, Default)]
struct TestClock {
    seconds: AtomicI64,
    millis: AtomicI64,
}

impl TestClock {
    fn at(seconds: i64) -> Arc<Self> {
        Arc::new(Self {
            seconds: AtomicI64::new(seconds),
            millis: AtomicI64::new(0),
        })
    }

    fn set(&self, seconds: i64) {
        self.seconds.store(seconds, Ordering::SeqCst);
    }
}

impl HealthClock for Arc<TestClock> {
    fn unix_seconds(&self) -> i64 {
        self.seconds.load(Ordering::SeqCst)
    }

    fn monotonic_millis(&self) -> u64 {
        self.millis.load(Ordering::SeqCst).max(0) as u64
    }
}

struct Fixture {
    _temp: TempDir,
    registry: NodeRegistry,
    clock: Arc<TestClock>,
    performer: String,
    limited: String,
    conductor: String,
    local: String,
}

fn fixture() -> Fixture {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
    let trust = |seed: u32, role: PeerRole, capabilities: &[&str]| {
        let (node_id, public_key, _) = peer_identity(seed);
        registry
            .import_manual_peer_with_transport(
                PeerRegistration {
                    node_id: node_id.clone(),
                    public_key,
                    role,
                    capabilities: capabilities.iter().map(|entry| entry.to_string()).collect(),
                    source: PeerSource::Manual,
                    actor: "health-plane-tests".to_string(),
                    reason: "health plane operations test peer".to_string(),
                },
                None,
            )
            .unwrap();
        node_id
    };
    let performer = trust(
        1,
        PeerRole::Performer,
        &["inventory-health", "notifications"],
    );
    let limited = trust(2, PeerRole::Performer, &["remote-run"]);
    let conductor = trust(3, PeerRole::Conductor, &[]);
    let local = registry.local_node_id().to_string();
    Fixture {
        _temp: temp,
        registry,
        clock: TestClock::at(BASE_NOW),
        performer,
        limited,
        conductor,
        local,
    }
}

impl Fixture {
    fn plane(&self) -> HealthPlane<'_> {
        HealthPlane::with_clock(&self.registry, Box::new(Arc::clone(&self.clock)))
    }

    fn ingest(&self, sender: &str, kind: &str, created_at: i64, payload: &Value) -> HealthIngest {
        self.plane()
            .ingest(InboundHealthMessage {
                sender,
                kind,
                created_at,
                canonical_len: 900,
                payload,
            })
            .unwrap()
    }

    fn code(&self, sender: &str, kind: &str, created_at: i64, payload: &Value) -> HealthCode {
        self.ingest(sender, kind, created_at, payload)
            .code()
            .expect("rejection")
    }
}

fn profile_payload(target: &str, message_seed: u64, revision: u64) -> Value {
    json!({
        "health_version": 1,
        "message_id": opaque_id_hex(message_seed),
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
        "message_id": opaque_id_hex(message_seed),
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
        "message_id": opaque_id_hex(message_seed),
        "signal": {
            "kind": "run-completed",
            "occurred_at": occurred_at,
            "run": {
                "exit_code": 0,
                "finished_at": occurred_at,
                "run_id": opaque_id_hex(signal_seed + 900_000),
                "script": "deploy",
                "state": "completed"
            },
            "sequence": sequence,
            "signal_id": opaque_id_hex(signal_seed),
            "subject": Value::Null
        },
        "target": target,
    })
}

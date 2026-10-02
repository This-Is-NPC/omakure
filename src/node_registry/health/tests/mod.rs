use super::super::{NodeRegistry, PeerRole, PeerState, RegistryError};
use super::feed::fleet_peer_in;
use super::rows::{CorruptHealthIdentity, cleanup_corrupt_health_rows, load_peer_state};
use super::types::HealthApplyRequest;
use crate::domain::health_plane::bounds::{
    AUDIT_ROW_BYTES, MAX_AGE_SECONDS, MAX_AUDIT_ROWS, MAX_FUTURE_SKEW_SECONDS,
    MAX_PERFORMERS_PER_CONDUCTOR, MAX_PROFILES_PER_PEER_PER_HOUR, MAX_REPLAY_ROWS,
    MAX_STORED_PROFILE_BYTES, MAX_STORED_PULSE_BYTES, MAX_STORED_SIGNAL_BYTES, REPLAY_ROW_BYTES,
    REPLAY_SECURITY_FLOOR_SECONDS, SIGNAL_INBOX_CAPACITY, SIGNAL_OUTBOX_CAPACITY,
    SIGNAL_RETENTION_SECONDS, STORAGE_CEILING_BYTES, VERSION_INCOMPATIBLE_EXPIRY_SECONDS,
    WORST_CASE_BYTES_PER_PERFORMER,
};
use crate::domain::health_plane::model::{
    HealthBody, HealthCode, HealthDecision, HealthKind, HealthPayload, ProfileSnapshot,
    PulseSnapshot, RunFact, RunnerFact, RuntimeFact, SignalEnqueueRequest, SignalKind,
    SignalRecord,
};
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::{PeerRegistration, PeerSource, SCHEMA_VERSION};
use crate::test_support::{node_context, opaque_id_hex, peer_identity};
use rusqlite::{Connection, TransactionBehavior, params};
use std::sync::Arc;
use tempfile::TempDir;

mod apply;
mod audit;
mod evaluate;
mod feed;
mod outbox;
mod prune;
mod rows;

const BASE_NOW: i64 = 1_700_000_000;

struct Fixture {
    _temp: TempDir,
    context: NodeContext,
    registry: NodeRegistry,
}

fn fixture() -> Fixture {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
    Fixture {
        _temp: temp,
        context,
        registry,
    }
}

fn reopen(fixture: &Fixture) -> NodeRegistry {
    let identity = NodeIdentity::load_existing(&fixture.context).unwrap();
    NodeRegistry::open(&fixture.context, identity.public_status()).unwrap()
}

fn trust(registry: &NodeRegistry, seed: u32, role: PeerRole, capabilities: &[&str]) -> String {
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
                reason: "health plane storage test peer".to_string(),
            },
            None,
        )
        .unwrap();
    node_id
}

fn performer(registry: &NodeRegistry) -> String {
    trust(
        registry,
        1,
        PeerRole::Performer,
        &["inventory-health", "notifications"],
    )
}

fn profile(target: &str, message_seed: u64, revision: u64) -> HealthPayload {
    HealthPayload {
        message_id: opaque_id_hex(message_seed),
        target: target.to_string(),
        body: HealthBody::Profile(ProfileSnapshot {
            agent_version: "0.3.0".to_string(),
            arch: "x86_64".to_string(),
            baseline_id: String::new(),
            baseline_observed_id: String::new(),
            capabilities: vec!["inventory-health".to_string(), "notifications".to_string()],
            display_name: "workshop-laptop".to_string(),
            distro_id: "arch".to_string(),
            distro_version: "rolling".to_string(),
            omarchy_channel: "stable".to_string(),
            omarchy_version: "2.1.0".to_string(),
            platform: "linux".to_string(),
            profile_revision: revision,
            role: "performer".to_string(),
            runtimes: vec![RuntimeFact {
                available: true,
                name: "bash".to_string(),
                version: "5.2.37".to_string(),
            }],
        }),
    }
}

fn pulse(target: &str, message_seed: u64, sequence: u64, emitted_at: i64) -> HealthPayload {
    HealthPayload {
        message_id: opaque_id_hex(message_seed),
        target: target.to_string(),
        body: HealthBody::Pulse(PulseSnapshot {
            emitted_at,
            last_run: None,
            profile_revision: 1,
            runner: RunnerFact {
                queue_depth: 0,
                scheduler: "running".to_string(),
                state: "idle".to_string(),
                workers_busy: 0,
                workers_configured: 1,
            },
            sequence,
            uptime_seconds: 3600,
        }),
    }
}

fn signal(
    target: &str,
    message_seed: u64,
    sequence: u64,
    signal_seed: u64,
    occurred_at: i64,
) -> HealthPayload {
    HealthPayload {
        message_id: opaque_id_hex(message_seed),
        target: target.to_string(),
        body: HealthBody::Signal(SignalRecord {
            kind: SignalKind::RunCompleted,
            occurred_at,
            run: Some(RunFact {
                exit_code: Some(0),
                finished_at: occurred_at,
                run_id: opaque_id_hex(signal_seed + 900_000),
                script: "deploy".to_string(),
                started_at: None,
                state: "completed".to_string(),
                trigger: None,
            }),
            sequence,
            signal_id: opaque_id_hex(signal_seed),
            subject: None,
        }),
    }
}

fn apply(
    registry: &NodeRegistry,
    sender: &str,
    payload: &HealthPayload,
    now: i64,
) -> HealthDecision {
    apply_at(registry, sender, payload, now, now)
}

fn apply_at(
    registry: &NodeRegistry,
    sender: &str,
    payload: &HealthPayload,
    created_at: i64,
    now: i64,
) -> HealthDecision {
    let bytes = match payload.body.kind() {
        HealthKind::Profile => 1_327,
        HealthKind::Pulse => 926,
        HealthKind::Signal => 777,
        _ => 510,
    };
    registry
        .apply_health_message(HealthApplyRequest {
            sender,
            payload,
            created_at,
            now,
            message_bytes: bytes,
        })
        .unwrap()
}

fn accepted(cursor: u64) -> HealthDecision {
    HealthDecision::Accepted { cursor }
}

#[test]
fn schema_creates_bounded_empty_tables_beside_trust_rows() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let connection = Connection::open(fixture.registry.path()).unwrap();

    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
    let marker: String = connection
        .query_row(
            "SELECT value FROM metadata WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(marker, "8");
    for (object_type, name) in super::super::validate::HEALTH_PLANE_OBJECTS {
        let present: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                params![object_type, name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(present, 1, "missing {object_type} {name}");
    }
    // Every Health Plane table starts empty and the trust rows are intact.
    for table in [
        "health_peers",
        "health_profiles",
        "health_pulses",
        "health_signals",
        "health_outbox",
        "health_replay_keys",
        "health_audit",
    ] {
        let rows: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "{table} must start empty");
    }
    let trust_state: String = connection
        .query_row(
            "SELECT state FROM trusted_peers WHERE node_id = ?1",
            params![node_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trust_state, "active");
}

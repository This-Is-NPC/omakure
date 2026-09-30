use super::*;
use crate::test_support::{node_context, opaque_id_hex, peer_identity, scalar};
use crate::util::hex;

#[test]
fn the_transport_failure_mapping_matches_the_frozen_table() {
    assert_eq!(
        transport_failure_code(TransportError::HandshakeFailed),
        HealthCode::InvalidMessage
    );
    assert_eq!(
        transport_failure_code(TransportError::IdentityMismatch),
        HealthCode::InvalidMessage
    );
    assert_eq!(
        transport_failure_code(TransportError::Replay),
        HealthCode::Replay
    );
    assert_eq!(
        transport_failure_code(TransportError::InvalidFrame),
        HealthCode::InvalidMessage
    );
    assert_eq!(
        transport_failure_code(TransportError::MessageTooLarge),
        HealthCode::MessageTooLarge
    );
    assert_eq!(
        transport_failure_code(TransportError::RateLimited),
        HealthCode::InvalidMessage
    );
}

#[test]
fn a_fresh_id_is_thirty_two_lowercase_hex_characters_and_unique() {
    let first = fresh_id();
    assert_eq!(first.len(), 32);
    assert!(hex::is_lower(&first));
    assert_ne!(first, fresh_id());
}

#[test]
fn the_tick_is_shorter_than_every_frozen_cadence() {
    assert!(TICK.as_secs() as i64 <= ACK_TIMEOUT_SECONDS);
    assert!(TICK.as_secs() as i64 <= MIN_PULSE_INTERVAL_SECONDS);
}

// -----------------------------------------------------------------------
// Emission schedule, over an injected clock so every frozen interval is
// exercised at its exact boundary with no real waiting.
// -----------------------------------------------------------------------

use crate::direct_transport::{envelope_kind_hint, envelope_view, verify_envelope};
use crate::health_plane::model::RunFact;
use crate::health_plane::model::RunnerFact;
use crate::health_plane::report::{HealthFactsSource, ProfileFacts, PulseFacts};

use crate::node_registry::{PeerRegistration, PeerSource};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;
use tempfile::TempDir;

mod emission;
mod signals;

const BASE_NOW: i64 = 1_700_000_000;
const SESSION_ID: [u8; 32] = [0x5a; 32];

#[derive(Debug, Default)]
struct TestClock {
    seconds: AtomicI64,
    millis: AtomicU64,
}

impl TestClock {
    fn at(seconds: i64) -> Arc<Self> {
        Arc::new(Self {
            seconds: AtomicI64::new(seconds),
            millis: AtomicU64::new(0),
        })
    }

    fn set(&self, seconds: i64) {
        self.seconds.store(seconds, Ordering::SeqCst);
    }

    fn advance(&self, seconds: i64) {
        self.seconds.fetch_add(seconds, Ordering::SeqCst);
    }
}

impl HealthClock for TestClock {
    fn unix_seconds(&self) -> i64 {
        self.seconds.load(Ordering::SeqCst)
    }

    fn monotonic_millis(&self) -> u64 {
        self.millis.load(Ordering::SeqCst)
    }
}

/// A fact source whose display name the test can change, which is the
/// smallest possible material Profile change.
struct MutableFacts {
    display_name: StdMutex<String>,
    terminal: StdMutex<Vec<RunFact>>,
}

impl MutableFacts {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            display_name: StdMutex::new("workshop".to_string()),
            terminal: StdMutex::new(Vec::new()),
        })
    }

    /// Record one already-terminal run, exactly as the local run log would.
    fn finish_run(&self, run_id: &str, script: &str, finished_at: i64) {
        self.terminal.lock().unwrap().insert(
            0,
            RunFact {
                exit_code: Some(0),
                finished_at,
                run_id: run_id.to_string(),
                script: script.to_string(),
                started_at: None,
                state: "completed".to_string(),
                trigger: None,
            },
        );
    }
}

impl HealthFactsSource for Arc<MutableFacts> {
    fn profile_facts(&self) -> ProfileFacts {
        ProfileFacts {
            agent_version: "0.3.0".to_string(),
            arch: "x86_64".to_string(),
            baseline_id: String::new(),
            baseline_observed_id: String::new(),
            capabilities: Vec::new(),
            display_name: self.display_name.lock().unwrap().clone(),
            distro_id: "arch".to_string(),
            distro_version: "rolling".to_string(),
            omarchy_channel: "stable".to_string(),
            omarchy_version: "2.1.0".to_string(),
            platform: "linux".to_string(),
            runtimes: Vec::new(),
        }
    }

    fn pulse_facts(&self) -> PulseFacts {
        PulseFacts {
            runner: RunnerFact {
                queue_depth: 0,
                scheduler: "running".to_string(),
                state: "idle".to_string(),
                workers_busy: 0,
                workers_configured: 1,
            },
            last_run: None,
            uptime_seconds: 60,
        }
    }

    fn terminal_runs(&self, limit: usize) -> Vec<RunFact> {
        let mut runs = self.terminal.lock().unwrap().clone();
        runs.truncate(limit);
        runs
    }
}

struct Fixture {
    _temp: TempDir,
    identity: NodeIdentity,
    /// The deterministic signing identity corresponding to `conductor`.
    /// Keeping its private key in the fixture lets carriage tests send a
    /// valid inbound message after revocation, exactly as a live peer can.
    conductor_identity: NodeIdentity,
    registry: NodeRegistry,
    conductor: String,
    conductor_key: [u8; 32],
    performer: String,
    clock: Arc<TestClock>,
    facts: Arc<MutableFacts>,
}

fn fixture() -> Fixture {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
    let conductor_root = temp.path().join("conductor");
    std::fs::create_dir_all(&conductor_root).unwrap();
    let conductor_context = node_context(&conductor_root);
    let conductor_identity = NodeIdentity::import(&conductor_context, &scalar(11)).unwrap();
    let trust = |seed: u32, role: PeerRole, capabilities: &[&str]| {
        let (node_id, public_key, xonly) = peer_identity(seed);
        registry
            .import_manual_peer_with_transport(
                PeerRegistration {
                    node_id: node_id.clone(),
                    public_key,
                    role,
                    capabilities: capabilities.iter().map(|entry| entry.to_string()).collect(),
                    source: PeerSource::Manual,
                    actor: "direct-health-tests".to_string(),
                    reason: "health plane carriage test peer".to_string(),
                },
                None,
            )
            .unwrap();
        (node_id, xonly)
    };
    let (conductor, conductor_key) = trust(11, PeerRole::Conductor, &["inventory-health"]);
    let (performer, _) = trust(12, PeerRole::Performer, &["inventory-health"]);
    Fixture {
        _temp: temp,
        identity,
        conductor_identity,
        registry,
        conductor,
        conductor_key,
        performer,
        clock: TestClock::at(BASE_NOW),
        facts: MutableFacts::new(),
    }
}

/// Grant the frozen `notifications` capability to this node's Conductor.
///
/// Signals require it, and the base fixture deliberately does not have it,
/// so every pre-existing Profile/Pulse test also proves that a Performer
/// without `notifications` never emits a Signal.
fn grant_notifications(fixture: &Fixture) {
    fixture
        .registry
        .update_peer_capabilities(
            &fixture.conductor,
            vec!["inventory-health".to_string(), "notifications".to_string()],
            "direct-health-tests",
            "grant the frozen notifications capability",
        )
        .expect("grant notifications");
}

/// Drive the session to the point where the Signal feed is the only thing
/// left to send: Profile acknowledged, Pulse acknowledged, cadence armed.
fn settle(fixture: &Fixture, session: &mut HealthSession<'_>) {
    let (kind, profile) = decode(fixture, &session.tick().expect("profile"));
    assert_eq!(kind, "health_profile");
    ack(session, &profile);
    let (kind, pulse) = decode(fixture, &session.tick().expect("pulse"));
    assert_eq!(kind, "health_pulse");
    ack(session, &pulse);
}

impl Fixture {
    fn session(&self, peer: &str, peer_key: [u8; 32]) -> HealthSession<'_> {
        let reporter = Arc::new(HealthReporter::new(Box::new(Arc::clone(&self.facts))));
        HealthSession::with_clock(
            &self.identity,
            &self.registry,
            peer,
            &peer_key,
            SESSION_ID,
            Some(reporter),
            Arc::clone(&self.clock) as Arc<dyn HealthClock>,
        )
    }

    fn conductor_session(&self) -> HealthSession<'_> {
        self.session(&self.conductor, self.conductor_key)
    }
}

/// Decode one emitted envelope, verifying it with the frozen verifier so a
/// test can never assert on bytes the production receiver would reject.
fn decode(fixture: &Fixture, encoded: &[u8]) -> (String, Value) {
    let kind = envelope_kind_hint(encoded).expect("kind hint").to_string();
    let nonce = crate::direct_transport::envelope_nonce(encoded).expect("nonce");
    let local = fixture.identity.public_status();
    let key: [u8; 32] = hex::decode_array(&local.public_key_hex).unwrap();
    verify_envelope(encoded, &local.node_id, &key, &kind, &SESSION_ID, &nonce)
        .expect("emitted envelope must satisfy the frozen verifier");
    let view = envelope_view(encoded).expect("view");
    (kind, view.payload)
}

/// Feed the session the acknowledgement its own Conductor would return, so
/// the pending send resolves without a socket.
fn ack(session: &mut HealthSession<'_>, payload: &Value) {
    let acked = payload["message_id"].as_str().expect("message id");
    session.absorb_reply(
        &serde_json::json!({
            "ack": {"accepted": true, "acked_message_id": acked, "cursor": 0}
        }),
        Some(HealthKind::Ack),
        true,
    );
}

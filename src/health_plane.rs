//! Protocol-neutral Health Plane domain types and shared operations.
//!
//! This module is the single fail-closed owner of Health Plane authorization,
//! freshness, idempotency, ordering, bounded storage, and fleet-status
//! derivation.  It knows nothing about transport framing, scheduling, or any
//! product adapter: callers hand it an already-authenticated message and it
//! returns a stable decision plus the reply the frozen contract permits.
//!
//! Every bound it enforces is transcribed from `docs/internal/health-plane-contract.md`
//! into [`bounds`]; none of them is derived, negotiated, or widened at runtime.

pub mod bounds;
pub mod lifecycle;
pub mod model;
pub mod report;
pub mod schema;

use crate::node_registry::health::{
    HealthApplyRequest, HealthAuditEvent, HealthAuthorization, HealthFleetPeer, HealthOutboxEntry,
    HealthPruneReport,
};
use crate::node_registry::{NodeRegistry, PeerRole, PeerState, RegistryError};
use bounds::{PROCESSING_BUDGET_MILLIS, SIGNATURE_BYTES};
use model::{
    HealthCode, HealthDecision, HealthKind, Presence, ProfileSnapshot, PulseSnapshot, RunFact,
    SignalKind, SignalRecord,
};
use serde::Serialize;
use serde_json::Value;
use std::time::Instant;

/// Injected time. Production reads the system clock; tests drive it directly.
pub trait HealthClock: Send + Sync {
    /// UTC Unix seconds, the only clock source the contract permits.
    fn unix_seconds(&self) -> i64;
    /// Monotonic milliseconds used only for the per-message processing budget.
    fn monotonic_millis(&self) -> u64;
}

/// The production clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemHealthClock {
    started: Option<Instant>,
}

impl SystemHealthClock {
    /// Build a clock anchored at the current instant.
    pub fn new() -> Self {
        Self {
            started: Some(Instant::now()),
        }
    }
}

impl HealthClock for SystemHealthClock {
    fn unix_seconds(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn monotonic_millis(&self) -> u64 {
        match self.started {
            Some(started) => started.elapsed().as_millis() as u64,
            None => 0,
        }
    }
}

/// One inbound Health Plane message whose transport framing, session binding,
/// and BIP-340 envelope signature the caller has already verified.
#[derive(Debug, Clone, Copy)]
pub struct InboundHealthMessage<'a> {
    /// The session's authenticated node ID.
    pub sender: &'a str,
    /// The envelope `kind` string.
    pub kind: &'a str,
    /// The envelope `created_at`, in UTC Unix seconds.
    pub created_at: i64,
    /// The canonical envelope byte length, excluding the 64-byte signature.
    pub canonical_len: usize,
    /// The envelope `payload` object.
    pub payload: &'a Value,
}

/// The reply the frozen contract permits for one evaluated message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthReply {
    /// Drop and audit: the sender was not yet authorized and target-bound.
    None,
    /// Positive acknowledgement carrying the receiver's Signal cursor.
    Ack {
        acked_message_id: String,
        cursor: u64,
    },
    /// Bounded rejection carrying only a stable code and its name.
    Error {
        acked_message_id: String,
        code: HealthCode,
    },
}

/// The complete outcome of evaluating one inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthIngest {
    pub kind: Option<HealthKind>,
    pub message_id: Option<String>,
    pub decision: HealthDecision,
    pub reply: HealthReply,
}

impl HealthIngest {
    /// The stable rejection code, when the message was rejected.
    pub fn code(&self) -> Option<HealthCode> {
        self.decision.code()
    }

    /// Whether the message was applied to Health Plane state.
    pub fn accepted(&self) -> bool {
        matches!(self.decision, HealthDecision::Accepted { .. })
    }
}

/// What a Conductor concludes about one Performer's baseline.
///
/// Derived here and stored nowhere. A Performer reports two facts — the set it
/// recorded installing and the set its disk currently holds — and never a
/// verdict, because it does not know what it was supposed to have. This is the
/// comparison, and it is recomputed from the stored Profile on every read, so a
/// Profile that arrives after a script changed moves the answer with no second
/// row to keep in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineStatus {
    /// No Profile has arrived, so this node has said nothing either way.
    /// Deliberately not `None`: "has not reported" and "reported holding
    /// nothing" are different facts and neither is a drift verdict.
    Unknown,
    /// The Performer reported holding no baseline. It was never pushed one, so
    /// it is neither in sync nor drifted.
    None,
    /// What is on disk is the set the Performer recorded installing.
    InSync,
    /// It is not.
    Drifted,
}

impl BaselineStatus {
    /// Read the verdict out of a stored Profile.
    ///
    /// The closed schema already refuses evidence without a claim, so the only
    /// pairs that reach here are the four the contract names.
    fn derive(profile: Option<&ProfileSnapshot>) -> Self {
        let Some(profile) = profile else {
            return Self::Unknown;
        };
        if profile.baseline_id.is_empty() {
            return Self::None;
        }
        if profile.baseline_id == profile.baseline_observed_id {
            Self::InSync
        } else {
            Self::Drifted
        }
    }
}

/// One row of the Conductor-local fleet-status projection.
///
/// Every field is privacy class P0. No hostname, username, address, path,
/// gauge, or payload body can reach this type, because the closed schema
/// rejects those fields before anything is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FleetNode {
    pub node_id: String,
    pub role: String,
    pub capabilities: Vec<String>,
    pub trust_state: String,
    pub presence: Presence,
    pub last_pulse_at: Option<i64>,
    /// The comparison of the two baseline facts in `profile`, derived on read.
    pub baseline_status: BaselineStatus,
    pub profile: Option<ProfileSnapshot>,
    pub pulse: Option<PulseSnapshot>,
    pub signal_cursor: u64,
    pub stored_signals: u64,
    pub held_signals: u64,
    pub version_incompatible: bool,
}

/// The bounded Signal read surface, as one snapshot of one instant.
///
/// Every field below was read in the same registry transaction, which is what
/// lets the caller render cursors and Signals that agree with each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetSignalFeed {
    /// The UTC Unix second the whole feed was read at.
    pub observed_at: i64,
    /// The Conductor-local lifecycle Signals, projected from the trust log.
    pub local: Vec<SignalRecord>,
    /// Per-peer cursor state, ordered by node ID.
    pub nodes: Vec<FleetSignalCursor>,
    /// The bounded page reported by Performers, newest first.
    pub signals: Vec<FleetSignal>,
}

/// One Performer's Signal cursor state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetSignalCursor {
    pub node_id: String,
    pub trust_state: String,
    pub cursor: u64,
    pub stored: u64,
    pub held: u64,
}

/// One Signal in the bounded page, with the Performer that reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetSignal {
    pub source: String,
    pub signal: SignalRecord,
}

/// The shared, protocol-neutral Health Plane operations.
pub struct HealthPlane<'registry> {
    registry: &'registry NodeRegistry,
    clock: Box<dyn HealthClock>,
}

impl<'registry> HealthPlane<'registry> {
    /// Build the operations facade over the production clock.
    pub fn new(registry: &'registry NodeRegistry) -> Self {
        Self::with_clock(registry, Box::new(SystemHealthClock::new()))
    }

    /// Build the operations facade over an injected clock.
    pub fn with_clock(registry: &'registry NodeRegistry, clock: Box<dyn HealthClock>) -> Self {
        Self { registry, clock }
    }

    /// The current UTC Unix second according to the injected clock.
    pub fn now(&self) -> i64 {
        self.clock.unix_seconds()
    }

    /// Whether Health Plane storage is available on this node.
    pub fn enabled(&self) -> Result<bool, RegistryError> {
        self.registry.health_plane_enabled()
    }

    /// Evaluate one inbound message under the frozen receive order.
    ///
    /// Steps 1 and 3 (transport framing, envelope shape, and signature) are the
    /// caller's responsibility. This method performs step 2 (size), step 4
    /// (version), step 5 (strict closed schema), step 6 (target binding), and
    /// then hands steps 7 through 15 to the registry, which applies them in
    /// exactly one transaction.
    pub fn ingest(&self, message: InboundHealthMessage<'_>) -> Result<HealthIngest, RegistryError> {
        let now = self.clock.unix_seconds();
        let started = self.clock.monotonic_millis();
        let byte_count = (message.canonical_len + SIGNATURE_BYTES) as i64;

        // Fail closed when the Health Plane migration did not land: the node
        // keeps serving transport, enrollment, HTTP, and runs, and every Health
        // Plane message is refused instead of half-applied.
        if !self.registry.health_plane_enabled()? {
            return Ok(HealthIngest {
                kind: HealthKind::parse(message.kind),
                message_id: None,
                decision: HealthDecision::Rejected(HealthCode::CorruptState),
                reply: HealthReply::None,
            });
        }

        // The sender is the session's authenticated node ID. A syntactically
        // impossible one cannot be audited against a peer, so it is dropped.
        if !is_node_id(message.sender) {
            return Ok(HealthIngest {
                kind: HealthKind::parse(message.kind),
                message_id: None,
                decision: HealthDecision::Rejected(HealthCode::InvalidMessage),
                reply: HealthReply::None,
            });
        }

        // Step 3 completion: the envelope kind must be one of the closed five.
        let Some(kind) = HealthKind::parse(message.kind) else {
            return self.reject_before_storage(
                &message,
                None,
                None,
                HealthCode::UnknownField,
                byte_count,
                now,
            );
        };

        // Step 2: size, before parsing.
        if message.canonical_len > kind.max_canonical_bytes() {
            return self.reject_before_storage(
                &message,
                Some(kind),
                None,
                HealthCode::MessageTooLarge,
                byte_count,
                now,
            );
        }

        // Step 4: Health Plane version.
        if let Err(code) = schema::validate_version(message.payload) {
            if code == HealthCode::UnsupportedVersion {
                return self.reject_unsupported_version(&message, kind, byte_count, now);
            }
            return self.reject_before_storage(&message, Some(kind), None, code, byte_count, now);
        }

        // Step 5: strict closed schema.
        let payload = match schema::validate_payload(kind, message.payload, message.created_at) {
            Ok(payload) => payload,
            Err(code) => {
                return self.reject_before_storage(
                    &message,
                    Some(kind),
                    None,
                    code,
                    byte_count,
                    now,
                )
            }
        };

        // Step 6: target binding.
        if payload.target != self.registry.local_node_id() {
            return self.reject_before_storage(
                &message,
                Some(kind),
                Some(payload.message_id.clone()),
                HealthCode::WrongTarget,
                byte_count,
                now,
            );
        }

        // Receiver processing budget. The budget is checked before anything is
        // applied, so an over-budget message can never leave a partial write.
        if self.clock.monotonic_millis().saturating_sub(started) >= PROCESSING_BUDGET_MILLIS {
            return self.reject_before_storage(
                &message,
                Some(kind),
                Some(payload.message_id.clone()),
                HealthCode::CorruptState,
                byte_count,
                now,
            );
        }

        // Steps 7 through 15, in exactly one transaction.
        let decision = self.registry.apply_health_message(HealthApplyRequest {
            sender: message.sender,
            payload: &payload,
            created_at: message.created_at,
            now,
            message_bytes: byte_count,
        })?;
        let reply = self.reply_for(kind, &payload.message_id, &decision);
        Ok(HealthIngest {
            kind: Some(kind),
            message_id: Some(payload.message_id),
            decision,
            reply,
        })
    }

    /// The fleet-status projection over every actively trusted peer.
    ///
    /// The whole projection comes from one registry snapshot. Read peer by
    /// peer, it could report counts, presence, and baselines that belong to
    /// different instants, which is a status report of a fleet that never
    /// existed.
    pub fn fleet_status(&self) -> Result<Vec<FleetNode>, RegistryError> {
        let now = self.clock.unix_seconds();
        Ok(self
            .registry
            .health_fleet_snapshot(now)?
            .into_iter()
            .map(|peer| project(peer, now))
            .collect())
    }

    /// The fleet-status projection for one peer.
    pub fn node_status(&self, node_id: &str) -> Result<Option<FleetNode>, RegistryError> {
        let now = self.clock.unix_seconds();
        Ok(self
            .registry
            .health_node_snapshot(node_id, now)?
            .map(|peer| project(peer, now)))
    }

    /// The bounded Signal read surface, read as one snapshot.
    ///
    /// The cursors, the Signals, and the trust transitions the local
    /// lifecycle Signals project from all describe `observed_at`. A feed
    /// assembled from separate reads can contradict itself — a Signal beside
    /// a cursor that has not counted it — and `gap`, which tells an operator
    /// whether delivery has stalled, is derived from the same counters.
    pub fn signal_feed(&self, limit: usize) -> Result<FleetSignalFeed, RegistryError> {
        let observed_at = self.clock.unix_seconds();
        let feed = self.registry.health_signal_feed(limit, observed_at)?;
        let nodes = feed
            .peers
            .into_iter()
            .map(|peer| FleetSignalCursor {
                node_id: peer.state.node_id,
                trust_state: peer
                    .authorization
                    .map(|authorization| peer_state_name(authorization.state).to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                cursor: peer.state.cursor,
                stored: peer.state.stored_signals,
                held: peer.state.held_signals,
            })
            .collect();
        let signals = feed
            .signals
            .into_iter()
            .map(|entry| FleetSignal {
                source: entry.node_id,
                signal: entry.signal,
            })
            .collect();
        Ok(FleetSignalFeed {
            observed_at,
            local: lifecycle::project(&feed.lifecycle, observed_at, limit),
            nodes,
            signals,
        })
    }

    /// The bounded, ordered Signal inbox for one peer.
    pub fn signals(&self, node_id: &str, limit: usize) -> Result<Vec<SignalRecord>, RegistryError> {
        self.registry
            .health_signals(node_id, limit, self.clock.unix_seconds())
    }

    /// The bounded, newest-first Conductor-local lifecycle Signal feed.
    ///
    /// `enrolled` and `revoked` are decided by this node, so they are
    /// projected from the append-only trust audit rather than received,
    /// stored, or re-derived. Nothing is written by this call. See
    /// [`lifecycle`] for why projection is the only revocation-safe shape.
    pub fn local_signals(&self, limit: usize) -> Result<Vec<SignalRecord>, RegistryError> {
        let now = self.clock.unix_seconds();
        let events = self.registry.lifecycle_trust_events(usize::MAX)?;
        Ok(lifecycle::project(&events, now, limit))
    }

    /// The read-only authorization projection for one peer.
    pub fn authorization(
        &self,
        node_id: &str,
    ) -> Result<Option<HealthAuthorization>, RegistryError> {
        self.registry.health_authorization(node_id)
    }

    /// Append one Signal to the bounded Performer outbox.
    #[allow(clippy::too_many_arguments)]
    pub fn enqueue_signal(
        &self,
        target_node_id: &str,
        signal_id: &str,
        kind: SignalKind,
        occurred_at: i64,
        subject: Option<&str>,
        run: Option<&RunFact>,
        message_bytes: i64,
    ) -> Result<HealthOutboxEntry, RegistryError> {
        self.registry.health_enqueue_signal(
            target_node_id,
            signal_id,
            kind,
            occurred_at,
            subject,
            run,
            message_bytes,
            self.clock.unix_seconds(),
        )
    }

    /// Read the bounded Performer outbox in send order.
    pub fn outbox(&self, limit: usize) -> Result<Vec<HealthOutboxEntry>, RegistryError> {
        self.registry.health_outbox(limit)
    }

    /// Bind one outbox Signal to the `message_id` of a send attempt.
    pub fn mark_signal_sent(
        &self,
        signal_id: &str,
        message_id: &str,
    ) -> Result<bool, RegistryError> {
        self.registry
            .health_mark_signal_sent(signal_id, message_id, self.clock.unix_seconds())
    }

    /// Re-arm the delivery budget of every Signal queued for one peer.
    ///
    /// The frozen retry bound is per message *per session*: a Signal that
    /// spent its three attempts is retained in the bounded outbox and resent on
    /// the next session. Callers invoke this once, when a session to that peer
    /// is established; it re-arms nothing else and widens no bound.
    pub fn reset_outbox_attempts(&self, target_node_id: &str) -> Result<u64, RegistryError> {
        self.registry
            .health_reset_outbox_attempts(target_node_id, self.clock.unix_seconds())
    }

    /// How many Signals outbox overflow has dropped on this node.
    pub fn signals_dropped(&self) -> Result<i64, RegistryError> {
        self.registry.health_signals_dropped()
    }

    /// Enforce every retention and capacity bound.
    pub fn prune(&self) -> Result<HealthPruneReport, RegistryError> {
        self.registry.health_prune(self.clock.unix_seconds())
    }

    /// Delete Health Plane state for peers that are no longer actively trusted.
    pub fn purge_revoked(&self) -> Result<Vec<String>, RegistryError> {
        self.registry
            .health_purge_revoked(self.clock.unix_seconds())
    }

    /// The bytes the Health Plane currently accounts for.
    pub fn storage_bytes(&self) -> Result<i64, RegistryError> {
        self.registry.health_storage_bytes()
    }

    /// The redacted Health Plane audit trail, newest first.
    pub fn audit_events(&self, limit: usize) -> Result<Vec<HealthAuditEvent>, RegistryError> {
        self.registry.health_audit_events(limit)
    }

    fn reply_for(
        &self,
        kind: HealthKind,
        message_id: &str,
        decision: &HealthDecision,
    ) -> HealthReply {
        // A Conductor never replies to a reply.
        if matches!(kind, HealthKind::Ack | HealthKind::Error) {
            return HealthReply::None;
        }
        match decision {
            HealthDecision::Accepted { cursor } | HealthDecision::Held { cursor } => {
                HealthReply::Ack {
                    acked_message_id: message_id.to_string(),
                    cursor: *cursor,
                }
            }
            HealthDecision::Rejected(code) => {
                // A `health_error` is emitted only once the sender is
                // authenticated, authorized, and target-bound. Trust, role, and
                // capability failures are dropped and audited instead.
                if matches!(
                    code,
                    HealthCode::Revoked | HealthCode::WrongRole | HealthCode::MissingCapability
                ) {
                    HealthReply::None
                } else {
                    HealthReply::Error {
                        acked_message_id: message_id.to_string(),
                        code: *code,
                    }
                }
            }
        }
    }

    fn reject_before_storage(
        &self,
        message: &InboundHealthMessage<'_>,
        kind: Option<HealthKind>,
        message_id: Option<String>,
        code: HealthCode,
        byte_count: i64,
        now: i64,
    ) -> Result<HealthIngest, RegistryError> {
        self.audit(
            message.sender,
            kind,
            byte_count,
            "rejected",
            Some(code),
            now,
        )?;
        Ok(HealthIngest {
            kind,
            message_id,
            decision: HealthDecision::Rejected(code),
            reply: HealthReply::None,
        })
    }

    /// The mixed-version path.
    ///
    /// The receive order rejects an unsupported `health_version` at step 4,
    /// before target binding and authorization. The frozen mixed-version policy
    /// nevertheless requires the Conductor to reply `health_error` 1101 to the
    /// Performer it can identify, so this path re-establishes exactly the two
    /// preconditions a reply needs - target binding and active authorization -
    /// without reading any other payload field.
    fn reject_unsupported_version(
        &self,
        message: &InboundHealthMessage<'_>,
        kind: HealthKind,
        byte_count: i64,
        now: i64,
    ) -> Result<HealthIngest, RegistryError> {
        let code = HealthCode::UnsupportedVersion;
        self.audit(
            message.sender,
            Some(kind),
            byte_count,
            "rejected",
            Some(code),
            now,
        )?;
        let message_id = schema::peek_message_id(message.payload);
        let target = schema::peek_target(message.payload);
        let addressed = target.as_deref() == Some(self.registry.local_node_id());
        let authorized = match self.registry.health_authorization(message.sender)? {
            Some(authorization) => {
                authorization.state == PeerState::Active
                    && role_code(authorization.role) == kind.required_role()
            }
            None => false,
        };
        let reply = match (&message_id, addressed && authorized) {
            (Some(message_id), true) => {
                self.registry
                    .mark_health_version_incompatible(message.sender, now)?;
                HealthReply::Error {
                    acked_message_id: message_id.clone(),
                    code,
                }
            }
            _ => HealthReply::None,
        };
        Ok(HealthIngest {
            kind: Some(kind),
            message_id,
            decision: HealthDecision::Rejected(code),
            reply,
        })
    }

    fn audit(
        &self,
        sender: &str,
        kind: Option<HealthKind>,
        byte_count: i64,
        outcome: &str,
        code: Option<HealthCode>,
        now: i64,
    ) -> Result<(), RegistryError> {
        let kind_name = kind.map(HealthKind::wire).unwrap_or("unknown");
        self.registry.record_health_audit(
            kind_name,
            sender,
            kind_name,
            byte_count,
            outcome,
            code.map(HealthCode::code),
            now,
        )
    }
}

fn is_node_id(value: &str) -> bool {
    value.len() == bounds::NODE_ID_BYTES
        && value.starts_with("omk1_")
        && value[5..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn role_code(role: PeerRole) -> i64 {
    match role {
        PeerRole::Conductor => bounds::ROLE_CONDUCTOR,
        PeerRole::Performer => bounds::ROLE_PERFORMER,
    }
}

/// Render one fleet-status row from the snapshot it was read in.
fn project(peer: HealthFleetPeer, now: i64) -> FleetNode {
    let snapshot = peer.snapshot;
    let (trust_state, capabilities) = match peer.authorization {
        Some(authorization) => (
            peer_state_name(authorization.state).to_string(),
            authorization.capabilities,
        ),
        None => ("unknown".to_string(), Vec::new()),
    };
    FleetNode {
        node_id: snapshot.state.node_id.clone(),
        role: peer_role_name(snapshot.state.role).to_string(),
        capabilities,
        trust_state,
        presence: Presence::derive(snapshot.state.last_pulse_at, now),
        last_pulse_at: snapshot.state.last_pulse_at,
        baseline_status: BaselineStatus::derive(snapshot.profile.as_ref()),
        profile: snapshot.profile,
        pulse: snapshot.pulse,
        signal_cursor: snapshot.state.cursor,
        stored_signals: snapshot.state.stored_signals,
        held_signals: snapshot.state.held_signals,
        version_incompatible: snapshot.state.version_incompatible,
    }
}

fn peer_role_name(role: PeerRole) -> &'static str {
    match role {
        PeerRole::Conductor => "conductor",
        PeerRole::Performer => "performer",
    }
}

fn peer_state_name(state: PeerState) -> &'static str {
    match state {
        PeerState::Pending => "pending",
        PeerState::Active => "active",
        PeerState::Suspended => "suspended",
        PeerState::Revoked => "revoked",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NodeContext, NodePathOverrides, NodePlatform};
    use crate::node_identity::{node_id_for_x_only_public_key, NodeIdentity};
    use crate::node_registry::{PeerRegistration, PeerSource};
    use bounds::{
        MAX_AGE_SECONDS, MAX_CANONICAL_PROFILE, MAX_FUTURE_SKEW_SECONDS, PRESENCE_ONLINE_SECONDS,
        PRESENCE_STALE_SECONDS,
    };
    use serde_json::json;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

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

    fn scalar(seed: u32) -> [u8; 32] {
        let mut value = [0_u8; 32];
        value[28..].copy_from_slice(&seed.saturating_add(1).to_be_bytes());
        value
    }

    fn peer_identity(seed: u32) -> (String, String) {
        let key = k256::schnorr::SigningKey::from_slice(&scalar(seed)).unwrap();
        let xonly = key.verifying_key().to_bytes();
        let public_key = xonly.iter().map(|byte| format!("{byte:02x}")).collect();
        (node_id_for_x_only_public_key(&xonly), public_key)
    }

    fn fixture() -> Fixture {
        let temp = TempDir::new().unwrap();
        let context = NodeContext::resolve_for(
            NodePlatform::current(),
            NodePathOverrides::new(
                Some(temp.path().join("state")),
                Some(temp.path().join("node.toml")),
            ),
            true,
            None,
            None,
            None,
        )
        .unwrap();
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        let trust = |seed: u32, role: PeerRole, capabilities: &[&str]| {
            let (node_id, public_key) = peer_identity(seed);
            registry
                .import_manual_peer(PeerRegistration {
                    node_id: node_id.clone(),
                    public_key,
                    role,
                    capabilities: capabilities.iter().map(|entry| entry.to_string()).collect(),
                    source: PeerSource::Manual,
                    actor: "health-plane-tests".to_string(),
                    reason: "health plane operations test peer".to_string(),
                })
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

        fn ingest(
            &self,
            sender: &str,
            kind: &str,
            created_at: i64,
            payload: &Value,
        ) -> HealthIngest {
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

    #[test]
    fn the_frozen_registry_schema_version_matches_the_shipped_one() {
        assert_eq!(
            bounds::REGISTRY_SCHEMA_VERSION,
            crate::node_registry::SCHEMA_VERSION
        );
    }

    #[test]
    fn a_syntactically_impossible_sender_is_dropped_without_a_reply() {
        let fixture = fixture();
        let outcome = fixture.ingest(
            "not-a-node-id",
            "health_profile",
            BASE_NOW,
            &profile_payload(&fixture.local, 1, 1),
        );
        assert_eq!(outcome.code(), Some(HealthCode::InvalidMessage));
        assert_eq!(outcome.reply, HealthReply::None);
        assert!(fixture.plane().audit_events(10).unwrap().is_empty());
        assert!(fixture.plane().fleet_status().unwrap().is_empty());
    }

    #[test]
    fn a_complete_reporting_sequence_is_accepted_and_acknowledged() {
        let fixture = fixture();
        let plane = fixture.plane();

        let profile = fixture.ingest(
            &fixture.performer,
            "health_profile",
            BASE_NOW,
            &profile_payload(&fixture.local, 1, 1),
        );
        assert!(profile.accepted());
        assert_eq!(
            profile.reply,
            HealthReply::Ack {
                acked_message_id: hex16(1),
                cursor: 0
            }
        );

        fixture.clock.set(BASE_NOW + 30);
        let pulse = fixture.ingest(
            &fixture.performer,
            "health_pulse",
            BASE_NOW + 30,
            &pulse_payload(&fixture.local, 2, 1, BASE_NOW + 30),
        );
        assert!(pulse.accepted());

        fixture.clock.set(BASE_NOW + 60);
        let signal = fixture.ingest(
            &fixture.performer,
            "health_signal",
            BASE_NOW + 60,
            &signal_payload(&fixture.local, 3, 1, 101, BASE_NOW + 60),
        );
        assert_eq!(
            signal.reply,
            HealthReply::Ack {
                acked_message_id: hex16(3),
                cursor: 1
            }
        );

        let fleet = plane.fleet_status().unwrap();
        assert_eq!(fleet.len(), 1);
        let node = &fleet[0];
        assert_eq!(node.node_id, fixture.performer);
        assert_eq!(node.role, "performer");
        assert_eq!(node.trust_state, "active");
        assert_eq!(node.presence, Presence::Online);
        assert_eq!(node.signal_cursor, 1);
        assert_eq!(node.stored_signals, 1);
        assert_eq!(node.profile.as_ref().unwrap().profile_revision, 1);
        assert_eq!(node.pulse.as_ref().unwrap().sequence, 1);
        assert!(!node.version_incompatible);
        assert_eq!(plane.signals(&fixture.performer, 64).unwrap().len(), 1);
    }

    #[test]
    fn presence_boundaries_are_exact_and_derived_only_from_the_last_pulse() {
        let fixture = fixture();
        assert_eq!(Presence::derive(None, BASE_NOW), Presence::Unknown);
        fixture.ingest(
            &fixture.performer,
            "health_pulse",
            BASE_NOW,
            &pulse_payload(&fixture.local, 1, 1, BASE_NOW),
        );
        for (offset, expected) in [
            (0, Presence::Online),
            (PRESENCE_ONLINE_SECONDS, Presence::Online),
            (PRESENCE_ONLINE_SECONDS + 1, Presence::Stale),
            (PRESENCE_STALE_SECONDS, Presence::Stale),
            (PRESENCE_STALE_SECONDS + 1, Presence::Offline),
        ] {
            fixture.clock.set(BASE_NOW + offset);
            let node = fixture
                .plane()
                .node_status(&fixture.performer)
                .unwrap()
                .unwrap();
            assert_eq!(node.presence, expected, "at offset {offset}");
        }
    }

    /// The two baseline fields are a claim and its evidence, and the closed
    /// schema is what keeps both readable.
    ///
    /// The drift verdict is a comparison of these two strings, so a receiver
    /// that accepted a half-width identity would compare a truncated name
    /// against a whole one and read that as drift; and one that accepted
    /// evidence without a claim would store a verdict no set on disk could
    /// justify.
    #[test]
    fn a_profile_carrying_an_unreadable_baseline_pair_is_refused() {
        let fixture = fixture();
        let target = fixture.local.clone();
        let identity = "a".repeat(64);

        let mut accepted = profile_payload(&target, 1, 1);
        accepted["profile"]["baseline_id"] = json!(identity);
        accepted["profile"]["baseline_observed_id"] = json!(identity);
        assert!(
            fixture
                .ingest(&fixture.performer, "health_profile", BASE_NOW, &accepted)
                .accepted(),
            "a Performer reporting the set it holds must be accepted"
        );

        for (recorded, observed, why) in [
            (
                identity[..63].to_string(),
                identity.clone(),
                "an identity one character short is not a shorter identity",
            ),
            (
                identity.clone(),
                identity.to_uppercase(),
                "uppercase hex names the same bytes and must still be refused, \
                 because two spellings of one set would read as drift",
            ),
            (
                String::new(),
                identity.clone(),
                "evidence without a claim is a verdict no record justifies",
            ),
        ] {
            let mut payload = profile_payload(&target, 2, 2);
            payload["profile"]["baseline_id"] = json!(recorded);
            payload["profile"]["baseline_observed_id"] = json!(observed);
            assert_eq!(
                fixture
                    .ingest(&fixture.performer, "health_profile", BASE_NOW, &payload)
                    .code(),
                Some(HealthCode::InvalidMessage),
                "{why}"
            );
        }
    }

    #[test]
    fn every_contracted_rejection_produces_its_stable_code() {
        let fixture = fixture();
        let target = fixture.local.clone();

        // An envelope kind outside the closed set.
        assert_eq!(
            fixture.code(
                &fixture.performer,
                "health_inventory",
                BASE_NOW,
                &profile_payload(&target, 1, 1)
            ),
            HealthCode::UnknownField
        );
        // Oversize, checked before parsing.
        assert_eq!(
            fixture
                .plane()
                .ingest(InboundHealthMessage {
                    sender: &fixture.performer,
                    kind: "health_profile",
                    created_at: BASE_NOW,
                    canonical_len: MAX_CANONICAL_PROFILE + 1,
                    payload: &profile_payload(&target, 1, 1),
                })
                .unwrap()
                .code()
                .unwrap(),
            HealthCode::MessageTooLarge
        );
        // Missing and unknown fields.
        let mut payload = profile_payload(&target, 1, 1);
        payload.as_object_mut().unwrap().remove("health_version");
        assert_eq!(
            fixture.code(&fixture.performer, "health_profile", BASE_NOW, &payload),
            HealthCode::UnknownField
        );
        let mut payload = profile_payload(&target, 1, 1);
        payload.as_object_mut().unwrap()["profile"]
            .as_object_mut()
            .unwrap()
            .insert("hostname".to_string(), json!("workshop.local"));
        assert_eq!(
            fixture.code(&fixture.performer, "health_profile", BASE_NOW, &payload),
            HealthCode::UnknownField
        );
        // An unsupported schema version.
        let mut payload = profile_payload(&target, 1, 1);
        payload.as_object_mut().unwrap()["health_version"] = json!(2);
        assert_eq!(
            fixture.code(&fixture.performer, "health_profile", BASE_NOW, &payload),
            HealthCode::UnsupportedVersion
        );
        // Grammar, secret references, and floating point numbers.
        let mut payload = profile_payload(&target, 1, 1);
        payload.as_object_mut().unwrap()["profile"]
            .as_object_mut()
            .unwrap()["display_name"] = json!("/etc/shadow");
        assert_eq!(
            fixture.code(&fixture.performer, "health_profile", BASE_NOW, &payload),
            HealthCode::InvalidMessage
        );
        let mut payload = profile_payload(&target, 1, 1);
        payload.as_object_mut().unwrap()["profile"]
            .as_object_mut()
            .unwrap()["distro_version"] = json!("secret://vault/token");
        assert_eq!(
            fixture.code(&fixture.performer, "health_profile", BASE_NOW, &payload),
            HealthCode::InvalidMessage
        );
        let mut payload = pulse_payload(&target, 1, 1, BASE_NOW);
        payload.as_object_mut().unwrap()["pulse"]
            .as_object_mut()
            .unwrap()["uptime_seconds"] = json!(1.5);
        assert_eq!(
            fixture.code(&fixture.performer, "health_pulse", BASE_NOW, &payload),
            HealthCode::InvalidMessage
        );
        // A third party's node ID as the target.
        let (other, _) = peer_identity(40);
        assert_eq!(
            fixture.code(
                &fixture.performer,
                "health_profile",
                BASE_NOW,
                &profile_payload(&other, 1, 1)
            ),
            HealthCode::WrongTarget
        );
        // Role and capability.
        assert_eq!(
            fixture.code(
                &fixture.conductor,
                "health_profile",
                BASE_NOW,
                &profile_payload(&target, 1, 1)
            ),
            HealthCode::WrongRole
        );
        assert_eq!(
            fixture.code(
                &fixture.limited,
                "health_profile",
                BASE_NOW,
                &profile_payload(&target, 2, 1)
            ),
            HealthCode::MissingCapability
        );
        // An identity the registry has never seen.
        let (stranger, _) = peer_identity(41);
        assert_eq!(
            fixture.code(
                &stranger,
                "health_profile",
                BASE_NOW,
                &profile_payload(&target, 3, 1)
            ),
            HealthCode::Revoked
        );
        // Freshness, inclusive at exactly the frozen boundaries.
        assert_eq!(
            fixture.code(
                &fixture.performer,
                "health_profile",
                BASE_NOW - MAX_AGE_SECONDS - 1,
                &profile_payload(&target, 4, 1)
            ),
            HealthCode::Stale
        );
        assert_eq!(
            fixture.code(
                &fixture.performer,
                "health_profile",
                BASE_NOW + MAX_FUTURE_SKEW_SECONDS + 1,
                &profile_payload(&target, 5, 1)
            ),
            HealthCode::Future
        );
        // Replay and reordering.
        assert!(fixture
            .ingest(
                &fixture.performer,
                "health_profile",
                BASE_NOW,
                &profile_payload(&target, 6, 1)
            )
            .accepted());
        assert_eq!(
            fixture.code(
                &fixture.performer,
                "health_profile",
                BASE_NOW,
                &profile_payload(&target, 6, 2)
            ),
            HealthCode::Replay
        );
        assert_eq!(
            fixture.code(
                &fixture.performer,
                "health_signal",
                BASE_NOW,
                &signal_payload(&target, 7, 99, 199, BASE_NOW)
            ),
            HealthCode::Reordered
        );

        // Nothing above created or reactivated a trust row.
        let authorization = fixture.plane().authorization(&stranger).unwrap();
        assert!(authorization.is_none());
    }

    #[test]
    fn a_health_error_is_only_sent_to_an_authorized_target_bound_peer() {
        let fixture = fixture();
        let target = fixture.local.clone();

        // Trust, role, and capability failures are dropped and audited only.
        for (sender, payload) in [
            (&fixture.conductor, profile_payload(&target, 1, 1)),
            (&fixture.limited, profile_payload(&target, 2, 1)),
        ] {
            let outcome = fixture.ingest(sender, "health_profile", BASE_NOW, &payload);
            assert_eq!(outcome.reply, HealthReply::None);
        }
        // A failure after authorization carries the stable code back.
        let outcome = fixture.ingest(
            &fixture.performer,
            "health_profile",
            BASE_NOW - MAX_AGE_SECONDS - 1,
            &profile_payload(&target, 3, 1),
        );
        assert_eq!(
            outcome.reply,
            HealthReply::Error {
                acked_message_id: hex16(3),
                code: HealthCode::Stale
            }
        );
        // The rejections are audited with their stable codes and nothing else.
        let audit = fixture.plane().audit_events(10).unwrap();
        assert!(audit.iter().all(|event| event.outcome != "accepted"));
        assert!(audit
            .iter()
            .any(|event| event.error_code == Some(HealthCode::Stale.code())));
    }

    #[test]
    fn an_unsupported_version_replies_only_when_the_peer_is_addressed_and_authorized() {
        let fixture = fixture();
        let target = fixture.local.clone();
        let mut payload = profile_payload(&target, 1, 1);
        payload.as_object_mut().unwrap()["health_version"] = json!(2);

        // The peer must first be tracked for the projection to record anything.
        assert!(fixture
            .ingest(
                &fixture.performer,
                "health_profile",
                BASE_NOW,
                &profile_payload(&target, 9, 1)
            )
            .accepted());

        let outcome = fixture.ingest(&fixture.performer, "health_profile", BASE_NOW, &payload);
        assert_eq!(outcome.code(), Some(HealthCode::UnsupportedVersion));
        assert_eq!(
            outcome.reply,
            HealthReply::Error {
                acked_message_id: hex16(1),
                code: HealthCode::UnsupportedVersion
            }
        );
        let node = fixture
            .plane()
            .node_status(&fixture.performer)
            .unwrap()
            .unwrap();
        assert!(node.version_incompatible);

        // A message addressed elsewhere gets no reply at all.
        let (other, _) = peer_identity(42);
        let mut payload = profile_payload(&other, 2, 1);
        payload.as_object_mut().unwrap()["health_version"] = json!(3);
        let outcome = fixture.ingest(&fixture.performer, "health_profile", BASE_NOW, &payload);
        assert_eq!(outcome.code(), Some(HealthCode::UnsupportedVersion));
        assert_eq!(outcome.reply, HealthReply::None);

        // Trust and transport state are untouched by a version mismatch.
        let authorization = fixture
            .plane()
            .authorization(&fixture.performer)
            .unwrap()
            .unwrap();
        assert_eq!(authorization.state, PeerState::Active);
        assert_eq!(authorization.role, PeerRole::Performer);
    }

    #[test]
    fn a_held_signal_acknowledges_the_unchanged_cursor() {
        let fixture = fixture();
        let target = fixture.local.clone();
        assert!(fixture
            .ingest(
                &fixture.performer,
                "health_signal",
                BASE_NOW,
                &signal_payload(&target, 1, 1, 101, BASE_NOW)
            )
            .accepted());
        fixture.clock.set(BASE_NOW + 20);
        let held = fixture.ingest(
            &fixture.performer,
            "health_signal",
            BASE_NOW + 20,
            &signal_payload(&target, 2, 3, 103, BASE_NOW + 20),
        );
        assert_eq!(held.decision, HealthDecision::Held { cursor: 1 });
        assert_eq!(
            held.reply,
            HealthReply::Ack {
                acked_message_id: hex16(2),
                cursor: 1
            }
        );
        // The stalled cursor never exposes a hole to a reader.
        assert_eq!(
            fixture
                .plane()
                .signals(&fixture.performer, 64)
                .unwrap()
                .iter()
                .map(|signal| signal.sequence)
                .collect::<Vec<_>>(),
            vec![1]
        );
    }

    /// The verdict is a comparison of two reported facts, made here.
    ///
    /// Every case moves the *Profile* and reads the projection, because a
    /// verdict computed anywhere but from the pair the Performer reported would
    /// be an inference the Conductor is not entitled to make.
    #[test]
    fn a_performers_baseline_reads_as_unknown_none_in_sync_or_drifted() {
        let fixture = fixture();
        let target = fixture.local.clone();
        let installed = "1".repeat(64);
        let on_disk = "2".repeat(64);

        assert_eq!(
            fixture
                .plane()
                .node_status(&fixture.performer)
                .unwrap()
                .map(|node| node.baseline_status),
            None,
            "a peer that has never reported has no row at all"
        );

        // A Performer whose Pulse arrived before its Profile has a row and has
        // still said nothing about a baseline. Reading that as "holds none"
        // would be a verdict on a machine that has not answered.
        let pulse = pulse_payload(&target, 9, 1, BASE_NOW);
        assert!(fixture
            .ingest(&fixture.performer, "health_pulse", BASE_NOW, &pulse)
            .accepted());
        assert_eq!(
            fixture
                .plane()
                .node_status(&fixture.performer)
                .unwrap()
                .expect("a pulsing peer has a row")
                .baseline_status,
            BaselineStatus::Unknown,
            "presence without a Profile is not an answer about a baseline"
        );

        let mut revision = 0;
        let mut report = |recorded: &str, observed: &str| {
            revision += 1;
            let mut payload = profile_payload(&target, revision, revision);
            payload["profile"]["baseline_id"] = json!(recorded);
            payload["profile"]["baseline_observed_id"] = json!(observed);
            assert!(
                fixture
                    .ingest(&fixture.performer, "health_profile", BASE_NOW, &payload)
                    .accepted(),
                "the Profile under test must be accepted, or the verdict is about nothing"
            );
            fixture
                .plane()
                .node_status(&fixture.performer)
                .unwrap()
                .expect("a reporting peer has a row")
                .baseline_status
        };

        assert_eq!(
            report("", ""),
            BaselineStatus::None,
            "a node that was never pushed a baseline has none, which is not a drift verdict"
        );
        assert_eq!(
            report(&installed, &installed),
            BaselineStatus::InSync,
            "a node running what it was pushed is in sync"
        );
        assert_eq!(
            report(&installed, &on_disk),
            BaselineStatus::Drifted,
            "a node whose scripts changed underneath it has drifted"
        );
        assert_eq!(
            report(&installed, &installed),
            BaselineStatus::InSync,
            "putting the set back must clear the verdict, or drift is one-way"
        );
    }

    #[test]
    fn the_public_fleet_projection_carries_only_permitted_fields() {
        let fixture = fixture();
        let target = fixture.local.clone();
        fixture.ingest(
            &fixture.performer,
            "health_profile",
            BASE_NOW,
            &profile_payload(&target, 1, 1),
        );
        fixture.clock.set(BASE_NOW + 30);
        fixture.ingest(
            &fixture.performer,
            "health_pulse",
            BASE_NOW + 30,
            &pulse_payload(&target, 2, 1, BASE_NOW + 30),
        );

        let fleet = fixture.plane().fleet_status().unwrap();
        let rendered = serde_json::to_value(&fleet).unwrap();
        let mut names = Vec::new();
        collect_field_names(&rendered, &mut names);
        names.sort();
        names.dedup();
        const PERMITTED: [&str; 33] = [
            "agent_version",
            "arch",
            "baseline_id",
            "baseline_observed_id",
            "baseline_status",
            "capabilities",
            "display_name",
            "distro_id",
            "distro_version",
            "emitted_at",
            "exit_code",
            "finished_at",
            "held_signals",
            "last_pulse_at",
            "last_run",
            "node_id",
            "omarchy_channel",
            "omarchy_version",
            "platform",
            "presence",
            "profile",
            "profile_revision",
            "pulse",
            "queue_depth",
            "role",
            "runner",
            "runtimes",
            "scheduler",
            "sequence",
            "signal_cursor",
            "state",
            "stored_signals",
            "trust_state",
        ];
        const ALSO_PERMITTED: [&str; 7] = [
            "available",
            "name",
            "uptime_seconds",
            "version",
            "version_incompatible",
            "workers_busy",
            "workers_configured",
        ];
        for name in &names {
            assert!(
                PERMITTED.contains(&name.as_str()) || ALSO_PERMITTED.contains(&name.as_str()),
                "unexpected field {name:?} in the public fleet projection"
            );
        }
        let text = serde_json::to_string(&fleet).unwrap();
        for forbidden in [
            "hostname",
            "username",
            "ip_address",
            "mac_address",
            "path",
            "secret",
            "token",
            "cpu",
            "memory",
            "disk",
        ] {
            assert!(
                !text.contains(forbidden),
                "public projection leaked {forbidden:?}"
            );
        }
    }

    fn collect_field_names(value: &Value, names: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (name, child) in map {
                    names.push(name.clone());
                    collect_field_names(child, names);
                }
            }
            Value::Array(items) => {
                for item in items {
                    collect_field_names(item, names);
                }
            }
            _ => {}
        }
    }

    /// The Signal feed is one snapshot, not a sequence of reads.
    ///
    /// A projection assembled from separate reads can contradict itself while
    /// ingest is running: the cursor and the `stored`/`held` counters are
    /// snapshotted, a Signal commits, and the later read returns a Signal the
    /// counters never counted. That is what `gap` — the field an operator
    /// reads to decide whether a fleet's Signal delivery has stalled — is
    /// derived from, so the contradiction is an operational defect and not
    /// only a test-visible one.
    ///
    /// The race is real time, so this drives ingest concurrently rather than
    /// pretending to schedule it. Against the split reads it replaced, every
    /// single read observed the contradiction; against one transaction the
    /// invariants below cannot be violated at all, so the test never fails
    /// for timing reasons.
    #[test]
    fn the_signal_feed_never_reports_a_signal_its_own_cursor_has_not_counted() {
        let fixture = Arc::new(fixture());
        let performer = fixture.performer.clone();
        let local = fixture.local.clone();
        let writer = {
            let fixture = Arc::clone(&fixture);
            std::thread::spawn(move || {
                // Ten seconds apart keeps the frozen per-minute Signal rate
                // limit satisfied while the reader hammers the feed.
                for sequence in 1_u64..=SIGNALS_UNDER_CONCURRENT_READ {
                    let at = BASE_NOW + (sequence as i64) * 10;
                    fixture.clock.set(at);
                    let outcome = fixture.ingest(
                        &performer,
                        "health_signal",
                        at,
                        &signal_payload(&local, 1_000 + sequence, sequence, 5_000 + sequence, at),
                    );
                    assert!(
                        matches!(outcome.decision, HealthDecision::Accepted { .. }),
                        "signal {sequence} was not accepted: {:?}",
                        outcome.decision
                    );
                }
            })
        };

        let mut reads = 0_u64;
        while !writer.is_finished() {
            let feed = fixture.plane().signal_feed(64).unwrap();
            reads += 1;
            for signal in &feed.signals {
                let cursor = feed
                    .nodes
                    .iter()
                    .find(|node| node.node_id == signal.source)
                    .unwrap_or_else(|| {
                        panic!(
                            "the feed carried a Signal from {} with no cursor",
                            signal.source
                        )
                    });
                assert!(
                    signal.signal.sequence <= cursor.cursor,
                    "sequence {} is beyond the cursor {} the same feed reports",
                    signal.signal.sequence,
                    cursor.cursor
                );
            }
            for node in &feed.nodes {
                let carried = feed
                    .signals
                    .iter()
                    .filter(|signal| signal.source == node.node_id)
                    .count();
                assert!(
                    carried as u64 <= node.stored,
                    "the feed carried {carried} Signals from {} beside stored={}",
                    node.node_id,
                    node.stored
                );
            }
        }
        writer.join().unwrap();
        assert!(reads > 0, "the reader never observed the feed");

        // The settled feed still renders every Signal the cursor accepted.
        let feed = fixture.plane().signal_feed(64).unwrap();
        let cursor = feed
            .nodes
            .iter()
            .find(|node| node.node_id == fixture.performer)
            .expect("performer cursor");
        assert_eq!(cursor.cursor, SIGNALS_UNDER_CONCURRENT_READ);
        assert_eq!(cursor.stored, SIGNALS_UNDER_CONCURRENT_READ);
        assert_eq!(cursor.held, 0);
        assert_eq!(
            feed.signals
                .iter()
                .filter(|signal| signal.source == fixture.performer)
                .count() as u64,
            SIGNALS_UNDER_CONCURRENT_READ
        );
    }
}

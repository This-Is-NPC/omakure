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

pub use crate::domain::health_plane::{bounds, model};
pub mod lifecycle;
pub mod report;
pub mod schema;

use crate::domain::is_node_id;
use crate::node_registry::health::{
    HealthApplyRequest, HealthAuditEvent, HealthAuthorization, HealthFleetPeer, HealthOutboxEntry,
    HealthPruneReport,
};
use crate::node_registry::{NodeRegistry, PeerState, RegistryError};
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
                );
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
                    .map(|authorization| authorization.state.as_str().to_string())
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
            local: lifecycle::project(
                feed.lifecycle
                    .iter()
                    .map(|event| event.lifecycle_transition()),
                observed_at,
                limit,
            ),
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
        Ok(lifecycle::project(
            events.iter().map(|event| event.lifecycle_transition()),
            now,
            limit,
        ))
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
                    && authorization.role.code() == kind.required_role()
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
        self.registry
            .record_health_audit(crate::node_registry::health::HealthAuditRecord {
                event_code: kind_name,
                node_id: sender,
                message_kind: kind_name,
                byte_count,
                outcome,
                error_code: code.map(HealthCode::code),
                now,
            })
    }
}

/// Render one fleet-status row from the snapshot it was read in.
fn project(peer: HealthFleetPeer, now: i64) -> FleetNode {
    let snapshot = peer.snapshot;
    let (trust_state, capabilities) = match peer.authorization {
        Some(authorization) => (
            authorization.state.as_str().to_string(),
            authorization.capabilities,
        ),
        None => ("unknown".to_string(), Vec::new()),
    };
    FleetNode {
        node_id: snapshot.state.node_id.clone(),
        role: snapshot.state.role.as_str().to_string(),
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

#[cfg(test)]
mod tests;

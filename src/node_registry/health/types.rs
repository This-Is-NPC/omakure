use super::super::{AuditEvent, PeerRole, PeerState};
use crate::health_plane::model::{HealthPayload, ProfileSnapshot, PulseSnapshot, SignalRecord};

/// The single read-only projection over `trusted_peers` the Health Plane needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthAuthorization {
    pub node_id: String,
    pub state: PeerState,
    pub role: PeerRole,
    pub capabilities: Vec<String>,
}

/// The durable per-peer Health Plane state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthPeerState {
    pub node_id: String,
    pub role: PeerRole,
    pub cursor: u64,
    pub last_profile_revision: u64,
    pub last_pulse_sequence: u64,
    pub last_pulse_at: Option<i64>,
    pub stored_signals: u64,
    pub held_signals: u64,
    pub version_incompatible: bool,
    pub first_seen: i64,
    pub updated_at: i64,
}

/// One redacted Health Plane audit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthAuditEvent {
    pub id: i64,
    pub event_code: String,
    pub node_id: String,
    pub message_kind: String,
    pub byte_count: i64,
    pub outcome: String,
    pub error_code: Option<u16>,
    pub occurred_at: i64,
}

/// One pending Signal in the bounded Performer outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthOutboxEntry {
    pub signal_id: String,
    pub target_node_id: String,
    pub sequence: u64,
    pub signal: SignalRecord,
    pub attempts: i64,
    pub enqueued_at: i64,
    pub expires_at: i64,
}

/// What one pruning pass removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HealthPruneReport {
    pub expired_signals: u64,
    pub evicted_signals: u64,
    pub expired_held_signals: u64,
    pub expired_replay_keys: u64,
    pub evicted_replay_keys: u64,
    pub pruned_audit_rows: u64,
    pub expired_outbox_signals: u64,
    pub cleared_version_incompatible: u64,
}

/// Everything the Health Plane currently stores about one peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthPeerSnapshot {
    pub state: HealthPeerState,
    pub profile: Option<ProfileSnapshot>,
    pub pulse: Option<PulseSnapshot>,
}

/// Everything one fleet-status row is projected from, read together.
///
/// The authorization travels beside the stored state because the projection
/// reports both as one row: a peer whose trust ends between two reads would
/// otherwise be rendered from a stored snapshot that the trust decision no
/// longer matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthFleetPeer {
    pub snapshot: HealthPeerSnapshot,
    pub authorization: Option<HealthAuthorization>,
}

/// One peer's Signal cursor state, as the feed reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthFeedPeer {
    pub state: HealthPeerState,
    pub authorization: Option<HealthAuthorization>,
}

/// One Signal in the bounded feed page, tagged with the peer that reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthFeedSignal {
    pub node_id: String,
    pub signal: SignalRecord,
}

/// The whole Signal read surface, captured in exactly one transaction.
///
/// The cursors and the Signals they describe are read together on purpose.
/// Assembled from separate reads, the projection can contradict itself: the
/// counters are snapshotted, ingest commits, and the later read returns a
/// Signal the counters have not counted. That is not only a test-visible
/// oddity — `gap`, the field an operator reads to decide whether a fleet's
/// Signal delivery has stalled, is derived from those same counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthSignalFeed {
    /// Per-peer cursor state, ordered by node ID.
    pub peers: Vec<HealthFeedPeer>,
    /// The bounded fleet-wide page, newest first.
    pub signals: Vec<HealthFeedSignal>,
    /// The append-only trust transitions the local lifecycle Signals project
    /// from, read in the same transaction as everything they are merged with.
    pub lifecycle: Vec<AuditEvent>,
}

/// A validated message ready to be applied under the frozen receive order.
#[derive(Debug, Clone)]
pub(crate) struct HealthApplyRequest<'a> {
    pub sender: &'a str,
    pub payload: &'a HealthPayload,
    pub created_at: i64,
    pub now: i64,
    pub message_bytes: i64,
}

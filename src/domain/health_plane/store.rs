//! Protocol-neutral Health Plane storage contract and read views.

use super::model::{
    HealthDecision, HealthPayload, ProfileSnapshot, PulseSnapshot, SignalEnqueueRequest,
    SignalRecord,
};

/// Authorization facts needed by the mixed-version receive path.
pub trait HealthAuthorizationView {
    fn is_active_for_role(&self, role: i64) -> bool;
}

/// One peer's stored state and trust facts, captured together.
pub struct HealthFleetView {
    pub node_id: String,
    pub role: String,
    pub capabilities: Vec<String>,
    pub trust_state: String,
    pub last_pulse_at: Option<i64>,
    pub profile: Option<ProfileSnapshot>,
    pub pulse: Option<PulseSnapshot>,
    pub cursor: u64,
    pub stored_signals: u64,
    pub held_signals: u64,
    pub version_incompatible: bool,
}

/// One peer's Signal cursor and trust facts from a consistent feed read.
pub struct HealthCursorView {
    pub node_id: String,
    pub trust_state: String,
    pub cursor: u64,
    pub stored: u64,
    pub held: u64,
}

/// A trust transition without audit actor or reason.
pub struct HealthTransitionView {
    pub id: i64,
    pub node_id: String,
    pub from_state: Option<super::model::LifecycleState>,
    pub to_state: Option<super::model::LifecycleState>,
    pub occurred_at: Option<i64>,
}

impl HealthTransitionView {
    pub fn as_transition(&self) -> super::model::LifecycleTransition<'_> {
        super::model::LifecycleTransition {
            id: self.id,
            node_id: &self.node_id,
            from_state: self.from_state,
            to_state: self.to_state,
            occurred_at: self.occurred_at,
        }
    }
}

/// A consistent, bounded Signal feed read.
pub struct HealthSignalView {
    pub nodes: Vec<HealthCursorView>,
    pub signals: Vec<(String, SignalRecord)>,
    pub lifecycle: Vec<HealthTransitionView>,
}

/// Redacted metadata recorded for an early receive rejection.
pub struct HealthAuditInput<'a> {
    pub event_code: &'a str,
    pub node_id: &'a str,
    pub message_kind: &'a str,
    pub byte_count: i64,
    pub outcome: &'a str,
    pub error_code: Option<u16>,
    pub now: i64,
}

/// Persistence required by the Health Plane application facade.
/// Each method preserves the store's own transaction and error semantics.
pub trait HealthStore {
    type Error;
    type Authorization: HealthAuthorizationView;
    type OutboxEntry;
    type PruneReport;
    type AuditEvent;

    fn local_node_id(&self) -> &str;
    fn apply_message(
        &self,
        sender: &str,
        payload: &HealthPayload,
        created_at: i64,
        now: i64,
        message_bytes: i64,
    ) -> Result<HealthDecision, Self::Error>;
    fn fleet_snapshot(&self, now: i64) -> Result<Vec<HealthFleetView>, Self::Error>;
    fn node_snapshot(
        &self,
        node_id: &str,
        now: i64,
    ) -> Result<Option<HealthFleetView>, Self::Error>;
    fn signal_feed(&self, limit: usize, now: i64) -> Result<HealthSignalView, Self::Error>;
    fn signals(
        &self,
        node_id: &str,
        limit: usize,
        now: i64,
    ) -> Result<Vec<SignalRecord>, Self::Error>;
    fn authorization(&self, node_id: &str) -> Result<Option<Self::Authorization>, Self::Error>;
    fn enqueue_signal(
        &self,
        request: SignalEnqueueRequest<'_>,
        now: i64,
    ) -> Result<Self::OutboxEntry, Self::Error>;
    fn outbox(&self, limit: usize) -> Result<Vec<Self::OutboxEntry>, Self::Error>;
    fn mark_signal_sent(
        &self,
        signal_id: &str,
        message_id: &str,
        now: i64,
    ) -> Result<bool, Self::Error>;
    fn reset_outbox_attempts(&self, target_node_id: &str, now: i64) -> Result<u64, Self::Error>;
    fn signals_dropped(&self) -> Result<i64, Self::Error>;
    fn prune(&self, now: i64) -> Result<Self::PruneReport, Self::Error>;
    fn purge_revoked(&self, now: i64) -> Result<Vec<String>, Self::Error>;
    fn storage_bytes(&self) -> Result<i64, Self::Error>;
    fn audit_events(&self, limit: usize) -> Result<Vec<Self::AuditEvent>, Self::Error>;
    fn mark_version_incompatible(&self, node_id: &str, now: i64) -> Result<(), Self::Error>;
    fn record_audit(&self, input: HealthAuditInput<'_>) -> Result<(), Self::Error>;
}

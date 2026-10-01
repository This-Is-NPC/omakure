//! Node registry implementation of the Health Plane storage boundary.

use super::types::{HealthApplyRequest, HealthSignalFeed};
use super::{
    HealthAuditEvent, HealthAuditRecord, HealthAuthorization, HealthFleetPeer, HealthOutboxEntry,
    HealthPruneReport,
};
use crate::domain::health_plane::model::{
    HealthDecision, HealthPayload, SignalEnqueueRequest, SignalRecord,
};
use crate::domain::health_plane::store::{
    HealthAuditInput, HealthAuthorizationView, HealthCursorView, HealthFleetView, HealthSignalView,
    HealthStore, HealthTransitionView,
};
use crate::node_registry::{AuditEvent, NodeRegistry, PeerState, RegistryError};

impl HealthAuthorizationView for HealthAuthorization {
    fn is_active_for_role(&self, role: i64) -> bool {
        self.state == PeerState::Active && self.role.code() == role
    }
}

fn fleet_view(peer: HealthFleetPeer) -> HealthFleetView {
    let state = peer.snapshot.state;
    let (trust_state, capabilities) = match peer.authorization {
        Some(authorization) => (
            authorization.state.as_str().to_owned(),
            authorization.capabilities,
        ),
        None => ("unknown".to_owned(), Vec::new()),
    };
    HealthFleetView {
        node_id: state.node_id,
        role: state.role.as_str().to_owned(),
        capabilities,
        trust_state,
        last_pulse_at: state.last_pulse_at,
        profile: peer.snapshot.profile,
        pulse: peer.snapshot.pulse,
        cursor: state.cursor,
        stored_signals: state.stored_signals,
        held_signals: state.held_signals,
        version_incompatible: state.version_incompatible,
    }
}

fn transition_view(event: AuditEvent) -> HealthTransitionView {
    let transition = event.lifecycle_transition();
    HealthTransitionView {
        id: transition.id,
        node_id: transition.node_id.to_owned(),
        from_state: transition.from_state,
        to_state: transition.to_state,
        occurred_at: transition.occurred_at,
    }
}

fn signal_view(feed: HealthSignalFeed) -> HealthSignalView {
    HealthSignalView {
        nodes: feed
            .peers
            .into_iter()
            .map(|peer| HealthCursorView {
                node_id: peer.state.node_id,
                trust_state: peer
                    .authorization
                    .map(|authorization| authorization.state.as_str().to_owned())
                    .unwrap_or_else(|| "unknown".to_owned()),
                cursor: peer.state.cursor,
                stored: peer.state.stored_signals,
                held: peer.state.held_signals,
            })
            .collect(),
        signals: feed
            .signals
            .into_iter()
            .map(|entry| (entry.node_id, entry.signal))
            .collect(),
        lifecycle: feed.lifecycle.into_iter().map(transition_view).collect(),
    }
}

impl HealthStore for NodeRegistry {
    type Error = RegistryError;
    type Authorization = HealthAuthorization;
    type OutboxEntry = HealthOutboxEntry;
    type PruneReport = HealthPruneReport;
    type AuditEvent = HealthAuditEvent;

    fn local_node_id(&self) -> &str {
        NodeRegistry::local_node_id(self)
    }
    fn apply_message(
        &self,
        sender: &str,
        payload: &HealthPayload,
        created_at: i64,
        now: i64,
        message_bytes: i64,
    ) -> Result<HealthDecision, Self::Error> {
        self.apply_health_message(HealthApplyRequest {
            sender,
            payload,
            created_at,
            now,
            message_bytes,
        })
    }
    fn fleet_snapshot(&self, now: i64) -> Result<Vec<HealthFleetView>, Self::Error> {
        self.health_fleet_snapshot(now)
            .map(|peers| peers.into_iter().map(fleet_view).collect())
    }
    fn node_snapshot(
        &self,
        node_id: &str,
        now: i64,
    ) -> Result<Option<HealthFleetView>, Self::Error> {
        self.health_node_snapshot(node_id, now)
            .map(|peer| peer.map(fleet_view))
    }
    fn signal_feed(&self, limit: usize, now: i64) -> Result<HealthSignalView, Self::Error> {
        self.health_signal_feed(limit, now).map(signal_view)
    }
    fn signals(
        &self,
        node_id: &str,
        limit: usize,
        now: i64,
    ) -> Result<Vec<SignalRecord>, Self::Error> {
        self.health_signals(node_id, limit, now)
    }
    fn authorization(&self, node_id: &str) -> Result<Option<Self::Authorization>, Self::Error> {
        self.health_authorization(node_id)
    }
    fn enqueue_signal(
        &self,
        request: SignalEnqueueRequest<'_>,
        now: i64,
    ) -> Result<Self::OutboxEntry, Self::Error> {
        self.health_enqueue_signal(request, now)
    }
    fn outbox(&self, limit: usize) -> Result<Vec<Self::OutboxEntry>, Self::Error> {
        self.health_outbox(limit)
    }
    fn mark_signal_sent(
        &self,
        signal_id: &str,
        message_id: &str,
        now: i64,
    ) -> Result<bool, Self::Error> {
        self.health_mark_signal_sent(signal_id, message_id, now)
    }
    fn reset_outbox_attempts(&self, target_node_id: &str, now: i64) -> Result<u64, Self::Error> {
        self.health_reset_outbox_attempts(target_node_id, now)
    }
    fn signals_dropped(&self) -> Result<i64, Self::Error> {
        self.health_signals_dropped()
    }
    fn prune(&self, now: i64) -> Result<Self::PruneReport, Self::Error> {
        self.health_prune(now)
    }
    fn purge_revoked(&self, now: i64) -> Result<Vec<String>, Self::Error> {
        self.health_purge_revoked(now)
    }
    fn storage_bytes(&self) -> Result<i64, Self::Error> {
        self.health_storage_bytes()
    }
    fn audit_events(&self, limit: usize) -> Result<Vec<Self::AuditEvent>, Self::Error> {
        self.health_audit_events(limit)
    }
    fn mark_version_incompatible(&self, node_id: &str, now: i64) -> Result<(), Self::Error> {
        self.mark_health_version_incompatible(node_id, now)
    }
    fn record_audit(&self, input: HealthAuditInput<'_>) -> Result<(), Self::Error> {
        self.record_health_audit(HealthAuditRecord {
            event_code: input.event_code,
            node_id: input.node_id,
            message_kind: input.message_kind,
            byte_count: input.byte_count,
            outcome: input.outcome,
            error_code: input.error_code,
            now: input.now,
        })
    }
}

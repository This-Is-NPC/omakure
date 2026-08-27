//! Health Plane carriage over an established direct session.
//!
//! This module is the seam the frozen contract authorizes between the shipped
//! Noise transport and the Wave 2 Health Plane operations. It owns exactly
//! three things:
//!
//! * turning one already-decrypted application envelope into a decision by
//!   calling [`HealthPlane::ingest`], and nothing else;
//! * signing the frozen `health_ack` / `health_error` reply the shared
//!   operations chose;
//! * the Performer-side emission schedule for Profile, Pulse, and the bounded
//!   `run-completed` Signal outbox.
//!
//! It deliberately does **not** own authorization, presence, ordering,
//! idempotency, capacity, Signal storage, or any Health Plane table. Wave 2 is
//! the single fail-closed owner of all of those, and every inbound message
//! reaches it through `HealthPlane::ingest` with no shortcut, no cache, and no
//! second opinion. The only registry call made here is the redacted audit row
//! for a transport-layer failure, which happens before a message can reach
//! ingest at all.

use crate::direct_transport::{
    envelope_kind_hint, envelope_nonce, envelope_view, sign_health_envelope, verify_envelope,
    TransportError, HEALTH_KIND_PREFIX,
};
use crate::health_plane::bounds::{
    ACK_TIMEOUT_SECONDS, CAPABILITY_SIGNAL, MAX_RETRIES, MAX_SIGNALS_PER_PEER_PER_MINUTE,
    MIN_PULSE_INTERVAL_SECONDS, RATE_MINUTE_WINDOW_SECONDS, RETRY_BACKOFF_SECONDS,
    SIGNAL_OUTBOX_CAPACITY, VERSION_INCOMPATIBLE_BACKOFF_SECONDS,
};
use crate::health_plane::model::{HealthCode, HealthKind, SignalKind, SignalRecord};
use crate::health_plane::report::{
    ack_payload, error_payload, run_signal_id, signal_encoded_bytes, signal_payload, HealthReporter,
};
use crate::health_plane::{
    HealthClock, HealthPlane, HealthReply, InboundHealthMessage, SystemHealthClock,
};
use crate::node_identity::NodeIdentity;
use crate::node_registry::health::HealthOutboxEntry;
use crate::node_registry::{NodeRegistry, PeerRole, PeerState};
use rand::rngs::OsRng;
use rand::RngCore;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// How often the loop re-reads authorization and re-checks the schedule.
///
/// One second is well below every frozen cadence, so a revocation, a role
/// change, or a capability removal takes effect on the next tick rather than
/// at the next Pulse.
pub const TICK: Duration = Duration::from_secs(1);

/// The role this node plays for one peer, read from the local registry only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalRole {
    /// The peer is an active trusted Performer: ingest its health, never emit.
    Conductor,
    /// The peer is our active trusted Conductor: emit health, never ingest it.
    Performer,
    /// The peer is not actively trusted, or trust has not been established.
    None,
}

/// One outstanding Profile, Pulse, or Signal awaiting its acknowledgement.
struct Pending {
    kind: HealthKind,
    message_id: String,
    /// The durable outbox key when this attempt carried a Signal.
    ///
    /// A Signal is retried from the outbox rather than rebuilt from live
    /// facts, because the frozen contract requires a resend to reuse the
    /// original `signal_id` and `sequence` while using a fresh `message_id`.
    signal_id: Option<String>,
    /// The UTC Unix second the attempt left this node.
    sent_at: i64,
    attempts: i64,
}

/// A clock shared between the emission schedule and the shared operations.
///
/// The schedule is expressed in UTC Unix seconds rather than in monotonic
/// instants because the frozen contract already ties `pulse.sequence` and
/// `emitted_at` to the wall clock. One injected clock therefore drives the
/// cadence, the acknowledgement timeout, the retry backoff, the version
/// backoff, and every timestamp on the wire, which is what makes the schedule
/// deterministically testable.
struct SharedClock(Arc<dyn HealthClock>);

impl HealthClock for SharedClock {
    fn unix_seconds(&self) -> i64 {
        self.0.unix_seconds()
    }

    fn monotonic_millis(&self) -> u64 {
        self.0.monotonic_millis()
    }
}

/// The Health Plane state attached to one established direct session.
pub struct HealthSession<'a> {
    identity: &'a NodeIdentity,
    registry: &'a NodeRegistry,
    remote_node_id: String,
    remote_identity_key: [u8; 32],
    session_id: [u8; 32],
    reporter: Option<Arc<HealthReporter>>,
    clock: Arc<dyn HealthClock>,
    pending: Option<Pending>,
    /// When the next Profile should be built, if one is due.
    profile_due: bool,
    /// The UTC Unix second at which the next Pulse is due.
    next_pulse: Option<i64>,
    /// The UTC Unix second the last Pulse actually left this node.
    last_pulse_sent: Option<i64>,
    /// Set when the Conductor answered `health_unsupported_version` (1101).
    suppressed_until: Option<i64>,
    /// Start of the current one-minute Signal send window.
    signal_window_start: Option<i64>,
    /// Signals sent inside the current window, held under the frozen
    /// per-peer-per-minute Signal bound so this node can never manufacture a
    /// `health_rate_limited` rejection against itself.
    signals_in_window: i64,
    /// Whether this session has already re-armed the outbox delivery budget.
    ///
    /// The frozen retry bound is three attempts per message *per session*, and
    /// a Signal that spent them is resent on the next session. Re-arming
    /// exactly once, before this session has sent anything, is what makes both
    /// halves of that rule true: the budget is fresh for a new session and can
    /// never be refreshed mid-session into an unbounded retry loop.
    outbox_rearmed: bool,
}

/// What the caller must do with the result of one inbound envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthOutcome {
    /// The envelope was not a Health Plane message; preserve existing behavior.
    NotHealth,
    /// The message was handled and no reply is permitted.
    Handled,
    /// The message was handled; write this signed envelope back.
    Reply(Vec<u8>),
}

impl<'a> HealthSession<'a> {
    /// Attach Health Plane carriage to one established session.
    pub fn new(
        identity: &'a NodeIdentity,
        registry: &'a NodeRegistry,
        remote_node_id: &str,
        remote_identity_key: &[u8; 32],
        session_id: [u8; 32],
        reporter: Option<Arc<HealthReporter>>,
    ) -> Self {
        Self::with_clock(
            identity,
            registry,
            remote_node_id,
            remote_identity_key,
            session_id,
            reporter,
            Arc::new(SystemHealthClock::new()),
        )
    }

    /// Attach Health Plane carriage over an injected clock.
    #[allow(clippy::too_many_arguments)]
    pub fn with_clock(
        identity: &'a NodeIdentity,
        registry: &'a NodeRegistry,
        remote_node_id: &str,
        remote_identity_key: &[u8; 32],
        session_id: [u8; 32],
        reporter: Option<Arc<HealthReporter>>,
        clock: Arc<dyn HealthClock>,
    ) -> Self {
        Self {
            identity,
            registry,
            remote_node_id: remote_node_id.to_string(),
            remote_identity_key: *remote_identity_key,
            session_id,
            reporter,
            clock,
            pending: None,
            profile_due: true,
            next_pulse: None,
            last_pulse_sent: None,
            suppressed_until: None,
            signal_window_start: None,
            signals_in_window: 0,
            outbox_rearmed: false,
        }
    }

    /// The Wave 2 shared operations over this session's clock.
    fn plane(&self) -> HealthPlane<'_> {
        HealthPlane::with_clock(
            self.registry,
            Box::new(SharedClock(Arc::clone(&self.clock))),
        )
    }

    /// Handle one decrypted application envelope.
    ///
    /// Returns [`HealthOutcome::NotHealth`] for anything that is not a Health
    /// Plane message, so the caller's existing steady-state behavior for other
    /// application traffic is untouched.
    pub fn handle_envelope(&mut self, encoded: &[u8]) -> HealthOutcome {
        let Some(kind_text) = envelope_kind_hint(encoded) else {
            return HealthOutcome::NotHealth;
        };
        if !kind_text.starts_with(HEALTH_KIND_PREFIX) {
            return HealthOutcome::NotHealth;
        }
        let kind_text = kind_text.to_string();
        let canonical_len = encoded.len().saturating_sub(64);
        let plane = self.plane();
        let now = plane.now();

        // Step 1: the frozen transport verification path. A failure here is
        // dropped and audited without a reply, because the sender is not yet
        // proven authorized and target-bound.
        if let Err(error) = self.verify(encoded, &kind_text) {
            self.audit_transport_failure(&kind_text, encoded.len(), error, now);
            return HealthOutcome::Handled;
        }
        let Ok(view) = envelope_view(encoded) else {
            self.audit_transport_failure(
                &kind_text,
                encoded.len(),
                TransportError::InvalidFrame,
                now,
            );
            return HealthOutcome::Handled;
        };

        // Steps 2 through 15 belong to the Wave 2 shared operations, in full.
        let ingest = plane.ingest(InboundHealthMessage {
            sender: &self.remote_node_id,
            kind: &kind_text,
            created_at: view.created_at,
            canonical_len,
            payload: &view.payload,
        });
        let Ok(ingest) = ingest else {
            return HealthOutcome::Handled;
        };

        // A reply that acknowledges our own Profile or Pulse resolves the
        // pending send and, for 1101, opens the frozen version backoff.
        if matches!(ingest.kind, Some(HealthKind::Ack) | Some(HealthKind::Error)) {
            self.absorb_reply(&view.payload, ingest.kind, ingest.accepted());
        }

        match ingest.reply {
            HealthReply::None => HealthOutcome::Handled,
            HealthReply::Ack {
                acked_message_id,
                cursor,
            } => self.sign_reply(
                HealthKind::Ack,
                ack_payload(&self.remote_node_id, &fresh_id(), &acked_message_id, cursor),
                now,
            ),
            HealthReply::Error {
                acked_message_id,
                code,
            } => self.sign_reply(
                HealthKind::Error,
                error_payload(&self.remote_node_id, &fresh_id(), &acked_message_id, code),
                now,
            ),
        }
    }

    /// The Performer-side schedule. Returns the next envelope to send, if any.
    ///
    /// Emission happens only when the local registry currently records this
    /// peer as an active trusted Conductor, so revocation, a role change, or a
    /// capability removal stops the reporting stream on the next tick without
    /// any peer message being involved.
    pub fn tick(&mut self) -> Option<Vec<u8>> {
        let reporter = self.reporter.clone()?;
        let authorization = self.authorization();
        let now = self.clock.unix_seconds();
        if authorization.0 != LocalRole::Performer {
            // Not (or no longer) reporting to this peer. Any pending send is
            // abandoned rather than retried against an unauthorized peer.
            self.pending = None;
            return None;
        }
        if self.suppressed_until.is_some_and(|until| now < until) {
            return None;
        }
        // A newly established session to this Conductor re-arms the delivery
        // budget of everything still queued for it, which is the frozen
        // "resent on the next session" rule. It happens once, before this
        // session has sent anything, so the frozen three-attempt bound still
        // holds for every message inside the session.
        if !self.outbox_rearmed {
            self.outbox_rearmed = true;
            let _ = self.plane().reset_outbox_attempts(&self.remote_node_id);
        }
        // Terminal runs become durable outbox entries before anything is sent,
        // so a `run-completed` Signal survives this session, this connection,
        // and this process. Nothing here starts, schedules, or cancels work:
        // the run log is read only after a run already reached a terminal
        // result.
        self.harvest_run_signals(&reporter, &authorization.1);
        if let Some(retry) = self.retry_due(now) {
            return retry;
        }
        if self.pending.is_some() {
            return None;
        }
        if !self.profile_due && reporter.profile_changed(&authorization.1) {
            self.profile_due = true;
        }
        if self.profile_due {
            let message =
                reporter.profile(&self.remote_node_id, &fresh_id(), &authorization.1, now);
            self.profile_due = false;
            return self.send(HealthKind::Profile, message.payload, now);
        }
        // Pulse keeps priority over the Signal feed, because presence is what
        // an operator loses first and the frozen 10-per-minute Signal bound
        // already leaves most of the 30-second Pulse window free for Signals.
        if self.next_pulse.is_none_or(|due| now >= due) {
            if let Some(message) = reporter.pulse(&self.remote_node_id, &fresh_id(), now) {
                self.next_pulse =
                    Some(now.saturating_add(HealthReporter::pulse_interval_seconds()));
                self.last_pulse_sent = Some(now);
                return self.send(HealthKind::Pulse, message.payload, now);
            }
        }
        self.send_next_signal(&authorization.1, now)
    }

    /// Turn newly terminal runs into bounded, durable outbox Signals.
    ///
    /// Enqueueing goes through the Wave 2 shared operations, which own the
    /// 64-entry capacity, the drop-oldest overflow rule, the local sequence,
    /// the 7-day expiry, and the `signals_dropped` counter. Nothing here
    /// writes a Health Plane row.
    fn harvest_run_signals(&self, reporter: &HealthReporter, granted: &[String]) {
        if !granted.iter().any(|entry| entry == CAPABILITY_SIGNAL) {
            // The Conductor has not granted `notifications`. A Performer that
            // reports Profile and Pulse but refuses Signals is an enforceable
            // posture the frozen contract names, so nothing is queued at all.
            return;
        }
        let plane = self.plane();
        for run in reporter.run_signals() {
            let signal_id = run_signal_id(&run.run_id);
            let record = SignalRecord {
                kind: SignalKind::RunCompleted,
                occurred_at: run.finished_at,
                run: Some(run.clone()),
                sequence: 1,
                signal_id: signal_id.clone(),
                subject: None,
            };
            let message_bytes = signal_encoded_bytes(&self.remote_node_id, &record);
            // A duplicate or a full outbox is a bounded, already-audited
            // outcome inside the shared operations; it is never a reason to
            // retry a run or to widen a bound here.
            let _ = plane.enqueue_signal(
                &self.remote_node_id,
                &signal_id,
                SignalKind::RunCompleted,
                run.finished_at,
                None,
                Some(&run),
                message_bytes,
            );
        }
    }

    /// Send the oldest undelivered Signal, if the frozen budget allows it.
    fn send_next_signal(&mut self, granted: &[String], now: i64) -> Option<Vec<u8>> {
        if !granted.iter().any(|entry| entry == CAPABILITY_SIGNAL) {
            return None;
        }
        if !self.signal_budget_available(now) {
            return None;
        }
        let entry = self
            .plane()
            .outbox(1)
            .ok()?
            .into_iter()
            .next()
            .filter(|entry| entry.target_node_id == self.remote_node_id)?;
        self.send_signal(&entry, now)
    }

    /// Sign and record one attempt at delivering a durable outbox Signal.
    ///
    /// The resend rule is frozen: the original `signal_id` and `sequence` are
    /// reused so the Conductor can recognise the same logical Signal, while a
    /// fresh `message_id` and a fresh nonce keep it outside the replay window.
    fn send_signal(&mut self, entry: &HealthOutboxEntry, now: i64) -> Option<Vec<u8>> {
        // The durable attempt counter is the authority. The frozen bound is
        // three attempts per message per session; the outbox column enforces
        // the same ceiling, so exceeding it is refused here rather than at the
        // database. The counter is re-armed once when the next session is
        // established, never mid-session.
        if entry.attempts >= MAX_RETRIES {
            return None;
        }
        let message_id = fresh_id();
        let payload = signal_payload(&self.remote_node_id, &message_id, &entry.signal);
        let encoded = self.sign(HealthKind::Signal, payload, now)?;
        if !self
            .plane()
            .mark_signal_sent(&entry.signal_id, &message_id)
            .ok()?
        {
            return None;
        }
        self.consume_signal_budget(now);
        self.pending = Some(Pending {
            kind: HealthKind::Signal,
            message_id,
            signal_id: Some(entry.signal_id.clone()),
            sent_at: now,
            attempts: entry.attempts,
        });
        Some(encoded)
    }

    /// Whether the frozen per-peer-per-minute Signal bound still has room.
    fn signal_budget_available(&self, now: i64) -> bool {
        match self.signal_window_start {
            Some(start) if now.saturating_sub(start) < RATE_MINUTE_WINDOW_SECONDS => {
                self.signals_in_window < MAX_SIGNALS_PER_PEER_PER_MINUTE
            }
            _ => true,
        }
    }

    fn consume_signal_budget(&mut self, now: i64) {
        match self.signal_window_start {
            Some(start) if now.saturating_sub(start) < RATE_MINUTE_WINDOW_SECONDS => {
                self.signals_in_window = self.signals_in_window.saturating_add(1);
            }
            _ => {
                self.signal_window_start = Some(now);
                self.signals_in_window = 1;
            }
        }
    }

    /// Whether this session carries Health Plane traffic in either direction.
    pub fn engaged(&self) -> bool {
        self.reporter.is_some() || self.authorization().0 != LocalRole::None
    }

    /// The finite retry schedule for one unacknowledged Profile or Pulse.
    ///
    /// A retry is a freshly built message with a fresh `message_id`, a fresh
    /// nonce, and a fresh `created_at`, because the frozen replay and freshness
    /// rules reject a byte-identical resend. A Pulse retry is additionally held
    /// back to the frozen minimum accepted Pulse interval, so the retry
    /// schedule can never manufacture a `health_rate_limited` rejection.
    fn retry_due(&mut self, now: i64) -> Option<Option<Vec<u8>>> {
        let pending = self.pending.as_ref()?;
        let waited = now.saturating_sub(pending.sent_at);
        if waited < ACK_TIMEOUT_SECONDS {
            return Some(None);
        }
        let attempts = pending.attempts;
        let kind = pending.kind;
        if attempts >= MAX_RETRIES {
            // Final retry exhausted. For a Profile or a Pulse the frozen rule
            // is that the send is dropped and the next *scheduled* one
            // supersedes it. A Profile is scheduled by a material change or by
            // a new session, never by its own failure, so re-arming it here
            // would turn one unreachable Conductor into an unbounded Profile
            // loop that the frozen 12-per-hour bound forbids. A Signal is
            // instead retained in the durable outbox within its 64-entry and
            // 7-day bounds and resent on the next session; see `retry_signal`.
            self.pending = None;
            return Some(None);
        }
        let backoff = RETRY_BACKOFF_SECONDS
            .get(attempts as usize)
            .copied()
            .unwrap_or(*RETRY_BACKOFF_SECONDS.last().unwrap_or(&1));
        let mut wait = backoff;
        if kind == HealthKind::Pulse {
            let since = self
                .last_pulse_sent
                .map(|sent| now.saturating_sub(sent))
                .unwrap_or(MIN_PULSE_INTERVAL_SECONDS);
            wait = wait.max(MIN_PULSE_INTERVAL_SECONDS.saturating_sub(since));
        }
        if waited < ACK_TIMEOUT_SECONDS.saturating_add(wait) {
            return Some(None);
        }
        if kind == HealthKind::Signal {
            return Some(self.retry_signal(now));
        }
        let reporter = self.reporter.clone()?;
        let authorization = self.authorization();
        let payload = match kind {
            HealthKind::Profile => Some(
                reporter
                    .profile(&self.remote_node_id, &fresh_id(), &authorization.1, now)
                    .payload,
            ),
            HealthKind::Pulse => reporter
                .pulse(&self.remote_node_id, &fresh_id(), now)
                .map(|message| message.payload),
            _ => None,
        };
        let Some(payload) = payload else {
            return Some(None);
        };
        if kind == HealthKind::Pulse {
            self.last_pulse_sent = Some(now);
        }
        let encoded = self.send(kind, payload, now);
        if let Some(pending) = self.pending.as_mut() {
            pending.attempts = attempts + 1;
        }
        Some(encoded)
    }

    /// Retry one unacknowledged Signal from the durable outbox.
    ///
    /// The outbox is the single source of truth. When the entry is gone the
    /// Conductor already acknowledged it and the Wave 2 apply step removed it,
    /// so there is nothing to retry. When it is still there but has spent its
    /// frozen three attempts for *this* session, it stays in the outbox within
    /// its 64-entry and 7-day bounds and is resent on the next session, which
    /// re-arms it once on connect.
    fn retry_signal(&mut self, now: i64) -> Option<Vec<u8>> {
        let signal_id = self.pending.as_ref().and_then(|pending| {
            pending
                .signal_id
                .as_ref()
                .filter(|_| pending.kind == HealthKind::Signal)
                .cloned()
        })?;
        self.pending = None;
        if !self.signal_budget_available(now) {
            return None;
        }
        let entry = self
            .plane()
            .outbox(SIGNAL_OUTBOX_CAPACITY as usize)
            .ok()?
            .into_iter()
            .find(|entry| entry.signal_id == signal_id)?;
        self.send_signal(&entry, now)
    }

    fn send(&mut self, kind: HealthKind, payload: Value, now: i64) -> Option<Vec<u8>> {
        let message_id = payload
            .get("message_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let encoded = self.sign(kind, payload, now)?;
        self.pending = Some(Pending {
            kind,
            message_id,
            signal_id: None,
            sent_at: now,
            attempts: 0,
        });
        Some(encoded)
    }

    fn sign(&self, kind: HealthKind, payload: Value, now: i64) -> Option<Vec<u8>> {
        let mut nonce = [0u8; 16];
        OsRng.fill_bytes(&mut nonce);
        let created_at = u64::try_from(now).ok()?;
        sign_health_envelope(
            self.identity,
            kind.wire(),
            &self.session_id,
            nonce,
            payload,
            created_at,
        )
        .ok()
        .map(|envelope| envelope.encoded())
    }

    fn sign_reply(&self, kind: HealthKind, payload: Value, now: i64) -> HealthOutcome {
        match self.sign(kind, payload, now) {
            Some(encoded) => HealthOutcome::Reply(encoded),
            None => HealthOutcome::Handled,
        }
    }

    fn verify(&self, encoded: &[u8], kind: &str) -> Result<(), TransportError> {
        let nonce = envelope_nonce(encoded)?;
        verify_envelope(
            encoded,
            &self.remote_node_id,
            &self.remote_identity_key,
            kind,
            &self.session_id,
            &nonce,
        )
    }

    /// Resolve a pending send against an inbound `health_ack` or `health_error`.
    fn absorb_reply(&mut self, payload: &Value, kind: Option<HealthKind>, accepted: bool) {
        if !accepted {
            return;
        }
        let body = match kind {
            Some(HealthKind::Ack) => payload.get("ack"),
            Some(HealthKind::Error) => payload.get("error"),
            _ => None,
        };
        let Some(body) = body else {
            return;
        };
        let acked = body.get("acked_message_id").and_then(Value::as_str);
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| Some(pending.message_id.as_str()) == acked)
        {
            self.pending = None;
        }
        let code = body.get("code").and_then(Value::as_u64);
        if code == Some(u64::from(HealthCode::UnsupportedVersion.code())) {
            self.suppressed_until = Some(
                self.clock
                    .unix_seconds()
                    .saturating_add(VERSION_INCOMPATIBLE_BACKOFF_SECONDS),
            );
        }
    }

    /// The peer's current role and granted capabilities, from the local
    /// registry only. This is the shipped read-only projection; nothing here
    /// reads a field out of a peer message.
    fn authorization(&self) -> (LocalRole, Vec<String>) {
        let Ok(Some(authorization)) = self.registry.health_authorization(&self.remote_node_id)
        else {
            return (LocalRole::None, Vec::new());
        };
        if authorization.state != PeerState::Active {
            return (LocalRole::None, Vec::new());
        }
        let role = match authorization.role {
            PeerRole::Conductor => LocalRole::Performer,
            PeerRole::Performer => LocalRole::Conductor,
        };
        (role, authorization.capabilities)
    }

    /// Record the redacted audit row for a step-1 transport failure.
    ///
    /// The row carries only the stable code, the peer node ID, the message
    /// kind, and the byte count, exactly as the frozen contract requires.
    fn audit_transport_failure(
        &self,
        kind: &str,
        byte_count: usize,
        error: TransportError,
        now: i64,
    ) {
        let code = transport_failure_code(error);
        let kind = HealthKind::parse(kind)
            .map(HealthKind::wire)
            .unwrap_or("unknown");
        let _ = self.registry.record_health_audit(
            kind,
            &self.remote_node_id,
            kind,
            byte_count as i64,
            "dropped",
            Some(code.code()),
            now,
        );
    }
}

/// The frozen transport-layer failure mapping.
///
/// See `.docs/health-plane-contract.md`, "Transport-layer failure mapping".
fn transport_failure_code(error: TransportError) -> HealthCode {
    match error {
        TransportError::Replay => HealthCode::Replay,
        TransportError::MessageTooLarge => HealthCode::MessageTooLarge,
        _ => HealthCode::InvalidMessage,
    }
}

/// A fresh 16-byte CSPRNG identifier as 32 lowercase hex characters.
fn fresh_id() -> String {
    let mut bytes = [0u8; 16];
    OsRng.fill_bytes(&mut bytes);
    crate::health_plane::report::hex_lower(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(first
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
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
    use crate::node::{NodeContext, NodePathOverrides, NodePlatform};
    use crate::node_registry::{PeerRegistration, PeerSource};
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
    use std::sync::Mutex as StdMutex;
    use tempfile::TempDir;

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
        registry: NodeRegistry,
        conductor: String,
        conductor_key: [u8; 32],
        performer: String,
        clock: Arc<TestClock>,
        facts: Arc<MutableFacts>,
    }

    fn scalar(seed: u32) -> [u8; 32] {
        let mut value = [0_u8; 32];
        value[28..].copy_from_slice(&seed.saturating_add(1).to_be_bytes());
        value
    }

    fn peer_identity(seed: u32) -> (String, String, [u8; 32]) {
        let key = k256::schnorr::SigningKey::from_slice(&scalar(seed)).unwrap();
        let xonly: [u8; 32] = key
            .verifying_key()
            .to_bytes()
            .as_slice()
            .try_into()
            .unwrap();
        let public_key = xonly.iter().map(|byte| format!("{byte:02x}")).collect();
        (
            crate::node_identity::node_id_for_x_only_public_key(&xonly),
            public_key,
            xonly,
        )
    }

    fn fixture() -> Fixture {
        let temp = TempDir::new().unwrap();
        let context = NodeContext::resolve_for(
            NodePlatform::Linux,
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
            let (node_id, public_key, xonly) = peer_identity(seed);
            registry
                .import_manual_peer(PeerRegistration {
                    node_id: node_id.clone(),
                    public_key,
                    role,
                    capabilities: capabilities.iter().map(|entry| entry.to_string()).collect(),
                    source: PeerSource::Manual,
                    actor: "direct-health-tests".to_string(),
                    reason: "health plane carriage test peer".to_string(),
                })
                .unwrap();
            (node_id, xonly)
        };
        let (conductor, conductor_key) = trust(11, PeerRole::Conductor, &["inventory-health"]);
        let (performer, _) = trust(12, PeerRole::Performer, &["inventory-health"]);
        Fixture {
            _temp: temp,
            identity,
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
        let key: [u8; 32] = (0..32)
            .map(|index| {
                u8::from_str_radix(&local.public_key_hex[index * 2..index * 2 + 2], 16).unwrap()
            })
            .collect::<Vec<u8>>()
            .try_into()
            .unwrap();
        verify_envelope(encoded, &local.node_id, &key, &kind, &SESSION_ID, &nonce)
            .expect("emitted envelope must satisfy the frozen verifier");
        let view = envelope_view(encoded).expect("view");
        (kind, view.payload)
    }

    #[test]
    fn a_performer_sends_profile_first_then_pulse_at_the_frozen_cadence() {
        let fixture = fixture();
        let mut session = fixture.conductor_session();

        let profile = session.tick().expect("profile on connect");
        let (kind, payload) = decode(&fixture, &profile);
        assert_eq!(kind, "health_profile");
        assert_eq!(payload["profile"]["role"], "performer");
        assert_eq!(payload["target"], fixture.conductor);
        // Display-only echo of what this Conductor granted us locally.
        assert_eq!(
            payload["profile"]["capabilities"],
            serde_json::json!(["inventory-health"])
        );

        // Nothing more leaves the node until the Profile is acknowledged.
        assert!(session.tick().is_none());
        ack(&mut session, &payload);

        let pulse = session.tick().expect("pulse immediately after the profile");
        let (kind, payload) = decode(&fixture, &pulse);
        assert_eq!(kind, "health_pulse");
        assert_eq!(payload["pulse"]["sequence"], BASE_NOW);
        assert_eq!(payload["pulse"]["emitted_at"], BASE_NOW);
        ack(&mut session, &payload);

        // One second before the frozen interval: silence.
        fixture
            .clock
            .set(BASE_NOW + HealthReporter::pulse_interval_seconds() - 1);
        assert!(session.tick().is_none());

        // Exactly at the frozen interval: the next Pulse.
        fixture
            .clock
            .set(BASE_NOW + HealthReporter::pulse_interval_seconds());
        let pulse = session.tick().expect("pulse at the frozen cadence");
        let (_, payload) = decode(&fixture, &pulse);
        assert_eq!(
            payload["pulse"]["sequence"],
            BASE_NOW + HealthReporter::pulse_interval_seconds()
        );
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

    #[test]
    fn a_material_profile_change_re_emits_a_profile_with_a_higher_revision() {
        let fixture = fixture();
        let mut session = fixture.conductor_session();
        let (_, first) = decode(&fixture, &session.tick().expect("first profile"));
        ack(&mut session, &first);
        let revision = first["profile"]["profile_revision"].as_u64().unwrap();

        // No change: only Pulses flow, however many ticks pass.
        for step in 0..3 {
            fixture
                .clock
                .set(BASE_NOW + HealthReporter::pulse_interval_seconds() * step);
            if let Some(encoded) = session.tick() {
                let (kind, payload) = decode(&fixture, &encoded);
                assert_eq!(kind, "health_pulse", "unexpected {kind} without a change");
                ack(&mut session, &payload);
            }
        }

        *fixture.facts.display_name.lock().unwrap() = "workbench".to_string();
        fixture.clock.advance(1);
        let (kind, second) = decode(&fixture, &session.tick().expect("profile after change"));
        assert_eq!(kind, "health_profile");
        assert_eq!(second["profile"]["display_name"], "workbench");
        assert!(
            second["profile"]["profile_revision"].as_u64().unwrap() > revision,
            "a material change must strictly advance profile_revision"
        );
    }

    #[test]
    fn an_unacknowledged_profile_retries_finitely_then_is_superseded() {
        let fixture = fixture();
        let mut session = fixture.conductor_session();
        assert!(session.tick().is_some(), "first profile attempt");

        // Walk the frozen 5-second acknowledgement timeout plus the 1/2/4
        // second backoff one second at a time, never acknowledging, and stop
        // when the send is finally dropped.
        let mut retries = 0;
        let mut elapsed = 0;
        while session.pending.is_some() && elapsed < 120 {
            elapsed += 1;
            fixture.clock.set(BASE_NOW + elapsed);
            if session.tick().is_some() {
                retries += 1;
            }
        }
        assert_eq!(
            retries, MAX_RETRIES,
            "exactly the frozen retry count, then the send is dropped"
        );
        assert!(
            session.pending.is_none(),
            "the final retry must leave nothing queued"
        );
        // 5 s timeout, then 5+1, 5+2, 5+4: the frozen backoff, and finite.
        assert_eq!(elapsed, 6 + 7 + 9 + 5);

        // A dropped Profile must not re-arm itself: an unreachable Conductor
        // can never be turned into an unbounded Profile loop.
        fixture
            .clock
            .set(BASE_NOW + elapsed + HealthReporter::pulse_interval_seconds());
        let next = session.tick().expect("the schedule continues with a Pulse");
        let (kind, _) = decode(&fixture, &next);
        assert_eq!(kind, "health_pulse");
    }

    #[test]
    fn revoking_the_conductor_stops_emission_on_the_next_tick() {
        let fixture = fixture();
        let mut session = fixture.conductor_session();
        let (_, profile) = decode(&fixture, &session.tick().expect("first profile"));
        ack(&mut session, &profile);

        fixture
            .registry
            .revoke_peer(
                &fixture.conductor,
                "direct-health-tests",
                "revoked mid-session",
            )
            .expect("revoke the conductor");

        fixture
            .clock
            .advance(HealthReporter::pulse_interval_seconds());
        assert!(
            session.tick().is_none(),
            "a revoked Conductor must stop receiving health immediately"
        );
        assert!(
            session.authorization().0 == LocalRole::None,
            "a revoked peer must project no local role at all"
        );
    }

    #[test]
    fn a_performer_peer_never_receives_profile_or_pulse_from_this_node() {
        let fixture = fixture();
        let (_, _, performer_key) = peer_identity(12);
        let mut session = fixture.session(&fixture.performer, performer_key);
        for step in 0..4 {
            fixture
                .clock
                .set(BASE_NOW + HealthReporter::pulse_interval_seconds() * step);
            assert!(
                session.tick().is_none(),
                "this node is the Conductor for that peer and must never report to it"
            );
        }
    }

    #[test]
    fn an_unsupported_version_error_opens_the_frozen_backoff() {
        let fixture = fixture();
        let mut session = fixture.conductor_session();
        let (_, profile) = decode(&fixture, &session.tick().expect("first profile"));
        let acked = profile["message_id"].as_str().unwrap();
        session.absorb_reply(
            &serde_json::json!({
                "error": {
                    "accepted": false,
                    "acked_message_id": acked,
                    "code": HealthCode::UnsupportedVersion.code(),
                    "reason": HealthCode::UnsupportedVersion.name(),
                }
            }),
            Some(HealthKind::Error),
            true,
        );
        fixture
            .clock
            .advance(VERSION_INCOMPATIBLE_BACKOFF_SECONDS - 1);
        assert!(
            session.tick().is_none(),
            "the frozen 300-second version backoff must silence this node"
        );
        fixture.clock.advance(1);
        assert!(
            session.tick().is_some(),
            "the node retries once the frozen backoff expires"
        );
    }

    #[test]
    fn a_terminal_run_becomes_exactly_one_bounded_redacted_signal() {
        let fixture = fixture();
        grant_notifications(&fixture);
        let mut session = fixture.conductor_session();
        settle(&fixture, &mut session);

        // Nothing has finished yet, so the feed is silent.
        fixture.clock.advance(1);
        assert!(session.tick().is_none());

        fixture
            .facts
            .finish_run(&"a".repeat(32), "deploy", BASE_NOW + 1);
        fixture.clock.advance(1);
        let encoded = session.tick().expect("run-completed signal");
        let (kind, payload) = decode(&fixture, &encoded);
        assert_eq!(kind, "health_signal");
        assert_eq!(payload["target"], fixture.conductor);
        assert_eq!(payload["signal"]["kind"], "run-completed");
        assert_eq!(payload["signal"]["sequence"], 1);
        assert!(payload["signal"]["subject"].is_null());
        assert_eq!(payload["signal"]["occurred_at"], BASE_NOW + 1);
        assert_eq!(payload["signal"]["run"]["finished_at"], BASE_NOW + 1);
        assert_eq!(payload["signal"]["run"]["script"], "deploy");
        assert_eq!(payload["signal"]["run"]["state"], "completed");
        assert_eq!(payload["signal"]["run"].as_object().unwrap().len(), 5);
        assert!(
            encoded.len() <= HealthKind::Signal.max_encoded_bytes(),
            "signal envelope exceeded the frozen per-kind cap"
        );

        // The same terminal run never produces a second outbox entry.
        assert_eq!(session.plane().outbox(64).expect("outbox").len(), 1);
        fixture.clock.advance(1);
        assert!(session.tick().is_none(), "one in-flight message at a time");
        assert_eq!(session.plane().outbox(64).expect("outbox").len(), 1);
    }

    #[test]
    fn a_performer_without_the_notifications_capability_never_emits_a_signal() {
        let fixture = fixture();
        let mut session = fixture.conductor_session();
        settle(&fixture, &mut session);
        fixture
            .facts
            .finish_run(&"b".repeat(32), "deploy", BASE_NOW + 1);
        for _ in 0..5 {
            fixture.clock.advance(1);
            assert!(
                session.tick().is_none(),
                "a Signal must never leave a node whose Conductor granted no notifications"
            );
        }
        assert!(
            session.plane().outbox(64).expect("outbox").is_empty(),
            "nothing is even queued without the frozen capability"
        );
    }

    #[test]
    fn an_unacknowledged_signal_is_retried_from_the_outbox_and_then_bounded() {
        let fixture = fixture();
        grant_notifications(&fixture);
        let mut session = fixture.conductor_session();
        settle(&fixture, &mut session);
        fixture
            .facts
            .finish_run(&"c".repeat(32), "deploy", BASE_NOW + 1);
        fixture.clock.advance(1);
        let (_, first) = decode(&fixture, &session.tick().expect("first attempt"));
        let signal_id = first["signal"]["signal_id"].as_str().unwrap().to_string();
        let mut message_ids = vec![first["message_id"].as_str().unwrap().to_string()];

        // Frozen backoff: 1 s, then 2 s, each after the 5 s acknowledgement
        // timeout. Three attempts in total, then the entry is retained rather
        // than resent forever.
        for backoff in [RETRY_BACKOFF_SECONDS[0], RETRY_BACKOFF_SECONDS[1]] {
            fixture.clock.advance(ACK_TIMEOUT_SECONDS + backoff);
            let (kind, retry) = decode(&fixture, &session.tick().expect("retry"));
            assert_eq!(kind, "health_signal");
            assert_eq!(
                retry["signal"]["signal_id"], signal_id,
                "a resend reuses the frozen idempotency key"
            );
            assert_eq!(retry["signal"]["sequence"], 1, "and the same sequence");
            let message_id = retry["message_id"].as_str().unwrap().to_string();
            assert!(
                !message_ids.contains(&message_id),
                "a resend must use a fresh message_id"
            );
            message_ids.push(message_id);
        }
        assert_eq!(message_ids.len(), MAX_RETRIES as usize);

        fixture
            .clock
            .advance(ACK_TIMEOUT_SECONDS + RETRY_BACKOFF_SECONDS[2]);
        assert!(
            session.tick().is_none(),
            "the frozen three-attempt bound must stop the resend loop"
        );
        let outbox = session.plane().outbox(64).expect("outbox");
        assert_eq!(outbox.len(), 1, "the Signal is retained, not dropped");
        assert_eq!(outbox[0].attempts, MAX_RETRIES);
        assert_eq!(
            outbox[0].expires_at - outbox[0].enqueued_at,
            crate::health_plane::bounds::SIGNAL_RETENTION_SECONDS
        );
    }

    #[test]
    fn an_exhausted_signal_is_resent_on_the_next_session() {
        let fixture = fixture();
        grant_notifications(&fixture);
        let signal_id;
        let mut message_ids: Vec<String> = Vec::new();
        {
            let mut session = fixture.conductor_session();
            settle(&fixture, &mut session);
            fixture
                .facts
                .finish_run(&"f".repeat(32), "deploy", BASE_NOW + 1);
            fixture.clock.advance(1);
            let (_, first) = decode(&fixture, &session.tick().expect("first attempt"));
            signal_id = first["signal"]["signal_id"].as_str().unwrap().to_string();
            message_ids.push(first["message_id"].as_str().unwrap().to_string());

            // Spend the frozen three attempts for this session.
            for backoff in [RETRY_BACKOFF_SECONDS[0], RETRY_BACKOFF_SECONDS[1]] {
                fixture.clock.advance(ACK_TIMEOUT_SECONDS + backoff);
                let (_, retry) = decode(&fixture, &session.tick().expect("retry"));
                message_ids.push(retry["message_id"].as_str().unwrap().to_string());
            }
            fixture
                .clock
                .advance(ACK_TIMEOUT_SECONDS + RETRY_BACKOFF_SECONDS[2]);
            assert!(
                session.tick().is_none(),
                "the frozen bound is three attempts per session"
            );
            let outbox = session.plane().outbox(64).expect("outbox");
            assert_eq!(outbox.len(), 1, "the Signal is retained, never dropped");
            assert_eq!(outbox[0].attempts, MAX_RETRIES);
        }
        assert_eq!(message_ids.len(), MAX_RETRIES as usize);

        // A new session re-arms the delivery budget exactly once, which is the
        // frozen "resent on the next session" rule.
        let mut next = fixture.conductor_session();
        settle(&fixture, &mut next);
        fixture.clock.advance(1);
        let (kind, resent) = decode(&fixture, &next.tick().expect("resend on the next session"));
        assert_eq!(kind, "health_signal");
        assert_eq!(
            resent["signal"]["signal_id"], signal_id,
            "a resend reuses the frozen idempotency key"
        );
        assert_eq!(resent["signal"]["sequence"], 1, "and the same sequence");
        let resent_message_id = resent["message_id"].as_str().unwrap().to_string();
        assert!(
            !message_ids.contains(&resent_message_id),
            "a resend must use a fresh message_id"
        );

        // Re-armed, not widened: the new session gets three attempts, not four,
        // and the outbox still holds exactly one bounded entry.
        let outbox = next.plane().outbox(64).expect("outbox");
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].attempts, 1, "one attempt spent in this session");
        assert_eq!(
            outbox[0].expires_at - outbox[0].enqueued_at,
            crate::health_plane::bounds::SIGNAL_RETENTION_SECONDS,
            "re-arming never extends the frozen 7-day retention"
        );

        // The budget is re-armed on connect, never mid-session: this session
        // still stops after its own three attempts.
        let mut sent = 1;
        for backoff in [RETRY_BACKOFF_SECONDS[0], RETRY_BACKOFF_SECONDS[1]] {
            fixture.clock.advance(ACK_TIMEOUT_SECONDS + backoff);
            if next.tick().is_some() {
                sent += 1;
            }
        }
        fixture
            .clock
            .advance(ACK_TIMEOUT_SECONDS + RETRY_BACKOFF_SECONDS[2]);
        assert!(next.tick().is_none());
        assert_eq!(sent, MAX_RETRIES, "still three attempts inside one session");
        assert_eq!(
            next.plane().outbox(64).expect("outbox")[0].attempts,
            MAX_RETRIES
        );
    }

    #[test]
    fn a_new_session_resends_the_same_logical_signal_after_a_reconnect() {
        let fixture = fixture();
        grant_notifications(&fixture);
        let signal_id;
        let first_message_id;
        {
            let mut session = fixture.conductor_session();
            settle(&fixture, &mut session);
            fixture
                .facts
                .finish_run(&"d".repeat(32), "deploy", BASE_NOW + 1);
            fixture.clock.advance(1);
            let (_, payload) = decode(&fixture, &session.tick().expect("first attempt"));
            signal_id = payload["signal"]["signal_id"].as_str().unwrap().to_string();
            first_message_id = payload["message_id"].as_str().unwrap().to_string();
        }

        // A brand new session over a brand new reporter: the durable outbox is
        // the only thing that carries the Signal across the reconnect.
        let mut session = fixture.conductor_session();
        settle(&fixture, &mut session);
        fixture.clock.advance(1);
        let (kind, resent) = decode(&fixture, &session.tick().expect("resend"));
        assert_eq!(kind, "health_signal");
        assert_eq!(resent["signal"]["signal_id"], signal_id);
        assert_eq!(resent["signal"]["sequence"], 1);
        assert_ne!(resent["message_id"], first_message_id.as_str());
        assert_eq!(
            session.plane().outbox(64).expect("outbox").len(),
            1,
            "a reconnect must not duplicate the queued Signal"
        );
    }

    #[test]
    fn the_signal_send_rate_stays_inside_the_frozen_per_minute_bound() {
        let fixture = fixture();
        grant_notifications(&fixture);
        let mut session = fixture.conductor_session();
        settle(&fixture, &mut session);
        for index in 0..(MAX_SIGNALS_PER_PEER_PER_MINUTE as usize + 4) {
            fixture
                .facts
                .finish_run(&format!("{index:032x}"), "deploy", BASE_NOW + 1);
        }

        let mut sent = 0;
        // One in-flight message at a time, so each accepted Signal is
        // acknowledged before the next is offered. The window is one minute.
        for _ in 0..120 {
            fixture.clock.advance(1);
            let Some(encoded) = session.tick() else {
                continue;
            };
            let (kind, payload) = decode(&fixture, &encoded);
            ack(&mut session, &payload);
            if kind == "health_signal" {
                sent += 1;
            }
            if fixture.clock.unix_seconds() >= BASE_NOW + RATE_MINUTE_WINDOW_SECONDS {
                break;
            }
        }
        assert!(
            sent <= MAX_SIGNALS_PER_PEER_PER_MINUTE,
            "sent {sent} Signals in one minute; the frozen bound is {MAX_SIGNALS_PER_PEER_PER_MINUTE}"
        );
        assert!(sent > 0, "the feed must actually drain");
    }

    #[test]
    fn revoking_the_conductor_stops_the_signal_feed_on_the_next_tick() {
        let fixture = fixture();
        grant_notifications(&fixture);
        let mut session = fixture.conductor_session();
        settle(&fixture, &mut session);
        fixture
            .facts
            .finish_run(&"e".repeat(32), "deploy", BASE_NOW + 1);
        fixture.clock.advance(1);
        assert!(session.tick().is_some(), "the Signal leaves while trusted");

        fixture
            .registry
            .revoke_peer(
                &fixture.conductor,
                "direct-health-tests",
                "revoked during the certification",
            )
            .expect("revoke conductor");
        for _ in 0..5 {
            fixture.clock.advance(ACK_TIMEOUT_SECONDS + 8);
            assert!(
                session.tick().is_none(),
                "a revoked Conductor must never receive another Signal"
            );
        }
    }

    #[test]
    fn a_non_health_envelope_is_left_to_the_existing_steady_state_behavior() {
        let fixture = fixture();
        let mut session = fixture.conductor_session();
        let probe = crate::direct_transport::sign_probe(
            &fixture.identity,
            &SESSION_ID,
            [0x11; 16],
            BASE_NOW as u64,
        )
        .expect("sign probe");
        assert_eq!(
            session.handle_envelope(&probe.encoded()),
            HealthOutcome::NotHealth
        );
    }
}

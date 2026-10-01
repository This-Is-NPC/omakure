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
use crate::util::entropy;
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
    /// The registry could neither decide nor record the message.
    ///
    /// This is not a drop. A drop is a decision the contract audits durably;
    /// this is the audit itself failing, and the message is lost with it. The
    /// silence the contract prescribes is toward the sender, so the caller
    /// still writes nothing back, but it owes the operator the failure.
    Failed { kind: String, error: String },
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
            return self.audit_transport_failure(&kind_text, encoded.len(), error, now);
        }
        let Ok(view) = envelope_view(encoded) else {
            return self.audit_transport_failure(
                &kind_text,
                encoded.len(),
                TransportError::InvalidFrame,
                now,
            );
        };

        // Steps 2 through 15 belong to the Wave 2 shared operations, in full.
        let ingest = plane.ingest(InboundHealthMessage {
            sender: &self.remote_node_id,
            kind: &kind_text,
            created_at: view.created_at,
            canonical_len,
            payload: &view.payload,
        });
        let ingest = match ingest {
            Ok(ingest) => ingest,
            Err(error) => {
                return HealthOutcome::Failed {
                    kind: kind_text,
                    error: error.to_string(),
                }
            }
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
        entropy::fill_bytes(&mut nonce);
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
    /// Drop the envelope and audit the drop; the audit failing is the outcome.
    fn audit_transport_failure(
        &self,
        kind: &str,
        byte_count: usize,
        error: TransportError,
        now: i64,
    ) -> HealthOutcome {
        let code = transport_failure_code(error);
        let wire = HealthKind::parse(kind)
            .map(HealthKind::wire)
            .unwrap_or("unknown");
        match self.registry.record_health_audit(
            wire,
            &self.remote_node_id,
            wire,
            byte_count as i64,
            "dropped",
            Some(code.code()),
            now,
        ) {
            Ok(()) => HealthOutcome::Handled,
            Err(error) => HealthOutcome::Failed {
                kind: kind.to_string(),
                error: error.to_string(),
            },
        }
    }
}

/// The frozen transport-layer failure mapping.
///
/// See `docs/internal/health-plane-contract.md`, "Transport-layer failure mapping".
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
    entropy::fill_bytes(&mut bytes);
    crate::util::hex::encode(&bytes)
}

#[cfg(test)]
mod tests;

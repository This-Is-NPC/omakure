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
//! * the Performer-side emission schedule for Profile and Pulse.
//!
//! It deliberately does **not** own authorization, presence, ordering,
//! idempotency, capacity, or any Health Plane table. Wave 2 is the single
//! fail-closed owner of all of those, and every inbound message reaches it
//! through `HealthPlane::ingest` with no shortcut, no cache, and no second
//! opinion. The only registry call made here is the redacted audit row for a
//! transport-layer failure, which happens before a message can reach ingest at
//! all.

use crate::direct_transport::{
    envelope_kind_hint, envelope_nonce, health_envelope_view, sign_health_envelope,
    verify_envelope, TransportError, HEALTH_KIND_PREFIX,
};
use crate::health_plane::bounds::{
    ACK_TIMEOUT_SECONDS, MAX_RETRIES, MIN_PULSE_INTERVAL_SECONDS, RETRY_BACKOFF_SECONDS,
    VERSION_INCOMPATIBLE_BACKOFF_SECONDS,
};
use crate::health_plane::model::{HealthCode, HealthKind};
use crate::health_plane::report::{ack_payload, error_payload, HealthReporter};
use crate::health_plane::{HealthPlane, HealthReply, InboundHealthMessage};
use crate::node_identity::NodeIdentity;
use crate::node_registry::{NodeRegistry, PeerRole, PeerState};
use rand::rngs::OsRng;
use rand::RngCore;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// One outstanding Profile or Pulse awaiting its acknowledgement.
struct Pending {
    kind: HealthKind,
    message_id: String,
    sent_at: Instant,
    attempts: i64,
}

/// The Health Plane state attached to one established direct session.
pub struct HealthSession<'a> {
    identity: &'a NodeIdentity,
    registry: &'a NodeRegistry,
    remote_node_id: String,
    remote_identity_key: [u8; 32],
    session_id: [u8; 32],
    reporter: Option<Arc<HealthReporter>>,
    pending: Option<Pending>,
    /// When the next Profile should be built, if one is due.
    profile_due: bool,
    /// The instant the next Pulse is due.
    next_pulse: Instant,
    /// The instant the last Pulse actually left this node.
    last_pulse_sent: Option<Instant>,
    /// Set when the Conductor answered `health_unsupported_version` (1101).
    suppressed_until: Option<Instant>,
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
        Self {
            identity,
            registry,
            remote_node_id: remote_node_id.to_string(),
            remote_identity_key: *remote_identity_key,
            session_id,
            reporter,
            pending: None,
            profile_due: true,
            next_pulse: Instant::now(),
            last_pulse_sent: None,
            suppressed_until: None,
        }
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
        let plane = HealthPlane::new(self.registry);
        let now = plane.now();

        // Step 1: the frozen transport verification path. A failure here is
        // dropped and audited without a reply, because the sender is not yet
        // proven authorized and target-bound.
        if let Err(error) = self.verify(encoded, &kind_text) {
            self.audit_transport_failure(&kind_text, encoded.len(), error, now);
            return HealthOutcome::Handled;
        }
        let Ok(view) = health_envelope_view(encoded) else {
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
        if authorization.0 != LocalRole::Performer {
            // Not (or no longer) reporting to this peer. Any pending send is
            // abandoned rather than retried against an unauthorized peer.
            self.pending = None;
            return None;
        }
        if self
            .suppressed_until
            .is_some_and(|until| Instant::now() < until)
        {
            return None;
        }
        let plane = HealthPlane::new(self.registry);
        let now = plane.now();
        if let Some(retry) = self.retry_due() {
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
        if Instant::now() < self.next_pulse {
            return None;
        }
        let message = reporter.pulse(&self.remote_node_id, &fresh_id(), now)?;
        self.next_pulse = Instant::now()
            + Duration::from_secs(HealthReporter::pulse_interval_seconds().max(1) as u64);
        self.last_pulse_sent = Some(Instant::now());
        self.send(HealthKind::Pulse, message.payload, now)
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
    fn retry_due(&mut self) -> Option<Option<Vec<u8>>> {
        let pending = self.pending.as_ref()?;
        if pending.sent_at.elapsed() < Duration::from_secs(ACK_TIMEOUT_SECONDS.max(0) as u64) {
            return Some(None);
        }
        let attempts = pending.attempts;
        let kind = pending.kind;
        if attempts >= MAX_RETRIES {
            // Final retry exhausted. The next scheduled Profile or Pulse
            // supersedes it; nothing is queued and nothing is appended.
            self.pending = None;
            if kind == HealthKind::Profile {
                self.profile_due = true;
            }
            return Some(None);
        }
        let backoff = RETRY_BACKOFF_SECONDS
            .get(attempts as usize)
            .copied()
            .unwrap_or(*RETRY_BACKOFF_SECONDS.last().unwrap_or(&1));
        let mut wait = Duration::from_secs(backoff.max(0) as u64);
        if kind == HealthKind::Pulse {
            let floor = Duration::from_secs(MIN_PULSE_INTERVAL_SECONDS.max(0) as u64);
            let since = self
                .last_pulse_sent
                .map(|sent| sent.elapsed())
                .unwrap_or(floor);
            wait = wait.max(floor.saturating_sub(since));
        }
        if pending.sent_at.elapsed() < Duration::from_secs(ACK_TIMEOUT_SECONDS.max(0) as u64) + wait
        {
            return Some(None);
        }
        let reporter = self.reporter.clone()?;
        let authorization = self.authorization();
        let plane = HealthPlane::new(self.registry);
        let now = plane.now();
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
            self.last_pulse_sent = Some(Instant::now());
        }
        let encoded = self.send(kind, payload, now);
        if let Some(pending) = self.pending.as_mut() {
            pending.attempts = attempts + 1;
        }
        Some(encoded)
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
            sent_at: Instant::now(),
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
                Instant::now()
                    + Duration::from_secs(VERSION_INCOMPATIBLE_BACKOFF_SECONDS.max(0) as u64),
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
}

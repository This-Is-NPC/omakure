use super::IDLE_TIMEOUT;
use super::baseline::{BaselineAckMatch, OutboundBaseline, sign_pending_baseline};
use super::connection::ConnectionState;
use super::cue::{CueAckMatch, OutboundCue, sign_pending_cue};
use super::error::DirectServiceError;
use super::outbox::OutboxGuard;
use super::stream::{read_frame, write_bytes};
use crate::direct_health::{HealthOutcome, HealthSession};
use crate::direct_transport::{
    ENVELOPE_KIND, Frame, TransportError, TransportSession, unix_seconds,
};
use crate::node_identity::NodeIdentity;
use crate::node_registry::{NodeRegistry, PeerState, TransportPeer};
use crate::remote_cue::{CueCode, CueOutcome};
use std::io::{self, Read};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

enum SessionStep {
    Continue,
    Stop,
}

struct ActiveSession<'a, 'health, 'cue, 'baseline> {
    stream: &'a mut TcpStream,
    transport: &'a mut TransportSession,
    state: &'a Arc<ConnectionState>,
    identity: &'a NodeIdentity,
    registry: &'a NodeRegistry,
    peer_node_id: &'a str,
    peer_identity_key: &'a [u8; 32],
    health: &'a mut HealthSession<'health>,
    cue: &'a mut Option<crate::remote_cue::CueSession<'cue>>,
    baseline: &'a mut Option<crate::baseline_push::BaselineSession<'baseline>>,
    last_activity: Instant,
    outbound_cue: Option<OutboundCue>,
    outbound_baseline: Option<OutboundBaseline>,
}

pub(super) struct SessionInputs<'a, 'health, 'cue, 'baseline> {
    pub(super) state: &'a Arc<ConnectionState>,
    pub(super) identity: &'a NodeIdentity,
    pub(super) registry: &'a NodeRegistry,
    pub(super) peer_node_id: &'a str,
    pub(super) peer_identity_key: &'a [u8; 32],
    pub(super) health: Option<HealthSession<'health>>,
    pub(super) cue: Option<crate::remote_cue::CueSession<'cue>>,
    pub(super) baseline: Option<crate::baseline_push::BaselineSession<'baseline>>,
}

/// The single shared steady-state receive loop for both connection directions.
///
/// With `health` absent the loop keeps its original behavior exactly: it
/// decrypts each application frame and discards the plaintext, and it returns
/// when the peer goes idle for `IDLE_TIMEOUT`.
///
/// With `health` present the loop additionally dispatches Health Plane
/// envelopes into the Wave 2 shared operations and runs the Performer emission
/// schedule. It waits on a short readability tick rather than a long blocking
/// read so a cadence, a retry, a revocation, or a stop request is observed
/// promptly; the tick never consumes bytes, so a partially arrived frame can
/// never desynchronize the stream.
pub(super) fn hold_session(
    stream: &mut TcpStream,
    session: &mut TransportSession,
    mut inputs: SessionInputs<'_, '_, '_, '_>,
) -> Result<(), DirectServiceError> {
    if inputs
        .health
        .as_ref()
        .is_some_and(|health| !health.engaged())
    {
        inputs.health = None;
    }
    let Some(health) = inputs.health.as_mut() else {
        return hold_session_idle(stream, session, inputs.state).map_err(Into::into);
    };
    stream
        .set_read_timeout(Some(crate::direct_health::TICK))
        .map_err(|_| TransportError::Internal)?;
    // Anything still queued when this session ends must not wait out its
    // budget for a connection that is gone.
    let _drain = OutboxGuard {
        state: inputs.state,
        peer_node_id: inputs.peer_node_id,
    };
    let mut active = ActiveSession {
        stream,
        transport: session,
        state: inputs.state,
        identity: inputs.identity,
        registry: inputs.registry,
        peer_node_id: inputs.peer_node_id,
        peer_identity_key: inputs.peer_identity_key,
        health,
        cue: &mut inputs.cue,
        baseline: &mut inputs.baseline,
        last_activity: Instant::now(),
        outbound_cue: None,
        outbound_baseline: None,
    };
    while !active.state.stop.load(Ordering::SeqCst) {
        if matches!(active.step()?, SessionStep::Stop) {
            return Ok(());
        }
    }
    Ok(())
}

impl ActiveSession<'_, '_, '_, '_> {
    fn step(&mut self) -> Result<SessionStep, DirectServiceError> {
        self.send_pending_cue()?;
        self.send_pending_baseline()?;
        self.refresh_outbound();
        self.send_health_tick()?;
        match wait_readable(self.stream, crate::direct_health::TICK) {
            Readiness::Readable => self.receive_frame(),
            Readiness::Idle => {
                // Withdrawal is checked even when no inbound frame arrives.
                if let Some(error) = health_authorization_error(self.registry, self.peer_node_id) {
                    return Err(error);
                }
                if self.last_activity.elapsed() >= IDLE_TIMEOUT {
                    Ok(SessionStep::Stop)
                } else {
                    Ok(SessionStep::Continue)
                }
            }
            Readiness::Closed => Ok(SessionStep::Stop),
            Readiness::Failed(error) => Err(error.into()),
        }
    }

    fn send_pending_cue(&mut self) -> Result<(), DirectServiceError> {
        // One Cue in flight per session.
        if let Some(pending) = self
            .outbound_cue
            .is_none()
            .then(|| self.state.take_pending_cue(self.peer_node_id))
            .flatten()
        {
            let deadline = Instant::now() + IDLE_TIMEOUT;
            match sign_pending_cue(self.identity, self.transport.session_id(), &pending) {
                Ok(encoded) => {
                    write_bytes(
                        self.stream,
                        &self.transport.write(ENVELOPE_KIND, &encoded)?,
                        deadline,
                    )
                    .map_err(error_to_transport)?;
                    self.last_activity = Instant::now();
                    self.outbound_cue = Some(OutboundCue::new(pending));
                }
                Err(_) => pending.answer(false, false, CueCode::InvalidMessage.code(), false),
            }
        }
        Ok(())
    }

    fn send_pending_baseline(&mut self) -> Result<(), DirectServiceError> {
        // An answered slot remains available for late ack correlation.
        if let Some(pending) = self
            .outbound_baseline
            .as_ref()
            .is_none_or(OutboundBaseline::is_answered)
            .then(|| self.state.take_pending_baseline(self.peer_node_id))
            .flatten()
        {
            let deadline = Instant::now() + IDLE_TIMEOUT;
            match sign_pending_baseline(self.identity, self.transport.session_id(), &pending) {
                Ok(encoded) => {
                    write_bytes(
                        self.stream,
                        &self.transport.write(ENVELOPE_KIND, &encoded)?,
                        deadline,
                    )
                    .map_err(error_to_transport)?;
                    self.last_activity = Instant::now();
                    self.outbound_baseline = Some(OutboundBaseline::new(pending));
                }
                Err(code) => pending.answer(false, false, code),
            }
        }
        Ok(())
    }

    fn refresh_outbound(&mut self) {
        if self
            .outbound_cue
            .as_mut()
            .is_some_and(|in_flight| in_flight.resolve(self.registry, self.peer_node_id))
        {
            self.outbound_cue = None;
        }
        if let Some(in_flight) = self.outbound_baseline.as_mut() {
            in_flight.expire_if_due();
        }
    }

    fn send_health_tick(&mut self) -> Result<(), DirectServiceError> {
        if let Some(outbound) = self.health.tick() {
            let deadline = Instant::now() + IDLE_TIMEOUT;
            write_bytes(
                self.stream,
                &self.transport.write(ENVELOPE_KIND, &outbound)?,
                deadline,
            )
            .map_err(error_to_transport)?;
            self.last_activity = Instant::now();
        }
        Ok(())
    }

    fn receive_frame(&mut self) -> Result<SessionStep, DirectServiceError> {
        let deadline = Instant::now() + IDLE_TIMEOUT;
        let encoded = match read_frame(self.stream, deadline) {
            Ok(encoded) => encoded,
            Err(DirectServiceError::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Ok(SessionStep::Stop);
            }
            Err(error) => return Err(error_to_transport(error).into()),
        };
        let frame = Frame::parse(&encoded)?;
        if frame.kind != 2 {
            return Err(TransportError::InvalidFrame.into());
        }
        let message = self.transport.read(&encoded)?;
        self.last_activity = Instant::now();
        if message.kind == ENVELOPE_KIND {
            self.handle_envelope(&message.body, deadline)?;
        }
        Ok(SessionStep::Continue)
    }

    fn handle_envelope(
        &mut self,
        body: &[u8],
        deadline: Instant,
    ) -> Result<(), DirectServiceError> {
        // Health ingest precedes the proactive local trust check, so a queued
        // message from a revoked peer receives its durable health audit.
        let outcome = self.health.handle_envelope(body);
        if let Some(error) = health_authorization_error(self.registry, self.peer_node_id) {
            return Err(error);
        }
        if self.absorb_cue_ack(body) || self.absorb_baseline_ack(body) {
            return Ok(());
        }
        match outcome {
            HealthOutcome::NotHealth => self.dispatch_other_planes(body, deadline)?,
            HealthOutcome::Handled => {}
            HealthOutcome::Reply(reply) => {
                write_bytes(
                    self.stream,
                    &self.transport.write(ENVELOPE_KIND, &reply)?,
                    deadline,
                )
                .map_err(error_to_transport)?;
            }
            HealthOutcome::Failed { kind, error } => {
                eprintln!(
                    "omakure.health_ingest_failure {}",
                    serde_json::json!({
                        "peer": self.peer_node_id,
                        "kind": kind,
                        "error": error,
                    })
                );
            }
        }
        Ok(())
    }

    fn absorb_cue_ack(&mut self, body: &[u8]) -> bool {
        let Some(in_flight) = self.outbound_cue.as_mut() else {
            return false;
        };
        match in_flight.absorb_ack(
            body,
            self.peer_node_id,
            self.peer_identity_key,
            self.transport.session_id(),
        ) {
            CueAckMatch::Other => false,
            CueAckMatch::Accepted => true,
            CueAckMatch::Refused => {
                self.outbound_cue = None;
                true
            }
        }
    }

    fn absorb_baseline_ack(&mut self, body: &[u8]) -> bool {
        let Some(in_flight) = self.outbound_baseline.as_mut() else {
            return false;
        };
        match in_flight.absorb_ack(
            body,
            self.peer_node_id,
            self.peer_identity_key,
            self.transport.session_id(),
        ) {
            BaselineAckMatch::Other => false,
            BaselineAckMatch::Answered => {
                self.outbound_baseline = None;
                true
            }
            BaselineAckMatch::Late { accepted, code } => {
                let _ =
                    self.registry
                        .record_transport_audit(crate::node_registry::TransportAudit {
                            event_type: "baseline_answered_late",
                            node_id: self.peer_node_id,
                            session_id: Some(self.transport.session_id()),
                            direction: None,
                            byte_count: 0,
                            outcome: if accepted { "accepted" } else { "rejected" },
                            error_code: (!accepted).then_some(code),
                            cue: None,
                        });
                self.outbound_baseline = None;
                true
            }
        }
    }

    fn dispatch_other_planes(
        &mut self,
        body: &[u8],
        deadline: Instant,
    ) -> Result<(), DirectServiceError> {
        if let Some(baseline) = self.baseline.as_mut().and_then(|baseline| {
            (baseline.handle_envelope(body, unix_seconds())
                != crate::baseline_push::BaselineOutcome::NotBaseline)
                .then_some(baseline)
        }) {
            if let Some(reply) = baseline.take_reply() {
                write_bytes(
                    self.stream,
                    &self.transport.write(ENVELOPE_KIND, &reply)?,
                    deadline,
                )
                .map_err(error_to_transport)?;
            }
            return Ok(());
        }
        if let Some(cue) = self.cue.as_mut() {
            match cue.handle_envelope(body, unix_seconds() as i64) {
                CueOutcome::EnqueueFailed(error) => {
                    return Err(DirectServiceError::CueEnqueueFailed { error });
                }
                CueOutcome::Decided(_) | CueOutcome::Repeat => {
                    if let Some(reply) = cue.take_reply() {
                        write_bytes(
                            self.stream,
                            &self.transport.write(ENVELOPE_KIND, &reply)?,
                            deadline,
                        )
                        .map_err(error_to_transport)?;
                    }
                }
                CueOutcome::NotCue => {}
            }
        }
        Ok(())
    }
}

/// The pre-Health-Plane steady-state loop.
fn hold_session_idle(
    stream: &mut TcpStream,
    session: &mut TransportSession,
    state: &Arc<ConnectionState>,
) -> Result<(), TransportError> {
    stream
        .set_read_timeout(Some(IDLE_TIMEOUT))
        .map_err(|_| TransportError::Internal)?;
    while !state.stop.load(Ordering::SeqCst) {
        let deadline = Instant::now() + IDLE_TIMEOUT;
        // Idle time is distinct from the deadline for a partial frame header.
        match wait_readable(stream, IDLE_TIMEOUT) {
            Readiness::Readable => {}
            Readiness::Idle | Readiness::Closed => return Ok(()),
            Readiness::Failed(error) => return Err(error),
        }
        match read_frame(stream, deadline) {
            Ok(encoded) => {
                let frame = Frame::parse(&encoded)?;
                if frame.kind != 2 {
                    return Err(TransportError::InvalidFrame);
                }
                let _ = session.read(&encoded)?;
            }
            Err(DirectServiceError::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error_to_transport(error)),
        }
    }
    Ok(())
}

/// The result of one non-consuming readability probe.
pub(super) enum Readiness {
    Readable,
    Idle,
    Closed,
    Failed(TransportError),
}

/// Read and discard whatever the peer is still sending, briefly.
///
/// Closing a socket whose receive queue still holds unread bytes makes the
/// kernel send RST, and an RST discards data this node has written but the peer
/// has not read yet. A refusal written immediately before the close is exactly
/// such data: the initiator writes its probe the instant the handshake
/// finishes, so those bytes are almost always sitting unread when the refusal
/// goes out. Without this the peer would be back to inferring `internal` from a
/// dead connection, which is the failure this exists to fix.
///
/// Bounded by time and by bytes, because the peer on the other end is one this
/// node has just refused and it does not get to hold a worker by talking.
pub(super) fn drain_until_hangup(stream: &mut TcpStream) {
    const BUDGET: Duration = Duration::from_millis(250);
    const MAX_BYTES: usize = 64 * 1024;
    let deadline = Instant::now() + BUDGET;
    let mut scratch = [0u8; 4096];
    let mut seen = 0usize;
    while Instant::now() < deadline && seen < MAX_BYTES {
        match wait_readable(stream, Duration::from_millis(25)) {
            Readiness::Readable => match stream.read(&mut scratch) {
                Ok(0) | Err(_) => return,
                Ok(count) => seen = seen.saturating_add(count),
            },
            Readiness::Idle => continue,
            Readiness::Closed | Readiness::Failed(_) => return,
        }
    }
}

/// Wait up to `tick` for the peer to send something, without consuming it.
pub(super) fn wait_readable(stream: &TcpStream, tick: Duration) -> Readiness {
    if stream.set_read_timeout(Some(tick)).is_err() {
        return Readiness::Failed(TransportError::Internal);
    }
    let mut probe = [0u8; 1];
    match stream.peek(&mut probe) {
        Ok(0) => Readiness::Closed,
        Ok(_) => Readiness::Readable,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ) =>
        {
            Readiness::Idle
        }
        // Anything other than an orderly close or a tick expiry is the same
        // failure the blocking loop reported before the tick existed, so it
        // stays an error and stays audited.
        Err(_) => Readiness::Failed(TransportError::Internal),
    }
}

pub(super) fn error_to_transport(error: DirectServiceError) -> TransportError {
    match error {
        DirectServiceError::Protocol(error) => error,
        // A refusal already carries a code from the frozen table; flattening it
        // to `internal` would be the same loss this fix exists to stop.
        DirectServiceError::PeerNotActive { protocol, .. } => protocol,
        _ => TransportError::Internal,
    }
}

pub(super) fn peer_authorization(
    peer: &TransportPeer,
) -> (
    &str,
    &[u8; 32],
    Option<&[u8; 32]>,
    Option<u64>,
    crate::node_registry::PeerState,
) {
    (
        &peer.node_id,
        &peer.identity_key,
        peer.transport_public_key.as_ref(),
        peer.key_epoch,
        peer.state,
    )
}

fn health_authorization_error(
    registry: &NodeRegistry,
    peer_node_id: &str,
) -> Option<DirectServiceError> {
    let Ok(Some(authorization)) = registry.health_authorization(peer_node_id) else {
        return None;
    };
    if authorization.state == PeerState::Active {
        return None;
    }
    Some(match authorization.state {
        PeerState::Revoked => DirectServiceError::Protocol(TransportError::Revoked),
        _ => DirectServiceError::Protocol(TransportError::NotEnrolled),
    })
}

pub(super) fn audit_error(
    registry: &NodeRegistry,
    node_id: &str,
    session_id: Option<&[u8; 32]>,
    error: &TransportError,
) -> Result<(), DirectServiceError> {
    registry.record_transport_audit(crate::node_registry::TransportAudit {
        event_type: "probe_rejected",
        node_id,
        session_id,
        direction: Some(0),
        byte_count: 0,
        outcome: "rejected",
        error_code: Some(error.code() as u16),
        cue: None,
    })?;
    Ok(())
}

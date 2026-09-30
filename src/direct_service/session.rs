use super::baseline::{sign_pending_baseline, BaselineAckMatch, OutboundBaseline};
use super::connection::ConnectionState;
use super::cue::{sign_pending_cue, CueAckMatch, OutboundCue};
use super::error::DirectServiceError;
use super::outbox::OutboxGuard;
use super::stream::{read_frame, write_bytes};
use super::IDLE_TIMEOUT;
use crate::direct_health::{HealthOutcome, HealthSession};
use crate::direct_transport::{
    unix_seconds, Frame, TransportError, TransportSession, ENVELOPE_KIND,
};
use crate::node_identity::NodeIdentity;
use crate::node_registry::{NodeRegistry, PeerState, TransportPeer};
use crate::remote_cue::{CueCode, CueOutcome};
use std::io::{self, Read};
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
#[allow(clippy::too_many_arguments)]
pub(super) fn hold_session(
    stream: &mut TcpStream,
    session: &mut TransportSession,
    state: &Arc<ConnectionState>,
    identity: &NodeIdentity,
    registry: &NodeRegistry,
    peer_node_id: &str,
    peer_identity_key: &[u8; 32],
    mut health: Option<HealthSession<'_>>,
    mut cue: Option<crate::remote_cue::CueSession<'_>>,
    mut baseline: Option<crate::baseline_push::BaselineSession<'_>>,
) -> Result<(), DirectServiceError> {
    if health.as_ref().is_some_and(|health| !health.engaged()) {
        health = None;
    }
    let Some(health) = health.as_mut() else {
        return hold_session_idle(stream, session, state).map_err(Into::into);
    };
    stream
        .set_read_timeout(Some(crate::direct_health::TICK))
        .map_err(|_| TransportError::Internal)?;
    let mut last_activity = Instant::now();
    let mut outbound_cue: Option<OutboundCue> = None;
    let mut outbound_baseline: Option<OutboundBaseline> = None;
    // Anything still queued when this session ends must not wait out its
    // budget for a connection that is gone.
    let _drain = OutboxGuard {
        state,
        peer_node_id,
    };
    while !state.stop.load(Ordering::SeqCst) {
        // One Cue in flight per session, which is the bound the contract
        // already freezes at `concurrent_cue_runs_per_peer = 1`.
        if outbound_cue.is_none() {
            if let Some(pending) = state.take_pending_cue(peer_node_id) {
                let deadline = Instant::now() + IDLE_TIMEOUT;
                match sign_pending_cue(identity, session.session_id(), &pending) {
                    Ok(encoded) => {
                        write_bytes(stream, &session.write(ENVELOPE_KIND, &encoded)?, deadline)
                            .map_err(error_to_transport)?;
                        last_activity = Instant::now();
                        outbound_cue = Some(OutboundCue::new(pending));
                    }
                    // A Cue this node cannot even sign is answered rather than
                    // dropped; the caller is waiting on the channel.
                    Err(_) => pending.answer(false, false, CueCode::InvalidMessage.code(), false),
                }
            }
        }
        // One baseline in flight per session. A second would put megabytes on
        // the wire behind a message the peer has not answered yet, and the
        // answer is what says whether the first one was even wanted.
        if outbound_baseline
            .as_ref()
            .is_none_or(OutboundBaseline::is_answered)
        {
            if let Some(pending) = state.take_pending_baseline(peer_node_id) {
                let deadline = Instant::now() + IDLE_TIMEOUT;
                match sign_pending_baseline(identity, session.session_id(), &pending) {
                    Ok(encoded) => {
                        write_bytes(stream, &session.write(ENVELOPE_KIND, &encoded)?, deadline)
                            .map_err(error_to_transport)?;
                        last_activity = Instant::now();
                        outbound_baseline = Some(OutboundBaseline::new(pending));
                    }
                    // Too large to carry, or unsignable. The caller is waiting
                    // on the channel and must be told rather than left to time
                    // out on something this node already knows the answer to.
                    Err(code) => pending.answer(false, false, code),
                }
            }
        }
        if let Some(in_flight) = outbound_cue.as_mut() {
            if in_flight.resolve(registry, peer_node_id) {
                outbound_cue = None;
            }
        }
        if let Some(in_flight) = outbound_baseline.as_mut() {
            in_flight.expire_if_due();
        }
        if let Some(outbound) = health.tick() {
            let deadline = Instant::now() + IDLE_TIMEOUT;
            write_bytes(stream, &session.write(ENVELOPE_KIND, &outbound)?, deadline)
                .map_err(error_to_transport)?;
            last_activity = Instant::now();
        }
        match wait_readable(stream, crate::direct_health::TICK) {
            Readiness::Readable => {}
            Readiness::Idle => {
                // An idle session still closes as soon as local authorization
                // is withdrawn; no inbound frame is available to hand to
                // HealthSession first.
                if let Some(error) = health_authorization_error(registry, peer_node_id) {
                    return Err(error);
                }
                if last_activity.elapsed() >= IDLE_TIMEOUT {
                    return Ok(());
                }
                continue;
            }
            Readiness::Closed => return Ok(()),
            Readiness::Failed(error) => return Err(error.into()),
        }
        let deadline = Instant::now() + IDLE_TIMEOUT;
        let encoded = match read_frame(stream, deadline) {
            Ok(encoded) => encoded,
            Err(DirectServiceError::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error_to_transport(error).into()),
        };
        let frame = Frame::parse(&encoded)?;
        if frame.kind != 2 {
            return Err(TransportError::InvalidFrame.into());
        }
        let message = session.read(&encoded)?;
        last_activity = Instant::now();
        if message.kind != ENVELOPE_KIND {
            continue;
        }
        // Readable frames get one and only one Health Plane ingest attempt
        // before the proactive authorization close. In particular, a revoked
        // peer's queued Health message records durable 1107 while the shared
        // operations leave all Health state untouched.
        let health_outcome = health.handle_envelope(&message.body);
        if let Some(error) = health_authorization_error(registry, peer_node_id) {
            return Err(error);
        }
        if let Some(in_flight) = outbound_cue.as_mut() {
            match in_flight.absorb_ack(
                &message.body,
                peer_node_id,
                peer_identity_key,
                session.session_id(),
            ) {
                CueAckMatch::Other => {}
                // The slot stays: the Cue is not finished until its outcome is
                // read back from the registry. The envelope *is* finished, and
                // handing it on is what audited an acceptance as malformed.
                CueAckMatch::Accepted => continue,
                CueAckMatch::Refused => {
                    outbound_cue = None;
                    continue;
                }
            }
        }
        if let Some(in_flight) = outbound_baseline.as_mut() {
            match in_flight.absorb_ack(
                &message.body,
                peer_node_id,
                peer_identity_key,
                session.session_id(),
            ) {
                BaselineAckMatch::Other => {}
                BaselineAckMatch::Answered => {
                    outbound_baseline = None;
                    continue;
                }
                // The caller was told `answered: false` and has gone. This row
                // is the only thing that can tell an operator the push landed
                // anyway, which is the difference between "retry it" and
                // "you already have it".
                BaselineAckMatch::Late { accepted, code } => {
                    let _ = registry.record_transport_audit(
                        "baseline_answered_late",
                        peer_node_id,
                        Some(session.session_id()),
                        None,
                        0,
                        if accepted { "accepted" } else { "rejected" },
                        (!accepted).then_some(code),
                    );
                    outbound_baseline = None;
                    continue;
                }
            }
        }
        match health_outcome {
            // A non-health envelope used to be discarded here without a trace.
            // Cue traffic is decided and audited instead; anything else keeps
            // the original silence, so the dispatcher never becomes an oracle
            // that answers unknown kinds.
            HealthOutcome::NotHealth => {
                if let Some(baseline) = baseline.as_mut() {
                    // The same door the Cue plane came in by: one fall-through
                    // from the Health dispatch, and each plane answers only for
                    // its own kind namespace.
                    if baseline.handle_envelope(&message.body, unix_seconds())
                        != crate::baseline_push::BaselineOutcome::NotBaseline
                    {
                        if let Some(reply) = baseline.take_reply() {
                            write_bytes(stream, &session.write(ENVELOPE_KIND, &reply)?, deadline)
                                .map_err(error_to_transport)?;
                        }
                        continue;
                    }
                }
                if let Some(cue) = cue.as_mut() {
                    // The Cue session verifies the envelope against the same
                    // handshake identity and session id the Health Plane uses;
                    // nothing here decides anything.
                    match cue.handle_envelope(&message.body, unix_seconds() as i64) {
                        CueOutcome::EnqueueFailed(error) => {
                            return Err(DirectServiceError::CueEnqueueFailed { error });
                        }
                        CueOutcome::Decided(_) | CueOutcome::Repeat => {
                            if let Some(reply) = cue.take_reply() {
                                write_bytes(
                                    stream,
                                    &session.write(ENVELOPE_KIND, &reply)?,
                                    deadline,
                                )
                                .map_err(error_to_transport)?;
                            }
                        }
                        CueOutcome::NotCue => {}
                    }
                }
            }
            HealthOutcome::Handled => {}
            HealthOutcome::Reply(reply) => {
                write_bytes(stream, &session.write(ENVELOPE_KIND, &reply)?, deadline)
                    .map_err(error_to_transport)?;
            }
            // Nothing goes back to the peer, as for any drop; the session
            // goes on, because the failure was the registry's moment, not the
            // peer's. What must not happen is what happened before: the
            // message vanishing with no reply, no audit row, and no line.
            HealthOutcome::Failed { kind, error } => {
                eprintln!(
                    "omakure.health_ingest_failure {}",
                    serde_json::json!({
                        "peer": peer_node_id,
                        "kind": kind,
                        "error": error,
                    })
                );
            }
        }
    }
    Ok(())
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
    registry.record_transport_audit(
        "probe_rejected",
        node_id,
        session_id,
        Some(0),
        0,
        "rejected",
        Some(error.code() as u16),
    )?;
    Ok(())
}

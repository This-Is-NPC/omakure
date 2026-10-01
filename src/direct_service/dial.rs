use super::connection::{ConnectionDirection, ConnectionState};
use super::error::DirectServiceError;
use super::resolver::Resolver;
use super::session::{
    SessionInputs, audit_error, error_to_transport, hold_session, peer_authorization,
};
use super::status::StaticPeer;
use super::stream::{initiator_deadline, read_frame, set_stream_timeouts, time_until, write_bytes};
use super::{RETRY_BACKOFF, RETRY_BACKOFF_CEILING, RETRY_JITTER_MAX};
use crate::direct_health::HealthSession;
use crate::direct_transport::{
    ENVELOPE_KIND, HandshakeRole, TransportError, authorize_peer, sign_probe, unix_seconds,
    verify_envelope,
};
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::NodeRegistry;
use crate::node_transport::LocalTransport;
use crate::util::entropy;
use crate::util::hex;
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub(super) fn dialer_loop(
    peer: StaticPeer,
    context: NodeContext,
    state: Arc<ConnectionState>,
    stop: Arc<AtomicBool>,
    resolver: Arc<Resolver>,
) {
    if !state.should_initiate(&peer.node_id) {
        // Only the lexicographically lower node ID dials a static peer. The
        // other side remains listener-only for this pair, which makes a
        // simultaneous restart converge without duplicate handshakes.
        while !stop.load(Ordering::SeqCst) {
            sleep_or_stop(Duration::from_millis(250), &stop);
        }
        return;
    }
    let mut failures = 0usize;
    while !stop.load(Ordering::SeqCst) {
        if state
            .active
            .lock()
            .ok()
            .is_some_and(|active| active.contains_key(&peer.node_id))
        {
            sleep_or_stop(Duration::from_millis(250), &stop);
            continue;
        }
        match connect_and_hold(&peer, &context, &state, &resolver) {
            Ok(()) => failures = 0,
            Err(error) => {
                state.record_direct_error(&peer.node_id, &error);
                if is_fatal_connection_error(&error_to_transport(error)) {
                    return;
                }
                // Nothing respawns this thread, so a peer that is merely
                // unreachable must never be allowed to retire it: the link
                // would stay dead until the process restarts. Back off toward
                // the ceiling instead, keeping the jitter so a fleet that
                // restarts together does not resynchronise on one instant.
                let delay = retry_backoff(failures).saturating_add(retry_jitter());
                failures = failures.saturating_add(1);
                sleep_or_stop(delay, &stop);
            }
        }
    }
}

fn is_fatal_connection_error(error: &TransportError) -> bool {
    matches!(
        error,
        TransportError::UnsupportedVersion
            | TransportError::InvalidFrame
            | TransportError::MessageTooLarge
            | TransportError::HandshakeFailed
            | TransportError::IdentityMismatch
            | TransportError::NotEnrolled
            | TransportError::Revoked
            | TransportError::Expired
            | TransportError::Replay
    )
}

/// The delay before the attempt that follows `failures` transient failures.
pub(super) fn retry_backoff(failures: usize) -> Duration {
    u32::try_from(failures)
        .ok()
        .and_then(|steps| 1u32.checked_shl(steps))
        .and_then(|factor| RETRY_BACKOFF[0].checked_mul(factor))
        .unwrap_or(RETRY_BACKOFF_CEILING)
        .min(RETRY_BACKOFF_CEILING)
}

fn retry_jitter() -> Duration {
    Duration::from_millis(
        u64::from(entropy::next_u32()) % (RETRY_JITTER_MAX.as_millis() as u64 + 1),
    )
}

fn sleep_or_stop(duration: Duration, stop: &AtomicBool) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
        thread::sleep(
            Duration::from_millis(25).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

pub(super) fn connect_and_hold(
    peer: &StaticPeer,
    context: &NodeContext,
    state: &Arc<ConnectionState>,
    resolver: &Resolver,
) -> Result<(), DirectServiceError> {
    let deadline = initiator_deadline(Instant::now());
    let endpoints = resolver.resolve(&peer.endpoint, deadline, &state.stop)?;
    // Everything that can fail without a socket is done before there is one.
    //
    // Each of these five steps used to run between the TCP connect and the
    // first handshake write, and each of them returns early. A local failure
    // -- no admission budget, an identity or transport file that has gone
    // away, a registry that will not open -- therefore left the peer holding
    // an accepted connection that sent nothing. The peer charges that stray to
    // its admission controller and writes it into its audit trail, so a fault
    // entirely on this side is recorded as the other side's problem.
    //
    // That was survivable while the dialer retired after three attempts. It is
    // not now: retries are unbounded with a sixty-second ceiling, so a
    // persistent local failure produces one stray a minute forever.
    //
    // None of these takes the stream, so the only cost of hoisting them is
    // that a dial now holds its admission reservation across the connect
    // attempt as well as the handshake. That is the more honest accounting --
    // a dial in flight is occupying a handshake slot -- and the reservation is
    // released by `Drop` on every early return below.
    //
    // The first handshake message is built here too, for the same reason: it
    // is fallible and it needs no socket. It carries no timestamp and no
    // freshness data -- the initiator's first Noise XX message has an empty
    // payload -- so building it before the connect changes nothing on the
    // wire. Unlike the five below it cannot be driven to fail on a fresh
    // handshake, so it is hoisted on the argument rather than on a test.
    //
    // What is left after the connect is `set_stream_timeouts` and the write
    // itself, and the only way either abandons the socket is the initiator
    // deadline expiring in between. `tests/docker_health_plane_exhaustion.rs`
    // records why that keeps its stray count a bound rather than zero.
    let mut reservation = state
        .admission
        .reserve_dial()
        .ok_or(TransportError::RateLimited)?;
    let identity = NodeIdentity::load_existing(context).map_err(|_| TransportError::Internal)?;
    let local =
        LocalTransport::load_existing(context, &identity).map_err(|_| TransportError::Internal)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())
        .map_err(|_| TransportError::Internal)?;
    let mut handshake = local
        .handshake(HandshakeRole::Initiator)
        .map_err(|_| TransportError::Internal)?;
    let opening_message = handshake.write_next()?;
    let mut stream = None;
    for endpoint in endpoints {
        let remaining = time_until(deadline)?;
        if let Ok(candidate) = TcpStream::connect_timeout(&endpoint, remaining) {
            stream = Some(candidate);
            break;
        }
    }
    let mut stream = stream.ok_or(TransportError::Internal)?;
    set_stream_timeouts(&stream, deadline)?;
    write_bytes(&mut stream, &opening_message, deadline).map_err(error_to_transport)?;
    let frame = read_frame(&mut stream, deadline).map_err(error_to_transport)?;
    handshake.read_next(&frame, unix_seconds())?;
    write_bytes(&mut stream, &handshake.write_next()?, deadline).map_err(error_to_transport)?;
    let remote = handshake
        .remote_certificate()
        .cloned()
        .ok_or(TransportError::HandshakeFailed)?;
    if remote.node_id() != peer.node_id {
        return Err(TransportError::IdentityMismatch.into());
    }
    let trusted = registry
        .transport_peer(remote.node_id(), &hex::encode(remote.identity_key()))
        .map_err(|_| TransportError::Internal)?;
    if let Err(error) = authorize_peer(
        &remote,
        trusted.as_ref().map(peer_authorization),
        unix_seconds(),
    ) {
        // The remote is authenticated, but this node's local authorization
        // refuses it before a TransportSession or probe exists. Record the
        // single redacted initiator-side refusal before returning; the
        // responder will never receive a probe to audit as a duplicate.
        audit_error(&registry, remote.node_id(), None, &error)?;
        return Err(error.into());
    }
    state
        .admission
        .migrate_node(&mut reservation, remote.node_id())?;
    let mut session = handshake.into_session()?;
    let mut nonce = [0u8; 16];
    entropy::fill_bytes(&mut nonce);
    let probe = sign_probe(&identity, session.session_id(), nonce, unix_seconds())?;
    write_bytes(
        &mut stream,
        &session.write(ENVELOPE_KIND, &probe.encoded())?,
        deadline,
    )
    .map_err(error_to_transport)?;
    let frame = read_frame(&mut stream, deadline).map_err(error_to_transport)?;
    let response = session.read(&frame)?;
    // A stated refusal is the peer's verdict, and it is the only way this node
    // can learn one: nothing local knows the peer revoked it. Reported as
    // itself so `is_fatal_connection_error` can retire this dialer instead of
    // reading a refusal as a transient fault and retrying it forever.
    if let Some(stated) = crate::direct_transport::stated_error(&response) {
        return Err(stated.into());
    }
    if response.kind != ENVELOPE_KIND {
        return Err(TransportError::InvalidFrame.into());
    }
    verify_envelope(
        &response.body,
        remote.node_id(),
        remote.identity_key(),
        "ack",
        session.session_id(),
        &nonce,
    )?;
    reservation.promote_session()?;
    let session_id = *session.session_id();
    let _claim = state.register(
        remote.node_id(),
        ConnectionDirection::Initiator,
        session_id,
        &stream,
    )?;
    registry
        .record_transport_audit(crate::node_registry::TransportAudit {
            event_type: "probe_accepted",
            node_id: remote.node_id(),
            session_id: Some(&session_id),
            direction: Some(0),
            byte_count: response.body.len() + probe.encoded().len(),
            outcome: "accepted",
            error_code: None,
            cue: None,
        })
        .map_err(|_| TransportError::Internal)?;
    let health = HealthSession::new(
        &identity,
        &registry,
        remote.node_id(),
        remote.identity_key(),
        session_id,
        state.reporter.clone(),
    );
    let cue = crate::remote_cue::CueSession::new(
        &registry,
        &identity,
        remote.node_id(),
        *remote.identity_key(),
        session_id,
        crate::remote_cue::read_policy(context),
        state
            .workspace_root
            .as_ref()
            .map(|root| crate::workspace::Workspace::new(root.clone())),
    );
    let baseline = crate::baseline_push::BaselineSession::new(
        &registry,
        &identity,
        remote.node_id(),
        *remote.identity_key(),
        session_id,
        crate::baseline_push::read_policy(context),
        state
            .workspace_root
            .as_ref()
            .map(|root| crate::workspace::Workspace::new(root.clone())),
    );
    hold_session(
        &mut stream,
        &mut session,
        SessionInputs {
            state,
            identity: &identity,
            registry: &registry,
            peer_node_id: remote.node_id(),
            peer_identity_key: remote.identity_key(),
            health: Some(health),
            cue: Some(cue),
            baseline: Some(baseline),
        },
    )
}

pub fn probe(
    endpoint: SocketAddr,
    expected_node_id: &str,
    context: &NodeContext,
) -> Result<(), DirectServiceError> {
    let identity = NodeIdentity::load_existing(context)?;
    let local = LocalTransport::load_existing(context, &identity)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())?;
    let deadline = initiator_deadline(Instant::now());
    let mut stream = TcpStream::connect_timeout(&endpoint, time_until(deadline)?)?;
    set_stream_timeouts(&stream, deadline).map_err(|_| io::Error::from(io::ErrorKind::Other))?;
    let mut handshake = local.handshake(HandshakeRole::Initiator)?;
    write_bytes(&mut stream, &handshake.write_next()?, deadline)?;
    handshake.read_next(&read_frame(&mut stream, deadline)?, unix_seconds())?;
    write_bytes(&mut stream, &handshake.write_next()?, deadline)?;
    let remote = handshake
        .remote_certificate()
        .cloned()
        .ok_or(TransportError::HandshakeFailed)?;
    if remote.node_id() != expected_node_id {
        audit_error(
            &registry,
            remote.node_id(),
            None,
            &TransportError::IdentityMismatch,
        )?;
        return Err(TransportError::IdentityMismatch.into());
    }
    let peer = registry.transport_peer(remote.node_id(), &hex::encode(remote.identity_key()))?;
    if let Err(error) = authorize_peer(
        &remote,
        peer.as_ref().map(peer_authorization),
        unix_seconds(),
    ) {
        audit_error(&registry, remote.node_id(), None, &error)?;
        return Err(error.into());
    }
    let mut session = handshake.into_session()?;
    let mut nonce = [0u8; 16];
    entropy::fill_bytes(&mut nonce);
    let probe = sign_probe(&identity, session.session_id(), nonce, unix_seconds())?;
    write_bytes(
        &mut stream,
        &session.write(ENVELOPE_KIND, &probe.encoded())?,
        deadline,
    )?;
    let response = session.read(&read_frame(&mut stream, deadline)?)?;
    if let Some(stated) = crate::direct_transport::stated_error(&response) {
        return Err(stated.into());
    }
    if response.kind != ENVELOPE_KIND {
        return Err(TransportError::InvalidFrame.into());
    }
    verify_envelope(
        &response.body,
        remote.node_id(),
        remote.identity_key(),
        "ack",
        session.session_id(),
        &nonce,
    )?;
    registry.record_transport_audit(crate::node_registry::TransportAudit {
        event_type: "probe_accepted",
        node_id: remote.node_id(),
        session_id: Some(session.session_id()),
        direction: Some(0),
        byte_count: response.body.len(),
        outcome: "accepted",
        error_code: None,
        cue: None,
    })?;
    Ok(())
}

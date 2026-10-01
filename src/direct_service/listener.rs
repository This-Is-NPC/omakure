use super::admission::{AdmissionController, AdmissionReservation, AdmissionState};
use super::connection::{ConnectionDirection, ConnectionState};
use super::enrollment::serve_enrollment_request;
use super::error::DirectServiceError;
use super::session::{drain_until_hangup, hold_session, peer_authorization, SessionInputs};
use super::stream::{read_frame, set_stream_timeouts, write_bytes};
use super::{DIRECT_QUEUE_CAPACITY, DIRECT_WORKERS, HANDSHAKE_TIMEOUT, UNKNOWN_NODE_ID};
use crate::direct_health::HealthSession;
use crate::direct_transport::{
    authorize_peer, envelope_nonce, sign_ack, unix_seconds, verify_envelope, HandshakeRole,
    TransportError, ENVELOPE_KIND,
};
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::{NodeRegistry, PeerState};
use crate::node_transport::LocalTransport;
use crate::util::hex;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub struct DirectListener {
    stop: Arc<AtomicBool>,
    sender: Option<SyncSender<QueuedConnection>>,
    handles: Vec<JoinHandle<()>>,
    state: Arc<ConnectionState>,
}

struct QueuedConnection {
    stream: TcpStream,
    peer_addr: SocketAddr,
    reservation: AdmissionReservation,
    deadline: Instant,
}

impl DirectListener {
    pub fn start(bind: SocketAddr, context: NodeContext) -> Result<Self, DirectServiceError> {
        let identity = NodeIdentity::load_existing(&context)?;
        let stop = Arc::new(AtomicBool::new(false));
        let admission = Arc::new(AdmissionController {
            state: Mutex::new(AdmissionState::default()),
        });
        let state = ConnectionState::new(
            context.clone(),
            &identity,
            &[],
            Arc::clone(&stop),
            true,
            admission,
            None,
            None,
        );
        Self::start_with_state_and_stop(bind, context, state, stop)
    }

    pub(super) fn start_with_state(
        bind: SocketAddr,
        context: NodeContext,
        state: Arc<ConnectionState>,
    ) -> Result<Self, DirectServiceError> {
        Self::start_with_state_and_stop(bind, context, Arc::clone(&state), Arc::clone(&state.stop))
    }

    fn start_with_state_and_stop(
        bind: SocketAddr,
        context: NodeContext,
        state: Arc<ConnectionState>,
        stop: Arc<AtomicBool>,
    ) -> Result<Self, DirectServiceError> {
        let listener = TcpListener::bind(bind)?;
        listener.set_nonblocking(true)?;
        let (sender, receiver) = sync_channel(DIRECT_QUEUE_CAPACITY);
        let receiver = Arc::new(Mutex::new(receiver));
        let mut handles = Vec::with_capacity(DIRECT_WORKERS + 1);
        for _ in 0..DIRECT_WORKERS {
            let stop_for_worker = Arc::clone(&stop);
            let receiver = Arc::clone(&receiver);
            let context = context.clone();
            let state = Arc::clone(&state);
            handles.push(thread::spawn(move || {
                worker_loop(stop_for_worker, receiver, context, state)
            }));
        }
        let stop_for_thread = Arc::clone(&stop);
        let sender_for_thread = sender.clone();
        let state_for_accept = Arc::clone(&state);
        let handle = thread::spawn(move || {
            while !stop_for_thread.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, peer_addr)) => {
                        // The listener is nonblocking only to make the accept loop
                        // stoppable. Workers use blocking streams with explicit
                        // read/write deadlines.
                        if stream.set_nonblocking(false).is_err() {
                            drop(stream);
                            continue;
                        }
                        let Some(reservation) = state_for_accept
                            .admission
                            .reserve(peer_addr.ip(), Instant::now())
                        else {
                            drop(stream);
                            continue;
                        };
                        let connection = QueuedConnection {
                            stream,
                            peer_addr,
                            reservation,
                            deadline: Instant::now() + HANDSHAKE_TIMEOUT,
                        };
                        match sender_for_thread.try_send(connection) {
                            Ok(()) => {}
                            Err(TrySendError::Full(connection)) => drop(connection),
                            Err(TrySendError::Disconnected(connection)) => {
                                drop(connection);
                                return;
                            }
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(25));
                    }
                    Err(_) => return,
                }
            }
        });
        handles.push(handle);
        Ok(Self {
            stop,
            sender: Some(sender),
            handles,
            state,
        })
    }
}

impl Drop for DirectListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.state.close_active();
        self.sender.take();
        for handle in self.handles.drain(..).rev() {
            let _ = handle.join();
        }
    }
}

fn worker_loop(
    stop: Arc<AtomicBool>,
    receiver: Arc<Mutex<Receiver<QueuedConnection>>>,
    context: NodeContext,
    state: Arc<ConnectionState>,
) {
    while !stop.load(Ordering::SeqCst) {
        let connection = match receiver
            .lock()
            .ok()
            .and_then(|receiver| receiver.recv().ok())
        {
            Some(stream) => stream,
            None => return,
        };
        let _ = serve_connection(connection, &context, &state);
    }
}

fn serve_connection(
    connection: QueuedConnection,
    context: &NodeContext,
    state: &Arc<ConnectionState>,
) -> Result<(), DirectServiceError> {
    let QueuedConnection {
        mut stream,
        peer_addr: _peer_addr,
        mut reservation,
        deadline,
    } = connection;
    set_stream_timeouts(&stream, deadline).map_err(|_| io::Error::from(io::ErrorKind::Other))?;
    let identity = NodeIdentity::load_existing(context)?;
    let local = LocalTransport::load_existing(context, &identity)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())?;
    let mut handshake = local.handshake(HandshakeRole::Responder)?;
    let mut remote_node_id = None;
    let mut rejection_audit_recorded = false;
    let result = (|| {
        let frame = read_frame(&mut stream, deadline)?;
        handshake.read_next(&frame, unix_seconds())?;
        write_bytes(&mut stream, &handshake.write_next()?, deadline)?;
        let frame = read_frame(&mut stream, deadline)?;
        handshake.read_next(&frame, unix_seconds())?;
        let remote = handshake
            .remote_certificate()
            .cloned()
            .ok_or(TransportError::HandshakeFailed)?;
        remote_node_id = Some(remote.node_id().to_string());
        let peer =
            registry.transport_peer(remote.node_id(), &hex::encode(remote.identity_key()))?;
        // A revoked identity is not an enrollment candidate.
        //
        // `authenticated_untrusted` is the enrollment-only channel for a peer
        // whose certificate is valid but whose node this registry does not know
        // yet. A revoked one is not that: revocation rows are append-only, the
        // schema's own triggers refuse to resurrect a revoked identity, and the
        // contract says reconnects using any revoked material fail with
        // `revoked`. Sending it down the staging path made the refusal come out
        // as `identity_mismatch` -- it had presented a probe, and the staging
        // path expects a manual request -- so the recorded reason for turning
        // away a machine the operator revoked was that its identity did not
        // match. It does match. That is the whole point.
        //
        // The refusal is stated to the peer, because it has authenticated as
        // exactly the node that was revoked and nothing else can tell it. A
        // dialer that is not told keeps the code it can infer, `internal`,
        // which is the one code the contract says to retry.
        if peer
            .as_ref()
            .is_some_and(|peer| peer.state == PeerState::Revoked)
        {
            // This peer authenticated successfully, so persist the specific
            // revoked refusal before telling it to stop. The outer rejection
            // audit below handles failures before authentication; recording
            // here avoids turning one revoked handshake into two rows.
            registry.record_transport_audit(
                "probe_rejected",
                remote.node_id(),
                None,
                Some(1),
                0,
                "rejected",
                Some(TransportError::Revoked.code() as u16),
            )?;
            rejection_audit_recorded = true;
            let mut session = handshake.into_session()?;
            if let Ok(frame) =
                session.write_error(crate::direct_transport::ProtocolErrorCode::Revoked)
            {
                let _ = write_bytes(&mut stream, &frame, deadline);
                let _ = stream.shutdown(std::net::Shutdown::Write);
                drain_until_hangup(&mut stream);
            }
            return Err(DirectServiceError::Protocol(TransportError::Revoked));
        }
        if peer
            .as_ref()
            .is_none_or(|peer| peer.state != PeerState::Active)
        {
            state
                .admission
                .migrate_node(&mut reservation, remote.node_id())?;
            let mut session = handshake.into_session()?;
            return serve_enrollment_request(
                &mut stream,
                &mut session,
                &identity,
                &registry,
                &remote,
                context,
                deadline,
            );
        }
        authorize_peer(
            &remote,
            peer.as_ref().map(peer_authorization),
            unix_seconds(),
        )?;
        state
            .admission
            .migrate_node(&mut reservation, remote.node_id())?;
        let mut session = handshake.into_session()?;
        let frame = read_frame(&mut stream, deadline)?;
        let request = session.read(&frame)?;
        if request.kind != ENVELOPE_KIND {
            return Err(DirectServiceError::Protocol(TransportError::InvalidFrame));
        }
        let nonce = envelope_nonce(&request.body)?;
        verify_envelope(
            &request.body,
            remote.node_id(),
            remote.identity_key(),
            "probe",
            session.session_id(),
            &nonce,
        )?;
        let session_id = *session.session_id();
        reservation.promote_session()?;
        let _claim = state.register(
            remote.node_id(),
            ConnectionDirection::Responder,
            session_id,
            &stream,
        )?;
        let ack = sign_ack(&identity, session.session_id(), nonce, unix_seconds())?;
        let encoded_ack = ack.encoded();
        registry.record_transport_audit(
            "probe_accepted",
            remote.node_id(),
            Some(session.session_id()),
            Some(1),
            request.body.len() + ack.encoded().len(),
            "accepted",
            None,
        )?;
        write_bytes(
            &mut stream,
            &session.write(ENVELOPE_KIND, &encoded_ack)?,
            deadline,
        )?;
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
        )?;
        Ok(())
    })();
    let rejection_audit = if let Err(error) = &result {
        let node_id = remote_node_id.as_deref().unwrap_or(UNKNOWN_NODE_ID);
        state.record_direct_error(node_id, error);
        if rejection_audit_recorded {
            None
        } else {
            let protocol = match error {
                DirectServiceError::Protocol(error) => Some(error.code() as u16),
                _ => None,
            };
            Some(registry.record_transport_audit(
                "probe_rejected",
                node_id,
                None,
                Some(1),
                0,
                "rejected",
                protocol,
            ))
        }
    } else {
        None
    };
    match rejection_audit {
        Some(Err(error)) => Err(DirectServiceError::Registry(error)),
        Some(Ok(())) | None => result,
    }
}

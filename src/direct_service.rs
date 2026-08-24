//! Socket adapter for the direct transport core.

use crate::direct_transport::{
    authorize_peer, envelope_nonce, sign_ack, sign_probe, unix_seconds, verify_envelope, Frame,
    HandshakeRole, TransportError, ENVELOPE_KIND,
};
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::{NodeRegistry, RegistryError, TransportPeer};
use crate::node_transport::{LocalTransport, NodeTransportError};
use rand::rngs::OsRng;
use rand::RngCore;
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;

const HEADER_TIMEOUT: Duration = Duration::from_secs(2);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const DIRECT_WORKERS: usize = 4;
const DIRECT_QUEUE_CAPACITY: usize = 64;
const DIRECT_MAX_HANDSHAKES: usize = DIRECT_WORKERS;
const DIRECT_MAX_SESSIONS: usize = DIRECT_WORKERS;
const DIRECT_MAX_BYTES: usize = DIRECT_MAX_SESSIONS * crate::direct_transport::MAX_FRAME_BYTES;
const DIRECT_MAX_SOURCE_HANDSHAKES: usize = 2;
const DIRECT_MAX_SOURCE_SESSIONS: usize = 2;
const DIRECT_MAX_SOURCE_BYTES: usize =
    DIRECT_MAX_SOURCE_SESSIONS * crate::direct_transport::MAX_FRAME_BYTES;
const DIRECT_RATE_LIMIT: usize = 8;
const DIRECT_RATE_WINDOW: Duration = Duration::from_secs(1);
const DIRECT_MAX_SOURCE_ENTRIES: usize = 256;
const UNKNOWN_NODE_ID: &str =
    "omk1_0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Error)]
pub enum DirectServiceError {
    #[error("direct transport I/O failed")]
    Io(#[from] io::Error),
    #[error("direct transport protocol failed: {0}")]
    Protocol(#[from] TransportError),
    #[error("direct transport registry failed: {0}")]
    Registry(#[from] RegistryError),
    #[error("direct transport state failed: {0}")]
    State(#[from] NodeTransportError),
    #[error("direct transport node identity failed: {0}")]
    Identity(#[from] crate::node_identity::NodeIdentityError),
}

pub struct DirectListener {
    stop: Arc<AtomicBool>,
    sender: Option<SyncSender<QueuedConnection>>,
    handles: Vec<JoinHandle<()>>,
}

struct QueuedConnection {
    stream: TcpStream,
    peer_addr: SocketAddr,
    reservation: AdmissionReservation,
}

#[derive(Default)]
struct AdmissionState {
    handshakes: usize,
    sessions: usize,
    bytes: usize,
    sources: HashMap<IpAddr, SourceAdmission>,
}

#[derive(Default)]
struct SourceAdmission {
    handshakes: usize,
    sessions: usize,
    bytes: usize,
    attempts: VecDeque<Instant>,
}

struct Admission {
    state: Mutex<AdmissionState>,
}

struct AdmissionReservation {
    admission: Arc<Admission>,
    source: IpAddr,
    phase: AdmissionPhase,
    bytes: usize,
}

#[derive(Clone, Copy)]
enum AdmissionPhase {
    Handshake,
    Session,
}

impl Admission {
    fn reserve(self: &Arc<Self>, source: IpAddr, now: Instant) -> Option<AdmissionReservation> {
        let mut state = self.state.lock().ok()?;
        prune_sources(&mut state, now);
        if !state.sources.contains_key(&source) {
            if state.sources.len() >= DIRECT_MAX_SOURCE_ENTRIES {
                return None;
            }
            state.sources.insert(source, SourceAdmission::default());
        }
        let (source_handshakes, source_bytes, source_attempts) = {
            let source_state = state.sources.get_mut(&source)?;
            while source_state
                .attempts
                .front()
                .is_some_and(|attempt| now.duration_since(*attempt) >= DIRECT_RATE_WINDOW)
            {
                source_state.attempts.pop_front();
            }
            (
                source_state.handshakes,
                source_state.bytes,
                source_state.attempts.len(),
            )
        };
        let source_allowed = state.handshakes < DIRECT_MAX_HANDSHAKES
            && state
                .bytes
                .saturating_add(crate::direct_transport::MAX_FRAME_BYTES)
                <= DIRECT_MAX_BYTES
            && source_handshakes < DIRECT_MAX_SOURCE_HANDSHAKES
            && source_bytes.saturating_add(crate::direct_transport::MAX_FRAME_BYTES)
                <= DIRECT_MAX_SOURCE_BYTES
            && source_attempts < DIRECT_RATE_LIMIT;
        if !source_allowed {
            return None;
        }
        state.handshakes += 1;
        state.bytes += crate::direct_transport::MAX_FRAME_BYTES;
        let source_state = state.sources.get_mut(&source)?;
        source_state.attempts.push_back(now);
        source_state.handshakes += 1;
        source_state.bytes += crate::direct_transport::MAX_FRAME_BYTES;
        Some(AdmissionReservation {
            admission: Arc::clone(self),
            source,
            phase: AdmissionPhase::Handshake,
            bytes: crate::direct_transport::MAX_FRAME_BYTES,
        })
    }
}

fn prune_sources(state: &mut AdmissionState, now: Instant) {
    for source in state.sources.values_mut() {
        while source
            .attempts
            .front()
            .is_some_and(|attempt| now.duration_since(*attempt) >= DIRECT_RATE_WINDOW)
        {
            source.attempts.pop_front();
        }
    }
    state.sources.retain(|_, source| {
        source.handshakes != 0
            || source.sessions != 0
            || source.bytes != 0
            || !source.attempts.is_empty()
    });
}

impl AdmissionReservation {
    fn promote_session(&mut self) -> Result<(), TransportError> {
        let mut state = self
            .admission
            .state
            .lock()
            .map_err(|_| TransportError::Internal)?;
        let source_sessions = state
            .sources
            .get(&self.source)
            .ok_or(TransportError::Internal)?
            .sessions;
        if state.sessions >= DIRECT_MAX_SESSIONS || source_sessions >= DIRECT_MAX_SOURCE_SESSIONS {
            return Err(TransportError::RateLimited);
        }
        state.handshakes = state.handshakes.saturating_sub(1);
        state.sessions += 1;
        let source_state = state
            .sources
            .get_mut(&self.source)
            .ok_or(TransportError::Internal)?;
        source_state.handshakes = source_state.handshakes.saturating_sub(1);
        source_state.sessions += 1;
        self.phase = AdmissionPhase::Session;
        Ok(())
    }
}

impl Drop for AdmissionReservation {
    fn drop(&mut self) {
        let Ok(mut state) = self.admission.state.lock() else {
            return;
        };
        if !state.sources.contains_key(&self.source) {
            return;
        }
        match self.phase {
            AdmissionPhase::Handshake => {
                state.handshakes = state.handshakes.saturating_sub(1);
            }
            AdmissionPhase::Session => {
                state.sessions = state.sessions.saturating_sub(1);
            }
        }
        state.bytes = state.bytes.saturating_sub(self.bytes);
        let source_state = state
            .sources
            .get_mut(&self.source)
            .expect("source admission exists");
        match self.phase {
            AdmissionPhase::Handshake => {
                source_state.handshakes = source_state.handshakes.saturating_sub(1);
            }
            AdmissionPhase::Session => {
                source_state.sessions = source_state.sessions.saturating_sub(1);
            }
        }
        source_state.bytes = source_state.bytes.saturating_sub(self.bytes);
        if source_state.handshakes == 0
            && source_state.sessions == 0
            && source_state.bytes == 0
            && source_state.attempts.is_empty()
        {
            state.sources.remove(&self.source);
        }
    }
}

impl DirectListener {
    pub fn start(bind: SocketAddr, context: NodeContext) -> Result<Self, DirectServiceError> {
        let listener = TcpListener::bind(bind)?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = sync_channel(DIRECT_QUEUE_CAPACITY);
        let admission = Arc::new(Admission {
            state: Mutex::new(AdmissionState::default()),
        });
        let receiver = Arc::new(Mutex::new(receiver));
        let mut handles = Vec::with_capacity(DIRECT_WORKERS + 1);
        for _ in 0..DIRECT_WORKERS {
            let stop_for_worker = Arc::clone(&stop);
            let receiver = Arc::clone(&receiver);
            let context = context.clone();
            handles.push(thread::spawn(move || {
                worker_loop(stop_for_worker, receiver, context)
            }));
        }
        let stop_for_thread = Arc::clone(&stop);
        let sender_for_thread = sender.clone();
        let admission_for_thread = Arc::clone(&admission);
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
                        let Some(reservation) =
                            admission_for_thread.reserve(peer_addr.ip(), Instant::now())
                        else {
                            drop(stream);
                            continue;
                        };
                        let connection = QueuedConnection {
                            stream,
                            peer_addr,
                            reservation,
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
        })
    }
}

impl Drop for DirectListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
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
        let _ = serve_connection(connection, &context);
    }
}

pub fn probe(
    endpoint: SocketAddr,
    expected_node_id: &str,
    context: &NodeContext,
) -> Result<(), DirectServiceError> {
    let identity = NodeIdentity::load_existing(context)?;
    let local = LocalTransport::load_existing(context, &identity)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())?;
    let mut stream = TcpStream::connect_timeout(&endpoint, HANDSHAKE_TIMEOUT)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let mut handshake = local.handshake(HandshakeRole::Initiator)?;
    write_bytes(&mut stream, &handshake.write_next()?)?;
    handshake.read_next(&read_frame(&mut stream)?, unix_seconds())?;
    write_bytes(&mut stream, &handshake.write_next()?)?;
    let remote = handshake
        .remote_certificate()
        .cloned()
        .ok_or(TransportError::HandshakeFailed)?;
    if remote.node_id() != expected_node_id {
        audit_error(
            &registry,
            remote.node_id(),
            None,
            TransportError::IdentityMismatch,
        )?;
        return Err(TransportError::IdentityMismatch.into());
    }
    let peer = registry.transport_peer(remote.node_id(), &hex(remote.identity_key()))?;
    authorize_peer(
        &remote,
        peer.as_ref().map(peer_authorization),
        unix_seconds(),
    )?;
    let mut session = handshake.into_session()?;
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let probe = sign_probe(&identity, session.session_id(), nonce, unix_seconds())?;
    write_bytes(
        &mut stream,
        &session.write(ENVELOPE_KIND, &probe.encoded())?,
    )?;
    let response = session.read(&read_frame(&mut stream)?)?;
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
    registry.record_transport_audit(
        "probe_accepted",
        remote.node_id(),
        Some(session.session_id()),
        Some(0),
        response.body.len(),
        "accepted",
        None,
    )?;
    Ok(())
}

fn serve_connection(
    connection: QueuedConnection,
    context: &NodeContext,
) -> Result<(), DirectServiceError> {
    let QueuedConnection {
        mut stream,
        peer_addr: _peer_addr,
        mut reservation,
    } = connection;
    stream.set_read_timeout(Some(HEADER_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let identity = NodeIdentity::load_existing(context)?;
    let local = LocalTransport::load_existing(context, &identity)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())?;
    let mut handshake = local.handshake(HandshakeRole::Responder)?;
    let mut remote_node_id = None;
    let result = (|| {
        handshake.read_next(&read_frame(&mut stream)?, unix_seconds())?;
        write_bytes(&mut stream, &handshake.write_next()?)?;
        handshake.read_next(&read_frame(&mut stream)?, unix_seconds())?;
        let remote = handshake
            .remote_certificate()
            .cloned()
            .ok_or(TransportError::HandshakeFailed)?;
        remote_node_id = Some(remote.node_id().to_string());
        let peer = registry.transport_peer(remote.node_id(), &hex(remote.identity_key()))?;
        authorize_peer(
            &remote,
            peer.as_ref().map(peer_authorization),
            unix_seconds(),
        )?;
        reservation.promote_session()?;
        let mut session = handshake.into_session()?;
        let request = session.read(&read_frame(&mut stream)?)?;
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
        write_bytes(&mut stream, &session.write(ENVELOPE_KIND, &encoded_ack)?)?;
        Ok(())
    })();
    let rejection_audit = if let Err(error) = &result {
        let node_id = remote_node_id.as_deref().unwrap_or(UNKNOWN_NODE_ID);
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
    } else {
        None
    };
    match rejection_audit {
        Some(Err(error)) => Err(DirectServiceError::Registry(error)),
        Some(Ok(())) | None => result,
    }
}

fn peer_authorization(
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

fn audit_error(
    registry: &NodeRegistry,
    node_id: &str,
    session_id: Option<&[u8; 32]>,
    error: TransportError,
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

fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>, DirectServiceError> {
    let mut prefix = [0u8; 4];
    stream.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if !(4..=crate::direct_transport::MAX_FRAME_LENGTH).contains(&length) {
        return Err(TransportError::MessageTooLarge.into());
    }
    let mut frame = Vec::with_capacity(length + 4);
    frame.extend_from_slice(&prefix);
    frame.resize(length + 4, 0);
    stream.read_exact(&mut frame[4..])?;
    Frame::parse(&frame)?;
    Ok(frame)
}

fn write_bytes(stream: &mut TcpStream, bytes: &[u8]) -> Result<(), DirectServiceError> {
    stream.write_all(bytes)?;
    stream.flush()?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_limits_each_source_without_starving_another_source() {
        let admission = Arc::new(Admission {
            state: Mutex::new(AdmissionState::default()),
        });
        let first = "127.0.0.1:10001".parse::<SocketAddr>().unwrap();
        let second = "127.0.0.2:10001".parse::<SocketAddr>().unwrap();
        let first_one = admission.reserve(first.ip(), Instant::now()).unwrap();
        let first_two = admission.reserve(first.ip(), Instant::now()).unwrap();
        assert!(admission.reserve(first.ip(), Instant::now()).is_none());
        let second_one = admission.reserve(second.ip(), Instant::now()).unwrap();
        drop(first_one);
        assert!(admission.reserve(first.ip(), Instant::now()).is_some());
        drop(first_two);
        drop(second_one);
    }

    #[test]
    fn admission_reservation_releases_handshake_and_byte_capacity() {
        let admission = Arc::new(Admission {
            state: Mutex::new(AdmissionState::default()),
        });
        let mut reservations = Vec::new();
        for octet in 1..=DIRECT_MAX_HANDSHAKES {
            let source = format!("127.0.0.{octet}:10001")
                .parse::<SocketAddr>()
                .unwrap()
                .ip();
            reservations.push(admission.reserve(source, Instant::now()).unwrap());
        }
        assert!(admission
            .reserve("127.0.0.250".parse::<IpAddr>().unwrap(), Instant::now())
            .is_none());
        drop(reservations);
        assert!(admission
            .reserve("127.0.0.250".parse::<IpAddr>().unwrap(), Instant::now())
            .is_some());
    }

    #[test]
    fn admission_prunes_stale_sources_and_bounds_unique_source_churn() {
        let admission = Arc::new(Admission {
            state: Mutex::new(AdmissionState::default()),
        });
        let start = Instant::now();
        for value in 0..(DIRECT_MAX_SOURCE_ENTRIES * 2) {
            let source = IpAddr::V6(std::net::Ipv6Addr::from(value as u128));
            let _ = admission.reserve(source, start);
        }
        let state = admission.state.lock().unwrap();
        assert_eq!(state.sources.len(), DIRECT_MAX_SOURCE_ENTRIES);
        drop(state);

        let active_source = IpAddr::V6(std::net::Ipv6Addr::from(999_000u128));
        let active = admission.reserve(active_source, start + DIRECT_RATE_WINDOW);
        assert!(active.is_some());
        let state = admission.state.lock().unwrap();
        assert!(state.sources.contains_key(&active_source));
        assert!(state.sources.len() <= DIRECT_MAX_SOURCE_ENTRIES);
        drop(state);
        drop(active);

        let later_source = IpAddr::V6(std::net::Ipv6Addr::from(999_001u128));
        assert!(admission
            .reserve(later_source, start + DIRECT_RATE_WINDOW)
            .is_some());
        let state = admission.state.lock().unwrap();
        assert!(state.sources.len() <= DIRECT_MAX_SOURCE_ENTRIES);
        assert!(state.sources.contains_key(&active_source));
    }
}

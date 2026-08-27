//! Socket adapter for the direct transport core.

use crate::direct_health::{HealthOutcome, HealthSession};
use crate::direct_transport::{
    authorize_peer, enrollment_ack_accepted, enrollment_ack_offer, enrollment_request_bytes,
    envelope_nonce, sign_ack, sign_manual_ack, sign_manual_request, sign_probe, unix_seconds,
    verify_envelope, Frame, HandshakeRole, TransportError, TransportSession, ENVELOPE_KIND,
};
use crate::enrollment::{EnrollmentRole, ManualEnrollmentRequest};
use crate::health_plane::report::HealthReporter;
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::{NodeRegistry, PeerState, RegistryError, TransportPeer};
use crate::node_transport::{LocalTransport, NodeTransportError};
use crate::remote_cue::{CueCode, CueOutcome};
use hickory_resolver::config::ResolverConfig;
use hickory_resolver::name_server::TokioConnectionProvider;
use hickory_resolver::TokioResolver;
use rand::rngs::OsRng;
use rand::RngCore;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio::task::JoinSet;

pub const HEADER_TIMEOUT: Duration = Duration::from_secs(2);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const DIRECT_WORKERS: usize = 4;
const DIRECT_QUEUE_CAPACITY: usize = 64;
/// The opening dial-retry delays.
///
/// Past the last one the delay keeps doubling until it reaches
/// `RETRY_BACKOFF_CEILING`; it never runs out.
pub const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];
/// The longest a static-peer dialer waits between attempts.
///
/// A peer that comes back has to be redialed while the fleet still counts it
/// Online, so one whole delay plus its jitter plus `CONNECT_TIMEOUT` plus
/// `HANDSHAKE_TIMEOUT` has to fit inside `PRESENCE_ONLINE_SECONDS`;
/// `retry_ceiling_redials_within_the_presence_window` holds that. Sixty
/// seconds is also the lease cadence `runs::HEARTBEAT_MS` already treats as
/// live, and a twentieth of `IDLE_TIMEOUT`, so a peer that stays down is
/// polled rather than hammered and its failures do not drown the status.
pub const RETRY_BACKOFF_CEILING: Duration = Duration::from_secs(60);
pub const RETRY_JITTER_MAX: Duration = Duration::from_millis(250);
const RESOLVER_QUEUE_CAPACITY: usize = 8;
const RESOLVER_CONCURRENCY: usize = 4;
const DIRECT_MAX_HANDSHAKES: usize = 256;
const DIRECT_MAX_SESSIONS: usize = 1024;
const DIRECT_MAX_BYTES: usize = 64 * 1024 * 1024;
const DIRECT_MAX_SOURCE_HANDSHAKES: usize = 4;
const DIRECT_MAX_SOURCE_SESSIONS: usize = 4;
const DIRECT_MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
const DIRECT_MAX_NODE_HANDSHAKES: usize = 4;
const DIRECT_MAX_NODE_SESSIONS: usize = 4;
const DIRECT_MAX_NODE_BYTES: usize = 4 * 1024 * 1024;
const DIRECT_RATE_LIMIT: usize = 4;
const DIRECT_RATE_WINDOW: Duration = Duration::from_secs(60);
const DIRECT_MAX_SOURCE_ENTRIES: usize = 256;
const ADMISSION_BYTES: usize = crate::direct_transport::MAX_PLAINTEXT_BYTES;
const UNKNOWN_NODE_ID: &str =
    "omk1_0000000000000000000000000000000000000000000000000000000000000000";
type ManualEnrollmentReciprocal = Option<(Vec<u8>, Vec<u8>)>;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticPeer {
    pub node_id: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TransportPeerStatus {
    pub node_id: String,
    pub state: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct TransportStatus {
    pub enabled: bool,
    pub listening: bool,
    pub expected_peer_count: usize,
    pub connected_peer_count: usize,
    pub expected_connected_peer_count: usize,
    pub peers: Vec<TransportPeerStatus>,
    pub last_errors: BTreeMap<String, String>,
}

pub type TransportStatusHandle = Arc<Mutex<TransportStatus>>;

pub fn parse_static_peer(value: &str) -> Result<StaticPeer, DirectServiceError> {
    let Some((node_id, endpoint)) = value.split_once('@') else {
        return Err(DirectServiceError::Protocol(TransportError::InvalidFrame));
    };
    if node_id.is_empty() || endpoint.is_empty() || endpoint.contains('@') {
        return Err(DirectServiceError::Protocol(TransportError::InvalidFrame));
    }
    Ok(StaticPeer {
        node_id: node_id.to_string(),
        endpoint: endpoint.to_string(),
    })
}

fn validate_static_peers(peers: &[StaticPeer]) -> Result<(), DirectServiceError> {
    let mut node_ids = HashSet::new();
    let mut endpoints = HashSet::new();
    for peer in peers {
        if !node_ids.insert(&peer.node_id) || !endpoints.insert(&peer.endpoint) {
            return Err(DirectServiceError::Protocol(TransportError::InvalidFrame));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ConnectionDirection {
    Initiator,
    Responder,
}

struct ActiveConnection {
    session_id: [u8; 32],
    stream: TcpStream,
}

struct ConnectionState {
    local_node_id: String,
    expected: HashSet<String>,
    stop: Arc<AtomicBool>,
    active: Mutex<HashMap<String, ActiveConnection>>,
    status: TransportStatusHandle,
    admission: Arc<AdmissionController>,
    /// The Performer-side Health Plane reporter, when this node serves health.
    ///
    /// `None` leaves every session behaving exactly as it did before the
    /// Health Plane existed: application frames are decrypted and discarded.
    reporter: Option<Arc<HealthReporter>>,
    /// Root of the workspace whose scripts a Cue may name, when this node
    /// serves runs.
    ///
    /// Held as a path rather than a `Workspace` so the shared state stays
    /// cheaply shareable across session threads. `None` means an accepted Cue is
    /// decided and audited but never enqueued, which is what a node with no
    /// workspace should do: it has nothing to run.
    workspace_root: Option<std::path::PathBuf>,
    /// Cues waiting for the session that can carry them, by peer node id.
    ///
    /// The cipher state of a live session lives in the thread holding it, so
    /// nothing outside that thread can write to a peer. A caller that wants to
    /// reach a peer this node is already connected to therefore hands the
    /// instruction here and the session thread sends it. This is what makes a
    /// Cue work in a managed fleet: dialling a second time is refused, and
    /// correctly so -- two sessions with one peer would give the Health Plane
    /// two cursors for the same node.
    outbox: Mutex<HashMap<String, Vec<PendingCue>>>,
}

/// A Cue handed to the session thread, with the channel its answer goes back on.
struct PendingCue {
    cue_id: String,
    script: String,
    reason: String,
    expected_run_id: String,
    deadline: Instant,
    reply: std::sync::mpsc::SyncSender<CueDispatchOutcome>,
}

impl ConnectionState {
    fn new(
        identity: &NodeIdentity,
        static_peers: &[StaticPeer],
        stop: Arc<AtomicBool>,
        listening: bool,
        admission: Arc<AdmissionController>,
        reporter: Option<Arc<HealthReporter>>,
        workspace_root: Option<std::path::PathBuf>,
    ) -> Arc<Self> {
        let expected = static_peers
            .iter()
            .map(|peer| peer.node_id.clone())
            .collect::<HashSet<_>>();
        let status = Arc::new(Mutex::new(TransportStatus {
            enabled: true,
            listening,
            expected_peer_count: expected.len(),
            connected_peer_count: 0,
            expected_connected_peer_count: 0,
            peers: expected
                .iter()
                .map(|node_id| TransportPeerStatus {
                    node_id: node_id.clone(),
                    state: "disconnected",
                })
                .collect(),
            last_errors: BTreeMap::new(),
        }));
        refresh_status(&status, &expected, &HashMap::new());
        Arc::new(Self {
            local_node_id: identity.public_status().node_id.clone(),
            expected,
            stop,
            active: Mutex::new(HashMap::new()),
            outbox: Mutex::new(HashMap::new()),
            status,
            admission,
            reporter,
            workspace_root,
        })
    }

    fn status(&self) -> TransportStatusHandle {
        Arc::clone(&self.status)
    }

    fn should_initiate(&self, remote_node_id: &str) -> bool {
        self.local_node_id.as_str() < remote_node_id
    }

    /// Hand a Cue to whichever thread holds the session with this peer.
    ///
    /// Refused when there is no live session: a caller must not be told its
    /// instruction is on its way when nothing can carry it.
    fn enqueue_cue(&self, peer_node_id: &str, pending: PendingCue) -> Result<(), TransportError> {
        let active = self.active.lock().map_err(|_| TransportError::Internal)?;
        if !active.contains_key(peer_node_id) {
            return Err(TransportError::NotEnrolled);
        }
        drop(active);
        self.outbox
            .lock()
            .map_err(|_| TransportError::Internal)?
            .entry(peer_node_id.to_string())
            .or_default()
            .push(pending);
        Ok(())
    }

    /// The next Cue this session should carry, if any.
    fn take_pending_cue(&self, peer_node_id: &str) -> Option<PendingCue> {
        let mut outbox = self.outbox.lock().ok()?;
        let queue = outbox.get_mut(peer_node_id)?;
        if queue.is_empty() {
            return None;
        }
        Some(queue.remove(0))
    }

    /// Fail every Cue still waiting on a session that just ended.
    ///
    /// Dropping the reply channel is what unblocks the caller; without this a
    /// request would wait out its whole budget for a session that is gone.
    fn drain_outbox(&self, peer_node_id: &str) {
        if let Ok(mut outbox) = self.outbox.lock() {
            outbox.remove(peer_node_id);
        }
    }

    fn register(
        self: &Arc<Self>,
        remote_node_id: &str,
        direction: ConnectionDirection,
        session_id: [u8; 32],
        stream: &TcpStream,
    ) -> Result<ConnectionClaim, TransportError> {
        if direction == ConnectionDirection::Initiator && !self.expected.contains(remote_node_id) {
            return Err(TransportError::NotEnrolled);
        }
        if self.expected.contains(remote_node_id) {
            if direction == ConnectionDirection::Initiator && !self.should_initiate(remote_node_id)
            {
                return Err(TransportError::RateLimited);
            }
            if direction == ConnectionDirection::Responder && self.should_initiate(remote_node_id) {
                return Err(TransportError::RateLimited);
            }
        }
        let mut active = self.active.lock().map_err(|_| TransportError::Internal)?;
        if active.contains_key(remote_node_id) {
            // Never replace a live session. Deterministic dial ownership keeps
            // normal peers from racing; a concurrent loser must close itself.
            return Err(TransportError::RateLimited);
        }
        let tracked = stream.try_clone().map_err(|_| TransportError::Internal)?;
        active.insert(
            remote_node_id.to_string(),
            ActiveConnection {
                session_id,
                stream: tracked,
            },
        );
        refresh_status(&self.status, &self.expected, &active);
        self.clear_error(remote_node_id);
        Ok(ConnectionClaim {
            state: Arc::clone(self),
            remote_node_id: remote_node_id.to_string(),
            session_id,
        })
    }

    fn unregister(&self, remote_node_id: &str, session_id: [u8; 32]) {
        let Ok(mut active) = self.active.lock() else {
            return;
        };
        if active
            .get(remote_node_id)
            .is_some_and(|connection| connection.session_id == session_id)
        {
            active.remove(remote_node_id);
            refresh_status(&self.status, &self.expected, &active);
        }
    }

    fn record_error(&self, node_id: &str, error: &TransportError) {
        if let Ok(mut status) = self.status.lock() {
            status
                .last_errors
                .insert(node_id.to_string(), error.code().as_str().to_string());
        }
    }

    /// Drop a peer's recorded failure now that it holds a session again.
    ///
    /// `last_errors` is read to answer "why is this peer not connected", so a
    /// connected peer must not answer it. Keeping the entry made the map name
    /// the last thing that ever went wrong rather than the current cause.
    fn clear_error(&self, node_id: &str) {
        if let Ok(mut status) = self.status.lock() {
            status.last_errors.remove(node_id);
        }
    }

    fn close_active(&self) {
        let Ok(active) = self.active.lock() else {
            return;
        };
        for connection in active.values() {
            let _ = connection.stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

struct ConnectionClaim {
    state: Arc<ConnectionState>,
    remote_node_id: String,
    session_id: [u8; 32],
}

impl Drop for ConnectionClaim {
    fn drop(&mut self) {
        self.state.unregister(&self.remote_node_id, self.session_id);
    }
}

fn refresh_status(
    status: &TransportStatusHandle,
    expected: &HashSet<String>,
    active: &HashMap<String, ActiveConnection>,
) {
    let Ok(mut status) = status.lock() else {
        return;
    };
    let mut node_ids = expected.iter().cloned().collect::<Vec<_>>();
    node_ids.extend(active.keys().filter(|id| !expected.contains(*id)).cloned());
    node_ids.sort();
    node_ids.dedup();
    status.connected_peer_count = active.len();
    status.expected_connected_peer_count = expected
        .iter()
        .filter(|node_id| active.contains_key(*node_id))
        .count();
    status.peers = node_ids
        .into_iter()
        .map(|node_id| TransportPeerStatus {
            state: if active.contains_key(&node_id) {
                "connected"
            } else {
                "disconnected"
            },
            node_id,
        })
        .collect();
}

pub struct DirectService {
    stop: Arc<AtomicBool>,
    listener: Option<DirectListener>,
    dialer_handles: Vec<JoinHandle<()>>,
    resolver: Option<Arc<Resolver>>,
    state: Arc<ConnectionState>,
}

impl DirectService {
    /// A handle for sending Cues over the sessions this service already holds.
    pub fn cue_dispatcher(&self) -> CueDispatcher {
        CueDispatcher {
            state: Arc::clone(&self.state),
        }
    }

    pub fn start(
        bind: Option<SocketAddr>,
        static_peer_values: &[String],
        context: NodeContext,
        reporter: Option<Arc<HealthReporter>>,
        workspace_root: Option<std::path::PathBuf>,
    ) -> Result<Self, DirectServiceError> {
        let static_peers = static_peer_values
            .iter()
            .map(|value| parse_static_peer(value))
            .collect::<Result<Vec<_>, _>>()?;
        if bind.is_none() && static_peers.is_empty() {
            return Err(DirectServiceError::Protocol(TransportError::Internal));
        }
        validate_static_peers(&static_peers)?;
        let identity = NodeIdentity::load_existing(&context)?;
        let stop = Arc::new(AtomicBool::new(false));
        let admission = Arc::new(AdmissionController {
            state: Mutex::new(AdmissionState::default()),
        });
        let state = ConnectionState::new(
            &identity,
            &static_peers,
            Arc::clone(&stop),
            bind.is_some(),
            Arc::clone(&admission),
            reporter,
            workspace_root,
        );
        let listener = bind
            .map(|bind| DirectListener::start_with_state(bind, context.clone(), Arc::clone(&state)))
            .transpose()?;
        let resolver = if static_peers.is_empty() {
            None
        } else {
            Some(Resolver::start().map_err(DirectServiceError::Io)?)
        };
        let mut dialer_handles = Vec::with_capacity(static_peers.len());
        for peer in static_peers {
            let stop_for_dialer = Arc::clone(&stop);
            let state_for_dialer = Arc::clone(&state);
            let context_for_dialer = context.clone();
            let resolver_for_dialer = resolver.as_ref().expect("resolver for static peer").clone();
            dialer_handles.push(thread::spawn(move || {
                dialer_loop(
                    peer,
                    context_for_dialer,
                    state_for_dialer,
                    stop_for_dialer,
                    resolver_for_dialer,
                )
            }));
        }
        Ok(Self {
            stop,
            listener,
            dialer_handles,
            resolver,
            state,
        })
    }

    pub fn status(&self) -> TransportStatusHandle {
        self.state.status()
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(resolver) = self.resolver.as_ref() {
            resolver.cancel();
        }
        self.state.close_active();
        self.listener.take();
        for handle in self.dialer_handles.drain(..) {
            let _ = handle.join();
        }
        if let Some(resolver) = self.resolver.take() {
            resolver.shutdown();
        }
    }
}

impl Drop for DirectService {
    fn drop(&mut self) {
        self.stop();
    }
}

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
}

#[derive(Debug)]
struct ResolveRequest {
    host: String,
    port: u16,
    deadline: Instant,
    response: SyncSender<io::Result<Vec<SocketAddr>>>,
}

struct Resolver {
    sender: mpsc::Sender<ResolveRequest>,
    stop: Arc<AtomicBool>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Resolver {
    fn start() -> io::Result<Arc<Self>> {
        Self::start_with_config(None)
    }

    fn start_with_config(config: Option<ResolverConfig>) -> io::Result<Arc<Self>> {
        let (sender, receiver) = mpsc::channel(RESOLVER_QUEUE_CAPACITY);
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        let (ready_sender, ready_receiver) = sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_worker = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("omakure-direct-dns".to_string())
            .spawn(move || {
                ACTIVE_RESOLVER_WORKERS.fetch_add(1, Ordering::SeqCst);
                let _worker_guard = ResolverWorkerGuard;
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_sender.send(Err(io::Error::other(error)));
                        return;
                    }
                };
                let resolver = match build_resolver(config) {
                    Ok(resolver) => resolver,
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                        return;
                    }
                };
                let _ = ready_sender.send(Ok(()));
                runtime.block_on(run_resolver(
                    receiver,
                    shutdown_receiver,
                    resolver,
                    stop_for_worker,
                ));
                // Hickory uses Tokio sockets and tasks only. run_resolver has
                // joined every request task before this owned runtime drops.
            })?;
        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Arc::new(Self {
                sender,
                stop,
                shutdown: Mutex::new(Some(shutdown_sender)),
                handle: Mutex::new(Some(handle)),
            })),
            Ok(Err(error)) => {
                let _ = handle.join();
                Err(error)
            }
            Err(_) => {
                let _ = handle.join();
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "resolver worker exited during startup",
                ))
            }
        }
    }

    fn cancel(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut shutdown) = self.shutdown.lock() {
            if let Some(sender) = shutdown.take() {
                let _ = sender.send(());
            }
        }
    }

    fn shutdown(&self) {
        self.cancel();
        if let Ok(mut handle) = self.handle.lock() {
            if let Some(handle) = handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn resolve(
        &self,
        endpoint: &str,
        deadline: Instant,
        stop: &AtomicBool,
    ) -> Result<Vec<SocketAddr>, TransportError> {
        let (host, port) = split_endpoint(endpoint)?;
        if let Ok(address) = host.parse::<IpAddr>() {
            return Ok(vec![SocketAddr::new(address, port)]);
        }
        if stop.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst) {
            return Err(TransportError::Internal);
        }
        let (response_sender, response_receiver) = sync_channel(1);
        let mut request = ResolveRequest {
            host,
            port,
            deadline,
            response: response_sender,
        };
        loop {
            if stop.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst) {
                return Err(TransportError::Internal);
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(TransportError::Internal)?;
            match self.sender.try_send(request) {
                Ok(()) => break,
                Err(mpsc::error::TrySendError::Full(returned_request)) => {
                    thread::sleep(Duration::from_millis(5).min(remaining));
                    request = returned_request;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return Err(TransportError::Internal),
            }
        }
        loop {
            if stop.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst) {
                return Err(TransportError::Internal);
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(TransportError::Internal)?;
            match response_receiver.recv_timeout(Duration::from_millis(25).min(remaining)) {
                Ok(result) => return result.map_err(|_| TransportError::Internal),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(TransportError::Internal)
                }
            }
        }
    }
}

impl Drop for Resolver {
    fn drop(&mut self) {
        self.shutdown();
    }
}

static ACTIVE_RESOLVER_WORKERS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static ACTIVE_RESOLVER_TASKS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

struct ResolverWorkerGuard;

impl Drop for ResolverWorkerGuard {
    fn drop(&mut self) {
        ACTIVE_RESOLVER_WORKERS.fetch_sub(1, Ordering::SeqCst);
    }
}

struct ResolverTaskGuard;

impl Drop for ResolverTaskGuard {
    fn drop(&mut self) {
        ACTIVE_RESOLVER_TASKS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn build_resolver(config: Option<ResolverConfig>) -> io::Result<TokioResolver> {
    let builder = match config {
        Some(config) => {
            TokioResolver::builder_with_config(config, TokioConnectionProvider::default())
        }
        None => {
            #[cfg(any(unix, target_os = "windows"))]
            {
                TokioResolver::builder_tokio()
                    .map_err(|error| io::Error::other(error.to_string()))?
            }
            #[cfg(not(any(unix, target_os = "windows")))]
            {
                TokioResolver::builder_with_config(
                    ResolverConfig::default(),
                    TokioConnectionProvider::default(),
                )
            }
        }
    };
    Ok(builder.build())
}

async fn run_resolver(
    mut receiver: mpsc::Receiver<ResolveRequest>,
    mut shutdown: oneshot::Receiver<()>,
    resolver: TokioResolver,
    stop: Arc<AtomicBool>,
) {
    let permits = Arc::new(Semaphore::new(RESOLVER_CONCURRENCY));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
            request = receiver.recv() => {
                let Some(request) = request else { break };
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let remaining = match request.deadline.checked_duration_since(Instant::now()) {
                    Some(remaining) if !remaining.is_zero() => remaining,
                    _ => {
                        let _ = request.response.send(Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "resolver deadline",
                        )));
                        continue;
                    }
                };
                let permit = tokio::select! {
                    _ = &mut shutdown => break,
                    permit = tokio::time::timeout(remaining, Arc::clone(&permits).acquire_owned()) => {
                        match permit {
                            Ok(Ok(permit)) => permit,
                            _ => {
                                let _ = request.response.send(Err(io::Error::new(
                                    io::ErrorKind::TimedOut,
                                    "resolver queue deadline",
                                )));
                                continue;
                            }
                        }
                    }
                };
                let resolver = resolver.clone();
                ACTIVE_RESOLVER_TASKS.fetch_add(1, Ordering::SeqCst);
                tasks.spawn(async move {
                    let _task_guard = ResolverTaskGuard;
                    let host = request.host;
                    let port = request.port;
                    let result = tokio::time::timeout(remaining, async move {
                        let lookup = resolver
                            .lookup_ip(host)
                            .await
                            .map_err(|error| io::Error::other(error.to_string()))?;
                        let mut addresses = Vec::new();
                        for ip in lookup.iter() {
                            let address = SocketAddr::new(ip, port);
                            if !addresses.contains(&address) {
                                addresses.push(address);
                            }
                        }
                        if addresses.is_empty() {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                "resolver returned no addresses",
                            ));
                        }
                        Ok(addresses)
                    })
                    .await
                    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "resolver timeout"))
                    .and_then(|result| result);
                    let _ = request.response.send(result);
                    drop(permit);
                });
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}

fn split_endpoint(endpoint: &str) -> Result<(String, u16), TransportError> {
    if let Ok(address) = endpoint.parse::<SocketAddr>() {
        return Ok((address.ip().to_string(), address.port()));
    }
    if let Some(host_and_port) = endpoint.strip_prefix('[') {
        let Some((host, port)) = host_and_port.split_once("]:") else {
            return Err(TransportError::Internal);
        };
        return port
            .parse::<u16>()
            .map(|port| (host.to_string(), port))
            .map_err(|_| TransportError::Internal);
    }
    let Some((host, port)) = endpoint.rsplit_once(':') else {
        return Err(TransportError::Internal);
    };
    if host.is_empty() {
        return Err(TransportError::Internal);
    }
    port.parse::<u16>()
        .map(|port| (host.to_string(), port))
        .map_err(|_| TransportError::Internal)
}

#[derive(Default)]
struct AdmissionState {
    handshakes: usize,
    sessions: usize,
    bytes: usize,
    sources: HashMap<IpAddr, SourceAdmission>,
    nodes: HashMap<String, SourceAdmission>,
}

#[derive(Default, Clone)]
struct SourceAdmission {
    handshakes: usize,
    sessions: usize,
    bytes: usize,
    attempts: VecDeque<Instant>,
}

struct AdmissionController {
    state: Mutex<AdmissionState>,
}

struct AdmissionReservation {
    admission: Arc<AdmissionController>,
    /// The per-source-IP rate key, or `None` for a dial this node made.
    ///
    /// Only inbound work has a source to budget. A dial of our own has no
    /// stranger behind it to hold to account.
    source: Option<IpAddr>,
    node_id: Option<String>,
    phase: AdmissionPhase,
    bytes: usize,
}

#[derive(Clone, Copy)]
enum AdmissionPhase {
    Handshake,
    Session,
}

impl AdmissionController {
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
            && state.bytes.saturating_add(ADMISSION_BYTES) <= DIRECT_MAX_BYTES
            && source_handshakes < DIRECT_MAX_SOURCE_HANDSHAKES
            && source_bytes.saturating_add(ADMISSION_BYTES) <= DIRECT_MAX_SOURCE_BYTES
            && source_attempts < DIRECT_RATE_LIMIT;
        if !source_allowed {
            return None;
        }
        state.handshakes += 1;
        state.bytes += ADMISSION_BYTES;
        let source_state = state.sources.get_mut(&source)?;
        source_state.attempts.push_back(now);
        source_state.handshakes += 1;
        source_state.bytes += ADMISSION_BYTES;
        Some(AdmissionReservation {
            admission: Arc::clone(self),
            source: Some(source),
            node_id: None,
            phase: AdmissionPhase::Handshake,
            bytes: ADMISSION_BYTES,
        })
    }

    /// Take global admission capacity for a dial this node is making.
    ///
    /// Deliberately takes no per-source-IP budget. That budget bounds
    /// unauthenticated pressure arriving from a stranger, and an outgoing dial
    /// to a configured static peer is neither. Charging it to the local
    /// address made every node that shares an address with its peers --
    /// loopback, host networking, several nodes in one container -- spend
    /// their inbound flood budget on its own outgoing links. The global
    /// handshake, byte, and session ceilings still apply, and the peer's
    /// certificate is still charged per identity once it authenticates.
    fn reserve_dial(self: &Arc<Self>) -> Option<AdmissionReservation> {
        let mut state = self.state.lock().ok()?;
        if state.handshakes >= DIRECT_MAX_HANDSHAKES
            || state.bytes.saturating_add(ADMISSION_BYTES) > DIRECT_MAX_BYTES
        {
            return None;
        }
        state.handshakes += 1;
        state.bytes += ADMISSION_BYTES;
        Some(AdmissionReservation {
            admission: Arc::clone(self),
            source: None,
            node_id: None,
            phase: AdmissionPhase::Handshake,
            bytes: ADMISSION_BYTES,
        })
    }

    fn migrate_node(
        &self,
        reservation: &mut AdmissionReservation,
        node_id: &str,
    ) -> Result<(), TransportError> {
        // Keep the pre-auth IP reservation and add an authenticated node
        // reservation. This enforces both dimensions without allowing a node
        // to bypass limits by changing source addresses.
        let mut state = self.state.lock().map_err(|_| TransportError::Internal)?;
        if reservation.node_id.as_deref() == Some(node_id) {
            return Ok(());
        }
        if reservation
            .source
            .is_some_and(|source| !state.sources.contains_key(&source))
        {
            return Err(TransportError::Internal);
        }
        let current = SourceAdmission {
            handshakes: usize::from(matches!(reservation.phase, AdmissionPhase::Handshake)),
            sessions: usize::from(matches!(reservation.phase, AdmissionPhase::Session)),
            bytes: reservation.bytes,
            attempts: VecDeque::new(),
        };
        let existing = state.nodes.get(node_id).cloned().unwrap_or_default();
        if existing.handshakes + current.handshakes > DIRECT_MAX_NODE_HANDSHAKES
            || existing.sessions + current.sessions > DIRECT_MAX_NODE_SESSIONS
            || existing.bytes.saturating_add(current.bytes) > DIRECT_MAX_NODE_BYTES
        {
            return Err(TransportError::RateLimited);
        }
        state.nodes.insert(
            node_id.to_string(),
            SourceAdmission {
                handshakes: existing.handshakes + current.handshakes,
                sessions: existing.sessions + current.sessions,
                bytes: existing.bytes + current.bytes,
                attempts: existing
                    .attempts
                    .into_iter()
                    .chain(current.attempts)
                    .collect(),
            },
        );
        reservation.node_id = Some(node_id.to_string());
        Ok(())
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
        let source_sessions = self
            .source
            .map(|source| {
                state
                    .sources
                    .get(&source)
                    .ok_or(TransportError::Internal)
                    .map(|source| source.sessions)
            })
            .transpose()?;
        let node_sessions = self
            .node_id
            .as_deref()
            .map(|node_id| {
                state
                    .nodes
                    .get(node_id)
                    .ok_or(TransportError::Internal)
                    .map(|node| node.sessions)
            })
            .transpose()?;
        if state.sessions >= DIRECT_MAX_SESSIONS
            || source_sessions.is_some_and(|sessions| sessions >= DIRECT_MAX_SOURCE_SESSIONS)
            || node_sessions.is_some_and(|sessions| sessions >= DIRECT_MAX_NODE_SESSIONS)
        {
            return Err(TransportError::RateLimited);
        }
        state.handshakes = state.handshakes.saturating_sub(1);
        state.sessions += 1;
        if let Some(source) = self.source {
            let source_state = state
                .sources
                .get_mut(&source)
                .ok_or(TransportError::Internal)?;
            source_state.handshakes = source_state.handshakes.saturating_sub(1);
            source_state.sessions += 1;
        }
        if let Some(node_id) = self.node_id.as_deref() {
            let node_state = state
                .nodes
                .get_mut(node_id)
                .expect("node admission exists after preflight");
            node_state.handshakes = node_state.handshakes.saturating_sub(1);
            node_state.sessions += 1;
        }
        self.phase = AdmissionPhase::Session;
        Ok(())
    }
}

impl Drop for AdmissionReservation {
    fn drop(&mut self) {
        let Ok(mut state) = self.admission.state.lock() else {
            return;
        };
        match self.phase {
            AdmissionPhase::Handshake => {
                state.handshakes = state.handshakes.saturating_sub(1);
            }
            AdmissionPhase::Session => {
                state.sessions = state.sessions.saturating_sub(1);
            }
        }
        state.bytes = state.bytes.saturating_sub(self.bytes);
        if let Some(source) = self.source {
            if let Some(source_state) = state.sources.get_mut(&source) {
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
                    state.sources.remove(&source);
                }
            }
        }
        if let Some(node_id) = self.node_id.as_deref() {
            if let Some(node_state) = state.nodes.get_mut(node_id) {
                match self.phase {
                    AdmissionPhase::Handshake => {
                        node_state.handshakes = node_state.handshakes.saturating_sub(1);
                    }
                    AdmissionPhase::Session => {
                        node_state.sessions = node_state.sessions.saturating_sub(1);
                    }
                }
                node_state.bytes = node_state.bytes.saturating_sub(self.bytes);
                if node_state.handshakes == 0 && node_state.sessions == 0 && node_state.bytes == 0 {
                    state.nodes.remove(node_id);
                }
            }
        }
    }
}

impl DirectListener {
    pub fn start(bind: SocketAddr, context: NodeContext) -> Result<Self, DirectServiceError> {
        let identity = NodeIdentity::load_existing(&context)?;
        let stop = Arc::new(AtomicBool::new(false));
        let admission = Arc::new(AdmissionController {
            state: Mutex::new(AdmissionState::default()),
        });
        let state = ConnectionState::new(
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

    fn start_with_state(
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

fn dialer_loop(
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
                state.record_error(&peer.node_id, &error);
                if is_fatal_connection_error(&error) {
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
fn retry_backoff(failures: usize) -> Duration {
    u32::try_from(failures)
        .ok()
        .and_then(|steps| 1u32.checked_shl(steps))
        .and_then(|factor| RETRY_BACKOFF[0].checked_mul(factor))
        .unwrap_or(RETRY_BACKOFF_CEILING)
        .min(RETRY_BACKOFF_CEILING)
}

fn retry_jitter() -> Duration {
    Duration::from_millis(u64::from(OsRng.next_u32()) % (RETRY_JITTER_MAX.as_millis() as u64 + 1))
}

fn sleep_or_stop(duration: Duration, stop: &AtomicBool) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
        thread::sleep(
            Duration::from_millis(25).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

fn connect_and_hold(
    peer: &StaticPeer,
    context: &NodeContext,
    state: &Arc<ConnectionState>,
    resolver: &Resolver,
) -> Result<(), TransportError> {
    let deadline = initiator_deadline(Instant::now());
    let endpoints = resolver.resolve(&peer.endpoint, deadline, &state.stop)?;
    let mut stream = None;
    for endpoint in endpoints {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(TransportError::Internal)?;
        if let Ok(candidate) = TcpStream::connect_timeout(&endpoint, remaining) {
            stream = Some(candidate);
            break;
        }
    }
    let mut stream = stream.ok_or(TransportError::Internal)?;
    set_stream_timeouts(&stream, deadline)?;
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
    write_bytes(&mut stream, &handshake.write_next()?, deadline).map_err(error_to_transport)?;
    let frame = read_frame(&mut stream, deadline).map_err(error_to_transport)?;
    handshake.read_next(&frame, unix_seconds())?;
    write_bytes(&mut stream, &handshake.write_next()?, deadline).map_err(error_to_transport)?;
    let remote = handshake
        .remote_certificate()
        .cloned()
        .ok_or(TransportError::HandshakeFailed)?;
    if remote.node_id() != peer.node_id {
        return Err(TransportError::IdentityMismatch);
    }
    let trusted = registry
        .transport_peer(remote.node_id(), &hex(remote.identity_key()))
        .map_err(|_| TransportError::Internal)?;
    authorize_peer(
        &remote,
        trusted.as_ref().map(peer_authorization),
        unix_seconds(),
    )?;
    state
        .admission
        .migrate_node(&mut reservation, remote.node_id())?;
    let mut session = handshake.into_session()?;
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let probe = sign_probe(&identity, session.session_id(), nonce, unix_seconds())?;
    write_bytes(
        &mut stream,
        &session.write(ENVELOPE_KIND, &probe.encoded())?,
        deadline,
    )
    .map_err(error_to_transport)?;
    let frame = read_frame(&mut stream, deadline).map_err(error_to_transport)?;
    let response = session.read(&frame)?;
    if response.kind != ENVELOPE_KIND {
        return Err(TransportError::InvalidFrame);
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
        .record_transport_audit(
            "probe_accepted",
            remote.node_id(),
            Some(&session_id),
            Some(0),
            response.body.len() + probe.encoded().len(),
            "accepted",
            None,
        )
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
    hold_session(
        &mut stream,
        &mut session,
        state,
        &identity,
        &registry,
        remote.node_id(),
        remote.identity_key(),
        Some(health),
        Some(cue),
    )
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
#[allow(clippy::too_many_arguments)]
fn hold_session(
    stream: &mut TcpStream,
    session: &mut TransportSession,
    state: &Arc<ConnectionState>,
    identity: &NodeIdentity,
    registry: &NodeRegistry,
    peer_node_id: &str,
    peer_identity_key: &[u8; 32],
    mut health: Option<HealthSession<'_>>,
    mut cue: Option<crate::remote_cue::CueSession<'_>>,
) -> Result<(), TransportError> {
    if health.as_ref().is_some_and(|health| !health.engaged()) {
        health = None;
    }
    let Some(health) = health.as_mut() else {
        return hold_session_idle(stream, session, state);
    };
    stream
        .set_read_timeout(Some(crate::direct_health::TICK))
        .map_err(|_| TransportError::Internal)?;
    let mut last_activity = Instant::now();
    let mut outbound_cue: Option<OutboundCue> = None;
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
        if let Some(in_flight) = outbound_cue.as_mut() {
            if in_flight.resolve(registry, peer_node_id) {
                outbound_cue = None;
            }
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
                if last_activity.elapsed() >= IDLE_TIMEOUT {
                    return Ok(());
                }
                continue;
            }
            Readiness::Closed => return Ok(()),
            Readiness::Failed(error) => return Err(error),
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
            Err(error) => return Err(error_to_transport(error)),
        };
        let frame = Frame::parse(&encoded)?;
        if frame.kind != 2 {
            return Err(TransportError::InvalidFrame);
        }
        let message = session.read(&encoded)?;
        last_activity = Instant::now();
        if message.kind != ENVELOPE_KIND {
            continue;
        }
        if let Some(in_flight) = outbound_cue.as_mut() {
            if in_flight.absorb_ack(&message.body, peer_node_id, peer_identity_key, session) {
                outbound_cue = None;
                continue;
            }
        }
        match health.handle_envelope(&message.body) {
            // A non-health envelope used to be discarded here without a trace.
            // Cue traffic is decided and audited instead; anything else keeps
            // the original silence, so the dispatcher never becomes an oracle
            // that answers unknown kinds.
            HealthOutcome::NotHealth => {
                if let Some(cue) = cue.as_mut() {
                    // The Cue session verifies the envelope against the same
                    // handshake identity and session id the Health Plane uses;
                    // nothing here decides anything.
                    if cue.handle_envelope(&message.body, unix_seconds() as i64)
                        != CueOutcome::NotCue
                    {
                        if let Some(reply) = cue.take_reply() {
                            write_bytes(stream, &session.write(ENVELOPE_KIND, &reply)?, deadline)
                                .map_err(error_to_transport)?;
                        }
                    }
                }
            }
            HealthOutcome::Handled => {}
            HealthOutcome::Reply(reply) => {
                write_bytes(stream, &session.write(ENVELOPE_KIND, &reply)?, deadline)
                    .map_err(error_to_transport)?;
            }
        }
    }
    Ok(())
}

/// Sends Cues over the sessions the running service already holds.
///
/// This is the path that works in a managed fleet. A separate process cannot
/// dial a peer this node is already connected to -- `register` refuses it, and
/// should, because two sessions with one peer would give the Health Plane two
/// cursors for the same node. So the instruction is handed to the thread that
/// owns the session instead of racing it for a new one.
#[derive(Clone)]
pub struct CueDispatcher {
    state: Arc<ConnectionState>,
}

impl CueDispatcher {
    /// Send one Cue and wait for as much of an answer as arrives in budget.
    ///
    /// `NotEnrolled` means there is no live session with that peer, which is a
    /// different fact from a refusal and is reported as one.
    pub fn dispatch(
        &self,
        peer_node_id: &str,
        script: &str,
        reason: &str,
        wait: Duration,
    ) -> Result<CueDispatchOutcome, DirectServiceError> {
        if !crate::remote_cue::is_well_formed_script_name(script) {
            return Err(TransportError::InvalidFrame.into());
        }
        if reason.is_empty() || reason.len() > crate::remote_cue::MAX_REASON_BYTES {
            return Err(TransportError::InvalidFrame.into());
        }
        let mut cue_id_bytes = [0u8; 16];
        OsRng.fill_bytes(&mut cue_id_bytes);
        let cue_id = hex(&cue_id_bytes);
        let expected_run_id =
            crate::health_plane::report::opaque_run_id(&crate::remote_cue::derive_run_id(&cue_id));
        let (reply, answers) = std::sync::mpsc::sync_channel(1);
        self.state.enqueue_cue(
            peer_node_id,
            PendingCue {
                cue_id: cue_id.clone(),
                script: script.to_string(),
                reason: reason.to_string(),
                expected_run_id: expected_run_id.clone(),
                deadline: Instant::now() + wait,
                reply,
            },
        )?;
        // A little past the session thread's own deadline, so the answer it is
        // about to send wins over this timeout.
        match answers.recv_timeout(wait + crate::direct_health::TICK * 2) {
            Ok(outcome) => Ok(outcome),
            // The session ended, or it never got to us. Neither is a verdict.
            Err(_) => Ok(CueDispatchOutcome {
                cue_id,
                expected_run_id,
                answered: false,
                accepted: false,
                code: 0,
                outcome_seen: false,
            }),
        }
    }

    /// Whether a live session with this peer exists to carry a Cue.
    pub fn has_session(&self, peer_node_id: &str) -> bool {
        self.state
            .active
            .lock()
            .map(|active| active.contains_key(peer_node_id))
            .unwrap_or(false)
    }
}

/// Fail everything still queued for a peer when its session ends.
struct OutboxGuard<'a> {
    state: &'a Arc<ConnectionState>,
    peer_node_id: &'a str,
}

impl Drop for OutboxGuard<'_> {
    fn drop(&mut self) {
        self.state.drain_outbox(self.peer_node_id);
    }
}

impl PendingCue {
    /// Answer the waiting caller. A closed channel means it gave up; that is
    /// not an error here, and must not take the session down with it.
    fn answer(self, answered: bool, accepted: bool, code: u16, outcome_seen: bool) {
        let _ = self.reply.try_send(CueDispatchOutcome {
            cue_id: self.cue_id,
            expected_run_id: self.expected_run_id,
            answered,
            accepted,
            code,
            outcome_seen,
        });
    }
}

/// One Cue written on this session, waiting for its ack and then its outcome.
struct OutboundCue {
    pending: PendingCue,
    /// `None` until the Performer answers. A refusal on trust, role, or
    /// capability is silent by design, so staying `None` is a real answer.
    code: Option<u16>,
}

impl OutboundCue {
    fn new(pending: PendingCue) -> Self {
        Self {
            pending,
            code: None,
        }
    }

    /// Take the `cue_ack` for this Cue out of the stream, if this is it.
    ///
    /// Returns `true` when the exchange is finished and the caller has been
    /// answered. A refusal ends it here; an acceptance keeps waiting for the
    /// outcome, which arrives as an ordinary Signal the Health Plane records.
    fn absorb_ack(
        &mut self,
        body: &[u8],
        peer_node_id: &str,
        peer_identity_key: &[u8; 32],
        session: &TransportSession,
    ) -> bool {
        if crate::direct_transport::envelope_kind_hint(body) != Some(crate::remote_cue::KIND_ACK) {
            return false;
        }
        let Ok(nonce) = envelope_nonce(body) else {
            return false;
        };
        // Anchored to the identity the handshake established, not to anything
        // the message says about itself.
        if verify_envelope(
            body,
            peer_node_id,
            peer_identity_key,
            crate::remote_cue::KIND_ACK,
            session.session_id(),
            &nonce,
        )
        .is_err()
        {
            return false;
        }
        let Ok(view) = crate::direct_transport::envelope_view(body) else {
            return false;
        };
        let Some(ack) = view.payload.as_object() else {
            return false;
        };
        // An ack for a different Cue is not an answer to this one.
        if ack.get("cue_id").and_then(serde_json::Value::as_str) != Some(&self.pending.cue_id) {
            return false;
        }
        let accepted = ack
            .get("accepted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if accepted {
            self.code = Some(0);
            return false;
        }
        let code = ack
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|code| u16::try_from(code).ok())
            .unwrap_or_else(|| CueCode::InvalidMessage.code());
        std::mem::replace(&mut self.pending, placeholder_cue()).answer(true, false, code, false);
        true
    }

    /// Finish once the outcome is recorded or the budget runs out.
    ///
    /// The stop condition is read back from the registry after the Health Plane
    /// session verified and recorded the Signal, never from a payload this code
    /// inspected itself.
    fn resolve(&mut self, registry: &NodeRegistry, peer_node_id: &str) -> bool {
        if self.code == Some(0) {
            let plane = crate::health_plane::HealthPlane::new(registry);
            if signal_recorded(&plane, peer_node_id, &self.pending.expected_run_id) {
                std::mem::replace(&mut self.pending, placeholder_cue()).answer(true, true, 0, true);
                return true;
            }
        }
        if Instant::now() < self.pending.deadline {
            return false;
        }
        let answered = self.code.is_some();
        let accepted = self.code == Some(0);
        let code = self.code.unwrap_or(0);
        std::mem::replace(&mut self.pending, placeholder_cue())
            .answer(answered, accepted, code, false);
        true
    }
}

/// A spent `PendingCue`, so the real one can be moved out to answer with.
///
/// Its channel has no receiver, so answering it is a no-op by construction.
fn placeholder_cue() -> PendingCue {
    let (reply, _) = std::sync::mpsc::sync_channel(1);
    PendingCue {
        cue_id: String::new(),
        script: String::new(),
        reason: String::new(),
        expected_run_id: String::new(),
        deadline: Instant::now(),
        reply,
    }
}

/// Sign the `cue_dispatch` for a queued Cue on this session.
fn sign_pending_cue(
    identity: &NodeIdentity,
    session_id: &[u8; 32],
    pending: &PendingCue,
) -> Result<Vec<u8>, TransportError> {
    let now = unix_seconds();
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    Ok(crate::direct_transport::sign_cue_envelope(
        identity,
        crate::remote_cue::KIND_DISPATCH,
        session_id,
        nonce,
        serde_json::json!({
            "version": 1,
            "cue_id": pending.cue_id,
            "script": pending.script,
            "not_before": now,
            "expires_at": now + crate::remote_cue::MAX_LIFETIME_SECONDS as u64,
            "reason": pending.reason,
        }),
        now,
    )?
    .encoded())
}

/// The pre-Health-Plane steady-state loop, unchanged.
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
enum Readiness {
    Readable,
    Idle,
    Closed,
    Failed(TransportError),
}

/// Wait up to `tick` for the peer to send something, without consuming it.
fn wait_readable(stream: &TcpStream, tick: Duration) -> Readiness {
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

fn error_to_transport(error: DirectServiceError) -> TransportError {
    match error {
        DirectServiceError::Protocol(error) => error,
        _ => TransportError::Internal,
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
    let deadline = initiator_deadline(Instant::now());
    let mut stream = TcpStream::connect_timeout(
        &endpoint,
        deadline
            .checked_duration_since(Instant::now())
            .ok_or(TransportError::Internal)?,
    )?;
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
    let peer = registry.transport_peer(remote.node_id(), &hex(remote.identity_key()))?;
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
    OsRng.fill_bytes(&mut nonce);
    let probe = sign_probe(&identity, session.session_id(), nonce, unix_seconds())?;
    write_bytes(
        &mut stream,
        &session.write(ENVELOPE_KIND, &probe.encoded())?,
        deadline,
    )?;
    let response = session.read(&read_frame(&mut stream, deadline)?)?;
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

/// Ask one trusted Performer to run a script it has already declared.
///
/// A one-shot dial mirroring `probe`, deliberately with no Conductor-side
/// durable outbox: a Cue is an instruction with a short validity window, and a
/// queue of instructions that outlive their window is a way to deliver
/// surprises. If the dial fails, the operator dials again with a new id.
///
/// Returns the minted `cue_id`, from which the Conductor can compute the opaque
/// run id it will later see on the `run-completed` Signal — which is why no
/// correlation field is added to any message.
pub fn dispatch_cue(
    endpoint: SocketAddr,
    expected_node_id: &str,
    script: &str,
    reason: &str,
    wait_seconds: u32,
    context: &NodeContext,
) -> Result<CueDispatchOutcome, DirectServiceError> {
    if !crate::remote_cue::is_well_formed_script_name(script) {
        return Err(TransportError::InvalidFrame.into());
    }
    if reason.is_empty() || reason.len() > 128 {
        return Err(TransportError::InvalidFrame.into());
    }

    let identity = NodeIdentity::load_existing(context)?;
    let local = LocalTransport::load_existing(context, &identity)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())?;
    let deadline = initiator_deadline(Instant::now());
    let mut stream = TcpStream::connect_timeout(
        &endpoint,
        deadline
            .checked_duration_since(Instant::now())
            .ok_or(TransportError::Internal)?,
    )?;
    set_stream_timeouts(&stream, deadline)?;

    let mut handshake = local.handshake(HandshakeRole::Initiator)?;
    write_bytes(&mut stream, &handshake.write_next()?, deadline)?;
    handshake.read_next(&read_frame(&mut stream, deadline)?, unix_seconds())?;
    write_bytes(&mut stream, &handshake.write_next()?, deadline)?;
    let remote = handshake
        .remote_certificate()
        .cloned()
        .ok_or(TransportError::HandshakeFailed)?;
    if remote.node_id() != expected_node_id {
        return Err(TransportError::IdentityMismatch.into());
    }
    let trusted = registry.transport_peer(remote.node_id(), &hex(remote.identity_key()))?;
    authorize_peer(
        &remote,
        trusted.as_ref().map(peer_authorization),
        unix_seconds(),
    )?;
    let mut session = handshake.into_session()?;

    // The responder's steady-state loop is only reachable through the existing
    // probe/ack entry. A Cue is new traffic *inside* an established session,
    // not a new way to open one, so the ritual is performed unchanged rather
    // than given a second door that would need its own review.
    let mut probe_nonce = [0u8; 16];
    OsRng.fill_bytes(&mut probe_nonce);
    let probe = sign_probe(&identity, session.session_id(), probe_nonce, unix_seconds())?;
    write_bytes(
        &mut stream,
        &session.write(ENVELOPE_KIND, &probe.encoded())?,
        deadline,
    )?;
    let response = session.read(&read_frame(&mut stream, deadline)?)?;
    if response.kind != ENVELOPE_KIND {
        return Err(TransportError::InvalidFrame.into());
    }
    verify_envelope(
        &response.body,
        remote.node_id(),
        remote.identity_key(),
        "ack",
        session.session_id(),
        &probe_nonce,
    )?;

    let mut cue_id_bytes = [0u8; 16];
    OsRng.fill_bytes(&mut cue_id_bytes);
    let cue_id = hex(&cue_id_bytes);
    let now = unix_seconds();
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let dispatch = crate::direct_transport::sign_cue_envelope(
        &identity,
        crate::remote_cue::KIND_DISPATCH,
        session.session_id(),
        nonce,
        serde_json::json!({
            "version": 1,
            "cue_id": cue_id,
            "script": script,
            "not_before": now,
            "expires_at": now + crate::remote_cue::MAX_LIFETIME_SECONDS as u64,
            "reason": reason,
        }),
        now,
    )?;
    write_bytes(
        &mut stream,
        &session.write(ENVELOPE_KIND, &dispatch.encoded())?,
        deadline,
    )?;

    // A refusal the sender is not authorized to hear is silent by design, so
    // the absence of an ack is a legitimate answer and not an error. It is
    // reported as `answered: false` rather than being turned into a code the
    // Performer never sent.
    let acknowledgement = read_cue_ack(&mut stream, &mut session, &remote, &cue_id, deadline);

    registry.record_transport_audit(
        "cue_dispatched",
        remote.node_id(),
        Some(session.session_id()),
        Some(0),
        dispatch.encoded().len(),
        "accepted",
        None,
    )?;
    // The Conductor computes the opaque run id it will see on the
    // `run-completed` Signal from the cue id it just minted. No message
    // carries a correlation field; both sides derive it.
    let expected_run_id =
        crate::health_plane::report::opaque_run_id(&crate::remote_cue::derive_run_id(&cue_id));

    // Wait for the outcome on the session already open, rather than requiring a
    // standing one. A Performer that already holds a session with this
    // Conductor refuses this dial outright -- `register` will not accept from a
    // peer it owns the dial to, nor a second connection to a peer it already
    // has -- so the configuration that would deliver the Signal is exactly the
    // one in which the Cue could not be sent. The Performer already pushes
    // Health traffic down this session unprompted; this reads it.
    let outcome_seen = if wait_seconds > 0 && acknowledgement == Some(0) {
        let until = Instant::now() + Duration::from_secs(u64::from(wait_seconds));
        set_stream_timeouts(&stream, until)?;
        await_cue_outcome(
            &mut stream,
            &mut session,
            &remote,
            &identity,
            &registry,
            &expected_run_id,
            until,
        )
    } else {
        false
    };

    Ok(CueDispatchOutcome {
        expected_run_id,
        cue_id,
        answered: acknowledgement.is_some(),
        accepted: acknowledgement.is_some_and(|code| code == 0),
        code: acknowledgement.unwrap_or(0),
        outcome_seen,
    })
}

/// Read Health traffic on this session until the Cue's outcome shows up.
///
/// The dispatcher behaves as an ordinary Conductor receiver for the duration:
/// the `HealthSession` verifies, records, and acknowledges exactly as the
/// service would, so nothing here is a second, looser path into the Health
/// Plane. The stop condition is read back from the registry after the session
/// recorded it, never from an unverified payload.
#[allow(clippy::too_many_arguments)]
fn await_cue_outcome(
    stream: &mut TcpStream,
    session: &mut TransportSession,
    remote: &crate::direct_transport::TransportCertificate,
    identity: &NodeIdentity,
    registry: &NodeRegistry,
    expected_run_id: &str,
    until: Instant,
) -> bool {
    let mut health = HealthSession::new(
        identity,
        registry,
        remote.node_id(),
        remote.identity_key(),
        *session.session_id(),
        None,
    );
    let plane = crate::health_plane::HealthPlane::new(registry);
    loop {
        if signal_recorded(&plane, remote.node_id(), expected_run_id) {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        match wait_readable(stream, crate::direct_health::TICK) {
            Readiness::Readable => {}
            Readiness::Idle => continue,
            Readiness::Closed | Readiness::Failed(_) => {
                // One last look: the Signal may have landed on the frame that
                // arrived immediately before the peer hung up.
                return signal_recorded(&plane, remote.node_id(), expected_run_id);
            }
        }
        let Ok(encoded) = read_frame(stream, until) else {
            return signal_recorded(&plane, remote.node_id(), expected_run_id);
        };
        let Ok(message) = session.read(&encoded) else {
            return false;
        };
        if message.kind != ENVELOPE_KIND {
            continue;
        }
        if let crate::direct_health::HealthOutcome::Reply(reply) =
            health.handle_envelope(&message.body)
        {
            let Ok(frame) = session.write(ENVELOPE_KIND, &reply) else {
                return false;
            };
            if write_bytes(stream, &frame, Instant::now() + IDLE_TIMEOUT).is_err() {
                return false;
            }
        }
    }
}

/// Whether this peer's recorded Signals already carry the awaited run.
fn signal_recorded(
    plane: &crate::health_plane::HealthPlane<'_>,
    node_id: &str,
    expected_run_id: &str,
) -> bool {
    plane
        .signals(
            node_id,
            crate::health_plane::bounds::SIGNAL_INBOX_CAPACITY as usize,
        )
        .map(|signals| {
            signals.iter().any(|signal| {
                signal.kind == crate::health_plane::model::SignalKind::RunCompleted
                    && signal
                        .run
                        .as_ref()
                        .is_some_and(|run| run.run_id == expected_run_id)
            })
        })
        .unwrap_or(false)
}

/// What the Conductor can honestly say about one dispatch.
///
/// `answered` is separate from `accepted` on purpose: a Performer that refuses
/// on trust, role, or capability says nothing at all, so "no answer" and
/// "refused with a code" are different facts and collapsing them would invent
/// a verdict nobody sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueDispatchOutcome {
    pub cue_id: String,
    /// The `run.run_id` the matching `run-completed` Signal will carry.
    pub expected_run_id: String,
    pub answered: bool,
    pub accepted: bool,
    pub code: u16,
    /// Whether the `run-completed` Signal for this Cue arrived before the wait
    /// budget ran out. `false` is not a failure -- the run may simply still be
    /// going, and the Signal will reach a standing session later.
    pub outcome_seen: bool,
}

/// Read one `cue_ack` for this cue id, or `None` if none arrives in budget.
///
/// The session is shared with the Health Plane, whose reporter greets a trusted
/// Conductor with a Profile the moment the connection opens, so the ack is very
/// often not the first frame. Anything that is not this cue's ack is skipped:
/// a one-shot dispatcher holds no Health session and has no business answering
/// Health traffic.
///
/// Bounded twice over -- by the connection deadline and by a frame count -- so
/// a peer cannot hold the dispatcher open by talking.
fn read_cue_ack(
    stream: &mut TcpStream,
    session: &mut TransportSession,
    remote: &crate::direct_transport::TransportCertificate,
    cue_id: &str,
    deadline: Instant,
) -> Option<u16> {
    /// Enough for a Profile, a Pulse, and a Signal to precede the ack.
    const MAX_FRAMES_BEFORE_ACK: usize = 8;

    for _ in 0..MAX_FRAMES_BEFORE_ACK {
        if Instant::now() >= deadline {
            return None;
        }
        let frame = read_frame(stream, deadline).ok()?;
        let message = session.read(&frame).ok()?;
        if message.kind != ENVELOPE_KIND {
            continue;
        }
        if crate::direct_transport::envelope_kind_hint(&message.body)
            != Some(crate::remote_cue::KIND_ACK)
        {
            continue;
        }
        let nonce = envelope_nonce(&message.body).ok()?;
        verify_envelope(
            &message.body,
            remote.node_id(),
            remote.identity_key(),
            crate::remote_cue::KIND_ACK,
            session.session_id(),
            &nonce,
        )
        .ok()?;
        let view = crate::direct_transport::envelope_view(&message.body).ok()?;
        let ack = view.payload.as_object()?;
        // An ack for a different cue id is not an answer to this dispatch.
        if ack.get("cue_id").and_then(serde_json::Value::as_str) != Some(cue_id) {
            return None;
        }
        if ack.get("accepted").and_then(serde_json::Value::as_bool)? {
            return Some(0);
        }
        let code = ack
            .get("error")?
            .get("code")
            .and_then(serde_json::Value::as_u64)?;
        // Zero is how acceptance is spelled, so a refusal must never land on it.
        return u16::try_from(code).ok().filter(|code| *code != 0);
    }
    None
}

pub fn request_manual_enrollment(
    endpoint: SocketAddr,
    context: &NodeContext,
    request: &[u8],
) -> Result<ManualEnrollmentReciprocal, DirectServiceError> {
    if request.len() > crate::enrollment::MAX_REQUEST_BYTES {
        return Err(DirectServiceError::Protocol(
            TransportError::MessageTooLarge,
        ));
    }
    let local_request = ManualEnrollmentRequest::decode(request)
        .map_err(|_| DirectServiceError::Protocol(TransportError::InvalidFrame))?;
    let identity = NodeIdentity::load_existing(context)?;
    let local = LocalTransport::load_existing(context, &identity)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())?;
    let deadline = initiator_deadline(Instant::now());
    let mut stream = TcpStream::connect_timeout(
        &endpoint,
        deadline
            .checked_duration_since(Instant::now())
            .ok_or(TransportError::Internal)?,
    )?;
    set_stream_timeouts(&stream, deadline).map_err(|_| TransportError::Internal)?;
    let mut handshake = local.handshake(HandshakeRole::Initiator)?;
    write_bytes(&mut stream, &handshake.write_next()?, deadline)?;
    handshake.read_next(&read_frame(&mut stream, deadline)?, unix_seconds())?;
    write_bytes(&mut stream, &handshake.write_next()?, deadline)?;
    let remote = handshake
        .remote_certificate()
        .cloned()
        .ok_or(TransportError::HandshakeFailed)?;
    if remote.node_id() == identity.public_status().node_id {
        return Err(TransportError::IdentityMismatch.into());
    }
    let mut session = handshake.into_session()?;
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let message = sign_manual_request(
        &identity,
        session.session_id(),
        nonce,
        request,
        unix_seconds(),
    )?;
    write_bytes(
        &mut stream,
        &session.write(ENVELOPE_KIND, &message.encoded())?,
        deadline,
    )?;
    let response = session.read(&read_frame(&mut stream, deadline)?)?;
    if response.kind != ENVELOPE_KIND {
        return Err(TransportError::NotEnrolled.into());
    }
    verify_envelope(
        &response.body,
        remote.node_id(),
        remote.identity_key(),
        "manual_ack",
        session.session_id(),
        &nonce,
    )?;
    let accepted = enrollment_ack_accepted(&response.body)?;
    let reciprocal = if accepted {
        Some(enrollment_ack_offer(&response.body)?)
    } else {
        None
    };
    registry.record_transport_audit(
        "enrollment_request",
        remote.node_id(),
        Some(session.session_id()),
        Some(0),
        request.len() + response.body.len(),
        if accepted { "accepted" } else { "rejected" },
        None,
    )?;
    if let Some((reciprocal_request, reciprocal_code)) = reciprocal {
        let reciprocal_request = ManualEnrollmentRequest::decode(&reciprocal_request)
            .map_err(|_| TransportError::InvalidFrame)?;
        reciprocal_request
            .verify(unix_seconds())
            .map_err(|_| TransportError::InvalidFrame)?;
        if reciprocal_request.proposer_node_id != remote.node_id()
            || reciprocal_request.proposer_xonly != *remote.identity_key()
            || reciprocal_request.proposer_transport_x25519 != *remote.transport_public()
            || reciprocal_request.pairing_id != local_request.pairing_id
        {
            return Err(TransportError::IdentityMismatch.into());
        }
        registry.stage_manual_enrollment(
            &reciprocal_request,
            remote.as_bytes(),
            "local-enrollment-request",
            "staged reciprocal manual enrollment request",
            unix_seconds(),
        )?;
        Ok(Some((reciprocal_request.encode(), reciprocal_code)))
    } else {
        Ok(None)
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
    } = connection;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    set_stream_timeouts(&stream, deadline).map_err(|_| io::Error::from(io::ErrorKind::Other))?;
    let identity = NodeIdentity::load_existing(context)?;
    let local = LocalTransport::load_existing(context, &identity)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())?;
    let mut handshake = local.handshake(HandshakeRole::Responder)?;
    let mut remote_node_id = None;
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
        let peer = registry.transport_peer(remote.node_id(), &hex(remote.identity_key()))?;
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
        hold_session(
            &mut stream,
            &mut session,
            state,
            &identity,
            &registry,
            remote.node_id(),
            remote.identity_key(),
            Some(health),
            Some(cue),
        )
        .map_err(DirectServiceError::Protocol)?;
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

fn serve_enrollment_request(
    stream: &mut TcpStream,
    session: &mut TransportSession,
    identity: &NodeIdentity,
    registry: &NodeRegistry,
    remote: &crate::direct_transport::TransportCertificate,
    context: &NodeContext,
    deadline: Instant,
) -> Result<(), DirectServiceError> {
    let frame = read_frame(stream, deadline)?;
    let request = session.read(&frame)?;
    if request.kind != ENVELOPE_KIND {
        return Err(TransportError::NotEnrolled.into());
    }
    let nonce = envelope_nonce(&request.body)?;
    verify_envelope(
        &request.body,
        remote.node_id(),
        remote.identity_key(),
        "manual_request",
        session.session_id(),
        &nonce,
    )?;
    let request_bytes = enrollment_request_bytes(&request.body)?;
    let enrollment_request = ManualEnrollmentRequest::decode(&request_bytes)
        .map_err(|_| TransportError::InvalidFrame)?;
    enrollment_request
        .verify(unix_seconds())
        .map_err(|error| match error {
            crate::enrollment::EnrollmentError::Expired => TransportError::Expired,
            crate::enrollment::EnrollmentError::IdentityMismatch => {
                TransportError::IdentityMismatch
            }
            _ => TransportError::InvalidFrame,
        })?;
    if enrollment_request.proposer_node_id != remote.node_id()
        || enrollment_request.proposer_xonly != *remote.identity_key()
        || enrollment_request.proposer_transport_x25519 != *remote.transport_public()
    {
        return Err(TransportError::IdentityMismatch.into());
    }
    crate::operations::node::manual_enrollment_enabled(context)
        .map_err(|_| TransportError::NotEnrolled)?;
    registry.stage_manual_enrollment(
        &enrollment_request,
        remote.as_bytes(),
        "authenticated-untrusted",
        "authenticated manual enrollment request",
        unix_seconds(),
    )?;
    let transport =
        LocalTransport::load_existing(context, identity).map_err(|_| TransportError::Internal)?;
    let reciprocal = ManualEnrollmentRequest::create_with_pairing_id(
        identity,
        *transport.certificate().transport_public(),
        EnrollmentRole::Conductor,
        Vec::new(),
        unix_seconds(),
        300,
        enrollment_request.pairing_id,
    )
    .map_err(|_| TransportError::InvalidFrame)?;
    let reciprocal_request = reciprocal.request.encode();
    let ack = sign_manual_ack(
        identity,
        session.session_id(),
        nonce,
        true,
        Some(&reciprocal_request),
        Some(&reciprocal.code),
        unix_seconds(),
    )?;
    write_bytes(
        stream,
        &session.write(ENVELOPE_KIND, &ack.encoded())?,
        deadline,
    )?;
    Ok(())
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

fn deadline_timeout(deadline: Instant) -> Result<Duration, TransportError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or(TransportError::Internal)
}

fn initiator_deadline(started: Instant) -> Instant {
    started + CONNECT_TIMEOUT
}

fn set_stream_timeouts(stream: &TcpStream, deadline: Instant) -> Result<(), TransportError> {
    let timeout = deadline_timeout(deadline)?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|_| TransportError::Internal)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|_| TransportError::Internal)
}

fn read_frame(stream: &mut TcpStream, deadline: Instant) -> Result<Vec<u8>, DirectServiceError> {
    stream.set_read_timeout(Some(
        deadline_timeout(deadline).map_err(DirectServiceError::Protocol)?,
    ))?;
    let mut prefix = [0u8; 4];
    stream.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if !(4..=crate::direct_transport::MAX_FRAME_LENGTH).contains(&length) {
        return Err(TransportError::MessageTooLarge.into());
    }
    let mut frame = Vec::with_capacity(length + 4);
    frame.extend_from_slice(&prefix);
    frame.resize(length + 4, 0);
    let started_kib = length.saturating_add(65_535) / 65_536;
    let body_timeout = Duration::from_secs(1)
        .saturating_add(Duration::from_secs(started_kib as u64))
        .min(deadline_timeout(deadline).map_err(DirectServiceError::Protocol)?);
    stream.set_read_timeout(Some(body_timeout))?;
    stream.read_exact(&mut frame[4..])?;
    Frame::parse(&frame)?;
    Ok(frame)
}

fn write_bytes(
    stream: &mut TcpStream,
    bytes: &[u8],
    deadline: Instant,
) -> Result<(), DirectServiceError> {
    stream.set_write_timeout(Some(
        deadline_timeout(deadline).map_err(DirectServiceError::Protocol)?,
    ))?;
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

    static RESOLVER_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn retry_backoff_keeps_the_opening_ladder_then_holds_at_the_ceiling() {
        for (failures, expected) in RETRY_BACKOFF.iter().enumerate() {
            assert_eq!(retry_backoff(failures), *expected);
        }
        assert_eq!(retry_backoff(RETRY_BACKOFF.len()), Duration::from_secs(8));
        assert_eq!(retry_backoff(usize::MAX), RETRY_BACKOFF_CEILING);
        let mut previous = Duration::ZERO;
        for failures in 0..64 {
            let delay = retry_backoff(failures);
            assert!(delay >= previous, "backoff shrank at {failures} failures");
            assert!(
                delay <= RETRY_BACKOFF_CEILING,
                "backoff passed the ceiling at {failures} failures"
            );
            previous = delay;
        }
    }

    /// The ceiling is only defensible if a peer that comes back is redialed
    /// while the fleet still counts it Online.
    #[test]
    fn retry_ceiling_redials_within_the_presence_window() {
        let worst_case =
            RETRY_BACKOFF_CEILING + RETRY_JITTER_MAX + CONNECT_TIMEOUT + HANDSHAKE_TIMEOUT;
        let online = u64::try_from(crate::health_plane::bounds::PRESENCE_ONLINE_SECONDS)
            .expect("the Online window is a positive number of seconds");
        let online = Duration::from_secs(online);
        assert!(
            worst_case < online,
            "a redial can take {worst_case:?}, past the {online:?} Online window"
        );
    }

    /// The pre-auth budget is all that stands between an unenrolled stranger
    /// and the handshake path, so anything narrowed elsewhere has to leave it
    /// exactly this strict. No identity is known anywhere in this test; that
    /// is the point.
    ///
    /// Three caps enforce the budget and all three land on four: concurrent
    /// handshakes, reserved bytes, and arrival rate. So this freezes the
    /// behaviour rather than any one cap, and deleting a single one will not
    /// redden it. Deleting the per-address dimension will.
    #[test]
    fn an_unauthenticated_flood_from_one_address_is_still_refused() {
        let admission = Arc::new(AdmissionController {
            state: Mutex::new(AdmissionState::default()),
        });
        let flooder = "203.0.113.7".parse::<IpAddr>().unwrap();
        let bystander = "203.0.113.8".parse::<IpAddr>().unwrap();
        let start = Instant::now();

        let held = (0..DIRECT_MAX_SOURCE_HANDSHAKES)
            .map(|_| {
                admission
                    .reserve(flooder, start)
                    .expect("the budget admits its own allowance")
            })
            .collect::<Vec<_>>();
        assert!(
            admission.reserve(flooder, start).is_none(),
            "an unauthenticated flood passed the per-address budget"
        );
        assert!(
            admission.reserve(bystander, start).is_some(),
            "the flood starved an unrelated address"
        );

        // Hanging up does not reopen the door. Arrival rate is budgeted apart
        // from concurrency, so a stranger cannot flood by closing and
        // reopening inside the window.
        drop(held);
        assert!(
            admission.reserve(flooder, start).is_none(),
            "the flood was readmitted by closing its own connections"
        );
        assert!(
            admission
                .reserve(flooder, start + DIRECT_RATE_WINDOW)
                .is_some(),
            "a caller was still refused after the rate window rolled"
        );
    }

    /// Waiting is not a way around the budget either.
    ///
    /// A stranger that respects the arrival rate exactly -- one handshake per
    /// window, never hanging up -- is still bounded by what it is holding, so
    /// the rate cap is not the only thing standing in front of the handshake
    /// path.
    #[test]
    fn a_patient_stranger_cannot_hold_more_than_its_address_allowance() {
        let admission = Arc::new(AdmissionController {
            state: Mutex::new(AdmissionState::default()),
        });
        let stranger = "203.0.113.9".parse::<IpAddr>().unwrap();
        let start = Instant::now();

        let held = (0..DIRECT_MAX_SOURCE_HANDSHAKES)
            .map(|window| {
                let now = start + DIRECT_RATE_WINDOW * u32::try_from(window).unwrap();
                admission
                    .reserve(stranger, now)
                    .unwrap_or_else(|| panic!("the arrival rate refused window {window}"))
            })
            .collect::<Vec<_>>();
        let after = start + DIRECT_RATE_WINDOW * u32::try_from(held.len()).unwrap();
        assert!(
            admission.reserve(stranger, after).is_none(),
            "a stranger that waited out every rate window held more than its \
             address allowance"
        );
        drop(held);
    }

    /// On one host every address in play is the same address, so a node that
    /// charged its own dials to it spent the budget that protects it from
    /// strangers on its own outgoing links.
    #[test]
    fn a_nodes_own_dials_leave_the_inbound_budget_to_its_peers() {
        let admission = Arc::new(AdmissionController {
            state: Mutex::new(AdmissionState::default()),
        });
        let shared = "127.0.0.1".parse::<IpAddr>().unwrap();
        let start = Instant::now();

        let dials = (0..DIRECT_MAX_SOURCE_HANDSHAKES)
            .map(|_| admission.reserve_dial().expect("a dial of this node's own"))
            .collect::<Vec<_>>();
        let inbound = (0..DIRECT_MAX_SOURCE_HANDSHAKES)
            .map(|index| {
                admission
                    .reserve(shared, start)
                    .unwrap_or_else(|| panic!("inbound peer {index} lost its budget to our dials"))
            })
            .collect::<Vec<_>>();
        assert!(
            admission.reserve(shared, start).is_none(),
            "the per-address budget stopped applying to inbound peers"
        );

        drop(inbound);
        drop(dials);
        let state = admission.state.lock().unwrap();
        assert_eq!(
            state.handshakes, 0,
            "a released dial left global handshake capacity behind"
        );
        assert_eq!(
            state.bytes, 0,
            "a released dial left global byte capacity behind"
        );
    }

    #[test]
    fn admission_limits_each_source_without_starving_another_source() {
        let admission = Arc::new(AdmissionController {
            state: Mutex::new(AdmissionState::default()),
        });
        let first = "127.0.0.1:10001".parse::<SocketAddr>().unwrap();
        let second = "127.0.0.2:10001".parse::<SocketAddr>().unwrap();
        let now = Instant::now();
        let first_reservations = (0..DIRECT_MAX_SOURCE_HANDSHAKES)
            .map(|_| admission.reserve(first.ip(), now).unwrap())
            .collect::<Vec<_>>();
        assert!(admission.reserve(first.ip(), now).is_none());
        let second_one = admission.reserve(second.ip(), now).unwrap();
        drop(first_reservations.into_iter().next());
        assert!(admission
            .reserve(first.ip(), now + DIRECT_RATE_WINDOW)
            .is_some());
        drop(second_one);
    }

    #[test]
    fn admission_reservation_releases_handshake_and_byte_capacity() {
        let admission = Arc::new(AdmissionController {
            state: Mutex::new(AdmissionState::default()),
        });
        let mut reservations = Vec::new();
        let max_bytes_reservations = DIRECT_MAX_BYTES / ADMISSION_BYTES;
        for octet in 1..=max_bytes_reservations {
            let source = IpAddr::V6(std::net::Ipv6Addr::from(octet as u128));
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
        let admission = Arc::new(AdmissionController {
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

    #[test]
    fn admission_migrates_handshake_capacity_to_the_authenticated_node() {
        let admission = Arc::new(AdmissionController {
            state: Mutex::new(AdmissionState::default()),
        });
        let mut reservations = Vec::new();
        for octet in 1..=DIRECT_MAX_SOURCE_SESSIONS {
            let source = IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, octet as u8));
            let mut reservation = admission.reserve(source, Instant::now()).unwrap();
            admission.migrate_node(&mut reservation, "node-a").unwrap();
            reservations.push(reservation);
        }
        let mut rejected = admission
            .reserve(
                IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 1, 1)),
                Instant::now(),
            )
            .unwrap();
        assert_eq!(
            admission.migrate_node(&mut rejected, "node-a"),
            Err(TransportError::RateLimited)
        );
    }

    #[test]
    fn inbound_registration_does_not_use_static_peers_as_an_allowlist() {
        let expected = HashSet::from(["expected-peer".to_string()]);
        let status = Arc::new(Mutex::new(TransportStatus {
            enabled: true,
            listening: true,
            expected_peer_count: 1,
            connected_peer_count: 0,
            expected_connected_peer_count: 0,
            peers: Vec::new(),
            last_errors: BTreeMap::new(),
        }));
        let state = Arc::new(ConnectionState {
            local_node_id: "local-peer".to_string(),
            expected,
            stop: Arc::new(AtomicBool::new(false)),
            active: Mutex::new(HashMap::new()),
            outbox: Mutex::new(HashMap::new()),
            status: Arc::clone(&status),
            admission: Arc::new(AdmissionController {
                state: Mutex::new(AdmissionState::default()),
            }),
            reporter: None,
            workspace_root: None,
        });
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let claim = state
            .register(
                "trusted-but-not-static",
                ConnectionDirection::Responder,
                [7; 32],
                &server,
            )
            .unwrap();
        let status = status.lock().unwrap().clone();
        assert_eq!(status.connected_peer_count, 1);
        assert_eq!(status.expected_connected_peer_count, 0);
        drop(claim);
        drop(client);
    }

    #[test]
    fn blocked_resolution_returns_when_service_stop_is_requested() {
        let _test_lock = RESOLVER_TEST_LOCK.lock().unwrap();
        let resolver = Resolver::start_with_config(Some(blackhole_resolver_config())).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_resolver = Arc::clone(&stop);
        let resolver_for_request = Arc::clone(&resolver);
        let deadline = Instant::now() + Duration::from_secs(10);
        let started = Instant::now();
        let handle = thread::spawn(move || {
            resolver_for_request.resolve("blocked.invalid:7879", deadline, &stop_for_resolver)
        });
        thread::sleep(Duration::from_millis(30));
        stop.store(true, Ordering::SeqCst);
        resolver.cancel();
        assert!(handle.join().unwrap().is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        resolver.shutdown();
        assert_eq!(ACTIVE_RESOLVER_TASKS.load(Ordering::SeqCst), 0);
        assert_eq!(ACTIVE_RESOLVER_WORKERS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn timed_out_resolution_joins_all_async_work() {
        let _test_lock = RESOLVER_TEST_LOCK.lock().unwrap();
        let resolver = Resolver::start_with_config(Some(blackhole_resolver_config())).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + Duration::from_millis(50);
        let started = Instant::now();
        assert!(resolver
            .resolve("timeout.invalid:7879", deadline, &stop)
            .is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        resolver.shutdown();
        assert_eq!(ACTIVE_RESOLVER_TASKS.load(Ordering::SeqCst), 0);
        assert_eq!(ACTIVE_RESOLVER_WORKERS.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn repeated_direct_service_start_stop_does_not_accumulate_resolver_workers() {
        let _test_lock = RESOLVER_TEST_LOCK.lock().unwrap();
        use crate::node::{NodePathOverrides, NodePlatform};
        use tempfile::TempDir;

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
        NodeIdentity::load_or_initialize(&context).unwrap();
        let baseline_workers = ACTIVE_RESOLVER_WORKERS.load(Ordering::SeqCst);
        let baseline_tasks = ACTIVE_RESOLVER_TASKS.load(Ordering::SeqCst);
        for _ in 0..8 {
            let mut service = DirectService::start(
                None,
                &["zzzz@blocked.invalid:7879".to_string()],
                context.clone(),
                None,
                None,
            )
            .unwrap();
            assert_eq!(
                ACTIVE_RESOLVER_WORKERS.load(Ordering::SeqCst),
                baseline_workers + 1
            );
            service.stop();
            assert_eq!(
                ACTIVE_RESOLVER_WORKERS.load(Ordering::SeqCst),
                baseline_workers
            );
            assert_eq!(ACTIVE_RESOLVER_TASKS.load(Ordering::SeqCst), baseline_tasks);
        }
    }

    #[test]
    fn initiator_deadline_is_one_ten_second_budget() {
        let started = Instant::now();
        let deadline = initiator_deadline(started);
        assert_eq!(deadline.duration_since(started), CONNECT_TIMEOUT);
        thread::sleep(Duration::from_millis(20));
        let remaining = deadline_timeout(deadline).unwrap();
        assert!(remaining < CONNECT_TIMEOUT);
        assert!(remaining > Duration::from_secs(9));
    }

    fn blackhole_resolver_config() -> ResolverConfig {
        use hickory_resolver::config::NameServerConfig;
        use hickory_resolver::proto::xfer::Protocol;
        ResolverConfig::from_parts(
            None,
            Vec::new(),
            vec![NameServerConfig::new(
                SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1)), 53),
                Protocol::Udp,
            )],
        )
    }
}

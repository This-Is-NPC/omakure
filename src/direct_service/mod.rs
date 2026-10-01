//! Socket adapter for the direct transport core.

use crate::direct_transport::TransportError;
use crate::health_plane::report::HealthReporter;
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use admission::{AdmissionController, AdmissionState};
use connection::ConnectionState;
use dial::dialer_loop;
use resolver::Resolver;
use status::validate_static_peers;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

mod ack;
mod admission;
mod baseline;
mod connection;
mod cue;
mod dial;
mod enrollment;
mod error;
mod listener;
mod outbox;
mod resolver;
mod session;
mod status;
mod stream;

pub use baseline::{BaselineDispatcher, BaselinePushOutcome};
pub use cue::{CueDispatchOutcome, CueDispatcher, dispatch_cue};
pub use dial::probe;
pub use enrollment::request_manual_enrollment;
pub use error::DirectServiceError;
pub use listener::DirectListener;
pub use outbox::{dispatch_answer_deadline, dispatch_client_timeout};
pub use status::{
    StaticPeer, TransportPeerStatus, TransportStatus, TransportStatusHandle, parse_static_peer,
};

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
/// live, and a fifth of `IDLE_TIMEOUT`, so a peer that stays down is polled
/// rather than hammered and its failures do not drown the status.
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

    pub fn baseline_dispatcher(&self) -> BaselineDispatcher {
        BaselineDispatcher {
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
            context.clone(),
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

#[cfg(test)]
mod tests;

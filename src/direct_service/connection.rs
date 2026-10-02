use super::admission::AdmissionController;
use super::baseline::PendingBaseline;
use super::cue::PendingCue;
use super::error::DirectServiceError;
use super::outbox::{Outbox, push_pending, take_pending};
use super::status::{
    StaticPeer, TransportPeerStatus, TransportStatus, TransportStatusHandle, refresh_status,
};
use crate::direct_transport::TransportError;
use crate::health_plane::report::HealthReporter;
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::{NodeRegistry, PeerState};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::TcpStream;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ConnectionDirection {
    Initiator,
    Responder,
}

pub(super) struct ActiveConnection {
    session_id: [u8; 32],
    stream: TcpStream,
}

pub(super) struct ConnectionState {
    pub(super) local_node_id: String,
    /// Where this node's own trust registry lives.
    ///
    /// Held so anything handed to a session thread can be checked against the
    /// registry *at the moment it is asked for*, not against whatever was true
    /// when the session opened. Revocation is a local durable fact that no peer
    /// is told about, so a standing session is not evidence of trust.
    pub(super) context: NodeContext,
    /// The public half of this node's identity, which is all `open_existing`
    /// needs to reopen the registry. Kept instead of the `NodeIdentity` so the
    /// shared state never holds a private key.
    pub(super) identity_status: crate::node_identity::NodeIdentityStatus,
    pub(super) expected: HashSet<String>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) active: Mutex<HashMap<String, ActiveConnection>>,
    pub(super) status: TransportStatusHandle,
    pub(super) admission: Arc<AdmissionController>,
    /// The Performer-side Health Plane reporter, when this node serves health.
    ///
    /// `None` leaves every session behaving exactly as it did before the
    /// Health Plane existed: application frames are decrypted and discarded.
    pub(super) reporter: Option<Arc<HealthReporter>>,
    /// Root of the workspace whose scripts a Cue may name, when this node
    /// serves runs.
    ///
    /// Held as a path rather than a `Workspace` so the shared state stays
    /// cheaply shareable across session threads. `None` means an accepted Cue is
    /// decided and audited but never enqueued, which is what a node with no
    /// workspace should do: it has nothing to run.
    pub(super) workspace_root: Option<std::path::PathBuf>,
    /// Cues waiting for the session that can carry them, by peer node id.
    ///
    /// The cipher state of a live session lives in the thread holding it, so
    /// nothing outside that thread can write to a peer. A caller that wants to
    /// reach a peer this node is already connected to therefore hands the
    /// instruction here and the session thread sends it. This is what makes a
    /// Cue work in a managed fleet: dialling a second time is refused, and
    /// correctly so -- two sessions with one peer would give the Health Plane
    /// two cursors for the same node.
    pub(super) outbox: Outbox<PendingCue>,
    /// Baselines waiting for the session that can carry them, by peer node id.
    ///
    /// A separate queue from the Cue outbox rather than one queue of a sum
    /// type: the two have different in-flight state machines -- a Cue is not
    /// finished until its run outcome lands, a baseline is finished at its ack
    /// -- and folding them together would have meant reworking the Cue path
    /// that item 6 certified, to no benefit. The door into the session thread
    /// is the same door; only the queue is new.
    pub(super) baseline_outbox: Outbox<PendingBaseline>,
}

pub(super) struct ConnectionOptions {
    pub(super) stop: Arc<AtomicBool>,
    pub(super) listening: bool,
    pub(super) admission: Arc<AdmissionController>,
    pub(super) reporter: Option<Arc<HealthReporter>>,
    pub(super) workspace_root: Option<std::path::PathBuf>,
}

impl ConnectionState {
    pub(super) fn new(
        context: NodeContext,
        identity: &NodeIdentity,
        static_peers: &[StaticPeer],
        options: ConnectionOptions,
    ) -> Arc<Self> {
        let expected = static_peers
            .iter()
            .map(|peer| peer.node_id.clone())
            .collect::<HashSet<_>>();
        let status = Arc::new(Mutex::new(TransportStatus {
            enabled: true,
            listening: options.listening,
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
            context,
            identity_status: identity.public_status().clone(),
            expected,
            stop: options.stop,
            active: Mutex::new(HashMap::new()),
            outbox: Mutex::new(HashMap::new()),
            baseline_outbox: Mutex::new(HashMap::new()),
            status,
            admission: options.admission,
            reporter: options.reporter,
            workspace_root: options.workspace_root,
        })
    }

    pub(super) fn status(&self) -> TransportStatusHandle {
        Arc::clone(&self.status)
    }

    pub(super) fn should_initiate(&self, remote_node_id: &str) -> bool {
        self.local_node_id.as_str() < remote_node_id
    }

    /// Refuse anything aimed at a peer this node does not trust *right now*.
    ///
    /// The sender is the only place this can be enforced. Every receiving gate
    /// is fail-closed against the receiver's own registry, which is correct and
    /// is also why it cannot help here: revocation is local durable state and
    /// the revoked node is never told, so it goes on seeing the revoker as an
    /// active peer and goes on honouring what it is asked to do. A standing
    /// session is not evidence of trust either -- it was authorized when it
    /// opened and nothing re-checks it -- so the registry is read here, at the
    /// moment of the ask.
    ///
    /// The codes are the frozen table's own, chosen the same way
    /// `authorize_peer` chooses them: `revoked` for a withdrawn peer, and
    /// `not_enrolled` for one that is unknown, still pending, or suspended. A
    /// registry that will not open is `internal` and still a refusal: a node
    /// that cannot read its own trust state cannot claim a peer is trusted.
    pub(super) fn require_active_peer(&self, peer_node_id: &str) -> Result<(), DirectServiceError> {
        let refuse = |state: &'static str, protocol: TransportError| {
            Err(DirectServiceError::PeerNotActive {
                peer_node_id: peer_node_id.to_string(),
                state,
                protocol,
            })
        };
        let Ok(registry) = NodeRegistry::open_existing(&self.context, &self.identity_status) else {
            return refuse("unreadable", TransportError::Internal);
        };
        match registry.peer(peer_node_id) {
            Ok(Some(peer)) => match peer.state {
                PeerState::Active => Ok(()),
                PeerState::Revoked => refuse("revoked", TransportError::Revoked),
                PeerState::Pending => refuse("pending", TransportError::NotEnrolled),
                PeerState::Suspended => refuse("suspended", TransportError::NotEnrolled),
            },
            Ok(None) => refuse("absent", TransportError::NotEnrolled),
            Err(_) => refuse("unreadable", TransportError::Internal),
        }
    }

    /// Hand a Cue to whichever thread holds the session with this peer.
    ///
    /// Refused when there is no live session: a caller must not be told its
    /// instruction is on its way when nothing can carry it.
    pub(super) fn enqueue_cue(
        &self,
        peer_node_id: &str,
        pending: PendingCue,
    ) -> Result<(), TransportError> {
        self.require_session(peer_node_id)?;
        push_pending(&self.outbox, peer_node_id, pending)
    }

    /// The next Cue this session should carry, if any.
    pub(super) fn take_pending_cue(&self, peer_node_id: &str) -> Option<PendingCue> {
        take_pending(&self.outbox, peer_node_id)
    }

    /// Hand a baseline to whichever thread holds the session with this peer.
    ///
    /// Refused when there is no live session, for the reason the Cue outbox
    /// exists at all: the cipher state of a live session lives in the thread
    /// holding it, a second dial to the same peer is refused by `register`,
    /// and telling a caller its baseline is on its way when nothing can carry
    /// it would be a lie.
    pub(super) fn enqueue_baseline(
        &self,
        peer_node_id: &str,
        pending: PendingBaseline,
    ) -> Result<(), TransportError> {
        self.require_session(peer_node_id)?;
        push_pending(&self.baseline_outbox, peer_node_id, pending)
    }

    /// The next baseline this session should carry, if any.
    pub(super) fn take_pending_baseline(&self, peer_node_id: &str) -> Option<PendingBaseline> {
        take_pending(&self.baseline_outbox, peer_node_id)
    }

    /// Whether a live session with this peer exists to carry an instruction.
    pub(super) fn holds_session(&self, peer_node_id: &str) -> bool {
        self.active
            .lock()
            .map(|active| active.contains_key(peer_node_id))
            .unwrap_or(false)
    }

    fn require_session(&self, peer_node_id: &str) -> Result<(), TransportError> {
        let active = self.active.lock().map_err(|_| TransportError::Internal)?;
        if active.contains_key(peer_node_id) {
            Ok(())
        } else {
            Err(TransportError::NotEnrolled)
        }
    }

    /// Fail every Cue still waiting on a session that just ended.
    ///
    /// Dropping the reply channel is what unblocks the caller; without this a
    /// request would wait out its whole budget for a session that is gone.
    pub(super) fn drain_outbox(&self, peer_node_id: &str) {
        if let Ok(mut outbox) = self.outbox.lock() {
            outbox.remove(peer_node_id);
        }
        if let Ok(mut outbox) = self.baseline_outbox.lock() {
            outbox.remove(peer_node_id);
        }
    }

    pub(super) fn register(
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

    pub(super) fn record_direct_error(&self, node_id: &str, error: &DirectServiceError) {
        if let DirectServiceError::CueEnqueueFailed { .. } = error {
            if let Ok(mut status) = self.status.lock() {
                status
                    .last_errors
                    .insert(node_id.to_string(), "cue_enqueue_failed".to_string());
            }
            return;
        }
        let transport = match error {
            DirectServiceError::Protocol(error) => TransportError::from_code(error.code()),
            DirectServiceError::PeerNotActive { protocol, .. } => {
                TransportError::from_code(protocol.code())
            }
            _ => TransportError::Internal,
        };
        self.record_error(node_id, &transport);
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

    pub(super) fn close_active(&self) {
        let Ok(active) = self.active.lock() else {
            return;
        };
        for connection in active.values() {
            let _ = connection.stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

pub(super) struct ConnectionClaim {
    state: Arc<ConnectionState>,
    remote_node_id: String,
    session_id: [u8; 32],
}

impl Drop for ConnectionClaim {
    fn drop(&mut self) {
        self.state.unregister(&self.remote_node_id, self.session_id);
    }
}

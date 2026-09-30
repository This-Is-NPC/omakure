use crate::direct_transport::TransportError;
use crate::node_registry::RegistryError;
use crate::node_transport::NodeTransportError;
use std::io;
use thiserror::Error;

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
    /// Refused before anything reached the wire, because the target is not an
    /// active peer in *this* node's registry.
    ///
    /// Carries the peer and the state it was found in, because "refused" on its
    /// own is not something an operator can act on: a peer that was revoked and
    /// a peer that was never enrolled need different answers.
    #[error(
        "direct transport refused {peer_node_id}: this node's registry has it {state}, \
         not active ({protocol})"
    )]
    PeerNotActive {
        peer_node_id: String,
        state: &'static str,
        protocol: TransportError,
    },
    /// A receiver authorized a Cue but could not persist its run locally.
    ///
    /// This is intentionally not a protocol error: no peer acknowledgement is
    /// sent for it. The owning service records the stable local failure instead
    /// of turning it into the same unanswered result used for silent refusal.
    #[error("direct transport Cue enqueue failed locally: {error}")]
    CueEnqueueFailed {
        error: crate::remote_cue::CueEnqueueError,
    },
}

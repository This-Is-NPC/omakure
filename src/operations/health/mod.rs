//! Protocol-neutral Health Plane operations.
//!
//! Three responsibilities, all deliberately adapter-free:
//!
//! 1. [`fleet_status`] projects the Conductor-local Health Plane state that the
//!    Wave 2 shared operations own into one bounded, redacted report. The CLI
//!    and the HTTP route both render exactly this value, which is what makes
//!    them return identical status.
//! 2. [`signal_feed`] projects the bounded, newest-first closed Signal feed
//!    the same way, merging the per-Performer inbox with the Conductor-local
//!    lifecycle projection.
//! 3. [`NodeHealthFacts`] reads the live local node so a Performer can report
//!    Profile, Pulse, and `run-completed`. It reads the local node only;
//!    nothing here consults a peer message, and nothing here can mutate trust.
//!
//! Every bound below is transcribed from `docs/internal/health-plane-contract.md` via
//! `crate::health_plane::bounds`. None of them is chosen here.

mod facts;
mod fleet;
mod signals;

pub use facts::NodeHealthFacts;
pub use fleet::{fleet_status, BaselineCounts, FleetStatusReport, PresenceCounts};
pub use signals::{signal_feed, SignalCursor, SignalEntry, SignalFeedReport};

use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::NodeRegistry;
use crate::operations::node::{map_identity_error, map_registry_error, registry_error};
use crate::operations::OperationResult;

/// Open the observational registry for one-shot CLI reads.
///
/// Long-lived HTTP surfaces must reuse the process-owned registry instead of
/// calling this on every request.
pub(crate) fn open_observational_registry(context: &NodeContext) -> OperationResult<NodeRegistry> {
    let state_present = context
        .validate_existing_state_contents()
        .map_err(crate::operations::node::map_node_error)?;
    if !state_present {
        return Err(registry_error(crate::node::STATE_NOT_INITIALIZED));
    }
    let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
    NodeRegistry::open_health_observational(context, identity.public_status())
        .map_err(map_registry_error)
}

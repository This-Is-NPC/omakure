//! Bounded Health Plane persistence owned by the node registry.
//!
//! Every statement in this module touches Health Plane tables only.  There is
//! no code path from a Health Plane message to identity, trust, capability,
//! revocation, transport session, or run state, and the module never creates a
//! second database, a generic repository, an event bus, a metric store, or a
//! historical query engine.

mod apply;
mod audit;
mod evaluate;
mod feed;
mod outbox;
mod prune;
mod rows;
mod store;
mod types;

pub(crate) use audit::HealthAuditRecord;
pub use types::{
    HealthAuditEvent, HealthAuthorization, HealthFeedPeer, HealthFeedSignal, HealthFleetPeer,
    HealthOutboxEntry, HealthPeerSnapshot, HealthPeerState, HealthPruneReport, HealthSignalFeed,
};

#[cfg(test)]
mod tests;

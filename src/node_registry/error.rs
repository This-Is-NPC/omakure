use super::types::PeerState;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("node registry I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("node registry SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("node registry node error: {0}")]
    Node(#[from] crate::node::NodeError),
    #[error("node registry input is invalid: {0}")]
    InvalidInput(String),
    #[error("node registry schema is invalid: {0}")]
    InvalidSchema(String),
    #[error("node registry is corrupt: {0}")]
    Corrupt(String),
    #[error("peer already exists or conflicts with existing state: {0}")]
    Duplicate(String),
    #[error("peer cannot trust itself")]
    SelfTrust,
    #[error("peer has a retained revocation and cannot be resurrected: {0}")]
    Revoked(String),
    #[error("invalid trust transition from {from} to {to}")]
    InvalidTransition { from: PeerState, to: PeerState },
    #[error("peer was not found: {0}")]
    NotFound(String),
    #[error("peer update would not change state: {0}")]
    Unchanged(String),
    #[error("transport audit capacity is exhausted")]
    AuditCapacity,
    #[error("manual enrollment request was replayed")]
    EnrollmentReplay,
    #[error("manual enrollment request conflicts with existing trust state")]
    EnrollmentConflict,
    #[error("manual enrollment replay capacity is exhausted")]
    EnrollmentCapacity,
    #[error("manual enrollment evidence does not match staged state")]
    EnrollmentMismatch,
    #[error("signed enrollment bundle was replayed")]
    BundleReplay,
    #[error("signed enrollment bundle conflicts with existing trust state")]
    BundleConflict,
    #[error("signed enrollment replay capacity is exhausted")]
    BundleCapacity,
    #[error("signed enrollment bundle rate limit exceeded")]
    BundleRateLimited,
    #[error("signed enrollment bootstrap proof was already consumed")]
    BootstrapProofConsumed,
    #[error("an active conductor already exists")]
    ConductorConflict,
    #[error("a baseline publisher cannot also be a conductor")]
    PublisherConductorConflict,
}

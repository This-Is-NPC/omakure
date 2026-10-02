//! The node-owned trust and delivery persistence boundary.
//!
//! This module intentionally owns `node.sqlite` exclusively.  It does not
//! import or call the run-history repository, and it contains no transport or
//! enrollment behavior.  Trust changes are explicit, transactional operations
//! with an actor and reason recorded in the append-only audit log.

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod audit;
mod bundle;
mod error;
mod fields;
mod manual_enrollment;
mod open;
mod peers;
mod projection;
mod schema;
mod types;
mod validate;

pub mod health;

pub(crate) use audit::{CueAudit, TransportAudit};
pub(crate) use bundle::PendingBootstrapCleanup;
pub use error::RegistryError;
pub use types::{
    AuditEvent, PeerCounts, PeerRecord, PeerRegistration, PeerRole, PeerSource, PeerState,
    RevocationRecord, TransportPeer,
};

pub const SCHEMA_VERSION: i64 = 8;
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_ACTOR_BYTES: usize = 256;
const MAX_REASON_BYTES: usize = 1024;
const MAX_CAPABILITIES_JSON_BYTES: usize = 4096;
const TOO_MANY_CAPABILITIES: &str = "too many peer capabilities";
const PUBLIC_KEY_BYTES: usize = 64;
const TRANSPORT_CERTIFICATE_BYTES: usize = 245;
const MAX_TRANSPORT_AUDIT_ROWS: i64 = 1_000_000;
/// Newest audit rows scanned when projecting Conductor-local lifecycle
/// Signals. The projection itself is bounded by the frozen Signal capacity;
/// this only bounds how far back a single read may look.
const MAX_LIFECYCLE_SCAN_ROWS: usize = 4_096;
const HEALTH_PLANE_ENABLED: &str = "enabled";
const MAX_ENROLLMENT_REPLAY_ROWS: i64 = 1_000_000;
const MAX_ENROLLMENT_AUDIT_ROWS: i64 = 1_000_000;
const MAX_ENROLLMENT_REQUEST_ROWS: i64 = 1_000_000;
const MAX_BOOTSTRAP_PROOF_ROWS: i64 = 1_000_000;
const MAX_ENROLLMENT_CLEANUP_ROWS: i64 = 10_000;
const MAX_BUNDLE_ACTIVATIONS_PER_MINUTE: i64 = 4;

#[derive(Debug, Clone)]
pub struct NodeRegistry {
    path: PathBuf,
    /// Consulted, never written. A node is a baseline publisher exactly when
    /// this file exists, so the trust writes below read the same fact the
    /// publisher key custody enforces rather than a copy of it that could drift.
    publisher_key_path: PathBuf,
    local_node_id: String,
    local_public_key: String,
    /// Observational opens must not create the schema or change journal mode.
    schema_mutation_allowed: bool,
    /// Reused for observational reads so HTTP health surfaces do not open SQLite
    /// per request (Windows lock races against the ingest writer).
    read_connection: Option<Arc<Mutex<Connection>>>,
}

impl NodeRegistry {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn local_node_id(&self) -> &str {
        &self.local_node_id
    }
}

#[cfg(test)]
mod tests;

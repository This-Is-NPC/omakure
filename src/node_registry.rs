//! The node-owned trust and delivery persistence boundary.
//!
//! This module intentionally owns `node.sqlite` exclusively.  It does not
//! import or call the run-history repository, and it contains no transport or
//! enrollment behavior.  Trust changes are explicit, transactional operations
//! with an actor and reason recorded in the append-only audit log.

use crate::direct_transport::TransportCertificate;
use crate::enrollment::{self, ManualEnrollmentRequest, SignedEnrollmentBundle};
use crate::node::NodeContext;
use crate::node_identity::{node_id_for_x_only_public_key, NodeIdentityStatus};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

pub mod health;

pub const SCHEMA_VERSION: i64 = 8;
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_ACTOR_BYTES: usize = 256;
const MAX_REASON_BYTES: usize = 1024;
const MAX_CAPABILITIES: usize = 32;
const MAX_CAPABILITY_BYTES: usize = 64;
const MAX_CAPABILITIES_JSON_BYTES: usize = 4096;
const NODE_ID_BYTES: usize = 69;
const PUBLIC_KEY_BYTES: usize = 64;
const TRANSPORT_CERTIFICATE_BYTES: usize = 245;
const MAX_TRANSPORT_AUDIT_ROWS: i64 = 1_000_000;
/// Newest audit rows scanned when projecting Conductor-local lifecycle
/// Signals. The projection itself is bounded by the frozen Signal capacity;
/// this only bounds how far back a single read may look.
const MAX_LIFECYCLE_SCAN_ROWS: usize = 4_096;
const HEALTH_PLANE_DISABLED: &str = "disabled";
/// Test-only fault injection for the Health Plane migration, so the
/// "migration failed, keep serving with the plane disabled" path can be proven
/// without leaving unexpected schema objects behind.
#[cfg(test)]
static HEALTH_MIGRATION_FAULT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
const HEALTH_PLANE_ENABLED: &str = "enabled";
const MAX_ENROLLMENT_REPLAY_ROWS: i64 = 1_000_000;
const MAX_ENROLLMENT_AUDIT_ROWS: i64 = 1_000_000;
const MAX_ENROLLMENT_REQUEST_ROWS: i64 = 1_000_000;
const MAX_BOOTSTRAP_PROOF_ROWS: i64 = 1_000_000;
const MAX_ENROLLMENT_CLEANUP_ROWS: i64 = 10_000;
const MAX_BUNDLE_ACTIVATIONS_PER_MINUTE: i64 = 4;
type StagedManualEnrollment = (
    Option<Vec<u8>>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    i64,
    i64,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    i64,
    i64,
    i64,
    String,
    String,
);

const SUPPORTED_CAPABILITIES: &[&str] = &[
    "backup-orchestration",
    "baseline-push",
    "inventory-health",
    "lost-device-revocation",
    "notifications",
    "remote-run",
    "ssh-credential-rotation",
];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerRole {
    Conductor,
    Performer,
}

impl PeerRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Conductor => "conductor",
            Self::Performer => "performer",
        }
    }

    fn parse(value: &str) -> Result<Self, RegistryError> {
        match value {
            "conductor" => Ok(Self::Conductor),
            "performer" => Ok(Self::Performer),
            _ => Err(RegistryError::InvalidSchema(format!(
                "unknown peer role {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    Pending,
    Active,
    Suspended,
    Revoked,
}

impl PeerState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Revoked => "revoked",
        }
    }

    fn parse(value: &str) -> Result<Self, RegistryError> {
        match value {
            "pending" => Ok(Self::Pending),
            "active" => Ok(Self::Active),
            "suspended" => Ok(Self::Suspended),
            "revoked" => Ok(Self::Revoked),
            _ => Err(RegistryError::InvalidSchema(format!(
                "unknown peer state {value:?}"
            ))),
        }
    }
}

impl std::fmt::Display for PeerState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSource {
    Manual,
    Bundle,
    Recovery,
}

impl PeerSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Bundle => "bundle",
            Self::Recovery => "recovery",
        }
    }

    fn parse(value: &str) -> Result<Self, RegistryError> {
        match value {
            "manual" => Ok(Self::Manual),
            "bundle" => Ok(Self::Bundle),
            "recovery" => Ok(Self::Recovery),
            _ => Err(RegistryError::InvalidSchema(format!(
                "unknown peer source {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRegistration {
    pub node_id: String,
    pub public_key: String,
    pub role: PeerRole,
    pub capabilities: Vec<String>,
    pub source: PeerSource,
    pub actor: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRecord {
    pub node_id: String,
    pub public_key: String,
    pub role: PeerRole,
    pub state: PeerState,
    pub capabilities: Vec<String>,
    pub added_at: String,
    pub updated_at: String,
    pub last_seen: Option<String>,
    pub source: PeerSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportPeer {
    pub node_id: String,
    pub identity_key: [u8; 32],
    pub transport_public_key: Option<[u8; 32]>,
    pub key_epoch: Option<u64>,
    pub state: PeerState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCounts {
    pub total: usize,
    pub active: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationRecord {
    pub id: i64,
    pub node_id: String,
    pub public_key: String,
    pub revoked_at: String,
    pub reason: String,
    pub replacement_node_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    pub id: i64,
    pub event_type: String,
    pub node_id: String,
    pub from_state: Option<PeerState>,
    pub to_state: Option<PeerState>,
    pub actor: String,
    pub reason: String,
    pub occurred_at: String,
}

#[derive(Debug, Clone)]
pub struct NodeRegistry {
    path: PathBuf,
    /// Consulted, never written. A node is a baseline publisher exactly when
    /// this file exists, so the trust writes below read the same fact the
    /// publisher key custody enforces rather than a copy of it that could drift.
    publisher_key_path: PathBuf,
    local_node_id: String,
    local_public_key: String,
    /// Observational opens must not migrate the schema or change journal mode.
    schema_mutation_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingBootstrapCleanup {
    pub organization: String,
    pub token_hash: [u8; 32],
    pub nonce_hash: [u8; 32],
    pub bundle_id: [u8; 16],
}

impl NodeRegistry {
    /// Open and validate the node-owned database for the supplied public identity.
    pub(crate) fn open(
        context: &NodeContext,
        identity: &NodeIdentityStatus,
    ) -> Result<Self, RegistryError> {
        Self::open_with_mode(context, identity, false)
    }

    pub(crate) fn open_for_initialization(
        context: &NodeContext,
        identity: &NodeIdentityStatus,
    ) -> Result<Self, RegistryError> {
        Self::open_with_mode(context, identity, true)
    }

    fn open_with_mode(
        context: &NodeContext,
        identity: &NodeIdentityStatus,
        allow_create: bool,
    ) -> Result<Self, RegistryError> {
        context.ensure_state_directory()?;
        let path = context.database_path();
        let database_existed = std::fs::symlink_metadata(&path).is_ok();
        if !database_existed && !allow_create {
            return Err(RegistryError::NotFound(
                "node trust registry is not initialized".to_string(),
            ));
        }
        let sidecars_existed = database_sidecar_presence(&path);
        let (node_id, public_key) = validate_identity(identity)?;
        let registry = Self {
            path,
            publisher_key_path: context.publisher_key_path(),
            local_node_id: node_id,
            local_public_key: public_key,
            schema_mutation_allowed: true,
        };
        registry.with_connection(|connection| {
            if !database_existed {
                set_new_database_mode(&registry.path)?;
            }
            for (sidecar, existed) in database_sidecar_paths(&registry.path)
                .into_iter()
                .zip(sidecars_existed)
            {
                if !existed && sidecar.exists() {
                    set_new_database_mode(&sidecar)?;
                }
            }
            validate_database_security(context, &registry.path)?;
            initialize_database(connection, &registry)
        })?;
        Ok(registry)
    }
    /// Open and validate an existing registry without creating state or
    /// mutating its schema. Schema creation and migrations belong to the
    /// serialized node initialization path (`open`).
    pub fn open_existing(
        context: &NodeContext,
        identity: &NodeIdentityStatus,
    ) -> Result<Self, RegistryError> {
        if !context.validate_existing_state_directory()? {
            return Err(RegistryError::NotFound(
                "node state is not initialized".to_string(),
            ));
        }
        let path = context.database_path();
        match std::fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() =>
            {
                return Err(RegistryError::InvalidSchema(
                    "node.sqlite has an unexpected file type".to_string(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(RegistryError::NotFound(
                    "node trust registry is not initialized".to_string(),
                ));
            }
            Err(error) => return Err(error.into()),
        }
        let (node_id, public_key) = validate_identity(identity)?;
        validate_database_security(context, &path)?;
        let registry = Self {
            path,
            publisher_key_path: context.publisher_key_path(),
            local_node_id: node_id,
            local_public_key: public_key,
            schema_mutation_allowed: false,
        };
        registry.with_connection(|connection| {
            validate_database_security(context, &registry.path)?;
            validate_existing_database(connection, &registry)
        })?;
        Ok(registry)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether this node holds the key that signs baselines.
    ///
    /// Item 6 built a Cue that "names a script and never carries one" so that
    /// ordering execution and supplying what gets executed are two different
    /// powers. A node that could do both would hand a single compromise the
    /// ability to write a script and then order every Performer to run it,
    /// which is the separation gone. So the two are refused together here, in
    /// the store that records trust, rather than left to whoever wires the
    /// commands.
    fn holds_publisher_key(&self) -> bool {
        std::fs::symlink_metadata(&self.publisher_key_path)
            .is_ok_and(|metadata| metadata.file_type().is_file())
    }

    /// Refuse to record a Performer peer — this node acting as their Conductor
    /// — while this node also publishes baselines.
    ///
    /// Called inside each trust transaction rather than before it, so a
    /// publisher key appearing concurrently cannot slip between the check and
    /// the write: the key is on disk before `BaselinePublisher::create` opens
    /// its own transaction, and SQLite serializes the two.
    fn reject_publisher_conflict(&self, role: PeerRole) -> Result<(), RegistryError> {
        if role == PeerRole::Performer && self.holds_publisher_key() {
            return Err(RegistryError::PublisherConductorConflict);
        }
        Ok(())
    }

    /// Refuse to become a baseline publisher while this node holds Conductor
    /// authority over anyone.
    ///
    /// The other direction of the same rule. "Conductor authority" is any peer
    /// recorded as a Performer that has not been revoked: a suspended peer can
    /// be reactivated and a pending one approved, so only revocation actually
    /// ends the relationship.
    pub(crate) fn reject_conductor_authority(&self) -> Result<(), RegistryError> {
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let holds: i64 = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM peers WHERE role = 'performer' AND state <> 'revoked')",
                [],
                |row| row.get(0),
            )?;
            transaction.commit()?;
            if holds != 0 {
                return Err(RegistryError::PublisherConductorConflict);
            }
            Ok(())
        })
    }

    pub(crate) fn bootstrap_proof_consumed(
        &self,
        organization: &str,
    ) -> Result<bool, RegistryError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM bootstrap_proofs
                         WHERE target_node_id = ?1 AND organization = ?2 AND consumed_at IS NOT NULL
                     )",
                    params![&self.local_node_id, organization],
                    |row| row.get::<_, i64>(0),
                )
                .map(|value| value != 0)
                .map_err(RegistryError::from)
        })
    }

    pub(crate) fn pending_bootstrap_cleanups(
        &self,
        organization: &str,
        limit: usize,
    ) -> Result<Vec<PendingBootstrapCleanup>, RegistryError> {
        let query_limit = i64::try_from(limit.saturating_add(1))
            .map_err(|_| RegistryError::InvalidInput("cleanup limit is too large".to_string()))?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT organization, token_hash, nonce_hash, bundle_id
                 FROM bootstrap_proofs
                 WHERE target_node_id = ?1 AND organization = ?2
                   AND consumed_at IS NOT NULL AND cleanup_state = 'pending'
                 ORDER BY consumed_at, bundle_id
                 LIMIT ?3",
            )?;
            let rows = statement
                .query_map(
                    params![&self.local_node_id, organization, query_limit],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Vec<u8>>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                            row.get::<_, Vec<u8>>(3)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let rows = rows
                .into_iter()
                .map(|(organization, token_hash, nonce_hash, bundle_id)| {
                    Ok(PendingBootstrapCleanup {
                        organization,
                        token_hash: token_hash.try_into().map_err(|_| {
                            RegistryError::InvalidSchema(
                                "pending bootstrap token hash has invalid length".to_string(),
                            )
                        })?,
                        nonce_hash: nonce_hash.try_into().map_err(|_| {
                            RegistryError::InvalidSchema(
                                "pending bootstrap nonce hash has invalid length".to_string(),
                            )
                        })?,
                        bundle_id: bundle_id.try_into().map_err(|_| {
                            RegistryError::InvalidSchema(
                                "pending bootstrap bundle ID has invalid length".to_string(),
                            )
                        })?,
                    })
                })
                .collect::<Result<Vec<_>, RegistryError>>()?;
            if rows.len() > limit {
                return Err(RegistryError::InvalidSchema(
                    "too many pending bootstrap cleanups".to_string(),
                ));
            }
            Ok(rows)
        })
    }

    pub(crate) fn complete_bootstrap_cleanup(
        &self,
        cleanup: &PendingBootstrapCleanup,
        bundle_digest: Option<&[u8; 32]>,
    ) -> Result<(), RegistryError> {
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let updated = transaction.execute(
                "UPDATE bootstrap_proofs
                 SET cleanup_state = 'complete'
                 WHERE target_node_id = ?1 AND organization = ?2
                   AND token_hash = ?3 AND nonce_hash = ?4
                   AND bundle_id = ?5 AND consumed_at IS NOT NULL
                   AND cleanup_state = 'pending'",
                params![
                    &self.local_node_id,
                    &cleanup.organization,
                    cleanup.token_hash.as_slice(),
                    cleanup.nonce_hash.as_slice(),
                    cleanup.bundle_id.as_slice(),
                ],
            )?;
            if updated != 1 {
                return Err(RegistryError::InvalidSchema(
                    "pending bootstrap cleanup disappeared".to_string(),
                ));
            }
            record_enrollment_audit_tx(
                &transaction,
                "cleanup_completed",
                Some(&cleanup.bundle_id),
                bundle_digest,
                &self.local_node_id,
                "accepted",
                "bootstrap token cleanup completed",
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn local_node_id(&self) -> &str {
        &self.local_node_id
    }

    /// Insert only a pending peer.  Observation, discovery, endpoints, and
    /// matching identifiers have no API that can insert active trust.
    pub fn register_pending(
        &self,
        registration: PeerRegistration,
    ) -> Result<PeerRecord, RegistryError> {
        self.register_pending_with_transport(registration, None)
    }

    pub fn register_pending_with_transport(
        &self,
        registration: PeerRegistration,
        certificate: Option<&[u8]>,
    ) -> Result<PeerRecord, RegistryError> {
        validate_registration(&registration, &self.local_node_id, &self.local_public_key)?;
        let now = now_timestamp();
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            reject_retained_revocation(
                &transaction,
                &registration.node_id,
                &registration.public_key,
            )?;
            if peer_exists(&transaction, &registration.node_id)? {
                return Err(RegistryError::Duplicate(registration.node_id.clone()));
            }
            if public_key_exists(&transaction, &registration.public_key)? {
                return Err(RegistryError::Duplicate(registration.public_key.clone()));
            }
            self.reject_publisher_conflict(registration.role)?;
            transaction.execute(
                "INSERT INTO peers (node_id, public_key, role, state, capabilities_json, added_at, updated_at, last_seen, source)
                 VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?5, NULL, ?6)",
                params![
                    registration.node_id,
                    registration.public_key,
                    registration.role.as_str(),
                    capabilities_json(&registration.capabilities)?,
                    now,
                    registration.source.as_str(),
                ],
            )?;
            insert_v2_identity_projection(
                &transaction,
                &registration,
                timestamp_seconds(&now)?,
                "authenticated_untrusted",
            )?;
            if let Some(certificate) = certificate {
                insert_v2_pending_transport_projection(
                    &transaction,
                    &registration,
                    timestamp_seconds(&now)?,
                    certificate,
                )?;
            }
            record_audit(
                &transaction,
                AuditInput {
                    event_type: "peer_registered",
                    node_id: &registration.node_id,
                    from_state: None,
                    to_state: Some(PeerState::Pending),
                    actor: &registration.actor,
                    reason: &registration.reason,
                    occurred_at: &now,
                },
            )?;
            let peer = load_peer(&transaction, &registration.node_id)?
                .ok_or_else(|| RegistryError::Corrupt("inserted peer disappeared".to_string()))?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    pub fn stage_manual_enrollment(
        &self,
        request: &ManualEnrollmentRequest,
        certificate: &[u8],
        actor: &str,
        reason: &str,
        now: u64,
    ) -> Result<PeerRecord, RegistryError> {
        if let Err(error) = request.verify(now) {
            self.record_enrollment_audit(
                if matches!(error, crate::enrollment::EnrollmentError::Replay) {
                    "replay"
                } else if matches!(error, crate::enrollment::EnrollmentError::Expired) {
                    "expired"
                } else {
                    "malformed"
                },
                Some(&request.request_id),
                None,
                &self.local_node_id,
                "rejected",
                "request verification failed",
            )?;
            return Err(RegistryError::InvalidInput(error.to_string()));
        }
        if request.proposer_node_id == self.local_node_id {
            self.record_enrollment_audit(
                "self_request",
                Some(&request.request_id),
                Some(&digest(&request.encode())),
                &request.proposer_node_id,
                "rejected",
                "a node may stage only a remote enrollment request",
            )?;
            return Err(RegistryError::SelfTrust);
        }
        validate_actor_reason(actor, reason)?;
        let registration = registration_from_manual(request, actor, reason)?;
        let certificate = TransportCertificate::from_bytes(certificate)
            .map_err(|_| RegistryError::InvalidInput("transport certificate is invalid".into()))?;
        certificate
            .verify_time(now)
            .map_err(|_| RegistryError::InvalidInput("transport certificate is expired".into()))?;
        if certificate.node_id() != registration.node_id
            || certificate.identity_key().as_slice()
                != decode_hex(&registration.public_key)?.as_slice()
            || certificate.transport_public() != &request.proposer_transport_x25519
        {
            return Err(RegistryError::InvalidInput(
                "transport certificate does not match manual enrollment identity".into(),
            ));
        }
        let first_seen = i64::try_from(now)
            .map_err(|_| RegistryError::InvalidInput("enrollment timestamp is too large".into()))?;
        let expires_at = i64::try_from(request.replay_expiry())
            .map_err(|_| RegistryError::InvalidInput("enrollment expiry is too large".into()))?;
        let request_bytes = request.encode();
        let request_digest = digest(&request_bytes);
        let certificate_digest = digest(certificate.as_bytes());
        let capabilities = capabilities_json(&registration.capabilities)?.into_bytes();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            cleanup_enrollment_replays(&transaction, now)?;
            let replay_count: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM enrollment_replays WHERE replay_kind = 'manual_request'",
                [],
                |row| row.get(0),
            )?;
            if replay_count >= MAX_ENROLLMENT_REPLAY_ROWS {
                return Err(RegistryError::EnrollmentCapacity);
            }
            let request_count: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM manual_enrollment_requests WHERE state = 'pending'",
                [],
                |row| row.get(0),
            )?;
            if request_count >= MAX_ENROLLMENT_REQUEST_ROWS {
                return Err(RegistryError::EnrollmentCapacity);
            }
            if transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM enrollment_replays WHERE replay_kind = 'manual_request' AND replay_id = ?1)",
                [&request.request_id[..]],
                |row| row.get::<_, i64>(0),
            )? != 0 {
                record_enrollment_audit_tx(
                    &transaction,
                    "replay",
                    Some(&request.request_id),
                    Some(&request_digest),
                    &registration.node_id,
                    "rejected",
                    "request replay was already retained",
                )?;
                return Err(RegistryError::EnrollmentReplay);
            }
            reject_retained_revocation(
                &transaction,
                &registration.node_id,
                &registration.public_key,
            )?;
            if let Some((state, source)) = transaction
                .query_row(
                    "SELECT state, source FROM peers WHERE node_id = ?1",
                    [&registration.node_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?
            {
                let error = if source == "manual" && state == "pending" {
                    RegistryError::EnrollmentReplay
                } else {
                    RegistryError::EnrollmentConflict
                };
                record_enrollment_audit_tx(
                    &transaction,
                    if matches!(error, RegistryError::EnrollmentReplay) {
                        "replay"
                    } else {
                        "concurrent"
                    },
                    Some(&request.request_id),
                    Some(&request_digest),
                    &registration.node_id,
                    "rejected",
                    "request conflicts with existing state",
                )?;
                return Err(error);
            }
            if public_key_exists(&transaction, &registration.public_key)? {
                record_enrollment_audit_tx(
                    &transaction,
                    "concurrent",
                    Some(&request.request_id),
                    Some(&request_digest),
                    &registration.node_id,
                    "rejected",
                    "request identity conflicts with existing state",
                )?;
                return Err(RegistryError::EnrollmentConflict);
            }
            self.reject_publisher_conflict(registration.role)?;
            transaction.execute(
                "INSERT INTO peers (node_id, public_key, role, state, capabilities_json, added_at, updated_at, last_seen, source)
                 VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?5, NULL, 'manual')",
                params![
                    registration.node_id,
                    registration.public_key,
                    registration.role.as_str(),
                    capabilities_json(&registration.capabilities)?,
                    now_timestamp(),
                ],
            )?;
            insert_v2_identity_projection(
                &transaction,
                &registration,
                first_seen,
                "authenticated_untrusted",
            )?;
            insert_v2_pending_transport_projection(
                &transaction,
                &registration,
                first_seen,
                certificate.as_bytes(),
            )?;
            transaction.execute(
                "INSERT INTO manual_enrollment_requests
                 (pairing_id, request_id, request_bytes, request_digest, code_hash, node_id, identity_key,
                  transport_key, role, capabilities, request_created_at, request_expires_at,
                  certificate, certificate_digest, certificate_id, key_epoch, not_before, not_after,
                  state, source, staged_at, resolved_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                         'pending', 'manual', ?19, NULL)",
                params![
                    &request.pairing_id[..],
                    &request.request_id[..],
                    &request_bytes,
                    &request_digest[..],
                    &request.code_hash[..],
                    &registration.node_id,
                    &request.proposer_xonly[..],
                    &request.proposer_transport_x25519[..],
                    request.role as u8,
                    capabilities,
                    i64::try_from(request.created_at).map_err(|_| RegistryError::InvalidInput("request timestamp is too large".into()))?,
                    i64::try_from(request.expires_at).map_err(|_| RegistryError::InvalidInput("request timestamp is too large".into()))?,
                    certificate.as_bytes(),
                    &certificate_digest[..],
                    certificate.certificate_id().as_slice(),
                    i64::try_from(certificate.key_epoch()).map_err(|_| RegistryError::InvalidInput("certificate epoch is too large".into()))?,
                    i64::try_from(certificate.not_before()).map_err(|_| RegistryError::InvalidInput("certificate timestamp is too large".into()))?,
                    i64::try_from(certificate.not_after()).map_err(|_| RegistryError::InvalidInput("certificate timestamp is too large".into()))?,
                    first_seen,
                ],
            )?;
            transaction.execute(
                "INSERT INTO enrollment_replays (replay_kind, replay_id, expires_at, first_seen)
                 VALUES ('manual_request', ?1, ?2, ?3)",
                params![&request.request_id[..], expires_at, first_seen],
            )?;
            record_audit(
                &transaction,
                AuditInput {
                    event_type: "enrollment_pending",
                    node_id: &registration.node_id,
                    from_state: None,
                    to_state: Some(PeerState::Pending),
                    actor,
                    reason,
                    occurred_at: &now_timestamp(),
                },
            )?;
            record_enrollment_audit_tx(
                &transaction,
                "pending",
                Some(&request.request_id),
                Some(&request_digest),
                &registration.node_id,
                "staged",
                "manual enrollment request staged for local approval",
            )?;
            let peer = load_peer(&transaction, &registration.node_id)?
                .ok_or_else(|| RegistryError::Corrupt("staged enrollment disappeared".into()))?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    pub fn activate_signed_bundle(
        &self,
        bundle: &SignedEnrollmentBundle,
        actor: &str,
        reason: &str,
        now: u64,
        token_hash: &[u8; 32],
        nonce_hash: &[u8; 32],
    ) -> Result<PeerRecord, RegistryError> {
        let bundle_bytes = bundle.encode();
        let bundle_digest = digest(&bundle_bytes);
        let result = self.activate_signed_bundle_transaction(
            bundle,
            actor,
            reason,
            now,
            token_hash,
            nonce_hash,
            &bundle_digest,
        );
        match result {
            Ok(peer) => Ok(peer),
            Err(error) => match self.record_enrollment_audit(
                bundle_failure_code(&error),
                Some(&bundle.bundle_id),
                Some(&bundle_digest),
                &bundle.subject_node_id,
                "rejected",
                bundle_failure_detail(&error),
            ) {
                Ok(()) => Err(error),
                Err(audit_error) => Err(audit_error),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn activate_signed_bundle_transaction(
        &self,
        bundle: &SignedEnrollmentBundle,
        actor: &str,
        reason: &str,
        now: u64,
        token_hash: &[u8; 32],
        nonce_hash: &[u8; 32],
        bundle_digest: &[u8; 32],
    ) -> Result<PeerRecord, RegistryError> {
        if bundle.subject_node_id == self.local_node_id {
            return Err(RegistryError::SelfTrust);
        }
        validate_actor_reason(actor, reason)?;
        let role = match bundle.role {
            crate::enrollment::EnrollmentRole::Conductor => PeerRole::Conductor,
            crate::enrollment::EnrollmentRole::Performer => PeerRole::Performer,
        };
        let registration = PeerRegistration {
            node_id: bundle.subject_node_id.clone(),
            public_key: enrollment::hex_bytes(bundle.subject_xonly.as_slice()),
            role,
            capabilities: bundle.capabilities.clone(),
            source: PeerSource::Bundle,
            actor: actor.to_string(),
            reason: reason.to_string(),
        };
        let first_seen = i64::try_from(now)
            .map_err(|_| RegistryError::InvalidInput("enrollment timestamp is too large".into()))?;
        let replay_expiry = i64::try_from(bundle.replay_expiry())
            .map_err(|_| RegistryError::InvalidInput("enrollment expiry is too large".into()))?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            cleanup_enrollment_replays(&transaction, now)?;
            cleanup_bootstrap_proofs(&transaction, now)?;
            let proof_exists: Option<(Option<i64>, Option<Vec<u8>>)> = transaction
                .query_row(
                    "SELECT consumed_at, bundle_id FROM bootstrap_proofs
                     WHERE target_node_id = ?1 AND organization = ?2
                       AND token_hash = ?3 AND nonce_hash = ?4",
                    params![
                        &self.local_node_id,
                        &bundle.organization,
                        token_hash.as_slice(),
                        nonce_hash.as_slice(),
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if proof_exists.as_ref().is_some_and(|(consumed_at, _)| consumed_at.is_some()) {
                return Err(RegistryError::BootstrapProofConsumed);
            }
            if proof_exists.is_none() {
                let proof_count: i64 = transaction.query_row(
                    "SELECT COUNT(*) FROM bootstrap_proofs",
                    [],
                    |row| row.get(0),
                )?;
                if proof_count >= MAX_BOOTSTRAP_PROOF_ROWS {
                    return Err(RegistryError::BundleCapacity);
                }
                transaction.execute(
                    "INSERT INTO bootstrap_proofs
                     (target_node_id, organization, token_hash, nonce_hash, expires_at, consumed_at, bundle_id, cleanup_state)
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, NULL)",
                    params![
                        &self.local_node_id,
                        &bundle.organization,
                        token_hash.as_slice(),
                        nonce_hash.as_slice(),
                        i64::try_from(bundle.replay_expiry()).map_err(|_| {
                            RegistryError::InvalidInput("bootstrap proof expiry is too large".into())
                        })?,
                    ],
                )?;
            }
            if transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM enrollment_replays
                 WHERE replay_kind = 'bundle' AND replay_id = ?1)",
                [&bundle.bundle_id[..]],
                |row| row.get::<_, i64>(0),
            )? != 0
            {
                return Err(RegistryError::BundleReplay);
            }
            let replay_count: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM enrollment_replays",
                [],
                |row| row.get(0),
            )?;
            if replay_count >= MAX_ENROLLMENT_REPLAY_ROWS {
                return Err(RegistryError::BundleCapacity);
            }
            let rate_floor = first_seen.saturating_sub(60);
            let recent_attempts: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM enrollment_audits
                 WHERE event_code IN ('bundle_completed', 'concurrent', 'revoked_authority')
                   AND occurred_at >= ?1",
                [rate_floor],
                |row| row.get(0),
            )?;
            if recent_attempts >= MAX_BUNDLE_ACTIVATIONS_PER_MINUTE {
                return Err(RegistryError::BundleRateLimited);
            }
            reject_retained_revocation(
                &transaction,
                &registration.node_id,
                &registration.public_key,
            )?;
            if peer_exists(&transaction, &registration.node_id)?
                || public_key_exists(&transaction, &registration.public_key)?
            {
                return Err(RegistryError::BundleConflict);
            }
            self.reject_publisher_conflict(registration.role)?;
            if registration.role == PeerRole::Conductor
                && transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM peers WHERE role = 'conductor' AND state = 'active')",
                    [],
                    |row| row.get::<_, i64>(0),
                )? != 0
            {
                return Err(RegistryError::ConductorConflict);
            }
            let timestamp = now_timestamp();
            transaction.execute(
                "INSERT INTO peers (node_id, public_key, role, state, capabilities_json, added_at, updated_at, last_seen, source)
                 VALUES (?1, ?2, ?3, 'active', ?4, ?5, ?5, NULL, 'bundle')",
                params![
                    registration.node_id,
                    registration.public_key,
                    registration.role.as_str(),
                    capabilities_json(&registration.capabilities)?,
                    timestamp,
                ],
            )?;
            insert_v2_trust_projection(
                &transaction,
                &registration,
                first_seen,
                Some(&bundle.subject_certificate),
            )?;
            transaction.execute(
                "INSERT INTO enrollment_replays (replay_kind, replay_id, expires_at, first_seen)
                 VALUES ('bundle', ?1, ?2, ?3)",
                params![&bundle.bundle_id[..], replay_expiry, first_seen],
            )?;
            transaction.execute(
                "UPDATE bootstrap_proofs
                 SET consumed_at = ?1, bundle_id = ?2, cleanup_state = 'pending'
                 WHERE target_node_id = ?3 AND organization = ?4
                   AND token_hash = ?5 AND nonce_hash = ?6 AND consumed_at IS NULL",
                params![
                    first_seen,
                    &bundle.bundle_id[..],
                    &self.local_node_id,
                    &bundle.organization,
                    token_hash.as_slice(),
                    nonce_hash.as_slice(),
                ],
            )?;
            record_audit(
                &transaction,
                AuditInput {
                    event_type: "enrollment_completed",
                    node_id: &registration.node_id,
                    from_state: None,
                    to_state: Some(PeerState::Active),
                    actor,
                    reason,
                    occurred_at: &timestamp,
                },
            )?;
            record_enrollment_audit_tx(
                &transaction,
                "bundle_completed",
                Some(&bundle.bundle_id),
                Some(bundle_digest),
                &registration.node_id,
                "accepted",
                "signed enrollment bundle activated trust",
            )?;
            let peer = load_peer(&transaction, &registration.node_id)?.ok_or_else(|| {
                RegistryError::Corrupt("signed enrollment peer disappeared".to_string())
            })?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    pub fn approve_manual_enrollment(
        &self,
        request: &ManualEnrollmentRequest,
        certificate: &[u8],
        code: &[u8],
        actor: &str,
        reason: &str,
        now: u64,
    ) -> Result<PeerRecord, RegistryError> {
        if let Err(error) = request.verify(now) {
            self.record_enrollment_audit(
                if matches!(error, crate::enrollment::EnrollmentError::Expired) {
                    "expired"
                } else {
                    "malformed"
                },
                Some(&request.request_id),
                Some(&digest(&request.encode())),
                &request.proposer_node_id,
                "rejected",
                "approval request verification failed",
            )?;
            return Err(RegistryError::InvalidInput(error.to_string()));
        }
        if let Err(error) = request.verify_code(code) {
            let request_bytes = request.encode();
            let request_digest = digest(&request_bytes);
            self.record_enrollment_audit(
                "wrong_code",
                Some(&request.request_id),
                Some(&request_digest),
                &request.proposer_node_id,
                "rejected",
                "approval code did not match staged code hash",
            )?;
            return Err(RegistryError::InvalidInput(error.to_string()));
        }
        validate_actor_reason(actor, reason)?;
        let registration = registration_from_manual(request, actor, reason)?;
        let certificate = TransportCertificate::from_bytes(certificate)
            .map_err(|_| RegistryError::InvalidInput("transport certificate is invalid".into()))?;
        if certificate.node_id() != registration.node_id
            || certificate.identity_key().as_slice()
                != decode_hex(&registration.public_key)?.as_slice()
            || certificate.transport_public() != &request.proposer_transport_x25519
        {
            return Err(RegistryError::InvalidInput(
                "transport certificate does not match manual enrollment identity".into(),
            ));
        }
        certificate
            .verify_time(now)
            .map_err(|_| RegistryError::InvalidInput("transport certificate is expired".into()))?;
        let request_bytes = request.encode();
        let request_digest = digest(&request_bytes);
        let certificate_digest = digest(certificate.as_bytes());
        let now_timestamp = now_timestamp();
        let now_seconds = i64::try_from(now)
            .map_err(|_| RegistryError::InvalidInput("enrollment timestamp is too large".into()))?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = load_peer(&transaction, &registration.node_id)?
                .ok_or_else(|| RegistryError::NotFound(registration.node_id.clone()))?;
            let staged: Option<StagedManualEnrollment> = transaction
                .query_row(
                    "SELECT pairing_id, request_bytes, request_digest, code_hash, identity_key, transport_key,
                            request_created_at, request_expires_at, certificate, certificate_digest,
                            certificate_id, key_epoch, not_before, not_after, state, source
                     FROM manual_enrollment_requests WHERE request_id = ?1",
                    [&request.request_id[..]],
                    |row| {
                        Ok((
                            row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?,
                            row.get(5)?, row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?,
                            row.get(10)?, row.get(11)?, row.get(12)?, row.get(13)?, row.get(14)?,
                            row.get(15)?,
                        ))
                    },
                )
                .optional()?;
            let Some((
                staged_pairing_id,
                staged_bytes,
                staged_digest,
                staged_code_hash,
                staged_identity_key,
                staged_transport_key,
                staged_created_at,
                staged_expires_at,
                staged_certificate,
                staged_certificate_digest,
                staged_certificate_id,
                staged_key_epoch,
                staged_not_before,
                staged_not_after,
                staged_state,
                staged_source,
            )) = staged
            else {
                return Err(RegistryError::NotFound(registration.node_id.clone()));
            };
            if staged_source != "manual" || staged_state != "pending" {
                return Err(RegistryError::EnrollmentConflict);
            }
            if staged_pairing_id.as_deref() != Some(request.pairing_id.as_slice())
                || staged_bytes != request_bytes
                || staged_digest != request_digest
                || staged_code_hash != request.code_hash
                || staged_identity_key != request.proposer_xonly
                || staged_transport_key != request.proposer_transport_x25519
                || staged_created_at != i64::try_from(request.created_at).unwrap_or_default()
                || staged_expires_at != i64::try_from(request.expires_at).unwrap_or_default()
                || staged_certificate != certificate.as_bytes()
                || staged_certificate_digest != certificate_digest
                || staged_certificate_id != certificate.certificate_id()
                || staged_key_epoch != i64::try_from(certificate.key_epoch()).unwrap_or_default()
                || staged_not_before != i64::try_from(certificate.not_before()).unwrap_or_default()
                || staged_not_after != i64::try_from(certificate.not_after()).unwrap_or_default()
            {
                return Err(RegistryError::EnrollmentMismatch);
            }
            if current.source != PeerSource::Manual {
                return Err(RegistryError::EnrollmentConflict);
            }
            if current.public_key != registration.public_key
                || current.role != registration.role
                || current.capabilities != registration.capabilities
            {
                return Err(RegistryError::InvalidInput(
                    "manual enrollment request does not match pending identity".into(),
                ));
            }
            if current.state != PeerState::Pending {
                return Err(RegistryError::InvalidTransition {
                    from: current.state,
                    to: PeerState::Active,
                });
            }
            reject_retained_revocation(
                &transaction,
                &registration.node_id,
                &registration.public_key,
            )?;
            let pending_transport: Option<Vec<u8>> = transaction
                .query_row(
                    "SELECT public_key FROM transport_key_epochs WHERE node_id = ?1 AND state = 'pending'",
                    [&registration.node_id],
                    |row| row.get(0),
                )
                .optional()?;
            if pending_transport.as_deref() != Some(certificate.transport_public().as_slice()) {
                return Err(RegistryError::InvalidInput(
                    "manual enrollment transport key does not match pending identity".into(),
                ));
            }
            transaction.execute(
                "UPDATE peers SET state = 'active', updated_at = ?1 WHERE node_id = ?2 AND state = 'pending'",
                params![now_timestamp, registration.node_id],
            )?;
            transaction.execute(
                "UPDATE manual_enrollment_requests SET state = 'approved', resolved_at = ?1
                 WHERE request_id = ?2 AND state = 'pending'",
                params![now_seconds, &request.request_id[..]],
            )?;
            project_v2_transition(&transaction, &current, PeerState::Active, now_seconds)?;
            record_audit(
                &transaction,
                AuditInput {
                    event_type: "enrollment_approved",
                    node_id: &registration.node_id,
                    from_state: Some(PeerState::Pending),
                    to_state: Some(PeerState::Active),
                    actor,
                    reason,
                    occurred_at: &now_timestamp,
                },
            )?;
            record_enrollment_audit_tx(
                &transaction,
                "approved",
                Some(&request.request_id),
                Some(&request_digest),
                &registration.node_id,
                "approved",
                "manual enrollment activated after explicit approval",
            )?;
            let peer = load_peer(&transaction, &registration.node_id)?
                .ok_or_else(|| RegistryError::Corrupt("approved enrollment disappeared".into()))?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    pub fn reject_manual_enrollment(
        &self,
        node_id: &str,
        actor: &str,
        reason: &str,
    ) -> Result<PeerRecord, RegistryError> {
        validate_actor_reason(actor, reason)?;
        let now = now_timestamp();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = load_peer(&transaction, node_id)?
                .ok_or_else(|| RegistryError::NotFound(node_id.to_string()))?;
            if current.source != PeerSource::Manual || current.state != PeerState::Pending {
                return Err(RegistryError::EnrollmentConflict);
            }
            let (request_id, request_digest): (Vec<u8>, Vec<u8>) = transaction.query_row(
                "SELECT request_id, request_digest
                 FROM manual_enrollment_requests
                 WHERE node_id = ?1 AND source = 'manual' AND state = 'pending'",
                [node_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let request_id: [u8; enrollment::REQUEST_ID_BYTES] =
                request_id.try_into().map_err(|_| {
                    RegistryError::Corrupt("manual request ID has invalid length".into())
                })?;
            let request_digest: [u8; 32] = request_digest.try_into().map_err(|_| {
                RegistryError::Corrupt("manual request digest has invalid length".into())
            })?;
            transaction.execute(
                "UPDATE peers SET state = 'suspended', updated_at = ?1
                 WHERE node_id = ?2 AND state = 'pending' AND source = 'manual'",
                params![now, node_id],
            )?;
            let changed = transaction.execute(
                "UPDATE manual_enrollment_requests SET state = 'rejected', resolved_at = ?1
                 WHERE node_id = ?2 AND source = 'manual' AND state = 'pending'",
                params![timestamp_seconds(&now)?, node_id],
            )?;
            if changed != 1 {
                return Err(RegistryError::EnrollmentConflict);
            }
            record_audit(
                &transaction,
                AuditInput {
                    event_type: "enrollment_rejected",
                    node_id,
                    from_state: Some(PeerState::Pending),
                    to_state: Some(PeerState::Suspended),
                    actor,
                    reason,
                    occurred_at: &now,
                },
            )?;
            record_enrollment_audit_tx(
                &transaction,
                "rejected",
                Some(&request_id),
                Some(&request_digest),
                node_id,
                "rejected",
                "manual enrollment request rejected by local operator",
            )?;
            let peer = load_peer(&transaction, node_id)?
                .ok_or_else(|| RegistryError::Corrupt("rejected enrollment disappeared".into()))?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    /// Atomically import a manually approved peer as active trust. The
    /// operation is intentionally separate from observation/pending
    /// registration and records the approval evidence in the same transaction
    /// as the peer row.
    pub fn import_manual_peer(
        &self,
        registration: PeerRegistration,
    ) -> Result<PeerRecord, RegistryError> {
        self.import_manual_peer_with_transport(registration, None)
    }

    pub fn import_manual_peer_with_transport(
        &self,
        registration: PeerRegistration,
        certificate: Option<&[u8]>,
    ) -> Result<PeerRecord, RegistryError> {
        validate_registration(&registration, &self.local_node_id, &self.local_public_key)?;
        let now = now_timestamp();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            reject_retained_revocation(
                &transaction,
                &registration.node_id,
                &registration.public_key,
            )?;
            if peer_exists(&transaction, &registration.node_id)?
                || public_key_exists(&transaction, &registration.public_key)?
            {
                return Err(RegistryError::Duplicate(registration.node_id.clone()));
            }
            self.reject_publisher_conflict(registration.role)?;
            transaction.execute(
                "INSERT INTO peers (node_id, public_key, role, state, capabilities_json, added_at, updated_at, last_seen, source)
                 VALUES (?1, ?2, ?3, 'active', ?4, ?5, ?5, NULL, ?6)",
                params![
                    registration.node_id,
                    registration.public_key,
                    registration.role.as_str(),
                    capabilities_json(&registration.capabilities)?,
                    now,
                    registration.source.as_str(),
                ],
            )?;
            insert_v2_trust_projection(
                &transaction,
                &registration,
                timestamp_seconds(&now)?,
                certificate,
            )?;
            record_audit(
                &transaction,
                AuditInput {
                    event_type: "peer_trusted",
                    node_id: &registration.node_id,
                    from_state: None,
                    to_state: Some(PeerState::Active),
                    actor: &registration.actor,
                    reason: &registration.reason,
                    occurred_at: &now,
                },
            )?;
            let peer = load_peer(&transaction, &registration.node_id)?
                .ok_or_else(|| RegistryError::Corrupt("imported peer disappeared".to_string()))?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    /// Explicitly authorize a pending or suspended peer.  The actor and
    /// reason are mandatory evidence; there is no implicit activation path.
    pub fn activate_peer(
        &self,
        node_id: &str,
        actor: &str,
        reason: &str,
    ) -> Result<PeerRecord, RegistryError> {
        self.transition_peer(node_id, PeerState::Active, actor, reason)
    }

    pub fn suspend_peer(
        &self,
        node_id: &str,
        actor: &str,
        reason: &str,
    ) -> Result<PeerRecord, RegistryError> {
        self.transition_peer(node_id, PeerState::Suspended, actor, reason)
    }

    /// Revoke a peer and retain its identity forever in `revocations`.
    pub fn revoke_peer(
        &self,
        node_id: &str,
        actor: &str,
        reason: &str,
    ) -> Result<PeerRecord, RegistryError> {
        self.transition_peer(node_id, PeerState::Revoked, actor, reason)
    }

    pub fn transition_peer(
        &self,
        node_id: &str,
        target: PeerState,
        actor: &str,
        reason: &str,
    ) -> Result<PeerRecord, RegistryError> {
        validate_node_id(node_id)?;
        validate_actor_reason(actor, reason)?;
        let now = now_timestamp();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = load_peer(&transaction, node_id)?
                .ok_or_else(|| RegistryError::NotFound(node_id.to_string()))?;
            if target == PeerState::Active {
                reject_retained_revocation(&transaction, &current.node_id, &current.public_key)?;
            }
            if !allowed_transition(current.state, target) {
                return Err(RegistryError::InvalidTransition {
                    from: current.state,
                    to: target,
                });
            }
            transaction.execute(
                "UPDATE peers SET state = ?1, updated_at = ?2 WHERE node_id = ?3",
                params![target.as_str(), now, node_id],
            )?;
            project_v2_transition(&transaction, &current, target, timestamp_seconds(&now)?)?;
            if target == PeerState::Revoked {
                insert_revocation(&transaction, &current, &now, reason, None)?;
            }
            record_audit(
                &transaction,
                AuditInput {
                    event_type: "peer_transition",
                    node_id,
                    from_state: Some(current.state),
                    to_state: Some(target),
                    actor,
                    reason,
                    occurred_at: &now,
                },
            )?;
            let peer = load_peer(&transaction, node_id)?.ok_or_else(|| {
                RegistryError::Corrupt("transitioned peer disappeared".to_string())
            })?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    /// Update a peer's capability allow-list with explicit audit evidence.
    /// Repeating an identical update is rejected so replayed input cannot
    /// append another apparent trust decision.
    pub fn update_peer_capabilities(
        &self,
        node_id: &str,
        capabilities: Vec<String>,
        actor: &str,
        reason: &str,
    ) -> Result<PeerRecord, RegistryError> {
        validate_node_id(node_id)?;
        validate_capabilities(&capabilities)?;
        validate_actor_reason(actor, reason)?;
        let now = now_timestamp();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = load_peer(&transaction, node_id)?
                .ok_or_else(|| RegistryError::NotFound(node_id.to_string()))?;
            if current.state == PeerState::Revoked {
                return Err(RegistryError::InvalidTransition {
                    from: current.state,
                    to: current.state,
                });
            }
            if current.capabilities == capabilities {
                return Err(RegistryError::Unchanged(node_id.to_string()));
            }
            transaction.execute(
                "UPDATE peers SET capabilities_json = ?1, updated_at = ?2 WHERE node_id = ?3",
                params![capabilities_json(&capabilities)?, now, node_id],
            )?;
            transaction.execute(
                "UPDATE trusted_peers SET capabilities = ?1, updated_at = ?2 WHERE node_id = ?3",
                params![
                    capabilities_json(&capabilities)?.as_bytes(),
                    timestamp_seconds(&now)?,
                    node_id
                ],
            )?;
            record_audit(
                &transaction,
                AuditInput {
                    event_type: "peer_capabilities_updated",
                    node_id,
                    from_state: Some(current.state),
                    to_state: Some(current.state),
                    actor,
                    reason,
                    occurred_at: &now,
                },
            )?;
            let peer = load_peer(&transaction, node_id)?
                .ok_or_else(|| RegistryError::Corrupt("updated peer disappeared".to_string()))?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    /// Record a key replacement atomically.  The replacement remains pending;
    /// activation is a separate explicit trust decision.
    pub fn replace_peer(
        &self,
        old_node_id: &str,
        replacement: PeerRegistration,
    ) -> Result<PeerRecord, RegistryError> {
        validate_node_id(old_node_id)?;
        validate_registration(&replacement, &self.local_node_id, &self.local_public_key)?;
        let now = now_timestamp();
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let old = load_peer(&transaction, old_node_id)?
                .ok_or_else(|| RegistryError::NotFound(old_node_id.to_string()))?;
            if old.state == PeerState::Revoked {
                return Err(RegistryError::InvalidTransition {
                    from: old.state,
                    to: PeerState::Revoked,
                });
            }
            if old_node_id == replacement.node_id {
                return Err(RegistryError::Duplicate(old_node_id.to_string()));
            }
            reject_retained_revocation(
                &transaction,
                &replacement.node_id,
                &replacement.public_key,
            )?;
            if peer_exists(&transaction, &replacement.node_id)?
                || public_key_exists(&transaction, &replacement.public_key)?
            {
                return Err(RegistryError::Duplicate(replacement.node_id.clone()));
            }
            self.reject_publisher_conflict(replacement.role)?;
            transaction.execute(
                "INSERT INTO peers (node_id, public_key, role, state, capabilities_json, added_at, updated_at, last_seen, source)
                 VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?5, NULL, ?6)",
                params![
                    replacement.node_id,
                    replacement.public_key,
                    replacement.role.as_str(),
                    capabilities_json(&replacement.capabilities)?,
                    now,
                    replacement.source.as_str(),
                ],
            )?;
            transaction.execute(
                "UPDATE peers SET state = 'revoked', updated_at = ?1 WHERE node_id = ?2",
                params![now, old_node_id],
            )?;
            project_v2_replacement(
                &transaction,
                &old,
                &replacement,
                timestamp_seconds(&now)?,
            )?;
            insert_revocation(
                &transaction,
                &old,
                &now,
                &replacement.reason,
                Some(&replacement.node_id),
            )?;
            record_audit(
                &transaction,
                AuditInput {
                    event_type: "peer_replaced",
                    node_id: old_node_id,
                    from_state: Some(old.state),
                    to_state: Some(PeerState::Revoked),
                    actor: &replacement.actor,
                    reason: &replacement.reason,
                    occurred_at: &now,
                },
            )?;
            let peer = load_peer(&transaction, &replacement.node_id)?
                .ok_or_else(|| RegistryError::Corrupt("replacement peer disappeared".to_string()))?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    pub fn peer(&self, node_id: &str) -> Result<Option<PeerRecord>, RegistryError> {
        validate_node_id(node_id)?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let peer = load_peer(&transaction, node_id)?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    pub fn peers(&self) -> Result<Vec<PeerRecord>, RegistryError> {
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut statement = transaction.prepare(
                "SELECT node_id, public_key, role, state, capabilities_json, added_at, updated_at, last_seen, source
                 FROM peers ORDER BY node_id",
            )?;
            let rows = statement.query_map([], peer_from_row)?;
            let peers = rows.collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            transaction.commit()?;
            Ok(peers)
        })
    }

    pub fn peers_limited(&self, limit: usize) -> Result<Vec<PeerRecord>, RegistryError> {
        if limit == 0 || limit > 1024 {
            return Err(RegistryError::InvalidInput(
                "peer listing limit must be between 1 and 1024".to_string(),
            ));
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut statement = transaction.prepare(
                "SELECT node_id, public_key, role, state, capabilities_json, added_at, updated_at, last_seen, source
                 FROM peers ORDER BY node_id LIMIT ?1",
            )?;
            let rows = statement.query_map([limit as i64], peer_from_row)?;
            let peers = rows.collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            transaction.commit()?;
            Ok(peers)
        })
    }

    pub fn peer_counts(&self) -> Result<PeerCounts, RegistryError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*), COALESCE(SUM(state = 'active'), 0) FROM peers",
                    [],
                    |row| {
                        Ok(PeerCounts {
                            total: row.get::<_, i64>(0)? as usize,
                            active: row.get::<_, i64>(1)? as usize,
                        })
                    },
                )
                .map_err(Into::into)
        })
    }

    pub fn revocations(&self) -> Result<Vec<RevocationRecord>, RegistryError> {
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut statement = transaction.prepare(
                "SELECT id, node_id, public_key, revoked_at, reason, replacement_node_id
                 FROM revocations ORDER BY id",
            )?;
            let rows = statement.query_map([], revocation_from_row)?;
            let records = rows.collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            transaction.commit()?;
            Ok(records)
        })
    }

    pub fn audit_events(&self) -> Result<Vec<AuditEvent>, RegistryError> {
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut statement = transaction.prepare(
                "SELECT id, event_type, node_id, from_state, to_state, actor, reason, occurred_at
                 FROM audit_events ORDER BY id",
            )?;
            let rows = statement.query_map([], audit_from_row)?;
            let events = rows.collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            transaction.commit()?;
            Ok(events)
        })
    }

    /// The bounded, newest-first trust transitions the Health Plane projects
    /// into Conductor-local `enrolled` and `revoked` Signals.
    ///
    /// This is a **read-only** projection over the existing append-only
    /// `audit_events` table. It adds no table, no trigger, no write path, and
    /// no new trust state: the authoritative local record of a peer becoming
    /// trusted or being revoked already exists, and the Health Plane only
    /// reads it. Rows whose `to_state` is neither `active` nor `revoked` are
    /// not lifecycle transitions and never leave the registry.
    pub fn lifecycle_trust_events(&self, limit: usize) -> Result<Vec<AuditEvent>, RegistryError> {
        self.with_connection(|connection| lifecycle_trust_events_in(connection, limit))
    }

    /// Return the v2 projection for transport authorization. Legacy `peers`
    /// rows are intentionally not consulted by the runtime path.
    pub fn transport_peer(
        &self,
        node_id: &str,
        public_key_hex: &str,
    ) -> Result<Option<TransportPeer>, RegistryError> {
        validate_node_id(node_id)?;
        validate_public_key(public_key_hex)?;
        let identity_key = decode_hex(public_key_hex)?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let peer = transaction
                .query_row(
                    "SELECT r.node_id, r.identity_key, r.state,
                            t.key_epoch, t.public_key, t.state, p.state
                     FROM remote_identities r
                     LEFT JOIN trusted_peers p ON p.node_id = r.node_id
                     LEFT JOIN transport_key_epochs t
                       ON t.node_id = r.node_id AND t.state = 'active'
                     WHERE r.node_id = ?1 AND r.identity_key = ?2",
                    params![node_id, identity_key],
                    |row| {
                        let node_id: String = row.get(0)?;
                        let identity_key: Vec<u8> = row.get(1)?;
                        let identity_key = identity_key.try_into().map_err(|_| {
                            rusqlite::Error::InvalidColumnType(
                                1,
                                "identity_key".to_string(),
                                rusqlite::types::Type::Blob,
                            )
                        })?;
                        let identity_state: String = row.get(2)?;
                        let key_epoch: Option<i64> = row.get(3)?;
                        let transport_public_key: Option<Vec<u8>> = row.get(4)?;
                        let transport_public_key = transport_public_key
                            .map(|key| {
                                key.try_into().map_err(|_| {
                                    rusqlite::Error::InvalidColumnType(
                                        4,
                                        "public_key".to_string(),
                                        rusqlite::types::Type::Blob,
                                    )
                                })
                            })
                            .transpose()?;
                        let epoch_state: Option<String> = row.get(5)?;
                        let trust_state: Option<String> = row.get(6)?;
                        let state = if identity_state == "revoked"
                            || trust_state.as_deref() == Some("revoked")
                            || epoch_state.as_deref() == Some("revoked")
                        {
                            PeerState::Revoked
                        } else if identity_state == "active"
                            && trust_state.as_deref() == Some("active")
                        {
                            PeerState::Active
                        } else {
                            PeerState::Pending
                        };
                        Ok(TransportPeer {
                            node_id,
                            identity_key,
                            transport_public_key,
                            key_epoch: key_epoch.map(|epoch| epoch as u64),
                            state,
                        })
                    },
                )
                .optional()?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    /// Append a redacted transport outcome. The method deliberately accepts
    /// only bounded metadata and never accepts frames, plaintext, keys, or
    /// signatures.
    #[allow(clippy::too_many_arguments)]
    pub fn record_transport_audit(
        &self,
        event_type: &str,
        node_id: &str,
        session_id: Option<&[u8; 32]>,
        direction: Option<u8>,
        byte_count: usize,
        outcome: &str,
        error_code: Option<u16>,
    ) -> Result<(), RegistryError> {
        validate_bounded_text("transport event type", event_type, 64)?;
        validate_node_id(node_id)?;
        validate_bounded_text("transport outcome", outcome, 32)?;
        if let Some(session_id) = session_id {
            if session_id.len() != 32 {
                return Err(RegistryError::InvalidInput(
                    "transport session ID must be 32 bytes".to_string(),
                ));
            }
        }
        if !matches!(direction, None | Some(0) | Some(1))
            || error_code.is_some_and(|code| !(1000..=1999).contains(&code))
        {
            return Err(RegistryError::InvalidInput(
                "transport audit metadata is invalid".to_string(),
            ));
        }
        let now = chrono::Utc::now().timestamp();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let audit_count: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM transport_audit",
                [],
                |row| row.get(0),
            )?;
            if audit_count >= MAX_TRANSPORT_AUDIT_ROWS {
                return Err(RegistryError::AuditCapacity);
            }
            transaction.execute(
                "INSERT INTO transport_audit
                 (event_type, node_id, session_id, bundle_id, direction, byte_count, outcome, error_code, occurred_at)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8)",
                params![
                    event_type,
                    node_id,
                    session_id.map(|value| value.as_slice()),
                    direction,
                    i64::try_from(byte_count).map_err(|_| RegistryError::InvalidInput(
                        "transport byte count is too large".to_string()
                    ))?,
                    outcome,
                    error_code,
                    now,
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    /// Append a transport audit row with the bounded Cue correlation fields.
    /// The caller omits these fields for trust failures so unauthorized peers
    /// cannot turn the audit path into a subject-disclosure channel.
    #[allow(clippy::too_many_arguments)]
    pub fn record_cue_transport_audit(
        &self,
        event_type: &str,
        node_id: &str,
        session_id: Option<&[u8; 32]>,
        direction: Option<u8>,
        byte_count: usize,
        outcome: &str,
        error_code: Option<u16>,
        cue_id: Option<&str>,
        cue_script: Option<&str>,
        cue_reason: Option<&str>,
    ) -> Result<(), RegistryError> {
        if cue_id.is_some() != cue_script.is_some() || cue_id.is_some() != cue_reason.is_some() {
            return Err(RegistryError::InvalidInput(
                "Cue audit correlation must be complete".to_string(),
            ));
        }
        if let Some(cue_id) = cue_id {
            if cue_id.len() != 32
                || !cue_id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(RegistryError::InvalidInput(
                    "Cue audit id must be 32 lowercase hex characters".to_string(),
                ));
            }
            validate_bounded_text("Cue audit script", cue_script.unwrap_or_default(), 64)?;
            validate_bounded_text("Cue audit reason", cue_reason.unwrap_or_default(), 128)?;
        }
        validate_bounded_text("transport event type", event_type, 64)?;
        validate_node_id(node_id)?;
        validate_bounded_text("transport outcome", outcome, 32)?;
        if let Some(session_id) = session_id {
            if session_id.len() != 32 {
                return Err(RegistryError::InvalidInput(
                    "transport session ID must be 32 bytes".to_string(),
                ));
            }
        }
        if !matches!(direction, None | Some(0) | Some(1))
            || error_code.is_some_and(|code| !(1000..=1999).contains(&code))
        {
            return Err(RegistryError::InvalidInput(
                "transport audit metadata is invalid".to_string(),
            ));
        }
        let now = chrono::Utc::now().timestamp();
        self.with_connection(|connection| {
            ensure_transport_audit_cue_columns(connection)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let audit_count: i64 =
                transaction
                    .query_row("SELECT COUNT(*) FROM transport_audit", [], |row| row.get(0))?;
            if audit_count >= MAX_TRANSPORT_AUDIT_ROWS {
                return Err(RegistryError::AuditCapacity);
            }
            transaction.execute(
                "INSERT INTO transport_audit
                 (event_type, node_id, session_id, bundle_id, direction, byte_count, outcome,
                  error_code, cue_id, cue_script, cue_reason, occurred_at)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    event_type,
                    node_id,
                    session_id.map(|value| value.as_slice()),
                    direction,
                    i64::try_from(byte_count).map_err(|_| RegistryError::InvalidInput(
                        "transport byte count is too large".to_string()
                    ))?,
                    outcome,
                    error_code,
                    cue_id,
                    cue_script,
                    cue_reason,
                    now,
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    /// Atomically consume one Cue rate token for a peer. This state lives in
    /// the registry so reconnects and multiple live sessions cannot reset or
    /// bypass the per-peer bound.
    pub fn consume_cue_rate(&self, node_id: &str, now: i64) -> Result<bool, RegistryError> {
        validate_node_id(node_id)?;
        if now <= 0 {
            return Err(RegistryError::InvalidInput(
                "Cue rate timestamp must be positive".to_string(),
            ));
        }
        self.with_connection(|connection| {
            ensure_transport_audit_cue_columns(connection)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing: Option<(i64, i64)> = transaction
                .query_row(
                    "SELECT window_start, count FROM cue_rate_limits WHERE node_id = ?1",
                    [node_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let (window_start, count) = match existing {
                Some((window_start, count)) if now.saturating_sub(window_start) < 60 => {
                    (window_start, count)
                }
                _ => (now, 0),
            };
            if count
                >= (crate::remote_cue::MAX_CUES_PER_MINUTE
                    + crate::remote_cue::RATE_BURST_ALLOWANCE) as i64
            {
                transaction.commit()?;
                return Ok(false);
            }
            transaction.execute(
                "INSERT INTO cue_rate_limits (node_id, window_start, count)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(node_id) DO UPDATE SET
                   window_start = excluded.window_start,
                   count = excluded.count",
                params![node_id, window_start, count + 1],
            )?;
            transaction.commit()?;
            Ok(true)
        })
    }

    pub fn record_enrollment_audit(
        &self,
        event_code: &str,
        request_id: Option<&[u8; 16]>,
        request_digest: Option<&[u8; 32]>,
        node_id: &str,
        outcome: &str,
        detail: &str,
    ) -> Result<(), RegistryError> {
        validate_bounded_text("enrollment event code", event_code, 64)?;
        validate_node_id(node_id)?;
        validate_bounded_text("enrollment outcome", outcome, 32)?;
        validate_bounded_text("enrollment detail", detail, 256)?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            record_enrollment_audit_tx(
                &transaction,
                event_code,
                request_id,
                request_digest,
                node_id,
                outcome,
                detail,
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, RegistryError>,
    ) -> Result<T, RegistryError> {
        let mut connection = Connection::open(&self.path)?;
        if self.schema_mutation_allowed {
            configure_connection(&mut connection)?;
        } else {
            configure_connection_read_only(&mut connection)?;
        }
        operation(&mut connection)
    }
}

fn record_enrollment_audit_tx(
    transaction: &Transaction<'_>,
    event_code: &str,
    request_id: Option<&[u8; 16]>,
    request_digest: Option<&[u8; 32]>,
    node_id: &str,
    outcome: &str,
    detail: &str,
) -> Result<(), RegistryError> {
    let count: i64 =
        transaction.query_row("SELECT COUNT(*) FROM enrollment_audits", [], |row| {
            row.get(0)
        })?;
    if count >= MAX_ENROLLMENT_AUDIT_ROWS {
        return Err(RegistryError::AuditCapacity);
    }
    transaction.execute(
        "INSERT INTO enrollment_audits
         (event_code, request_id, request_digest, node_id, outcome, detail, occurred_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            event_code,
            request_id.map(|value| value.as_slice()),
            request_digest.map(|value| value.as_slice()),
            node_id,
            outcome,
            detail,
            Utc::now().timestamp(),
        ],
    )?;
    Ok(())
}

fn validate_database_security(context: &NodeContext, path: &Path) -> Result<(), RegistryError> {
    context.validate_private_file(path)?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = path.with_file_name(format!(
            "{}{}",
            path.file_name().unwrap().to_string_lossy(),
            suffix
        ));
        match std::fs::symlink_metadata(&sidecar) {
            Ok(metadata)
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() =>
            {
                return Err(RegistryError::InvalidSchema(
                    "node SQLite sidecar has an unexpected file type".to_string(),
                ));
            }
            Ok(_) => context.validate_private_file(&sidecar)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn database_sidecar_paths(path: &Path) -> [PathBuf; 2] {
    [
        path.with_file_name(format!(
            "{}-wal",
            path.file_name().unwrap().to_string_lossy()
        )),
        path.with_file_name(format!(
            "{}-shm",
            path.file_name().unwrap().to_string_lossy()
        )),
    ]
}

fn database_sidecar_presence(path: &Path) -> [bool; 2] {
    database_sidecar_paths(path).map(|path| std::fs::symlink_metadata(path).is_ok())
}

fn set_new_database_mode(path: &Path) -> Result<(), RegistryError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let _ = path;
    Ok(())
}

fn validate_existing_database(
    connection: &Connection,
    registry: &NodeRegistry,
) -> Result<(), RegistryError> {
    integrity_check(connection)?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 0 {
        return Err(RegistryError::InvalidSchema(
            "existing node.sqlite has no schema version".to_string(),
        ));
    }
    if version > SCHEMA_VERSION {
        return Err(RegistryError::InvalidSchema(format!(
            "database version {version} is newer than supported version {SCHEMA_VERSION}"
        )));
    }
    if version < SCHEMA_VERSION {
        let health_plane: Option<String> = connection
            .query_row(
                "SELECT value FROM metadata WHERE key = 'health_plane'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if !(version == 6 && health_plane.as_deref() == Some(HEALTH_PLANE_DISABLED)) {
            return Err(RegistryError::InvalidSchema(format!(
                "database schema marker is {version}, expected {SCHEMA_VERSION}"
            )));
        }
    }
    validate_schema(connection, registry)
}

fn initialize_database(
    connection: &mut Connection,
    registry: &NodeRegistry,
) -> Result<(), RegistryError> {
    integrity_check(connection)?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(RegistryError::InvalidSchema(format!(
            "database version {version} is newer than supported version {SCHEMA_VERSION}"
        )));
    }
    if version < 0 {
        return Err(RegistryError::InvalidSchema(
            "negative database version".to_string(),
        ));
    }
    if version == 0 {
        if has_user_objects(connection)? {
            return Err(RegistryError::InvalidSchema(
                "unversioned database contains objects".to_string(),
            ));
        }
        let transaction = connection.transaction()?;
        create_schema(&transaction, registry)?;
        transaction.execute_batch("PRAGMA user_version = 1")?;
        transaction.commit()?;
    }
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 1 {
        migrate_v1_to_v2(connection, registry)?;
    }
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 2 {
        migrate_v2_to_v3(connection)?;
    }
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 3 {
        migrate_v3_to_v4(connection)?;
    }
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 4 {
        migrate_v4_to_v5(connection)?;
    }
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 5 {
        migrate_v5_to_v6(connection)?;
    }
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 6 {
        apply_health_plane_migration(connection)?;
    }
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 7 {
        migrate_v7_to_v8(connection)?;
    }
    ensure_transport_audit_cue_columns(connection)?;
    validate_schema(connection, registry)
}

/// Add the Cue-only schema pieces to registries created before Cue support.
/// This is intentionally additive and does not alter the trust schema version:
/// old audit rows remain valid with NULL Cue metadata.
fn ensure_transport_audit_cue_columns(connection: &Connection) -> Result<(), RegistryError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS cue_rate_limits (
           node_id TEXT PRIMARY KEY,
           window_start INTEGER NOT NULL CHECK (window_start > 0),
           count INTEGER NOT NULL CHECK (count >= 0)
         )",
    )?;
    let mut statement = connection.prepare("PRAGMA table_info(transport_audit)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    drop(statement);

    if !columns.contains("cue_id") {
        connection.execute(
            "ALTER TABLE transport_audit ADD COLUMN cue_id TEXT NULL CHECK (cue_id IS NULL OR length(cue_id) = 32)",
            [],
        )?;
    }
    if !columns.contains("cue_script") {
        connection.execute(
            "ALTER TABLE transport_audit ADD COLUMN cue_script TEXT NULL CHECK (cue_script IS NULL OR length(CAST(cue_script AS BLOB)) BETWEEN 1 AND 64)",
            [],
        )?;
    }
    if !columns.contains("cue_reason") {
        connection.execute(
            "ALTER TABLE transport_audit ADD COLUMN cue_reason TEXT NULL CHECK (cue_reason IS NULL OR length(CAST(cue_reason AS BLOB)) BETWEEN 1 AND 128)",
            [],
        )?;
    }
    Ok(())
}

/// Widen the stored Profile with the baseline a Performer holds.
///
/// Forward-only and fail-hard, unlike the Health Plane migration it follows.
/// That one degraded gracefully because the plane it created was new and a node
/// could serve everything else without it; there is no equivalent half-state
/// here. The Profile schema is closed, so a receiver whose storage lacks these
/// two columns and whose validator requires them would accept a Profile it
/// cannot write, and reporting drift from a table that does not hold the answer
/// is worse than refusing to open.
///
/// A node that never completed the Health Plane migration stays at version 6
/// with the plane disabled and never reaches here.
fn migrate_v7_to_v8(connection: &mut Connection) -> Result<(), RegistryError> {
    let metadata_version: String = connection.query_row(
        "SELECT value FROM metadata WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    if metadata_version != "7" {
        return Err(RegistryError::InvalidSchema(
            "v7 database metadata marker is invalid".to_string(),
        ));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Empty is the shipped answer for every existing row: a Performer that
    // already reported under version 7 said nothing about a baseline, and
    // inventing one for it would make a node read as in sync with a set it
    // never received. The next Profile it sends replaces the row.
    transaction.execute_batch(
        "ALTER TABLE health_profiles ADD COLUMN baseline_id TEXT NOT NULL DEFAULT ''
           CHECK (baseline_id = '' OR length(baseline_id) = 64);
         ALTER TABLE health_profiles ADD COLUMN baseline_observed_id TEXT NOT NULL DEFAULT ''
           CHECK (baseline_observed_id = '' OR length(baseline_observed_id) = 64);",
    )?;
    transaction.execute(
        "UPDATE metadata SET value = '8' WHERE key = 'schema_version'",
        [],
    )?;
    transaction.execute_batch("PRAGMA user_version = 8")?;
    transaction.commit()?;
    Ok(())
}

/// Attempt the Health Plane migration to schema version 7.
///
/// The migration is forward-only and runs in one transaction. A failure rolls
/// back completely, leaves the database at version 6, records a
/// `health_corrupt_state` (1115) transport audit, and marks the Health Plane
/// disabled so transport, enrollment, HTTP, and runs keep serving. Retry is
/// never automatic; an operator clears the marker explicitly.
fn apply_health_plane_migration(connection: &mut Connection) -> Result<(), RegistryError> {
    let blocked: Option<String> = connection
        .query_row(
            "SELECT value FROM metadata WHERE key = 'health_plane'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if blocked.as_deref() == Some(HEALTH_PLANE_DISABLED) {
        return Ok(());
    }
    match migrate_v6_to_v7(connection) {
        Ok(()) => Ok(()),
        Err(_) => {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute(
                "INSERT OR REPLACE INTO metadata (key, value) VALUES ('health_plane', ?1)",
                [HEALTH_PLANE_DISABLED],
            )?;
            let audit_count: i64 =
                transaction
                    .query_row("SELECT COUNT(*) FROM transport_audit", [], |row| row.get(0))?;
            if audit_count < MAX_TRANSPORT_AUDIT_ROWS {
                transaction.execute(
                    "INSERT INTO transport_audit
                     (event_type, node_id, session_id, bundle_id, direction, byte_count,
                      outcome, error_code, occurred_at)
                     VALUES ('health_plane_migration', (SELECT value FROM metadata WHERE key = 'node_id'),
                             NULL, NULL, NULL, 0, 'rejected', 1115, ?1)",
                    params![Utc::now().timestamp()],
                )?;
            }
            transaction.commit()?;
            Ok(())
        }
    }
}

fn configure_connection(connection: &mut Connection) -> Result<(), RegistryError> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    let mut journal_mode: String =
        connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        journal_mode = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    }
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(RegistryError::InvalidSchema(format!(
            "journal mode is {journal_mode:?}, expected WAL"
        )));
    }
    connection.execute_batch("PRAGMA foreign_keys = ON")?;
    let foreign_keys: i64 = connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if foreign_keys != 1 {
        return Err(RegistryError::InvalidSchema(
            "foreign key enforcement is disabled".to_string(),
        ));
    }
    Ok(())
}

fn configure_connection_read_only(connection: &mut Connection) -> Result<(), RegistryError> {
    connection.busy_timeout(BUSY_TIMEOUT)?;
    let journal_mode: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(RegistryError::InvalidSchema(format!(
            "journal mode is {journal_mode:?}, expected WAL"
        )));
    }
    connection.execute_batch("PRAGMA foreign_keys = ON")?;
    let foreign_keys: i64 = connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if foreign_keys != 1 {
        return Err(RegistryError::InvalidSchema(
            "foreign key enforcement is disabled".to_string(),
        ));
    }
    Ok(())
}

fn integrity_check(connection: &Connection) -> Result<(), RegistryError> {
    let result: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if result != "ok" {
        return Err(RegistryError::Corrupt(result));
    }
    Ok(())
}

fn has_user_objects(connection: &Connection) -> Result<bool, RegistryError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type IN ('table', 'index', 'trigger', 'view') AND name NOT LIKE 'sqlite_%')",
        [],
        |row| row.get::<_, i64>(0),
    )? != 0)
}

fn create_schema(
    transaction: &Transaction<'_>,
    registry: &NodeRegistry,
) -> Result<(), RegistryError> {
    transaction.execute_batch(
        "CREATE TABLE metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE peers (
            node_id TEXT PRIMARY KEY,
            public_key TEXT NOT NULL UNIQUE,
            role TEXT NOT NULL,
            state TEXT NOT NULL,
            capabilities_json TEXT NOT NULL,
            added_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            last_seen TEXT NULL,
            source TEXT NOT NULL
        );
        CREATE TABLE revocations (
            id INTEGER PRIMARY KEY,
            node_id TEXT NOT NULL,
            public_key TEXT NOT NULL,
            revoked_at TEXT NOT NULL,
            reason TEXT NOT NULL,
            replacement_node_id TEXT NULL
        );
        CREATE TABLE audit_events (
            id INTEGER PRIMARY KEY,
            event_type TEXT NOT NULL,
            node_id TEXT NOT NULL,
            from_state TEXT NULL,
            to_state TEXT NULL,
            actor TEXT NOT NULL,
            reason TEXT NOT NULL,
            occurred_at TEXT NOT NULL
        );
        CREATE TABLE replay_keys (
            key TEXT PRIMARY KEY,
            first_seen TEXT NOT NULL,
            expires_at TEXT NOT NULL
        );
        -- Declared, integrity-checked and exported, and deliberately never
        -- written. It was sketched for remote Cues before that plane existed;
        -- when Cues shipped, the design refused it, because it would have been
        -- eight states re-implementing `RunState` and the durable at-most-once
        -- guarantee already falls out of `runs.run_id` being a primary key
        -- derived from the cue id. Left in place rather than dropped: removing
        -- it costs a registry schema bump, and this comment costs nothing.
        -- If you are looking for where a Cue is recorded, it is the runs table.
        CREATE TABLE inbox (
            cue_id TEXT PRIMARY KEY,
            state TEXT NOT NULL,
            received_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            outcome_hash TEXT NULL
        );
        CREATE INDEX peers_state_idx ON peers(state);
        CREATE INDEX audit_events_node_idx ON audit_events(node_id, id);
        CREATE TRIGGER revocations_no_update BEFORE UPDATE ON revocations
        BEGIN SELECT RAISE(ABORT, 'revocations are append-only'); END;
        CREATE TRIGGER revocations_no_delete BEFORE DELETE ON revocations
        BEGIN SELECT RAISE(ABORT, 'revocations are append-only'); END;
        CREATE TRIGGER audit_events_no_update BEFORE UPDATE ON audit_events
        BEGIN SELECT RAISE(ABORT, 'audit events are append-only'); END;
        CREATE TRIGGER audit_events_no_delete BEFORE DELETE ON audit_events
        BEGIN SELECT RAISE(ABORT, 'audit events are append-only'); END;
        INSERT INTO metadata (key, value) VALUES
            ('schema_version', '1'),
            ('node_id', ''),
            ('public_key_encoding', 'x-only-bip340-hex-lowercase');",
    )?;
    transaction.execute(
        "UPDATE metadata SET value = ?1 WHERE key = 'node_id'",
        [registry.local_node_id.as_str()],
    )?;
    Ok(())
}

fn migrate_v1_to_v2(
    connection: &mut Connection,
    registry: &NodeRegistry,
) -> Result<(), RegistryError> {
    validate_v1_preflight(connection, registry)?;
    let metadata_version: String = connection.query_row(
        "SELECT value FROM metadata WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    if metadata_version != "1" {
        return Err(RegistryError::InvalidSchema(
            "v1 database metadata marker is invalid".to_string(),
        ));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    create_v2_schema(&transaction)?;

    let mut statement = transaction.prepare(
        "SELECT node_id, public_key, role, state, capabilities_json, added_at, updated_at
         FROM peers ORDER BY node_id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (node_id, public_key, role, state, capabilities_json, added_at, updated_at) in rows {
        let identity_key = decode_hex(&public_key)?;
        let first_seen = timestamp_seconds(&added_at)?;
        let updated = timestamp_seconds(&updated_at)?;
        let (identity_state, revoked_at) = match state.as_str() {
            "active" => ("active", None),
            "revoked" => ("revoked", Some(updated)),
            "pending" | "suspended" => ("authenticated_untrusted", None),
            _ => {
                return Err(RegistryError::InvalidSchema(format!(
                    "unknown v1 peer state {state:?}"
                )))
            }
        };
        transaction.execute(
            "INSERT INTO remote_identities
             (node_id, identity_key, state, first_seen, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                node_id,
                identity_key,
                identity_state,
                first_seen,
                revoked_at
            ],
        )?;
        if state == "active" || state == "suspended" || state == "revoked" {
            let role = match role.as_str() {
                "conductor" => 1,
                "performer" => 2,
                _ => {
                    return Err(RegistryError::InvalidSchema(format!(
                        "unknown v1 peer role {role:?}"
                    )))
                }
            };
            validate_capabilities_json_bytes(&capabilities_json)?;
            transaction.execute(
                "INSERT INTO trusted_peers
                 (node_id, role, capabilities, state, added_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    node_id,
                    role,
                    capabilities_json.as_bytes(),
                    if state == "active" {
                        "active"
                    } else {
                        "revoked"
                    },
                    first_seen,
                    updated,
                ],
            )?;
        }
    }
    transaction.execute(
        "UPDATE metadata SET value = '2' WHERE key = 'schema_version'",
        [],
    )?;
    transaction.execute_batch("PRAGMA user_version = 2")?;
    transaction.commit()?;
    validate_v2_invariants(connection, registry)
}

fn migrate_v2_to_v3(connection: &mut Connection) -> Result<(), RegistryError> {
    let metadata_version: String = connection.query_row(
        "SELECT value FROM metadata WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    if metadata_version != "2" {
        return Err(RegistryError::InvalidSchema(
            "v2 database metadata marker is invalid".to_string(),
        ));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    create_v3_schema(&transaction)?;
    transaction.execute(
        "UPDATE metadata SET value = '3' WHERE key = 'schema_version'",
        [],
    )?;
    transaction.execute_batch("PRAGMA user_version = 3")?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v3_to_v4(connection: &mut Connection) -> Result<(), RegistryError> {
    let metadata_version: String = connection.query_row(
        "SELECT value FROM metadata WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    if metadata_version != "3" {
        return Err(RegistryError::InvalidSchema(
            "v3 database metadata marker is invalid".to_string(),
        ));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "ALTER TABLE manual_enrollment_requests ADD COLUMN pairing_id BLOB NULL
             CHECK (pairing_id IS NULL OR length(pairing_id) = 16);
         CREATE INDEX manual_enrollment_requests_pairing_idx
             ON manual_enrollment_requests(pairing_id);",
    )?;
    transaction.execute_batch(
        "DROP TRIGGER manual_enrollment_request_immutable;
         CREATE TRIGGER manual_enrollment_request_immutable
         BEFORE UPDATE ON manual_enrollment_requests
          WHEN NEW.request_id <> OLD.request_id
            OR COALESCE(NEW.pairing_id, X'') <> COALESCE(OLD.pairing_id, X'')
            OR NEW.request_bytes <> OLD.request_bytes
           OR NEW.request_digest <> OLD.request_digest
           OR NEW.code_hash <> OLD.code_hash
           OR NEW.node_id <> OLD.node_id
           OR NEW.identity_key <> OLD.identity_key
           OR NEW.transport_key <> OLD.transport_key
           OR NEW.role <> OLD.role
           OR NEW.capabilities <> OLD.capabilities
           OR NEW.request_created_at <> OLD.request_created_at
           OR NEW.request_expires_at <> OLD.request_expires_at
           OR NEW.certificate <> OLD.certificate
           OR NEW.certificate_digest <> OLD.certificate_digest
           OR NEW.certificate_id <> OLD.certificate_id
           OR NEW.key_epoch <> OLD.key_epoch
           OR NEW.not_before <> OLD.not_before
           OR NEW.not_after <> OLD.not_after
           OR NEW.source <> OLD.source
           OR NEW.staged_at <> OLD.staged_at
         BEGIN SELECT RAISE(ABORT, 'manual enrollment evidence is immutable'); END;",
    )?;
    let migration_now = Utc::now().timestamp().max(1);
    let migration_timestamp = now_timestamp();
    let legacy_pending: Vec<([u8; 16], [u8; 32], String)> = {
        let mut statement = transaction.prepare(
            "SELECT request_id, request_digest, node_id
             FROM manual_enrollment_requests
             WHERE state = 'pending'
             ORDER BY request_id",
        )?;
        let rows = statement
            .query_map([], |row| {
                let request_id: Vec<u8> = row.get(0)?;
                let request_digest: Vec<u8> = row.get(1)?;
                let node_id: String = row.get(2)?;
                let request_id = request_id.try_into().map_err(|_| {
                    rusqlite::Error::FromSqlConversionFailure(
                        16,
                        rusqlite::types::Type::Blob,
                        Box::new(RegistryError::InvalidSchema(
                            "legacy enrollment request ID has invalid length".to_string(),
                        )),
                    )
                })?;
                let request_digest = request_digest.try_into().map_err(|_| {
                    rusqlite::Error::FromSqlConversionFailure(
                        32,
                        rusqlite::types::Type::Blob,
                        Box::new(RegistryError::InvalidSchema(
                            "legacy enrollment request digest has invalid length".to_string(),
                        )),
                    )
                })?;
                Ok((request_id, request_digest, node_id))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for (request_id, request_digest, node_id) in &legacy_pending {
        record_enrollment_audit_tx(
            &transaction,
            "legacy_expired",
            Some(request_id),
            Some(request_digest),
            node_id,
            "rejected",
            "legacy manual enrollment request expired during schema migration",
        )?;
        let current = load_peer(&transaction, node_id)?.ok_or_else(|| {
            RegistryError::InvalidSchema(
                "legacy pending enrollment is missing its peer projection".to_string(),
            )
        })?;
        if current.source != PeerSource::Manual || current.state != PeerState::Pending {
            return Err(RegistryError::InvalidSchema(
                "legacy pending enrollment has inconsistent peer state".to_string(),
            ));
        }
        project_v2_transition(&transaction, &current, PeerState::Suspended, migration_now)?;
        transaction.execute(
            "UPDATE peers SET state = 'suspended', updated_at = ?1
             WHERE node_id = ?2 AND state = 'pending' AND source = 'manual'",
            params![migration_timestamp, node_id],
        )?;
    }
    transaction.execute(
        "UPDATE manual_enrollment_requests
         SET state = 'rejected',
             resolved_at = CASE WHEN staged_at > ?1 THEN staged_at ELSE ?1 END
         WHERE state = 'pending'",
        [migration_now],
    )?;
    transaction.execute(
        "UPDATE metadata SET value = '4' WHERE key = 'schema_version'",
        [],
    )?;
    transaction.execute_batch("PRAGMA user_version = 4")?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v4_to_v5(connection: &mut Connection) -> Result<(), RegistryError> {
    let metadata_version: String = connection.query_row(
        "SELECT value FROM metadata WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    if metadata_version != "4" {
        return Err(RegistryError::InvalidSchema(
            "v4 database metadata marker is invalid".to_string(),
        ));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "CREATE TABLE bootstrap_proofs (
           target_node_id TEXT NOT NULL CHECK (length(CAST(target_node_id AS BLOB)) = 69),
           organization TEXT NOT NULL CHECK (length(CAST(organization AS BLOB)) BETWEEN 1 AND 128),
           token_hash BLOB NOT NULL CHECK (length(token_hash) = 32),
           nonce_hash BLOB NOT NULL CHECK (length(nonce_hash) = 32),
           expires_at INTEGER NOT NULL CHECK (expires_at > 0),
           consumed_at INTEGER NULL CHECK (consumed_at IS NULL OR consumed_at > 0),
           bundle_id BLOB NULL CHECK (bundle_id IS NULL OR length(bundle_id) = 16),
           PRIMARY KEY (target_node_id, organization, token_hash, nonce_hash)
         );
         CREATE INDEX bootstrap_proofs_expiry_idx ON bootstrap_proofs(expires_at);
         CREATE TRIGGER bootstrap_proofs_no_update
         BEFORE UPDATE ON bootstrap_proofs
         WHEN NEW.target_node_id <> OLD.target_node_id
           OR NEW.organization <> OLD.organization
           OR NEW.token_hash <> OLD.token_hash
           OR NEW.nonce_hash <> OLD.nonce_hash
           OR NEW.expires_at <> OLD.expires_at
           OR (OLD.consumed_at IS NOT NULL AND (
                NEW.consumed_at IS NOT OLD.consumed_at
                OR NEW.bundle_id IS NOT OLD.bundle_id
              ))
           OR (OLD.consumed_at IS NULL AND (
                NEW.consumed_at IS NULL OR NEW.bundle_id IS NULL
              ))
         BEGIN SELECT RAISE(ABORT, 'bootstrap proof identity is immutable'); END;",
    )?;
    transaction.execute(
        "UPDATE metadata SET value = '5' WHERE key = 'schema_version'",
        [],
    )?;
    transaction.execute_batch("PRAGMA user_version = 5")?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v5_to_v6(connection: &mut Connection) -> Result<(), RegistryError> {
    let metadata_version: String = connection.query_row(
        "SELECT value FROM metadata WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    if metadata_version != "5" {
        return Err(RegistryError::InvalidSchema(
            "v5 database metadata marker is invalid".to_string(),
        ));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "ALTER TABLE bootstrap_proofs ADD COLUMN cleanup_state TEXT NULL
           CHECK (cleanup_state IS NULL OR cleanup_state IN ('pending', 'complete'));
         UPDATE bootstrap_proofs
            SET cleanup_state = 'complete'
          WHERE consumed_at IS NOT NULL;
         DROP TRIGGER bootstrap_proofs_no_update;
         CREATE TRIGGER bootstrap_proofs_no_update
         BEFORE UPDATE ON bootstrap_proofs
         WHEN NEW.target_node_id <> OLD.target_node_id
           OR NEW.organization <> OLD.organization
           OR NEW.token_hash <> OLD.token_hash
           OR NEW.nonce_hash <> OLD.nonce_hash
           OR NEW.expires_at <> OLD.expires_at
           OR (OLD.consumed_at IS NOT NULL AND (
                NEW.consumed_at IS NOT OLD.consumed_at
                OR NEW.bundle_id IS NOT OLD.bundle_id
                OR NEW.cleanup_state IS NULL
                OR NEW.cleanup_state NOT IN ('pending', 'complete')
                OR (OLD.cleanup_state = 'complete' AND NEW.cleanup_state <> OLD.cleanup_state)
              ))
           OR (OLD.consumed_at IS NULL AND (
                NEW.consumed_at IS NULL OR NEW.bundle_id IS NULL
                OR (NEW.cleanup_state IS NOT NULL AND NEW.cleanup_state <> 'pending')
              ))
         BEGIN SELECT RAISE(ABORT, 'bootstrap proof identity is immutable'); END;",
    )?;
    transaction.execute(
        "UPDATE metadata SET value = '6' WHERE key = 'schema_version'",
        [],
    )?;
    transaction.execute_batch("PRAGMA user_version = 6")?;
    transaction.commit()?;
    Ok(())
}

/// Create the bounded Health Plane tables. The migration is additive: it never
/// mutates, drops, or rewrites a row created by schema versions 1 through 6.
fn migrate_v6_to_v7(connection: &mut Connection) -> Result<(), RegistryError> {
    #[cfg(test)]
    if HEALTH_MIGRATION_FAULT.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(RegistryError::InvalidSchema(
            "injected health plane migration failure".to_string(),
        ));
    }
    let metadata_version: String = connection.query_row(
        "SELECT value FROM metadata WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    if metadata_version != "6" {
        return Err(RegistryError::InvalidSchema(
            "v6 database metadata marker is invalid".to_string(),
        ));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(HEALTH_PLANE_SCHEMA)?;
    transaction.execute(
        "UPDATE metadata SET value = '7' WHERE key = 'schema_version'",
        [],
    )?;
    transaction.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('health_plane', ?1)",
        [HEALTH_PLANE_ENABLED],
    )?;
    transaction.execute_batch("PRAGMA user_version = 7")?;
    transaction.commit()?;
    Ok(())
}

/// The complete schema version 7 Health Plane object set.
const HEALTH_PLANE_SCHEMA: &str = "
    CREATE TABLE health_peers (
      node_id TEXT PRIMARY KEY REFERENCES remote_identities(node_id),
      role INTEGER NOT NULL CHECK (role IN (1, 2)),
      cursor INTEGER NOT NULL DEFAULT 0 CHECK (cursor >= 0),
      last_profile_revision INTEGER NOT NULL DEFAULT 0 CHECK (last_profile_revision >= 0),
      last_pulse_sequence INTEGER NOT NULL DEFAULT 0 CHECK (last_pulse_sequence >= 0),
      last_pulse_at INTEGER NULL,
      version_incompatible_at INTEGER NULL,
      minute_window_start INTEGER NOT NULL DEFAULT 0,
      minute_messages INTEGER NOT NULL DEFAULT 0 CHECK (minute_messages >= 0),
      minute_signals INTEGER NOT NULL DEFAULT 0 CHECK (minute_signals >= 0),
      hour_window_start INTEGER NOT NULL DEFAULT 0,
      hour_profiles INTEGER NOT NULL DEFAULT 0 CHECK (hour_profiles >= 0),
      first_seen INTEGER NOT NULL CHECK (first_seen > 0),
      updated_at INTEGER NOT NULL CHECK (updated_at >= first_seen)
    );
    CREATE TABLE health_profiles (
      node_id TEXT PRIMARY KEY REFERENCES health_peers(node_id),
      profile_revision INTEGER NOT NULL CHECK (profile_revision >= 1),
      agent_version TEXT NOT NULL CHECK (length(agent_version) BETWEEN 1 AND 32),
      arch TEXT NOT NULL CHECK (arch IN ('x86_64', 'aarch64', 'unknown')),
      capabilities TEXT NOT NULL CHECK (length(capabilities) <= 2048),
      display_name TEXT NOT NULL CHECK (length(display_name) <= 64),
      distro_id TEXT NOT NULL CHECK (length(distro_id) <= 32),
      distro_version TEXT NOT NULL CHECK (length(distro_version) <= 32),
      omarchy_channel TEXT NOT NULL CHECK (omarchy_channel IN ('', 'stable', 'dev')),
      omarchy_version TEXT NOT NULL CHECK (length(omarchy_version) <= 32),
      platform TEXT NOT NULL CHECK (platform IN ('linux', 'macos', 'windows')),
      role TEXT NOT NULL CHECK (role = 'performer'),
      runtimes TEXT NOT NULL CHECK (length(runtimes) <= 512),
      message_bytes INTEGER NOT NULL CHECK (message_bytes BETWEEN 1 AND 2112),
      received_at INTEGER NOT NULL CHECK (received_at > 0)
    );
    CREATE TABLE health_pulses (
      node_id TEXT PRIMARY KEY REFERENCES health_peers(node_id),
      sequence INTEGER NOT NULL CHECK (sequence >= 1),
      emitted_at INTEGER NOT NULL CHECK (emitted_at > 0),
      profile_revision INTEGER NOT NULL CHECK (profile_revision >= 0),
      runner_state TEXT NOT NULL
        CHECK (runner_state IN ('idle', 'busy', 'paused', 'degraded', 'stopped')),
      scheduler_state TEXT NOT NULL CHECK (scheduler_state IN ('running', 'disabled')),
      queue_depth INTEGER NOT NULL CHECK (queue_depth BETWEEN 0 AND 65535),
      workers_busy INTEGER NOT NULL CHECK (workers_busy BETWEEN 0 AND 255),
      workers_configured INTEGER NOT NULL CHECK (workers_configured BETWEEN 0 AND 255),
      uptime_seconds INTEGER NOT NULL CHECK (uptime_seconds BETWEEN 0 AND 4294967295),
      last_run TEXT NULL CHECK (last_run IS NULL OR length(last_run) <= 512),
      message_bytes INTEGER NOT NULL CHECK (message_bytes BETWEEN 1 AND 1344),
      received_at INTEGER NOT NULL CHECK (received_at > 0)
    );
    CREATE TABLE health_signals (
      node_id TEXT NOT NULL REFERENCES health_peers(node_id),
      signal_id BLOB NOT NULL CHECK (length(signal_id) = 16),
      sequence INTEGER NOT NULL CHECK (sequence >= 1),
      state TEXT NOT NULL CHECK (state IN ('applied', 'held')),
      kind TEXT NOT NULL CHECK (kind IN ('enrolled', 'revoked', 'run-completed')),
      occurred_at INTEGER NOT NULL CHECK (occurred_at > 0),
      subject TEXT NULL CHECK (subject IS NULL OR length(subject) = 69),
      run TEXT NULL CHECK (run IS NULL OR length(run) <= 512),
      message_bytes INTEGER NOT NULL CHECK (message_bytes BETWEEN 1 AND 1088),
      received_at INTEGER NOT NULL CHECK (received_at > 0),
      expires_at INTEGER NOT NULL CHECK (expires_at > received_at),
      PRIMARY KEY (node_id, signal_id),
      UNIQUE (node_id, sequence)
    );
    CREATE TABLE health_outbox (
      signal_id BLOB PRIMARY KEY CHECK (length(signal_id) = 16),
      target_node_id TEXT NOT NULL CHECK (length(target_node_id) = 69),
      sequence INTEGER NOT NULL UNIQUE CHECK (sequence >= 1),
      kind TEXT NOT NULL CHECK (kind IN ('enrolled', 'revoked', 'run-completed')),
      occurred_at INTEGER NOT NULL CHECK (occurred_at > 0),
      subject TEXT NULL CHECK (subject IS NULL OR length(subject) = 69),
      run TEXT NULL CHECK (run IS NULL OR length(run) <= 512),
      message_bytes INTEGER NOT NULL CHECK (message_bytes BETWEEN 1 AND 1088),
      attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 3),
      last_message_id BLOB NULL CHECK (last_message_id IS NULL OR length(last_message_id) = 16),
      enqueued_at INTEGER NOT NULL CHECK (enqueued_at > 0),
      updated_at INTEGER NOT NULL CHECK (updated_at >= enqueued_at),
      expires_at INTEGER NOT NULL CHECK (expires_at > enqueued_at)
    );
    CREATE TABLE health_replay_keys (
      message_id BLOB PRIMARY KEY CHECK (length(message_id) = 16),
      node_id TEXT NOT NULL CHECK (length(node_id) = 69),
      first_seen INTEGER NOT NULL CHECK (first_seen > 0),
      expires_at INTEGER NOT NULL CHECK (expires_at > first_seen)
    );
    CREATE TABLE health_audit (
      id INTEGER PRIMARY KEY,
      event_code TEXT NOT NULL CHECK (length(event_code) BETWEEN 1 AND 64),
      node_id TEXT NOT NULL CHECK (length(node_id) = 69),
      message_kind TEXT NOT NULL CHECK (length(message_kind) BETWEEN 1 AND 64),
      byte_count INTEGER NOT NULL CHECK (byte_count >= 0),
      outcome TEXT NOT NULL CHECK (outcome IN ('accepted', 'held', 'rejected', 'dropped', 'purged')),
      error_code INTEGER NULL CHECK (error_code IS NULL OR error_code BETWEEN 1000 AND 1999),
      occurred_at INTEGER NOT NULL CHECK (occurred_at > 0)
    );
    CREATE TABLE health_local (
      key TEXT PRIMARY KEY CHECK (key IN ('signal_sequence', 'signals_dropped')),
      value INTEGER NOT NULL CHECK (value >= 0)
    );
    CREATE INDEX health_audit_expiry_idx ON health_audit(occurred_at);
    CREATE INDEX health_outbox_order_idx ON health_outbox(sequence);
    CREATE INDEX health_replay_keys_expiry_idx ON health_replay_keys(expires_at);
    CREATE INDEX health_signals_expiry_idx ON health_signals(expires_at);
    CREATE INDEX health_signals_order_idx ON health_signals(node_id, state, sequence);
    CREATE TRIGGER health_audit_no_update
    BEFORE UPDATE ON health_audit
    BEGIN SELECT RAISE(ABORT, 'health audit rows are append-only'); END;
    CREATE TRIGGER health_peers_require_active_trust
    BEFORE INSERT ON health_peers
    WHEN NOT EXISTS (
      SELECT 1 FROM trusted_peers t JOIN remote_identities r ON r.node_id = t.node_id
      WHERE t.node_id = NEW.node_id AND t.state = 'active' AND r.state = 'active'
    )
    BEGIN SELECT RAISE(ABORT, 'health state requires an active trusted peer'); END;
    INSERT INTO health_local (key, value) VALUES ('signal_sequence', 0), ('signals_dropped', 0);
";

fn validate_v1_preflight(
    connection: &Connection,
    registry: &NodeRegistry,
) -> Result<(), RegistryError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != 1 {
        return Err(RegistryError::InvalidSchema(
            "v1 preflight requires schema version 1".to_string(),
        ));
    }
    let expected_metadata = [
        ("schema_version", "1"),
        ("node_id", registry.local_node_id.as_str()),
        ("public_key_encoding", "x-only-bip340-hex-lowercase"),
    ];
    let metadata: Vec<(String, String)> = connection
        .prepare("SELECT key, value FROM metadata ORDER BY key")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    if metadata.len() != expected_metadata.len()
        || expected_metadata.iter().any(|expected| {
            !metadata
                .iter()
                .any(|actual| actual.0 == expected.0 && actual.1 == expected.1)
        })
    {
        return Err(RegistryError::InvalidSchema(
            "v1 metadata does not match the active identity".to_string(),
        ));
    }
    validate_columns(connection, "metadata", &["key", "value"])?;
    validate_columns(
        connection,
        "peers",
        &[
            "node_id",
            "public_key",
            "role",
            "state",
            "capabilities_json",
            "added_at",
            "updated_at",
            "last_seen",
            "source",
        ],
    )?;
    validate_columns(
        connection,
        "revocations",
        &[
            "id",
            "node_id",
            "public_key",
            "revoked_at",
            "reason",
            "replacement_node_id",
        ],
    )?;
    validate_columns(
        connection,
        "audit_events",
        &[
            "id",
            "event_type",
            "node_id",
            "from_state",
            "to_state",
            "actor",
            "reason",
            "occurred_at",
        ],
    )?;
    validate_columns(
        connection,
        "replay_keys",
        &["key", "first_seen", "expires_at"],
    )?;
    validate_columns(
        connection,
        "inbox",
        &[
            "cue_id",
            "state",
            "received_at",
            "updated_at",
            "expires_at",
            "outcome_hash",
        ],
    )?;
    let actual_objects: Vec<(String, String)> = connection
        .prepare(
            "SELECT type, name FROM sqlite_master
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let expected_objects = vec![
        ("index".to_string(), "audit_events_node_idx".to_string()),
        ("index".to_string(), "peers_state_idx".to_string()),
        ("table".to_string(), "audit_events".to_string()),
        ("table".to_string(), "inbox".to_string()),
        ("table".to_string(), "metadata".to_string()),
        ("table".to_string(), "peers".to_string()),
        ("table".to_string(), "replay_keys".to_string()),
        ("table".to_string(), "revocations".to_string()),
        ("trigger".to_string(), "audit_events_no_delete".to_string()),
        ("trigger".to_string(), "audit_events_no_update".to_string()),
        ("trigger".to_string(), "revocations_no_delete".to_string()),
        ("trigger".to_string(), "revocations_no_update".to_string()),
    ];
    if actual_objects != expected_objects {
        return Err(RegistryError::InvalidSchema(
            "v1 database contains unexpected or missing schema objects".to_string(),
        ));
    }
    validate_all_rows(connection)
}

fn create_v2_schema(transaction: &Transaction<'_>) -> Result<(), RegistryError> {
    transaction.execute_batch(
        "CREATE TABLE remote_identities (
          node_id TEXT PRIMARY KEY CHECK (length(CAST(node_id AS BLOB)) = 69),
          identity_key BLOB NOT NULL UNIQUE CHECK (length(identity_key) = 32),
          state TEXT NOT NULL CHECK (state IN ('authenticated_untrusted', 'active', 'revoked')),
          first_seen INTEGER NOT NULL CHECK (first_seen > 0),
          revoked_at INTEGER NULL CHECK (revoked_at IS NULL OR revoked_at >= first_seen)
        );
        CREATE TABLE trusted_peers (
          node_id TEXT PRIMARY KEY REFERENCES remote_identities(node_id),
          role INTEGER NOT NULL CHECK (role IN (1, 2)),
          capabilities BLOB NOT NULL CHECK (length(capabilities) <= 4096),
          state TEXT NOT NULL CHECK (state IN ('active', 'revoked')),
          added_at INTEGER NOT NULL CHECK (added_at > 0),
          updated_at INTEGER NOT NULL CHECK (updated_at >= added_at)
        );
        CREATE TABLE transport_key_epochs (
          node_id TEXT NOT NULL REFERENCES remote_identities(node_id),
          key_epoch INTEGER NOT NULL CHECK (key_epoch > 0),
          public_key BLOB NOT NULL CHECK (length(public_key) = 32),
          certificate BLOB NOT NULL CHECK (length(certificate) = 245),
          state TEXT NOT NULL CHECK (state IN ('pending', 'active', 'revoked')),
          added_at INTEGER NOT NULL CHECK (added_at > 0),
          retired_at INTEGER NULL CHECK (retired_at IS NULL OR retired_at >= added_at),
          PRIMARY KEY (node_id, key_epoch),
          UNIQUE (node_id, public_key)
        );
        CREATE TABLE channel_sessions (
          session_id BLOB PRIMARY KEY CHECK (length(session_id) = 32),
          node_id TEXT NOT NULL REFERENCES remote_identities(node_id),
          direction INTEGER NOT NULL CHECK (direction IN (0, 1)),
          send_sequence INTEGER NOT NULL CHECK (send_sequence >= 0),
          receive_sequence INTEGER NOT NULL CHECK (receive_sequence >= 0),
          state TEXT NOT NULL CHECK (state IN ('handshaking', 'authenticated_untrusted', 'active', 'closed')),
          started_at INTEGER NOT NULL CHECK (started_at > 0),
          last_seen INTEGER NOT NULL CHECK (last_seen >= started_at),
          expires_at INTEGER NOT NULL CHECK (expires_at >= last_seen)
        );
        CREATE TABLE enrollment_replays (
          replay_kind TEXT NOT NULL CHECK (replay_kind IN ('bundle', 'manual_request')),
          replay_id BLOB NOT NULL CHECK (length(replay_id) = 16),
          expires_at INTEGER NOT NULL CHECK (expires_at > 0),
          first_seen INTEGER NOT NULL CHECK (first_seen > 0),
          PRIMARY KEY (replay_kind, replay_id)
        );
        CREATE TABLE transport_audit (
          id INTEGER PRIMARY KEY,
          event_type TEXT NOT NULL CHECK (length(CAST(event_type AS BLOB)) BETWEEN 1 AND 64),
          node_id TEXT NOT NULL,
          session_id BLOB NULL CHECK (session_id IS NULL OR length(session_id) = 32),
          bundle_id BLOB NULL CHECK (bundle_id IS NULL OR length(bundle_id) = 16),
          direction INTEGER NULL CHECK (direction IS NULL OR direction IN (0, 1)),
          byte_count INTEGER NOT NULL CHECK (byte_count >= 0),
           outcome TEXT NOT NULL CHECK (length(CAST(outcome AS BLOB)) BETWEEN 1 AND 32),
           error_code INTEGER NULL CHECK (error_code IS NULL OR error_code BETWEEN 1000 AND 1999),
           cue_id TEXT NULL CHECK (cue_id IS NULL OR length(cue_id) = 32),
           cue_script TEXT NULL CHECK (cue_script IS NULL OR length(CAST(cue_script AS BLOB)) BETWEEN 1 AND 64),
           cue_reason TEXT NULL CHECK (cue_reason IS NULL OR length(CAST(cue_reason AS BLOB)) BETWEEN 1 AND 128),
           occurred_at INTEGER NOT NULL CHECK (occurred_at > 0)
        );
        CREATE TABLE cue_rate_limits (
          node_id TEXT PRIMARY KEY,
          window_start INTEGER NOT NULL CHECK (window_start > 0),
          count INTEGER NOT NULL CHECK (count >= 0)
        );
        CREATE INDEX transport_key_epochs_state_idx ON transport_key_epochs(state, node_id);
        CREATE UNIQUE INDEX transport_key_epochs_one_active
          ON transport_key_epochs(node_id) WHERE state = 'active';
        CREATE INDEX channel_sessions_peer_idx ON channel_sessions(node_id, state, last_seen);
         CREATE INDEX enrollment_replays_expiry_idx ON enrollment_replays(expires_at);
         CREATE INDEX transport_audit_node_idx ON transport_audit(node_id, id);
         CREATE INDEX transport_audit_expiry_idx ON transport_audit(occurred_at);
         CREATE TRIGGER trusted_peers_require_known_identity
        BEFORE INSERT ON trusted_peers
        WHEN (SELECT state FROM remote_identities WHERE node_id = NEW.node_id)
          NOT IN ('authenticated_untrusted', 'active')
        BEGIN SELECT RAISE(ABORT, 'trusted peer requires known identity'); END;
        CREATE TRIGGER transport_key_epochs_active_require_trust
        BEFORE INSERT ON transport_key_epochs
          WHEN NEW.state = 'active' AND (
          (SELECT state FROM remote_identities WHERE node_id = NEW.node_id) <> 'active'
          OR NOT EXISTS (SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active')
        )
        BEGIN SELECT RAISE(ABORT, 'active transport key requires active trusted peer'); END;
        CREATE TRIGGER transport_key_epochs_active_update_require_trust
        BEFORE UPDATE OF state ON transport_key_epochs
          WHEN NEW.state = 'active' AND (
          (SELECT state FROM remote_identities WHERE node_id = NEW.node_id) <> 'active'
          OR NOT EXISTS (SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active')
        )
        BEGIN SELECT RAISE(ABORT, 'active transport key requires active trusted peer'); END;
        CREATE TRIGGER remote_identities_no_untrusted_trust_update
        BEFORE UPDATE OF state ON remote_identities
        WHEN NEW.state = 'active' AND NOT EXISTS (
          SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id
        )
        BEGIN SELECT RAISE(ABORT, 'active identity requires trusted peer'); END;
        CREATE TRIGGER trusted_peers_no_identity_demotion
        BEFORE UPDATE OF state ON remote_identities
        WHEN NEW.state <> 'active' AND EXISTS (
          SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active'
        )
        BEGIN SELECT RAISE(ABORT, 'trusted peer must be revoked before identity demotion'); END;
        CREATE TRIGGER channel_sessions_active_requires_trust
        BEFORE INSERT ON channel_sessions
        WHEN NEW.state = 'active' AND (
          (SELECT state FROM remote_identities WHERE node_id = NEW.node_id) <> 'active'
          OR NOT EXISTS (SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active')
        )
        BEGIN SELECT RAISE(ABORT, 'active session requires active trusted peer'); END;
        CREATE TRIGGER channel_sessions_active_update_requires_trust
        BEFORE UPDATE OF state, node_id ON channel_sessions
        WHEN NEW.state = 'active' AND (
          (SELECT state FROM remote_identities WHERE node_id = NEW.node_id) <> 'active'
          OR NOT EXISTS (SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active')
        )
        BEGIN SELECT RAISE(ABORT, 'active session requires active trusted peer'); END;
        CREATE TRIGGER transport_key_epochs_monotonic_insert
        BEFORE INSERT ON transport_key_epochs
        WHEN NEW.key_epoch <= COALESCE((SELECT MAX(key_epoch) FROM transport_key_epochs WHERE node_id = NEW.node_id), 0)
        BEGIN SELECT RAISE(ABORT, 'transport key epoch must increase'); END;
        CREATE TRIGGER transport_key_epochs_monotonic_update
        BEFORE UPDATE OF key_epoch ON transport_key_epochs
        WHEN NEW.key_epoch <= COALESCE((SELECT MAX(key_epoch) FROM transport_key_epochs WHERE node_id = NEW.node_id AND key_epoch <> OLD.key_epoch), 0)
        BEGIN SELECT RAISE(ABORT, 'transport key epoch must increase'); END;
        CREATE TRIGGER remote_identities_no_delete
        BEFORE DELETE ON remote_identities
        BEGIN SELECT RAISE(ABORT, 'remote identities are retained'); END;
        CREATE TRIGGER trusted_peers_no_delete
        BEFORE DELETE ON trusted_peers
        BEGIN SELECT RAISE(ABORT, 'trusted peer history is retained'); END;
        CREATE TRIGGER transport_key_epochs_no_delete
        BEFORE DELETE ON transport_key_epochs
        BEGIN SELECT RAISE(ABORT, 'transport key epochs are retained'); END;
        CREATE TRIGGER revoked_identity_no_resurrection
        BEFORE UPDATE OF state ON remote_identities
        WHEN OLD.state = 'revoked' AND NEW.state <> 'revoked'
        BEGIN SELECT RAISE(ABORT, 'revoked identity cannot be resurrected'); END;
        CREATE TRIGGER revoked_trusted_peer_no_resurrection
        BEFORE UPDATE OF state ON trusted_peers
        WHEN OLD.state = 'revoked' AND NEW.state <> 'revoked'
        BEGIN SELECT RAISE(ABORT, 'revoked trust cannot be resurrected'); END;
        CREATE TRIGGER revoked_transport_epoch_no_resurrection
        BEFORE UPDATE OF state ON transport_key_epochs
        WHEN OLD.state = 'revoked' AND NEW.state <> 'revoked'
        BEGIN SELECT RAISE(ABORT, 'revoked transport epoch cannot be resurrected'); END;",
    )?;
    Ok(())
}

fn create_v3_schema(transaction: &Transaction<'_>) -> Result<(), RegistryError> {
    transaction.execute_batch(
        "CREATE TABLE manual_enrollment_requests (
           request_id BLOB PRIMARY KEY CHECK (length(request_id) = 16),
           request_bytes BLOB NOT NULL CHECK (length(request_bytes) BETWEEN 1 AND 2048),
           request_digest BLOB NOT NULL CHECK (length(request_digest) = 32),
           code_hash BLOB NOT NULL CHECK (length(code_hash) = 32),
           node_id TEXT NOT NULL CHECK (length(CAST(node_id AS BLOB)) = 69),
           identity_key BLOB NOT NULL CHECK (length(identity_key) = 32),
           transport_key BLOB NOT NULL CHECK (length(transport_key) = 32),
           role INTEGER NOT NULL CHECK (role IN (1, 2)),
           capabilities BLOB NOT NULL CHECK (length(capabilities) <= 4096),
           request_created_at INTEGER NOT NULL CHECK (request_created_at > 0),
           request_expires_at INTEGER NOT NULL CHECK (request_expires_at > request_created_at),
           certificate BLOB NOT NULL CHECK (length(certificate) = 245),
           certificate_digest BLOB NOT NULL CHECK (length(certificate_digest) = 32),
           certificate_id BLOB NOT NULL CHECK (length(certificate_id) = 16),
           key_epoch INTEGER NOT NULL CHECK (key_epoch > 0),
           not_before INTEGER NOT NULL CHECK (not_before > 0),
           not_after INTEGER NOT NULL CHECK (not_after > not_before),
           state TEXT NOT NULL CHECK (state IN ('pending', 'approved', 'rejected')),
           source TEXT NOT NULL CHECK (source = 'manual'),
           staged_at INTEGER NOT NULL CHECK (staged_at > 0),
           resolved_at INTEGER NULL CHECK (resolved_at IS NULL OR resolved_at >= staged_at)
         );
         CREATE TABLE enrollment_audits (
           id INTEGER PRIMARY KEY,
           event_code TEXT NOT NULL CHECK (length(CAST(event_code AS BLOB)) BETWEEN 1 AND 64),
           request_id BLOB NULL CHECK (request_id IS NULL OR length(request_id) = 16),
           request_digest BLOB NULL CHECK (request_digest IS NULL OR length(request_digest) = 32),
           node_id TEXT NOT NULL,
           outcome TEXT NOT NULL CHECK (length(CAST(outcome AS BLOB)) BETWEEN 1 AND 32),
           detail TEXT NOT NULL CHECK (length(CAST(detail AS BLOB)) <= 256),
           occurred_at INTEGER NOT NULL CHECK (occurred_at > 0)
         );
         CREATE INDEX manual_enrollment_requests_state_idx
           ON manual_enrollment_requests(state, node_id);
         CREATE INDEX enrollment_audits_node_idx ON enrollment_audits(node_id, id);
         CREATE INDEX enrollment_audits_request_idx ON enrollment_audits(request_id, id);
         CREATE TRIGGER manual_enrollment_request_immutable
         BEFORE UPDATE ON manual_enrollment_requests
          WHEN NEW.request_id <> OLD.request_id
            OR NEW.request_bytes <> OLD.request_bytes
           OR NEW.request_digest <> OLD.request_digest
           OR NEW.code_hash <> OLD.code_hash
           OR NEW.node_id <> OLD.node_id
           OR NEW.identity_key <> OLD.identity_key
           OR NEW.transport_key <> OLD.transport_key
           OR NEW.role <> OLD.role
           OR NEW.capabilities <> OLD.capabilities
           OR NEW.request_created_at <> OLD.request_created_at
           OR NEW.request_expires_at <> OLD.request_expires_at
           OR NEW.certificate <> OLD.certificate
           OR NEW.certificate_digest <> OLD.certificate_digest
           OR NEW.certificate_id <> OLD.certificate_id
           OR NEW.key_epoch <> OLD.key_epoch
           OR NEW.not_before <> OLD.not_before
           OR NEW.not_after <> OLD.not_after
           OR NEW.source <> OLD.source
           OR NEW.staged_at <> OLD.staged_at
         BEGIN SELECT RAISE(ABORT, 'manual enrollment evidence is immutable'); END;
         CREATE TRIGGER enrollment_audits_no_update
         BEFORE UPDATE ON enrollment_audits
         BEGIN SELECT RAISE(ABORT, 'enrollment audits are append-only'); END;
         CREATE TRIGGER enrollment_audits_no_delete
         BEFORE DELETE ON enrollment_audits
         BEGIN SELECT RAISE(ABORT, 'enrollment audits are append-only'); END;",
    )?;
    Ok(())
}

fn validate_v2_invariants(
    connection: &Connection,
    _registry: &NodeRegistry,
) -> Result<(), RegistryError> {
    let active_without_trust: i64 = connection.query_row(
        "SELECT COUNT(*) FROM remote_identities WHERE state = 'active' AND NOT EXISTS
         (SELECT 1 FROM trusted_peers WHERE node_id = remote_identities.node_id AND state = 'active')",
        [],
        |row| row.get(0),
    )?;
    if active_without_trust != 0 {
        return Err(RegistryError::InvalidSchema(
            "active identity has no active trusted peer".to_string(),
        ));
    }
    Ok(())
}

fn timestamp_seconds(value: &str) -> Result<i64, RegistryError> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|_| RegistryError::InvalidSchema(format!("invalid v1 timestamp {value:?}")))?;
    if parsed.offset().local_minus_utc() != 0 {
        return Err(RegistryError::InvalidSchema(format!(
            "v1 timestamp is not UTC: {value:?}"
        )));
    }
    let seconds = parsed.timestamp();
    if seconds <= 0 {
        return Err(RegistryError::InvalidSchema(
            "v1 timestamp is not positive".to_string(),
        ));
    }
    Ok(seconds)
}

fn validate_capabilities_json_bytes(value: &str) -> Result<(), RegistryError> {
    let capabilities: Vec<String> = serde_json::from_str(value).map_err(|_| {
        RegistryError::InvalidSchema("v1 capabilities are not valid JSON".to_string())
    })?;
    validate_capabilities(&capabilities)
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn bundle_failure_code(error: &RegistryError) -> &'static str {
    match error {
        RegistryError::BundleReplay | RegistryError::BootstrapProofConsumed => "replay",
        RegistryError::ConductorConflict | RegistryError::BundleConflict => "concurrent",
        RegistryError::Revoked(_) => "revoked_authority",
        RegistryError::BundleCapacity => "capacity",
        RegistryError::BundleRateLimited => "rate_limited",
        _ => "rejected",
    }
}

fn bundle_failure_detail(error: &RegistryError) -> &'static str {
    match error {
        RegistryError::BundleReplay => "signed enrollment bundle was already consumed",
        RegistryError::BootstrapProofConsumed => "bootstrap proof was already consumed",
        RegistryError::ConductorConflict => "an active conductor already exists",
        RegistryError::PublisherConductorConflict => {
            "this node publishes baselines and cannot also conduct"
        }
        RegistryError::BundleConflict => "signed enrollment conflicts with existing trust state",
        RegistryError::Revoked(_) => "signed enrollment identity is retained as revoked",
        RegistryError::BundleCapacity => "signed enrollment capacity is exhausted",
        RegistryError::BundleRateLimited => "signed enrollment rate limit exceeded",
        _ => "signed enrollment activation was rejected",
    }
}

fn cleanup_enrollment_replays(
    transaction: &Transaction<'_>,
    now: u64,
) -> Result<(), RegistryError> {
    let now = i64::try_from(now)
        .map_err(|_| RegistryError::InvalidInput("enrollment timestamp is too large".into()))?;
    transaction.execute(
        "DELETE FROM enrollment_replays
         WHERE rowid IN (
           SELECT rowid FROM enrollment_replays
           WHERE replay_kind IN ('manual_request', 'bundle') AND expires_at <= ?1
           ORDER BY expires_at LIMIT ?2
         )",
        params![now, MAX_ENROLLMENT_CLEANUP_ROWS],
    )?;
    Ok(())
}

fn cleanup_bootstrap_proofs(transaction: &Transaction<'_>, now: u64) -> Result<(), RegistryError> {
    let now = i64::try_from(now)
        .map_err(|_| RegistryError::InvalidInput("enrollment timestamp is too large".into()))?;
    transaction.execute(
        "DELETE FROM bootstrap_proofs
         WHERE rowid IN (
           SELECT rowid FROM bootstrap_proofs
           WHERE consumed_at IS NULL AND expires_at <= ?1
           ORDER BY expires_at LIMIT ?2
         )",
        params![now, MAX_ENROLLMENT_CLEANUP_ROWS],
    )?;
    Ok(())
}

fn validate_schema(connection: &Connection, registry: &NodeRegistry) -> Result<(), RegistryError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let health_plane: Option<String> = connection
        .query_row(
            "SELECT value FROM metadata WHERE key = 'health_plane'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    // A node whose Health Plane migration failed stays at version 6 with the
    // plane disabled and keeps serving transport, enrollment, HTTP, and runs.
    let health_plane_present = match (version, health_plane.as_deref()) {
        (SCHEMA_VERSION, Some(HEALTH_PLANE_ENABLED)) => true,
        (6, Some(HEALTH_PLANE_DISABLED)) => false,
        _ => {
            return Err(RegistryError::InvalidSchema(format!(
                "database schema marker is {version}, expected {SCHEMA_VERSION}"
            )))
        }
    };
    validate_objects(connection, health_plane_present)?;
    validate_columns(connection, "metadata", &["key", "value"])?;
    validate_columns(
        connection,
        "peers",
        &[
            "node_id",
            "public_key",
            "role",
            "state",
            "capabilities_json",
            "added_at",
            "updated_at",
            "last_seen",
            "source",
        ],
    )?;
    validate_columns(
        connection,
        "revocations",
        &[
            "id",
            "node_id",
            "public_key",
            "revoked_at",
            "reason",
            "replacement_node_id",
        ],
    )?;
    validate_columns(
        connection,
        "audit_events",
        &[
            "id",
            "event_type",
            "node_id",
            "from_state",
            "to_state",
            "actor",
            "reason",
            "occurred_at",
        ],
    )?;
    validate_columns(
        connection,
        "replay_keys",
        &["key", "first_seen", "expires_at"],
    )?;
    validate_columns(
        connection,
        "inbox",
        &[
            "cue_id",
            "state",
            "received_at",
            "updated_at",
            "expires_at",
            "outcome_hash",
        ],
    )?;
    validate_columns(
        connection,
        "remote_identities",
        &[
            "node_id",
            "identity_key",
            "state",
            "first_seen",
            "revoked_at",
        ],
    )?;
    validate_columns(
        connection,
        "trusted_peers",
        &[
            "node_id",
            "role",
            "capabilities",
            "state",
            "added_at",
            "updated_at",
        ],
    )?;
    validate_columns(
        connection,
        "transport_key_epochs",
        &[
            "node_id",
            "key_epoch",
            "public_key",
            "certificate",
            "state",
            "added_at",
            "retired_at",
        ],
    )?;
    validate_columns(
        connection,
        "channel_sessions",
        &[
            "session_id",
            "node_id",
            "direction",
            "send_sequence",
            "receive_sequence",
            "state",
            "started_at",
            "last_seen",
            "expires_at",
        ],
    )?;
    validate_columns(
        connection,
        "enrollment_replays",
        &["replay_kind", "replay_id", "expires_at", "first_seen"],
    )?;
    validate_columns(
        connection,
        "transport_audit",
        &[
            "id",
            "event_type",
            "node_id",
            "session_id",
            "bundle_id",
            "direction",
            "byte_count",
            "outcome",
            "error_code",
            "cue_id",
            "cue_script",
            "cue_reason",
            "occurred_at",
        ],
    )?;
    validate_columns(
        connection,
        "cue_rate_limits",
        &["node_id", "window_start", "count"],
    )?;
    validate_columns(
        connection,
        "manual_enrollment_requests",
        &[
            "request_id",
            "request_bytes",
            "request_digest",
            "code_hash",
            "node_id",
            "identity_key",
            "transport_key",
            "role",
            "capabilities",
            "request_created_at",
            "request_expires_at",
            "certificate",
            "certificate_digest",
            "certificate_id",
            "key_epoch",
            "not_before",
            "not_after",
            "state",
            "source",
            "staged_at",
            "resolved_at",
            "pairing_id",
        ],
    )?;
    validate_columns(
        connection,
        "enrollment_audits",
        &[
            "id",
            "event_code",
            "request_id",
            "request_digest",
            "node_id",
            "outcome",
            "detail",
            "occurred_at",
        ],
    )?;
    validate_columns(
        connection,
        "bootstrap_proofs",
        &[
            "target_node_id",
            "organization",
            "token_hash",
            "nonce_hash",
            "expires_at",
            "consumed_at",
            "bundle_id",
            "cleanup_state",
        ],
    )?;
    if health_plane_present {
        validate_health_plane_columns(connection)?;
    }
    let metadata: Vec<(String, String)> = connection
        .prepare("SELECT key, value FROM metadata ORDER BY key")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    if metadata.len() != 4 {
        return Err(RegistryError::InvalidSchema(
            "metadata contains unexpected keys".to_string(),
        ));
    }
    let expected = [
        (
            "schema_version",
            if health_plane_present { "8" } else { "6" },
        ),
        (
            "health_plane",
            if health_plane_present {
                HEALTH_PLANE_ENABLED
            } else {
                HEALTH_PLANE_DISABLED
            },
        ),
        ("node_id", registry.local_node_id.as_str()),
        ("public_key_encoding", "x-only-bip340-hex-lowercase"),
    ];
    for (key, value) in expected {
        if !metadata.iter().any(|item| item.0 == key && item.1 == value) {
            return Err(RegistryError::InvalidSchema(format!(
                "metadata {key:?} does not match the active identity"
            )));
        }
    }
    validate_all_rows(connection)
}

fn validate_objects(
    connection: &Connection,
    health_plane_present: bool,
) -> Result<(), RegistryError> {
    let actual: Vec<(String, String)> = connection
        .prepare(
            "SELECT type, name FROM sqlite_master
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut expected = vec![
        ("index".to_string(), "audit_events_node_idx".to_string()),
        (
            "index".to_string(),
            "bootstrap_proofs_expiry_idx".to_string(),
        ),
        ("index".to_string(), "channel_sessions_peer_idx".to_string()),
        (
            "index".to_string(),
            "enrollment_audits_node_idx".to_string(),
        ),
        (
            "index".to_string(),
            "enrollment_audits_request_idx".to_string(),
        ),
        (
            "index".to_string(),
            "enrollment_replays_expiry_idx".to_string(),
        ),
        (
            "index".to_string(),
            "manual_enrollment_requests_pairing_idx".to_string(),
        ),
        (
            "index".to_string(),
            "manual_enrollment_requests_state_idx".to_string(),
        ),
        ("index".to_string(), "peers_state_idx".to_string()),
        (
            "index".to_string(),
            "transport_audit_expiry_idx".to_string(),
        ),
        ("index".to_string(), "transport_audit_node_idx".to_string()),
        (
            "index".to_string(),
            "transport_key_epochs_one_active".to_string(),
        ),
        (
            "index".to_string(),
            "transport_key_epochs_state_idx".to_string(),
        ),
        ("table".to_string(), "audit_events".to_string()),
        ("table".to_string(), "bootstrap_proofs".to_string()),
        ("table".to_string(), "channel_sessions".to_string()),
        ("table".to_string(), "cue_rate_limits".to_string()),
        ("table".to_string(), "enrollment_audits".to_string()),
        ("table".to_string(), "enrollment_replays".to_string()),
        ("table".to_string(), "inbox".to_string()),
        (
            "table".to_string(),
            "manual_enrollment_requests".to_string(),
        ),
        ("table".to_string(), "metadata".to_string()),
        ("table".to_string(), "peers".to_string()),
        ("table".to_string(), "remote_identities".to_string()),
        ("table".to_string(), "replay_keys".to_string()),
        ("table".to_string(), "revocations".to_string()),
        ("table".to_string(), "transport_audit".to_string()),
        ("table".to_string(), "transport_key_epochs".to_string()),
        ("table".to_string(), "trusted_peers".to_string()),
        ("trigger".to_string(), "audit_events_no_delete".to_string()),
        ("trigger".to_string(), "audit_events_no_update".to_string()),
        (
            "trigger".to_string(),
            "bootstrap_proofs_no_update".to_string(),
        ),
        (
            "trigger".to_string(),
            "channel_sessions_active_requires_trust".to_string(),
        ),
        (
            "trigger".to_string(),
            "channel_sessions_active_update_requires_trust".to_string(),
        ),
        (
            "trigger".to_string(),
            "enrollment_audits_no_delete".to_string(),
        ),
        (
            "trigger".to_string(),
            "enrollment_audits_no_update".to_string(),
        ),
        (
            "trigger".to_string(),
            "manual_enrollment_request_immutable".to_string(),
        ),
        (
            "trigger".to_string(),
            "remote_identities_no_delete".to_string(),
        ),
        (
            "trigger".to_string(),
            "remote_identities_no_untrusted_trust_update".to_string(),
        ),
        ("trigger".to_string(), "revocations_no_delete".to_string()),
        ("trigger".to_string(), "revocations_no_update".to_string()),
        (
            "trigger".to_string(),
            "revoked_identity_no_resurrection".to_string(),
        ),
        (
            "trigger".to_string(),
            "revoked_transport_epoch_no_resurrection".to_string(),
        ),
        (
            "trigger".to_string(),
            "revoked_trusted_peer_no_resurrection".to_string(),
        ),
        (
            "trigger".to_string(),
            "transport_key_epochs_active_require_trust".to_string(),
        ),
        (
            "trigger".to_string(),
            "transport_key_epochs_active_update_require_trust".to_string(),
        ),
        (
            "trigger".to_string(),
            "transport_key_epochs_monotonic_insert".to_string(),
        ),
        (
            "trigger".to_string(),
            "transport_key_epochs_monotonic_update".to_string(),
        ),
        (
            "trigger".to_string(),
            "transport_key_epochs_no_delete".to_string(),
        ),
        ("trigger".to_string(), "trusted_peers_no_delete".to_string()),
        (
            "trigger".to_string(),
            "trusted_peers_no_identity_demotion".to_string(),
        ),
        (
            "trigger".to_string(),
            "trusted_peers_require_known_identity".to_string(),
        ),
    ];
    if health_plane_present {
        expected.extend(
            HEALTH_PLANE_OBJECTS
                .iter()
                .map(|(object_type, name)| ((*object_type).to_string(), (*name).to_string())),
        );
        expected.sort();
    }
    if actual != expected {
        return Err(RegistryError::InvalidSchema(
            "database contains unexpected or missing schema objects".to_string(),
        ));
    }
    Ok(())
}

fn validate_health_plane_columns(connection: &Connection) -> Result<(), RegistryError> {
    validate_columns(
        connection,
        "health_peers",
        &[
            "node_id",
            "role",
            "cursor",
            "last_profile_revision",
            "last_pulse_sequence",
            "last_pulse_at",
            "version_incompatible_at",
            "minute_window_start",
            "minute_messages",
            "minute_signals",
            "hour_window_start",
            "hour_profiles",
            "first_seen",
            "updated_at",
        ],
    )?;
    validate_columns(
        connection,
        "health_profiles",
        &[
            "node_id",
            "profile_revision",
            "agent_version",
            "arch",
            "capabilities",
            "display_name",
            "distro_id",
            "distro_version",
            "omarchy_channel",
            "omarchy_version",
            "platform",
            "role",
            "runtimes",
            "message_bytes",
            "received_at",
            "baseline_id",
            "baseline_observed_id",
        ],
    )?;
    validate_columns(
        connection,
        "health_pulses",
        &[
            "node_id",
            "sequence",
            "emitted_at",
            "profile_revision",
            "runner_state",
            "scheduler_state",
            "queue_depth",
            "workers_busy",
            "workers_configured",
            "uptime_seconds",
            "last_run",
            "message_bytes",
            "received_at",
        ],
    )?;
    validate_columns(
        connection,
        "health_signals",
        &[
            "node_id",
            "signal_id",
            "sequence",
            "state",
            "kind",
            "occurred_at",
            "subject",
            "run",
            "message_bytes",
            "received_at",
            "expires_at",
        ],
    )?;
    validate_columns(
        connection,
        "health_outbox",
        &[
            "signal_id",
            "target_node_id",
            "sequence",
            "kind",
            "occurred_at",
            "subject",
            "run",
            "message_bytes",
            "attempts",
            "last_message_id",
            "enqueued_at",
            "updated_at",
            "expires_at",
        ],
    )?;
    validate_columns(
        connection,
        "health_replay_keys",
        &["message_id", "node_id", "first_seen", "expires_at"],
    )?;
    validate_columns(
        connection,
        "health_audit",
        &[
            "id",
            "event_code",
            "node_id",
            "message_kind",
            "byte_count",
            "outcome",
            "error_code",
            "occurred_at",
        ],
    )?;
    validate_columns(connection, "health_local", &["key", "value"])
}

/// Every object the schema version 7 Health Plane migration creates.
const HEALTH_PLANE_OBJECTS: [(&str, &str); 15] = [
    ("index", "health_audit_expiry_idx"),
    ("index", "health_outbox_order_idx"),
    ("index", "health_replay_keys_expiry_idx"),
    ("index", "health_signals_expiry_idx"),
    ("index", "health_signals_order_idx"),
    ("table", "health_audit"),
    ("table", "health_local"),
    ("table", "health_outbox"),
    ("table", "health_peers"),
    ("table", "health_profiles"),
    ("table", "health_pulses"),
    ("table", "health_replay_keys"),
    ("table", "health_signals"),
    ("trigger", "health_audit_no_update"),
    ("trigger", "health_peers_require_active_trust"),
];

fn validate_columns(
    connection: &Connection,
    table: &str,
    expected: &[&str],
) -> Result<(), RegistryError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let actual = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if actual != expected {
        return Err(RegistryError::InvalidSchema(format!(
            "table {table:?} has unexpected columns"
        )));
    }
    Ok(())
}

fn validate_all_rows(connection: &Connection) -> Result<(), RegistryError> {
    let mut peers = connection.prepare(
        "SELECT node_id, public_key, role, state, capabilities_json, added_at, updated_at, last_seen, source FROM peers",
    )?;
    for row in peers.query_map([], peer_from_row)? {
        row?;
    }
    let mut revocations = connection.prepare(
        "SELECT id, node_id, public_key, revoked_at, reason, replacement_node_id FROM revocations",
    )?;
    for row in revocations.query_map([], revocation_from_row)? {
        row?;
    }
    let mut audit = connection.prepare(
        "SELECT id, event_type, node_id, from_state, to_state, actor, reason, occurred_at FROM audit_events",
    )?;
    for row in audit.query_map([], audit_from_row)? {
        row?;
    }
    let mut replay = connection.prepare("SELECT key, first_seen, expires_at FROM replay_keys")?;
    for row in replay.query_map([], |row| {
        let key: String = row.get(0)?;
        let first_seen: String = row.get(1)?;
        let expires_at: String = row.get(2)?;
        Ok((key, first_seen, expires_at))
    })? {
        let (key, first_seen, expires_at) = row?;
        validate_bounded_text("replay key", &key, MAX_REASON_BYTES)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        validate_timestamp(&first_seen)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        validate_timestamp(&expires_at)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    }
    let mut inbox = connection.prepare(
        "SELECT cue_id, state, received_at, updated_at, expires_at, outcome_hash FROM inbox",
    )?;
    for row in inbox.query_map([], |row| {
        let cue_id: String = row.get(0)?;
        let state: String = row.get(1)?;
        let received_at: String = row.get(2)?;
        let updated_at: String = row.get(3)?;
        let expires_at: String = row.get(4)?;
        let outcome_hash: Option<String> = row.get(5)?;
        Ok((
            cue_id,
            state,
            received_at,
            updated_at,
            expires_at,
            outcome_hash,
        ))
    })? {
        let (cue_id, state, received_at, updated_at, expires_at, outcome_hash) = row?;
        validate_bounded_text("cue id", &cue_id, MAX_REASON_BYTES)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        if !matches!(
            state.as_str(),
            "received"
                | "accepted"
                | "running"
                | "succeeded"
                | "failed"
                | "rejected"
                | "expired"
                | "interrupted"
        ) {
            return Err(RegistryError::InvalidSchema(format!(
                "unknown inbox state {state:?}"
            )));
        }
        for timestamp in [received_at, updated_at, expires_at] {
            validate_timestamp(&timestamp)?;
        }
        if let Some(hash) = outcome_hash {
            validate_bounded_text("outcome hash", &hash, 256)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        }
    }
    let has_bootstrap_proofs: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'bootstrap_proofs')",
        [],
        |row| row.get::<_, i64>(0),
    )? != 0;
    if has_bootstrap_proofs {
        let mut proofs = connection.prepare(
            "SELECT target_node_id, organization, token_hash, nonce_hash, expires_at,
                    consumed_at, bundle_id, cleanup_state
             FROM bootstrap_proofs",
        )?;
        for row in proofs.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<Vec<u8>>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })? {
            let (
                target,
                organization,
                token_hash,
                nonce_hash,
                expires_at,
                consumed_at,
                bundle_id,
                cleanup_state,
            ) = row?;
            validate_node_id(&target)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            validate_bounded_text("bootstrap organization", &organization, 128)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            if token_hash.len() != 32
                || nonce_hash.len() != 32
                || expires_at <= 0
                || consumed_at.is_some_and(|value| value <= 0)
                || bundle_id.as_ref().is_some_and(|value| value.len() != 16)
                || cleanup_state
                    .as_deref()
                    .is_some_and(|value| !matches!(value, "pending" | "complete"))
                || (consumed_at.is_none() && cleanup_state.is_some())
                || (consumed_at.is_some() && cleanup_state.is_none())
            {
                return Err(RegistryError::InvalidSchema(
                    "bootstrap proof contains invalid evidence".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_identity(identity: &NodeIdentityStatus) -> Result<(String, String), RegistryError> {
    validate_public_key(&identity.public_key_hex)?;
    let public_key = identity.public_key_hex.to_ascii_lowercase();
    let bytes = decode_hex(&public_key)?;
    let derived = node_id_for_x_only_public_key(&bytes);
    if identity.node_id != derived {
        return Err(RegistryError::InvalidInput(
            "identity node ID does not match its public key".to_string(),
        ));
    }
    validate_node_id(&identity.node_id)?;
    Ok((identity.node_id.clone(), public_key))
}

fn validate_registration(
    registration: &PeerRegistration,
    local_node_id: &str,
    local_public_key: &str,
) -> Result<(), RegistryError> {
    validate_public_key(&registration.public_key)?;
    validate_node_id(&registration.node_id)?;
    if registration.node_id == local_node_id || registration.public_key == local_public_key {
        return Err(RegistryError::SelfTrust);
    }
    let bytes = decode_hex(&registration.public_key)?;
    if node_id_for_x_only_public_key(&bytes) != registration.node_id {
        return Err(RegistryError::InvalidInput(
            "peer node ID does not match its public key".to_string(),
        ));
    }
    validate_capabilities(&registration.capabilities)?;
    validate_actor_reason(&registration.actor, &registration.reason)?;
    Ok(())
}

fn validate_public_key(value: &str) -> Result<(), RegistryError> {
    if value.len() != PUBLIC_KEY_BYTES
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(RegistryError::InvalidInput(
            "public key must be 64 lowercase hexadecimal x-only bytes".to_string(),
        ));
    }
    let bytes = decode_hex(value)?;
    k256::schnorr::VerifyingKey::from_slice(&bytes).map_err(|_| {
        RegistryError::InvalidInput("public key is not a valid BIP-340 x-only key".to_string())
    })?;
    Ok(())
}

fn validate_node_id(value: &str) -> Result<(), RegistryError> {
    if value.len() != NODE_ID_BYTES
        || !value.starts_with("omk1_")
        || value[5..]
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(RegistryError::InvalidInput(
            "node ID must be omk1_ followed by 64 lowercase hexadecimal characters".to_string(),
        ));
    }
    Ok(())
}

fn decode_hex(value: &str) -> Result<Vec<u8>, RegistryError> {
    if !value.len().is_multiple_of(2) {
        return Err(RegistryError::InvalidInput(
            "hex value has odd length".to_string(),
        ));
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| RegistryError::InvalidInput("invalid hexadecimal value".to_string()))
        })
        .collect()
}

fn validate_capabilities(capabilities: &[String]) -> Result<(), RegistryError> {
    if capabilities.len() > MAX_CAPABILITIES {
        return Err(RegistryError::InvalidInput(
            "too many peer capabilities".to_string(),
        ));
    }
    let mut previous: Option<&str> = None;
    for capability in capabilities {
        if capability.is_empty()
            || capability.len() > MAX_CAPABILITY_BYTES
            || capability.bytes().any(|byte| {
                !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte))
            })
            || !SUPPORTED_CAPABILITIES.contains(&capability.as_str())
        {
            return Err(RegistryError::InvalidInput(format!(
                "unsupported or invalid capability {capability:?}"
            )));
        }
        if previous.is_some_and(|value| value >= capability.as_str()) {
            return Err(RegistryError::InvalidInput(
                "capabilities must be sorted and unique".to_string(),
            ));
        }
        previous = Some(capability);
    }
    let json = capabilities_json(capabilities)?;
    if json.len() > MAX_CAPABILITIES_JSON_BYTES {
        return Err(RegistryError::InvalidInput(
            "capabilities JSON is too large".to_string(),
        ));
    }
    Ok(())
}

fn capabilities_json(capabilities: &[String]) -> Result<String, RegistryError> {
    validate_capabilities_without_json(capabilities)?;
    serde_json::to_string(capabilities)
        .map_err(|error| RegistryError::InvalidInput(format!("capabilities JSON: {error}")))
}

fn validate_capabilities_without_json(capabilities: &[String]) -> Result<(), RegistryError> {
    if capabilities.len() > MAX_CAPABILITIES {
        return Err(RegistryError::InvalidInput(
            "too many peer capabilities".to_string(),
        ));
    }
    Ok(())
}

fn validate_actor_reason(actor: &str, reason: &str) -> Result<(), RegistryError> {
    validate_bounded_text("actor", actor, MAX_ACTOR_BYTES)?;
    validate_bounded_text("reason", reason, MAX_REASON_BYTES)?;
    Ok(())
}

fn validate_bounded_text(label: &str, value: &str, max_bytes: usize) -> Result<(), RegistryError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(RegistryError::InvalidInput(format!(
            "{label} is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

fn validate_timestamp(value: &str) -> Result<(), RegistryError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| {
        RegistryError::InvalidSchema(format!("invalid RFC3339 timestamp {value:?}"))
    })?;
    if parsed.offset().local_minus_utc() != 0
        || parsed.to_rfc3339_opts(SecondsFormat::Millis, true) != value
    {
        return Err(RegistryError::InvalidSchema(format!(
            "timestamp is not canonical UTC RFC3339 milliseconds: {value:?}"
        )));
    }
    if parsed.with_timezone(&Utc) > Utc::now() + chrono::Duration::minutes(5) {
        return Err(RegistryError::InvalidSchema(format!(
            "timestamp is too far in the future: {value:?}"
        )));
    }
    Ok(())
}

fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn allowed_transition(from: PeerState, to: PeerState) -> bool {
    matches!(
        (from, to),
        (PeerState::Pending, PeerState::Active)
            | (PeerState::Pending, PeerState::Suspended)
            | (PeerState::Pending, PeerState::Revoked)
            | (PeerState::Active, PeerState::Suspended)
            | (PeerState::Active, PeerState::Revoked)
            | (PeerState::Suspended, PeerState::Active)
            | (PeerState::Suspended, PeerState::Revoked)
    )
}

fn peer_exists(transaction: &Transaction<'_>, node_id: &str) -> Result<bool, RegistryError> {
    Ok(transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM peers WHERE node_id = ?1)",
        [node_id],
        |row| row.get::<_, i64>(0),
    )? != 0)
}

fn public_key_exists(
    transaction: &Transaction<'_>,
    public_key: &str,
) -> Result<bool, RegistryError> {
    Ok(transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM peers WHERE public_key = ?1)",
        [public_key],
        |row| row.get::<_, i64>(0),
    )? != 0)
}

fn reject_retained_revocation(
    transaction: &Transaction<'_>,
    node_id: &str,
    public_key: &str,
) -> Result<(), RegistryError> {
    let revoked: Option<String> = transaction
        .query_row(
            "SELECT node_id FROM revocations WHERE node_id = ?1 OR public_key = ?2 LIMIT 1",
            params![node_id, public_key],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(revoked) = revoked {
        return Err(RegistryError::Revoked(revoked));
    }
    Ok(())
}

fn insert_revocation(
    transaction: &Transaction<'_>,
    peer: &PeerRecord,
    revoked_at: &str,
    reason: &str,
    replacement_node_id: Option<&str>,
) -> Result<(), RegistryError> {
    validate_bounded_text("reason", reason, MAX_REASON_BYTES)?;
    if let Some(replacement) = replacement_node_id {
        validate_node_id(replacement)?;
    }
    transaction.execute(
        "INSERT INTO revocations (node_id, public_key, revoked_at, reason, replacement_node_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            peer.node_id,
            peer.public_key,
            revoked_at,
            reason,
            replacement_node_id
        ],
    )?;
    Ok(())
}

fn insert_v2_trust_projection(
    transaction: &Transaction<'_>,
    registration: &PeerRegistration,
    now: i64,
    certificate: Option<&[u8]>,
) -> Result<(), RegistryError> {
    let identity_key = decode_hex(&registration.public_key)?;
    insert_v2_identity_projection(transaction, registration, now, "active")?;
    transaction.execute(
        "INSERT INTO trusted_peers
         (node_id, role, capabilities, state, added_at, updated_at)
         VALUES (?1, ?2, ?3, 'active', ?4, ?4)",
        params![
            registration.node_id,
            match registration.role {
                PeerRole::Conductor => 1,
                PeerRole::Performer => 2,
            },
            capabilities_json(&registration.capabilities)?.as_bytes(),
            now,
        ],
    )?;
    let Some(certificate) = certificate else {
        return Ok(());
    };
    if certificate.len() != TRANSPORT_CERTIFICATE_BYTES
        || &certificate[40..109] != registration.node_id.as_bytes()
        || &certificate[8..40] != identity_key.as_slice()
    {
        return Err(RegistryError::InvalidInput(
            "transport certificate does not match trusted identity".to_string(),
        ));
    }
    let key_epoch = u64::from_be_bytes(certificate[141..149].try_into().unwrap());
    if key_epoch == 0 {
        return Err(RegistryError::InvalidInput(
            "transport certificate epoch must be positive".to_string(),
        ));
    }
    transaction.execute(
        "INSERT INTO transport_key_epochs
         (node_id, key_epoch, public_key, certificate, state, added_at, retired_at)
         VALUES (?1, ?2, ?3, ?4, 'active', ?5, NULL)",
        params![
            registration.node_id,
            i64::try_from(key_epoch).map_err(|_| {
                RegistryError::InvalidInput("transport certificate epoch is too large".to_string())
            })?,
            &certificate[109..141],
            certificate,
            now,
        ],
    )?;
    Ok(())
}

fn insert_v2_identity_projection(
    transaction: &Transaction<'_>,
    registration: &PeerRegistration,
    now: i64,
    state: &str,
) -> Result<(), RegistryError> {
    let identity_key = decode_hex(&registration.public_key)?;
    transaction.execute(
        "INSERT INTO remote_identities
         (node_id, identity_key, state, first_seen, revoked_at)
         VALUES (?1, ?2, ?3, ?4, NULL)",
        params![registration.node_id, identity_key, state, now],
    )?;
    Ok(())
}

fn insert_v2_pending_transport_projection(
    transaction: &Transaction<'_>,
    registration: &PeerRegistration,
    now: i64,
    certificate: &[u8],
) -> Result<(), RegistryError> {
    let certificate = TransportCertificate::from_bytes(certificate)
        .map_err(|_| RegistryError::InvalidInput("transport certificate is invalid".into()))?;
    let identity_key = decode_hex(&registration.public_key)?;
    if certificate.node_id() != registration.node_id
        || certificate.identity_key().as_slice() != identity_key.as_slice()
    {
        return Err(RegistryError::InvalidInput(
            "transport certificate does not match trusted identity".into(),
        ));
    }
    let key_epoch = certificate.key_epoch();
    if key_epoch == 0 {
        return Err(RegistryError::InvalidInput(
            "transport certificate epoch must be positive".into(),
        ));
    }
    transaction.execute(
        "INSERT INTO transport_key_epochs
         (node_id, key_epoch, public_key, certificate, state, added_at, retired_at)
         VALUES (?1, ?2, ?3, ?4, 'pending', ?5, NULL)",
        params![
            registration.node_id,
            i64::try_from(key_epoch).map_err(|_| {
                RegistryError::InvalidInput("transport certificate epoch is too large".into())
            })?,
            certificate.transport_public().as_slice(),
            certificate.as_bytes().as_slice(),
            now,
        ],
    )?;
    Ok(())
}

fn registration_from_manual(
    request: &ManualEnrollmentRequest,
    actor: &str,
    reason: &str,
) -> Result<PeerRegistration, RegistryError> {
    let role = match request.role {
        crate::enrollment::EnrollmentRole::Conductor => PeerRole::Conductor,
        crate::enrollment::EnrollmentRole::Performer => PeerRole::Performer,
    };
    Ok(PeerRegistration {
        node_id: request.proposer_node_id.clone(),
        public_key: request.public_key_hex(),
        role,
        capabilities: request.capabilities.clone(),
        source: PeerSource::Manual,
        actor: actor.to_string(),
        reason: reason.to_string(),
    })
}

fn project_v2_transition(
    transaction: &Transaction<'_>,
    current: &PeerRecord,
    target: PeerState,
    now: i64,
) -> Result<(), RegistryError> {
    match target {
        PeerState::Active => {
            let identity_state: Option<String> = transaction
                .query_row(
                    "SELECT state FROM remote_identities WHERE node_id = ?1",
                    [&current.node_id],
                    |row| row.get(0),
                )
                .optional()?;
            if identity_state.is_none() {
                let registration = PeerRegistration {
                    node_id: current.node_id.clone(),
                    public_key: current.public_key.clone(),
                    role: current.role,
                    capabilities: current.capabilities.clone(),
                    source: current.source,
                    actor: "migration".to_string(),
                    reason: "v2 projection".to_string(),
                };
                insert_v2_identity_projection(
                    transaction,
                    &registration,
                    now,
                    "authenticated_untrusted",
                )?;
            }
            transaction.execute(
                "INSERT INTO trusted_peers (node_id, role, capabilities, state, added_at, updated_at)
                 VALUES (?1, ?2, ?3, 'active', ?4, ?4)
                 ON CONFLICT(node_id) DO UPDATE SET state = 'active', updated_at = excluded.updated_at",
                params![
                    current.node_id,
                    match current.role { PeerRole::Conductor => 1, PeerRole::Performer => 2 },
                    capabilities_json(&current.capabilities)?.as_bytes(),
                    now,
                ],
            )?;
            transaction.execute(
                "UPDATE remote_identities SET state = 'active', revoked_at = NULL WHERE node_id = ?1",
                [&current.node_id],
            )?;
            transaction.execute(
                "UPDATE transport_key_epochs SET state = 'active', retired_at = NULL
                 WHERE node_id = ?1 AND state = 'pending'",
                [&current.node_id],
            )?;
        }
        PeerState::Suspended => {
            transaction.execute(
                "UPDATE transport_key_epochs SET state = 'pending', retired_at = ?1
                 WHERE node_id = ?2 AND state = 'active'",
                params![now, current.node_id],
            )?;
            let identity_exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM remote_identities WHERE node_id = ?1)",
                [&current.node_id],
                |row| row.get::<_, i64>(0),
            )? != 0;
            if !identity_exists {
                let registration = PeerRegistration {
                    node_id: current.node_id.clone(),
                    public_key: current.public_key.clone(),
                    role: current.role,
                    capabilities: current.capabilities.clone(),
                    source: current.source,
                    actor: "migration".to_string(),
                    reason: "v2 projection".to_string(),
                };
                insert_v2_identity_projection(
                    transaction,
                    &registration,
                    now,
                    "authenticated_untrusted",
                )?;
            }
        }
        PeerState::Revoked => {
            transaction.execute(
                "UPDATE transport_key_epochs SET state = 'revoked', retired_at = ?1
                 WHERE node_id = ?2 AND state <> 'revoked'",
                params![now, current.node_id],
            )?;
            transaction.execute(
                "UPDATE trusted_peers SET state = 'revoked', updated_at = ?1 WHERE node_id = ?2 AND state <> 'revoked'",
                params![now, current.node_id],
            )?;
            transaction.execute(
                "UPDATE remote_identities SET state = 'revoked', revoked_at = ?1 WHERE node_id = ?2 AND state <> 'revoked'",
                params![now, current.node_id],
            )?;
        }
        PeerState::Pending => {}
    }
    Ok(())
}

fn project_v2_replacement(
    transaction: &Transaction<'_>,
    old: &PeerRecord,
    replacement: &PeerRegistration,
    now: i64,
) -> Result<(), RegistryError> {
    project_v2_transition(transaction, old, PeerState::Revoked, now)?;
    insert_v2_identity_projection(transaction, replacement, now, "authenticated_untrusted")
}

struct AuditInput<'a> {
    event_type: &'a str,
    node_id: &'a str,
    from_state: Option<PeerState>,
    to_state: Option<PeerState>,
    actor: &'a str,
    reason: &'a str,
    occurred_at: &'a str,
}

fn record_audit(transaction: &Transaction<'_>, input: AuditInput<'_>) -> Result<(), RegistryError> {
    validate_bounded_text("event type", input.event_type, 64)?;
    validate_node_id(input.node_id)?;
    validate_actor_reason(input.actor, input.reason)?;
    transaction.execute(
        "INSERT INTO audit_events (event_type, node_id, from_state, to_state, actor, reason, occurred_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            input.event_type,
            input.node_id,
            input.from_state.map(PeerState::as_str),
            input.to_state.map(PeerState::as_str),
            input.actor,
            input.reason,
            input.occurred_at,
        ],
    )?;
    Ok(())
}

fn load_peer(
    transaction: &Transaction<'_>,
    node_id: &str,
) -> Result<Option<PeerRecord>, RegistryError> {
    Ok(transaction
        .query_row(
            "SELECT node_id, public_key, role, state, capabilities_json, added_at, updated_at, last_seen, source
             FROM peers WHERE node_id = ?1",
            [node_id],
            peer_from_row,
        )
        .optional()?)
}

fn peer_from_row(row: &Row<'_>) -> rusqlite::Result<PeerRecord> {
    let node_id: String = row.get(0)?;
    let public_key: String = row.get(1)?;
    let role: String = row.get(2)?;
    let state: String = row.get(3)?;
    let capabilities_json: String = row.get(4)?;
    let added_at: String = row.get(5)?;
    let updated_at: String = row.get(6)?;
    let last_seen: Option<String> = row.get(7)?;
    let source: String = row.get(8)?;
    let capabilities: Vec<String> = serde_json::from_str::<Value>(&capabilities_json)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .and_then(|values| {
            values
                .into_iter()
                .map(|value| value.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| rusqlite::Error::InvalidQuery)?;
    validate_public_key(&public_key).map_err(sqlite_validation_error)?;
    validate_node_id(&node_id).map_err(sqlite_validation_error)?;
    if node_id_for_x_only_public_key(&decode_hex(&public_key).map_err(sqlite_validation_error)?)
        != node_id
    {
        return Err(sqlite_validation_error(RegistryError::InvalidSchema(
            "peer key and node ID do not pair".to_string(),
        )));
    }
    validate_capabilities(&capabilities).map_err(sqlite_validation_error)?;
    let canonical_capabilities = serde_json::to_string(&capabilities).map_err(|error| {
        sqlite_validation_error(RegistryError::InvalidSchema(error.to_string()))
    })?;
    if canonical_capabilities != capabilities_json {
        return Err(sqlite_validation_error(RegistryError::InvalidSchema(
            "capabilities JSON is not canonical".to_string(),
        )));
    }
    validate_timestamp(&added_at).map_err(sqlite_validation_error)?;
    validate_timestamp(&updated_at).map_err(sqlite_validation_error)?;
    if let Some(value) = &last_seen {
        validate_timestamp(value).map_err(sqlite_validation_error)?;
    }
    Ok(PeerRecord {
        node_id,
        public_key,
        role: PeerRole::parse(&role).map_err(sqlite_validation_error)?,
        state: PeerState::parse(&state).map_err(sqlite_validation_error)?,
        capabilities,
        added_at,
        updated_at,
        last_seen,
        source: PeerSource::parse(&source).map_err(sqlite_validation_error)?,
    })
}

fn revocation_from_row(row: &Row<'_>) -> rusqlite::Result<RevocationRecord> {
    let id = row.get(0)?;
    let node_id: String = row.get(1)?;
    let public_key: String = row.get(2)?;
    let revoked_at: String = row.get(3)?;
    let reason: String = row.get(4)?;
    let replacement_node_id: Option<String> = row.get(5)?;
    validate_node_id(&node_id).map_err(sqlite_validation_error)?;
    validate_public_key(&public_key).map_err(sqlite_validation_error)?;
    if node_id_for_x_only_public_key(&decode_hex(&public_key).map_err(sqlite_validation_error)?)
        != node_id
    {
        return Err(sqlite_validation_error(RegistryError::InvalidSchema(
            "revocation key and node ID do not pair".to_string(),
        )));
    }
    validate_timestamp(&revoked_at).map_err(sqlite_validation_error)?;
    validate_bounded_text("reason", &reason, MAX_REASON_BYTES).map_err(sqlite_validation_error)?;
    if let Some(value) = &replacement_node_id {
        validate_node_id(value).map_err(sqlite_validation_error)?;
    }
    Ok(RevocationRecord {
        id,
        node_id,
        public_key,
        revoked_at,
        reason,
        replacement_node_id,
    })
}

/// Read the bounded, newest-first lifecycle trust transitions on a connection
/// the caller owns.
///
/// The Health Plane Signal feed reads these rows inside the same transaction
/// that reads the Signal cursors, so the projected `enrolled` and `revoked`
/// Signals describe the same instant as everything beside them.
pub(crate) fn lifecycle_trust_events_in(
    connection: &Connection,
    limit: usize,
) -> Result<Vec<AuditEvent>, RegistryError> {
    let limit = limit.min(MAX_LIFECYCLE_SCAN_ROWS) as i64;
    let mut statement = connection.prepare(
        "SELECT id, event_type, node_id, from_state, to_state, actor, reason, occurred_at
         FROM audit_events
         WHERE to_state IN ('active', 'revoked')
         ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = statement.query_map(params![limit], audit_from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn audit_from_row(row: &Row<'_>) -> rusqlite::Result<AuditEvent> {
    let id = row.get(0)?;
    let event_type: String = row.get(1)?;
    let node_id: String = row.get(2)?;
    let from_state: Option<String> = row.get(3)?;
    let to_state: Option<String> = row.get(4)?;
    let actor: String = row.get(5)?;
    let reason: String = row.get(6)?;
    let occurred_at: String = row.get(7)?;
    validate_bounded_text("event type", &event_type, 64).map_err(sqlite_validation_error)?;
    validate_node_id(&node_id).map_err(sqlite_validation_error)?;
    validate_actor_reason(&actor, &reason).map_err(sqlite_validation_error)?;
    validate_timestamp(&occurred_at).map_err(sqlite_validation_error)?;
    Ok(AuditEvent {
        id,
        event_type,
        node_id,
        from_state: from_state
            .as_deref()
            .map(PeerState::parse)
            .transpose()
            .map_err(sqlite_validation_error)?,
        to_state: to_state
            .as_deref()
            .map(PeerState::parse)
            .transpose()
            .map_err(sqlite_validation_error)?,
        actor,
        reason,
        occurred_at,
    })
}

fn sqlite_validation_error(error: RegistryError) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NodePathOverrides, NodePlatform};
    use crate::node_identity::NodeIdentity;
    use std::fs;
    use std::sync::Arc;
    use std::thread;
    use tempfile::TempDir;

    fn context(temp: &TempDir) -> NodeContext {
        NodeContext::resolve_for(
            NodePlatform::current(),
            NodePathOverrides::new(
                Some(temp.path().join("state")),
                Some(temp.path().join("node.toml")),
            ),
            true,
            None,
            None,
            None,
        )
        .unwrap()
    }

    #[test]
    fn cue_rate_limit_is_durable_and_allows_the_frozen_burst() {
        let temp = TempDir::new().unwrap();
        let node_context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&node_context).unwrap();
        let registry = NodeRegistry::open(&node_context, identity.public_status()).unwrap();
        let peer = identity.public_status().node_id.clone();

        for _ in
            0..(crate::remote_cue::MAX_CUES_PER_MINUTE + crate::remote_cue::RATE_BURST_ALLOWANCE)
        {
            assert!(registry.consume_cue_rate(&peer, 100).unwrap());
        }
        assert!(!registry.consume_cue_rate(&peer, 101).unwrap());
        drop(registry);
        drop(identity);
        let identity = NodeIdentity::load_or_initialize(&node_context).unwrap();
        let registry = NodeRegistry::open(&node_context, identity.public_status()).unwrap();
        assert!(!registry.consume_cue_rate(&peer, 101).unwrap());
        assert!(registry.consume_cue_rate(&peer, 160).unwrap());
    }

    #[test]
    fn cue_audit_persists_correlation_without_changing_legacy_rows() {
        let temp = TempDir::new().unwrap();
        let node_context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&node_context).unwrap();
        let registry = NodeRegistry::open(&node_context, identity.public_status()).unwrap();
        let cue_id = "0123456789abcdef0123456789abcdef";

        registry
            .record_cue_transport_audit(
                "cue_rejected",
                &identity.public_status().node_id,
                None,
                None,
                0,
                "rejected",
                Some(1206),
                Some(cue_id),
                Some("deploy.sh"),
                Some("approved by operator"),
            )
            .unwrap();
        registry
            .record_transport_audit(
                "legacy_event",
                &identity.public_status().node_id,
                None,
                None,
                0,
                "accepted",
                None,
            )
            .unwrap();

        let connection = Connection::open(node_context.database_path()).unwrap();
        let stored: (String, String, String) = connection
            .query_row(
                "SELECT cue_id, cue_script, cue_reason FROM transport_audit
                 WHERE cue_id = ?1",
                [cue_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            stored,
            (
                cue_id.into(),
                "deploy.sh".into(),
                "approved by operator".into()
            )
        );
        let legacy_nulls: (Option<String>, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT cue_id, cue_script, cue_reason FROM transport_audit
                 WHERE event_type = 'legacy_event'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(legacy_nulls, (None, None, None));
    }

    fn seed_v3_pending_enrollment() -> (TempDir, NodeContext, NodeRegistry, [u8; 16], String) {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        fs::remove_file(context.database_path()).unwrap();

        let mut connection = Connection::open(context.database_path()).unwrap();
        configure_connection(&mut connection).unwrap();
        let transaction = connection.transaction().unwrap();
        create_schema(&transaction, &registry).unwrap();
        transaction
            .execute_batch("PRAGMA user_version = 1")
            .unwrap();
        transaction.commit().unwrap();
        migrate_v1_to_v2(&mut connection, &registry).unwrap();
        migrate_v2_to_v3(&mut connection).unwrap();

        let remote_key = k256::schnorr::SigningKey::from_slice(&[3; 32]).unwrap();
        let remote_xonly = remote_key.verifying_key().to_bytes();
        let remote_public_key = remote_xonly
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let remote_node_id = node_id_for_x_only_public_key(&remote_xonly);
        let request_id = [0x11; 16];
        let transport_key = [9u8; 32];
        let certificate = [0u8; 245];
        let code_hash = [7u8; 32];
        let certificate_digest = [8u8; 32];
        let certificate_id = [6u8; 16];
        let request_bytes = b"OMMA legacy v1 request".to_vec();
        let request_digest = digest(&request_bytes);
        let capabilities = "[\"remote-run\"]";
        let now = now_timestamp();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute(
                "INSERT INTO peers
                 (node_id, public_key, role, state, capabilities_json, added_at, updated_at, last_seen, source)
                 VALUES (?1, ?2, 'performer', 'pending', ?3, ?4, ?4, NULL, 'manual')",
                params![remote_node_id, remote_public_key, capabilities, now],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO remote_identities
                 (node_id, identity_key, state, first_seen, revoked_at)
                 VALUES (?1, ?2, 'authenticated_untrusted', 100, NULL)",
                params![remote_node_id, remote_xonly.as_slice()],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO transport_key_epochs
                 (node_id, key_epoch, public_key, certificate, state, added_at, retired_at)
                 VALUES (?1, 1, ?2, ?3, 'pending', 100, NULL)",
                params![
                    remote_node_id,
                    transport_key.as_slice(),
                    certificate.as_slice()
                ],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO manual_enrollment_requests
                 (request_id, request_bytes, request_digest, code_hash, node_id, identity_key,
                  transport_key, role, capabilities, request_created_at, request_expires_at,
                  certificate, certificate_digest, certificate_id, key_epoch, not_before, not_after,
                  state, source, staged_at, resolved_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 2, ?8, 100, 200, ?9, ?10, ?11, 1, 100, 200,
                         'pending', 'manual', 100, NULL)",
                params![
                    request_id.as_slice(),
                    &request_bytes,
                    request_digest.as_slice(),
                    code_hash.as_slice(),
                    remote_node_id,
                    remote_xonly.as_slice(),
                    transport_key.as_slice(),
                    capabilities.as_bytes(),
                    certificate.as_slice(),
                    certificate_digest.as_slice(),
                    certificate_id.as_slice(),
                ],
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO enrollment_replays (replay_kind, replay_id, expires_at, first_seen)
                 VALUES ('manual_request', ?1, 1000, 100)",
                [request_id.as_slice()],
            )
            .unwrap();
        transaction.commit().unwrap();
        (temp, context, registry, request_id, remote_node_id)
    }

    fn registration(identity: &NodeIdentity, scalar: u8) -> PeerRegistration {
        let key = k256::schnorr::SigningKey::from_slice(&[scalar; 32]).unwrap();
        let public_key = key.verifying_key().to_bytes();
        let public_key = public_key
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let node_id = node_id_for_x_only_public_key(
            &public_key
                .as_bytes()
                .chunks(2)
                .map(|chunk| u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap())
                .collect::<Vec<_>>(),
        );
        assert_ne!(node_id, identity.public_status().node_id);
        PeerRegistration {
            node_id,
            public_key,
            role: PeerRole::Performer,
            capabilities: vec!["remote-run".to_string()],
            source: PeerSource::Manual,
            actor: "operator".to_string(),
            reason: "test decision".to_string(),
        }
    }

    /// A second identity, to stand in for the peer being trusted.
    fn remote_identity(temp: &TempDir) -> (NodeContext, NodeIdentity) {
        let remote_context = NodeContext::resolve_for(
            NodePlatform::current(),
            NodePathOverrides::new(
                Some(temp.path().join("remote-state")),
                Some(temp.path().join("remote-node.toml")),
            ),
            true,
            None,
            None,
            None,
        )
        .unwrap();
        let identity = NodeIdentity::load_or_initialize(&remote_context).unwrap();
        (remote_context, identity)
    }

    const REMOTE_TRANSPORT_PUBLIC: [u8; 32] = [7; 32];

    fn remote_certificate(
        identity: &NodeIdentity,
        now: u64,
    ) -> crate::direct_transport::TransportCertificate {
        crate::direct_transport::TransportCertificate::issue(
            identity,
            REMOTE_TRANSPORT_PUBLIC,
            1,
            now,
            now + 600,
            [1; 16],
        )
        .unwrap()
    }

    /// Direction one: a node that signs baselines may not take Conductor
    /// authority over anyone, on any path that records a Performer.
    ///
    /// Every entry point is exercised rather than the shared helper, because
    /// the helper being right is worth nothing if one path forgets to call it.
    #[test]
    fn a_baseline_publisher_is_refused_conductor_authority_on_every_path() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        crate::baseline_publisher::BaselinePublisher::create(&context, &registry).unwrap();

        assert!(matches!(
            registry.register_pending(registration(&identity, 3)),
            Err(RegistryError::PublisherConductorConflict)
        ));
        assert!(matches!(
            registry.import_manual_peer(registration(&identity, 4)),
            Err(RegistryError::PublisherConductorConflict)
        ));

        let now = crate::enrollment::now_seconds();
        let (_remote_context, remote) = remote_identity(&temp);
        let certificate = remote_certificate(&remote, now);
        let offer = ManualEnrollmentRequest::create(
            &remote,
            REMOTE_TRANSPORT_PUBLIC,
            crate::enrollment::EnrollmentRole::Performer,
            vec!["remote-run".to_string()],
            now,
            600,
        )
        .unwrap();
        assert!(matches!(
            registry.stage_manual_enrollment(
                &offer.request,
                certificate.as_bytes(),
                "operator",
                "staging a performer",
                now,
            ),
            Err(RegistryError::PublisherConductorConflict)
        ));

        let bundle = SignedEnrollmentBundle::sign_with_material(
            &[5u8; 32],
            [2; crate::enrollment::REQUEST_ID_BYTES],
            [8; crate::enrollment::BUNDLE_AUTHORITY_ID_BYTES],
            "omakure".to_string(),
            identity.public_status().node_id.clone(),
            remote.public_status().node_id.clone(),
            crate::enrollment::parse_hex(&remote.public_status().public_key_hex, 32)
                .unwrap()
                .try_into()
                .unwrap(),
            REMOTE_TRANSPORT_PUBLIC,
            *certificate.as_bytes(),
            crate::enrollment::EnrollmentRole::Performer,
            vec!["remote-run".to_string()],
            now,
            now + 600,
        )
        .unwrap();
        assert!(matches!(
            registry.activate_signed_bundle(
                &bundle,
                "operator",
                "activating a performer",
                now,
                &[1; 32],
                &[2; 32],
            ),
            Err(RegistryError::PublisherConductorConflict)
        ));

        // `replace_peer` needs something to replace, and only a Conductor can
        // be there while a publisher key is held.
        let mut conductor = registration(&identity, 6);
        conductor.role = PeerRole::Conductor;
        registry.register_pending(conductor.clone()).unwrap();
        let mut successor = registration(&identity, 7);
        successor.role = PeerRole::Performer;
        assert!(matches!(
            registry.replace_peer(&conductor.node_id, successor),
            Err(RegistryError::PublisherConductorConflict)
        ));

        assert!(
            registry
                .peers()
                .unwrap()
                .iter()
                .all(|peer| peer.role == PeerRole::Conductor),
            "no refused path may have left a Performer behind"
        );
    }

    /// Direction two, and the other order: a node that already conducts someone
    /// cannot become a publisher — and the refused attempt leaves no key.
    #[test]
    fn a_conductor_is_refused_a_publisher_key_and_keeps_none() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        let performer = registry
            .register_pending(registration(&identity, 3))
            .unwrap();
        assert_eq!(performer.role, PeerRole::Performer);

        assert!(
            crate::baseline_publisher::BaselinePublisher::create(&context, &registry).is_err(),
            "a node that conducts a Performer must not become a publisher"
        );
        assert!(
            !context.publisher_key_path().exists(),
            "a refused create must not leave a key on disk for a later load to find"
        );
        assert!(
            context.validate_existing_state_contents().unwrap(),
            "and must not leave a stray temporary file in the state directory"
        );
    }

    /// The refusal is about the combination, not about publishers.
    ///
    /// Without this the whole rule could have been "a publisher records no
    /// peers", which would be a different and much blunter thing.
    #[test]
    fn a_publisher_may_still_record_the_conductor_it_answers_to() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        crate::baseline_publisher::BaselinePublisher::create(&context, &registry).unwrap();

        let mut conductor = registration(&identity, 3);
        conductor.role = PeerRole::Conductor;
        let peer = registry.import_manual_peer(conductor).unwrap();
        assert_eq!(peer.state, PeerState::Active);
    }

    /// Only revocation ends a Conductor relationship, so only revocation frees
    /// the node to publish.
    #[test]
    fn a_performer_blocks_publishing_until_it_is_revoked() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        let performer = registry
            .register_pending(registration(&identity, 3))
            .unwrap();

        registry
            .suspend_peer(&performer.node_id, "operator", "paused")
            .unwrap();
        assert!(
            crate::baseline_publisher::BaselinePublisher::create(&context, &registry).is_err(),
            "a suspended Performer can be reactivated, so it still counts"
        );

        registry
            .revoke_peer(&performer.node_id, "operator", "decommissioned")
            .unwrap();
        crate::baseline_publisher::BaselinePublisher::create(&context, &registry)
            .expect("a revoked Performer is terminal and no longer blocks publishing");
    }

    #[test]
    fn initializes_reopens_and_keeps_runs_path_separate() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        assert!(registry.path().ends_with("node.sqlite"));
        assert!(!registry.path().ends_with("runs.sqlite"));
        assert_eq!(
            NodeRegistry::open(&context, identity.public_status())
                .unwrap()
                .peers()
                .unwrap()
                .len(),
            0
        );
        let connection = Connection::open(context.database_path()).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "wal"
        );
    }

    #[test]
    fn full_transition_graph_and_revocation_precedence() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        let peer = registry
            .register_pending(registration(&identity, 3))
            .unwrap();
        assert_eq!(peer.state, PeerState::Pending);
        assert!(registry.activate_peer(&peer.node_id, "", "reason").is_err());
        assert_eq!(
            registry
                .activate_peer(&peer.node_id, "operator", "approve")
                .unwrap()
                .state,
            PeerState::Active
        );
        assert_eq!(
            registry
                .suspend_peer(&peer.node_id, "operator", "pause")
                .unwrap()
                .state,
            PeerState::Suspended
        );
        assert_eq!(
            registry
                .activate_peer(&peer.node_id, "operator", "resume")
                .unwrap()
                .state,
            PeerState::Active
        );
        assert_eq!(
            registry
                .revoke_peer(&peer.node_id, "operator", "retire")
                .unwrap()
                .state,
            PeerState::Revoked
        );
        assert!(matches!(
            registry.activate_peer(&peer.node_id, "operator", "resurrect"),
            Err(RegistryError::Revoked(_))
        ));
        assert_eq!(registry.revocations().unwrap().len(), 1);
        assert_eq!(registry.audit_events().unwrap().len(), 5);
    }

    #[test]
    fn rejects_self_duplicates_invalid_capabilities_and_bad_transitions() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        let mut self_registration = registration(&identity, 5);
        self_registration.node_id = identity.public_status().node_id.clone();
        self_registration.public_key = identity.public_status().public_key_hex.clone();
        assert!(matches!(
            registry.register_pending(self_registration),
            Err(RegistryError::SelfTrust)
        ));
        let peer = registry
            .register_pending(registration(&identity, 7))
            .unwrap();
        assert!(matches!(
            registry.register_pending(registration(&identity, 7)),
            Err(RegistryError::Duplicate(_))
        ));
        assert!(registry
            .suspend_peer(&peer.node_id, "operator", "bad")
            .is_ok());
        assert!(matches!(
            registry.suspend_peer(&peer.node_id, "operator", "again"),
            Err(RegistryError::InvalidTransition { .. })
        ));
        let mut unsupported = registration(&identity, 9);
        unsupported.capabilities = vec!["not-supported".to_string()];
        assert!(registry.register_pending(unsupported).is_err());
    }

    #[test]
    fn transaction_failure_does_not_leave_partial_peer_or_audit() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        let peer = registry
            .register_pending(registration(&identity, 11))
            .unwrap();
        assert!(registry
            .activate_peer(&peer.node_id, "operator", " ")
            .is_err());
        assert_eq!(
            registry.peer(&peer.node_id).unwrap().unwrap().state,
            PeerState::Pending
        );
        assert_eq!(registry.audit_events().unwrap().len(), 1);
    }

    #[test]
    fn concurrent_register_operations_are_serialized() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let registry = Arc::new(NodeRegistry::open(&context, identity.public_status()).unwrap());
        let registrations = (2..18)
            .map(|scalar| registration(&identity, scalar))
            .collect::<Vec<_>>();
        let threads = (2..18)
            .zip(registrations)
            .map(|(_, registration)| {
                let registry = Arc::clone(&registry);
                thread::spawn(move || registry.register_pending(registration))
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        assert_eq!(registry.peers().unwrap().len(), 16);
    }

    #[test]
    fn enrollment_cleanup_is_bounded_and_caps_are_frozen() {
        assert_eq!(MAX_ENROLLMENT_REPLAY_ROWS, 1_000_000);
        assert_eq!(MAX_ENROLLMENT_AUDIT_ROWS, 1_000_000);
        assert_eq!(MAX_ENROLLMENT_REQUEST_ROWS, 1_000_000);
        assert_eq!(MAX_BOOTSTRAP_PROOF_ROWS, 1_000_000);
        assert_eq!(MAX_ENROLLMENT_CLEANUP_ROWS, 10_000);

        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let _registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        let mut connection = Connection::open(context.database_path()).unwrap();
        configure_connection(&mut connection).unwrap();
        let transaction = connection.transaction().unwrap();
        for value in 0_u64..=10_000 {
            let mut replay_id = [0_u8; 16];
            replay_id[..8].copy_from_slice(&value.to_be_bytes());
            transaction
                .execute(
                    "INSERT INTO enrollment_replays
                     (replay_kind, replay_id, expires_at, first_seen)
                     VALUES ('bundle', ?1, 1, 1)",
                    [&replay_id[..]],
                )
                .unwrap();
        }
        cleanup_enrollment_replays(&transaction, 2).unwrap();
        transaction.commit().unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM enrollment_replays", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
    }

    #[test]
    fn v3_pending_legacy_enrollment_is_terminalized_with_redacted_evidence() {
        let (_temp, context, registry, request_id, remote_node_id) = seed_v3_pending_enrollment();
        let mut connection = Connection::open(context.database_path()).unwrap();
        configure_connection(&mut connection).unwrap();
        migrate_v3_to_v4(&mut connection).unwrap();
        migrate_v4_to_v5(&mut connection).unwrap();
        migrate_v5_to_v6(&mut connection).unwrap();
        migrate_v6_to_v7(&mut connection).unwrap();
        migrate_v7_to_v8(&mut connection).unwrap();
        validate_schema(&connection, &registry).unwrap();
        set_new_database_mode(&context.database_path()).unwrap();
        drop(connection);

        let identity = NodeIdentity::load_existing(&context).unwrap();
        let reopened = NodeRegistry::open(&context, identity.public_status()).unwrap();
        let peer = reopened.peer(&remote_node_id).unwrap().unwrap();
        assert_eq!(peer.state, PeerState::Suspended);

        let connection = Connection::open(context.database_path()).unwrap();
        let request_state: (String, Option<Vec<u8>>) = connection
            .query_row(
                "SELECT state, pairing_id FROM manual_enrollment_requests WHERE request_id = ?1",
                [request_id.as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(request_state.0, "rejected");
        assert!(request_state.1.is_none());
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM manual_enrollment_requests WHERE state = 'pending'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM enrollment_replays WHERE replay_kind = 'manual_request' AND replay_id = ?1",
                    [request_id.as_slice()],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        let audit: (String, String, String, Vec<u8>) = connection
            .query_row(
                "SELECT event_code, outcome, detail, request_digest
                 FROM enrollment_audits WHERE request_id = ?1",
                [request_id.as_slice()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(audit.0, "legacy_expired");
        assert_eq!(audit.1, "rejected");
        assert_eq!(
            audit.2,
            "legacy manual enrollment request expired during schema migration"
        );
        assert_eq!(audit.3.len(), 32);
        assert!(!audit.2.contains("OMMA"));
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM remote_identities WHERE state = 'active'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM trusted_peers WHERE state = 'active'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn v3_migration_rolls_back_when_legacy_audit_cannot_be_recorded() {
        let (_temp, context, registry, request_id, remote_node_id) = seed_v3_pending_enrollment();
        let connection = Connection::open(context.database_path()).unwrap();
        connection
            .execute_batch(
                "DROP TRIGGER enrollment_audits_no_update;
                 CREATE TRIGGER enrollment_audits_no_update
                 BEFORE INSERT ON enrollment_audits
                 BEGIN SELECT RAISE(ABORT, 'injected migration audit failure'); END;",
            )
            .unwrap();
        drop(connection);

        let mut connection = Connection::open(context.database_path()).unwrap();
        configure_connection(&mut connection).unwrap();
        assert!(migrate_v3_to_v4(&mut connection).is_err());
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert!(connection
            .query_row(
                "SELECT 1 FROM pragma_table_info('manual_enrollment_requests') WHERE name = 'pairing_id'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .unwrap()
            .is_none());
        assert_eq!(
            connection
                .query_row(
                    "SELECT state FROM manual_enrollment_requests WHERE request_id = ?1",
                    [request_id.as_slice()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "pending"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT state FROM peers WHERE node_id = ?1",
                    [&remote_node_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "pending"
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM enrollment_audits", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        assert!(registry.path().exists());
    }

    #[test]
    fn a_failed_health_plane_migration_keeps_the_node_serving_with_the_plane_disabled() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        fs::remove_file(context.database_path()).unwrap();
        let mut connection = Connection::open(context.database_path()).unwrap();
        configure_connection(&mut connection).unwrap();
        let transaction = connection.transaction().unwrap();
        create_schema(&transaction, &registry).unwrap();
        transaction
            .execute_batch("PRAGMA user_version = 1")
            .unwrap();
        transaction.commit().unwrap();
        migrate_v1_to_v2(&mut connection, &registry).unwrap();
        migrate_v2_to_v3(&mut connection).unwrap();
        migrate_v3_to_v4(&mut connection).unwrap();
        migrate_v4_to_v5(&mut connection).unwrap();
        migrate_v5_to_v6(&mut connection).unwrap();
        set_new_database_mode(&context.database_path()).unwrap();
        drop(connection);

        HEALTH_MIGRATION_FAULT.store(true, std::sync::atomic::Ordering::SeqCst);
        let identity = NodeIdentity::load_existing(&context).unwrap();
        let degraded = NodeRegistry::open(&context, identity.public_status()).unwrap();
        assert!(!degraded.health_plane_enabled().unwrap());

        let connection = Connection::open(context.database_path()).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 6, "the database stays at version 6");
        let audits: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM transport_audit
                 WHERE event_type = 'health_plane_migration' AND error_code = 1115",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(audits, 1, "the failure is audited once");
        let health_tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'health!_%' ESCAPE '!'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(health_tables, 0, "the failed migration left nothing behind");
        drop(connection);

        // Trust operations keep working while the Health Plane is disabled.
        let remote = k256::schnorr::SigningKey::from_slice(&[5; 32]).unwrap();
        let xonly = remote.verifying_key().to_bytes();
        let public_key: String = xonly.iter().map(|byte| format!("{byte:02x}")).collect();
        degraded
            .import_manual_peer(PeerRegistration {
                node_id: node_id_for_x_only_public_key(&xonly),
                public_key,
                role: PeerRole::Performer,
                capabilities: vec!["inventory-health".to_string()],
                source: PeerSource::Manual,
                actor: "operator".to_string(),
                reason: "degraded mode still serves trust".to_string(),
            })
            .unwrap();

        // A restart never retries the migration on its own.
        let reopened = NodeRegistry::open(&context, identity.public_status()).unwrap();
        assert!(!reopened.health_plane_enabled().unwrap());
        let connection = Connection::open(context.database_path()).unwrap();
        let audits: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM transport_audit
                 WHERE event_type = 'health_plane_migration' AND error_code = 1115",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(audits, 1, "retry is never automatic");
        drop(connection);

        // An explicit operator retry moves the registry forward.
        HEALTH_MIGRATION_FAULT.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(reopened.clear_health_plane_migration_block().unwrap());
        let repaired = NodeRegistry::open(&context, identity.public_status()).unwrap();
        assert!(repaired.health_plane_enabled().unwrap());
        let connection = Connection::open(context.database_path()).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn future_schema_corruption_and_metadata_downgrade_fail_closed() {
        let temp = TempDir::new().unwrap();
        let ctx = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&ctx).unwrap();
        let database = ctx.database_path();
        let connection = Connection::open(&database).unwrap();
        // One past whatever this build supports: the property is that a
        // database written by a newer Omakure is refused, and pinning the
        // literal here would quietly stop testing that at the next migration.
        connection
            .execute_batch(&format!("PRAGMA user_version = {}", SCHEMA_VERSION + 1))
            .unwrap();
        assert!(matches!(
            NodeRegistry::open(&ctx, identity.public_status()),
            Err(RegistryError::InvalidSchema(_))
        ));
        drop(connection);
        fs::remove_file(&database).unwrap();
        assert!(matches!(
            NodeRegistry::open(&ctx, identity.public_status()),
            Err(RegistryError::NotFound(_))
        ));
        assert!(ctx.identity_path().is_file());
        assert!(!ctx.database_path().exists());

        let temp = TempDir::new().unwrap();
        let context2 = context(&temp);
        let identity2 = NodeIdentity::load_or_initialize(&context2).unwrap();
        let connection = Connection::open(context2.database_path()).unwrap();
        connection
            .execute(
                "UPDATE metadata SET value = '0' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        drop(connection);
        assert!(matches!(
            NodeRegistry::open(&context2, identity2.public_status()),
            Err(RegistryError::InvalidSchema(_))
        ));
    }
}

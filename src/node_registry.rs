//! The node-owned trust and delivery persistence boundary.
//!
//! This module intentionally owns `node.sqlite` exclusively.  It does not
//! import or call the run-history repository, and it contains no transport or
//! enrollment behavior.  Trust changes are explicit, transactional operations
//! with an actor and reason recorded in the append-only audit log.

use crate::node::NodeContext;
use crate::node_identity::{node_id_for_x_only_public_key, NodeIdentityStatus};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

pub const SCHEMA_VERSION: i64 = 1;
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_ACTOR_BYTES: usize = 256;
const MAX_REASON_BYTES: usize = 1024;
const MAX_CAPABILITIES: usize = 32;
const MAX_CAPABILITY_BYTES: usize = 64;
const MAX_CAPABILITIES_JSON_BYTES: usize = 4096;
const NODE_ID_BYTES: usize = 69;
const PUBLIC_KEY_BYTES: usize = 64;

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
    local_node_id: String,
    local_public_key: String,
}

impl NodeRegistry {
    /// Open and validate the node-owned database for the supplied public identity.
    pub fn open(
        context: &NodeContext,
        identity: &NodeIdentityStatus,
    ) -> Result<Self, RegistryError> {
        context.ensure_state_directory()?;
        let path = context.database_path();
        let (node_id, public_key) = validate_identity(identity)?;
        let registry = Self {
            path,
            local_node_id: node_id,
            local_public_key: public_key,
        };
        registry.with_connection(|connection| initialize_database(connection, &registry))?;
        Ok(registry)
    }

    pub fn path(&self) -> &Path {
        &self.path
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

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, RegistryError>,
    ) -> Result<T, RegistryError> {
        let mut connection = Connection::open(&self.path)?;
        configure_connection(&mut connection)?;
        operation(&mut connection)
    }
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
    validate_schema(connection, registry)
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

fn validate_schema(connection: &Connection, registry: &NodeRegistry) -> Result<(), RegistryError> {
    validate_objects(connection)?;
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
    let metadata: Vec<(String, String)> = connection
        .prepare("SELECT key, value FROM metadata ORDER BY key")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    if metadata.len() != 3 {
        return Err(RegistryError::InvalidSchema(
            "metadata contains unexpected keys".to_string(),
        ));
    }
    let expected = [
        ("schema_version", "1"),
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

fn validate_objects(connection: &Connection) -> Result<(), RegistryError> {
    let actual: Vec<(String, String)> = connection
        .prepare(
            "SELECT type, name FROM sqlite_master
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let expected = vec![
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
    if actual != expected {
        return Err(RegistryError::InvalidSchema(
            "database contains unexpected or missing schema objects".to_string(),
        ));
    }
    Ok(())
}

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
        .unwrap()
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
    fn future_schema_corruption_and_metadata_downgrade_fail_closed() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let database = context.database_path();
        let connection = Connection::open(&database).unwrap();
        connection.execute_batch("PRAGMA user_version = 2").unwrap();
        assert!(matches!(
            NodeRegistry::open(&context, identity.public_status()),
            Err(RegistryError::InvalidSchema(_))
        ));
        drop(connection);
        fs::remove_file(&database).unwrap();
        let _ = NodeRegistry::open(&context, identity.public_status()).unwrap();
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "UPDATE metadata SET value = '0' WHERE key = 'schema_version'",
                [],
            )
            .unwrap();
        drop(connection);
        assert!(matches!(
            NodeRegistry::open(&context, identity.public_status()),
            Err(RegistryError::InvalidSchema(_))
        ));
    }
}

use super::audit::audit_from_row;
use super::error::RegistryError;
use super::fields::{validate_bounded_text, validate_node_id, validate_timestamp};
use super::peers::{peer_from_row, revocation_from_row};
use super::{NodeRegistry, HEALTH_PLANE_ENABLED, MAX_REASON_BYTES, SCHEMA_VERSION};
use rusqlite::Connection;

pub(super) fn validate_schema(
    connection: &Connection,
    registry: &NodeRegistry,
) -> Result<(), RegistryError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != SCHEMA_VERSION {
        return Err(RegistryError::InvalidSchema(format!(
            "database schema marker is {version}, expected {SCHEMA_VERSION}"
        )));
    }
    validate_objects(connection)?;
    validate_core_columns(connection)?;
    validate_health_plane_columns(connection)?;
    validate_metadata(connection, registry)?;
    validate_all_rows(connection)
}

const CORE_TABLE_COLUMNS: &[(&str, &[&str])] = &[
    ("metadata", &["key", "value"]),
    (
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
    ),
    (
        "revocations",
        &[
            "id",
            "node_id",
            "public_key",
            "revoked_at",
            "reason",
            "replacement_node_id",
        ],
    ),
    (
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
    ),
    ("replay_keys", &["key", "first_seen", "expires_at"]),
    (
        "inbox",
        &[
            "cue_id",
            "state",
            "received_at",
            "updated_at",
            "expires_at",
            "outcome_hash",
        ],
    ),
    (
        "remote_identities",
        &[
            "node_id",
            "identity_key",
            "state",
            "first_seen",
            "revoked_at",
        ],
    ),
    (
        "trusted_peers",
        &[
            "node_id",
            "role",
            "capabilities",
            "state",
            "added_at",
            "updated_at",
        ],
    ),
    (
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
    ),
    (
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
    ),
    (
        "enrollment_replays",
        &["replay_kind", "replay_id", "expires_at", "first_seen"],
    ),
    (
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
    ),
    ("cue_rate_limits", &["node_id", "window_start", "count"]),
    (
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
    ),
    (
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
    ),
    (
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
    ),
];

fn validate_core_columns(connection: &Connection) -> Result<(), RegistryError> {
    for (table, columns) in CORE_TABLE_COLUMNS {
        validate_columns(connection, table, columns)?;
    }
    Ok(())
}

fn validate_metadata(
    connection: &Connection,
    registry: &NodeRegistry,
) -> Result<(), RegistryError> {
    let metadata: Vec<(String, String)> = connection
        .prepare("SELECT key, value FROM metadata ORDER BY key")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    if metadata.len() != 4 {
        return Err(RegistryError::InvalidSchema(
            "metadata contains unexpected keys".to_string(),
        ));
    }
    let schema_version = SCHEMA_VERSION.to_string();
    let expected = [
        ("schema_version", schema_version.as_str()),
        ("health_plane", HEALTH_PLANE_ENABLED),
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
    Ok(())
}

fn validate_objects(connection: &Connection) -> Result<(), RegistryError> {
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
    expected.extend(
        HEALTH_PLANE_OBJECTS
            .iter()
            .map(|(object_type, name)| ((*object_type).to_string(), (*name).to_string())),
    );
    expected.sort();
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

/// Every Health Plane schema object.
pub(super) const HEALTH_PLANE_OBJECTS: [(&str, &str); 15] = [
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

pub(super) fn sqlite_validation_error(error: RegistryError) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

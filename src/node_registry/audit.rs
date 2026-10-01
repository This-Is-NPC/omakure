use super::error::RegistryError;
use super::fields::{
    validate_actor_reason, validate_bounded_text, validate_node_id, validate_timestamp,
};
use super::types::{AuditEvent, PeerState};
use super::validate::sqlite_validation_error;
use super::{
    MAX_ENROLLMENT_AUDIT_ROWS, MAX_LIFECYCLE_SCAN_ROWS, MAX_TRANSPORT_AUDIT_ROWS, NodeRegistry,
};
use crate::util::hex;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

pub(crate) struct TransportAudit<'a> {
    pub(crate) event_type: &'a str,
    pub(crate) node_id: &'a str,
    pub(crate) session_id: Option<&'a [u8; 32]>,
    pub(crate) direction: Option<u8>,
    pub(crate) byte_count: usize,
    pub(crate) outcome: &'a str,
    pub(crate) error_code: Option<u16>,
    pub(crate) cue: Option<CueAudit<'a>>,
}

pub(crate) struct CueAudit<'a> {
    pub(crate) id: Option<&'a str>,
    pub(crate) script: Option<&'a str>,
    pub(crate) reason: Option<&'a str>,
}

fn validate_transport_audit(input: &TransportAudit<'_>) -> Result<(), RegistryError> {
    if let Some(cue) = &input.cue {
        if cue.id.is_some() != cue.script.is_some() || cue.id.is_some() != cue.reason.is_some() {
            return Err(RegistryError::InvalidInput(
                "Cue audit correlation must be complete".to_string(),
            ));
        }
        if let Some(cue_id) = cue.id {
            if cue_id.len() != 32 || !hex::is_lower(cue_id) {
                return Err(RegistryError::InvalidInput(
                    "Cue audit id must be 32 lowercase hex characters".to_string(),
                ));
            }
            validate_bounded_text("Cue audit script", cue.script.unwrap_or_default(), 64)?;
            validate_bounded_text("Cue audit reason", cue.reason.unwrap_or_default(), 128)?;
        }
    }
    validate_bounded_text("transport event type", input.event_type, 64)?;
    validate_node_id(input.node_id)?;
    validate_bounded_text("transport outcome", input.outcome, 32)?;
    if !matches!(input.direction, None | Some(0) | Some(1))
        || input
            .error_code
            .is_some_and(|code| !(1000..=1999).contains(&code))
    {
        return Err(RegistryError::InvalidInput(
            "transport audit metadata is invalid".to_string(),
        ));
    }
    Ok(())
}

impl NodeRegistry {
    pub fn audit_events(&self) -> Result<Vec<AuditEvent>, RegistryError> {
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
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

    /// Append a redacted transport outcome with optional bounded Cue correlation.
    pub(crate) fn record_transport_audit(
        &self,
        input: TransportAudit<'_>,
    ) -> Result<(), RegistryError> {
        validate_transport_audit(&input)?;
        let now = chrono::Utc::now().timestamp();
        self.with_mutating_connection(|connection| {
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
                    input.event_type,
                    input.node_id,
                    input.session_id.map(|value| value.as_slice()),
                    input.direction,
                    i64::try_from(input.byte_count).map_err(|_| RegistryError::InvalidInput(
                        "transport byte count is too large".to_string()
                    ))?,
                    input.outcome,
                    input.error_code,
                    input.cue.as_ref().and_then(|cue| cue.id),
                    input.cue.as_ref().and_then(|cue| cue.script),
                    input.cue.as_ref().and_then(|cue| cue.reason),
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
        self.with_mutating_connection(|connection| {
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
        self.with_mutating_connection(|connection| {
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
}

pub(super) fn record_enrollment_audit_tx(
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

pub(super) struct AuditInput<'a> {
    pub(super) event_type: &'a str,
    pub(super) node_id: &'a str,
    pub(super) from_state: Option<PeerState>,
    pub(super) to_state: Option<PeerState>,
    pub(super) actor: &'a str,
    pub(super) reason: &'a str,
    pub(super) occurred_at: &'a str,
}

pub(super) fn record_audit(
    transaction: &Transaction<'_>,
    input: AuditInput<'_>,
) -> Result<(), RegistryError> {
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

pub(super) fn audit_from_row(row: &Row<'_>) -> rusqlite::Result<AuditEvent> {
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

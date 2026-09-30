use super::super::fields::validate_node_id;
use super::super::{NodeRegistry, RegistryError};
use super::types::HealthAuditEvent;
use crate::health_plane::bounds::MAX_AUDIT_ROWS;
use rusqlite::{params, Transaction, TransactionBehavior};

impl NodeRegistry {
    /// Append one redacted Health Plane audit row for an outcome decided before
    /// any storage was consulted.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_health_audit(
        &self,
        event_code: &str,
        node_id: &str,
        message_kind: &str,
        byte_count: i64,
        outcome: &str,
        error_code: Option<u16>,
        now: i64,
    ) -> Result<(), RegistryError> {
        validate_node_id(node_id)?;
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            record_health_audit_tx(
                &transaction,
                event_code,
                node_id,
                message_kind,
                byte_count,
                outcome,
                error_code,
                now,
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    /// The redacted Health Plane audit trail, newest first.
    pub fn health_audit_events(
        &self,
        limit: usize,
    ) -> Result<Vec<HealthAuditEvent>, RegistryError> {
        let limit = limit.min(MAX_AUDIT_ROWS as usize);
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, event_code, node_id, message_kind, byte_count, outcome,
                        error_code, occurred_at
                 FROM health_audit ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = statement
                .query_map(params![limit as i64], |row| {
                    Ok(HealthAuditEvent {
                        id: row.get(0)?,
                        event_code: row.get(1)?,
                        node_id: row.get(2)?,
                        message_kind: row.get(3)?,
                        byte_count: row.get(4)?,
                        outcome: row.get(5)?,
                        error_code: row.get::<_, Option<i64>>(6)?.map(|code| code as u16),
                        occurred_at: row.get(7)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }
}

/// Append one redacted Health Plane audit row. The row records only the stable
/// code, the peer node ID, the message kind, and byte counts; it never records
/// payload bytes, field values, signatures, or key material.
#[allow(clippy::too_many_arguments)]
pub(super) fn record_health_audit_tx(
    transaction: &Transaction<'_>,
    event_code: &str,
    node_id: &str,
    message_kind: &str,
    byte_count: i64,
    outcome: &str,
    error_code: Option<u16>,
    now: i64,
) -> Result<(), RegistryError> {
    if event_code.len() > 64 || message_kind.len() > 64 || outcome.len() > 32 {
        return Err(RegistryError::InvalidInput(
            "health audit metadata is invalid".to_string(),
        ));
    }
    if error_code.is_some_and(|code| !(1000..=1999).contains(&code)) {
        return Err(RegistryError::InvalidInput(
            "health audit error code is out of range".to_string(),
        ));
    }
    let rows: i64 =
        transaction.query_row("SELECT COUNT(*) FROM health_audit", [], |row| row.get(0))?;
    if rows >= MAX_AUDIT_ROWS {
        transaction.execute(
            "DELETE FROM health_audit WHERE id IN (
               SELECT id FROM health_audit ORDER BY id LIMIT ?1
             )",
            params![rows - MAX_AUDIT_ROWS + 1],
        )?;
    }
    transaction.execute(
        "INSERT INTO health_audit
         (event_code, node_id, message_kind, byte_count, outcome, error_code, occurred_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            event_code,
            node_id,
            message_kind,
            byte_count,
            outcome,
            error_code,
            now
        ],
    )?;
    Ok(())
}

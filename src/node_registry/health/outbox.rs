use super::super::fields::validate_node_id;
use super::super::{NodeRegistry, RegistryError};
use super::audit::record_health_audit_tx;
use super::rows::{decode_opaque_id, signal_from_row};
use super::types::HealthOutboxEntry;
use crate::domain::health_plane::bounds::{SIGNAL_OUTBOX_CAPACITY, SIGNAL_RETENTION_SECONDS};
use crate::domain::health_plane::model::{
    HealthCode, HealthKind, RunFact, SignalKind, SignalRecord,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

impl NodeRegistry {
    /// Append one Signal to the bounded Performer outbox.
    ///
    /// At capacity the oldest undelivered Signal is dropped, the local
    /// `signals_dropped` counter is incremented, and the drop is audited once.
    #[allow(clippy::too_many_arguments)]
    pub fn health_enqueue_signal(
        &self,
        target_node_id: &str,
        signal_id: &str,
        kind: SignalKind,
        occurred_at: i64,
        subject: Option<&str>,
        run: Option<&RunFact>,
        message_bytes: i64,
        now: i64,
    ) -> Result<HealthOutboxEntry, RegistryError> {
        validate_node_id(target_node_id)?;
        let raw_signal_id = decode_opaque_id(signal_id)?;
        if message_bytes < 1 || message_bytes > HealthKind::Signal.max_stored_bytes().unwrap_or(0) {
            return Err(RegistryError::InvalidInput(
                "health signal exceeds the frozen stored byte cap".to_string(),
            ));
        }
        if (subject.is_some() == run.is_some())
            || (matches!(kind, SignalKind::RunCompleted) != run.is_some())
        {
            return Err(RegistryError::InvalidInput(
                "health signal body does not match its kind".to_string(),
            ));
        }
        let run_json = run
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| RegistryError::InvalidInput("health run is not encodable".to_string()))?;
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let existing: Option<i64> = transaction
                .query_row(
                    "SELECT sequence FROM health_outbox WHERE signal_id = ?1",
                    params![raw_signal_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(sequence) = existing {
                transaction.commit()?;
                return Err(RegistryError::Duplicate(format!(
                    "health signal is already queued at sequence {sequence}"
                )));
            }
            let mut dropped = 0_i64;
            loop {
                let queued: i64 =
                    transaction
                        .query_row("SELECT COUNT(*) FROM health_outbox", [], |row| row.get(0))?;
                if queued < SIGNAL_OUTBOX_CAPACITY {
                    break;
                }
                let removed = transaction.execute(
                    "DELETE FROM health_outbox WHERE signal_id = (
                       SELECT signal_id FROM health_outbox
                       ORDER BY sequence LIMIT 1
                     )",
                    [],
                )?;
                if removed == 0 {
                    break;
                }
                dropped += 1;
            }
            if dropped > 0 {
                transaction.execute(
                    "UPDATE health_local SET value = value + ?1 WHERE key = 'signals_dropped'",
                    params![dropped],
                )?;
                record_health_audit_tx(
                    &transaction,
                    "outbox_overflow",
                    &self.local_node_id,
                    HealthKind::Signal.wire(),
                    dropped,
                    "dropped",
                    Some(HealthCode::QueueFull.code()),
                    now,
                )?;
            }
            let sequence: i64 = transaction.query_row(
                "SELECT value FROM health_local WHERE key = 'signal_sequence'",
                [],
                |row| row.get(0),
            )?;
            let sequence = sequence + 1;
            transaction.execute(
                "UPDATE health_local SET value = ?1 WHERE key = 'signal_sequence'",
                params![sequence],
            )?;
            let expires_at = now.saturating_add(SIGNAL_RETENTION_SECONDS);
            transaction.execute(
                "INSERT INTO health_outbox
                 (signal_id, target_node_id, sequence, kind, occurred_at, subject, run,
                  message_bytes, attempts, last_message_id, enqueued_at, updated_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, NULL, ?9, ?9, ?10)",
                params![
                    raw_signal_id,
                    target_node_id,
                    sequence,
                    kind.wire(),
                    occurred_at,
                    subject,
                    run_json,
                    message_bytes,
                    now,
                    expires_at,
                ],
            )?;
            transaction.commit()?;
            Ok(HealthOutboxEntry {
                signal_id: signal_id.to_string(),
                target_node_id: target_node_id.to_string(),
                sequence: sequence as u64,
                signal: SignalRecord {
                    kind,
                    occurred_at,
                    run: run.cloned(),
                    sequence: sequence as u64,
                    signal_id: signal_id.to_string(),
                    subject: subject.map(str::to_string),
                },
                attempts: 0,
                enqueued_at: now,
                expires_at,
            })
        })
    }

    /// Read the bounded Performer outbox in send order.
    pub fn health_outbox(&self, limit: usize) -> Result<Vec<HealthOutboxEntry>, RegistryError> {
        let limit = limit.min(SIGNAL_OUTBOX_CAPACITY as usize);
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT signal_id, target_node_id, sequence, kind, occurred_at, subject, run,
                        attempts, enqueued_at, expires_at
                 FROM health_outbox ORDER BY sequence LIMIT ?1",
            )?;
            let rows = statement
                .query_map(params![limit as i64], |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, i64>(9)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(|row| {
                    let signal = signal_from_row(&(row.0, row.2, row.3, row.4, row.5, row.6))?;
                    Ok(HealthOutboxEntry {
                        signal_id: signal.signal_id.clone(),
                        target_node_id: row.1,
                        sequence: row.2 as u64,
                        signal,
                        attempts: row.7,
                        enqueued_at: row.8,
                        expires_at: row.9,
                    })
                })
                .collect()
        })
    }

    /// Bind one outbox Signal to the `message_id` of a send attempt so a later
    /// acknowledgement can retire exactly that entry.
    pub fn health_mark_signal_sent(
        &self,
        signal_id: &str,
        message_id: &str,
        now: i64,
    ) -> Result<bool, RegistryError> {
        let raw_signal_id = decode_opaque_id(signal_id)?;
        let raw_message_id = decode_opaque_id(message_id)?;
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let updated = transaction.execute(
                "UPDATE health_outbox
                 SET attempts = attempts + 1, last_message_id = ?2, updated_at = ?3
                 WHERE signal_id = ?1",
                params![raw_signal_id, raw_message_id, now],
            )?;
            transaction.commit()?;
            Ok(updated > 0)
        })
    }

    /// Give every Signal queued for one peer a fresh delivery budget.
    ///
    /// The frozen contract says a Signal that spent its retries is *retained in
    /// the outbox within its 64-entry and 7-day bounds and resent on the next
    /// session*. `attempts` is therefore a per-session counter, and the only
    /// event that clears it is a newly established session to that peer. This
    /// resets exactly that counter for exactly that target: it never deletes a
    /// Signal, never renumbers a sequence, never touches `signal_id`, and never
    /// widens the 3-attempt bound the column already enforces.
    ///
    /// Returns how many queued Signals were re-armed.
    pub fn health_reset_outbox_attempts(
        &self,
        target_node_id: &str,
        now: i64,
    ) -> Result<u64, RegistryError> {
        validate_node_id(target_node_id)?;
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let reset = transaction.execute(
                "UPDATE health_outbox
                 SET attempts = 0, last_message_id = NULL, updated_at = ?2
                 WHERE target_node_id = ?1 AND attempts > 0",
                params![target_node_id, now],
            )?;
            transaction.commit()?;
            Ok(reset as u64)
        })
    }

    /// Count of Signals dropped by outbox overflow since this node was created.
    pub fn health_signals_dropped(&self) -> Result<i64, RegistryError> {
        self.with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT value FROM health_local WHERE key = 'signals_dropped'",
                [],
                |row| row.get(0),
            )?)
        })
    }
}

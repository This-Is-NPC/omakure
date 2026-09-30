use super::super::{NodeRegistry, RegistryError};
use super::audit::record_health_audit_tx;
use super::rows::active_trust_predicate;
use super::types::HealthPruneReport;
use crate::health_plane::bounds::{
    AUDIT_RETENTION_SECONDS, AUDIT_ROW_BYTES, MAX_AUDIT_ROWS, MAX_REPLAY_ROWS, REPLAY_ROW_BYTES,
    REPLAY_SECURITY_FLOOR_SECONDS, SIGNAL_INBOX_CAPACITY, VERSION_INCOMPATIBLE_EXPIRY_SECONDS,
};
use crate::health_plane::model::HealthCode;
use rusqlite::{params, Transaction, TransactionBehavior};

impl NodeRegistry {
    /// Delete all Health Plane state for peers that are no longer actively
    /// trusted. Health Plane state is derived and disposable; trust rows,
    /// revocations, and identities are never touched.
    pub fn health_purge_revoked(&self, now: i64) -> Result<Vec<String>, RegistryError> {
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let stale: Vec<String> = {
                let statement = format!(
                    "SELECT h.node_id FROM health_peers h
                     WHERE NOT {}
                     ORDER BY h.node_id",
                    active_trust_predicate("h.node_id")
                );
                let mut statement = transaction.prepare(&statement)?;
                let rows = statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            };
            for node_id in &stale {
                delete_peer_health(&transaction, node_id)?;
                record_health_audit_tx(
                    &transaction,
                    "revocation_cleanup",
                    node_id,
                    "none",
                    0,
                    "purged",
                    Some(HealthCode::Revoked.code()),
                    now,
                )?;
            }
            transaction.commit()?;
            Ok(stale)
        })
    }

    /// Enforce every retention and capacity bound in one pass.
    pub fn health_prune(&self, now: i64) -> Result<HealthPruneReport, RegistryError> {
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let report = prune_tx(&transaction, now)?;
            transaction.commit()?;
            Ok(report)
        })
    }

    /// The bytes the Health Plane currently accounts for, against the frozen
    /// ceiling of 25,464,832.
    pub fn health_storage_bytes(&self) -> Result<i64, RegistryError> {
        self.with_connection(|connection| {
            let payload: i64 = connection.query_row(
                "SELECT
                   (SELECT COALESCE(SUM(message_bytes), 0) FROM health_profiles)
                 + (SELECT COALESCE(SUM(message_bytes), 0) FROM health_pulses)
                 + (SELECT COALESCE(SUM(message_bytes), 0) FROM health_signals)",
                [],
                |row| row.get(0),
            )?;
            let replay: i64 =
                connection.query_row("SELECT COUNT(*) FROM health_replay_keys", [], |row| {
                    row.get(0)
                })?;
            let audit: i64 =
                connection.query_row("SELECT COUNT(*) FROM health_audit", [], |row| row.get(0))?;
            Ok(payload + replay * REPLAY_ROW_BYTES + audit * AUDIT_ROW_BYTES)
        })
    }
}

fn prune_tx(transaction: &Transaction<'_>, now: i64) -> Result<HealthPruneReport, RegistryError> {
    let mut report = HealthPruneReport {
        expired_held_signals: transaction.execute(
            "DELETE FROM health_signals WHERE state = 'held' AND expires_at <= ?1",
            params![now],
        )? as u64,
        expired_signals: transaction.execute(
            "DELETE FROM health_signals WHERE state = 'applied' AND expires_at <= ?1",
            params![now],
        )? as u64,
        ..HealthPruneReport::default()
    };
    let peers: Vec<String> = {
        let mut statement = transaction
            .prepare("SELECT node_id FROM health_signals GROUP BY node_id HAVING COUNT(*) > ?1")?;
        let rows = statement
            .query_map(params![SIGNAL_INBOX_CAPACITY], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    for node_id in peers {
        report.evicted_signals += transaction.execute(
            "DELETE FROM health_signals WHERE node_id = ?1 AND signal_id IN (
               SELECT signal_id FROM health_signals WHERE node_id = ?1
               ORDER BY occurred_at DESC, signal_id DESC LIMIT -1 OFFSET ?2
             )",
            params![node_id, SIGNAL_INBOX_CAPACITY],
        )? as u64;
    }
    report.expired_replay_keys = transaction.execute(
        "DELETE FROM health_replay_keys WHERE expires_at <= ?1 AND first_seen <= ?2",
        params![now, now.saturating_sub(REPLAY_SECURITY_FLOOR_SECONDS)],
    )? as u64;
    let replay_rows: i64 =
        transaction.query_row("SELECT COUNT(*) FROM health_replay_keys", [], |row| {
            row.get(0)
        })?;
    if replay_rows > MAX_REPLAY_ROWS {
        report.evicted_replay_keys = transaction.execute(
            "DELETE FROM health_replay_keys WHERE message_id IN (
               SELECT message_id FROM health_replay_keys
               WHERE first_seen <= ?1 ORDER BY expires_at, message_id LIMIT ?2
             )",
            params![
                now.saturating_sub(REPLAY_SECURITY_FLOOR_SECONDS),
                replay_rows - MAX_REPLAY_ROWS
            ],
        )? as u64;
    }
    report.expired_outbox_signals = transaction.execute(
        "DELETE FROM health_outbox WHERE expires_at <= ?1",
        params![now],
    )? as u64;
    report.cleared_version_incompatible = transaction.execute(
        "UPDATE health_peers SET version_incompatible_at = NULL
         WHERE version_incompatible_at IS NOT NULL AND version_incompatible_at <= ?1",
        params![now.saturating_sub(VERSION_INCOMPATIBLE_EXPIRY_SECONDS)],
    )? as u64;
    report.pruned_audit_rows = prune_audit_tx(transaction, now)?;
    Ok(report)
}

fn prune_audit_tx(transaction: &Transaction<'_>, now: i64) -> Result<u64, RegistryError> {
    let mut pruned = transaction.execute(
        "DELETE FROM health_audit WHERE occurred_at <= ?1",
        params![now.saturating_sub(AUDIT_RETENTION_SECONDS)],
    )? as u64;
    let rows: i64 =
        transaction.query_row("SELECT COUNT(*) FROM health_audit", [], |row| row.get(0))?;
    if rows > MAX_AUDIT_ROWS {
        pruned += transaction.execute(
            "DELETE FROM health_audit WHERE id IN (
               SELECT id FROM health_audit ORDER BY id LIMIT ?1
             )",
            params![rows - MAX_AUDIT_ROWS],
        )? as u64;
    }
    Ok(pruned)
}

fn delete_peer_health(transaction: &Transaction<'_>, node_id: &str) -> Result<(), RegistryError> {
    transaction.execute(
        "DELETE FROM health_signals WHERE node_id = ?1",
        params![node_id],
    )?;
    transaction.execute(
        "DELETE FROM health_profiles WHERE node_id = ?1",
        params![node_id],
    )?;
    transaction.execute(
        "DELETE FROM health_pulses WHERE node_id = ?1",
        params![node_id],
    )?;
    transaction.execute(
        "DELETE FROM health_peers WHERE node_id = ?1",
        params![node_id],
    )?;
    Ok(())
}

use super::super::fields::decode_hex;
use super::super::{NodeRegistry, PeerRole, PeerState, RegistryError};
use super::audit::record_health_audit_tx;
use super::types::{HealthAuthorization, HealthPeerState};
use crate::health_plane::model::{
    HealthCode, HealthKind, ProfileSnapshot, PulseSnapshot, RunFact, RunnerFact, RuntimeFact,
    SignalKind, SignalRecord,
};
use crate::util::hex;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

pub(super) fn health_authorization_from_row(
    node_id: &str,
    identity_state: &str,
    role: Option<i64>,
    capabilities: Option<Vec<u8>>,
    trust_state: Option<&str>,
) -> Result<HealthAuthorization, RegistryError> {
    let state = if identity_state == "revoked" || trust_state == Some("revoked") {
        PeerState::Revoked
    } else if identity_state == "active" && trust_state == Some("active") {
        PeerState::Active
    } else {
        PeerState::Pending
    };
    let role = match role {
        None => PeerRole::Performer,
        Some(code) => PeerRole::from_code(code).ok_or_else(|| {
            RegistryError::InvalidSchema(format!("unknown trusted peer role {code}"))
        })?,
    };
    let capabilities = match capabilities {
        Some(raw) => {
            let text = String::from_utf8(raw).map_err(|_| {
                RegistryError::InvalidSchema("trusted peer capabilities are not UTF-8".to_string())
            })?;
            serde_json::from_str::<Vec<String>>(&text).map_err(|_| {
                RegistryError::InvalidSchema("trusted peer capabilities are not JSON".to_string())
            })?
        }
        None => Vec::new(),
    };
    Ok(HealthAuthorization {
        node_id: node_id.to_string(),
        state,
        role,
        capabilities,
    })
}

/// The read-only authorization projection, on a connection the caller owns.
pub(super) fn authorization_in(
    connection: &Connection,
    node_id: &str,
) -> Result<Option<HealthAuthorization>, RegistryError> {
    connection
        .query_row(
            "SELECT r.state, p.role, p.capabilities, p.state
             FROM remote_identities r
             LEFT JOIN trusted_peers p ON p.node_id = r.node_id
             WHERE r.node_id = ?1",
            params![node_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?
        .map(|(identity_state, role, capabilities, trust_state)| {
            health_authorization_from_row(
                node_id,
                &identity_state,
                role,
                capabilities,
                trust_state.as_deref(),
            )
        })
        .transpose()
}

/// The one predicate that decides whether a peer is still actively trusted.
///
/// The Signal feed must hide exactly what the revocation cleanup deletes, so
/// both statements build their clause here and cannot drift apart: a peer
/// whose trust ends stops appearing in the operator's feed on the next read
/// rather than on the next cleanup tick.
pub(super) fn active_trust_predicate(node_column: &str) -> String {
    format!(
        "EXISTS (
           SELECT 1 FROM trusted_peers t
           JOIN remote_identities r ON r.node_id = t.node_id
           WHERE t.node_id = {node_column}
             AND t.state = 'active' AND r.state = 'active'
         )"
    )
}

pub(super) fn load_peer_state(
    transaction: &Transaction<'_>,
    node_id: &str,
) -> Result<Option<HealthPeerState>, RegistryError> {
    transaction
        .query_row(
            "SELECT p.node_id, p.role, p.cursor, p.last_profile_revision,
                    p.last_pulse_sequence, p.last_pulse_at, p.version_incompatible_at,
                    p.first_seen, p.updated_at,
                    (SELECT COUNT(*) FROM health_signals s
                      WHERE s.node_id = p.node_id AND s.state = 'applied'),
                    (SELECT COUNT(*) FROM health_signals s
                      WHERE s.node_id = p.node_id AND s.state = 'held')
             FROM health_peers p WHERE p.node_id = ?1",
            params![node_id],
            health_peer_from_row,
        )
        .optional()?
        .transpose()
}

type HealthPeerRow = Result<HealthPeerState, RegistryError>;

pub(super) fn health_peer_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HealthPeerRow> {
    let node_id: String = row.get(0)?;
    let role: i64 = row.get(1)?;
    let cursor: i64 = row.get(2)?;
    let last_profile_revision: i64 = row.get(3)?;
    let last_pulse_sequence: i64 = row.get(4)?;
    let last_pulse_at: Option<i64> = row.get(5)?;
    let version_incompatible_at: Option<i64> = row.get(6)?;
    let first_seen: i64 = row.get(7)?;
    let updated_at: i64 = row.get(8)?;
    let stored_signals: i64 = row.get(9)?;
    let held_signals: i64 = row.get(10)?;
    let Some(role) = PeerRole::from_code(role) else {
        return Ok(Err(RegistryError::Corrupt(format!(
            "health peer has unknown role {role}"
        ))));
    };
    Ok(Ok(HealthPeerState {
        node_id,
        role,
        cursor: cursor.max(0) as u64,
        last_profile_revision: last_profile_revision.max(0) as u64,
        last_pulse_sequence: last_pulse_sequence.max(0) as u64,
        last_pulse_at,
        stored_signals: stored_signals.max(0) as u64,
        held_signals: held_signals.max(0) as u64,
        version_incompatible: version_incompatible_at.is_some(),
        first_seen,
        updated_at,
    }))
}

pub(super) fn read_profile_observational(
    transaction: &Transaction<'_>,
    node_id: &str,
) -> Result<(Option<ProfileSnapshot>, Option<i64>), RegistryError> {
    let row = transaction
        .query_row(
            "SELECT profile_revision, agent_version, arch, capabilities, display_name,
                    distro_id, distro_version, omarchy_channel, omarchy_version, platform,
                    role, runtimes, baseline_id, baseline_observed_id
             FROM health_profiles WHERE node_id = ?1",
            params![node_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                ))
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok((None, None));
    };
    let capabilities = serde_json::from_str::<Vec<String>>(&row.3);
    let runtimes = serde_json::from_str::<Vec<RuntimeFact>>(&row.11);
    let (Ok(capabilities), Ok(runtimes)) = (capabilities, runtimes) else {
        return Ok((None, Some(row.0)));
    };
    Ok((
        Some(ProfileSnapshot {
            agent_version: row.1,
            arch: row.2,
            baseline_id: row.12,
            baseline_observed_id: row.13,
            capabilities,
            display_name: row.4,
            distro_id: row.5,
            distro_version: row.6,
            omarchy_channel: row.7,
            omarchy_version: row.8,
            platform: row.9,
            profile_revision: row.0.max(0) as u64,
            role: row.10,
            runtimes,
        }),
        None,
    ))
}

pub(super) fn read_pulse_observational(
    transaction: &Transaction<'_>,
    node_id: &str,
) -> Result<(Option<PulseSnapshot>, Option<i64>), RegistryError> {
    let row = transaction
        .query_row(
            "SELECT sequence, emitted_at, profile_revision, runner_state, scheduler_state,
                    queue_depth, workers_busy, workers_configured, uptime_seconds, last_run
             FROM health_pulses WHERE node_id = ?1",
            params![node_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok((None, None));
    };
    let last_run = match row.9.as_deref() {
        None => None,
        Some(text) => match serde_json::from_str::<RunFact>(text) {
            Ok(fact) => Some(fact),
            Err(_) => return Ok((None, Some(row.0))),
        },
    };
    Ok((
        Some(PulseSnapshot {
            emitted_at: row.1,
            last_run,
            profile_revision: row.2.max(0) as u64,
            runner: RunnerFact {
                queue_depth: row.5.max(0) as u64,
                scheduler: row.4,
                state: row.3,
                workers_busy: row.6.max(0) as u64,
                workers_configured: row.7.max(0) as u64,
            },
            sequence: row.0.max(0) as u64,
            uptime_seconds: row.8.max(0) as u64,
        }),
        None,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CorruptHealthIdentity {
    Profile { profile_revision: i64 },
    Pulse { sequence: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CorruptHealthRow {
    pub(super) table: &'static str,
    pub(super) node_id: String,
    pub(super) kind: HealthKind,
    pub(super) identity: CorruptHealthIdentity,
}

fn quarantine_row_if_observed(
    transaction: &Transaction<'_>,
    row: &CorruptHealthRow,
    now: i64,
) -> Result<(), RegistryError> {
    let deleted = match (row.table, row.identity) {
        ("health_profiles", CorruptHealthIdentity::Profile { profile_revision }) => transaction
            .execute(
                "DELETE FROM health_profiles
                 WHERE node_id = ?1 AND profile_revision = ?2",
                params![row.node_id, profile_revision],
            )?,
        ("health_pulses", CorruptHealthIdentity::Pulse { sequence }) => transaction.execute(
            "DELETE FROM health_pulses
                 WHERE node_id = ?1 AND sequence = ?2",
            params![row.node_id, sequence],
        )?,
        _ => {
            return Err(RegistryError::Corrupt(format!(
                "corrupt health row identity does not match table {:?}",
                row.table
            )))
        }
    };
    if deleted == 1 {
        record_health_audit_tx(
            transaction,
            "corrupt_row",
            &row.node_id,
            row.kind.wire(),
            0,
            "rejected",
            Some(HealthCode::CorruptState.code()),
            now,
        )?;
    }
    Ok(())
}

pub(super) fn cleanup_corrupt_health_rows(
    registry: &NodeRegistry,
    corrupt: &[CorruptHealthRow],
    now: i64,
) -> Result<(), RegistryError> {
    if corrupt.is_empty() {
        return Ok(());
    }
    registry.with_mutating_connection(|connection| {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for row in corrupt {
            quarantine_row_if_observed(&transaction, row, now)?;
        }
        transaction.commit()?;
        Ok(())
    })
}

type StoredSignalRow = (Vec<u8>, i64, String, i64, Option<String>, Option<String>);

pub(super) fn signal_from_row(row: &StoredSignalRow) -> Result<SignalRecord, RegistryError> {
    let kind = SignalKind::parse(&row.2)
        .ok_or_else(|| RegistryError::Corrupt("unknown stored signal kind".to_string()))?;
    let run = match row.5.as_deref() {
        None => None,
        Some(text) => Some(
            serde_json::from_str::<RunFact>(text)
                .map_err(|_| RegistryError::Corrupt("stored signal run is invalid".to_string()))?,
        ),
    };
    if row.0.len() != 16 {
        return Err(RegistryError::Corrupt(
            "stored signal id has invalid length".to_string(),
        ));
    }
    Ok(SignalRecord {
        kind,
        occurred_at: row.3,
        run,
        sequence: row.1.max(0) as u64,
        signal_id: hex::encode(&row.0),
        subject: row.4.clone(),
    })
}

pub(super) fn decode_opaque_id(value: &str) -> Result<Vec<u8>, RegistryError> {
    if value.len() != 32 {
        return Err(RegistryError::InvalidInput(
            "health opaque identifier must be 32 hexadecimal characters".to_string(),
        ));
    }
    decode_hex(value)
}

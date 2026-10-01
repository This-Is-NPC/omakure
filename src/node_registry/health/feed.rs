use super::super::audit::lifecycle_trust_events_in;
use super::super::fields::validate_node_id;
use super::super::{NodeRegistry, RegistryError};
use super::audit::record_health_audit_tx;
use super::rows::{
    active_trust_predicate, authorization_in, cleanup_corrupt_health_rows, health_peer_from_row,
    load_peer_state, read_profile_observational, read_pulse_observational, signal_from_row,
    CorruptHealthIdentity, CorruptHealthRow,
};
use super::types::{
    HealthFeedPeer, HealthFeedSignal, HealthFleetPeer, HealthPeerSnapshot, HealthPeerState,
    HealthSignalFeed,
};
use crate::domain::health_plane::bounds::SIGNAL_INBOX_CAPACITY;
use crate::domain::health_plane::model::{HealthCode, HealthKind, SignalRecord};
use rusqlite::{params, Connection, Transaction, TransactionBehavior};

const MAX_HEALTH_READ_ROWS: usize = 4_096;

impl NodeRegistry {
    /// The durable Health Plane state for every tracked peer.
    #[cfg(test)]
    pub(crate) fn health_peer_states(&self) -> Result<Vec<HealthPeerState>, RegistryError> {
        self.with_connection(|connection| peer_states_in(connection))
    }

    /// The fleet-status projection input for every tracked peer, read as one
    /// snapshot.
    ///
    /// One transaction covers the stored state, the Profile, the Pulse, and
    /// the trust decision of every peer, so the report describes a fleet the
    /// node actually had rather than a mixture of instants.
    pub fn health_fleet_snapshot(&self, now: i64) -> Result<Vec<HealthFleetPeer>, RegistryError> {
        let (peers, corrupt) = self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let mut peers = Vec::new();
            let mut corrupt = Vec::new();
            for state in peer_states_in(&transaction)? {
                let (peer, mut peer_corrupt) = fleet_peer_in(&transaction, state)?;
                peers.push(peer);
                corrupt.append(&mut peer_corrupt);
            }
            transaction.commit()?;
            Ok((peers, corrupt))
        })?;
        cleanup_corrupt_health_rows(self, &corrupt, now)?;
        Ok(peers)
    }

    /// The fleet-status projection input for one peer, read as one snapshot.
    pub fn health_node_snapshot(
        &self,
        node_id: &str,
        now: i64,
    ) -> Result<Option<HealthFleetPeer>, RegistryError> {
        validate_node_id(node_id)?;
        let (peer, corrupt) = self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let result = match load_peer_state(&transaction, node_id)? {
                Some(state) => {
                    let (peer, corrupt) = fleet_peer_in(&transaction, state)?;
                    (Some(peer), corrupt)
                }
                None => (None, Vec::new()),
            };
            transaction.commit()?;
            Ok(result)
        })?;
        cleanup_corrupt_health_rows(self, &corrupt, now)?;
        Ok(peer)
    }

    /// The whole bounded Signal read surface, read as one snapshot.
    ///
    /// The per-peer counters, the bounded page of Signals they describe, and
    /// the trust transitions the local lifecycle Signals project from are all
    /// read in one transaction. Separate reads let ingest commit in between,
    /// which is how a feed came to report a Signal beside a cursor that had
    /// not counted it.
    pub fn health_signal_feed(
        &self,
        limit: usize,
        now: i64,
    ) -> Result<HealthSignalFeed, RegistryError> {
        let limit = limit.min(SIGNAL_INBOX_CAPACITY as usize);
        let (peers, signals, lifecycle, corrupt) = self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let mut peers = Vec::new();
            for state in peer_states_in(&transaction)? {
                let authorization = authorization_in(&transaction, &state.node_id)?;
                peers.push(HealthFeedPeer {
                    state,
                    authorization,
                });
            }
            let (signals, corrupt) = feed_page_in(&transaction, limit)?;
            // The lifecycle projection collapses transitions per peer, so it
            // needs the whole bounded scan window rather than one page of it.
            let lifecycle = lifecycle_trust_events_in(&transaction, usize::MAX)?;
            transaction.commit()?;
            Ok((peers, signals, lifecycle, corrupt))
        })?;

        // A malformed Signal is quarantined after the consistent read snapshot
        // commits. Keeping cleanup in its own Immediate transaction means a
        // concurrent writer cannot make the observational transaction fail to
        // upgrade, while cleanup is still durable and never silently skipped.
        if !corrupt.is_empty() {
            self.with_mutating_connection(|connection| {
                let transaction =
                    connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                cleanup_corrupt_signal_rows(&transaction, &corrupt, now)?;
                transaction.commit()?;
                Ok(())
            })?;
        }

        Ok(HealthSignalFeed {
            peers,
            signals,
            lifecycle,
        })
    }

    /// The bounded, ordered Signal inbox for one peer. Held reorder-buffer rows
    /// are never returned: only Signals the cursor has accepted are visible.
    pub fn health_signals(
        &self,
        node_id: &str,
        limit: usize,
        now: i64,
    ) -> Result<Vec<SignalRecord>, RegistryError> {
        validate_node_id(node_id)?;
        let limit = limit.min(SIGNAL_INBOX_CAPACITY as usize);
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut corrupt = Vec::new();
            let signals = {
                let mut statement = transaction.prepare(
                    "SELECT signal_id, sequence, kind, occurred_at, subject, run
                     FROM health_signals
                     WHERE node_id = ?1 AND state = 'applied'
                     ORDER BY sequence LIMIT ?2",
                )?;
                let rows = statement
                    .query_map(params![node_id, limit as i64], |row| {
                        Ok((
                            row.get::<_, Vec<u8>>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut signals = Vec::with_capacity(rows.len());
                for row in rows {
                    match signal_from_row(&row) {
                        Ok(signal) => signals.push(signal),
                        Err(_) => corrupt.push(row.0),
                    }
                }
                signals
            };
            for signal_id in &corrupt {
                transaction.execute(
                    "DELETE FROM health_signals WHERE node_id = ?1 AND signal_id = ?2",
                    params![node_id, signal_id],
                )?;
                record_health_audit_tx(
                    &transaction,
                    "corrupt_row",
                    node_id,
                    HealthKind::Signal.wire(),
                    0,
                    "rejected",
                    Some(HealthCode::CorruptState.code()),
                    now,
                )?;
            }
            transaction.commit()?;
            Ok(signals)
        })
    }
}

/// The durable Health Plane state for every tracked peer, on a connection the
/// caller owns.
fn peer_states_in(connection: &Connection) -> Result<Vec<HealthPeerState>, RegistryError> {
    let mut statement = connection.prepare(
        "SELECT p.node_id, p.role, p.cursor, p.last_profile_revision,
                p.last_pulse_sequence, p.last_pulse_at, p.version_incompatible_at,
                p.first_seen, p.updated_at,
                (SELECT COUNT(*) FROM health_signals s
                  WHERE s.node_id = p.node_id AND s.state = 'applied'),
                (SELECT COUNT(*) FROM health_signals s
                  WHERE s.node_id = p.node_id AND s.state = 'held')
         FROM health_peers p ORDER BY p.node_id",
    )?;
    let rows = statement
        .query_map([], health_peer_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.len() > MAX_HEALTH_READ_ROWS {
        return Err(RegistryError::Corrupt(
            "health peer table exceeds the frozen node-count bound".to_string(),
        ));
    }
    rows.into_iter().collect::<Result<Vec<_>, _>>()
}

/// Everything one fleet-status row is projected from, inside one transaction.
pub(super) fn fleet_peer_in(
    transaction: &Transaction<'_>,
    state: HealthPeerState,
) -> Result<(HealthFleetPeer, Vec<CorruptHealthRow>), RegistryError> {
    let authorization = authorization_in(transaction, &state.node_id)?;
    let (profile, profile_corrupt) = read_profile_observational(transaction, &state.node_id)?;
    let (pulse, pulse_corrupt) = read_pulse_observational(transaction, &state.node_id)?;
    let mut corrupt = Vec::new();
    if let Some(profile_revision) = profile_corrupt {
        corrupt.push(CorruptHealthRow {
            table: "health_profiles",
            node_id: state.node_id.clone(),
            kind: HealthKind::Profile,
            identity: CorruptHealthIdentity::Profile { profile_revision },
        });
    }
    if let Some(sequence) = pulse_corrupt {
        corrupt.push(CorruptHealthRow {
            table: "health_pulses",
            node_id: state.node_id.clone(),
            kind: HealthKind::Pulse,
            identity: CorruptHealthIdentity::Pulse { sequence },
        });
    }
    Ok((
        HealthFleetPeer {
            snapshot: HealthPeerSnapshot {
                state,
                profile,
                pulse,
            },
            authorization,
        },
        corrupt,
    ))
}

/// The bounded, newest-first page of Signals across the actively trusted
/// fleet, inside one transaction.
///
/// Newest-first in SQL rather than in the caller keeps the working set at one
/// page no matter how many Performers this Conductor manages, which is what
/// the per-peer loop it replaces achieved by reducing after every peer.
type HealthFeedPage = (Vec<HealthFeedSignal>, Vec<CorruptSignalIdentity>);

type CorruptSignalIdentity = (String, Vec<u8>);

fn feed_page_in(
    transaction: &Transaction<'_>,
    limit: usize,
) -> Result<HealthFeedPage, RegistryError> {
    let mut corrupt: Vec<CorruptSignalIdentity> = Vec::new();
    let mut page = Vec::new();
    {
        // `signal_id` is a fixed 16-byte identifier, so ordering the blob
        // descending is the same total order the rendered hexadecimal gives.
        let statement = format!(
            "SELECT s.node_id, s.signal_id, s.sequence, s.kind, s.occurred_at, s.subject, s.run
             FROM health_signals s
             WHERE s.state = 'applied' AND {}
             ORDER BY s.occurred_at DESC, s.signal_id DESC
             LIMIT ?1",
            active_trust_predicate("s.node_id")
        );
        let mut statement = transaction.prepare(&statement)?;
        let rows = statement
            .query_map(params![limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ),
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (node_id, row) in rows {
            match signal_from_row(&row) {
                Ok(signal) => page.push(HealthFeedSignal { node_id, signal }),
                Err(_) => corrupt.push((node_id, row.0)),
            }
        }
    }
    Ok((page, corrupt))
}

fn cleanup_corrupt_signal_rows(
    transaction: &Transaction<'_>,
    corrupt: &[(String, Vec<u8>)],
    now: i64,
) -> Result<(), RegistryError> {
    for (node_id, signal_id) in corrupt {
        transaction.execute(
            "DELETE FROM health_signals WHERE node_id = ?1 AND signal_id = ?2",
            params![node_id, signal_id],
        )?;
        record_health_audit_tx(
            transaction,
            "corrupt_row",
            node_id,
            HealthKind::Signal.wire(),
            0,
            "rejected",
            Some(HealthCode::CorruptState.code()),
            now,
        )?;
    }
    Ok(())
}

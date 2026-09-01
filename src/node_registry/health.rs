//! Bounded Health Plane persistence owned by the node registry.
//!
//! Every statement in this module touches Health Plane tables only.  There is
//! no code path from a Health Plane message to identity, trust, capability,
//! revocation, transport session, or run state, and the module never creates a
//! second database, a generic repository, an event bus, a metric store, or a
//! historical query engine.

use super::{
    decode_hex, lifecycle_trust_events_in, validate_node_id, AuditEvent, NodeRegistry, PeerRole,
    PeerState, RegistryError, SCHEMA_VERSION,
};
use crate::health_plane::bounds::{
    AUDIT_RETENTION_SECONDS, AUDIT_ROW_BYTES, MAX_AUDIT_ROWS, MAX_CONDUCTORS_PER_PERFORMER,
    MAX_MESSAGES_PER_PEER_PER_MINUTE, MAX_PERFORMERS_PER_CONDUCTOR, MAX_PROFILES_PER_PEER_PER_HOUR,
    MAX_REPLAY_ROWS, MAX_SIGNALS_PER_PEER_PER_MINUTE, MIN_PULSE_INTERVAL_SECONDS,
    RATE_BURST_ALLOWANCE, RATE_HOUR_WINDOW_SECONDS, RATE_MINUTE_WINDOW_SECONDS,
    REORDER_BUFFER_ENTRIES, REORDER_BUFFER_SECONDS, REPLAY_RETENTION_SECONDS, REPLAY_ROW_BYTES,
    REPLAY_SECURITY_FLOOR_SECONDS, SIGNAL_GLOBAL_INBOX_CAPACITY, SIGNAL_INBOX_CAPACITY,
    SIGNAL_OUTBOX_CAPACITY, SIGNAL_RETENTION_SECONDS, VERSION_INCOMPATIBLE_EXPIRY_SECONDS,
};
use crate::health_plane::model::{
    HealthBody, HealthCode, HealthDecision, HealthKind, HealthPayload, ProfileSnapshot,
    PulseSnapshot, RunFact, RunnerFact, RuntimeFact, SignalKind, SignalRecord,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

const MAX_HEALTH_READ_ROWS: usize = 4_096;

/// The single read-only projection over `trusted_peers` the Health Plane needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthAuthorization {
    pub node_id: String,
    pub state: PeerState,
    pub role: PeerRole,
    pub capabilities: Vec<String>,
}

/// The durable per-peer Health Plane state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthPeerState {
    pub node_id: String,
    pub role: PeerRole,
    pub cursor: u64,
    pub last_profile_revision: u64,
    pub last_pulse_sequence: u64,
    pub last_pulse_at: Option<i64>,
    pub stored_signals: u64,
    pub held_signals: u64,
    pub version_incompatible: bool,
    pub first_seen: i64,
    pub updated_at: i64,
}

/// One redacted Health Plane audit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthAuditEvent {
    pub id: i64,
    pub event_code: String,
    pub node_id: String,
    pub message_kind: String,
    pub byte_count: i64,
    pub outcome: String,
    pub error_code: Option<u16>,
    pub occurred_at: i64,
}

/// One pending Signal in the bounded Performer outbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthOutboxEntry {
    pub signal_id: String,
    pub target_node_id: String,
    pub sequence: u64,
    pub signal: SignalRecord,
    pub attempts: i64,
    pub enqueued_at: i64,
    pub expires_at: i64,
}

/// What one pruning pass removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HealthPruneReport {
    pub expired_signals: u64,
    pub evicted_signals: u64,
    pub expired_held_signals: u64,
    pub expired_replay_keys: u64,
    pub evicted_replay_keys: u64,
    pub pruned_audit_rows: u64,
    pub expired_outbox_signals: u64,
    pub cleared_version_incompatible: u64,
}

/// Everything the Health Plane currently stores about one peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthPeerSnapshot {
    pub state: HealthPeerState,
    pub profile: Option<ProfileSnapshot>,
    pub pulse: Option<PulseSnapshot>,
}

/// Everything one fleet-status row is projected from, read together.
///
/// The authorization travels beside the stored state because the projection
/// reports both as one row: a peer whose trust ends between two reads would
/// otherwise be rendered from a stored snapshot that the trust decision no
/// longer matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthFleetPeer {
    pub snapshot: HealthPeerSnapshot,
    pub authorization: Option<HealthAuthorization>,
}

/// One peer's Signal cursor state, as the feed reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthFeedPeer {
    pub state: HealthPeerState,
    pub authorization: Option<HealthAuthorization>,
}

/// One Signal in the bounded feed page, tagged with the peer that reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthFeedSignal {
    pub node_id: String,
    pub signal: SignalRecord,
}

/// The whole Signal read surface, captured in exactly one transaction.
///
/// The cursors and the Signals they describe are read together on purpose.
/// Assembled from separate reads, the projection can contradict itself: the
/// counters are snapshotted, ingest commits, and the later read returns a
/// Signal the counters have not counted. That is not only a test-visible
/// oddity — `gap`, the field an operator reads to decide whether a fleet's
/// Signal delivery has stalled, is derived from those same counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthSignalFeed {
    /// Per-peer cursor state, ordered by node ID.
    pub peers: Vec<HealthFeedPeer>,
    /// The bounded fleet-wide page, newest first.
    pub signals: Vec<HealthFeedSignal>,
    /// The append-only trust transitions the local lifecycle Signals project
    /// from, read in the same transaction as everything they are merged with.
    pub lifecycle: Vec<AuditEvent>,
}

/// A validated message ready to be applied under the frozen receive order.
#[derive(Debug, Clone)]
pub(crate) struct HealthApplyRequest<'a> {
    pub sender: &'a str,
    pub payload: &'a HealthPayload,
    pub created_at: i64,
    pub now: i64,
    pub message_bytes: i64,
}

impl NodeRegistry {
    /// Report whether the schema version 7 Health Plane tables are present.
    /// A node whose Health Plane migration failed keeps serving transport,
    /// enrollment, HTTP, and runs with the Health Plane disabled.
    pub fn health_plane_enabled(&self) -> Result<bool, RegistryError> {
        self.with_connection(|connection| {
            let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            Ok(version >= SCHEMA_VERSION)
        })
    }

    /// Clear the "migration already failed" marker so an operator can retry the
    /// Health Plane migration explicitly. Retry is never automatic.
    pub fn clear_health_plane_migration_block(&self) -> Result<bool, RegistryError> {
        self.with_connection(|connection| {
            let removed = connection.execute(
                "DELETE FROM metadata WHERE key = 'health_plane' AND value = 'disabled'",
                [],
            )?;
            Ok(removed > 0)
        })
    }

    /// The one new read-only projection required by Health Plane
    /// authorization: `role` and `capabilities` from `trusted_peers` alongside
    /// the identity and trust state. It creates nothing and mutates nothing.
    pub fn health_authorization(
        &self,
        node_id: &str,
    ) -> Result<Option<HealthAuthorization>, RegistryError> {
        validate_node_id(node_id)?;
        self.with_connection(|connection| authorization_in(connection, node_id))
    }

    /// Apply receive-order steps 7 through 15 in exactly one transaction.
    ///
    /// Every rejection leaves identity, trust, revocation, transport session,
    /// and run state untouched, and writes one redacted audit row.
    pub(crate) fn apply_health_message(
        &self,
        request: HealthApplyRequest<'_>,
    ) -> Result<HealthDecision, RegistryError> {
        validate_node_id(request.sender)?;
        let kind = request.payload.body.kind();
        if let Some(cap) = kind.max_stored_bytes() {
            if request.message_bytes > cap {
                return Ok(HealthDecision::Rejected(HealthCode::MessageTooLarge));
            }
        }
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let decision = evaluate(&transaction, &request)?;
            record_health_audit_tx(
                &transaction,
                kind.wire(),
                request.sender,
                kind.wire(),
                request.message_bytes,
                decision.outcome(),
                decision.code().map(HealthCode::code),
                request.now,
            )?;
            transaction.commit()?;
            Ok(decision)
        })
    }

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
        self.with_connection(|connection| {
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

    /// Mark a peer as speaking an unsupported Health Plane version.
    pub(crate) fn mark_health_version_incompatible(
        &self,
        node_id: &str,
        now: i64,
    ) -> Result<(), RegistryError> {
        validate_node_id(node_id)?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute(
                "UPDATE health_peers SET version_incompatible_at = ?2, updated_at = ?2
                 WHERE node_id = ?1",
                params![node_id, now],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    /// The durable Health Plane state for every tracked peer.
    pub fn health_peer_states(&self) -> Result<Vec<HealthPeerState>, RegistryError> {
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
            self.with_connection(|connection| {
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

    /// The current Profile and Pulse snapshots for one peer.
    ///
    /// A single row that fails its integrity check is deleted, audited with
    /// `health_corrupt_state` (1115), and reported as absent.
    pub fn health_peer_snapshot(
        &self,
        node_id: &str,
        now: i64,
    ) -> Result<Option<HealthPeerSnapshot>, RegistryError> {
        validate_node_id(node_id)?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let state = load_peer_state(&transaction, node_id)?;
            let Some(state) = state else {
                transaction.commit()?;
                return Ok(None);
            };
            let profile = read_profile(&transaction, node_id, now)?;
            let pulse = read_pulse(&transaction, node_id, now)?;
            transaction.commit()?;
            Ok(Some(HealthPeerSnapshot {
                state,
                profile,
                pulse,
            }))
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
        self.with_connection(|connection| {
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

    /// Delete all Health Plane state for peers that are no longer actively
    /// trusted. Health Plane state is derived and disposable; trust rows,
    /// revocations, and identities are never touched.
    pub fn health_purge_revoked(&self, now: i64) -> Result<Vec<String>, RegistryError> {
        self.with_connection(|connection| {
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
        self.with_connection(|connection| {
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
        self.with_connection(|connection| {
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
        self.with_connection(|connection| {
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
        self.with_connection(|connection| {
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

fn health_authorization_from_row(
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
        Some(1) => PeerRole::Conductor,
        Some(2) | None => PeerRole::Performer,
        Some(other) => {
            return Err(RegistryError::InvalidSchema(format!(
                "unknown trusted peer role {other}"
            )))
        }
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

fn evaluate(
    transaction: &Transaction<'_>,
    request: &HealthApplyRequest<'_>,
) -> Result<HealthDecision, RegistryError> {
    let kind = request.payload.body.kind();

    // Step 7: trust, read from the local registry only.
    let authorization = transaction
        .query_row(
            "SELECT r.state, p.role, p.capabilities, p.state
             FROM remote_identities r
             LEFT JOIN trusted_peers p ON p.node_id = r.node_id
             WHERE r.node_id = ?1",
            params![request.sender],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((identity_state, role, capabilities, trust_state)) = authorization else {
        return Ok(HealthDecision::Rejected(HealthCode::Revoked));
    };
    let authorization = health_authorization_from_row(
        request.sender,
        &identity_state,
        role,
        capabilities,
        trust_state.as_deref(),
    )?;
    if authorization.state != PeerState::Active || trust_state.is_none() {
        return Ok(HealthDecision::Rejected(HealthCode::Revoked));
    }

    // Step 8: role and the single-Conductor bound.
    let stored_role = match authorization.role {
        PeerRole::Conductor => 1,
        PeerRole::Performer => 2,
    };
    if stored_role != kind.required_role() {
        return Ok(HealthDecision::Rejected(HealthCode::WrongRole));
    }
    if stored_role == 1 {
        let other_conductors: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM health_peers WHERE role = 1 AND node_id <> ?1",
            params![request.sender],
            |row| row.get(0),
        )?;
        if other_conductors >= MAX_CONDUCTORS_PER_PERFORMER {
            return Ok(HealthDecision::Rejected(HealthCode::WrongRole));
        }
    }

    // Step 9: capability, read from the local registry only.
    if let Some(required) = kind.required_capability() {
        if !authorization
            .capabilities
            .iter()
            .any(|entry| entry == required)
        {
            return Ok(HealthDecision::Rejected(HealthCode::MissingCapability));
        }
    }

    // Step 10: freshness.
    if request.created_at
        > request
            .now
            .saturating_add(crate::health_plane::bounds::MAX_FUTURE_SKEW_SECONDS)
    {
        return Ok(HealthDecision::Rejected(HealthCode::Future));
    }
    if request.now.saturating_sub(request.created_at) > crate::health_plane::bounds::MAX_AGE_SECONDS
    {
        return Ok(HealthDecision::Rejected(HealthCode::Stale));
    }

    let mut state = load_peer_state(transaction, request.sender)?;

    // Step 11: rate.
    if let Some(existing) = state.as_ref() {
        if let Some(code) = rate_check(transaction, existing, kind, request.now)? {
            return Ok(HealthDecision::Rejected(code));
        }
    }

    // Step 12: replay.
    let message_id = decode_opaque_id(&request.payload.message_id)?;
    let seen: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM health_replay_keys WHERE message_id = ?1",
        params![message_id],
        |row| row.get(0),
    )?;
    if seen > 0 {
        return Ok(HealthDecision::Rejected(HealthCode::Replay));
    }

    // Step 13: ordering.
    let cursor = state.as_ref().map(|state| state.cursor).unwrap_or(0);
    let mut hold = false;
    match &request.payload.body {
        HealthBody::Profile(profile) => {
            let last = state
                .as_ref()
                .map(|state| state.last_profile_revision)
                .unwrap_or(0);
            if profile.profile_revision <= last {
                return Ok(HealthDecision::Rejected(HealthCode::Replay));
            }
        }
        HealthBody::Pulse(pulse) => {
            let last = state
                .as_ref()
                .map(|state| state.last_pulse_sequence)
                .unwrap_or(0);
            if pulse.sequence <= last {
                return Ok(HealthDecision::Rejected(HealthCode::Replay));
            }
        }
        HealthBody::Signal(signal) => {
            let signal_id = decode_opaque_id(&signal.signal_id)?;
            let duplicate: i64 = transaction.query_row(
                "SELECT COUNT(*) FROM health_signals
                 WHERE node_id = ?1 AND (signal_id = ?2 OR sequence = ?3)",
                params![request.sender, signal_id, signal.sequence as i64],
                |row| row.get(0),
            )?;
            if signal.sequence <= cursor || duplicate > 0 {
                return Ok(HealthDecision::Rejected(HealthCode::Replay));
            }
            if signal.sequence > cursor.saturating_add(REORDER_BUFFER_ENTRIES) {
                return Ok(HealthDecision::Rejected(HealthCode::Reordered));
            }
            hold = signal.sequence != cursor + 1;
        }
        HealthBody::Ack(_) | HealthBody::Error(_) => {}
    }

    // Step 14: capacity.
    if state.is_none() {
        let tracked: i64 =
            transaction.query_row("SELECT COUNT(*) FROM health_peers", [], |row| row.get(0))?;
        if tracked >= MAX_PERFORMERS_PER_CONDUCTOR {
            return Ok(HealthDecision::Rejected(HealthCode::QueueFull));
        }
    }
    if matches!(request.payload.body, HealthBody::Signal(_)) {
        let stored: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM health_signals WHERE node_id = ?1",
            params![request.sender],
            |row| row.get(0),
        )?;
        let global: i64 =
            transaction.query_row("SELECT COUNT(*) FROM health_signals", [], |row| row.get(0))?;
        if stored >= SIGNAL_INBOX_CAPACITY || global >= SIGNAL_GLOBAL_INBOX_CAPACITY {
            return Ok(HealthDecision::Rejected(HealthCode::QueueFull));
        }
    }

    // Step 15: apply, inside this single transaction.
    if !record_replay_key(transaction, &message_id, request.sender, request.now)? {
        return Ok(HealthDecision::Rejected(HealthCode::RateLimited));
    }
    if state.is_none() {
        transaction.execute(
            "INSERT INTO health_peers
             (node_id, role, cursor, last_profile_revision, last_pulse_sequence, last_pulse_at,
              version_incompatible_at, minute_window_start, minute_messages, minute_signals,
              hour_window_start, hour_profiles, first_seen, updated_at)
             VALUES (?1, ?2, 0, 0, 0, NULL, NULL, ?3, 0, 0, ?3, 0, ?3, ?3)",
            params![request.sender, stored_role, request.now],
        )?;
        state = load_peer_state(transaction, request.sender)?;
    }
    let Some(existing) = state else {
        return Err(RegistryError::Corrupt(
            "health peer state disappeared during apply".to_string(),
        ));
    };
    count_rate(transaction, request.sender, kind, request.now)?;

    let decision = match &request.payload.body {
        HealthBody::Profile(profile) => {
            store_profile(transaction, request, profile)?;
            transaction.execute(
                "UPDATE health_peers SET last_profile_revision = ?2, updated_at = ?3
                 WHERE node_id = ?1",
                params![request.sender, profile.profile_revision as i64, request.now],
            )?;
            HealthDecision::Accepted {
                cursor: existing.cursor,
            }
        }
        HealthBody::Pulse(pulse) => {
            store_pulse(transaction, request, pulse)?;
            transaction.execute(
                "UPDATE health_peers
                 SET last_pulse_sequence = ?2, last_pulse_at = ?3, updated_at = ?3
                 WHERE node_id = ?1",
                params![request.sender, pulse.sequence as i64, request.now],
            )?;
            HealthDecision::Accepted {
                cursor: existing.cursor,
            }
        }
        HealthBody::Signal(signal) => {
            store_signal(transaction, request, signal, hold)?;
            if hold {
                HealthDecision::Held {
                    cursor: existing.cursor,
                }
            } else {
                let cursor = advance_cursor(transaction, request.sender, signal.sequence)?;
                HealthDecision::Accepted { cursor }
            }
        }
        HealthBody::Ack(ack) => {
            let acked = decode_opaque_id(&ack.acked_message_id)?;
            transaction.execute(
                "DELETE FROM health_outbox WHERE last_message_id = ?1",
                params![acked],
            )?;
            transaction.execute(
                "UPDATE health_peers SET cursor = ?2, updated_at = ?3 WHERE node_id = ?1",
                params![request.sender, ack.cursor as i64, request.now],
            )?;
            HealthDecision::Accepted { cursor: ack.cursor }
        }
        HealthBody::Error(error) => {
            if error.code == HealthCode::UnsupportedVersion.code() {
                transaction.execute(
                    "UPDATE health_peers SET version_incompatible_at = ?2, updated_at = ?2
                     WHERE node_id = ?1",
                    params![request.sender, request.now],
                )?;
            }
            HealthDecision::Accepted {
                cursor: existing.cursor,
            }
        }
    };
    Ok(decision)
}

fn rate_check(
    transaction: &Transaction<'_>,
    state: &HealthPeerState,
    kind: HealthKind,
    now: i64,
) -> Result<Option<HealthCode>, RegistryError> {
    let (minute_start, minute_messages, minute_signals, hour_start, hour_profiles): (
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = transaction.query_row(
        "SELECT minute_window_start, minute_messages, minute_signals,
                hour_window_start, hour_profiles
         FROM health_peers WHERE node_id = ?1",
        params![state.node_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let minute_expired = now.saturating_sub(minute_start) >= RATE_MINUTE_WINDOW_SECONDS;
    let minute_messages = if minute_expired { 0 } else { minute_messages };
    let minute_signals = if minute_expired { 0 } else { minute_signals };
    let hour_profiles = if now.saturating_sub(hour_start) >= RATE_HOUR_WINDOW_SECONDS {
        0
    } else {
        hour_profiles
    };
    if minute_messages >= MAX_MESSAGES_PER_PEER_PER_MINUTE + RATE_BURST_ALLOWANCE {
        return Ok(Some(HealthCode::RateLimited));
    }
    match kind {
        HealthKind::Profile if hour_profiles >= MAX_PROFILES_PER_PEER_PER_HOUR => {
            return Ok(Some(HealthCode::RateLimited))
        }
        HealthKind::Signal if minute_signals >= MAX_SIGNALS_PER_PEER_PER_MINUTE => {
            return Ok(Some(HealthCode::RateLimited))
        }
        HealthKind::Pulse => {
            if let Some(last_pulse_at) = state.last_pulse_at {
                if now.saturating_sub(last_pulse_at) < MIN_PULSE_INTERVAL_SECONDS {
                    return Ok(Some(HealthCode::RateLimited));
                }
            }
        }
        _ => {}
    }
    Ok(None)
}

fn count_rate(
    transaction: &Transaction<'_>,
    node_id: &str,
    kind: HealthKind,
    now: i64,
) -> Result<(), RegistryError> {
    transaction.execute(
        "UPDATE health_peers
         SET minute_messages = CASE
               WHEN ?2 - minute_window_start >= ?3 THEN 0 ELSE minute_messages END,
             minute_signals = CASE
               WHEN ?2 - minute_window_start >= ?3 THEN 0 ELSE minute_signals END,
             hour_profiles = CASE
               WHEN ?2 - hour_window_start >= ?4 THEN 0 ELSE hour_profiles END,
             minute_window_start = CASE
               WHEN ?2 - minute_window_start >= ?3 THEN ?2 ELSE minute_window_start END,
             hour_window_start = CASE
               WHEN ?2 - hour_window_start >= ?4 THEN ?2 ELSE hour_window_start END
         WHERE node_id = ?1",
        params![
            node_id,
            now,
            RATE_MINUTE_WINDOW_SECONDS,
            RATE_HOUR_WINDOW_SECONDS
        ],
    )?;
    transaction.execute(
        "UPDATE health_peers
         SET minute_messages = minute_messages + 1,
             minute_signals = minute_signals + ?2,
             hour_profiles = hour_profiles + ?3,
             updated_at = ?4
         WHERE node_id = ?1",
        params![
            node_id,
            i64::from(kind == HealthKind::Signal),
            i64::from(kind == HealthKind::Profile),
            now
        ],
    )?;
    Ok(())
}

fn record_replay_key(
    transaction: &Transaction<'_>,
    message_id: &[u8],
    node_id: &str,
    now: i64,
) -> Result<bool, RegistryError> {
    let mut rows: i64 =
        transaction.query_row("SELECT COUNT(*) FROM health_replay_keys", [], |row| {
            row.get(0)
        })?;
    while rows >= MAX_REPLAY_ROWS {
        let floor = now.saturating_sub(REPLAY_SECURITY_FLOOR_SECONDS);
        let removed = transaction.execute(
            "DELETE FROM health_replay_keys WHERE message_id = (
               SELECT message_id FROM health_replay_keys
               WHERE first_seen <= ?1 ORDER BY expires_at, message_id LIMIT 1
             )",
            params![floor],
        )?;
        if removed == 0 {
            return Ok(false);
        }
        rows -= removed as i64;
    }
    transaction.execute(
        "INSERT INTO health_replay_keys (message_id, node_id, first_seen, expires_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            message_id,
            node_id,
            now,
            now.saturating_add(REPLAY_RETENTION_SECONDS)
        ],
    )?;
    Ok(true)
}

fn store_profile(
    transaction: &Transaction<'_>,
    request: &HealthApplyRequest<'_>,
    profile: &ProfileSnapshot,
) -> Result<(), RegistryError> {
    let capabilities = serde_json::to_string(&profile.capabilities)
        .map_err(|_| RegistryError::InvalidInput("profile capabilities".to_string()))?;
    let runtimes = serde_json::to_string(&profile.runtimes)
        .map_err(|_| RegistryError::InvalidInput("profile runtimes".to_string()))?;
    transaction.execute(
        "INSERT INTO health_profiles
         (node_id, profile_revision, agent_version, arch, capabilities, display_name,
          distro_id, distro_version, omarchy_channel, omarchy_version, platform, role,
          runtimes, message_bytes, received_at, baseline_id, baseline_observed_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
         ON CONFLICT(node_id) DO UPDATE SET
           profile_revision = excluded.profile_revision,
           agent_version = excluded.agent_version,
           arch = excluded.arch,
           capabilities = excluded.capabilities,
           display_name = excluded.display_name,
           distro_id = excluded.distro_id,
           distro_version = excluded.distro_version,
           omarchy_channel = excluded.omarchy_channel,
           omarchy_version = excluded.omarchy_version,
           platform = excluded.platform,
           role = excluded.role,
           runtimes = excluded.runtimes,
           message_bytes = excluded.message_bytes,
           received_at = excluded.received_at,
           baseline_id = excluded.baseline_id,
           baseline_observed_id = excluded.baseline_observed_id",
        params![
            request.sender,
            profile.profile_revision as i64,
            profile.agent_version,
            profile.arch,
            capabilities,
            profile.display_name,
            profile.distro_id,
            profile.distro_version,
            profile.omarchy_channel,
            profile.omarchy_version,
            profile.platform,
            profile.role,
            runtimes,
            request.message_bytes,
            request.now,
            profile.baseline_id,
            profile.baseline_observed_id,
        ],
    )?;
    Ok(())
}

fn store_pulse(
    transaction: &Transaction<'_>,
    request: &HealthApplyRequest<'_>,
    pulse: &PulseSnapshot,
) -> Result<(), RegistryError> {
    let last_run = pulse
        .last_run
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| RegistryError::InvalidInput("pulse last run".to_string()))?;
    transaction.execute(
        "INSERT INTO health_pulses
         (node_id, sequence, emitted_at, profile_revision, runner_state, scheduler_state,
          queue_depth, workers_busy, workers_configured, uptime_seconds, last_run,
          message_bytes, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(node_id) DO UPDATE SET
           sequence = excluded.sequence,
           emitted_at = excluded.emitted_at,
           profile_revision = excluded.profile_revision,
           runner_state = excluded.runner_state,
           scheduler_state = excluded.scheduler_state,
           queue_depth = excluded.queue_depth,
           workers_busy = excluded.workers_busy,
           workers_configured = excluded.workers_configured,
           uptime_seconds = excluded.uptime_seconds,
           last_run = excluded.last_run,
           message_bytes = excluded.message_bytes,
           received_at = excluded.received_at",
        params![
            request.sender,
            pulse.sequence as i64,
            pulse.emitted_at,
            pulse.profile_revision as i64,
            pulse.runner.state,
            pulse.runner.scheduler,
            pulse.runner.queue_depth as i64,
            pulse.runner.workers_busy as i64,
            pulse.runner.workers_configured as i64,
            pulse.uptime_seconds as i64,
            last_run,
            request.message_bytes,
            request.now,
        ],
    )?;
    Ok(())
}

fn store_signal(
    transaction: &Transaction<'_>,
    request: &HealthApplyRequest<'_>,
    signal: &SignalRecord,
    hold: bool,
) -> Result<(), RegistryError> {
    let signal_id = decode_opaque_id(&signal.signal_id)?;
    let run = signal
        .run
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| RegistryError::InvalidInput("signal run".to_string()))?;
    let lifetime = if hold {
        REORDER_BUFFER_SECONDS
    } else {
        SIGNAL_RETENTION_SECONDS
    };
    transaction.execute(
        "INSERT INTO health_signals
         (node_id, signal_id, sequence, state, kind, occurred_at, subject, run,
          message_bytes, received_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            request.sender,
            signal_id,
            signal.sequence as i64,
            if hold { "held" } else { "applied" },
            signal.kind.wire(),
            signal.occurred_at,
            signal.subject,
            run,
            request.message_bytes,
            request.now,
            request.now.saturating_add(lifetime),
        ],
    )?;
    Ok(())
}

/// Advance the cursor to `sequence` and promote every contiguous held Signal.
fn advance_cursor(
    transaction: &Transaction<'_>,
    node_id: &str,
    sequence: u64,
) -> Result<u64, RegistryError> {
    let mut cursor = sequence;
    loop {
        let next = cursor.saturating_add(1) as i64;
        let promoted = transaction.execute(
            "UPDATE health_signals
             SET state = 'applied', expires_at = received_at + ?3
             WHERE node_id = ?1 AND sequence = ?2 AND state = 'held'",
            params![node_id, next, SIGNAL_RETENTION_SECONDS],
        )?;
        if promoted == 0 {
            break;
        }
        cursor = next as u64;
    }
    transaction.execute(
        "UPDATE health_peers SET cursor = ?2 WHERE node_id = ?1",
        params![node_id, cursor as i64],
    )?;
    Ok(cursor)
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

/// Append one redacted Health Plane audit row. The row records only the stable
/// code, the peer node ID, the message kind, and byte counts; it never records
/// payload bytes, field values, signatures, or key material.
#[allow(clippy::too_many_arguments)]
fn record_health_audit_tx(
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

/// The read-only authorization projection, on a connection the caller owns.
fn authorization_in(
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

/// Everything one fleet-status row is projected from, inside one transaction.
fn fleet_peer_in(
    transaction: &Transaction<'_>,
    state: HealthPeerState,
) -> Result<(HealthFleetPeer, Vec<CorruptHealthRow>), RegistryError> {
    let authorization = authorization_in(transaction, &state.node_id)?;
    let (profile, profile_corrupt) = read_profile_observational(transaction, &state.node_id)?;
    let (pulse, pulse_corrupt) = read_pulse_observational(transaction, &state.node_id)?;
    let mut corrupt = Vec::new();
    if profile_corrupt {
        corrupt.push(("health_profiles", state.node_id.clone(), HealthKind::Profile));
    }
    if pulse_corrupt {
        corrupt.push(("health_pulses", state.node_id.clone(), HealthKind::Pulse));
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
fn feed_page_in(
    transaction: &Transaction<'_>,
    limit: usize,
) -> Result<(Vec<HealthFeedSignal>, Vec<(String, Vec<u8>)>), RegistryError> {
    let mut corrupt: Vec<(String, Vec<u8>)> = Vec::new();
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

/// The one predicate that decides whether a peer is still actively trusted.
///
/// The Signal feed must hide exactly what the revocation cleanup deletes, so
/// both statements build their clause here and cannot drift apart: a peer
/// whose trust ends stops appearing in the operator's feed on the next read
/// rather than on the next cleanup tick.
fn active_trust_predicate(node_column: &str) -> String {
    format!(
        "EXISTS (
           SELECT 1 FROM trusted_peers t
           JOIN remote_identities r ON r.node_id = t.node_id
           WHERE t.node_id = {node_column}
             AND t.state = 'active' AND r.state = 'active'
         )"
    )
}

fn load_peer_state(
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

fn health_peer_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HealthPeerRow> {
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
    let role = match role {
        1 => PeerRole::Conductor,
        2 => PeerRole::Performer,
        other => {
            return Ok(Err(RegistryError::Corrupt(format!(
                "health peer has unknown role {other}"
            ))))
        }
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

fn read_profile(
    transaction: &Transaction<'_>,
    node_id: &str,
    now: i64,
) -> Result<Option<ProfileSnapshot>, RegistryError> {
    let (profile, corrupt) = read_profile_observational(transaction, node_id)?;
    if corrupt {
        quarantine_row(transaction, "health_profiles", node_id, HealthKind::Profile, now)?;
    }
    Ok(profile)
}

fn read_profile_observational(
    transaction: &Transaction<'_>,
    node_id: &str,
) -> Result<(Option<ProfileSnapshot>, bool), RegistryError> {
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
        return Ok((None, false));
    };
    let capabilities = serde_json::from_str::<Vec<String>>(&row.3);
    let runtimes = serde_json::from_str::<Vec<RuntimeFact>>(&row.11);
    let (Ok(capabilities), Ok(runtimes)) = (capabilities, runtimes) else {
        return Ok((None, true));
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
        false,
    ))
}

fn read_pulse(
    transaction: &Transaction<'_>,
    node_id: &str,
    now: i64,
) -> Result<Option<PulseSnapshot>, RegistryError> {
    let (pulse, corrupt) = read_pulse_observational(transaction, node_id)?;
    if corrupt {
        quarantine_row(transaction, "health_pulses", node_id, HealthKind::Pulse, now)?;
    }
    Ok(pulse)
}

fn read_pulse_observational(
    transaction: &Transaction<'_>,
    node_id: &str,
) -> Result<(Option<PulseSnapshot>, bool), RegistryError> {
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
        return Ok((None, false));
    };
    let last_run = match row.9.as_deref() {
        None => None,
        Some(text) => match serde_json::from_str::<RunFact>(text) {
            Ok(fact) => Some(fact),
            Err(_) => return Ok((None, true)),
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
        false,
    ))
}

fn quarantine_row(
    transaction: &Transaction<'_>,
    table: &str,
    node_id: &str,
    kind: HealthKind,
    now: i64,
) -> Result<(), RegistryError> {
    let statement = match table {
        "health_profiles" => "DELETE FROM health_profiles WHERE node_id = ?1",
        "health_pulses" => "DELETE FROM health_pulses WHERE node_id = ?1",
        other => {
            return Err(RegistryError::Corrupt(format!(
                "unknown health table {other:?}"
            )))
        }
    };
    transaction.execute(statement, params![node_id])?;
    record_health_audit_tx(
        transaction,
        "corrupt_row",
        node_id,
        kind.wire(),
        0,
        "rejected",
        Some(HealthCode::CorruptState.code()),
        now,
    )?;
    Ok(())
}

type CorruptHealthRow = (&'static str, String, HealthKind);

fn cleanup_corrupt_health_rows(
    registry: &NodeRegistry,
    corrupt: &[CorruptHealthRow],
    now: i64,
) -> Result<(), RegistryError> {
    if corrupt.is_empty() {
        return Ok(());
    }
    registry.with_connection(|connection| {
        let transaction =
            connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (table, node_id, kind) in corrupt {
            quarantine_row(&transaction, table, node_id, *kind, now)?;
        }
        transaction.commit()?;
        Ok(())
    })
}

type StoredSignalRow = (Vec<u8>, i64, String, i64, Option<String>, Option<String>);

fn signal_from_row(row: &StoredSignalRow) -> Result<SignalRecord, RegistryError> {
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
        signal_id: row.0.iter().map(|byte| format!("{byte:02x}")).collect(),
        subject: row.4.clone(),
    })
}

fn decode_opaque_id(value: &str) -> Result<Vec<u8>, RegistryError> {
    if value.len() != 32 {
        return Err(RegistryError::InvalidInput(
            "health opaque identifier must be 32 hexadecimal characters".to_string(),
        ));
    }
    decode_hex(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_plane::bounds::{
        MAX_AGE_SECONDS, MAX_FUTURE_SKEW_SECONDS, MAX_STORED_PROFILE_BYTES, MAX_STORED_PULSE_BYTES,
        MAX_STORED_SIGNAL_BYTES, STORAGE_CEILING_BYTES, WORST_CASE_BYTES_PER_PERFORMER,
    };
    use crate::health_plane::model::{HealthBody, HealthPayload};
    use crate::node::{NodeContext, NodePathOverrides, NodePlatform};
    use crate::node_identity::{node_id_for_x_only_public_key, NodeIdentity};
    use crate::node_registry::{PeerRegistration, PeerSource};
    use rusqlite::{Connection, TransactionBehavior};
    use std::sync::Arc;
    use tempfile::TempDir;

    const BASE_NOW: i64 = 1_700_000_000;

    struct Fixture {
        _temp: TempDir,
        context: NodeContext,
        registry: NodeRegistry,
    }

    fn fixture() -> Fixture {
        let temp = TempDir::new().unwrap();
        let context = NodeContext::resolve_for(
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
        .unwrap();
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
        Fixture {
            _temp: temp,
            context,
            registry,
        }
    }

    fn reopen(fixture: &Fixture) -> NodeRegistry {
        let identity = NodeIdentity::load_existing(&fixture.context).unwrap();
        NodeRegistry::open(&fixture.context, identity.public_status()).unwrap()
    }

    fn scalar(seed: u32) -> [u8; 32] {
        let mut value = [0_u8; 32];
        value[28..].copy_from_slice(&seed.saturating_add(1).to_be_bytes());
        value
    }

    fn peer_identity(seed: u32) -> (String, String) {
        let key = k256::schnorr::SigningKey::from_slice(&scalar(seed)).unwrap();
        let xonly = key.verifying_key().to_bytes();
        let public_key = xonly.iter().map(|byte| format!("{byte:02x}")).collect();
        (node_id_for_x_only_public_key(&xonly), public_key)
    }

    fn trust(registry: &NodeRegistry, seed: u32, role: PeerRole, capabilities: &[&str]) -> String {
        let (node_id, public_key) = peer_identity(seed);
        registry
            .import_manual_peer(PeerRegistration {
                node_id: node_id.clone(),
                public_key,
                role,
                capabilities: capabilities.iter().map(|entry| entry.to_string()).collect(),
                source: PeerSource::Manual,
                actor: "health-plane-tests".to_string(),
                reason: "health plane storage test peer".to_string(),
            })
            .unwrap();
        node_id
    }

    fn performer(registry: &NodeRegistry) -> String {
        trust(
            registry,
            1,
            PeerRole::Performer,
            &["inventory-health", "notifications"],
        )
    }

    fn hex16(seed: u64) -> String {
        format!("{seed:032x}")
    }

    fn profile(target: &str, message_seed: u64, revision: u64) -> HealthPayload {
        HealthPayload {
            message_id: hex16(message_seed),
            target: target.to_string(),
            body: HealthBody::Profile(ProfileSnapshot {
                agent_version: "0.3.0".to_string(),
                arch: "x86_64".to_string(),
                baseline_id: String::new(),
                baseline_observed_id: String::new(),
                capabilities: vec!["inventory-health".to_string(), "notifications".to_string()],
                display_name: "workshop-laptop".to_string(),
                distro_id: "arch".to_string(),
                distro_version: "rolling".to_string(),
                omarchy_channel: "stable".to_string(),
                omarchy_version: "2.1.0".to_string(),
                platform: "linux".to_string(),
                profile_revision: revision,
                role: "performer".to_string(),
                runtimes: vec![RuntimeFact {
                    available: true,
                    name: "bash".to_string(),
                    version: "5.2.37".to_string(),
                }],
            }),
        }
    }

    fn pulse(target: &str, message_seed: u64, sequence: u64, emitted_at: i64) -> HealthPayload {
        HealthPayload {
            message_id: hex16(message_seed),
            target: target.to_string(),
            body: HealthBody::Pulse(PulseSnapshot {
                emitted_at,
                last_run: None,
                profile_revision: 1,
                runner: RunnerFact {
                    queue_depth: 0,
                    scheduler: "running".to_string(),
                    state: "idle".to_string(),
                    workers_busy: 0,
                    workers_configured: 1,
                },
                sequence,
                uptime_seconds: 3600,
            }),
        }
    }

    fn signal(
        target: &str,
        message_seed: u64,
        sequence: u64,
        signal_seed: u64,
        occurred_at: i64,
    ) -> HealthPayload {
        HealthPayload {
            message_id: hex16(message_seed),
            target: target.to_string(),
            body: HealthBody::Signal(SignalRecord {
                kind: SignalKind::RunCompleted,
                occurred_at,
                run: Some(RunFact {
                    exit_code: Some(0),
                    finished_at: occurred_at,
                    run_id: hex16(signal_seed + 900_000),
                    script: "deploy".to_string(),
                    started_at: None,
                    state: "completed".to_string(),
                    trigger: None,
                }),
                sequence,
                signal_id: hex16(signal_seed),
                subject: None,
            }),
        }
    }

    fn apply(
        registry: &NodeRegistry,
        sender: &str,
        payload: &HealthPayload,
        now: i64,
    ) -> HealthDecision {
        apply_at(registry, sender, payload, now, now)
    }

    fn apply_at(
        registry: &NodeRegistry,
        sender: &str,
        payload: &HealthPayload,
        created_at: i64,
        now: i64,
    ) -> HealthDecision {
        let bytes = match payload.body.kind() {
            HealthKind::Profile => 1_327,
            HealthKind::Pulse => 926,
            HealthKind::Signal => 777,
            _ => 510,
        };
        registry
            .apply_health_message(HealthApplyRequest {
                sender,
                payload,
                created_at,
                now,
                message_bytes: bytes,
            })
            .unwrap()
    }

    fn accepted(cursor: u64) -> HealthDecision {
        HealthDecision::Accepted { cursor }
    }

    #[test]
    fn pure_health_reads_succeed_while_writer_is_reserved() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let mut writer = Connection::open(fixture.registry.path()).unwrap();
        super::super::configure_connection(&mut writer).unwrap();
        let writer_transaction = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();

        assert!(fixture
            .registry
            .health_fleet_snapshot(BASE_NOW)
            .is_ok());
        assert!(fixture
            .registry
            .health_node_snapshot(&node_id, BASE_NOW)
            .is_ok());
        assert!(fixture
            .registry
            .health_signal_feed(16, BASE_NOW)
            .is_ok());

        writer_transaction.rollback().unwrap();
    }

    #[test]
    fn migration_creates_bounded_tables_and_leaves_earlier_rows_untouched() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let connection = Connection::open(fixture.registry.path()).unwrap();

        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let marker: String = connection
            .query_row(
                "SELECT value FROM metadata WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marker, "8");
        for (object_type, name) in super::super::HEALTH_PLANE_OBJECTS {
            let present: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                    params![object_type, name],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(present, 1, "missing {object_type} {name}");
        }
        // Every Health Plane table starts empty and the trust rows created
        // before the plane existed are still exactly as they were.
        for table in [
            "health_peers",
            "health_profiles",
            "health_pulses",
            "health_signals",
            "health_outbox",
            "health_replay_keys",
            "health_audit",
        ] {
            let rows: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 0, "{table} must start empty");
        }
        let trust_state: String = connection
            .query_row(
                "SELECT state FROM trusted_peers WHERE node_id = ?1",
                params![node_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(trust_state, "active");
        assert!(fixture.registry.health_plane_enabled().unwrap());
    }

    #[test]
    fn authorization_projection_reports_role_and_capabilities_without_mutating() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let before = fixture.registry.audit_events().unwrap().len();

        let authorization = fixture
            .registry
            .health_authorization(&node_id)
            .unwrap()
            .expect("projection");
        assert_eq!(authorization.state, PeerState::Active);
        assert_eq!(authorization.role, PeerRole::Performer);
        assert_eq!(
            authorization.capabilities,
            vec!["inventory-health".to_string(), "notifications".to_string()]
        );
        // Reading authorization creates nothing, not even an audit row.
        assert_eq!(fixture.registry.audit_events().unwrap().len(), before);
        assert!(fixture.registry.health_peer_states().unwrap().is_empty());

        let (unknown, _) = peer_identity(77);
        assert!(fixture
            .registry
            .health_authorization(&unknown)
            .unwrap()
            .is_none());
    }

    #[test]
    fn profile_and_pulse_keep_exactly_one_latest_row_per_peer() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &profile(&local, 1, 1),
                BASE_NOW
            ),
            accepted(0)
        );
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &profile(&local, 2, 2),
                BASE_NOW + 5
            ),
            accepted(0)
        );
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &pulse(&local, 3, 1, BASE_NOW + 10),
                BASE_NOW + 10
            ),
            accepted(0)
        );
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &pulse(&local, 4, 2, BASE_NOW + 30),
                BASE_NOW + 30
            ),
            accepted(0)
        );

        let connection = Connection::open(fixture.registry.path()).unwrap();
        for table in ["health_profiles", "health_pulses"] {
            let rows: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 1, "{table} retains exactly the latest row");
        }
        let snapshot = fixture
            .registry
            .health_peer_snapshot(&node_id, BASE_NOW + 30)
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.profile.unwrap().profile_revision, 2);
        assert_eq!(snapshot.pulse.unwrap().sequence, 2);
        assert_eq!(snapshot.state.last_pulse_at, Some(BASE_NOW + 30));
    }

    #[test]
    fn duplicates_replays_and_regressions_are_rejected_without_mutation() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 1, 2),
            BASE_NOW,
        );
        // Same message ID.
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &profile(&local, 1, 3),
                BASE_NOW + 1
            ),
            HealthDecision::Rejected(HealthCode::Replay)
        );
        // Fresh message ID but an equal or lower revision.
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &profile(&local, 2, 2),
                BASE_NOW + 2
            ),
            HealthDecision::Rejected(HealthCode::Replay)
        );
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &profile(&local, 3, 1),
                BASE_NOW + 3
            ),
            HealthDecision::Rejected(HealthCode::Replay)
        );
        let snapshot = fixture
            .registry
            .health_peer_snapshot(&node_id, BASE_NOW + 3)
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.profile.unwrap().profile_revision, 2);

        apply(
            &fixture.registry,
            &node_id,
            &pulse(&local, 10, 5, BASE_NOW + 20),
            BASE_NOW + 20,
        );
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &pulse(&local, 11, 5, BASE_NOW + 40),
                BASE_NOW + 40
            ),
            HealthDecision::Rejected(HealthCode::Replay)
        );
    }

    #[test]
    fn signal_cursor_accepts_in_order_holds_gaps_and_refuses_far_future() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &signal(&local, 1, 1, 101, BASE_NOW),
                BASE_NOW
            ),
            accepted(1)
        );
        // A gap inside the 32-entry reorder window is held; the cursor stalls.
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &signal(&local, 2, 3, 103, BASE_NOW + 10),
                BASE_NOW + 10
            ),
            HealthDecision::Held { cursor: 1 }
        );
        // Beyond the window it is refused outright and never buffered.
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &signal(&local, 3, 40, 140, BASE_NOW + 20),
                BASE_NOW + 20
            ),
            HealthDecision::Rejected(HealthCode::Reordered)
        );
        // Filling the gap advances the cursor across the promoted Signal.
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &signal(&local, 4, 2, 102, BASE_NOW + 30),
                BASE_NOW + 30
            ),
            accepted(3)
        );
        let signals = fixture
            .registry
            .health_signals(&node_id, 64, BASE_NOW + 30)
            .unwrap();
        assert_eq!(
            signals
                .iter()
                .map(|entry| entry.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        // A held Signal that never fills its gap expires without moving the cursor.
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &signal(&local, 5, 6, 106, BASE_NOW + 40),
                BASE_NOW + 40
            ),
            HealthDecision::Held { cursor: 3 }
        );
        let report = fixture.registry.health_prune(BASE_NOW + 200).unwrap();
        assert_eq!(report.expired_held_signals, 1);
        assert_eq!(
            fixture
                .registry
                .health_peer_states()
                .unwrap()
                .first()
                .unwrap()
                .cursor,
            3
        );
    }

    #[test]
    fn signal_identity_is_idempotent_across_fresh_message_ids() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &signal(&local, 1, 1, 101, BASE_NOW),
                BASE_NOW
            ),
            accepted(1)
        );
        // A resend reuses signal_id and sequence with a fresh message_id.
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &signal(&local, 2, 1, 101, BASE_NOW + 10),
                BASE_NOW + 10
            ),
            HealthDecision::Rejected(HealthCode::Replay)
        );
        // A different sequence carrying an already stored signal_id is also a replay.
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &signal(&local, 3, 2, 101, BASE_NOW + 20),
                BASE_NOW + 20
            ),
            HealthDecision::Rejected(HealthCode::Replay)
        );
        assert_eq!(
            fixture
                .registry
                .health_signals(&node_id, 64, BASE_NOW + 20)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn signal_inbox_and_storage_stay_inside_their_frozen_bounds_under_flood() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        let mut now = BASE_NOW;
        let mut accepted_count = 0_u64;
        let mut queue_full = 0_u64;
        for index in 1..=100_u64 {
            now += 7;
            let decision = apply(
                &fixture.registry,
                &node_id,
                &signal(&local, 1_000 + index, index, 2_000 + index, now),
                now,
            );
            match decision {
                HealthDecision::Accepted { .. } => accepted_count += 1,
                HealthDecision::Rejected(HealthCode::QueueFull) => queue_full += 1,
                // Once the inbox is full the cursor stalls, so later sequences
                // eventually leave the 32-entry reorder window. Both outcomes
                // are bounded rejections that store nothing.
                HealthDecision::Rejected(HealthCode::RateLimited)
                | HealthDecision::Rejected(HealthCode::Reordered) => {}
                other => panic!("unexpected decision {other:?}"),
            }
        }
        assert_eq!(accepted_count, SIGNAL_INBOX_CAPACITY as u64);
        assert!(queue_full > 0, "the inbox bound must reject the overflow");
        let connection = Connection::open(fixture.registry.path()).unwrap();
        let stored: i64 = connection
            .query_row("SELECT COUNT(*) FROM health_signals", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, SIGNAL_INBOX_CAPACITY);

        let peers = fixture.registry.health_peer_states().unwrap().len() as i64;
        let bytes = fixture.registry.health_storage_bytes().unwrap();
        assert!(bytes <= peers * WORST_CASE_BYTES_PER_PERFORMER + MAX_AUDIT_ROWS * AUDIT_ROW_BYTES);
        assert!(bytes < STORAGE_CEILING_BYTES);
    }

    #[test]
    fn frozen_per_row_caps_multiply_out_to_the_frozen_ceiling() {
        assert_eq!(
            MAX_STORED_PROFILE_BYTES
                + MAX_STORED_PULSE_BYTES
                + SIGNAL_INBOX_CAPACITY * MAX_STORED_SIGNAL_BYTES,
            WORST_CASE_BYTES_PER_PERFORMER
        );
        assert_eq!(
            MAX_PERFORMERS_PER_CONDUCTOR * WORST_CASE_BYTES_PER_PERFORMER
                + MAX_REPLAY_ROWS * REPLAY_ROW_BYTES
                + MAX_AUDIT_ROWS * AUDIT_ROW_BYTES,
            STORAGE_CEILING_BYTES
        );
    }

    #[test]
    fn rate_limits_bound_a_flood_from_one_peer() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        // Profiles are capped per hour.
        let mut profiles = 0;
        for index in 1..=30_u64 {
            match apply(
                &fixture.registry,
                &node_id,
                &profile(&local, index, index),
                BASE_NOW,
            ) {
                HealthDecision::Accepted { .. } => profiles += 1,
                HealthDecision::Rejected(HealthCode::RateLimited) => {}
                other => panic!("unexpected decision {other:?}"),
            }
        }
        assert_eq!(profiles, MAX_PROFILES_PER_PEER_PER_HOUR);

        // Pulses honour the minimum accepted interval.
        let fixture = super::tests::fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &pulse(&local, 1, 1, BASE_NOW),
                BASE_NOW
            ),
            accepted(0)
        );
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &pulse(&local, 2, 2, BASE_NOW + 9),
                BASE_NOW + 9
            ),
            HealthDecision::Rejected(HealthCode::RateLimited)
        );
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &pulse(&local, 3, 2, BASE_NOW + 10),
                BASE_NOW + 10
            ),
            accepted(0)
        );
    }

    #[test]
    fn freshness_boundaries_are_inclusive_exactly_where_the_contract_says() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        let now = BASE_NOW + 1_000;
        assert_eq!(
            apply_at(
                &fixture.registry,
                &node_id,
                &profile(&local, 1, 1),
                now - MAX_AGE_SECONDS,
                now
            ),
            accepted(0)
        );
        assert_eq!(
            apply_at(
                &fixture.registry,
                &node_id,
                &profile(&local, 2, 2),
                now - MAX_AGE_SECONDS - 1,
                now
            ),
            HealthDecision::Rejected(HealthCode::Stale)
        );
        assert_eq!(
            apply_at(
                &fixture.registry,
                &node_id,
                &profile(&local, 3, 3),
                now + MAX_FUTURE_SKEW_SECONDS,
                now
            ),
            accepted(0)
        );
        assert_eq!(
            apply_at(
                &fixture.registry,
                &node_id,
                &profile(&local, 4, 4),
                now + MAX_FUTURE_SKEW_SECONDS + 1,
                now
            ),
            HealthDecision::Rejected(HealthCode::Future)
        );
    }

    #[test]
    fn unauthorized_and_revoked_peers_cannot_mutate_any_health_state() {
        let fixture = fixture();
        let local = fixture.registry.local_node_id().to_string();
        let performer = performer(&fixture.registry);
        let limited = trust(&fixture.registry, 2, PeerRole::Performer, &["remote-run"]);
        let conductor = trust(&fixture.registry, 3, PeerRole::Conductor, &[]);
        let (stranger, _) = peer_identity(9);

        // A peer with no registry row at all.
        assert_eq!(
            apply(
                &fixture.registry,
                &stranger,
                &profile(&local, 1, 1),
                BASE_NOW
            ),
            HealthDecision::Rejected(HealthCode::Revoked)
        );
        // A trusted peer without the required capability.
        assert_eq!(
            apply(
                &fixture.registry,
                &limited,
                &profile(&local, 2, 1),
                BASE_NOW
            ),
            HealthDecision::Rejected(HealthCode::MissingCapability)
        );
        // A Conductor cannot report health.
        assert_eq!(
            apply(
                &fixture.registry,
                &conductor,
                &profile(&local, 3, 1),
                BASE_NOW
            ),
            HealthDecision::Rejected(HealthCode::WrongRole)
        );
        // A Performer cannot acknowledge.
        let ack = HealthPayload {
            message_id: hex16(4),
            target: local.clone(),
            body: HealthBody::Ack(crate::health_plane::model::AckBody {
                accepted: true,
                acked_message_id: hex16(1),
                cursor: 0,
            }),
        };
        assert_eq!(
            apply(&fixture.registry, &performer, &ack, BASE_NOW),
            HealthDecision::Rejected(HealthCode::WrongRole)
        );

        assert!(fixture.registry.health_peer_states().unwrap().is_empty());
        let connection = Connection::open(fixture.registry.path()).unwrap();
        for table in ["health_profiles", "health_pulses", "health_signals"] {
            let rows: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 0, "{table} must stay empty");
        }
        // Trust rows are unchanged: nothing was created and nothing reactivated.
        let trusted: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM trusted_peers WHERE state = 'active'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(trusted, 3);
    }

    #[test]
    fn revocation_stops_ingest_and_purges_derived_state_without_touching_trust() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 1, 1),
            BASE_NOW,
        );
        apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 2, 1, 101, BASE_NOW + 5),
            BASE_NOW + 5,
        );
        assert_eq!(fixture.registry.health_peer_states().unwrap().len(), 1);

        fixture
            .registry
            .revoke_peer(&node_id, "operator", "lost device")
            .unwrap();

        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &profile(&local, 3, 2),
                BASE_NOW + 10
            ),
            HealthDecision::Rejected(HealthCode::Revoked)
        );
        let purged = fixture
            .registry
            .health_purge_revoked(BASE_NOW + 20)
            .unwrap();
        assert_eq!(purged, vec![node_id.clone()]);
        assert!(fixture.registry.health_peer_states().unwrap().is_empty());

        let connection = Connection::open(fixture.registry.path()).unwrap();
        for table in ["health_profiles", "health_pulses", "health_signals"] {
            let rows: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(rows, 0, "{table} must be purged");
        }
        // The revocation itself is retained: health cleanup never rewrites trust.
        let revocations: i64 = connection
            .query_row("SELECT COUNT(*) FROM revocations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(revocations, 1);
        let trust_state: String = connection
            .query_row(
                "SELECT state FROM trusted_peers WHERE node_id = ?1",
                params![node_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(trust_state, "revoked");
    }

    #[test]
    fn the_two_hundred_fifty_seventh_peer_is_refused_without_changing_trust() {
        let fixture = fixture();
        let local = fixture.registry.local_node_id().to_string();
        let mut peers = Vec::new();
        for seed in 0..(MAX_PERFORMERS_PER_CONDUCTOR as u32 + 1) {
            peers.push(trust(
                &fixture.registry,
                seed,
                PeerRole::Performer,
                &["inventory-health"],
            ));
        }
        for (index, node_id) in peers.iter().take(peers.len() - 1).enumerate() {
            assert_eq!(
                apply(
                    &fixture.registry,
                    node_id,
                    &profile(&local, 10_000 + index as u64, 1),
                    BASE_NOW
                ),
                accepted(0)
            );
        }
        assert_eq!(
            fixture.registry.health_peer_states().unwrap().len(),
            MAX_PERFORMERS_PER_CONDUCTOR as usize
        );
        assert_eq!(
            apply(
                &fixture.registry,
                peers.last().unwrap(),
                &profile(&local, 99_999, 1),
                BASE_NOW
            ),
            HealthDecision::Rejected(HealthCode::QueueFull)
        );
        assert_eq!(
            fixture.registry.health_peer_states().unwrap().len(),
            MAX_PERFORMERS_PER_CONDUCTOR as usize
        );
    }

    #[test]
    fn pruning_enforces_signal_replay_and_audit_retention() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 1, 1, 101, BASE_NOW),
            BASE_NOW,
        );
        let later = BASE_NOW + SIGNAL_RETENTION_SECONDS + 1;
        let report = fixture.registry.health_prune(later).unwrap();
        assert_eq!(report.expired_signals, 1);
        assert!(report.expired_replay_keys >= 1);
        assert!(fixture
            .registry
            .health_signals(&node_id, 64, later)
            .unwrap()
            .is_empty());

        // The replay security floor keeps young keys even past their retention.
        apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 2, 2, 102, later),
            later,
        );
        let connection = Connection::open(fixture.registry.path()).unwrap();
        connection
            .execute(
                "UPDATE health_replay_keys SET expires_at = first_seen + 1",
                [],
            )
            .unwrap();
        let report = fixture
            .registry
            .health_prune(later + REPLAY_SECURITY_FLOOR_SECONDS - 1)
            .unwrap();
        assert_eq!(report.expired_replay_keys, 0);
        let report = fixture
            .registry
            .health_prune(later + REPLAY_SECURITY_FLOOR_SECONDS)
            .unwrap();
        assert_eq!(report.expired_replay_keys, 1);
    }

    #[test]
    fn version_incompatibility_expires_without_operator_action() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 1, 1),
            BASE_NOW,
        );
        fixture
            .registry
            .mark_health_version_incompatible(&node_id, BASE_NOW)
            .unwrap();
        assert!(
            fixture.registry.health_peer_states().unwrap()[0].version_incompatible,
            "the peer must be marked"
        );
        fixture
            .registry
            .health_prune(BASE_NOW + VERSION_INCOMPATIBLE_EXPIRY_SECONDS - 1)
            .unwrap();
        assert!(fixture.registry.health_peer_states().unwrap()[0].version_incompatible);
        fixture
            .registry
            .health_prune(BASE_NOW + VERSION_INCOMPATIBLE_EXPIRY_SECONDS)
            .unwrap();
        assert!(!fixture.registry.health_peer_states().unwrap()[0].version_incompatible);
    }

    #[test]
    fn outbox_is_bounded_drops_the_oldest_and_retires_on_acknowledgement() {
        let fixture = fixture();
        let conductor = trust(&fixture.registry, 5, PeerRole::Conductor, &[]);

        for index in 1..=(SIGNAL_OUTBOX_CAPACITY as u64 + 4) {
            fixture
                .registry
                .health_enqueue_signal(
                    &conductor,
                    &hex16(3_000 + index),
                    SignalKind::Enrolled,
                    BASE_NOW + index as i64,
                    Some(&conductor),
                    None,
                    777,
                    BASE_NOW + index as i64,
                )
                .unwrap();
        }
        let outbox = fixture.registry.health_outbox(64).unwrap();
        assert_eq!(outbox.len(), SIGNAL_OUTBOX_CAPACITY as usize);
        assert_eq!(fixture.registry.health_signals_dropped().unwrap(), 4);
        assert_eq!(outbox.first().unwrap().sequence, 5);

        // Re-queuing the same signal_id is refused; idempotency is by signal_id.
        assert!(matches!(
            fixture.registry.health_enqueue_signal(
                &conductor,
                &hex16(3_000 + SIGNAL_OUTBOX_CAPACITY as u64),
                SignalKind::Enrolled,
                BASE_NOW,
                Some(&conductor),
                None,
                777,
                BASE_NOW,
            ),
            Err(RegistryError::Duplicate(_))
        ));

        let target = outbox.first().unwrap().signal_id.clone();
        assert!(fixture
            .registry
            .health_mark_signal_sent(&target, &hex16(7_777), BASE_NOW + 100)
            .unwrap());
        let local = fixture.registry.local_node_id().to_string();
        let ack = HealthPayload {
            message_id: hex16(8_888),
            target: local,
            body: HealthBody::Ack(crate::health_plane::model::AckBody {
                accepted: true,
                acked_message_id: hex16(7_777),
                cursor: 5,
            }),
        };
        assert_eq!(
            apply(&fixture.registry, &conductor, &ack, BASE_NOW + 100),
            accepted(5)
        );
        let outbox = fixture.registry.health_outbox(64).unwrap();
        assert_eq!(outbox.len(), SIGNAL_OUTBOX_CAPACITY as usize - 1);
        assert!(outbox.iter().all(|entry| entry.signal_id != target));
    }

    #[test]
    fn state_and_cursor_survive_a_restart() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 1, 4),
            BASE_NOW,
        );
        apply(
            &fixture.registry,
            &node_id,
            &pulse(&local, 2, 9, BASE_NOW + 5),
            BASE_NOW + 5,
        );
        apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 3, 1, 101, BASE_NOW + 10),
            BASE_NOW + 10,
        );

        let reopened = reopen(&fixture);
        let state = &reopened.health_peer_states().unwrap()[0];
        assert_eq!(state.cursor, 1);
        assert_eq!(state.last_profile_revision, 4);
        assert_eq!(state.last_pulse_sequence, 9);
        assert_eq!(state.stored_signals, 1);
        // The replay key survives too, so a restart cannot reopen a replay window.
        assert_eq!(
            apply(&reopened, &node_id, &profile(&local, 1, 5), BASE_NOW + 20),
            HealthDecision::Rejected(HealthCode::Replay)
        );
    }

    #[test]
    fn a_failed_apply_rolls_back_completely_and_preserves_evidence() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        let connection = Connection::open(fixture.registry.path()).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER health_profiles_injected_failure
                 BEFORE INSERT ON health_profiles
                 BEGIN SELECT RAISE(ABORT, 'injected health storage failure'); END;",
            )
            .unwrap();

        assert!(fixture
            .registry
            .apply_health_message(HealthApplyRequest {
                sender: &node_id,
                payload: &profile(&local, 1, 1),
                created_at: BASE_NOW,
                now: BASE_NOW,
                message_bytes: 1_327,
            })
            .is_err());

        // Nothing partial survives: no peer row, no replay key, no audit row.
        assert!(fixture.registry.health_peer_states().unwrap().is_empty());
        let replays: i64 = connection
            .query_row("SELECT COUNT(*) FROM health_replay_keys", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(replays, 0);
        assert!(fixture.registry.health_audit_events(10).unwrap().is_empty());
        // Trust and identity evidence is untouched by the failure.
        let trusted: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM trusted_peers WHERE state = 'active'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(trusted, 1);

        connection
            .execute_batch("DROP TRIGGER health_profiles_injected_failure;")
            .unwrap();
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &profile(&local, 1, 1),
                BASE_NOW
            ),
            accepted(0)
        );
    }

    #[test]
    fn a_corrupt_health_row_is_quarantined_and_audited_without_disabling_the_peer() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 1, 1),
            BASE_NOW,
        );
        apply(
            &fixture.registry,
            &node_id,
            &pulse(&local, 2, 1, BASE_NOW + 5),
            BASE_NOW + 5,
        );
        let connection = Connection::open(fixture.registry.path()).unwrap();
        connection
            .execute("UPDATE health_profiles SET runtimes = 'not-json'", [])
            .unwrap();

        let snapshot = fixture
            .registry
            .health_peer_snapshot(&node_id, BASE_NOW + 10)
            .unwrap()
            .unwrap();
        assert!(snapshot.profile.is_none(), "the corrupt row is quarantined");
        assert!(snapshot.pulse.is_some(), "the healthy row still reads");
        let audit = fixture.registry.health_audit_events(10).unwrap();
        assert!(audit.iter().any(|event| {
            event.event_code == "corrupt_row"
                && event.error_code == Some(HealthCode::CorruptState.code())
        }));
        let rows: i64 = connection
            .query_row("SELECT COUNT(*) FROM health_profiles", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
        // The peer keeps reporting; only the single bad row was discarded.
        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &profile(&local, 3, 2),
                BASE_NOW + 20
            ),
            accepted(0)
        );
    }

    #[test]
    fn a_corrupt_signal_in_feed_is_quarantined_after_snapshot() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        assert_eq!(
            apply(
                &fixture.registry,
                &node_id,
                &signal(&local, 1, 1, 101, BASE_NOW),
                BASE_NOW,
            ),
            accepted(1)
        );
        let connection = Connection::open(fixture.registry.path()).unwrap();
        connection
            .execute("UPDATE health_signals SET run = 'not-json'", [])
            .unwrap();

        let feed = fixture
            .registry
            .health_signal_feed(16, BASE_NOW + 1)
            .unwrap();
        assert!(feed.signals.is_empty(), "the corrupt Signal is hidden");
        let rows: i64 = connection
            .query_row("SELECT COUNT(*) FROM health_signals", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
        let audit = fixture.registry.health_audit_events(10).unwrap();
        assert!(audit.iter().any(|event| {
            event.event_code == "corrupt_row"
                && event.error_code == Some(HealthCode::CorruptState.code())
        }));
    }

    #[test]
    fn concurrent_ingest_applies_each_message_exactly_once() {
        let fixture = Arc::new(fixture());
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        let mut handles = Vec::new();
        for index in 0..8_u64 {
            let fixture = Arc::clone(&fixture);
            let node_id = node_id.clone();
            let local = local.clone();
            handles.push(std::thread::spawn(move || {
                apply(
                    &fixture.registry,
                    &node_id,
                    &profile(&local, 500, 1 + index),
                    BASE_NOW,
                )
            }));
        }
        let decisions: Vec<HealthDecision> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        let accepted_count = decisions
            .iter()
            .filter(|decision| matches!(decision, HealthDecision::Accepted { .. }))
            .count();
        assert_eq!(
            accepted_count, 1,
            "one shared message_id may be applied exactly once"
        );
        let connection = Connection::open(fixture.registry.path()).unwrap();
        let replays: i64 = connection
            .query_row("SELECT COUNT(*) FROM health_replay_keys", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(replays, 1);
        let profiles: i64 = connection
            .query_row("SELECT COUNT(*) FROM health_profiles", [], |row| row.get(0))
            .unwrap();
        assert_eq!(profiles, 1);
    }

    #[test]
    fn audit_rows_carry_only_redacted_metadata() {
        let fixture = fixture();
        let node_id = performer(&fixture.registry);
        let local = fixture.registry.local_node_id().to_string();

        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 1, 1),
            BASE_NOW,
        );
        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 1, 2),
            BASE_NOW + 1,
        );
        let events = fixture.registry.health_audit_events(10).unwrap();
        assert_eq!(events.len(), 2);
        for event in &events {
            assert_eq!(event.node_id, node_id);
            assert_eq!(event.message_kind, "health_profile");
            assert!(event.byte_count > 0);
            assert!(matches!(event.outcome.as_str(), "accepted" | "rejected"));
            let rendered = format!("{event:?}");
            for forbidden in [
                "workshop-laptop",
                "arch",
                "rolling",
                "bash",
                "5.2.37",
                "secret://",
            ] {
                assert!(
                    !rendered.contains(forbidden),
                    "audit row leaked {forbidden:?}: {rendered}"
                );
            }
        }
        assert_eq!(
            events
                .iter()
                .find(|event| event.outcome == "rejected")
                .unwrap()
                .error_code,
            Some(HealthCode::Replay.code())
        );
    }
}

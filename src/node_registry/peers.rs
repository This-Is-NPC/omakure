use super::audit::{record_audit, AuditInput};
use super::error::RegistryError;
use super::fields::{
    capabilities_json, decode_hex, now_timestamp, timestamp_seconds, validate_actor_reason,
    validate_bounded_text, validate_capabilities, validate_node_id, validate_public_key,
    validate_registration, validate_timestamp,
};
use super::projection::{
    insert_v2_identity_projection, insert_v2_pending_transport_projection, project_v2_transition,
};
use super::types::{
    PeerCounts, PeerRecord, PeerRegistration, PeerRole, PeerSource, PeerState, RevocationRecord,
    TransportPeer,
};
use super::validate::sqlite_validation_error;
use super::{NodeRegistry, MAX_REASON_BYTES};
use crate::node_identity::node_id_for_x_only_public_key;
use rusqlite::{params, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde_json::Value;

impl NodeRegistry {
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
    pub(super) fn reject_publisher_conflict(&self, role: PeerRole) -> Result<(), RegistryError> {
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

    /// Insert only a pending peer.  Observation, discovery, endpoints, and
    /// matching identifiers have no API that can insert active trust.
    pub fn register_pending_with_transport(
        &self,
        registration: PeerRegistration,
        certificate: Option<&[u8]>,
    ) -> Result<PeerRecord, RegistryError> {
        validate_registration(&registration, &self.local_node_id, &self.local_public_key)?;
        let now = now_timestamp();
        self.with_mutating_connection(|connection| {
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

    /// Revoke a peer and retain its identity forever in `revocations`.
    pub fn revoke_peer(
        &self,
        node_id: &str,
        actor: &str,
        reason: &str,
    ) -> Result<PeerRecord, RegistryError> {
        self.transition_peer(node_id, PeerState::Revoked, actor, reason)
    }

    /// Explicitly move a peer to `target`.  The actor and reason are mandatory
    /// evidence; there is no implicit activation path.
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
        self.with_mutating_connection(|connection| {
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
                insert_revocation(&transaction, &current, &now, reason)?;
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
        self.with_mutating_connection(|connection| {
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

    pub fn peer(&self, node_id: &str) -> Result<Option<PeerRecord>, RegistryError> {
        validate_node_id(node_id)?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let peer = load_peer(&transaction, node_id)?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    pub fn peers(&self) -> Result<Vec<PeerRecord>, RegistryError> {
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
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
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
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
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
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

    /// Return the v2 projection for transport authorization. `peers` rows are
    /// intentionally not consulted by the runtime path.
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
                connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
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

pub(super) fn peer_exists(
    transaction: &Transaction<'_>,
    node_id: &str,
) -> Result<bool, RegistryError> {
    Ok(transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM peers WHERE node_id = ?1)",
        [node_id],
        |row| row.get::<_, i64>(0),
    )? != 0)
}

pub(super) fn public_key_exists(
    transaction: &Transaction<'_>,
    public_key: &str,
) -> Result<bool, RegistryError> {
    Ok(transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM peers WHERE public_key = ?1)",
        [public_key],
        |row| row.get::<_, i64>(0),
    )? != 0)
}

pub(super) fn reject_retained_revocation(
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
) -> Result<(), RegistryError> {
    validate_bounded_text("reason", reason, MAX_REASON_BYTES)?;
    transaction.execute(
        "INSERT INTO revocations (node_id, public_key, revoked_at, reason, replacement_node_id)
         VALUES (?1, ?2, ?3, ?4, NULL)",
        params![peer.node_id, peer.public_key, revoked_at, reason],
    )?;
    Ok(())
}

pub(super) fn load_peer(
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

pub(super) fn peer_from_row(row: &Row<'_>) -> rusqlite::Result<PeerRecord> {
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

pub(super) fn revocation_from_row(row: &Row<'_>) -> rusqlite::Result<RevocationRecord> {
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

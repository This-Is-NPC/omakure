use super::audit::{record_audit, record_enrollment_audit_tx, AuditInput};
use super::error::RegistryError;
use super::fields::{capabilities_json, digest, now_timestamp, validate_actor_reason};
use super::peers::{load_peer, peer_exists, public_key_exists, reject_retained_revocation};
use super::projection::insert_v2_trust_projection;
use super::types::{PeerRecord, PeerRegistration, PeerRole, PeerSource, PeerState};
use super::{
    NodeRegistry, MAX_BOOTSTRAP_PROOF_ROWS, MAX_BUNDLE_ACTIVATIONS_PER_MINUTE,
    MAX_ENROLLMENT_CLEANUP_ROWS, MAX_ENROLLMENT_REPLAY_ROWS,
};
use crate::enrollment::SignedEnrollmentBundle;
use crate::util::hex;
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingBootstrapCleanup {
    pub organization: String,
    pub token_hash: [u8; 32],
    pub nonce_hash: [u8; 32],
    pub bundle_id: [u8; 16],
}

impl NodeRegistry {
    pub(crate) fn bootstrap_proof_consumed(
        &self,
        organization: &str,
    ) -> Result<bool, RegistryError> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM bootstrap_proofs
                         WHERE target_node_id = ?1 AND organization = ?2 AND consumed_at IS NOT NULL
                     )",
                    params![&self.local_node_id, organization],
                    |row| row.get::<_, i64>(0),
                )
                .map(|value| value != 0)
                .map_err(RegistryError::from)
        })
    }

    pub(crate) fn pending_bootstrap_cleanups(
        &self,
        organization: &str,
        limit: usize,
    ) -> Result<Vec<PendingBootstrapCleanup>, RegistryError> {
        let query_limit = i64::try_from(limit.saturating_add(1))
            .map_err(|_| RegistryError::InvalidInput("cleanup limit is too large".to_string()))?;
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT organization, token_hash, nonce_hash, bundle_id
                 FROM bootstrap_proofs
                 WHERE target_node_id = ?1 AND organization = ?2
                   AND consumed_at IS NOT NULL AND cleanup_state = 'pending'
                 ORDER BY consumed_at, bundle_id
                 LIMIT ?3",
            )?;
            let rows = statement
                .query_map(
                    params![&self.local_node_id, organization, query_limit],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Vec<u8>>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                            row.get::<_, Vec<u8>>(3)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let rows = rows
                .into_iter()
                .map(|(organization, token_hash, nonce_hash, bundle_id)| {
                    Ok(PendingBootstrapCleanup {
                        organization,
                        token_hash: token_hash.try_into().map_err(|_| {
                            RegistryError::InvalidSchema(
                                "pending bootstrap token hash has invalid length".to_string(),
                            )
                        })?,
                        nonce_hash: nonce_hash.try_into().map_err(|_| {
                            RegistryError::InvalidSchema(
                                "pending bootstrap nonce hash has invalid length".to_string(),
                            )
                        })?,
                        bundle_id: bundle_id.try_into().map_err(|_| {
                            RegistryError::InvalidSchema(
                                "pending bootstrap bundle ID has invalid length".to_string(),
                            )
                        })?,
                    })
                })
                .collect::<Result<Vec<_>, RegistryError>>()?;
            if rows.len() > limit {
                return Err(RegistryError::InvalidSchema(
                    "too many pending bootstrap cleanups".to_string(),
                ));
            }
            Ok(rows)
        })
    }

    pub(crate) fn complete_bootstrap_cleanup(
        &self,
        cleanup: &PendingBootstrapCleanup,
        bundle_digest: Option<&[u8; 32]>,
    ) -> Result<(), RegistryError> {
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let updated = transaction.execute(
                "UPDATE bootstrap_proofs
                 SET cleanup_state = 'complete'
                 WHERE target_node_id = ?1 AND organization = ?2
                   AND token_hash = ?3 AND nonce_hash = ?4
                   AND bundle_id = ?5 AND consumed_at IS NOT NULL
                   AND cleanup_state = 'pending'",
                params![
                    &self.local_node_id,
                    &cleanup.organization,
                    cleanup.token_hash.as_slice(),
                    cleanup.nonce_hash.as_slice(),
                    cleanup.bundle_id.as_slice(),
                ],
            )?;
            if updated != 1 {
                return Err(RegistryError::InvalidSchema(
                    "pending bootstrap cleanup disappeared".to_string(),
                ));
            }
            record_enrollment_audit_tx(
                &transaction,
                "cleanup_completed",
                Some(&cleanup.bundle_id),
                bundle_digest,
                &self.local_node_id,
                "accepted",
                "bootstrap token cleanup completed",
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn activate_signed_bundle(
        &self,
        bundle: &SignedEnrollmentBundle,
        actor: &str,
        reason: &str,
        now: u64,
        token_hash: &[u8; 32],
        nonce_hash: &[u8; 32],
    ) -> Result<PeerRecord, RegistryError> {
        let bundle_bytes = bundle.encode();
        let bundle_digest = digest(&bundle_bytes);
        let result = self.activate_signed_bundle_transaction(
            bundle,
            actor,
            reason,
            now,
            token_hash,
            nonce_hash,
            &bundle_digest,
        );
        match result {
            Ok(peer) => Ok(peer),
            Err(error) => match self.record_enrollment_audit(
                bundle_failure_code(&error),
                Some(&bundle.bundle_id),
                Some(&bundle_digest),
                &bundle.subject_node_id,
                "rejected",
                bundle_failure_detail(&error),
            ) {
                Ok(()) => Err(error),
                Err(audit_error) => Err(audit_error),
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn activate_signed_bundle_transaction(
        &self,
        bundle: &SignedEnrollmentBundle,
        actor: &str,
        reason: &str,
        now: u64,
        token_hash: &[u8; 32],
        nonce_hash: &[u8; 32],
        bundle_digest: &[u8; 32],
    ) -> Result<PeerRecord, RegistryError> {
        if bundle.subject_node_id == self.local_node_id {
            return Err(RegistryError::SelfTrust);
        }
        validate_actor_reason(actor, reason)?;
        let role = PeerRole::from(bundle.role);
        let registration = PeerRegistration {
            node_id: bundle.subject_node_id.clone(),
            public_key: hex::encode(bundle.subject_xonly.as_slice()),
            role,
            capabilities: bundle.capabilities.clone(),
            source: PeerSource::Bundle,
            actor: actor.to_string(),
            reason: reason.to_string(),
        };
        let first_seen = i64::try_from(now)
            .map_err(|_| RegistryError::InvalidInput("enrollment timestamp is too large".into()))?;
        let replay_expiry = i64::try_from(bundle.replay_expiry())
            .map_err(|_| RegistryError::InvalidInput("enrollment expiry is too large".into()))?;
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            cleanup_enrollment_replays(&transaction, now)?;
            cleanup_bootstrap_proofs(&transaction, now)?;
            ensure_bundle_bootstrap_proof(
                &transaction,
                &self.local_node_id,
                &bundle.organization,
                token_hash,
                nonce_hash,
                replay_expiry,
            )?;
            ensure_bundle_replay_available(&transaction, &bundle.bundle_id)?;
            ensure_bundle_rate_limit(&transaction, first_seen)?;
            ensure_bundle_peer_available(self, &transaction, &registration)?;
            let timestamp = now_timestamp();
            transaction.execute(
                "INSERT INTO peers (node_id, public_key, role, state, capabilities_json, added_at, updated_at, last_seen, source)
                 VALUES (?1, ?2, ?3, 'active', ?4, ?5, ?5, NULL, 'bundle')",
                params![
                    registration.node_id,
                    registration.public_key,
                    registration.role.as_str(),
                    capabilities_json(&registration.capabilities)?,
                    timestamp,
                ],
            )?;
            insert_v2_trust_projection(
                &transaction,
                &registration,
                first_seen,
                Some(&bundle.subject_certificate),
            )?;
            transaction.execute(
                "INSERT INTO enrollment_replays (replay_kind, replay_id, expires_at, first_seen)
                 VALUES ('bundle', ?1, ?2, ?3)",
                params![&bundle.bundle_id[..], replay_expiry, first_seen],
            )?;
            transaction.execute(
                "UPDATE bootstrap_proofs
                 SET consumed_at = ?1, bundle_id = ?2, cleanup_state = 'pending'
                 WHERE target_node_id = ?3 AND organization = ?4
                   AND token_hash = ?5 AND nonce_hash = ?6 AND consumed_at IS NULL",
                params![
                    first_seen,
                    &bundle.bundle_id[..],
                    &self.local_node_id,
                    &bundle.organization,
                    token_hash.as_slice(),
                    nonce_hash.as_slice(),
                ],
            )?;
            record_audit(
                &transaction,
                AuditInput {
                    event_type: "enrollment_completed",
                    node_id: &registration.node_id,
                    from_state: None,
                    to_state: Some(PeerState::Active),
                    actor,
                    reason,
                    occurred_at: &timestamp,
                },
            )?;
            record_enrollment_audit_tx(
                &transaction,
                "bundle_completed",
                Some(&bundle.bundle_id),
                Some(bundle_digest),
                &registration.node_id,
                "accepted",
                "signed enrollment bundle activated trust",
            )?;
            let peer = load_peer(&transaction, &registration.node_id)?.ok_or_else(|| {
                RegistryError::Corrupt("signed enrollment peer disappeared".to_string())
            })?;
            transaction.commit()?;
            Ok(peer)
        })
    }
}

fn bundle_failure_code(error: &RegistryError) -> &'static str {
    match error {
        RegistryError::BundleReplay | RegistryError::BootstrapProofConsumed => "replay",
        RegistryError::ConductorConflict | RegistryError::BundleConflict => "concurrent",
        RegistryError::Revoked(_) => "revoked_authority",
        RegistryError::BundleCapacity => "capacity",
        RegistryError::BundleRateLimited => "rate_limited",
        _ => "rejected",
    }
}

fn bundle_failure_detail(error: &RegistryError) -> &'static str {
    match error {
        RegistryError::BundleReplay => "signed enrollment bundle was already consumed",
        RegistryError::BootstrapProofConsumed => "bootstrap proof was already consumed",
        RegistryError::ConductorConflict => "an active conductor already exists",
        RegistryError::PublisherConductorConflict => {
            "this node publishes baselines and cannot also conduct"
        }
        RegistryError::BundleConflict => "signed enrollment conflicts with existing trust state",
        RegistryError::Revoked(_) => "signed enrollment identity is retained as revoked",
        RegistryError::BundleCapacity => "signed enrollment capacity is exhausted",
        RegistryError::BundleRateLimited => "signed enrollment rate limit exceeded",
        _ => "signed enrollment activation was rejected",
    }
}

fn ensure_bundle_bootstrap_proof(
    transaction: &Transaction<'_>,
    local_node_id: &str,
    organization: &str,
    token_hash: &[u8; 32],
    nonce_hash: &[u8; 32],
    replay_expiry: i64,
) -> Result<(), RegistryError> {
    let proof_exists: Option<(Option<i64>, Option<Vec<u8>>)> = transaction
        .query_row(
            "SELECT consumed_at, bundle_id FROM bootstrap_proofs
             WHERE target_node_id = ?1 AND organization = ?2
               AND token_hash = ?3 AND nonce_hash = ?4",
            params![
                local_node_id,
                organization,
                token_hash.as_slice(),
                nonce_hash.as_slice(),
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if proof_exists
        .as_ref()
        .is_some_and(|(consumed_at, _)| consumed_at.is_some())
    {
        return Err(RegistryError::BootstrapProofConsumed);
    }
    if proof_exists.is_none() {
        let proof_count: i64 =
            transaction.query_row("SELECT COUNT(*) FROM bootstrap_proofs", [], |row| {
                row.get(0)
            })?;
        if proof_count >= MAX_BOOTSTRAP_PROOF_ROWS {
            return Err(RegistryError::BundleCapacity);
        }
        transaction.execute(
            "INSERT INTO bootstrap_proofs
             (target_node_id, organization, token_hash, nonce_hash, expires_at, consumed_at, bundle_id, cleanup_state)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, NULL)",
            params![
                local_node_id,
                organization,
                token_hash.as_slice(),
                nonce_hash.as_slice(),
                replay_expiry,
            ],
        )?;
    }
    Ok(())
}

fn ensure_bundle_replay_available(
    transaction: &Transaction<'_>,
    bundle_id: &[u8; 16],
) -> Result<(), RegistryError> {
    if transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM enrollment_replays
         WHERE replay_kind = 'bundle' AND replay_id = ?1)",
        [bundle_id.as_slice()],
        |row| row.get::<_, i64>(0),
    )? != 0
    {
        return Err(RegistryError::BundleReplay);
    }
    let replay_count: i64 =
        transaction.query_row("SELECT COUNT(*) FROM enrollment_replays", [], |row| {
            row.get(0)
        })?;
    if replay_count >= MAX_ENROLLMENT_REPLAY_ROWS {
        return Err(RegistryError::BundleCapacity);
    }
    Ok(())
}

fn ensure_bundle_rate_limit(
    transaction: &Transaction<'_>,
    first_seen: i64,
) -> Result<(), RegistryError> {
    let rate_floor = first_seen.saturating_sub(60);
    let recent_attempts: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM enrollment_audits
         WHERE event_code IN ('bundle_completed', 'concurrent', 'revoked_authority')
           AND occurred_at >= ?1",
        [rate_floor],
        |row| row.get(0),
    )?;
    if recent_attempts >= MAX_BUNDLE_ACTIVATIONS_PER_MINUTE {
        return Err(RegistryError::BundleRateLimited);
    }
    Ok(())
}

fn ensure_bundle_peer_available(
    registry: &NodeRegistry,
    transaction: &Transaction<'_>,
    registration: &PeerRegistration,
) -> Result<(), RegistryError> {
    reject_retained_revocation(transaction, &registration.node_id, &registration.public_key)?;
    if peer_exists(transaction, &registration.node_id)?
        || public_key_exists(transaction, &registration.public_key)?
    {
        return Err(RegistryError::BundleConflict);
    }
    registry.reject_publisher_conflict(registration.role)?;
    if registration.role == PeerRole::Conductor
        && transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM peers WHERE role = 'conductor' AND state = 'active')",
            [],
            |row| row.get::<_, i64>(0),
        )? != 0
    {
        return Err(RegistryError::ConductorConflict);
    }
    Ok(())
}

pub(super) fn cleanup_enrollment_replays(
    transaction: &Transaction<'_>,
    now: u64,
) -> Result<(), RegistryError> {
    let now = i64::try_from(now)
        .map_err(|_| RegistryError::InvalidInput("enrollment timestamp is too large".into()))?;
    transaction.execute(
        "DELETE FROM enrollment_replays
         WHERE rowid IN (
           SELECT rowid FROM enrollment_replays
           WHERE replay_kind IN ('manual_request', 'bundle') AND expires_at <= ?1
           ORDER BY expires_at LIMIT ?2
         )",
        params![now, MAX_ENROLLMENT_CLEANUP_ROWS],
    )?;
    Ok(())
}

fn cleanup_bootstrap_proofs(transaction: &Transaction<'_>, now: u64) -> Result<(), RegistryError> {
    let now = i64::try_from(now)
        .map_err(|_| RegistryError::InvalidInput("enrollment timestamp is too large".into()))?;
    transaction.execute(
        "DELETE FROM bootstrap_proofs
         WHERE rowid IN (
           SELECT rowid FROM bootstrap_proofs
           WHERE consumed_at IS NULL AND expires_at <= ?1
           ORDER BY expires_at LIMIT ?2
         )",
        params![now, MAX_ENROLLMENT_CLEANUP_ROWS],
    )?;
    Ok(())
}

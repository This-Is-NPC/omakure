use super::audit::{record_audit, record_enrollment_audit_tx, AuditInput};
use super::bundle::cleanup_enrollment_replays;
use super::error::RegistryError;
use super::fields::{
    capabilities_json, decode_hex, digest, now_timestamp, timestamp_seconds, validate_actor_reason,
    validate_registration,
};
use super::peers::{load_peer, peer_exists, public_key_exists, reject_retained_revocation};
use super::projection::{
    insert_v2_identity_projection, insert_v2_pending_transport_projection,
    insert_v2_trust_projection, project_v2_transition,
};
use super::types::{PeerRecord, PeerRegistration, PeerRole, PeerSource, PeerState};
use super::{NodeRegistry, MAX_ENROLLMENT_REPLAY_ROWS, MAX_ENROLLMENT_REQUEST_ROWS};
use crate::direct_transport::TransportCertificate;
use crate::enrollment::{self, ManualEnrollmentRequest};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};

type StagedManualEnrollment = (
    Option<Vec<u8>>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    i64,
    i64,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    i64,
    i64,
    i64,
    String,
    String,
);

struct ManualStageInput<'a> {
    request: &'a ManualEnrollmentRequest,
    registration: PeerRegistration,
    certificate: TransportCertificate,
    request_bytes: Vec<u8>,
    request_digest: [u8; 32],
    certificate_digest: [u8; 32],
    capabilities: Vec<u8>,
    actor: &'a str,
    reason: &'a str,
    now: u64,
    first_seen: i64,
    expires_at: i64,
}

impl NodeRegistry {
    pub fn stage_manual_enrollment(
        &self,
        request: &ManualEnrollmentRequest,
        certificate: &[u8],
        actor: &str,
        reason: &str,
        now: u64,
    ) -> Result<PeerRecord, RegistryError> {
        validate_manual_stage_request(self, request, now)?;
        validate_actor_reason(actor, reason)?;
        let registration = registration_from_manual(request, actor, reason)?;
        let certificate = TransportCertificate::from_bytes(certificate)
            .map_err(|_| RegistryError::InvalidInput("transport certificate is invalid".into()))?;
        certificate
            .verify_time(now)
            .map_err(|_| RegistryError::InvalidInput("transport certificate is expired".into()))?;
        if certificate.node_id() != registration.node_id
            || certificate.identity_key().as_slice()
                != decode_hex(&registration.public_key)?.as_slice()
            || certificate.transport_public() != &request.proposer_transport_x25519
        {
            return Err(RegistryError::InvalidInput(
                "transport certificate does not match manual enrollment identity".into(),
            ));
        }
        let first_seen = i64::try_from(now)
            .map_err(|_| RegistryError::InvalidInput("enrollment timestamp is too large".into()))?;
        let expires_at = i64::try_from(request.replay_expiry())
            .map_err(|_| RegistryError::InvalidInput("enrollment expiry is too large".into()))?;
        let request_bytes = request.encode();
        let request_digest = digest(&request_bytes);
        let certificate_digest = digest(certificate.as_bytes());
        let capabilities = capabilities_json(&registration.capabilities)?.into_bytes();
        let stage = ManualStageInput {
            request,
            registration,
            certificate,
            request_bytes,
            request_digest,
            certificate_digest,
            capabilities,
            actor,
            reason,
            now,
            first_seen,
            expires_at,
        };
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            validate_manual_stage_capacity(&transaction, &stage)?;
            validate_manual_stage_identity_available(self, &transaction, &stage)?;
            insert_manual_stage(&transaction, &stage)?;
            let peer = load_peer(&transaction, &stage.registration.node_id)?
                .ok_or_else(|| RegistryError::Corrupt("staged enrollment disappeared".into()))?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    pub fn approve_manual_enrollment(
        &self,
        request: &ManualEnrollmentRequest,
        certificate: &[u8],
        code: &[u8],
        actor: &str,
        reason: &str,
        now: u64,
    ) -> Result<PeerRecord, RegistryError> {
        if let Err(error) = request.verify(now) {
            self.record_enrollment_audit(
                if matches!(error, crate::enrollment::EnrollmentError::Expired) {
                    "expired"
                } else {
                    "malformed"
                },
                Some(&request.request_id),
                Some(&digest(&request.encode())),
                &request.proposer_node_id,
                "rejected",
                "approval request verification failed",
            )?;
            return Err(RegistryError::InvalidInput(error.to_string()));
        }
        if let Err(error) = request.verify_code(code) {
            let request_bytes = request.encode();
            let request_digest = digest(&request_bytes);
            self.record_enrollment_audit(
                "wrong_code",
                Some(&request.request_id),
                Some(&request_digest),
                &request.proposer_node_id,
                "rejected",
                "approval code did not match staged code hash",
            )?;
            return Err(RegistryError::InvalidInput(error.to_string()));
        }
        validate_actor_reason(actor, reason)?;
        let registration = registration_from_manual(request, actor, reason)?;
        let certificate = TransportCertificate::from_bytes(certificate)
            .map_err(|_| RegistryError::InvalidInput("transport certificate is invalid".into()))?;
        if certificate.node_id() != registration.node_id
            || certificate.identity_key().as_slice()
                != decode_hex(&registration.public_key)?.as_slice()
            || certificate.transport_public() != &request.proposer_transport_x25519
        {
            return Err(RegistryError::InvalidInput(
                "transport certificate does not match manual enrollment identity".into(),
            ));
        }
        certificate
            .verify_time(now)
            .map_err(|_| RegistryError::InvalidInput("transport certificate is expired".into()))?;
        let request_bytes = request.encode();
        let request_digest = digest(&request_bytes);
        let certificate_digest = digest(certificate.as_bytes());
        let now_timestamp = now_timestamp();
        let now_seconds = i64::try_from(now)
            .map_err(|_| RegistryError::InvalidInput("enrollment timestamp is too large".into()))?;
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = load_peer(&transaction, &registration.node_id)?
                .ok_or_else(|| RegistryError::NotFound(registration.node_id.clone()))?;
            let staged = load_staged_manual_enrollment(
                &transaction,
                &request.request_id,
                &registration.node_id,
            )?;
            ensure_staged_manual_enrollment_matches(
                &staged,
                request,
                &request_bytes,
                &request_digest,
                &certificate,
                &certificate_digest,
            )?;
            ensure_manual_peer_can_be_approved(&current, &registration)?;
            reject_retained_revocation(
                &transaction,
                &registration.node_id,
                &registration.public_key,
            )?;
            ensure_pending_transport_key(&transaction, &registration.node_id, &certificate)?;
            transaction.execute(
                "UPDATE peers SET state = 'active', updated_at = ?1 WHERE node_id = ?2 AND state = 'pending'",
                params![now_timestamp, registration.node_id],
            )?;
            transaction.execute(
                "UPDATE manual_enrollment_requests SET state = 'approved', resolved_at = ?1
                 WHERE request_id = ?2 AND state = 'pending'",
                params![now_seconds, &request.request_id[..]],
            )?;
            project_v2_transition(&transaction, &current, PeerState::Active, now_seconds)?;
            record_audit(
                &transaction,
                AuditInput {
                    event_type: "enrollment_approved",
                    node_id: &registration.node_id,
                    from_state: Some(PeerState::Pending),
                    to_state: Some(PeerState::Active),
                    actor,
                    reason,
                    occurred_at: &now_timestamp,
                },
            )?;
            record_enrollment_audit_tx(
                &transaction,
                "approved",
                Some(&request.request_id),
                Some(&request_digest),
                &registration.node_id,
                "approved",
                "manual enrollment activated after explicit approval",
            )?;
            let peer = load_peer(&transaction, &registration.node_id)?
                .ok_or_else(|| RegistryError::Corrupt("approved enrollment disappeared".into()))?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    pub fn reject_manual_enrollment(
        &self,
        node_id: &str,
        actor: &str,
        reason: &str,
    ) -> Result<PeerRecord, RegistryError> {
        validate_actor_reason(actor, reason)?;
        let now = now_timestamp();
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = load_peer(&transaction, node_id)?
                .ok_or_else(|| RegistryError::NotFound(node_id.to_string()))?;
            if current.source != PeerSource::Manual || current.state != PeerState::Pending {
                return Err(RegistryError::EnrollmentConflict);
            }
            let (request_id, request_digest): (Vec<u8>, Vec<u8>) = transaction.query_row(
                "SELECT request_id, request_digest
                 FROM manual_enrollment_requests
                 WHERE node_id = ?1 AND source = 'manual' AND state = 'pending'",
                [node_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let request_id: [u8; enrollment::REQUEST_ID_BYTES] =
                request_id.try_into().map_err(|_| {
                    RegistryError::Corrupt("manual request ID has invalid length".into())
                })?;
            let request_digest: [u8; 32] = request_digest.try_into().map_err(|_| {
                RegistryError::Corrupt("manual request digest has invalid length".into())
            })?;
            transaction.execute(
                "UPDATE peers SET state = 'suspended', updated_at = ?1
                 WHERE node_id = ?2 AND state = 'pending' AND source = 'manual'",
                params![now, node_id],
            )?;
            let changed = transaction.execute(
                "UPDATE manual_enrollment_requests SET state = 'rejected', resolved_at = ?1
                 WHERE node_id = ?2 AND source = 'manual' AND state = 'pending'",
                params![timestamp_seconds(&now)?, node_id],
            )?;
            if changed != 1 {
                return Err(RegistryError::EnrollmentConflict);
            }
            record_audit(
                &transaction,
                AuditInput {
                    event_type: "enrollment_rejected",
                    node_id,
                    from_state: Some(PeerState::Pending),
                    to_state: Some(PeerState::Suspended),
                    actor,
                    reason,
                    occurred_at: &now,
                },
            )?;
            record_enrollment_audit_tx(
                &transaction,
                "rejected",
                Some(&request_id),
                Some(&request_digest),
                node_id,
                "rejected",
                "manual enrollment request rejected by local operator",
            )?;
            let peer = load_peer(&transaction, node_id)?
                .ok_or_else(|| RegistryError::Corrupt("rejected enrollment disappeared".into()))?;
            transaction.commit()?;
            Ok(peer)
        })
    }

    /// Atomically import a manually approved peer as active trust. The
    /// operation is intentionally separate from observation/pending
    /// registration and records the approval evidence in the same transaction
    /// as the peer row.
    pub fn import_manual_peer_with_transport(
        &self,
        registration: PeerRegistration,
        certificate: Option<&[u8]>,
    ) -> Result<PeerRecord, RegistryError> {
        validate_registration(&registration, &self.local_node_id, &self.local_public_key)?;
        let now = now_timestamp();
        self.with_mutating_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            reject_retained_revocation(
                &transaction,
                &registration.node_id,
                &registration.public_key,
            )?;
            if peer_exists(&transaction, &registration.node_id)?
                || public_key_exists(&transaction, &registration.public_key)?
            {
                return Err(RegistryError::Duplicate(registration.node_id.clone()));
            }
            self.reject_publisher_conflict(registration.role)?;
            transaction.execute(
                "INSERT INTO peers (node_id, public_key, role, state, capabilities_json, added_at, updated_at, last_seen, source)
                 VALUES (?1, ?2, ?3, 'active', ?4, ?5, ?5, NULL, ?6)",
                params![
                    registration.node_id,
                    registration.public_key,
                    registration.role.as_str(),
                    capabilities_json(&registration.capabilities)?,
                    now,
                    registration.source.as_str(),
                ],
            )?;
            insert_v2_trust_projection(
                &transaction,
                &registration,
                timestamp_seconds(&now)?,
                certificate,
            )?;
            record_audit(
                &transaction,
                AuditInput {
                    event_type: "peer_trusted",
                    node_id: &registration.node_id,
                    from_state: None,
                    to_state: Some(PeerState::Active),
                    actor: &registration.actor,
                    reason: &registration.reason,
                    occurred_at: &now,
                },
            )?;
            let peer = load_peer(&transaction, &registration.node_id)?
                .ok_or_else(|| RegistryError::Corrupt("imported peer disappeared".to_string()))?;
            transaction.commit()?;
            Ok(peer)
        })
    }
}

fn validate_manual_stage_request(
    registry: &NodeRegistry,
    request: &ManualEnrollmentRequest,
    now: u64,
) -> Result<(), RegistryError> {
    if let Err(error) = request.verify(now) {
        registry.record_enrollment_audit(
            if matches!(error, crate::enrollment::EnrollmentError::Replay) {
                "replay"
            } else if matches!(error, crate::enrollment::EnrollmentError::Expired) {
                "expired"
            } else {
                "malformed"
            },
            Some(&request.request_id),
            None,
            &registry.local_node_id,
            "rejected",
            "request verification failed",
        )?;
        return Err(RegistryError::InvalidInput(error.to_string()));
    }
    if request.proposer_node_id == registry.local_node_id {
        registry.record_enrollment_audit(
            "self_request",
            Some(&request.request_id),
            Some(&digest(&request.encode())),
            &request.proposer_node_id,
            "rejected",
            "a node may stage only a remote enrollment request",
        )?;
        return Err(RegistryError::SelfTrust);
    }
    Ok(())
}

fn validate_manual_stage_capacity(
    transaction: &Transaction<'_>,
    stage: &ManualStageInput<'_>,
) -> Result<(), RegistryError> {
    let request = stage.request;
    let request_digest = &stage.request_digest;
    let registration = &stage.registration;
    let now = stage.now;
    cleanup_enrollment_replays(transaction, now)?;
    let replay_count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM enrollment_replays WHERE replay_kind = 'manual_request'",
        [],
        |row| row.get(0),
    )?;
    if replay_count >= MAX_ENROLLMENT_REPLAY_ROWS {
        return Err(RegistryError::EnrollmentCapacity);
    }
    let request_count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM manual_enrollment_requests WHERE state = 'pending'",
        [],
        |row| row.get(0),
    )?;
    if request_count >= MAX_ENROLLMENT_REQUEST_ROWS {
        return Err(RegistryError::EnrollmentCapacity);
    }
    if transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM enrollment_replays WHERE replay_kind = 'manual_request' AND replay_id = ?1)",
                [&request.request_id[..]],
                |row| row.get::<_, i64>(0),
            )? != 0 {
                record_enrollment_audit_tx(
                    transaction,
                    "replay",
                    Some(&request.request_id),
                    Some(request_digest),
                    &registration.node_id,
                    "rejected",
                    "request replay was already retained",
                )?;
                return Err(RegistryError::EnrollmentReplay);
            }
    Ok(())
}

fn validate_manual_stage_identity_available(
    registry: &NodeRegistry,
    transaction: &Transaction<'_>,
    stage: &ManualStageInput<'_>,
) -> Result<(), RegistryError> {
    let request = stage.request;
    let request_digest = &stage.request_digest;
    let registration = &stage.registration;
    reject_retained_revocation(transaction, &registration.node_id, &registration.public_key)?;
    if let Some((state, source)) = transaction
        .query_row(
            "SELECT state, source FROM peers WHERE node_id = ?1",
            [&registration.node_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
    {
        let (error, audit_kind) = match (source.as_str(), state.as_str()) {
            ("manual", "pending") => (RegistryError::EnrollmentReplay, "replay"),
            _ => (RegistryError::EnrollmentConflict, "concurrent"),
        };
        record_enrollment_audit_tx(
            transaction,
            audit_kind,
            Some(&request.request_id),
            Some(request_digest),
            &registration.node_id,
            "rejected",
            "request conflicts with existing state",
        )?;
        return Err(error);
    }
    if public_key_exists(transaction, &registration.public_key)? {
        record_enrollment_audit_tx(
            transaction,
            "concurrent",
            Some(&request.request_id),
            Some(request_digest),
            &registration.node_id,
            "rejected",
            "request identity conflicts with existing state",
        )?;
        return Err(RegistryError::EnrollmentConflict);
    }
    registry.reject_publisher_conflict(registration.role)?;
    Ok(())
}

fn insert_manual_stage(
    transaction: &Transaction<'_>,
    stage: &ManualStageInput<'_>,
) -> Result<(), RegistryError> {
    let request = stage.request;
    let registration = &stage.registration;
    let certificate = &stage.certificate;
    let request_bytes = &stage.request_bytes;
    let request_digest = &stage.request_digest;
    let certificate_digest = &stage.certificate_digest;
    let capabilities = &stage.capabilities;
    let actor = stage.actor;
    let reason = stage.reason;
    let first_seen = stage.first_seen;
    let expires_at = stage.expires_at;
    transaction.execute(
                "INSERT INTO peers (node_id, public_key, role, state, capabilities_json, added_at, updated_at, last_seen, source)
                 VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?5, NULL, 'manual')",
                params![
                    registration.node_id,
                    registration.public_key,
                    registration.role.as_str(),
                    capabilities_json(&registration.capabilities)?,
                    now_timestamp(),
                ],
            )?;
    insert_v2_identity_projection(
        transaction,
        registration,
        first_seen,
        "authenticated_untrusted",
    )?;
    insert_v2_pending_transport_projection(
        transaction,
        registration,
        first_seen,
        certificate.as_bytes(),
    )?;
    transaction.execute(
                "INSERT INTO manual_enrollment_requests
                 (pairing_id, request_id, request_bytes, request_digest, code_hash, node_id, identity_key,
                  transport_key, role, capabilities, request_created_at, request_expires_at,
                  certificate, certificate_digest, certificate_id, key_epoch, not_before, not_after,
                  state, source, staged_at, resolved_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                         'pending', 'manual', ?19, NULL)",
                params![
                    &request.pairing_id[..],
                    &request.request_id[..],
                    request_bytes,
                    &request_digest[..],
                    &request.code_hash[..],
                    &registration.node_id,
                    &request.proposer_xonly[..],
                    &request.proposer_transport_x25519[..],
                    request.role as u8,
                    capabilities,
                    i64::try_from(request.created_at).map_err(|_| RegistryError::InvalidInput("request timestamp is too large".into()))?,
                    i64::try_from(request.expires_at).map_err(|_| RegistryError::InvalidInput("request timestamp is too large".into()))?,
                    certificate.as_bytes(),
                    &certificate_digest[..],
                    certificate.certificate_id().as_slice(),
                    i64::try_from(certificate.key_epoch()).map_err(|_| RegistryError::InvalidInput("certificate epoch is too large".into()))?,
                    i64::try_from(certificate.not_before()).map_err(|_| RegistryError::InvalidInput("certificate timestamp is too large".into()))?,
                    i64::try_from(certificate.not_after()).map_err(|_| RegistryError::InvalidInput("certificate timestamp is too large".into()))?,
                    first_seen,
                ],
            )?;
    transaction.execute(
        "INSERT INTO enrollment_replays (replay_kind, replay_id, expires_at, first_seen)
                 VALUES ('manual_request', ?1, ?2, ?3)",
        params![&request.request_id[..], expires_at, first_seen],
    )?;
    record_audit(
        transaction,
        AuditInput {
            event_type: "enrollment_pending",
            node_id: &registration.node_id,
            from_state: None,
            to_state: Some(PeerState::Pending),
            actor,
            reason,
            occurred_at: &now_timestamp(),
        },
    )?;
    record_enrollment_audit_tx(
        transaction,
        "pending",
        Some(&request.request_id),
        Some(request_digest),
        &registration.node_id,
        "staged",
        "manual enrollment request staged for local approval",
    )?;
    Ok(())
}

fn load_staged_manual_enrollment(
    transaction: &Transaction<'_>,
    request_id: &[u8; 16],
    node_id: &str,
) -> Result<StagedManualEnrollment, RegistryError> {
    transaction
        .query_row(
            "SELECT pairing_id, request_bytes, request_digest, code_hash, identity_key, transport_key,
                    request_created_at, request_expires_at, certificate, certificate_digest,
                    certificate_id, key_epoch, not_before, not_after, state, source
             FROM manual_enrollment_requests WHERE request_id = ?1",
            [request_id.as_slice()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                    row.get(15)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| RegistryError::NotFound(node_id.to_string()))
}

fn ensure_staged_manual_enrollment_matches(
    staged: &StagedManualEnrollment,
    request: &ManualEnrollmentRequest,
    request_bytes: &[u8],
    request_digest: &[u8; 32],
    certificate: &TransportCertificate,
    certificate_digest: &[u8; 32],
) -> Result<(), RegistryError> {
    let staged_source = &staged.15;
    let staged_state = &staged.14;
    if staged_source.as_str() != "manual" || staged_state.as_str() != "pending" {
        return Err(RegistryError::EnrollmentConflict);
    }
    ensure_staged_request_fields_match(staged, request, request_bytes, request_digest)?;
    ensure_staged_certificate_fields_match(staged, certificate, certificate_digest)
}

fn ensure_staged_request_fields_match(
    staged: &StagedManualEnrollment,
    request: &ManualEnrollmentRequest,
    request_bytes: &[u8],
    request_digest: &[u8; 32],
) -> Result<(), RegistryError> {
    if staged.0.as_deref() != Some(request.pairing_id.as_slice()) {
        return Err(RegistryError::EnrollmentMismatch);
    }
    if staged.1.as_slice() != request_bytes {
        return Err(RegistryError::EnrollmentMismatch);
    }
    if staged.2.as_slice() != request_digest.as_slice() {
        return Err(RegistryError::EnrollmentMismatch);
    }
    if staged.3.as_slice() != request.code_hash.as_slice() {
        return Err(RegistryError::EnrollmentMismatch);
    }
    if staged.4.as_slice() != request.proposer_xonly.as_slice() {
        return Err(RegistryError::EnrollmentMismatch);
    }
    if staged.5.as_slice() != request.proposer_transport_x25519.as_slice() {
        return Err(RegistryError::EnrollmentMismatch);
    }
    if staged.6 != i64::try_from(request.created_at).unwrap_or_default() {
        return Err(RegistryError::EnrollmentMismatch);
    }
    if staged.7 != i64::try_from(request.expires_at).unwrap_or_default() {
        return Err(RegistryError::EnrollmentMismatch);
    }
    Ok(())
}

fn ensure_staged_certificate_fields_match(
    staged: &StagedManualEnrollment,
    certificate: &TransportCertificate,
    certificate_digest: &[u8; 32],
) -> Result<(), RegistryError> {
    if staged.8.as_slice() != certificate.as_bytes() {
        return Err(RegistryError::EnrollmentMismatch);
    }
    if staged.9.as_slice() != certificate_digest.as_slice() {
        return Err(RegistryError::EnrollmentMismatch);
    }
    if staged.10.as_slice() != certificate.certificate_id().as_slice() {
        return Err(RegistryError::EnrollmentMismatch);
    }
    if staged.11 != i64::try_from(certificate.key_epoch()).unwrap_or_default() {
        return Err(RegistryError::EnrollmentMismatch);
    }
    if staged.12 != i64::try_from(certificate.not_before()).unwrap_or_default() {
        return Err(RegistryError::EnrollmentMismatch);
    }
    if staged.13 != i64::try_from(certificate.not_after()).unwrap_or_default() {
        return Err(RegistryError::EnrollmentMismatch);
    }
    Ok(())
}

fn ensure_manual_peer_can_be_approved(
    current: &PeerRecord,
    registration: &PeerRegistration,
) -> Result<(), RegistryError> {
    if current.source != PeerSource::Manual {
        return Err(RegistryError::EnrollmentConflict);
    }
    if current.public_key != registration.public_key
        || current.role != registration.role
        || current.capabilities != registration.capabilities
    {
        return Err(RegistryError::InvalidInput(
            "manual enrollment request does not match pending identity".into(),
        ));
    }
    if current.state != PeerState::Pending {
        return Err(RegistryError::InvalidTransition {
            from: current.state,
            to: PeerState::Active,
        });
    }
    Ok(())
}

fn ensure_pending_transport_key(
    transaction: &Transaction<'_>,
    node_id: &str,
    certificate: &TransportCertificate,
) -> Result<(), RegistryError> {
    let pending_transport: Option<Vec<u8>> = transaction
        .query_row(
            "SELECT public_key FROM transport_key_epochs WHERE node_id = ?1 AND state = 'pending'",
            [node_id],
            |row| row.get(0),
        )
        .optional()?;
    if pending_transport.as_deref() != Some(certificate.transport_public().as_slice()) {
        return Err(RegistryError::InvalidInput(
            "manual enrollment transport key does not match pending identity".into(),
        ));
    }
    Ok(())
}

fn registration_from_manual(
    request: &ManualEnrollmentRequest,
    actor: &str,
    reason: &str,
) -> Result<PeerRegistration, RegistryError> {
    let role = PeerRole::from(request.role);
    Ok(PeerRegistration {
        node_id: request.proposer_node_id.clone(),
        public_key: request.public_key_hex(),
        role,
        capabilities: request.capabilities.clone(),
        source: PeerSource::Manual,
        actor: actor.to_string(),
        reason: reason.to_string(),
    })
}

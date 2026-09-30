use super::error::RegistryError;
use super::fields::{capabilities_json, decode_hex};
use super::types::{PeerRecord, PeerRegistration, PeerState};
use super::TRANSPORT_CERTIFICATE_BYTES;
use crate::direct_transport::TransportCertificate;
use rusqlite::{params, Transaction};

pub(super) fn insert_v2_trust_projection(
    transaction: &Transaction<'_>,
    registration: &PeerRegistration,
    now: i64,
    certificate: Option<&[u8]>,
) -> Result<(), RegistryError> {
    let identity_key = decode_hex(&registration.public_key)?;
    insert_v2_identity_projection(transaction, registration, now, "active")?;
    transaction.execute(
        "INSERT INTO trusted_peers
         (node_id, role, capabilities, state, added_at, updated_at)
         VALUES (?1, ?2, ?3, 'active', ?4, ?4)",
        params![
            registration.node_id,
            registration.role.code(),
            capabilities_json(&registration.capabilities)?.as_bytes(),
            now,
        ],
    )?;
    let Some(certificate) = certificate else {
        return Ok(());
    };
    if certificate.len() != TRANSPORT_CERTIFICATE_BYTES
        || &certificate[40..109] != registration.node_id.as_bytes()
        || &certificate[8..40] != identity_key.as_slice()
    {
        return Err(RegistryError::InvalidInput(
            "transport certificate does not match trusted identity".to_string(),
        ));
    }
    let key_epoch = u64::from_be_bytes(certificate[141..149].try_into().unwrap());
    if key_epoch == 0 {
        return Err(RegistryError::InvalidInput(
            "transport certificate epoch must be positive".to_string(),
        ));
    }
    transaction.execute(
        "INSERT INTO transport_key_epochs
         (node_id, key_epoch, public_key, certificate, state, added_at, retired_at)
         VALUES (?1, ?2, ?3, ?4, 'active', ?5, NULL)",
        params![
            registration.node_id,
            i64::try_from(key_epoch).map_err(|_| {
                RegistryError::InvalidInput("transport certificate epoch is too large".to_string())
            })?,
            &certificate[109..141],
            certificate,
            now,
        ],
    )?;
    Ok(())
}

pub(super) fn insert_v2_identity_projection(
    transaction: &Transaction<'_>,
    registration: &PeerRegistration,
    now: i64,
    state: &str,
) -> Result<(), RegistryError> {
    let identity_key = decode_hex(&registration.public_key)?;
    transaction.execute(
        "INSERT INTO remote_identities
         (node_id, identity_key, state, first_seen, revoked_at)
         VALUES (?1, ?2, ?3, ?4, NULL)",
        params![registration.node_id, identity_key, state, now],
    )?;
    Ok(())
}

pub(super) fn insert_v2_pending_transport_projection(
    transaction: &Transaction<'_>,
    registration: &PeerRegistration,
    now: i64,
    certificate: &[u8],
) -> Result<(), RegistryError> {
    let certificate = TransportCertificate::from_bytes(certificate)
        .map_err(|_| RegistryError::InvalidInput("transport certificate is invalid".into()))?;
    let identity_key = decode_hex(&registration.public_key)?;
    if certificate.node_id() != registration.node_id
        || certificate.identity_key().as_slice() != identity_key.as_slice()
    {
        return Err(RegistryError::InvalidInput(
            "transport certificate does not match trusted identity".into(),
        ));
    }
    let key_epoch = certificate.key_epoch();
    if key_epoch == 0 {
        return Err(RegistryError::InvalidInput(
            "transport certificate epoch must be positive".into(),
        ));
    }
    transaction.execute(
        "INSERT INTO transport_key_epochs
         (node_id, key_epoch, public_key, certificate, state, added_at, retired_at)
         VALUES (?1, ?2, ?3, ?4, 'pending', ?5, NULL)",
        params![
            registration.node_id,
            i64::try_from(key_epoch).map_err(|_| {
                RegistryError::InvalidInput("transport certificate epoch is too large".into())
            })?,
            certificate.transport_public().as_slice(),
            certificate.as_bytes().as_slice(),
            now,
        ],
    )?;
    Ok(())
}

fn require_remote_identity(
    transaction: &Transaction<'_>,
    node_id: &str,
) -> Result<(), RegistryError> {
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM remote_identities WHERE node_id = ?1)",
        [node_id],
        |row| row.get::<_, i64>(0),
    )? != 0;
    if exists {
        Ok(())
    } else {
        Err(RegistryError::Corrupt(format!(
            "peer {node_id} has no remote identity"
        )))
    }
}

pub(super) fn project_v2_transition(
    transaction: &Transaction<'_>,
    current: &PeerRecord,
    target: PeerState,
    now: i64,
) -> Result<(), RegistryError> {
    match target {
        PeerState::Active => {
            require_remote_identity(transaction, &current.node_id)?;
            transaction.execute(
                "INSERT INTO trusted_peers (node_id, role, capabilities, state, added_at, updated_at)
                 VALUES (?1, ?2, ?3, 'active', ?4, ?4)
                 ON CONFLICT(node_id) DO UPDATE SET state = 'active', updated_at = excluded.updated_at",
                params![
                    current.node_id,
                    current.role.code(),
                    capabilities_json(&current.capabilities)?.as_bytes(),
                    now,
                ],
            )?;
            transaction.execute(
                "UPDATE remote_identities SET state = 'active', revoked_at = NULL WHERE node_id = ?1",
                [&current.node_id],
            )?;
            transaction.execute(
                "UPDATE transport_key_epochs SET state = 'active', retired_at = NULL
                 WHERE node_id = ?1 AND state = 'pending'",
                [&current.node_id],
            )?;
        }
        PeerState::Suspended => {
            transaction.execute(
                "UPDATE transport_key_epochs SET state = 'pending', retired_at = ?1
                 WHERE node_id = ?2 AND state = 'active'",
                params![now, current.node_id],
            )?;
            require_remote_identity(transaction, &current.node_id)?;
        }
        PeerState::Revoked => {
            transaction.execute(
                "UPDATE transport_key_epochs SET state = 'revoked', retired_at = ?1
                 WHERE node_id = ?2 AND state <> 'revoked'",
                params![now, current.node_id],
            )?;
            transaction.execute(
                "UPDATE trusted_peers SET state = 'revoked', updated_at = ?1 WHERE node_id = ?2 AND state <> 'revoked'",
                params![now, current.node_id],
            )?;
            transaction.execute(
                "UPDATE remote_identities SET state = 'revoked', revoked_at = ?1 WHERE node_id = ?2 AND state <> 'revoked'",
                params![now, current.node_id],
            )?;
        }
        PeerState::Pending => {}
    }
    Ok(())
}

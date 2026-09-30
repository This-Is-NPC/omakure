use super::error::RegistryError;
use super::types::PeerRegistration;
use super::{
    MAX_ACTOR_BYTES, MAX_CAPABILITIES_JSON_BYTES, MAX_REASON_BYTES, PUBLIC_KEY_BYTES,
    TOO_MANY_CAPABILITIES,
};
use crate::domain::is_node_id;
use crate::node_identity::{node_id_for_x_only_public_key, NodeIdentityStatus};
use crate::util::hex;
use chrono::{DateTime, SecondsFormat, Utc};
use sha2::{Digest, Sha256};

pub(super) fn timestamp_seconds(value: &str) -> Result<i64, RegistryError> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|_| RegistryError::InvalidSchema(format!("invalid timestamp {value:?}")))?;
    if parsed.offset().local_minus_utc() != 0 {
        return Err(RegistryError::InvalidSchema(format!(
            "timestamp is not UTC: {value:?}"
        )));
    }
    let seconds = parsed.timestamp();
    if seconds <= 0 {
        return Err(RegistryError::InvalidSchema(
            "timestamp is not positive".to_string(),
        ));
    }
    Ok(seconds)
}

pub(super) fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(super) fn validate_identity(
    identity: &NodeIdentityStatus,
) -> Result<(String, String), RegistryError> {
    validate_public_key(&identity.public_key_hex)?;
    let public_key = identity.public_key_hex.to_ascii_lowercase();
    let bytes = decode_hex(&public_key)?;
    let derived = node_id_for_x_only_public_key(&bytes);
    if identity.node_id != derived {
        return Err(RegistryError::InvalidInput(
            "identity node ID does not match its public key".to_string(),
        ));
    }
    validate_node_id(&identity.node_id)?;
    Ok((identity.node_id.clone(), public_key))
}

pub(super) fn validate_registration(
    registration: &PeerRegistration,
    local_node_id: &str,
    local_public_key: &str,
) -> Result<(), RegistryError> {
    validate_public_key(&registration.public_key)?;
    validate_node_id(&registration.node_id)?;
    if registration.node_id == local_node_id || registration.public_key == local_public_key {
        return Err(RegistryError::SelfTrust);
    }
    let bytes = decode_hex(&registration.public_key)?;
    if node_id_for_x_only_public_key(&bytes) != registration.node_id {
        return Err(RegistryError::InvalidInput(
            "peer node ID does not match its public key".to_string(),
        ));
    }
    validate_capabilities(&registration.capabilities)?;
    validate_actor_reason(&registration.actor, &registration.reason)?;
    Ok(())
}

pub(super) fn validate_public_key(value: &str) -> Result<(), RegistryError> {
    if value.len() != PUBLIC_KEY_BYTES || !hex::is_lower(value) {
        return Err(RegistryError::InvalidInput(
            "public key must be 64 lowercase hexadecimal x-only bytes".to_string(),
        ));
    }
    let bytes = decode_hex(value)?;
    k256::schnorr::VerifyingKey::from_slice(&bytes).map_err(|_| {
        RegistryError::InvalidInput("public key is not a valid BIP-340 x-only key".to_string())
    })?;
    Ok(())
}

pub(super) fn validate_node_id(value: &str) -> Result<(), RegistryError> {
    if !is_node_id(value) {
        return Err(RegistryError::InvalidInput(
            "node ID must be omk1_ followed by 64 lowercase hexadecimal characters".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn decode_hex(value: &str) -> Result<Vec<u8>, RegistryError> {
    if !value.len().is_multiple_of(2) {
        return Err(RegistryError::InvalidInput(
            "hex value has odd length".to_string(),
        ));
    }
    hex::decode(value)
        .ok_or_else(|| RegistryError::InvalidInput("invalid hexadecimal value".to_string()))
}

pub(super) fn validate_capabilities(capabilities: &[String]) -> Result<(), RegistryError> {
    use crate::domain::CapabilityListError;
    crate::domain::check_capability_list(capabilities).map_err(|error| {
        RegistryError::InvalidInput(match error {
            CapabilityListError::TooMany => TOO_MANY_CAPABILITIES.to_string(),
            CapabilityListError::Unsupported(capability) => {
                format!("unsupported or invalid capability {capability:?}")
            }
            CapabilityListError::Unsorted => "capabilities must be sorted and unique".to_string(),
        })
    })?;
    let json = capabilities_json(capabilities)?;
    if json.len() > MAX_CAPABILITIES_JSON_BYTES {
        return Err(RegistryError::InvalidInput(
            "capabilities JSON is too large".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn capabilities_json(capabilities: &[String]) -> Result<String, RegistryError> {
    validate_capabilities_without_json(capabilities)?;
    serde_json::to_string(capabilities)
        .map_err(|error| RegistryError::InvalidInput(format!("capabilities JSON: {error}")))
}

fn validate_capabilities_without_json(capabilities: &[String]) -> Result<(), RegistryError> {
    if capabilities.len() > crate::domain::MAX_CAPABILITIES {
        return Err(RegistryError::InvalidInput(
            TOO_MANY_CAPABILITIES.to_string(),
        ));
    }
    Ok(())
}

pub(super) fn validate_actor_reason(actor: &str, reason: &str) -> Result<(), RegistryError> {
    validate_bounded_text("actor", actor, MAX_ACTOR_BYTES)?;
    validate_bounded_text("reason", reason, MAX_REASON_BYTES)?;
    Ok(())
}

pub(super) fn validate_bounded_text(
    label: &str,
    value: &str,
    max_bytes: usize,
) -> Result<(), RegistryError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(RegistryError::InvalidInput(format!(
            "{label} is empty, oversized, or contains control characters"
        )));
    }
    Ok(())
}

pub(super) fn validate_timestamp(value: &str) -> Result<(), RegistryError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| {
        RegistryError::InvalidSchema(format!("invalid RFC3339 timestamp {value:?}"))
    })?;
    if parsed.offset().local_minus_utc() != 0
        || parsed.to_rfc3339_opts(SecondsFormat::Millis, true) != value
    {
        return Err(RegistryError::InvalidSchema(format!(
            "timestamp is not canonical UTC RFC3339 milliseconds: {value:?}"
        )));
    }
    if parsed.with_timezone(&Utc) > Utc::now() + chrono::Duration::minutes(5) {
        return Err(RegistryError::InvalidSchema(format!(
            "timestamp is too far in the future: {value:?}"
        )));
    }
    Ok(())
}

pub(super) fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

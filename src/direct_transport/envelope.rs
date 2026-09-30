use super::certificate::TransportCertificate;
use super::errors::TransportError;
use super::DIRECT_ENVELOPE_DOMAIN;
use crate::node_identity::{Bip340Signature, DirectEnvelopePrehash, NodeIdentity};
use crate::node_registry::PeerState;
use crate::util::digest::sha256_domain;
use crate::util::hex;
use k256::schnorr::{signature::hazmat::PrehashVerifier, Signature, VerifyingKey};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedEnvelope {
    pub canonical: Vec<u8>,
    pub signature: [u8; 64],
}

impl SignedEnvelope {
    pub fn encoded(&self) -> Vec<u8> {
        let mut encoded = self.canonical.clone();
        encoded.extend_from_slice(&self.signature);
        encoded
    }
}

pub fn sign_probe(
    identity: &NodeIdentity,
    session_id: &[u8; 32],
    nonce: [u8; 16],
    now: u64,
) -> Result<SignedEnvelope, TransportError> {
    sign_envelope(
        identity,
        "probe",
        session_id,
        nonce,
        Value::Object(serde_json::Map::new()),
        now,
    )
}

pub fn sign_manual_request(
    identity: &NodeIdentity,
    session_id: &[u8; 32],
    nonce: [u8; 16],
    request: &[u8],
    now: u64,
) -> Result<SignedEnvelope, TransportError> {
    if request.len() > crate::enrollment::MAX_REQUEST_BYTES {
        return Err(TransportError::MessageTooLarge);
    }
    let mut payload = serde_json::Map::new();
    payload.insert("request".into(), Value::from(hex::encode(request)));
    sign_envelope(
        identity,
        "manual_request",
        session_id,
        nonce,
        Value::Object(payload),
        now,
    )
}

pub fn sign_manual_ack(
    identity: &NodeIdentity,
    session_id: &[u8; 32],
    nonce: [u8; 16],
    accepted: bool,
    reciprocal_request: Option<&[u8]>,
    reciprocal_code: Option<&[u8]>,
    now: u64,
) -> Result<SignedEnvelope, TransportError> {
    let mut payload = serde_json::Map::new();
    payload.insert("accepted".into(), Value::from(accepted));
    match (accepted, reciprocal_request, reciprocal_code) {
        (true, Some(request), Some(code)) => {
            payload.insert("request".into(), Value::from(hex::encode(request)));
            payload.insert("code".into(), Value::from(hex::encode(code)));
        }
        (false, None, None) => {}
        _ => return Err(TransportError::InvalidFrame),
    }
    sign_envelope(
        identity,
        "manual_ack",
        session_id,
        nonce,
        Value::Object(payload),
        now,
    )
}

pub fn sign_ack(
    identity: &NodeIdentity,
    session_id: &[u8; 32],
    nonce: [u8; 16],
    now: u64,
) -> Result<SignedEnvelope, TransportError> {
    sign_envelope(
        identity,
        "ack",
        session_id,
        nonce,
        Value::Object(serde_json::Map::new()),
        now,
    )
}

pub fn verify_envelope(
    encoded: &[u8],
    expected_sender: &str,
    expected_identity_key: &[u8; 32],
    expected_kind: &str,
    expected_session_id: &[u8; 32],
    expected_nonce: &[u8; 16],
) -> Result<(), TransportError> {
    if encoded.len() < 64 {
        return Err(TransportError::InvalidFrame);
    }
    let split = encoded.len() - 64;
    let canonical = &encoded[..split];
    let signature =
        Signature::try_from(&encoded[split..]).map_err(|_| TransportError::HandshakeFailed)?;
    let value: Value =
        serde_json::from_slice(canonical).map_err(|_| TransportError::InvalidFrame)?;
    if canonical_json(&value).as_slice() != canonical {
        return Err(TransportError::InvalidFrame);
    }
    let object = value.as_object().ok_or(TransportError::InvalidFrame)?;
    if object.get("version").and_then(Value::as_u64) != Some(1)
        || object.get("sender").and_then(Value::as_str) != Some(expected_sender)
        || object.get("kind").and_then(Value::as_str) != Some(expected_kind)
    {
        return Err(TransportError::IdentityMismatch);
    }
    let expected_session = hex::encode(expected_session_id);
    let expected_nonce = hex::encode(expected_nonce);
    if object.get("session_id").and_then(Value::as_str) != Some(expected_session.as_str())
        || object.get("nonce").and_then(Value::as_str) != Some(expected_nonce.as_str())
    {
        return Err(TransportError::Replay);
    }
    let key = VerifyingKey::from_bytes(expected_identity_key.into())
        .map_err(|_| TransportError::IdentityMismatch)?;
    let digest = sha256_domain(DIRECT_ENVELOPE_DOMAIN, canonical);
    key.verify_prehash(&digest, &signature)
        .map_err(|_| TransportError::HandshakeFailed)
}

pub fn envelope_nonce(encoded: &[u8]) -> Result<[u8; 16], TransportError> {
    if encoded.len() < 64 {
        return Err(TransportError::InvalidFrame);
    }
    let value: Value = serde_json::from_slice(&encoded[..encoded.len() - 64])
        .map_err(|_| TransportError::InvalidFrame)?;
    let nonce = value
        .get("nonce")
        .and_then(Value::as_str)
        .and_then(hex::decode)
        .ok_or(TransportError::InvalidFrame)?;
    nonce.try_into().map_err(|_| TransportError::InvalidFrame)
}

pub(super) fn sign_envelope(
    identity: &NodeIdentity,
    kind: &str,
    session_id: &[u8; 32],
    nonce: [u8; 16],
    payload: Value,
    now: u64,
) -> Result<SignedEnvelope, TransportError> {
    let mut object = serde_json::Map::new();
    object.insert("created_at".into(), Value::from(now));
    object.insert("kind".into(), Value::from(kind));
    object.insert("nonce".into(), Value::from(hex::encode(&nonce)));
    object.insert("payload".into(), payload);
    object.insert(
        "sender".into(),
        Value::from(identity.public_status().node_id.clone()),
    );
    object.insert("session_id".into(), Value::from(hex::encode(session_id)));
    object.insert("version".into(), Value::from(1u8));
    let canonical = canonical_json(&Value::Object(object));
    let prehash = DirectEnvelopePrehash::from_canonical_bytes(&canonical);
    let signature: Bip340Signature = identity
        .sign_direct_envelope(prehash)
        .map_err(|_| TransportError::Internal)?;
    Ok(SignedEnvelope {
        canonical,
        signature: signature.to_bytes(),
    })
}

fn canonical_json(value: &Value) -> Vec<u8> {
    serde_jcs::to_vec(value).expect("validated JSON values are JCS serializable")
}

pub type PeerAuthorization<'a> = (
    &'a str,
    &'a [u8; 32],
    Option<&'a [u8; 32]>,
    Option<u64>,
    PeerState,
);

pub fn authorize_peer(
    certificate: &TransportCertificate,
    expected_peer: Option<PeerAuthorization<'_>>,
    now: u64,
) -> Result<(), TransportError> {
    certificate.verify_time(now)?;
    let Some((node_id, public_key, transport_public_key, key_epoch, state)) = expected_peer else {
        return Err(TransportError::NotEnrolled);
    };
    if state == PeerState::Revoked {
        return Err(TransportError::Revoked);
    }
    if state != PeerState::Active {
        return Err(TransportError::NotEnrolled);
    }
    if certificate.node_id() != node_id || certificate.identity_key() != public_key {
        return Err(TransportError::IdentityMismatch);
    }
    if certificate.transport_public() != transport_public_key.ok_or(TransportError::NotEnrolled)?
        || certificate.key_epoch() != key_epoch.ok_or(TransportError::NotEnrolled)?
    {
        return Err(TransportError::IdentityMismatch);
    }
    Ok(())
}

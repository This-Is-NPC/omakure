use super::errors::TransportError;
use super::x25519::validate_x25519_public;
use super::{
    CERTIFICATE_BODY_BYTES, CERTIFICATE_DOMAIN, CERTIFICATE_FUTURE_SKEW_SECONDS,
    CERTIFICATE_MAX_LIFETIME_SECONDS, CERT_MAGIC, MAX_CERTIFICATE_BYTES,
};
use crate::domain::is_node_id;
use crate::node_identity::NodeIdentity;
use crate::util::digest::sha256_domain;
use crate::util::hex;
use k256::schnorr::{signature::hazmat::PrehashVerifier, Signature, VerifyingKey};
use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub struct TransportCertificate {
    bytes: [u8; MAX_CERTIFICATE_BYTES],
}

impl fmt::Debug for TransportCertificate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransportCertificate")
            .field("identity_key", &hex::encode(&self.bytes[8..40]))
            .field("node_id", &String::from_utf8_lossy(&self.bytes[40..109]))
            .field("transport_key", &"<redacted-public-key>")
            .field("key_epoch", &self.key_epoch())
            .finish()
    }
}

impl TransportCertificate {
    pub fn issue(
        identity: &NodeIdentity,
        transport_public: [u8; 32],
        key_epoch: u64,
        not_before: u64,
        not_after: u64,
        certificate_id: [u8; 16],
    ) -> Result<Self, TransportError> {
        validate_x25519_public(&transport_public)?;
        if key_epoch == 0
            || not_after <= not_before
            || not_after - not_before > CERTIFICATE_MAX_LIFETIME_SECONDS
        {
            return Err(TransportError::Expired);
        }
        let status = identity.public_status();
        let key = hex::decode(&status.public_key_hex).ok_or(TransportError::IdentityMismatch)?;
        let mut body = Vec::with_capacity(CERTIFICATE_BODY_BYTES);
        body.extend_from_slice(CERT_MAGIC);
        body.extend_from_slice(&[1, 1]);
        body.extend_from_slice(&0u16.to_be_bytes());
        body.extend_from_slice(&key);
        if status.node_id.len() != 69 || !status.node_id.is_ascii() {
            return Err(TransportError::IdentityMismatch);
        }
        body.extend_from_slice(status.node_id.as_bytes());
        body.extend_from_slice(&transport_public);
        body.extend_from_slice(&key_epoch.to_be_bytes());
        body.extend_from_slice(&not_before.to_be_bytes());
        body.extend_from_slice(&not_after.to_be_bytes());
        body.extend_from_slice(&certificate_id);
        let signature = identity
            .sign_transport_certificate(&body)
            .map_err(|_| TransportError::Internal)?;
        body.extend_from_slice(&signature.to_bytes());
        Self::from_bytes(&body)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TransportError> {
        let bytes: [u8; MAX_CERTIFICATE_BYTES] = bytes
            .try_into()
            .map_err(|_| TransportError::HandshakeFailed)?;
        if &bytes[..4] != CERT_MAGIC || bytes[4] != 1 || bytes[5] != 1 || bytes[6..8] != [0, 0] {
            return Err(TransportError::UnsupportedVersion);
        }
        let key: [u8; 32] = bytes[8..40].try_into().unwrap();
        let node_id =
            std::str::from_utf8(&bytes[40..109]).map_err(|_| TransportError::IdentityMismatch)?;
        if !is_node_id(node_id)
            || crate::node_identity::node_id_for_x_only_public_key(&key) != node_id
        {
            return Err(TransportError::IdentityMismatch);
        }
        validate_x25519_public(&bytes[109..141])?;
        let epoch = u64::from_be_bytes(bytes[141..149].try_into().unwrap());
        let not_before = u64::from_be_bytes(bytes[149..157].try_into().unwrap());
        let not_after = u64::from_be_bytes(bytes[157..165].try_into().unwrap());
        if epoch == 0
            || not_after <= not_before
            || not_after - not_before > CERTIFICATE_MAX_LIFETIME_SECONDS
        {
            return Err(TransportError::Expired);
        }
        let verifying_key = VerifyingKey::from_bytes((&key).into())
            .map_err(|_| TransportError::IdentityMismatch)?;
        let signature =
            Signature::try_from(&bytes[181..]).map_err(|_| TransportError::HandshakeFailed)?;
        let digest = sha256_domain(CERTIFICATE_DOMAIN, &bytes[..CERTIFICATE_BODY_BYTES]);
        verifying_key
            .verify_prehash(&digest, &signature)
            .map_err(|_| TransportError::HandshakeFailed)?;
        Ok(Self { bytes })
    }

    pub fn verify_time(&self, now: u64) -> Result<(), TransportError> {
        let not_before = self.not_before();
        let not_after = self.not_after();
        if now.saturating_add(CERTIFICATE_FUTURE_SKEW_SECONDS) < not_before || now >= not_after {
            return Err(TransportError::Expired);
        }
        Ok(())
    }

    pub fn as_bytes(&self) -> &[u8; MAX_CERTIFICATE_BYTES] {
        &self.bytes
    }

    pub fn identity_key(&self) -> &[u8; 32] {
        self.bytes[8..40].try_into().unwrap()
    }

    pub fn node_id(&self) -> &str {
        std::str::from_utf8(&self.bytes[40..109]).expect("certificate node id validated")
    }

    pub fn transport_public(&self) -> &[u8; 32] {
        self.bytes[109..141].try_into().unwrap()
    }

    pub fn key_epoch(&self) -> u64 {
        u64::from_be_bytes(self.bytes[141..149].try_into().unwrap())
    }

    pub fn not_before(&self) -> u64 {
        u64::from_be_bytes(self.bytes[149..157].try_into().unwrap())
    }

    pub fn not_after(&self) -> u64 {
        u64::from_be_bytes(self.bytes[157..165].try_into().unwrap())
    }

    pub fn certificate_id(&self) -> &[u8; 16] {
        self.bytes[165..181].try_into().unwrap()
    }
}

pub(super) fn certificate_payload(certificate: &TransportCertificate) -> Vec<u8> {
    let mut payload = Vec::with_capacity(1 + MAX_CERTIFICATE_BYTES);
    payload.push(1);
    payload.extend_from_slice(certificate.as_bytes());
    payload
}

pub(super) fn parse_certificate_payload(
    payload: &[u8],
) -> Result<TransportCertificate, TransportError> {
    if payload.len() != 1 + MAX_CERTIFICATE_BYTES || payload[0] != 1 {
        return Err(TransportError::HandshakeFailed);
    }
    TransportCertificate::from_bytes(&payload[1..])
}

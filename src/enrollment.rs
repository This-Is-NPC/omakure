//! Protocol-neutral manual enrollment records.
//!
//! The request bytes in this module are the authority for the manual path.  A
//! transport or management adapter may carry them, but it must not re-encode
//! or reinterpret the signed record.

use crate::direct_transport::validate_x25519_public;
use crate::node_identity::NodeIdentity;
use k256::schnorr::{
    signature::hazmat::{PrehashSigner, PrehashVerifier},
    Signature, SigningKey, VerifyingKey,
};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use thiserror::Error;

pub const VERSION: u8 = 2;
pub const MAX_REQUEST_BYTES: usize = 2_048;
pub const MAX_CAPABILITIES: usize = 32;
pub const MAX_CAPABILITY_BYTES: usize = 64;
pub const FUTURE_SKEW_SECONDS: u64 = 300;
pub const MAX_LIFETIME_SECONDS: u64 = 30 * 24 * 60 * 60;
pub const REPLAY_RETENTION_SECONDS: u64 = 24 * 60 * 60;
pub const NODE_ID_BYTES: usize = 69;
pub const IDENTITY_KEY_BYTES: usize = 32;
pub const TRANSPORT_KEY_BYTES: usize = 32;
pub const REQUEST_ID_BYTES: usize = 16;
pub const PAIRING_ID_BYTES: usize = 16;
pub const CODE_BYTES: usize = 16;
pub const SIGNATURE_BYTES: usize = 64;
pub const DOMAIN: &[u8] = b"omakure/manual-enrollment/v1\0";
pub const BUNDLE_DOMAIN: &[u8] = b"omakure/enrollment-bundle/v1\0";
pub const BUNDLE_VERSION: u8 = 1;
pub const MAX_BUNDLE_BYTES: usize = 8_192;
pub const MAX_BUNDLE_INPUT_BYTES: usize = MAX_BUNDLE_BYTES * 2;
pub const MAX_BOOTSTRAP_TOKEN_BYTES: usize = 256;
pub const BUNDLE_AUTHORITY_ID_BYTES: usize = 16;
pub const MAX_ORGANIZATION_BYTES: usize = 128;
pub const BUNDLE_FUTURE_SKEW_SECONDS: u64 = 300;
pub const BUNDLE_MAX_LIFETIME_SECONDS: u64 = 30 * 24 * 60 * 60;

const MAGIC: &[u8; 4] = b"OMMA";
const SUPPORTED_CAPABILITIES: &[&str] = &[
    "backup-orchestration",
    "baseline-push",
    "inventory-health",
    "lost-device-revocation",
    "notifications",
    "remote-run",
    "ssh-credential-rotation",
];

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EnrollmentError {
    #[error("invalid manual enrollment request")]
    Invalid,
    #[error("manual enrollment request is expired")]
    Expired,
    #[error("manual enrollment request was replayed")]
    Replay,
    #[error("manual enrollment identity binding is invalid")]
    IdentityMismatch,
    #[error("manual enrollment request is too large")]
    TooLarge,
    #[error("signed enrollment bundle authority is not trusted")]
    AuthorityUnknown,
    #[error("signed enrollment bundle authority is revoked")]
    AuthorityRevoked,
    #[error("signed enrollment bundle organization does not match local policy")]
    OrganizationMismatch,
    #[error("signed enrollment bundle audience does not match this node")]
    AudienceMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleAuthority {
    pub key_id: [u8; BUNDLE_AUTHORITY_ID_BYTES],
    pub public_key: [u8; IDENTITY_KEY_BYTES],
    pub revoked: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SignedEnrollmentBundle {
    pub bundle_id: [u8; REQUEST_ID_BYTES],
    pub authority_key_id: [u8; BUNDLE_AUTHORITY_ID_BYTES],
    pub organization: String,
    pub audience_node_id: String,
    pub subject_node_id: String,
    pub subject_xonly: [u8; IDENTITY_KEY_BYTES],
    pub subject_transport_x25519: [u8; TRANSPORT_KEY_BYTES],
    pub subject_certificate: [u8; crate::direct_transport::MAX_CERTIFICATE_BYTES],
    pub role: EnrollmentRole,
    pub capabilities: Vec<String>,
    pub issued_at: u64,
    pub expires_at: u64,
    pub authority_signature: [u8; SIGNATURE_BYTES],
}

impl fmt::Debug for SignedEnrollmentBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignedEnrollmentBundle")
            .field("bundle_id", &hex(&self.bundle_id))
            .field("authority_key_id", &hex(&self.authority_key_id))
            .field("organization", &self.organization)
            .field("audience_node_id", &self.audience_node_id)
            .field("subject_node_id", &self.subject_node_id)
            .field("subject_xonly", &"<redacted-public-key>")
            .field("subject_transport_x25519", &"<redacted-public-key>")
            .field("subject_certificate", &"<redacted-certificate>")
            .field("role", &self.role)
            .field("capabilities", &self.capabilities)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .field("authority_signature", &"<redacted>")
            .finish()
    }
}

impl SignedEnrollmentBundle {
    #[allow(clippy::too_many_arguments)]
    pub fn sign_with_material(
        authority_private_key: &[u8],
        bundle_id: [u8; REQUEST_ID_BYTES],
        authority_key_id: [u8; BUNDLE_AUTHORITY_ID_BYTES],
        organization: String,
        audience_node_id: String,
        subject_node_id: String,
        subject_xonly: [u8; IDENTITY_KEY_BYTES],
        subject_transport_x25519: [u8; TRANSPORT_KEY_BYTES],
        subject_certificate: [u8; crate::direct_transport::MAX_CERTIFICATE_BYTES],
        role: EnrollmentRole,
        capabilities: Vec<String>,
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, EnrollmentError> {
        let signing_key = SigningKey::from_slice(authority_private_key)
            .map_err(|_| EnrollmentError::IdentityMismatch)?;
        let mut bundle = Self {
            bundle_id,
            authority_key_id,
            organization,
            audience_node_id,
            subject_node_id,
            subject_xonly,
            subject_transport_x25519,
            subject_certificate,
            role,
            capabilities,
            issued_at,
            expires_at,
            authority_signature: [0; SIGNATURE_BYTES],
        };
        let digest = hash_domain(&bundle.unsigned_bytes()?, BUNDLE_DOMAIN);
        bundle.authority_signature = signing_key
            .sign_prehash(&digest)
            .map_err(|_| EnrollmentError::IdentityMismatch)?
            .to_bytes();
        bundle.verify_shape()?;
        Ok(bundle)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, EnrollmentError> {
        if bytes.len() > MAX_BUNDLE_BYTES {
            return Err(EnrollmentError::TooLarge);
        }
        let mut cursor = Cursor::new(bytes);
        if cursor.take(4)? != b"OMEB"
            || cursor.byte()? != BUNDLE_VERSION
            || cursor.take(2)? != [0, 0]
        {
            return Err(EnrollmentError::Invalid);
        }
        let bundle_id = cursor.array::<REQUEST_ID_BYTES>()?;
        let authority_key_id = cursor.array::<BUNDLE_AUTHORITY_ID_BYTES>()?;
        let organization_length = usize::from(cursor.u16()?);
        let organization = cursor.text(organization_length)?;
        let audience_node_id = cursor.text(NODE_ID_BYTES)?;
        let subject_node_id = cursor.text(NODE_ID_BYTES)?;
        let subject_xonly = cursor.array::<IDENTITY_KEY_BYTES>()?;
        let subject_transport_x25519 = cursor.array::<TRANSPORT_KEY_BYTES>()?;
        let subject_certificate =
            cursor.array::<{ crate::direct_transport::MAX_CERTIFICATE_BYTES }>()?;
        let role = EnrollmentRole::from_u8(cursor.byte()?)?;
        let capability_count = usize::from(cursor.byte()?);
        if capability_count > MAX_CAPABILITIES {
            return Err(EnrollmentError::Invalid);
        }
        let mut capabilities = Vec::with_capacity(capability_count);
        for _ in 0..capability_count {
            let length = usize::from(cursor.u16()?);
            capabilities.push(cursor.text(length)?);
        }
        let issued_at = cursor.u64()?;
        let expires_at = cursor.u64()?;
        let authority_signature = cursor.array::<SIGNATURE_BYTES>()?;
        if cursor.remaining() != 0 {
            return Err(EnrollmentError::Invalid);
        }
        let bundle = Self {
            bundle_id,
            authority_key_id,
            organization,
            audience_node_id,
            subject_node_id,
            subject_xonly,
            subject_transport_x25519,
            subject_certificate,
            role,
            capabilities,
            issued_at,
            expires_at,
            authority_signature,
        };
        bundle.verify_shape()?;
        Ok(bundle)
    }

    pub fn encode(&self) -> Vec<u8> {
        self.unsigned_bytes()
            .expect("signed enrollment bundle must be valid")
            .into_iter()
            .chain(self.authority_signature)
            .collect()
    }

    pub fn verify(
        &self,
        authority: &BundleAuthority,
        organization: &str,
        audience_node_id: &str,
        now: u64,
    ) -> Result<(), EnrollmentError> {
        self.verify_shape()?;
        if self.authority_key_id != authority.key_id {
            return Err(EnrollmentError::AuthorityUnknown);
        }
        if authority.revoked {
            return Err(EnrollmentError::AuthorityRevoked);
        }
        if self.organization != organization {
            return Err(EnrollmentError::OrganizationMismatch);
        }
        if self.audience_node_id != audience_node_id {
            return Err(EnrollmentError::AudienceMismatch);
        }
        if now.saturating_add(BUNDLE_FUTURE_SKEW_SECONDS) < self.issued_at || now >= self.expires_at
        {
            return Err(EnrollmentError::Expired);
        }
        let key = VerifyingKey::from_slice(&authority.public_key)
            .map_err(|_| EnrollmentError::AuthorityUnknown)?;
        let signature = Signature::from_slice(&self.authority_signature)
            .map_err(|_| EnrollmentError::Invalid)?;
        let digest = hash_domain(&self.unsigned_bytes()?, BUNDLE_DOMAIN);
        key.verify_prehash(&digest, &signature)
            .map_err(|_| EnrollmentError::IdentityMismatch)
    }

    pub fn replay_expiry(&self) -> u64 {
        self.expires_at.saturating_add(REPLAY_RETENTION_SECONDS)
    }

    fn unsigned_bytes(&self) -> Result<Vec<u8>, EnrollmentError> {
        self.verify_shape()?;
        let mut bytes = Vec::with_capacity(604);
        bytes.extend_from_slice(b"OMEB");
        bytes.push(BUNDLE_VERSION);
        bytes.extend_from_slice(&0_u16.to_be_bytes());
        bytes.extend_from_slice(&self.bundle_id);
        bytes.extend_from_slice(&self.authority_key_id);
        bytes.extend_from_slice(
            &u16::try_from(self.organization.len())
                .map_err(|_| EnrollmentError::TooLarge)?
                .to_be_bytes(),
        );
        bytes.extend_from_slice(self.organization.as_bytes());
        bytes.extend_from_slice(self.audience_node_id.as_bytes());
        bytes.extend_from_slice(self.subject_node_id.as_bytes());
        bytes.extend_from_slice(&self.subject_xonly);
        bytes.extend_from_slice(&self.subject_transport_x25519);
        bytes.extend_from_slice(&self.subject_certificate);
        bytes.push(self.role as u8);
        bytes.push(u8::try_from(self.capabilities.len()).map_err(|_| EnrollmentError::Invalid)?);
        for capability in &self.capabilities {
            bytes.extend_from_slice(
                &u16::try_from(capability.len())
                    .map_err(|_| EnrollmentError::Invalid)?
                    .to_be_bytes(),
            );
            bytes.extend_from_slice(capability.as_bytes());
        }
        bytes.extend_from_slice(&self.issued_at.to_be_bytes());
        bytes.extend_from_slice(&self.expires_at.to_be_bytes());
        if bytes.len() + SIGNATURE_BYTES > MAX_BUNDLE_BYTES {
            return Err(EnrollmentError::TooLarge);
        }
        Ok(bytes)
    }

    fn verify_shape(&self) -> Result<(), EnrollmentError> {
        if self.bundle_id == [0; REQUEST_ID_BYTES]
            || self.authority_key_id == [0; BUNDLE_AUTHORITY_ID_BYTES]
            || self.organization.is_empty()
            || self.organization.len() > MAX_ORGANIZATION_BYTES
            || self
                .organization
                .bytes()
                .any(|byte| byte.is_ascii_control())
        {
            return Err(EnrollmentError::Invalid);
        }
        for node_id in [&self.audience_node_id, &self.subject_node_id] {
            if node_id.len() != NODE_ID_BYTES
                || !node_id.starts_with("omk1_")
                || !node_id[5..]
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(EnrollmentError::IdentityMismatch);
            }
        }
        if crate::node_identity::node_id_for_x_only_public_key(&self.subject_xonly)
            != self.subject_node_id
        {
            return Err(EnrollmentError::IdentityMismatch);
        }
        crate::direct_transport::validate_x25519_public(&self.subject_transport_x25519)
            .map_err(|_| EnrollmentError::Invalid)?;
        let certificate =
            crate::direct_transport::TransportCertificate::from_bytes(&self.subject_certificate)
                .map_err(|_| EnrollmentError::IdentityMismatch)?;
        if certificate.node_id() != self.subject_node_id
            || certificate.identity_key() != &self.subject_xonly
            || certificate.transport_public() != &self.subject_transport_x25519
        {
            return Err(EnrollmentError::IdentityMismatch);
        }
        if self.expires_at <= self.issued_at
            || self.expires_at - self.issued_at > BUNDLE_MAX_LIFETIME_SECONDS
        {
            return Err(EnrollmentError::Invalid);
        }
        validate_capabilities(&self.capabilities)
    }
}

pub fn hash_bootstrap_token(token: &[u8]) -> [u8; 32] {
    hash_domain(token, b"omakure/bootstrap-token/v1\0")
}

pub fn hash_bootstrap_nonce(nonce: &[u8]) -> [u8; 32] {
    hash_domain(nonce, b"omakure/bootstrap-nonce/v1\0")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EnrollmentRole {
    Conductor = 1,
    Performer = 2,
}

impl EnrollmentRole {
    pub fn from_u8(value: u8) -> Result<Self, EnrollmentError> {
        match value {
            1 => Ok(Self::Conductor),
            2 => Ok(Self::Performer),
            _ => Err(EnrollmentError::Invalid),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ManualEnrollmentRequest {
    pub pairing_id: [u8; PAIRING_ID_BYTES],
    pub request_id: [u8; REQUEST_ID_BYTES],
    pub proposer_node_id: String,
    pub proposer_xonly: [u8; IDENTITY_KEY_BYTES],
    pub proposer_transport_x25519: [u8; TRANSPORT_KEY_BYTES],
    pub role: EnrollmentRole,
    pub capabilities: Vec<String>,
    pub created_at: u64,
    pub expires_at: u64,
    pub code_hash: [u8; 32],
    pub signature: [u8; SIGNATURE_BYTES],
}

impl fmt::Debug for ManualEnrollmentRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManualEnrollmentRequest")
            .field("request_id", &hex(&self.request_id))
            .field("pairing_id", &hex(&self.pairing_id))
            .field("proposer_node_id", &self.proposer_node_id)
            .field("proposer_xonly", &"<redacted-public-key>")
            .field("proposer_transport_x25519", &"<redacted-public-key>")
            .field("role", &self.role)
            .field("capabilities", &self.capabilities)
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .field("code_hash", &"<redacted>")
            .field("signature", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualEnrollmentOffer {
    pub request: ManualEnrollmentRequest,
    pub code: [u8; CODE_BYTES],
}

impl ManualEnrollmentOffer {
    pub fn request_hex(&self) -> String {
        hex(&self.request.encode())
    }

    pub fn code_hex(&self) -> String {
        hex(&self.code)
    }
}

impl ManualEnrollmentRequest {
    pub fn pairing_id_hex(&self) -> String {
        hex(&self.pairing_id)
    }

    pub fn request_id_hex(&self) -> String {
        hex(&self.request_id)
    }

    pub fn create(
        identity: &NodeIdentity,
        proposer_transport_x25519: [u8; TRANSPORT_KEY_BYTES],
        role: EnrollmentRole,
        capabilities: Vec<String>,
        now: u64,
        lifetime_seconds: u64,
    ) -> Result<ManualEnrollmentOffer, EnrollmentError> {
        let mut pairing_id = [0u8; PAIRING_ID_BYTES];
        OsRng.fill_bytes(&mut pairing_id);
        Self::create_with_pairing_id(
            identity,
            proposer_transport_x25519,
            role,
            capabilities,
            now,
            lifetime_seconds,
            pairing_id,
        )
    }

    pub fn create_with_pairing_id(
        identity: &NodeIdentity,
        proposer_transport_x25519: [u8; TRANSPORT_KEY_BYTES],
        role: EnrollmentRole,
        capabilities: Vec<String>,
        now: u64,
        lifetime_seconds: u64,
        pairing_id: [u8; PAIRING_ID_BYTES],
    ) -> Result<ManualEnrollmentOffer, EnrollmentError> {
        validate_capabilities(&capabilities)?;
        validate_x25519_public(&proposer_transport_x25519).map_err(|_| EnrollmentError::Invalid)?;
        if lifetime_seconds == 0 || lifetime_seconds > MAX_LIFETIME_SECONDS {
            return Err(EnrollmentError::Invalid);
        }
        let mut request_id = [0u8; REQUEST_ID_BYTES];
        let mut code = [0u8; CODE_BYTES];
        OsRng.fill_bytes(&mut request_id);
        OsRng.fill_bytes(&mut code);
        Self::create_with_material(
            identity,
            proposer_transport_x25519,
            role,
            capabilities,
            now,
            lifetime_seconds,
            pairing_id,
            request_id,
            code,
        )
    }

    /// Construct a deterministic request for protocol vectors and fixtures.
    #[allow(clippy::too_many_arguments)] // Fixed protocol-vector material must remain explicit.
    pub fn create_with_material(
        identity: &NodeIdentity,
        proposer_transport_x25519: [u8; TRANSPORT_KEY_BYTES],
        role: EnrollmentRole,
        capabilities: Vec<String>,
        now: u64,
        lifetime_seconds: u64,
        pairing_id: [u8; PAIRING_ID_BYTES],
        request_id: [u8; REQUEST_ID_BYTES],
        code: [u8; CODE_BYTES],
    ) -> Result<ManualEnrollmentOffer, EnrollmentError> {
        validate_capabilities(&capabilities)?;
        validate_x25519_public(&proposer_transport_x25519).map_err(|_| EnrollmentError::Invalid)?;
        if lifetime_seconds == 0 || lifetime_seconds > MAX_LIFETIME_SECONDS {
            return Err(EnrollmentError::Invalid);
        }
        let code_hash = hash_code(&code);
        let mut request = Self {
            pairing_id,
            request_id,
            proposer_node_id: identity.public_status().node_id.clone(),
            proposer_xonly: decode_hex::<IDENTITY_KEY_BYTES>(
                &identity.public_status().public_key_hex,
            )
            .map_err(|_| EnrollmentError::IdentityMismatch)?,
            proposer_transport_x25519,
            role,
            capabilities,
            created_at: now,
            expires_at: now
                .checked_add(lifetime_seconds)
                .ok_or(EnrollmentError::Invalid)?,
            code_hash,
            signature: [0; SIGNATURE_BYTES],
        };
        let unsigned = request.unsigned_bytes()?;
        let signature = identity
            .sign_enrollment(&unsigned)
            .map_err(|_| EnrollmentError::IdentityMismatch)?;
        request.signature = signature.to_bytes();
        request.verify(now)?;
        Ok(ManualEnrollmentOffer { request, code })
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, EnrollmentError> {
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(EnrollmentError::TooLarge);
        }
        let mut cursor = Cursor::new(bytes);
        if cursor.take(4)? != MAGIC || cursor.byte()? != VERSION {
            return Err(EnrollmentError::Invalid);
        }
        let pairing_id = cursor.array::<PAIRING_ID_BYTES>()?;
        let request_id = cursor.array::<REQUEST_ID_BYTES>()?;
        let proposer_node_id = cursor.text(NODE_ID_BYTES)?;
        let proposer_xonly = cursor.array::<IDENTITY_KEY_BYTES>()?;
        let proposer_transport_x25519 = cursor.array::<TRANSPORT_KEY_BYTES>()?;
        validate_x25519_public(&proposer_transport_x25519).map_err(|_| EnrollmentError::Invalid)?;
        let role = EnrollmentRole::from_u8(cursor.byte()?)?;
        let capability_count = usize::from(cursor.byte()?);
        if capability_count > MAX_CAPABILITIES {
            return Err(EnrollmentError::Invalid);
        }
        let mut capabilities = Vec::with_capacity(capability_count);
        for _ in 0..capability_count {
            let length = usize::from(cursor.u16()?);
            if length == 0 || length > MAX_CAPABILITY_BYTES {
                return Err(EnrollmentError::Invalid);
            }
            capabilities.push(cursor.text(length)?);
        }
        let created_at = cursor.u64()?;
        let expires_at = cursor.u64()?;
        let code_hash = cursor.array::<32>()?;
        let signature = cursor.array::<SIGNATURE_BYTES>()?;
        if cursor.remaining() != 0 {
            return Err(EnrollmentError::Invalid);
        }
        let request = Self {
            pairing_id,
            request_id,
            proposer_node_id,
            proposer_xonly,
            proposer_transport_x25519,
            role,
            capabilities,
            created_at,
            expires_at,
            code_hash,
            signature,
        };
        request.validate_shape()?;
        Ok(request)
    }

    pub fn encode(&self) -> Vec<u8> {
        self.unsigned_bytes()
            .expect("manual enrollment request must be valid")
            .into_iter()
            .chain(self.signature)
            .collect()
    }

    pub fn verify(&self, now: u64) -> Result<(), EnrollmentError> {
        self.validate_shape()?;
        if now.saturating_add(FUTURE_SKEW_SECONDS) < self.created_at || now >= self.expires_at {
            return Err(EnrollmentError::Expired);
        }
        let key = VerifyingKey::from_slice(&self.proposer_xonly)
            .map_err(|_| EnrollmentError::IdentityMismatch)?;
        let digest = hash_domain(&self.unsigned_bytes()?, DOMAIN);
        let signature =
            Signature::from_slice(&self.signature).map_err(|_| EnrollmentError::Invalid)?;
        key.verify_prehash(&digest, &signature)
            .map_err(|_| EnrollmentError::IdentityMismatch)
    }

    pub fn verify_code(&self, code: &[u8]) -> Result<(), EnrollmentError> {
        if code.len() != CODE_BYTES || hash_code(code).ct_eq(&self.code_hash).unwrap_u8() != 1 {
            return Err(EnrollmentError::Invalid);
        }
        Ok(())
    }

    pub fn replay_expiry(&self) -> u64 {
        self.expires_at.saturating_add(REPLAY_RETENTION_SECONDS)
    }

    pub fn public_key_hex(&self) -> String {
        hex(&self.proposer_xonly)
    }

    fn unsigned_bytes(&self) -> Result<Vec<u8>, EnrollmentError> {
        self.validate_shape()?;
        let mut bytes = Vec::with_capacity(MAX_REQUEST_BYTES.min(512));
        bytes.extend_from_slice(MAGIC);
        bytes.push(VERSION);
        bytes.extend_from_slice(&self.pairing_id);
        bytes.extend_from_slice(&self.request_id);
        bytes.extend_from_slice(self.proposer_node_id.as_bytes());
        bytes.extend_from_slice(&self.proposer_xonly);
        bytes.extend_from_slice(&self.proposer_transport_x25519);
        bytes.push(self.role as u8);
        bytes.push(u8::try_from(self.capabilities.len()).map_err(|_| EnrollmentError::Invalid)?);
        for capability in &self.capabilities {
            let length = u16::try_from(capability.len()).map_err(|_| EnrollmentError::Invalid)?;
            bytes.extend_from_slice(&length.to_be_bytes());
            bytes.extend_from_slice(capability.as_bytes());
        }
        bytes.extend_from_slice(&self.created_at.to_be_bytes());
        bytes.extend_from_slice(&self.expires_at.to_be_bytes());
        bytes.extend_from_slice(&self.code_hash);
        if bytes.len() + SIGNATURE_BYTES > MAX_REQUEST_BYTES {
            return Err(EnrollmentError::TooLarge);
        }
        Ok(bytes)
    }

    fn validate_shape(&self) -> Result<(), EnrollmentError> {
        if self.pairing_id == [0; PAIRING_ID_BYTES] {
            return Err(EnrollmentError::Invalid);
        }
        if self.proposer_node_id.len() != NODE_ID_BYTES
            || !self.proposer_node_id.starts_with("omk1_")
            || !self.proposer_node_id[5..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || crate::node_identity::node_id_for_x_only_public_key(&self.proposer_xonly)
                != self.proposer_node_id
        {
            return Err(EnrollmentError::IdentityMismatch);
        }
        validate_x25519_public(&self.proposer_transport_x25519)
            .map_err(|_| EnrollmentError::Invalid)?;
        if self.expires_at <= self.created_at
            || self.expires_at - self.created_at > MAX_LIFETIME_SECONDS
        {
            return Err(EnrollmentError::Invalid);
        }
        validate_capabilities(&self.capabilities)?;
        if VerifyingKey::from_slice(&self.proposer_xonly).is_err() {
            return Err(EnrollmentError::IdentityMismatch);
        }
        Ok(())
    }
}

pub fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub fn parse_hex(value: &str, expected_bytes: usize) -> Result<Vec<u8>, EnrollmentError> {
    if value.len() != expected_bytes * 2
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(EnrollmentError::Invalid);
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16).map_err(|_| EnrollmentError::Invalid)
        })
        .collect()
}

pub fn hex_bytes(bytes: &[u8]) -> String {
    hex(bytes)
}

pub fn validate_capabilities(capabilities: &[String]) -> Result<(), EnrollmentError> {
    if capabilities.len() > MAX_CAPABILITIES {
        return Err(EnrollmentError::Invalid);
    }
    let mut previous = None;
    for capability in capabilities {
        if capability.is_empty()
            || capability.len() > MAX_CAPABILITY_BYTES
            || capability.bytes().any(|byte| {
                !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte))
            })
            || !SUPPORTED_CAPABILITIES.contains(&capability.as_str())
            || previous.is_some_and(|previous: &str| previous >= capability.as_str())
        {
            return Err(EnrollmentError::Invalid);
        }
        previous = Some(capability.as_str());
    }
    Ok(())
}

pub fn hash_code(code: &[u8]) -> [u8; 32] {
    hash_domain(code, DOMAIN)
}

fn hash_domain(bytes: &[u8], domain: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes);
    digest.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], EnrollmentError> {
    parse_hex(value, N)?
        .try_into()
        .map_err(|_| EnrollmentError::Invalid)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], EnrollmentError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(EnrollmentError::Invalid)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(EnrollmentError::Invalid)?;
        self.offset = end;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8, EnrollmentError> {
        Ok(*self.take(1)?.first().ok_or(EnrollmentError::Invalid)?)
    }

    fn u16(&mut self) -> Result<u16, EnrollmentError> {
        Ok(u16::from_be_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| EnrollmentError::Invalid)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, EnrollmentError> {
        Ok(u64::from_be_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| EnrollmentError::Invalid)?,
        ))
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], EnrollmentError> {
        self.take(N)?
            .try_into()
            .map_err(|_| EnrollmentError::Invalid)
    }

    fn text(&mut self, length: usize) -> Result<String, EnrollmentError> {
        let value =
            std::str::from_utf8(self.take(length)?).map_err(|_| EnrollmentError::Invalid)?;
        if value
            .chars()
            .any(|character| character == '\0' || character.is_control())
        {
            return Err(EnrollmentError::Invalid);
        }
        Ok(value.to_string())
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NodeContext, NodePathOverrides, NodePlatform};
    use crate::node_identity::NodeIdentity;
    use tempfile::TempDir;

    fn identity() -> (TempDir, NodeIdentity) {
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
        (temp, identity)
    }

    #[test]
    fn code_hash_matches_public_vector() {
        let code: Vec<u8> = (0..16).collect();
        assert_eq!(
            hex(&hash_code(&code)),
            "e9380fb38041d9a4cb70fbca9631da6d796fea839738fbd6e7015d829ccd54f7"
        );
    }

    #[test]
    fn request_round_trips_and_rejects_trailing_bytes() {
        let (_temp, identity) = identity();
        let offer = ManualEnrollmentRequest::create(
            &identity,
            [7; 32],
            EnrollmentRole::Performer,
            vec!["remote-run".into()],
            1_700_000_000,
            600,
        )
        .unwrap();
        let bytes = offer.request.encode();
        let parsed = ManualEnrollmentRequest::decode(&bytes).unwrap();
        parsed.verify(1_700_000_001).unwrap();
        parsed.verify_code(&offer.code).unwrap();
        assert!(ManualEnrollmentRequest::decode(&[bytes, vec![0]].concat()).is_err());
    }

    #[test]
    fn request_rejects_identity_mismatch_and_expiry() {
        let (_temp, identity) = identity();
        let offer = ManualEnrollmentRequest::create(
            &identity,
            [7; 32],
            EnrollmentRole::Performer,
            vec!["remote-run".into()],
            1_700_000_000,
            600,
        )
        .unwrap();
        let mut bytes = offer.request.encode();
        bytes[5 + 16 + 16] ^= 1;
        assert!(matches!(
            ManualEnrollmentRequest::decode(&bytes),
            Err(EnrollmentError::IdentityMismatch)
        ));
        assert!(matches!(
            offer.request.verify(1_700_000_600),
            Err(EnrollmentError::Expired)
        ));
    }

    #[test]
    fn signed_bundle_is_canonical_and_binds_authority_audience_and_certificate() {
        let manager_temp = TempDir::new().unwrap();
        let manager_context = NodeContext::resolve_for(
            NodePlatform::current(),
            NodePathOverrides::new(
                Some(manager_temp.path().join("state")),
                Some(manager_temp.path().join("node.toml")),
            ),
            true,
            None,
            None,
            None,
        )
        .unwrap();
        let manager = NodeIdentity::load_or_initialize(&manager_context).unwrap();
        let manager_transport =
            crate::node_transport::LocalTransport::provision_new(&manager_context, &manager)
                .unwrap();
        let target_temp = TempDir::new().unwrap();
        let target_context = NodeContext::resolve_for(
            NodePlatform::current(),
            NodePathOverrides::new(
                Some(target_temp.path().join("state")),
                Some(target_temp.path().join("node.toml")),
            ),
            true,
            None,
            None,
            None,
        )
        .unwrap();
        let target = NodeIdentity::load_or_initialize(&target_context).unwrap();
        let authority_private = [2_u8; 32];
        let authority_key = SigningKey::from_slice(&authority_private).unwrap();
        let bundle = SignedEnrollmentBundle::sign_with_material(
            &authority_private,
            [7; REQUEST_ID_BYTES],
            [8; BUNDLE_AUTHORITY_ID_BYTES],
            "omakure".to_string(),
            target.public_status().node_id.clone(),
            manager.public_status().node_id.clone(),
            decode_hex(&manager.public_status().public_key_hex).unwrap(),
            *manager_transport.certificate().transport_public(),
            *manager_transport.certificate().as_bytes(),
            EnrollmentRole::Conductor,
            vec!["baseline-push".to_string(), "remote-run".to_string()],
            1_700_000_000,
            1_700_000_600,
        )
        .unwrap();
        assert_eq!(bundle.encode().len(), 604);
        let parsed = SignedEnrollmentBundle::decode(&bundle.encode()).unwrap();
        parsed
            .verify(
                &BundleAuthority {
                    key_id: [8; BUNDLE_AUTHORITY_ID_BYTES],
                    public_key: authority_key.verifying_key().to_bytes().into(),
                    revoked: false,
                },
                "omakure",
                target.public_status().node_id.as_str(),
                1_700_000_001,
            )
            .unwrap();
        assert!(matches!(
            parsed.verify(
                &BundleAuthority {
                    key_id: [8; BUNDLE_AUTHORITY_ID_BYTES],
                    public_key: authority_key.verifying_key().to_bytes().into(),
                    revoked: false,
                },
                "other-org",
                target.public_status().node_id.as_str(),
                1_700_000_001,
            ),
            Err(EnrollmentError::OrganizationMismatch)
        ));
        assert!(matches!(
            parsed.verify(
                &BundleAuthority {
                    key_id: [8; BUNDLE_AUTHORITY_ID_BYTES],
                    public_key: authority_key.verifying_key().to_bytes().into(),
                    revoked: true,
                },
                "omakure",
                target.public_status().node_id.as_str(),
                1_700_000_001,
            ),
            Err(EnrollmentError::AuthorityRevoked)
        ));
        let mut altered = bundle.encode();
        altered[117] ^= 1;
        assert!(SignedEnrollmentBundle::decode(&altered).is_err());
    }
}

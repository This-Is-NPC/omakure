use super::*;
use crate::domain::{NODE_ID_BYTES, is_node_id};
use crate::node_identity::NodeIdentity;
use crate::util::hex;
use k256::schnorr::{Signature, VerifyingKey, signature::hazmat::PrehashVerifier};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beacon {
    pub node_id: String,
    pub identity_xonly: [u8; IDENTITY_BYTES],
    pub beacon_id: [u8; BEACON_ID_BYTES],
    pub direct_port: u16,
    pub issued_at: u64,
    pub expires_at: u64,
    pub sequence: u64,
    pub discovery_proof: Option<[u8; PROOF_BYTES]>,
    pub signature: [u8; SIGNATURE_BYTES],
}

impl Beacon {
    pub fn create(
        identity: &NodeIdentity,
        direct_port: u16,
        beacon_id: [u8; BEACON_ID_BYTES],
        sequence: u64,
        issued_at: u64,
        secret: Option<&[u8]>,
    ) -> Result<Self, DiscoveryError> {
        if direct_port == 0 {
            return Err(DiscoveryError::InvalidBeacon);
        }
        validate_secret(secret)?;
        let identity_key = hex::decode(&identity.public_status().public_key_hex)
            .ok_or(DiscoveryError::IdentityMismatch)?;
        let identity_xonly: [u8; IDENTITY_BYTES] = identity_key
            .try_into()
            .map_err(|_| DiscoveryError::IdentityMismatch)?;
        let expires_at = issued_at.saturating_add(BEACON_LIFETIME_SECONDS);
        let node_id = identity.public_status().node_id.clone();
        let mut beacon = Self {
            node_id,
            identity_xonly,
            beacon_id,
            direct_port,
            issued_at,
            expires_at,
            sequence,
            discovery_proof: None,
            signature: [0; SIGNATURE_BYTES],
        };
        if let Some(secret) = secret {
            // The proof is computed over the final flags value, before the
            // proof bytes themselves are appended to the signed body.
            beacon.discovery_proof = Some([0; PROOF_BYTES]);
            beacon.discovery_proof = Some(hmac_sha256(secret, &beacon.proof_input()));
        }
        let signature = identity
            .sign_discovery(&beacon.unsigned_bytes())
            .map_err(|_| DiscoveryError::Internal)?;
        beacon.signature = signature.to_bytes();
        Ok(beacon)
    }

    pub fn encode(&self) -> Result<Vec<u8>, DiscoveryError> {
        self.validate_shape()?;
        let mut encoded = self.unsigned_bytes();
        encoded.extend_from_slice(&self.signature);
        Ok(encoded)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, DiscoveryError> {
        if bytes.len() > MAX_DATAGRAM_BYTES {
            return Err(DiscoveryError::MessageTooLarge);
        }
        if bytes.len() < HEADER_BYTES {
            return Err(DiscoveryError::InvalidBeacon);
        }
        if &bytes[..4] != BEACON_MAGIC {
            return Err(DiscoveryError::InvalidBeacon);
        }
        if bytes[4] != BEACON_VERSION {
            return Err(DiscoveryError::UnsupportedVersion);
        }
        if bytes[5] != BEACON_KIND {
            return Err(DiscoveryError::InvalidBeacon);
        }
        let flags = u16::from_be_bytes([bytes[6], bytes[7]]);
        if flags & !PROOF_FLAG != 0 {
            return Err(DiscoveryError::InvalidBeacon);
        }
        let expected =
            UNSIGNED_BYTES + SIGNATURE_BYTES + usize::from(flags == PROOF_FLAG) * PROOF_BYTES;
        if bytes.len() != expected {
            return Err(if bytes.len() > MAX_BEACON_BYTES_WITH_PROOF {
                DiscoveryError::MessageTooLarge
            } else {
                DiscoveryError::InvalidBeacon
            });
        }
        let mut cursor = HEADER_BYTES;
        let node_id_bytes = &bytes[cursor..cursor + NODE_ID_BYTES];
        cursor += NODE_ID_BYTES;
        let node_id = std::str::from_utf8(node_id_bytes)
            .map_err(|_| DiscoveryError::InvalidBeacon)?
            .to_string();
        if !is_node_id(&node_id) {
            return Err(DiscoveryError::InvalidBeacon);
        }
        let identity_xonly: [u8; IDENTITY_BYTES] = bytes[cursor..cursor + IDENTITY_BYTES]
            .try_into()
            .map_err(|_| DiscoveryError::InvalidBeacon)?;
        cursor += IDENTITY_BYTES;
        let beacon_id: [u8; BEACON_ID_BYTES] = bytes[cursor..cursor + BEACON_ID_BYTES]
            .try_into()
            .map_err(|_| DiscoveryError::InvalidBeacon)?;
        cursor += BEACON_ID_BYTES;
        let direct_port = u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]);
        cursor += 2;
        let issued_at = read_u64(bytes, &mut cursor)?;
        let expires_at = read_u64(bytes, &mut cursor)?;
        let sequence = read_u64(bytes, &mut cursor)?;
        let discovery_proof = if flags == PROOF_FLAG {
            let proof: [u8; PROOF_BYTES] = bytes[cursor..cursor + PROOF_BYTES]
                .try_into()
                .map_err(|_| DiscoveryError::InvalidBeacon)?;
            cursor += PROOF_BYTES;
            Some(proof)
        } else {
            None
        };
        let signature: [u8; SIGNATURE_BYTES] = bytes[cursor..cursor + SIGNATURE_BYTES]
            .try_into()
            .map_err(|_| DiscoveryError::InvalidBeacon)?;
        let beacon = Self {
            node_id,
            identity_xonly,
            beacon_id,
            direct_port,
            issued_at,
            expires_at,
            sequence,
            discovery_proof,
            signature,
        };
        beacon.validate_shape()?;
        Ok(beacon)
    }

    pub fn verify(&self, now: u64, secret: Option<&[u8]>) -> Result<(), DiscoveryError> {
        self.validate_shape()?;
        validate_secret(secret)?;
        if self.issued_at > now.saturating_add(FUTURE_SKEW_SECONDS) {
            return Err(DiscoveryError::Future);
        }
        if now >= self.expires_at {
            return Err(DiscoveryError::Expired);
        }
        if let Some(secret) = secret {
            let proof = self.discovery_proof.ok_or(DiscoveryError::SecretMismatch)?;
            if !bool::from(proof.ct_eq(&hmac_sha256(secret, &self.proof_input()))) {
                return Err(DiscoveryError::SecretMismatch);
            }
        } else if self.discovery_proof.is_some() {
            return Err(DiscoveryError::SecretMismatch);
        }
        let expected_node_id =
            crate::node_identity::node_id_for_x_only_public_key(&self.identity_xonly);
        if self.node_id != expected_node_id {
            return Err(DiscoveryError::IdentityMismatch);
        }
        let key = VerifyingKey::from_slice(&self.identity_xonly)
            .map_err(|_| DiscoveryError::IdentityMismatch)?;
        let signature =
            Signature::from_slice(&self.signature).map_err(|_| DiscoveryError::SignatureInvalid)?;
        let digest =
            Sha256::digest([BEACON_SIGNATURE_DOMAIN, self.unsigned_bytes().as_slice()].concat());
        key.verify_prehash(&digest, &signature)
            .map_err(|_| DiscoveryError::SignatureInvalid)
    }

    fn validate_shape(&self) -> Result<(), DiscoveryError> {
        if !is_node_id(&self.node_id)
            || self.direct_port == 0
            || self.expires_at <= self.issued_at
            || self.expires_at - self.issued_at > BEACON_LIFETIME_SECONDS
            || self
                .discovery_proof
                .is_some_and(|proof| proof == [0; PROOF_BYTES])
        {
            return Err(DiscoveryError::InvalidBeacon);
        }
        if crate::node_identity::node_id_for_x_only_public_key(&self.identity_xonly) != self.node_id
        {
            return Err(DiscoveryError::IdentityMismatch);
        }
        Ok(())
    }

    fn unsigned_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(UNSIGNED_BYTES + PROOF_BYTES);
        bytes.extend_from_slice(BEACON_MAGIC);
        bytes.push(BEACON_VERSION);
        bytes.push(BEACON_KIND);
        bytes.extend_from_slice(&(u16::from(self.discovery_proof.is_some())).to_be_bytes());
        bytes.extend_from_slice(self.node_id.as_bytes());
        bytes.extend_from_slice(&self.identity_xonly);
        bytes.extend_from_slice(&self.beacon_id);
        bytes.extend_from_slice(&self.direct_port.to_be_bytes());
        bytes.extend_from_slice(&self.issued_at.to_be_bytes());
        bytes.extend_from_slice(&self.expires_at.to_be_bytes());
        bytes.extend_from_slice(&self.sequence.to_be_bytes());
        if let Some(proof) = self.discovery_proof {
            bytes.extend_from_slice(&proof);
        }
        bytes
    }

    fn proof_input(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(UNSIGNED_BYTES);
        bytes.extend_from_slice(DISCOVERY_PROOF_DOMAIN);
        let unsigned = self.unsigned_bytes();
        bytes.extend_from_slice(&unsigned[..UNSIGNED_BYTES]);
        bytes
    }
}

pub(super) fn validate_secret(secret: Option<&[u8]>) -> Result<(), DiscoveryError> {
    if secret.is_some_and(|value| value.is_empty() || value.len() > MAX_DISCOVERY_SECRET_BYTES) {
        return Err(DiscoveryError::SecretInvalid);
    }
    Ok(())
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, DiscoveryError> {
    let end = cursor.checked_add(8).ok_or(DiscoveryError::InvalidBeacon)?;
    let value = bytes
        .get(*cursor..end)
        .ok_or(DiscoveryError::InvalidBeacon)?;
    *cursor = end;
    Ok(u64::from_be_bytes(
        value
            .try_into()
            .map_err(|_| DiscoveryError::InvalidBeacon)?,
    ))
}

fn hmac_sha256(secret: &[u8], message: &[u8]) -> [u8; 32] {
    let mut key = [0_u8; 64];
    if secret.len() > key.len() {
        key[..32].copy_from_slice(&Sha256::digest(secret));
    } else {
        key[..secret.len()].copy_from_slice(secret);
    }
    let mut inner = [0x36_u8; 64];
    let mut outer = [0x5c_u8; 64];
    for index in 0..64 {
        inner[index] ^= key[index];
        outer[index] ^= key[index];
    }
    let mut inner_input = Vec::with_capacity(64 + message.len());
    inner_input.extend_from_slice(&inner);
    inner_input.extend_from_slice(message);
    let inner_hash = Sha256::digest(inner_input);
    let mut outer_input = Vec::with_capacity(64 + 32);
    outer_input.extend_from_slice(&outer);
    outer_input.extend_from_slice(&inner_hash);
    Sha256::digest(outer_input).into()
}

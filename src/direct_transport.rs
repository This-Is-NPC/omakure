//! Protocol-neutral direct transport primitives.
//!
//! This module owns bytes, cryptographic state, and authorization decisions but
//! deliberately does not own sockets, threads, or SQLite.  The node service
//! adapter is responsible for those effects.

use crate::node_identity::{Bip340Signature, DirectEnvelopePrehash, NodeIdentity};
use crate::node_registry::PeerState;
use curve25519_dalek::{constants::X25519_BASEPOINT, montgomery::MontgomeryPoint};
use k256::schnorr::{signature::hazmat::PrehashVerifier, Signature, VerifyingKey};
use rand::rngs::OsRng;
use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};
use snow::{params::NoiseParams, Builder, HandshakeState, TransportState};
use std::cmp::Ordering;
use std::fmt;
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const CONTRACT_ID: &[u8] = b"omakure/direct-transport/v1";
pub const PROLOGUE: &[u8] = b"omakure/direct-transport/v1\0";
pub const CERTIFICATE_DOMAIN: &[u8] = b"omakure/transport-cert/v1\0";
pub const DIRECT_ENVELOPE_DOMAIN: &[u8] = b"omakure/direct-envelope/v1\0";
pub const NOISE_NAME: &str = "Noise_XX_25519_ChaChaPoly_SHA256";

pub const MAX_FRAME_LENGTH: usize = 1_048_580;
pub const MAX_FRAME_BYTES: usize = 1_048_584;
pub const MAX_HANDSHAKE_MESSAGE_BYTES: usize = 4_096;
pub const MAX_PLAINTEXT_BYTES: usize = 1_048_520;
pub const MAX_CERTIFICATE_BYTES: usize = 245;
pub const CERTIFICATE_BODY_BYTES: usize = 181;
pub const CERTIFICATE_FUTURE_SKEW_SECONDS: u64 = 300;
pub const CERTIFICATE_MAX_LIFETIME_SECONDS: u64 = 63_072_000;
pub const REKEY_MESSAGES: u64 = 1_048_576;
pub const REKEY_PLAINTEXT_BYTES: u64 = 1_073_741_824;

const CERT_MAGIC: &[u8; 4] = b"OMTC";
const FRAME_VERSION: u8 = 1;
const HANDSHAKE_KIND: u8 = 1;
const ENCRYPTED_KIND: u8 = 2;
pub const ENVELOPE_KIND: u8 = 1;
const CLOSE_KIND: u8 = 2;
const ERROR_KIND: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ProtocolErrorCode {
    UnsupportedVersion = 1001,
    InvalidFrame = 1002,
    MessageTooLarge = 1003,
    HandshakeFailed = 1004,
    IdentityMismatch = 1005,
    NotEnrolled = 1006,
    Revoked = 1007,
    Expired = 1008,
    Replay = 1009,
    RateLimited = 1010,
    Internal = 1011,
}

impl ProtocolErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedVersion => "unsupported_version",
            Self::InvalidFrame => "invalid_frame",
            Self::MessageTooLarge => "message_too_large",
            Self::HandshakeFailed => "handshake_failed",
            Self::IdentityMismatch => "identity_mismatch",
            Self::NotEnrolled => "not_enrolled",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
            Self::Replay => "replay",
            Self::RateLimited => "rate_limited",
            Self::Internal => "internal",
        }
    }
}

impl fmt::Display for ProtocolErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TransportError {
    #[error("unsupported_version")]
    UnsupportedVersion,
    #[error("invalid_frame")]
    InvalidFrame,
    #[error("message_too_large")]
    MessageTooLarge,
    #[error("handshake_failed")]
    HandshakeFailed,
    #[error("identity_mismatch")]
    IdentityMismatch,
    #[error("not_enrolled")]
    NotEnrolled,
    #[error("revoked")]
    Revoked,
    #[error("expired")]
    Expired,
    #[error("replay")]
    Replay,
    #[error("rate_limited")]
    RateLimited,
    #[error("internal")]
    Internal,
}

impl TransportError {
    pub const fn code(&self) -> ProtocolErrorCode {
        match self {
            Self::UnsupportedVersion => ProtocolErrorCode::UnsupportedVersion,
            Self::InvalidFrame => ProtocolErrorCode::InvalidFrame,
            Self::MessageTooLarge => ProtocolErrorCode::MessageTooLarge,
            Self::HandshakeFailed => ProtocolErrorCode::HandshakeFailed,
            Self::IdentityMismatch => ProtocolErrorCode::IdentityMismatch,
            Self::NotEnrolled => ProtocolErrorCode::NotEnrolled,
            Self::Revoked => ProtocolErrorCode::Revoked,
            Self::Expired => ProtocolErrorCode::Expired,
            Self::Replay => ProtocolErrorCode::Replay,
            Self::RateLimited => ProtocolErrorCode::RateLimited,
            Self::Internal => ProtocolErrorCode::Internal,
        }
    }
}

impl From<io::Error> for TransportError {
    fn from(_: io::Error) -> Self {
        Self::Internal
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub version: u8,
    pub kind: u8,
    pub flags: u16,
    pub body: Vec<u8>,
}

impl Frame {
    pub fn handshake(message_number: u8, message: &[u8]) -> Result<Self, TransportError> {
        if !(1..=3).contains(&message_number) {
            return Err(TransportError::InvalidFrame);
        }
        if message.len() > MAX_HANDSHAKE_MESSAGE_BYTES {
            return Err(TransportError::MessageTooLarge);
        }
        let mut body = Vec::with_capacity(message.len() + 1);
        body.push(message_number);
        body.extend_from_slice(message);
        Ok(Self {
            version: FRAME_VERSION,
            kind: HANDSHAKE_KIND,
            flags: 0,
            body,
        })
    }

    pub fn encrypted(session_id: [u8; 32], ciphertext: &[u8]) -> Result<Self, TransportError> {
        let body_len = 32usize
            .checked_add(ciphertext.len())
            .ok_or(TransportError::MessageTooLarge)?;
        if body_len + 4 > MAX_FRAME_LENGTH {
            return Err(TransportError::MessageTooLarge);
        }
        let mut body = Vec::with_capacity(body_len);
        body.extend_from_slice(&session_id);
        body.extend_from_slice(ciphertext);
        Ok(Self {
            version: FRAME_VERSION,
            kind: ENCRYPTED_KIND,
            flags: 0,
            body,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, TransportError> {
        if self.version != FRAME_VERSION {
            return Err(TransportError::UnsupportedVersion);
        }
        if self.flags != 0 || !matches!(self.kind, HANDSHAKE_KIND | ENCRYPTED_KIND) {
            return Err(TransportError::InvalidFrame);
        }
        let length = 4usize
            .checked_add(self.body.len())
            .ok_or(TransportError::MessageTooLarge)?;
        if !(4..=MAX_FRAME_LENGTH).contains(&length) {
            return Err(TransportError::MessageTooLarge);
        }
        if self.kind == HANDSHAKE_KIND
            && (self.body.is_empty() || self.body.len() - 1 > MAX_HANDSHAKE_MESSAGE_BYTES)
        {
            return Err(if self.body.len() > MAX_HANDSHAKE_MESSAGE_BYTES + 1 {
                TransportError::MessageTooLarge
            } else {
                TransportError::InvalidFrame
            });
        }
        let mut encoded = Vec::with_capacity(length + 4);
        encoded.extend_from_slice(&(length as u32).to_be_bytes());
        encoded.push(self.version);
        encoded.push(self.kind);
        encoded.extend_from_slice(&self.flags.to_be_bytes());
        encoded.extend_from_slice(&self.body);
        Ok(encoded)
    }

    pub fn parse(encoded: &[u8]) -> Result<Self, TransportError> {
        if encoded.len() < 4 {
            return Err(TransportError::InvalidFrame);
        }
        let length = u32::from_be_bytes(encoded[..4].try_into().unwrap()) as usize;
        if length < 4 {
            return Err(TransportError::InvalidFrame);
        }
        if length > MAX_FRAME_LENGTH {
            return Err(TransportError::MessageTooLarge);
        }
        if encoded.len() != length + 4 {
            return Err(TransportError::InvalidFrame);
        }
        let version = encoded[4];
        if version != FRAME_VERSION {
            return Err(TransportError::UnsupportedVersion);
        }
        let kind = encoded[5];
        if !matches!(kind, HANDSHAKE_KIND | ENCRYPTED_KIND) {
            return Err(TransportError::InvalidFrame);
        }
        let flags = u16::from_be_bytes(encoded[6..8].try_into().unwrap());
        if flags != 0 {
            return Err(TransportError::InvalidFrame);
        }
        let body = encoded[8..].to_vec();
        if kind == HANDSHAKE_KIND {
            if body.is_empty() || !(1..=3).contains(&body[0]) {
                return Err(TransportError::InvalidFrame);
            }
            if body.len() - 1 > MAX_HANDSHAKE_MESSAGE_BYTES {
                return Err(TransportError::MessageTooLarge);
            }
        } else if body.len() < 32 + 16 {
            return Err(TransportError::InvalidFrame);
        }
        Ok(Self {
            version,
            kind,
            flags,
            body,
        })
    }

    pub fn message_number(&self) -> Result<u8, TransportError> {
        if self.kind != HANDSHAKE_KIND || self.body.is_empty() {
            return Err(TransportError::InvalidFrame);
        }
        Ok(self.body[0])
    }
}

pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub fn x25519_public_from_private(private: &[u8]) -> Result<[u8; 32], TransportError> {
    let scalar: [u8; 32] = private.try_into().map_err(|_| TransportError::Internal)?;
    let public = X25519_BASEPOINT.mul_clamped(scalar).to_bytes();
    validate_x25519_public(&public)?;
    Ok(public)
}

pub fn x25519_probe(private: &[u8], public: &[u8]) -> Result<[u8; 32], TransportError> {
    let scalar: [u8; 32] = private.try_into().map_err(|_| TransportError::Internal)?;
    let public: [u8; 32] = public
        .try_into()
        .map_err(|_| TransportError::HandshakeFailed)?;
    validate_x25519_public(&public)?;
    let shared = MontgomeryPoint(public).mul_clamped(scalar).to_bytes();
    if shared.iter().fold(0u8, |value, byte| value | byte) == 0 {
        return Err(TransportError::HandshakeFailed);
    }
    Ok(shared)
}

pub fn validate_x25519_public(public: &[u8]) -> Result<(), TransportError> {
    let public: [u8; 32] = public
        .try_into()
        .map_err(|_| TransportError::HandshakeFailed)?;
    if prohibited_x25519_public(&public) {
        return Err(TransportError::HandshakeFailed);
    }
    Ok(())
}

fn prohibited_x25519_public(public: &[u8; 32]) -> bool {
    prohibited_x25519_public_keys()
        .iter()
        .fold(0u8, |found, candidate| {
            let difference = candidate
                .iter()
                .zip(public)
                .fold(0u8, |value, (left, right)| value | (left ^ right));
            found | u8::from(difference == 0)
        })
        != 0
}

pub fn prohibited_x25519_public_keys() -> [[u8; 32]; 7] {
    [
        [0; 32],
        [
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ],
        hex_array("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800"),
        hex_array("5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157"),
        hex_array("ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"),
        hex_array("edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"),
        hex_array("eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"),
    ]
}

const fn hex_array(value: &str) -> [u8; 32] {
    let bytes = value.as_bytes();
    let mut output = [0u8; 32];
    let mut index = 0;
    while index < 32 {
        output[index] = (hex_nibble(bytes[index * 2]) << 4) | hex_nibble(bytes[index * 2 + 1]);
        index += 1;
    }
    output
}

const fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        _ => 0,
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct TransportCertificate {
    bytes: [u8; MAX_CERTIFICATE_BYTES],
}

impl fmt::Debug for TransportCertificate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransportCertificate")
            .field("identity_key", &hex(&self.bytes[8..40]))
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
        let key = decode_hex(&status.public_key_hex).ok_or(TransportError::IdentityMismatch)?;
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
        let digest = domain_hash(CERTIFICATE_DOMAIN, &bytes[..CERTIFICATE_BODY_BYTES]);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeRole {
    Initiator,
    Responder,
}

pub struct NoiseHandshake {
    state: HandshakeState,
    staged_state: HandshakeState,
    role: HandshakeRole,
    local_private: [u8; 32],
    local_certificate: TransportCertificate,
    remote_certificate: Option<TransportCertificate>,
    expected_message: u8,
}

impl NoiseHandshake {
    pub fn new(
        role: HandshakeRole,
        local_private: [u8; 32],
        local_certificate: TransportCertificate,
    ) -> Result<Self, TransportError> {
        let local_public = x25519_public_from_private(&local_private)?;
        if local_public != *local_certificate.transport_public() {
            return Err(TransportError::IdentityMismatch);
        }
        let mut fixed_ephemeral = [0u8; 32];
        OsRng.fill_bytes(&mut fixed_ephemeral);
        let state = build_handshake_state(role, local_private, &fixed_ephemeral)?;
        let staged_state = build_handshake_state(role, local_private, &fixed_ephemeral)?;
        Ok(Self {
            state,
            staged_state,
            role,
            local_private,
            local_certificate,
            remote_certificate: None,
            expected_message: 1,
        })
    }

    pub fn write_next(&mut self) -> Result<Vec<u8>, TransportError> {
        if self.expected_message > 3 {
            return Err(TransportError::HandshakeFailed);
        }
        let message_number = self.expected_message;
        let payload = match (self.role, message_number) {
            (HandshakeRole::Initiator, 1) => Vec::new(),
            (HandshakeRole::Responder, 2) | (HandshakeRole::Initiator, 3) => {
                certificate_payload(&self.local_certificate)
            }
            _ => return Err(TransportError::HandshakeFailed),
        };
        let mut message = vec![0u8; MAX_HANDSHAKE_MESSAGE_BYTES];
        let staged_length = self
            .staged_state
            .write_message(&payload, &mut message)
            .map_err(|_| TransportError::HandshakeFailed)?;
        let mut committed_message = vec![0u8; MAX_HANDSHAKE_MESSAGE_BYTES];
        let committed_length = self
            .state
            .write_message(&payload, &mut committed_message)
            .map_err(|_| TransportError::HandshakeFailed)?;
        if staged_length != committed_length
            || message[..staged_length] != committed_message[..committed_length]
        {
            return Err(TransportError::HandshakeFailed);
        }
        message.truncate(staged_length);
        let frame = Frame::handshake(message_number, &message)?;
        self.expected_message += 1;
        frame.encode()
    }

    pub fn read_next(&mut self, encoded: &[u8], now: u64) -> Result<(), TransportError> {
        let frame = Frame::parse(encoded)?;
        if frame.kind != HANDSHAKE_KIND || frame.message_number()? != self.expected_message {
            return Err(TransportError::HandshakeFailed);
        }
        let message_number = frame.body[0];
        let message = &frame.body[1..];
        if message_number <= 2 {
            x25519_probe(
                &self.local_private,
                message.get(..32).ok_or(TransportError::HandshakeFailed)?,
            )?;
        }
        let mut payload = vec![0u8; MAX_HANDSHAKE_MESSAGE_BYTES];
        let length = self
            .state
            .read_message(message, &mut payload)
            .map_err(|_| TransportError::HandshakeFailed)?;
        let mut staged_payload = vec![0u8; MAX_HANDSHAKE_MESSAGE_BYTES];
        let staged_length = self
            .staged_state
            .read_message(message, &mut staged_payload)
            .map_err(|_| TransportError::HandshakeFailed)?;
        if length != staged_length || payload[..length] != staged_payload[..staged_length] {
            return Err(TransportError::HandshakeFailed);
        }
        if message_number == 1 {
            if length != 0 {
                return Err(TransportError::HandshakeFailed);
            }
        } else {
            let remote_static = self
                .state
                .get_remote_static()
                .ok_or(TransportError::HandshakeFailed)?;
            x25519_probe(&self.local_private, remote_static)?;
            let certificate = parse_certificate_payload(&payload[..length])?;
            certificate.verify_time(now)?;
            if certificate.transport_public() != remote_static {
                return Err(TransportError::IdentityMismatch);
            }
            self.remote_certificate = Some(certificate);
        }
        self.expected_message += 1;
        Ok(())
    }

    pub fn is_finished(&self) -> bool {
        self.state.is_handshake_finished()
    }

    pub fn remote_certificate(&self) -> Option<&TransportCertificate> {
        self.remote_certificate.as_ref()
    }

    pub fn handshake_hash(&self) -> [u8; 32] {
        self.state.get_handshake_hash().try_into().unwrap()
    }

    pub fn into_session(self) -> Result<TransportSession, TransportError> {
        if !self.is_finished() || self.remote_certificate.is_none() {
            return Err(TransportError::HandshakeFailed);
        }
        let session_id = self.handshake_hash();
        Ok(TransportSession {
            state: self
                .state
                .into_transport_mode()
                .map_err(|_| TransportError::HandshakeFailed)?,
            staged_state: self
                .staged_state
                .into_transport_mode()
                .map_err(|_| TransportError::HandshakeFailed)?,
            session_id,
            send_sequence: 0,
            receive_sequence: 0,
            sent_messages: 0,
            sent_bytes: 0,
            received_messages: 0,
            received_bytes: 0,
            closed: false,
            last_received_ciphertext: None,
            consecutive_write_failures: 0,
            #[cfg(test)]
            write_fault: None,
        })
    }
}

fn build_handshake_state(
    role: HandshakeRole,
    local_private: [u8; 32],
    fixed_ephemeral: &[u8; 32],
) -> Result<HandshakeState, TransportError> {
    let params: NoiseParams = NOISE_NAME.parse().map_err(|_| TransportError::Internal)?;
    let mut builder = Builder::new(params);
    builder = builder
        .prologue(PROLOGUE)
        .map_err(|_| TransportError::Internal)?
        .local_private_key(&local_private)
        .map_err(|_| TransportError::Internal)?
        .fixed_ephemeral_key_for_testing_only(fixed_ephemeral);
    match role {
        HandshakeRole::Initiator => builder
            .build_initiator()
            .map_err(|_| TransportError::HandshakeFailed),
        HandshakeRole::Responder => builder
            .build_responder()
            .map_err(|_| TransportError::HandshakeFailed),
    }
}

pub struct TransportSession {
    state: TransportState,
    staged_state: TransportState,
    session_id: [u8; 32],
    send_sequence: u64,
    receive_sequence: u64,
    sent_messages: u64,
    sent_bytes: u64,
    received_messages: u64,
    received_bytes: u64,
    closed: bool,
    last_received_ciphertext: Option<Vec<u8>>,
    consecutive_write_failures: u8,
    #[cfg(test)]
    write_fault: Option<WriteFault>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteFault {
    BeforeEncryption,
}

impl fmt::Debug for TransportSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransportSession")
            .field("session_id", &hex(&self.session_id))
            .field("send_sequence", &self.send_sequence)
            .field("receive_sequence", &self.receive_sequence)
            .field("closed", &self.closed)
            .finish()
    }
}

impl TransportSession {
    pub fn session_id(&self) -> &[u8; 32] {
        &self.session_id
    }

    pub fn write(&mut self, inner_kind: u8, body: &[u8]) -> Result<Vec<u8>, TransportError> {
        if self.closed {
            return Err(TransportError::HandshakeFailed);
        }
        let mut plaintext = Vec::with_capacity(10 + body.len());
        plaintext.extend_from_slice(&self.send_sequence.to_be_bytes());
        plaintext.extend_from_slice(&[inner_kind, 1]);
        plaintext.extend_from_slice(body);
        if plaintext.len() > MAX_PLAINTEXT_BYTES {
            return self.failed_write(TransportError::MessageTooLarge);
        }
        let next_sequence = match self.send_sequence.checked_add(1) {
            Some(sequence) => sequence,
            None => return self.failed_write(TransportError::HandshakeFailed),
        };
        if self.state.sending_nonce() == u64::MAX || self.staged_state.sending_nonce() == u64::MAX {
            return self.failed_write(TransportError::HandshakeFailed);
        }
        #[cfg(test)]
        if self.write_fault == Some(WriteFault::BeforeEncryption) {
            self.write_fault = None;
            return self.failed_write(TransportError::Internal);
        }
        let rekey = self.should_rekey(plaintext.len() as u64);
        let encoded = match write_state(&mut self.staged_state, self.session_id, &plaintext, rekey)
        {
            Ok(encoded) => encoded,
            Err(error) => {
                self.closed = true;
                return Err(error);
            }
        };
        let committed = match write_state(&mut self.state, self.session_id, &plaintext, rekey) {
            Ok(committed) => committed,
            Err(error) => {
                self.closed = true;
                return Err(error);
            }
        };
        if encoded != committed {
            self.closed = true;
            return Err(TransportError::Internal);
        }
        self.send_sequence = next_sequence;
        self.sent_messages += 1;
        self.sent_bytes = self.sent_bytes.saturating_add(plaintext.len() as u64);
        self.consecutive_write_failures = 0;
        Ok(encoded)
    }

    pub fn read(&mut self, encoded: &[u8]) -> Result<ReceivedMessage, TransportError> {
        if self.closed {
            return Err(TransportError::HandshakeFailed);
        }
        let frame = match Frame::parse(encoded) {
            Ok(frame) => frame,
            Err(error) => {
                self.closed = true;
                return Err(error);
            }
        };
        if frame.kind != ENCRYPTED_KIND || frame.body.get(..32) != Some(self.session_id.as_slice())
        {
            self.closed = true;
            return Err(TransportError::HandshakeFailed);
        }
        let ciphertext = &frame.body[32..];
        if self
            .last_received_ciphertext
            .as_deref()
            .is_some_and(|previous| previous == ciphertext)
        {
            self.closed = true;
            return Err(TransportError::Replay);
        }
        let rekey = self.should_rekey_incoming(ciphertext.len().saturating_sub(16) as u64);
        let plaintext = match read_state(&mut self.staged_state, ciphertext, rekey) {
            Ok(plaintext) => plaintext,
            Err(_) => {
                self.closed = true;
                return Err(TransportError::HandshakeFailed);
            }
        };
        let committed = match read_state(&mut self.state, ciphertext, rekey) {
            Ok(plaintext) => plaintext,
            Err(_) => {
                self.closed = true;
                return Err(TransportError::HandshakeFailed);
            }
        };
        if plaintext != committed {
            self.closed = true;
            return Err(TransportError::Internal);
        }
        let length = plaintext.len();
        if !(10..=MAX_PLAINTEXT_BYTES).contains(&length) {
            self.closed = true;
            return Err(TransportError::InvalidFrame);
        }
        let plaintext = &plaintext[..length];
        let sequence = u64::from_be_bytes(plaintext[..8].try_into().unwrap());
        match sequence.cmp(&self.receive_sequence) {
            Ordering::Less => {
                self.closed = true;
                return Err(TransportError::Replay);
            }
            Ordering::Greater => {
                self.closed = true;
                return Err(TransportError::InvalidFrame);
            }
            Ordering::Equal => {}
        }
        let kind = plaintext[8];
        if !matches!(kind, ENVELOPE_KIND | CLOSE_KIND | ERROR_KIND) || plaintext[9] != 1 {
            self.closed = true;
            return Err(TransportError::InvalidFrame);
        }
        let body = match kind {
            CLOSE_KIND | ERROR_KIND if plaintext.len() != 12 => {
                self.closed = true;
                return Err(TransportError::InvalidFrame);
            }
            _ => plaintext[10..].to_vec(),
        };
        self.receive_sequence = self.receive_sequence.checked_add(1).ok_or_else(|| {
            self.closed = true;
            TransportError::HandshakeFailed
        })?;
        self.received_messages += 1;
        self.received_bytes = self.received_bytes.saturating_add(length as u64);
        self.last_received_ciphertext = Some(ciphertext.to_vec());
        Ok(ReceivedMessage {
            sequence,
            kind,
            body,
        })
    }

    pub fn write_close(&mut self, reason: u16) -> Result<Vec<u8>, TransportError> {
        let frame = self.write(CLOSE_KIND, &reason.to_be_bytes())?;
        self.closed = true;
        Ok(frame)
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    #[cfg(test)]
    fn inject_write_fault(&mut self, fault: WriteFault) {
        self.write_fault = Some(fault);
    }

    fn failed_write(&mut self, error: TransportError) -> Result<Vec<u8>, TransportError> {
        self.consecutive_write_failures = self.consecutive_write_failures.saturating_add(1);
        if self.consecutive_write_failures >= 3 {
            self.closed = true;
        }
        Err(error)
    }

    fn should_rekey(&self, next_plaintext: u64) -> bool {
        self.sent_messages.saturating_add(1) >= REKEY_MESSAGES
            || self.sent_bytes.saturating_add(next_plaintext) >= REKEY_PLAINTEXT_BYTES
    }

    fn should_rekey_incoming(&self, next_ciphertext_bytes: u64) -> bool {
        self.received_messages.saturating_add(1) >= REKEY_MESSAGES
            || self.received_bytes.saturating_add(next_ciphertext_bytes) >= REKEY_PLAINTEXT_BYTES
    }
}

fn write_state(
    state: &mut TransportState,
    session_id: [u8; 32],
    plaintext: &[u8],
    rekey: bool,
) -> Result<Vec<u8>, TransportError> {
    if rekey {
        state.rekey_outgoing();
    }
    let mut ciphertext = vec![0u8; plaintext.len() + 16];
    let length = state
        .write_message(plaintext, &mut ciphertext)
        .map_err(|_| TransportError::Internal)?;
    ciphertext.truncate(length);
    Frame::encrypted(session_id, &ciphertext)?.encode()
}

fn read_state(
    state: &mut TransportState,
    ciphertext: &[u8],
    rekey: bool,
) -> Result<Vec<u8>, TransportError> {
    if rekey {
        state.rekey_incoming();
    }
    let mut plaintext = vec![0u8; ciphertext.len()];
    let length = state
        .read_message(ciphertext, &mut plaintext)
        .map_err(|_| TransportError::HandshakeFailed)?;
    plaintext.truncate(length);
    Ok(plaintext)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedMessage {
    pub sequence: u64,
    pub kind: u8,
    pub body: Vec<u8>,
}

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
    payload.insert("request".into(), Value::from(hex(request)));
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
            payload.insert("request".into(), Value::from(hex(request)));
            payload.insert("code".into(), Value::from(hex(code)));
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
    let expected_session = hex(expected_session_id);
    let expected_nonce = hex(expected_nonce);
    if object.get("session_id").and_then(Value::as_str) != Some(expected_session.as_str())
        || object.get("nonce").and_then(Value::as_str) != Some(expected_nonce.as_str())
    {
        return Err(TransportError::Replay);
    }
    let key = VerifyingKey::from_bytes(expected_identity_key.into())
        .map_err(|_| TransportError::IdentityMismatch)?;
    let digest = domain_hash(DIRECT_ENVELOPE_DOMAIN, canonical);
    key.verify_prehash(&digest, &signature)
        .map_err(|_| TransportError::HandshakeFailed)
}

// ---------------------------------------------------------------------------
// Health Plane carriage
//
// The Health Plane adds envelope `kind` values only. It reuses `sign_envelope`
// verbatim, so the frozen BIP-340 construction, the RFC-8785 canonical prehash,
// the certificate, the Noise handshake, and the framing are all unchanged.
// See `.docs/health-plane-contract.md` "Production carriage feasibility".
// ---------------------------------------------------------------------------

/// The `kind` prefix that marks an envelope as a Health Plane message.
pub const HEALTH_KIND_PREFIX: &str = "health_";

/// Bytes scanned when reading `kind` without parsing the document.
///
/// RFC-8785 sorts the seven frozen envelope keys, so `kind` always precedes
/// `payload`. A bounded prefix scan therefore reads the real top-level `kind`
/// before any attacker-controlled body, which lets the receiver apply the
/// frozen per-kind size cap *before* JSON parsing allocates anything
/// proportional to the declared content.
const KIND_SCAN_LIMIT: usize = 256;

/// Read the envelope `kind` without parsing the envelope.
///
/// Returns `None` when the prefix does not contain a syntactically plausible
/// `kind`. A hint that disagrees with the real top-level `kind` cannot be
/// exploited: `verify_envelope` re-encodes canonically and compares `kind`
/// against the same expectation, so a mismatch is a bounded rejection.
pub fn envelope_kind_hint(encoded: &[u8]) -> Option<&str> {
    if encoded.len() < 64 {
        return None;
    }
    let canonical = &encoded[..encoded.len() - 64];
    let window = &canonical[..canonical.len().min(KIND_SCAN_LIMIT)];
    let marker = b"\"kind\":\"";
    let start = window
        .windows(marker.len())
        .position(|candidate| candidate == marker)?
        + marker.len();
    let rest = window.get(start..)?;
    let end = rest.iter().position(|byte| *byte == b'"')?;
    if end > MAX_ENVELOPE_KIND_BYTES {
        return None;
    }
    std::str::from_utf8(&rest[..end]).ok()
}

/// The longest envelope `kind` the shipped protocol defines.
const MAX_ENVELOPE_KIND_BYTES: usize = 32;

/// The read-only view a Health Plane receiver needs after `verify_envelope`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthEnvelopeView {
    /// The envelope `created_at`, in UTC Unix seconds.
    pub created_at: i64,
    /// The envelope `payload` object.
    pub payload: Value,
}

/// Read `created_at` and `payload` out of an already-verified envelope.
///
/// This is a read-only projection. It performs no signature, session, or
/// authorization work: `verify_envelope` owns all of that and must be called
/// first, and the Health Plane shared operations own everything after it.
pub fn health_envelope_view(encoded: &[u8]) -> Result<HealthEnvelopeView, TransportError> {
    if encoded.len() < 64 {
        return Err(TransportError::InvalidFrame);
    }
    let value: Value = serde_json::from_slice(&encoded[..encoded.len() - 64])
        .map_err(|_| TransportError::InvalidFrame)?;
    let object = value.as_object().ok_or(TransportError::InvalidFrame)?;
    let created_at = object
        .get("created_at")
        .and_then(Value::as_i64)
        .ok_or(TransportError::InvalidFrame)?;
    let payload = object
        .get("payload")
        .cloned()
        .ok_or(TransportError::InvalidFrame)?;
    Ok(HealthEnvelopeView {
        created_at,
        payload,
    })
}

/// Sign one Health Plane message with the frozen envelope construction.
///
/// The only thing this adds over `sign_probe` and its siblings is the `kind`
/// string and the payload object; the signing construction itself is untouched.
/// Kinds outside the closed Health Plane set are refused here so this wrapper
/// can never become a generic envelope-signing oracle.
pub fn sign_health_envelope(
    identity: &NodeIdentity,
    kind: &str,
    session_id: &[u8; 32],
    nonce: [u8; 16],
    payload: Value,
    now: u64,
) -> Result<SignedEnvelope, TransportError> {
    if !kind.starts_with(HEALTH_KIND_PREFIX) || kind.len() > MAX_ENVELOPE_KIND_BYTES {
        return Err(TransportError::InvalidFrame);
    }
    if !payload.is_object() {
        return Err(TransportError::InvalidFrame);
    }
    sign_envelope(identity, kind, session_id, nonce, payload, now)
}

pub fn enrollment_request_bytes(encoded: &[u8]) -> Result<Vec<u8>, TransportError> {
    if encoded.len() < 64 {
        return Err(TransportError::InvalidFrame);
    }
    let canonical = &encoded[..encoded.len() - 64];
    let value: Value =
        serde_json::from_slice(canonical).map_err(|_| TransportError::InvalidFrame)?;
    let request = value
        .get("payload")
        .and_then(Value::as_object)
        .and_then(|payload| payload.get("request"))
        .and_then(Value::as_str)
        .ok_or(TransportError::InvalidFrame)?;
    if request.len() > crate::enrollment::MAX_REQUEST_BYTES * 2 {
        return Err(TransportError::MessageTooLarge);
    }
    decode_hex(request).ok_or(TransportError::InvalidFrame)
}

pub fn enrollment_ack_accepted(encoded: &[u8]) -> Result<bool, TransportError> {
    if encoded.len() < 64 {
        return Err(TransportError::InvalidFrame);
    }
    let value: Value = serde_json::from_slice(&encoded[..encoded.len() - 64])
        .map_err(|_| TransportError::InvalidFrame)?;
    let payload = value
        .get("payload")
        .and_then(Value::as_object)
        .ok_or(TransportError::InvalidFrame)?;
    let accepted = payload
        .get("accepted")
        .and_then(Value::as_bool)
        .ok_or(TransportError::InvalidFrame)?;
    if accepted {
        let request = payload
            .get("request")
            .and_then(Value::as_str)
            .ok_or(TransportError::InvalidFrame)?;
        let code = payload
            .get("code")
            .and_then(Value::as_str)
            .ok_or(TransportError::InvalidFrame)?;
        if decode_hex(request).is_none() || decode_hex(code).is_none() {
            return Err(TransportError::InvalidFrame);
        }
    } else if payload.len() != 1 {
        return Err(TransportError::InvalidFrame);
    }
    Ok(accepted)
}

pub fn enrollment_ack_offer(encoded: &[u8]) -> Result<(Vec<u8>, Vec<u8>), TransportError> {
    if encoded.len() < 64 {
        return Err(TransportError::InvalidFrame);
    }
    let value: Value = serde_json::from_slice(&encoded[..encoded.len() - 64])
        .map_err(|_| TransportError::InvalidFrame)?;
    let payload = value
        .get("payload")
        .and_then(Value::as_object)
        .ok_or(TransportError::InvalidFrame)?;
    if payload.get("accepted").and_then(Value::as_bool) != Some(true) || payload.len() != 3 {
        return Err(TransportError::InvalidFrame);
    }
    let request = payload
        .get("request")
        .and_then(Value::as_str)
        .and_then(decode_hex)
        .ok_or(TransportError::InvalidFrame)?;
    let code = payload
        .get("code")
        .and_then(Value::as_str)
        .and_then(decode_hex)
        .ok_or(TransportError::InvalidFrame)?;
    if request.len() > crate::enrollment::MAX_REQUEST_BYTES
        || code.len() != crate::enrollment::CODE_BYTES
    {
        return Err(TransportError::InvalidFrame);
    }
    Ok((request, code))
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
        .and_then(decode_hex)
        .ok_or(TransportError::InvalidFrame)?;
    nonce.try_into().map_err(|_| TransportError::InvalidFrame)
}

fn sign_envelope(
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
    object.insert("nonce".into(), Value::from(hex(&nonce)));
    object.insert("payload".into(), payload);
    object.insert(
        "sender".into(),
        Value::from(identity.public_status().node_id.clone()),
    );
    object.insert("session_id".into(), Value::from(hex(session_id)));
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

fn certificate_payload(certificate: &TransportCertificate) -> Vec<u8> {
    let mut payload = Vec::with_capacity(1 + MAX_CERTIFICATE_BYTES);
    payload.push(1);
    payload.extend_from_slice(certificate.as_bytes());
    payload
}

fn parse_certificate_payload(payload: &[u8]) -> Result<TransportCertificate, TransportError> {
    if payload.len() != 1 + MAX_CERTIFICATE_BYTES || payload[0] != 1 {
        return Err(TransportError::HandshakeFailed);
    }
    TransportCertificate::from_bytes(&payload[1..])
}

fn is_node_id(value: &str) -> bool {
    value.len() == 69
        && value.starts_with("omk1_")
        && value[5..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn domain_hash(domain: &[u8], body: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(body);
    digest.finalize().into()
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NodeContext, NodePathOverrides, NodePlatform};
    use tempfile::TempDir;

    fn identity(temp: &TempDir) -> NodeIdentity {
        let context = NodeContext::resolve_for(
            NodePlatform::Linux,
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
        NodeIdentity::load_or_initialize(&context).unwrap()
    }

    #[test]
    fn frame_parser_is_bounded_and_exact() {
        let frame = Frame::handshake(1, &[7; 32]).unwrap().encode().unwrap();
        assert_eq!(Frame::parse(&frame).unwrap().body.len(), 33);
        assert_eq!(
            Frame::parse(&frame[..frame.len() - 1]),
            Err(TransportError::InvalidFrame)
        );
        assert_eq!(
            Frame::handshake(1, &[0; MAX_HANDSHAKE_MESSAGE_BYTES + 1]),
            Err(TransportError::MessageTooLarge)
        );
    }

    #[test]
    fn all_low_order_x25519_encodings_are_rejected() {
        let private = [7u8; 32];
        for public in prohibited_x25519_public_keys() {
            assert_eq!(
                validate_x25519_public(&public),
                Err(TransportError::HandshakeFailed)
            );
            assert_eq!(
                x25519_probe(&private, &public),
                Err(TransportError::HandshakeFailed)
            );
        }
        assert!(x25519_probe(&private, &x25519_public_from_private(&private).unwrap()).is_ok());
    }

    #[test]
    fn certificate_binds_identity_and_transport_key() {
        let temp = TempDir::new().unwrap();
        let identity = identity(&temp);
        let private = [9u8; 32];
        let public = x25519_public_from_private(&private).unwrap();
        let certificate = TransportCertificate::issue(
            &identity,
            public,
            1,
            1_700_000_000,
            1_700_000_100,
            [4; 16],
        )
        .unwrap();
        assert_eq!(
            TransportCertificate::from_bytes(certificate.as_bytes()).unwrap(),
            certificate
        );
        let mut mutated = *certificate.as_bytes();
        mutated[109] ^= 1;
        assert!(TransportCertificate::from_bytes(&mutated).is_err());
    }

    #[test]
    fn noise_xx_round_trip_and_replay_rejection() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let first_identity = identity(&first);
        let second_identity = identity(&second);
        let first_private = [11u8; 32];
        let second_private = [13u8; 32];
        let now = 1_700_000_000;
        let first_certificate = TransportCertificate::issue(
            &first_identity,
            x25519_public_from_private(&first_private).unwrap(),
            1,
            now - 1,
            now + 1000,
            [1; 16],
        )
        .unwrap();
        let second_certificate = TransportCertificate::issue(
            &second_identity,
            x25519_public_from_private(&second_private).unwrap(),
            1,
            now - 1,
            now + 1000,
            [2; 16],
        )
        .unwrap();
        let mut initiator =
            NoiseHandshake::new(HandshakeRole::Initiator, first_private, first_certificate)
                .unwrap();
        let mut responder =
            NoiseHandshake::new(HandshakeRole::Responder, second_private, second_certificate)
                .unwrap();
        let message_1 = initiator.write_next().unwrap();
        responder.read_next(&message_1, now).unwrap();
        let message_2 = responder.write_next().unwrap();
        initiator.read_next(&message_2, now).unwrap();
        let message_3 = initiator.write_next().unwrap();
        responder.read_next(&message_3, now).unwrap();
        let mut sender = initiator.into_session().unwrap();
        let mut receiver = responder.into_session().unwrap();
        assert_eq!(sender.session_id(), receiver.session_id());
        let frame = sender.write(ENVELOPE_KIND, b"probe").unwrap();
        assert_eq!(receiver.read(&frame).unwrap().body, b"probe");
        assert_eq!(receiver.read(&frame), Err(TransportError::Replay));
    }

    #[test]
    fn failed_writes_preserve_counters_and_close_on_third_safe_failure() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let first_identity = identity(&first);
        let second_identity = identity(&second);
        let now = 1_700_000_000;
        let first_private = [21u8; 32];
        let second_private = [23u8; 32];
        let first_certificate = TransportCertificate::issue(
            &first_identity,
            x25519_public_from_private(&first_private).unwrap(),
            1,
            now - 1,
            now + 1000,
            [4; 16],
        )
        .unwrap();
        let second_certificate = TransportCertificate::issue(
            &second_identity,
            x25519_public_from_private(&second_private).unwrap(),
            1,
            now - 1,
            now + 1000,
            [5; 16],
        )
        .unwrap();
        let mut initiator =
            NoiseHandshake::new(HandshakeRole::Initiator, first_private, first_certificate)
                .unwrap();
        let mut responder =
            NoiseHandshake::new(HandshakeRole::Responder, second_private, second_certificate)
                .unwrap();
        let message_1 = initiator.write_next().unwrap();
        responder.read_next(&message_1, now).unwrap();
        let message_2 = responder.write_next().unwrap();
        initiator.read_next(&message_2, now).unwrap();
        let message_3 = initiator.write_next().unwrap();
        responder.read_next(&message_3, now).unwrap();
        let mut sender = initiator.into_session().unwrap();

        for attempt in 0..3 {
            sender.inject_write_fault(WriteFault::BeforeEncryption);
            assert_eq!(
                sender.write(ENVELOPE_KIND, b"fault"),
                Err(TransportError::Internal)
            );
            assert_eq!(sender.send_sequence, 0);
            assert_eq!(sender.sent_messages, 0);
            assert_eq!(sender.sent_bytes, 0);
            assert_eq!(sender.is_closed(), attempt == 2);
        }
    }

    #[test]
    fn rekey_threshold_failures_close_on_third_and_successful_rekey_round_trip() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        let first_identity = identity(&first);
        let second_identity = identity(&second);
        let now = 1_700_000_000;
        let first_private = [31u8; 32];
        let second_private = [33u8; 32];
        let first_certificate = TransportCertificate::issue(
            &first_identity,
            x25519_public_from_private(&first_private).unwrap(),
            1,
            now - 1,
            now + 1000,
            [6; 16],
        )
        .unwrap();
        let second_certificate = TransportCertificate::issue(
            &second_identity,
            x25519_public_from_private(&second_private).unwrap(),
            1,
            now - 1,
            now + 1000,
            [7; 16],
        )
        .unwrap();
        let mut initiator =
            NoiseHandshake::new(HandshakeRole::Initiator, first_private, first_certificate)
                .unwrap();
        let mut responder =
            NoiseHandshake::new(HandshakeRole::Responder, second_private, second_certificate)
                .unwrap();
        let message_1 = initiator.write_next().unwrap();
        responder.read_next(&message_1, now).unwrap();
        let message_2 = responder.write_next().unwrap();
        initiator.read_next(&message_2, now).unwrap();
        let message_3 = initiator.write_next().unwrap();
        responder.read_next(&message_3, now).unwrap();
        let mut sender = initiator.into_session().unwrap();
        let mut receiver = responder.into_session().unwrap();
        sender.sent_messages = REKEY_MESSAGES - 1;
        receiver.received_messages = REKEY_MESSAGES - 1;
        let frame = sender.write(ENVELOPE_KIND, b"threshold").unwrap();
        assert_eq!(receiver.read(&frame).unwrap().body, b"threshold");

        receiver.sent_messages = REKEY_MESSAGES - 1;
        sender.received_messages = REKEY_MESSAGES - 1;
        let frame = receiver.write(ENVELOPE_KIND, b"reverse-threshold").unwrap();
        assert_eq!(sender.read(&frame).unwrap().body, b"reverse-threshold");

        sender.sent_messages = REKEY_MESSAGES - 1;
        let before_sequence = sender.send_sequence;
        for attempt in 0..3 {
            sender.inject_write_fault(WriteFault::BeforeEncryption);
            assert_eq!(
                sender.write(ENVELOPE_KIND, b"fault"),
                Err(TransportError::Internal)
            );
            assert_eq!(sender.is_closed(), attempt == 2);
        }
        assert_eq!(sender.send_sequence, before_sequence);
        assert_eq!(sender.sent_messages, REKEY_MESSAGES - 1);
    }

    #[test]
    fn signed_probe_requires_exact_sender_and_nonce() {
        let temp = TempDir::new().unwrap();
        let identity = identity(&temp);
        let session = [3u8; 32];
        let nonce = [5u8; 16];
        let probe = sign_probe(&identity, &session, nonce, 1_700_000_000).unwrap();
        verify_envelope(
            &probe.encoded(),
            &identity.public_status().node_id,
            &identity_key(&identity),
            "probe",
            &session,
            &nonce,
        )
        .unwrap();
        assert_eq!(
            verify_envelope(
                &probe.encoded(),
                &identity.public_status().node_id,
                &identity_key(&identity),
                "ack",
                &session,
                &nonce,
            ),
            Err(TransportError::IdentityMismatch)
        );
    }

    fn identity_key(identity: &NodeIdentity) -> [u8; 32] {
        let bytes = identity.public_status().public_key_hex.as_bytes();
        let mut key = [0u8; 32];
        for (index, chunk) in bytes.chunks_exact(2).enumerate() {
            key[index] = u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap();
        }
        key
    }
}

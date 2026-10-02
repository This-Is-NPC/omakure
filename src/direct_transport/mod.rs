//! Protocol-neutral direct transport primitives.
//!
//! This module owns bytes, cryptographic state, and authorization decisions but
//! deliberately does not own sockets, threads, or SQLite.  The node service
//! adapter is responsible for those effects.

pub use crate::util::time::unix_seconds;

mod carriage;
mod certificate;
mod envelope;
mod errors;
mod frame;
mod handshake;
mod session;
mod x25519;

pub use carriage::{
    BASELINE_KIND_PREFIX, CUE_KIND_PREFIX, EnvelopeView, HEALTH_KIND_PREFIX,
    enrollment_ack_accepted, enrollment_ack_offer, enrollment_request_bytes, envelope_kind_hint,
    envelope_view, sign_baseline_envelope, sign_cue_envelope, sign_health_envelope,
};
pub use certificate::TransportCertificate;
pub use envelope::{
    PeerAuthorization, SignedEnvelope, authorize_peer, envelope_nonce, sign_ack, sign_manual_ack,
    sign_manual_request, sign_probe, verify_envelope,
};
pub use errors::{ProtocolErrorCode, TransportError};
pub use frame::Frame;
pub use handshake::{HandshakeRole, NoiseHandshake};
pub use session::{ReceivedMessage, TransportSession, stated_error};
pub use x25519::{
    prohibited_x25519_public_keys, validate_x25519_public, x25519_probe, x25519_public_from_private,
};

pub const CONTRACT_ID: &[u8] = b"omakure/direct-transport/v1";
pub const PROLOGUE: &[u8] = b"omakure/direct-transport/v1\0";
pub const CERTIFICATE_DOMAIN: &[u8] = b"omakure/transport-cert/v1\0";
pub const DIRECT_ENVELOPE_DOMAIN: &[u8] = b"omakure/direct-envelope/v1\0";
pub const NOISE_NAME: &str = "Noise_XX_25519_ChaChaPoly_SHA256";

pub const MAX_FRAME_LENGTH: usize = 1_048_580;
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

#[cfg(test)]
mod tests;

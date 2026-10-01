use super::certificate::{certificate_payload, parse_certificate_payload, TransportCertificate};
use super::errors::TransportError;
use super::frame::Frame;
use super::session::TransportSession;
use super::x25519::{x25519_probe, x25519_public_from_private};
use super::{HANDSHAKE_KIND, MAX_HANDSHAKE_MESSAGE_BYTES, NOISE_NAME, PROLOGUE};
use crate::util::entropy;
use snow::{params::NoiseParams, Builder, HandshakeState};

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
        entropy::fill_bytes(&mut fixed_ephemeral);
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

use super::errors::TransportError;
use super::{
    ENCRYPTED_KIND, FRAME_VERSION, HANDSHAKE_KIND, MAX_FRAME_LENGTH, MAX_HANDSHAKE_MESSAGE_BYTES,
};

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

use super::errors::{ProtocolErrorCode, TransportError};
use super::frame::Frame;
use super::{
    CLOSE_KIND, ENCRYPTED_KIND, ENVELOPE_KIND, ERROR_KIND, MAX_PLAINTEXT_BYTES, REKEY_MESSAGES,
    REKEY_PLAINTEXT_BYTES,
};
use crate::util::hex;
use snow::TransportState;
use std::cmp::Ordering;
use std::fmt;

pub struct TransportSession {
    pub(super) state: TransportState,
    pub(super) staged_state: TransportState,
    pub(super) session_id: [u8; 32],
    pub(super) send_sequence: u64,
    pub(super) receive_sequence: u64,
    pub(super) sent_messages: u64,
    pub(super) sent_bytes: u64,
    pub(super) received_messages: u64,
    pub(super) received_bytes: u64,
    pub(super) closed: bool,
    pub(super) last_received_ciphertext: Option<Vec<u8>>,
    pub(super) consecutive_write_failures: u8,
    #[cfg(test)]
    pub(super) write_fault: Option<WriteFault>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WriteFault {
    BeforeEncryption,
}

impl fmt::Debug for TransportSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransportSession")
            .field("session_id", &hex::encode(&self.session_id))
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

    /// State one refusal to an authenticated peer, then stop talking.
    ///
    /// The frame kind, its two-byte body, and the codes it may carry are all
    /// already frozen -- `read` has parsed `ERROR_KIND` since version 1 -- but
    /// nothing ever wrote one, so every refusal after a completed handshake
    /// reached the peer as a closed socket and became `internal` on the way.
    /// That is the code the contract reserves for "bounded local failure", and
    /// it is the one code a dialer is told to retry, so a peer refused on its
    /// merits retried forever instead of stopping.
    ///
    /// Only ever sent after the handshake authenticated the peer's certificate.
    /// A malformed unauthenticated peer is still closed without a response.
    pub fn write_error(&mut self, code: ProtocolErrorCode) -> Result<Vec<u8>, TransportError> {
        let frame = self.write(ERROR_KIND, &(code as u16).to_be_bytes())?;
        self.closed = true;
        Ok(frame)
    }

    #[cfg(test)]
    pub(super) fn inject_write_fault(&mut self, fault: WriteFault) {
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

/// The refusal a peer stated, if this message is one.
///
/// An `ERROR_KIND` message carries the stable code and nothing else -- `read`
/// already refuses one of any other length -- so there is nothing here to
/// parse beyond the two bytes. An unrecognised code is `internal`: the peer
/// refused, this node cannot say why, and pretending the message was ordinary
/// traffic would be worse than saying so.
pub fn stated_error(message: &ReceivedMessage) -> Option<TransportError> {
    if message.kind != ERROR_KIND {
        return None;
    }
    let code = message
        .body
        .get(..2)
        .and_then(|bytes| <[u8; 2]>::try_from(bytes).ok())
        .map(u16::from_be_bytes)
        .and_then(ProtocolErrorCode::from_u16)
        .unwrap_or(ProtocolErrorCode::Internal);
    Some(TransportError::from_code(code))
}

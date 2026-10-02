use std::fmt;
use std::io;
use thiserror::Error;

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
    /// The code a peer stated, or `None` for anything outside the frozen table.
    ///
    /// Deliberately closed: a peer that names a number version 1 does not
    /// define has said nothing this node can act on, and inventing a meaning
    /// for it would be the one thing the stable table exists to prevent.
    pub const fn from_u16(value: u16) -> Option<Self> {
        match value {
            1001 => Some(Self::UnsupportedVersion),
            1002 => Some(Self::InvalidFrame),
            1003 => Some(Self::MessageTooLarge),
            1004 => Some(Self::HandshakeFailed),
            1005 => Some(Self::IdentityMismatch),
            1006 => Some(Self::NotEnrolled),
            1007 => Some(Self::Revoked),
            1008 => Some(Self::Expired),
            1009 => Some(Self::Replay),
            1010 => Some(Self::RateLimited),
            1011 => Some(Self::Internal),
            _ => None,
        }
    }

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

    pub const fn from_code(code: ProtocolErrorCode) -> Self {
        match code {
            ProtocolErrorCode::UnsupportedVersion => Self::UnsupportedVersion,
            ProtocolErrorCode::InvalidFrame => Self::InvalidFrame,
            ProtocolErrorCode::MessageTooLarge => Self::MessageTooLarge,
            ProtocolErrorCode::HandshakeFailed => Self::HandshakeFailed,
            ProtocolErrorCode::IdentityMismatch => Self::IdentityMismatch,
            ProtocolErrorCode::NotEnrolled => Self::NotEnrolled,
            ProtocolErrorCode::Revoked => Self::Revoked,
            ProtocolErrorCode::Expired => Self::Expired,
            ProtocolErrorCode::Replay => Self::Replay,
            ProtocolErrorCode::RateLimited => Self::RateLimited,
            ProtocolErrorCode::Internal => Self::Internal,
        }
    }
}

impl From<io::Error> for TransportError {
    fn from(_: io::Error) -> Self {
        Self::Internal
    }
}

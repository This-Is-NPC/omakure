pub mod baseline;
pub mod battery;
pub mod config;
pub mod core;
pub mod doctor;
pub mod envs;
pub mod health;
pub mod node;
pub mod scripts;
pub mod search;

use serde::Serialize;
use std::fmt;

pub type OperationResult<T> = Result<T, OperationError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationErrorCode {
    InvalidInput,
    Forbidden,
    NotFound,
    AlreadyExists,
    NotSynced,
    ManifestInvalid,
    UnsafePath,
    UnsupportedScript,
    Conflict,
    GitFailed,
    IoFailed,
    RegistryInvalid,
    PayloadTooLarge,
    TransportUnsupportedVersion,
    TransportInvalidFrame,
    TransportMessageTooLarge,
    TransportHandshakeFailed,
    TransportIdentityMismatch,
    TransportNotEnrolled,
    TransportRevoked,
    TransportExpired,
    TransportReplay,
    TransportRateLimited,
    TransportInternal,
    EnrollmentDisabled,
    EnrollmentInvalid,
    EnrollmentExpired,
    EnrollmentReplay,
    EnrollmentMismatch,
    EnrollmentDenied,
    EnrollmentRateLimited,
    DiscoveryUnsupportedVersion,
    DiscoveryInvalidBeacon,
    DiscoveryMessageTooLarge,
    DiscoveryExpired,
    DiscoveryFuture,
    DiscoverySecretMismatch,
    DiscoveryIdentityMismatch,
    DiscoverySignatureInvalid,
    DiscoveryRateLimited,
    DiscoveryCandidateLimit,
    DiscoveryUnsupportedPlatform,
    DiscoveryInternal,
}

impl OperationErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::AlreadyExists => "already_exists",
            Self::NotSynced => "not_synced",
            Self::ManifestInvalid => "manifest_invalid",
            Self::UnsafePath => "unsafe_path",
            Self::UnsupportedScript => "unsupported_script",
            Self::Conflict => "conflict",
            Self::GitFailed => "git_failed",
            Self::IoFailed => "io_failed",
            Self::RegistryInvalid => "registry_invalid",
            Self::PayloadTooLarge => "payload_too_large",
            Self::TransportUnsupportedVersion => "transport_unsupported_version",
            Self::TransportInvalidFrame => "transport_invalid_frame",
            Self::TransportMessageTooLarge => "transport_message_too_large",
            Self::TransportHandshakeFailed => "transport_handshake_failed",
            Self::TransportIdentityMismatch => "transport_identity_mismatch",
            Self::TransportNotEnrolled => "transport_not_enrolled",
            Self::TransportRevoked => "transport_revoked",
            Self::TransportExpired => "transport_expired",
            Self::TransportReplay => "transport_replay",
            Self::TransportRateLimited => "transport_rate_limited",
            Self::TransportInternal => "transport_internal",
            Self::EnrollmentDisabled => "enrollment_disabled",
            Self::EnrollmentInvalid => "enrollment_invalid",
            Self::EnrollmentExpired => "enrollment_expired",
            Self::EnrollmentReplay => "enrollment_replay",
            Self::EnrollmentMismatch => "enrollment_mismatch",
            Self::EnrollmentDenied => "enrollment_denied",
            Self::EnrollmentRateLimited => "enrollment_rate_limited",
            Self::DiscoveryUnsupportedVersion => "discovery_unsupported_version",
            Self::DiscoveryInvalidBeacon => "discovery_invalid_beacon",
            Self::DiscoveryMessageTooLarge => "discovery_message_too_large",
            Self::DiscoveryExpired => "discovery_expired",
            Self::DiscoveryFuture => "discovery_future",
            Self::DiscoverySecretMismatch => "discovery_secret_mismatch",
            Self::DiscoveryIdentityMismatch => "discovery_identity_mismatch",
            Self::DiscoverySignatureInvalid => "discovery_signature_invalid",
            Self::DiscoveryRateLimited => "discovery_rate_limited",
            Self::DiscoveryCandidateLimit => "discovery_candidate_limit",
            Self::DiscoveryUnsupportedPlatform => "discovery_unsupported_platform",
            Self::DiscoveryInternal => "discovery_internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperationError {
    pub code: OperationErrorCode,
    pub message: String,
}

impl OperationError {
    pub fn new(code: OperationErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for OperationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for OperationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_error_codes_match_battery_contract() {
        let cases = [
            (OperationErrorCode::InvalidInput, "invalid_input"),
            (OperationErrorCode::Forbidden, "forbidden"),
            (OperationErrorCode::NotFound, "not_found"),
            (OperationErrorCode::AlreadyExists, "already_exists"),
            (OperationErrorCode::NotSynced, "not_synced"),
            (OperationErrorCode::ManifestInvalid, "manifest_invalid"),
            (OperationErrorCode::UnsafePath, "unsafe_path"),
            (OperationErrorCode::UnsupportedScript, "unsupported_script"),
            (OperationErrorCode::Conflict, "conflict"),
            (OperationErrorCode::GitFailed, "git_failed"),
            (OperationErrorCode::IoFailed, "io_failed"),
            (OperationErrorCode::RegistryInvalid, "registry_invalid"),
            (OperationErrorCode::PayloadTooLarge, "payload_too_large"),
        ];

        for (code, expected) in cases {
            assert_eq!(code.as_str(), expected);
        }
    }

    #[test]
    fn operation_error_display_is_stable() {
        let err = OperationError::new(OperationErrorCode::InvalidInput, "name is required");
        assert_eq!(err.to_string(), "invalid_input: name is required");
    }
}

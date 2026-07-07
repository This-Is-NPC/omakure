pub mod battery;
pub mod config;
pub mod core;
pub mod doctor;
pub mod scripts;
pub mod search;

use serde::Serialize;
use std::fmt;

pub type OperationResult<T> = Result<T, OperationError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationErrorCode {
    InvalidInput,
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
}

impl OperationErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
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

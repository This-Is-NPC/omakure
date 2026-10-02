use crate::domain::NodeConfigError;
use std::io;
use thiserror::Error;

use layout::{NODE_CONFIG_ENV, NODE_STATE_DIR_ENV, NODE_TEST_MODE_ENV};

mod context;
#[cfg(unix)]
mod fs_unix;
#[cfg(windows)]
mod fs_windows;
mod layout;
mod lifecycle;
mod policy;
mod private_token;
mod security;
mod state;

pub use context::NodeContext;
pub use layout::{
    AUTHORITY_KEY_FILE, DATABASE_FILE, IDENTITY_KEY_FILE, IDENTITY_LOCK_FILE, IDENTITY_PUBLIC_FILE,
    LIFECYCLE_LOCK_FILE, NodeLayout, NodePathOverrides, NodePlatform, PUBLISHER_KEY_FILE,
    STATE_NOT_INITIALIZED, TRANSPORT_CERTIFICATE_FILE, TRANSPORT_KEY_FILE, default_layout,
};
pub(crate) use policy::{PolicyConfig, read_policy_config, warn_policy_unreadable};
pub(crate) use private_token::{
    PRIVATE_TOKEN_TOMBSTONE_RETRY_LIMIT, PrivateFileCommitStatus, PrivateTokenLease,
};
#[cfg(test)]
pub(crate) use private_token::{PrivateTokenFault, set_private_token_fault};
pub(crate) use security::{is_not_found, write_new_file_atomically};
pub use state::NodeInitialization;

#[derive(Debug, Error)]
pub enum NodeError {
    #[error("invalid node path for {field}: {reason}")]
    InvalidPath { field: &'static str, reason: String },
    #[error("node test-mode overrides require {NODE_STATE_DIR_ENV} and {NODE_CONFIG_ENV}")]
    IncompleteTestOverrides,
    #[error(
        "{NODE_STATE_DIR_ENV} and {NODE_CONFIG_ENV} are only allowed with {NODE_TEST_MODE_ENV}=1"
    )]
    TestOverrideOutsideTestMode,
    #[error("node test mode is unavailable in this build")]
    TestModeUnavailable,
    #[error("node path is unsafe: {0}")]
    UnsafePath(String),
    #[error("node path has unexpected file type: {0}")]
    UnexpectedFileType(String),
    #[error("node path is insecure: {0}")]
    InsecurePath(String),
    #[error("node configuration already exists and is invalid: {0}")]
    ExistingConfig(String),
    #[error("node service lifecycle lock is busy")]
    LifecycleBusy,
    #[error("node configuration error: {0}")]
    Config(#[from] NodeConfigError),
    #[error("node I/O error: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests;

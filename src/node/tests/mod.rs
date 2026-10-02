#[cfg(unix)]
use super::fs_unix::{
    grow_principal_lookup_buffer, lookup_unix_principal, owner_policy, validate_open_file_identity,
};
#[cfg(windows)]
use super::fs_windows::windows_security_access_allowed;
#[cfg(unix)]
use super::private_token::PRIVATE_TOKEN_TOMBSTONE_PREFIX;
use super::security::symlink_metadata_if_present;
#[cfg(unix)]
use super::security::validate_file_security;
use super::*;
use crate::domain::NodeConfig;
#[cfg(debug_assertions)]
use std::fs;
use std::io::{self};
use std::path::{Path, PathBuf};

mod layout;
#[cfg(windows)]
mod lifecycle;
#[cfg(unix)]
mod policy;
#[cfg(unix)]
mod private_token;
mod security;
mod state;

#[cfg(debug_assertions)]
fn test_context(root: &Path) -> NodeContext {
    NodeContext::resolve_for(
        NodePlatform::Linux,
        NodePathOverrides::new(Some(root.join("state")), Some(root.join("node.toml"))),
        true,
        None,
        None,
        None,
    )
    .unwrap()
}

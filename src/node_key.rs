//! Custody of the BIP-340 scalars a node keeps in its private state directory.
//!
//! One discipline for every key: created inside the 0700 state directory,
//! written atomically at 0600, re-validated for owner and mode on every read,
//! opened without following a symlink or reparse point, and never returned by
//! any read path.

use crate::node::{NodeContext, NodeError};
use k256::elliptic_curve::Generate;
use k256::schnorr::SigningKey;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::path::Path;

/// The bytes of a persisted scalar.
pub const PRIVATE_KEY_BYTES: usize = 32;

/// How a key's own error type surfaces key-file refusals, so each key keeps
/// its own messages and `Display`.
pub trait KeyFileError: From<io::Error> + From<NodeError> {
    fn state(detail: String) -> Self;
    fn invalid_key() -> Self;
}

/// Read a scalar without following a symlink, re-validating owner and mode.
/// `unexpected_type` is the refusal when the opened path is not a regular file.
pub fn read_private_key<E: KeyFileError>(
    context: &NodeContext,
    path: &Path,
    unexpected_type: &str,
) -> Result<[u8; PRIVATE_KEY_BYTES], E> {
    let mut options = crate::util::fs::no_follow_open_options();
    options.read(true);
    let mut file = options.open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(E::state(unexpected_type.to_string()));
    }
    context.validate_private_file(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    bytes.try_into().map_err(|_| E::invalid_key())
}

/// A signing key held in its own file beside the node identity, named in
/// refusals as `{article} {label} key` and its `{scalar} scalar`.
pub struct HeldKey {
    pub label: &'static str,
    pub article: &'static str,
    pub scalar: &'static str,
}

impl HeldKey {
    /// Generate and persist a new key at `path`, refusing to replace one that
    /// already exists. The caller validates the file once it has accepted it.
    pub fn generate<E: KeyFileError>(
        &self,
        context: &NodeContext,
        path: &Path,
    ) -> Result<SigningKey, E> {
        context.ensure_state_directory()?;
        if fs::symlink_metadata(path).is_ok() {
            return Err(E::state(format!(
                "this node already holds {} {} key",
                self.article, self.label
            )));
        }
        let signing_key = SigningKey::generate();
        crate::node::write_new_file_atomically(path, signing_key.to_bytes().as_ref(), 0o600)?;
        Ok(signing_key)
    }

    /// Load the key at `path`, without creating anything.
    pub fn load<E: KeyFileError>(
        &self,
        context: &NodeContext,
        path: &Path,
    ) -> Result<SigningKey, E> {
        if !context.validate_existing_state_directory()? {
            return Err(E::state(crate::node::STATE_NOT_INITIALIZED.to_string()));
        }
        let metadata = fs::symlink_metadata(path)
            .map_err(|_| E::state(format!("this node holds no {} key", self.label)))?;
        if !metadata.file_type().is_file() {
            return Err(E::state(format!(
                "the {} key is not a regular file",
                self.label
            )));
        }
        context.validate_private_file(path)?;
        let bytes = read_private_key::<E>(
            context,
            path,
            &format!("the {} key has an unexpected file type", self.label),
        )?;
        let signing_key = SigningKey::from_slice(&bytes).map_err(|_| E::invalid_key())?;
        // A scalar that was not stored even-Y normalized would sign under a
        // different public key than the one the fleet records.
        if signing_key.to_bytes().as_slice() != bytes.as_slice() {
            return Err(E::state(format!(
                "the persisted {} scalar is not even-Y normalized",
                self.scalar
            )));
        }
        Ok(signing_key)
    }
}

/// The x-only public key of `signing_key`.
pub fn xonly_public_key(signing_key: &SigningKey) -> [u8; 32] {
    let mut key = [0u8; 32];
    key.copy_from_slice(signing_key.verifying_key().to_bytes().as_slice());
    key
}

/// The stable id of `signing_key` under `domain`, derived from the public key
/// rather than stored so the two can never disagree.
pub fn derive_key_id<const N: usize>(domain: &[u8], signing_key: &SigningKey) -> [u8; N] {
    let digest = Sha256::digest([domain, &xonly_public_key(signing_key)[..]].concat());
    let mut id = [0u8; N];
    id.copy_from_slice(&digest[..N]);
    id
}

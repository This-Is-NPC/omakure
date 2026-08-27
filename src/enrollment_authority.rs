//! Custody of the key that says "this node belongs to my fleet".
//!
//! Signed-bundle enrollment shipped with only half of itself. A node can verify
//! and apply a bundle — that path is certified by
//! `tests/docker_signed_bundle_e2e.rs` — but nothing in the product could ever
//! *issue* one: `SignedEnrollmentBundle::sign_with_material` was reachable only
//! from test modules, and no authority key existed at rest anywhere. A fleet
//! could be told it was enrolled; it could not enroll anyone.
//!
//! The authority key is deliberately **not** the node identity key. They are
//! both BIP-340 scalars and reusing one would have cost nothing to write, but
//! it would mean that compromising any single node's identity hands the
//! attacker the right to mint membership for the whole fleet. Separate key,
//! separate file, separate blast radius.
//!
//! Custody mirrors `node_identity` exactly rather than inventing a second
//! discipline: created inside the 0700 state directory, written atomically at
//! 0600, re-validated for owner and mode on every read, opened with
//! `O_NOFOLLOW` so a symlink cannot redirect it, and never returned by any read
//! path.

use crate::node::{NodeContext, NodeError};
use k256::elliptic_curve::Generate;
use k256::schnorr::SigningKey;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::Path;

/// The bytes of the authority scalar.
const AUTHORITY_PRIVATE_BYTES: usize = 32;

/// Domain separator for the authority key id.
///
/// Load-bearing: without it the same public key would hash identically here and
/// in any other construction that hashes a key.
const AUTHORITY_ID_DOMAIN: &[u8] = b"omakure/enrollment-authority-id/v1\0";

#[derive(Debug)]
pub enum AuthorityError {
    /// The state directory is absent, insecure, or the key file is not a
    /// regular file owned by this user at 0600.
    State(String),
    /// The persisted scalar is not a valid, even-Y normalized BIP-340 key.
    InvalidKey,
    Io(std::io::Error),
    Signing(String),
}

impl std::fmt::Display for AuthorityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::State(detail) => write!(f, "{detail}"),
            Self::InvalidKey => write!(f, "the enrollment authority key is invalid"),
            Self::Io(error) => write!(f, "enrollment authority I/O failed: {error}"),
            Self::Signing(detail) => write!(f, "{detail}"),
        }
    }
}

impl From<std::io::Error> for AuthorityError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<NodeError> for AuthorityError {
    fn from(error: NodeError) -> Self {
        Self::State(error.to_string())
    }
}

/// The enrollment authority this node holds, if it holds one.
pub struct EnrollmentAuthority {
    signing_key: SigningKey,
}

impl EnrollmentAuthority {
    /// Create the authority key, refusing to replace one that already exists.
    ///
    /// Never silently rotates. Replacing an authority key invalidates every
    /// bundle it ever signed and every `trust.authorities` entry that names it,
    /// across every machine in the fleet — that is a fleet-wide event, not a
    /// side effect of running a command twice.
    pub fn create(context: &NodeContext) -> Result<Self, AuthorityError> {
        context.ensure_state_directory()?;
        let path = context.authority_key_path();
        if fs::symlink_metadata(&path).is_ok() {
            return Err(AuthorityError::State(
                "this node already holds an enrollment authority key".to_string(),
            ));
        }
        let signing_key = SigningKey::generate();
        crate::node::write_atomic_new(&path, signing_key.to_bytes().as_ref(), 0o600)?;
        context.validate_private_file(&path)?;
        Ok(Self { signing_key })
    }

    /// Load the authority key, without creating anything.
    pub fn load_existing(context: &NodeContext) -> Result<Self, AuthorityError> {
        if !context.validate_existing_state_directory()? {
            return Err(AuthorityError::State(
                "node state is not initialized".to_string(),
            ));
        }
        let path = context.authority_key_path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            AuthorityError::State("this node holds no enrollment authority key".to_string())
        })?;
        if !metadata.file_type().is_file() {
            return Err(AuthorityError::State(
                "the enrollment authority key is not a regular file".to_string(),
            ));
        }
        context.validate_private_file(&path)?;
        let bytes = read_authority_key(context, &path)?;
        let signing_key = SigningKey::from_slice(&bytes).map_err(|_| AuthorityError::InvalidKey)?;
        // The same normalization check the identity makes: a scalar that was
        // not stored even-Y normalized would sign under a different public key
        // than the one published in `trust.authorities`.
        if signing_key.to_bytes().as_slice() != bytes.as_slice() {
            return Err(AuthorityError::State(
                "the persisted authority scalar is not even-Y normalized".to_string(),
            ));
        }
        Ok(Self { signing_key })
    }

    /// Whether this node holds an authority key at all.
    pub fn is_present(context: &NodeContext) -> bool {
        fs::symlink_metadata(context.authority_key_path())
            .is_ok_and(|metadata| metadata.file_type().is_file())
    }

    /// The x-only public key, as `trust.authorities[].public_key` carries it.
    pub fn public_key(&self) -> [u8; 32] {
        let mut key = [0u8; 32];
        key.copy_from_slice(self.signing_key.verifying_key().to_bytes().as_slice());
        key
    }

    /// The stable id a bundle carries and `trust.authorities[].key_id` names.
    ///
    /// Derived from the public key rather than stored, so the two can never
    /// disagree and there is no second piece of state to keep in step.
    pub fn key_id(&self) -> [u8; crate::enrollment::BUNDLE_AUTHORITY_ID_BYTES] {
        let digest = Sha256::digest([AUTHORITY_ID_DOMAIN, &self.public_key()[..]].concat());
        let mut id = [0u8; crate::enrollment::BUNDLE_AUTHORITY_ID_BYTES];
        id.copy_from_slice(&digest[..crate::enrollment::BUNDLE_AUTHORITY_ID_BYTES]);
        id
    }

    /// Mint one bundle. The signing itself is the shipped, tested construction;
    /// this is the caller it never had.
    #[allow(clippy::too_many_arguments)]
    pub fn issue(
        &self,
        bundle_id: [u8; crate::enrollment::REQUEST_ID_BYTES],
        organization: String,
        audience_node_id: String,
        subject_node_id: String,
        subject_xonly: [u8; 32],
        subject_transport_x25519: [u8; 32],
        subject_certificate: [u8; crate::direct_transport::MAX_CERTIFICATE_BYTES],
        role: crate::enrollment::EnrollmentRole,
        capabilities: Vec<String>,
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Vec<u8>, AuthorityError> {
        // A bundle whose audience is its own subject would enrol a node into
        // trusting itself. Refused here rather than left to the receiver.
        if audience_node_id == subject_node_id {
            return Err(AuthorityError::Signing(
                "a bundle cannot name the same node as audience and subject".to_string(),
            ));
        }
        crate::enrollment::SignedEnrollmentBundle::sign_with_material(
            self.signing_key.to_bytes().as_ref(),
            bundle_id,
            self.key_id(),
            organization,
            audience_node_id,
            subject_node_id,
            subject_xonly,
            subject_transport_x25519,
            subject_certificate,
            role,
            capabilities,
            issued_at,
            expires_at,
        )
        .map(|bundle| bundle.encode())
        .map_err(|error| AuthorityError::Signing(format!("{error:?}")))
    }
}

/// Read the scalar without following a symlink, re-validating owner and mode.
fn read_authority_key(
    context: &NodeContext,
    path: &Path,
) -> Result<[u8; AUTHORITY_PRIVATE_BYTES], AuthorityError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options.open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(AuthorityError::State(
            "the enrollment authority key has an unexpected file type".to_string(),
        ));
    }
    context.validate_private_file(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    bytes.try_into().map_err(|_| AuthorityError::InvalidKey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NodePathOverrides, NodePlatform};

    fn node_context(root: &Path) -> NodeContext {
        let config = root.join("node.toml");
        std::fs::write(&config, "version = 1\n").expect("write config");
        NodeContext::resolve_for(
            NodePlatform::current(),
            NodePathOverrides::new(Some(root.join("state")), Some(config)),
            true,
            None,
            None,
            None,
        )
        .expect("resolve node context")
    }

    /// Creating twice must refuse rather than rotate.
    ///
    /// Replacing an authority key invalidates every bundle it signed and every
    /// `trust.authorities` entry naming it, on every machine in the fleet. That
    /// is not something a repeated command should do quietly.
    #[test]
    fn an_authority_key_is_never_silently_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let context = node_context(dir.path());

        let first = EnrollmentAuthority::create(&context).expect("create the authority");
        let before = first.public_key();

        assert!(
            EnrollmentAuthority::create(&context).is_err(),
            "a second create must refuse; rotation is a fleet-wide event"
        );
        assert_eq!(
            EnrollmentAuthority::load_existing(&context)
                .expect("load it back")
                .public_key(),
            before,
            "the original key must survive the refused create"
        );
    }

    /// The key id is derived, so it cannot drift from the key it names.
    #[test]
    fn the_key_id_is_a_function_of_the_public_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let context = node_context(dir.path());
        let authority = EnrollmentAuthority::create(&context).expect("create");

        let reloaded = EnrollmentAuthority::load_existing(&context).expect("load");
        assert_eq!(authority.key_id(), reloaded.key_id());

        // A different key must produce a different id, or the id identifies
        // nothing.
        let other_dir = tempfile::tempdir().expect("tempdir");
        let other_context = node_context(other_dir.path());
        let other = EnrollmentAuthority::create(&other_context).expect("create");
        assert_ne!(
            authority.key_id(),
            other.key_id(),
            "two authorities must not share an id"
        );
    }

    /// A node that holds no authority says so, rather than inventing one.
    #[test]
    fn loading_without_a_key_refuses_instead_of_creating_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let context = node_context(dir.path());
        context.ensure_state_directory().expect("state dir");

        assert!(!EnrollmentAuthority::is_present(&context));
        assert!(EnrollmentAuthority::load_existing(&context).is_err());
        assert!(
            !context.authority_key_path().exists(),
            "a failed load must not have created a key"
        );
    }

    /// The file discipline is real, not decorative.
    #[cfg(unix)]
    #[test]
    fn a_loosened_authority_key_is_refused() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let context = node_context(dir.path());
        EnrollmentAuthority::create(&context).expect("create");
        assert!(EnrollmentAuthority::load_existing(&context).is_ok());

        let path = context.authority_key_path();
        let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&path, perms).expect("loosen");

        assert!(
            EnrollmentAuthority::load_existing(&context).is_err(),
            "a world-readable authority key must not load"
        );
    }

    /// A symlink must not be able to stand in for the key.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_authority_key_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let context = node_context(dir.path());
        context.ensure_state_directory().expect("state dir");

        let elsewhere = dir.path().join("elsewhere.key");
        std::fs::write(&elsewhere, [7u8; 32]).expect("write");
        std::os::unix::fs::symlink(&elsewhere, context.authority_key_path()).expect("symlink");

        assert!(
            EnrollmentAuthority::load_existing(&context).is_err(),
            "a symlink must not redirect the authority key"
        );
        assert!(
            EnrollmentAuthority::create(&context).is_err(),
            "create must not overwrite through a symlink either"
        );
    }

    /// The state directory's closed entry list was amended for this key. It
    /// must still refuse everything it refused before.
    ///
    /// The control is the point: if the amendment had been written as a
    /// wildcard, the first half would pass and the second would too, and the
    /// security property would be gone with every test still green.
    #[test]
    fn the_amended_state_allow_list_admits_the_key_and_nothing_else() {
        let dir = tempfile::tempdir().expect("tempdir");
        let context = node_context(dir.path());
        crate::node_identity::NodeIdentity::load_or_initialize(&context).expect("identity");
        EnrollmentAuthority::create(&context).expect("create");

        assert!(
            context
                .validate_existing_state_contents()
                .expect("read the state directory"),
            "an authority key beside the identity must be admitted"
        );

        std::fs::write(context.state_dir().join("stray.txt"), b"x").expect("write stray");
        assert!(
            context.validate_existing_state_contents().is_err(),
            "an unlisted entry must still be refused; the list is a control, not decoration"
        );
    }

    /// A bundle naming one node as both audience and subject would enrol it
    /// into trusting itself.
    #[test]
    fn a_bundle_cannot_name_one_node_as_both_sides() {
        let dir = tempfile::tempdir().expect("tempdir");
        let context = node_context(dir.path());
        let authority = EnrollmentAuthority::create(&context).expect("create");
        let node = format!("omk1_{}", "a".repeat(64));

        assert!(authority
            .issue(
                [1u8; crate::enrollment::REQUEST_ID_BYTES],
                "org".to_string(),
                node.clone(),
                node,
                [2u8; 32],
                [3u8; 32],
                [0u8; crate::direct_transport::MAX_CERTIFICATE_BYTES],
                crate::enrollment::EnrollmentRole::Conductor,
                vec!["remote-run".to_string()],
                1_800_000_000,
                1_800_003_600,
            )
            .is_err());
    }
}

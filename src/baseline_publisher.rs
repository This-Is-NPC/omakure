//! Custody of the key that says "this code came from my fleet".
//!
//! Item 8 is the first time this product puts code onto a node, so it is the
//! first time a key can authorize bytes rather than an action. That key is
//! deliberately **not** the enrollment authority key. Both are BIP-340 scalars
//! and sharing one would cost nothing to write, but a key that both admits
//! members to the fleet and ships them code has a blast radius nobody chose:
//! stealing it would turn "can enrol a machine" into "can run anything on every
//! machine". Separate key, separate file, separate blast radius — the same
//! argument `enrollment_authority` makes against reusing the node identity.
//!
//! Custody is `enrollment_authority`'s discipline, not a second one: created
//! inside the 0700 state directory, written atomically at 0600, re-validated
//! for owner and mode on every read, opened with `O_NOFOLLOW` so a symlink
//! cannot redirect it, and never returned by any read path.

use crate::node::{NodeContext, NodeError};
use k256::elliptic_curve::Generate;
use k256::schnorr::SigningKey;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::Path;

/// The bytes of the publisher scalar.
const PUBLISHER_PRIVATE_BYTES: usize = 32;

/// Domain separator for the publisher key id.
///
/// Load-bearing, and distinct from the authority's: were they equal, one key
/// used for both purposes would present the same id in both places and the two
/// records would look consistent while naming one blast radius.
const PUBLISHER_ID_DOMAIN: &[u8] = b"omakure/baseline-publisher-id/v1\0";

#[derive(Debug)]
pub enum PublisherError {
    /// The state directory is absent, insecure, or the key file is not a
    /// regular file owned by this user at 0600.
    State(String),
    /// The persisted scalar is not a valid, even-Y normalized BIP-340 key.
    InvalidKey,
    Io(std::io::Error),
    Signing(String),
}

impl std::fmt::Display for PublisherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::State(detail) => write!(f, "{detail}"),
            Self::InvalidKey => write!(f, "the baseline publisher key is invalid"),
            Self::Io(error) => write!(f, "baseline publisher I/O failed: {error}"),
            Self::Signing(detail) => write!(f, "{detail}"),
        }
    }
}

impl From<std::io::Error> for PublisherError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<NodeError> for PublisherError {
    fn from(error: NodeError) -> Self {
        Self::State(error.to_string())
    }
}

/// The baseline publisher this node holds, if it holds one.
pub struct BaselinePublisher {
    signing_key: SigningKey,
}

impl BaselinePublisher {
    /// Create the publisher key, refusing to replace one that already exists.
    ///
    /// Never silently rotates. Replacing a publisher key orphans every baseline
    /// it ever signed: the manifests stay valid bytes but no longer verify
    /// under the key the fleet records, so every Performer's next drift answer
    /// becomes unanswerable at once. That is a fleet-wide event, not a side
    /// effect of running a command twice.
    pub fn create(context: &NodeContext) -> Result<Self, PublisherError> {
        context.ensure_state_directory()?;
        let path = context.publisher_key_path();
        if fs::symlink_metadata(&path).is_ok() {
            return Err(PublisherError::State(
                "this node already holds a baseline publisher key".to_string(),
            ));
        }
        let signing_key = SigningKey::generate();
        crate::node::write_atomic_new(&path, signing_key.to_bytes().as_ref(), 0o600)?;
        context.validate_private_file(&path)?;
        Ok(Self { signing_key })
    }

    /// Load the publisher key, without creating anything.
    pub fn load_existing(context: &NodeContext) -> Result<Self, PublisherError> {
        if !context.validate_existing_state_directory()? {
            return Err(PublisherError::State(
                "node state is not initialized".to_string(),
            ));
        }
        let path = context.publisher_key_path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            PublisherError::State("this node holds no baseline publisher key".to_string())
        })?;
        if !metadata.file_type().is_file() {
            return Err(PublisherError::State(
                "the baseline publisher key is not a regular file".to_string(),
            ));
        }
        context.validate_private_file(&path)?;
        let bytes = read_publisher_key(context, &path)?;
        let signing_key = SigningKey::from_slice(&bytes).map_err(|_| PublisherError::InvalidKey)?;
        // The same normalization check the identity and the authority make: a
        // scalar that was not stored even-Y normalized would sign under a
        // different public key than the one the fleet records.
        if signing_key.to_bytes().as_slice() != bytes.as_slice() {
            return Err(PublisherError::State(
                "the persisted publisher scalar is not even-Y normalized".to_string(),
            ));
        }
        Ok(Self { signing_key })
    }

    /// Whether this node holds a publisher key at all.
    pub fn is_present(context: &NodeContext) -> bool {
        fs::symlink_metadata(context.publisher_key_path())
            .is_ok_and(|metadata| metadata.file_type().is_file())
    }

    /// The x-only public key, as a receiver's recorded publisher carries it.
    pub fn public_key(&self) -> [u8; crate::baseline::PUBLISHER_KEY_BYTES] {
        let mut key = [0u8; crate::baseline::PUBLISHER_KEY_BYTES];
        key.copy_from_slice(self.signing_key.verifying_key().to_bytes().as_slice());
        key
    }

    /// The stable id a manifest carries and a receiver's publisher record
    /// names.
    ///
    /// Derived from the public key rather than stored, so the two can never
    /// disagree and there is no second piece of state to keep in step.
    pub fn key_id(&self) -> [u8; crate::baseline::PUBLISHER_ID_BYTES] {
        let digest = Sha256::digest([PUBLISHER_ID_DOMAIN, &self.public_key()[..]].concat());
        let mut id = [0u8; crate::baseline::PUBLISHER_ID_BYTES];
        id.copy_from_slice(&digest[..crate::baseline::PUBLISHER_ID_BYTES]);
        id
    }

    /// Sign one baseline over the script bodies themselves.
    ///
    /// Takes bodies rather than hashes so this node cannot publish a manifest
    /// describing content it does not hold.
    pub fn publish(
        &self,
        organization: String,
        scripts: &[(String, Vec<u8>)],
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Vec<u8>, PublisherError> {
        crate::baseline::SignedBaselineManifest::sign_with_material(
            self.signing_key.to_bytes().as_ref(),
            self.key_id(),
            organization,
            scripts,
            issued_at,
            expires_at,
        )
        .map(|manifest| manifest.encode())
        .map_err(|error| PublisherError::Signing(error.to_string()))
    }
}

/// Read the scalar without following a symlink, re-validating owner and mode.
fn read_publisher_key(
    context: &NodeContext,
    path: &Path,
) -> Result<[u8; PUBLISHER_PRIVATE_BYTES], PublisherError> {
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
        return Err(PublisherError::State(
            "the baseline publisher key has an unexpected file type".to_string(),
        ));
    }
    context.validate_private_file(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    bytes.try_into().map_err(|_| PublisherError::InvalidKey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::baseline::{BaselinePublisherKey, SignedBaselineManifest};
    use crate::node::{NodePathOverrides, NodePlatform};

    const ISSUED_AT: u64 = 1_800_000_000;
    const EXPIRES_AT: u64 = 1_800_003_600;

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

    fn scripts() -> Vec<(String, Vec<u8>)> {
        vec![
            ("ops/deploy.sh".to_string(), b"echo deploy\n".to_vec()),
            ("audit.py".to_string(), b"print('audit')\n".to_vec()),
        ]
    }

    /// Creating twice must refuse rather than rotate.
    #[test]
    fn a_publisher_key_is_never_silently_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let context = node_context(dir.path());

        let first = BaselinePublisher::create(&context).expect("create the publisher");
        let before = first.public_key();

        assert!(
            BaselinePublisher::create(&context).is_err(),
            "a second create must refuse; rotation orphans every signed baseline"
        );
        assert_eq!(
            BaselinePublisher::load_existing(&context)
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
        let publisher = BaselinePublisher::create(&context).expect("create");

        assert_eq!(
            publisher.key_id(),
            BaselinePublisher::load_existing(&context)
                .expect("load")
                .key_id()
        );

        let other_dir = tempfile::tempdir().expect("tempdir");
        let other_context = node_context(other_dir.path());
        assert_ne!(
            publisher.key_id(),
            BaselinePublisher::create(&other_context)
                .expect("create")
                .key_id(),
            "two publishers must not share an id"
        );
    }

    /// The publisher key and the authority key are different keys on the same
    /// node, not one key with two names.
    #[test]
    fn a_publisher_key_is_not_the_enrollment_authority_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let context = node_context(dir.path());

        let authority =
            crate::enrollment_authority::EnrollmentAuthority::create(&context).expect("authority");
        let publisher = BaselinePublisher::create(&context).expect("publisher");

        assert_ne!(
            authority.public_key(),
            publisher.public_key(),
            "shipping code and admitting members must not share one scalar"
        );
        assert_ne!(
            context.authority_key_path(),
            context.publisher_key_path(),
            "two keys need two files, or one create silently overwrites the other"
        );
    }

    /// A node that holds no publisher says so, rather than inventing one.
    #[test]
    fn loading_without_a_key_refuses_instead_of_creating_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let context = node_context(dir.path());
        context.ensure_state_directory().expect("state dir");

        assert!(!BaselinePublisher::is_present(&context));
        assert!(BaselinePublisher::load_existing(&context).is_err());
        assert!(
            !context.publisher_key_path().exists(),
            "a failed load must not have created a key"
        );
    }

    /// The file discipline is real, not decorative.
    #[cfg(unix)]
    #[test]
    fn a_loosened_publisher_key_is_refused() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let context = node_context(dir.path());
        BaselinePublisher::create(&context).expect("create");
        assert!(BaselinePublisher::load_existing(&context).is_ok());

        let path = context.publisher_key_path();
        let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&path, perms).expect("loosen");

        assert!(
            BaselinePublisher::load_existing(&context).is_err(),
            "a world-readable publisher key must not load"
        );
    }

    /// A symlink must not be able to stand in for the key.
    ///
    /// The target is a valid scalar at 0600 owned by this user on purpose: an
    /// attacker-controlled file that looked *wrong* would be refused by the
    /// mode check and prove nothing about symlinks. Here the only thing left
    /// between the redirect and a loaded key is the refusal to follow it.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_publisher_key_is_refused() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let context = node_context(dir.path());
        context.ensure_state_directory().expect("state dir");

        let decoy_dir = tempfile::tempdir().expect("tempdir");
        let decoy_context = node_context(decoy_dir.path());
        let decoy = BaselinePublisher::create(&decoy_context).expect("a real key elsewhere");
        let elsewhere = decoy_context.publisher_key_path();
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o600)).expect("0600");
        drop(decoy);
        std::os::unix::fs::symlink(&elsewhere, context.publisher_key_path()).expect("symlink");

        assert!(
            BaselinePublisher::load_existing(&context).is_err(),
            "a symlink must not redirect the publisher key"
        );
        assert!(
            BaselinePublisher::create(&context).is_err(),
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
        BaselinePublisher::create(&context).expect("create");

        assert!(
            context
                .validate_existing_state_contents()
                .expect("read the state directory"),
            "a publisher key beside the identity must be admitted"
        );

        std::fs::write(context.state_dir().join("stray.txt"), b"x").expect("write stray");
        assert!(
            context.validate_existing_state_contents().is_err(),
            "an unlisted entry must still be refused; the list is a control, not decoration"
        );
    }

    /// What this node publishes verifies under what this node advertises.
    ///
    /// The key id and the public key are separately derived; if either drifted,
    /// a fleet recording one would reject manifests signed by the other, and
    /// nothing in the custody tests alone would notice.
    #[test]
    fn a_published_baseline_verifies_under_this_publishers_own_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let context = node_context(dir.path());
        let publisher = BaselinePublisher::create(&context).expect("create");

        let encoded = publisher
            .publish("acme".to_string(), &scripts(), ISSUED_AT, EXPIRES_AT)
            .expect("publish");
        let manifest = SignedBaselineManifest::decode(&encoded).expect("decode");

        manifest
            .verify(
                &BaselinePublisherKey {
                    key_id: publisher.key_id(),
                    public_key: publisher.public_key(),
                    revoked: false,
                },
                "acme",
                ISSUED_AT + 1,
            )
            .expect("a fleet recording this publisher must accept what it signs");

        let other_dir = tempfile::tempdir().expect("tempdir");
        let other = BaselinePublisher::create(&node_context(other_dir.path())).expect("create");
        assert!(
            manifest
                .verify(
                    &BaselinePublisherKey {
                        key_id: other.key_id(),
                        public_key: other.public_key(),
                        revoked: false,
                    },
                    "acme",
                    ISSUED_AT + 1,
                )
                .is_err(),
            "a different publisher's record must not accept this baseline"
        );
    }
}

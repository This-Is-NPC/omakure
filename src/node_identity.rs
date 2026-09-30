use crate::domain::NODE_ID_PREFIX;
use crate::node::{
    write_new_file_atomically, NodeContext, NodeError, DATABASE_FILE, IDENTITY_KEY_FILE,
    IDENTITY_LOCK_FILE, IDENTITY_PUBLIC_FILE, LIFECYCLE_LOCK_FILE, STATE_NOT_INITIALIZED,
};
use crate::node_key::{KeyFileError, PRIVATE_KEY_BYTES};
use crate::node_registry::RegistryError;
use crate::util::digest::sha256_domain;
use crate::util::hex;
use fs2::FileExt;
use k256::elliptic_curve::Generate;
use k256::schnorr::{signature::hazmat::PrehashSigner, Signature, SigningKey};
use std::fs;
use std::io;
use std::path::Path;
use thiserror::Error;

const NODE_ID_DOMAIN: &[u8] = b"omakure/node-id/v1\0";

#[derive(Debug, Error)]
pub enum NodeIdentityError {
    #[error("node identity state error: {0}")]
    State(String),
    #[error("node identity path error: {0}")]
    Node(#[from] NodeError),
    #[error("node identity I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("node identity cryptographic state is invalid")]
    InvalidKey,
    #[error("BIP-340 signing failed")]
    Signing,
    #[error("node trust registry error: {0}")]
    Registry(#[from] RegistryError),
}

impl KeyFileError for NodeIdentityError {
    fn state(detail: String) -> Self {
        Self::State(detail)
    }

    fn invalid_key() -> Self {
        Self::InvalidKey
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeIdentityStatus {
    /// Lowercase hexadecimal BIP-340 x-only public key.
    pub public_key_hex: String,
    pub node_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectEnvelopePrehash([u8; 32]);

impl DirectEnvelopePrehash {
    /// Hash already RFC 8785-canonicalized envelope bytes with the direct domain.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Self {
        Self(sha256_domain(
            crate::direct_transport::DIRECT_ENVELOPE_DOMAIN,
            bytes,
        ))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Bip340Signature([u8; 64]);

impl std::fmt::Debug for Bip340Signature {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Bip340Signature")
            .field(&"<redacted-public-signature>")
            .finish()
    }
}

impl Bip340Signature {
    pub fn to_bytes(self) -> [u8; 64] {
        self.0
    }
}

pub struct NodeIdentity {
    signing_key: SigningKey,
    status: NodeIdentityStatus,
}

impl NodeIdentity {
    pub fn load_or_initialize(context: &NodeContext) -> Result<Self, NodeIdentityError> {
        Self::load_or_initialize_with(context, None)
    }

    /// Import a scalar explicitly, normalizing it before the first atomic write.
    pub fn import(context: &NodeContext, scalar: &[u8]) -> Result<Self, NodeIdentityError> {
        let signing_key =
            SigningKey::from_slice(scalar).map_err(|_| NodeIdentityError::InvalidKey)?;
        Self::load_or_initialize_with(context, Some(signing_key))
    }

    /// Load an existing identity without creating a state directory, lock
    /// file, identity, or registry. This is the fail-closed path for public
    /// status inspection.
    pub fn load_existing(context: &NodeContext) -> Result<Self, NodeIdentityError> {
        if !context.validate_existing_state_directory()? {
            return Err(NodeIdentityError::State(STATE_NOT_INITIALIZED.to_string()));
        }
        reject_public_companion(context.state_dir())?;
        let identity_path = context.identity_path();
        if !inspect_existing_state_file(&identity_path, IDENTITY_KEY_FILE)? {
            return Err(NodeIdentityError::State(
                "node identity is not initialized".to_string(),
            ));
        }
        context.validate_private_file(&identity_path)?;
        let bytes = read_private_key(context, &identity_path)?;
        let signing_key =
            SigningKey::from_slice(&bytes).map_err(|_| NodeIdentityError::InvalidKey)?;
        if signing_key.to_bytes().as_slice() != bytes.as_slice() {
            return Err(NodeIdentityError::State(
                "persisted identity scalar is not even-Y normalized".to_string(),
            ));
        }
        Ok(Self::from_signing_key(signing_key))
    }

    fn load_or_initialize_with(
        context: &NodeContext,
        imported: Option<SigningKey>,
    ) -> Result<Self, NodeIdentityError> {
        context.ensure_state_directory()?;
        let _lock = IdentityLock::acquire(context)?;
        cleanup_identity_temps(context.state_dir())?;

        let identity_path = context.identity_path();
        reject_public_companion(context.state_dir())?;
        let identity_exists = inspect_existing_state_file(&identity_path, IDENTITY_KEY_FILE)?;
        let database_exists = inspect_existing_state_file(&context.database_path(), DATABASE_FILE)?;
        if !identity_exists && database_exists {
            return Err(NodeIdentityError::State(
                "node state is missing its private identity".to_string(),
            ));
        }
        if identity_exists && !database_exists {
            return Err(NodeIdentityError::State(
                "node state is missing its trust registry".to_string(),
            ));
        }

        let created_identity = !identity_exists;
        let signing_key = if identity_exists {
            if imported.is_some() {
                return Err(NodeIdentityError::State(
                    "cannot import over an existing identity".to_string(),
                ));
            }
            context.validate_private_file(&identity_path)?;
            let bytes = read_private_key(context, &identity_path)?;
            let signing_key =
                SigningKey::from_slice(&bytes).map_err(|_| NodeIdentityError::InvalidKey)?;
            let normalized = signing_key.to_bytes();
            if normalized.as_slice() != bytes.as_slice() {
                return Err(NodeIdentityError::State(
                    "persisted identity scalar is not even-Y normalized".to_string(),
                ));
            }
            signing_key
        } else {
            let signing_key = imported.unwrap_or_else(SigningKey::generate);
            let normalized = signing_key.to_bytes();
            write_new_file_atomically(&identity_path, normalized.as_ref(), 0o600)?;
            context.validate_private_file(&identity_path)?;
            signing_key
        };

        let identity = Self::from_signing_key(signing_key);
        if created_identity {
            context.open_trust_registry_for_initialization(identity.public_status())?;
        } else {
            context.open_trust_registry(identity.public_status())?;
        }
        Ok(identity)
    }

    fn from_signing_key(signing_key: SigningKey) -> Self {
        let status = status_for_key(&signing_key);
        Self {
            signing_key,
            status,
        }
    }

    pub fn public_status(&self) -> &NodeIdentityStatus {
        &self.status
    }

    /// Sign a direct-envelope prehash; canonicalization is deliberately external.
    pub fn sign_direct_envelope(
        &self,
        prehash: DirectEnvelopePrehash,
    ) -> Result<Bip340Signature, NodeIdentityError> {
        self.sign_prehash(&prehash.0)
    }

    pub(crate) fn sign_transport_certificate(
        &self,
        body: &[u8],
    ) -> Result<Bip340Signature, NodeIdentityError> {
        self.sign_prehash(&sha256_domain(
            crate::direct_transport::CERTIFICATE_DOMAIN,
            body,
        ))
    }

    pub(crate) fn sign_discovery(&self, body: &[u8]) -> Result<Bip340Signature, NodeIdentityError> {
        self.sign_prehash(&sha256_domain(
            crate::discovery::BEACON_SIGNATURE_DOMAIN,
            body,
        ))
    }

    pub(crate) fn sign_enrollment(
        &self,
        body: &[u8],
    ) -> Result<Bip340Signature, NodeIdentityError> {
        self.sign_prehash(&sha256_domain(crate::enrollment::DOMAIN, body))
    }

    fn sign_prehash(&self, prehash: &[u8; 32]) -> Result<Bip340Signature, NodeIdentityError> {
        let signature: Signature = self
            .signing_key
            .sign_prehash(prehash)
            .map_err(|_| NodeIdentityError::Signing)?;
        Ok(Bip340Signature(signature.to_bytes()))
    }

    /// Remove the complete validated node-owned state for an explicit factory
    /// reset. The public config is removed only when it lives inside the state
    /// directory; normal platform config lives outside this boundary.
    pub(crate) fn execute_factory_reset(context: &NodeContext) -> Result<bool, NodeIdentityError> {
        if !context.validate_existing_state_contents()? {
            return Ok(false);
        }
        let lock = IdentityLock::acquire(context)?;
        let mut paths = Vec::new();
        for entry in fs::read_dir(context.state_dir())? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err(NodeIdentityError::State(
                    "node state contains an unexpected file type".to_string(),
                ));
            }
            paths.push(entry.path());
        }
        for path in paths.iter().filter(|path| {
            path.file_name()
                .map(|name| name != IDENTITY_LOCK_FILE && name != LIFECYCLE_LOCK_FILE)
                .unwrap_or(false)
        }) {
            fs::remove_file(path)?;
        }
        drop(lock);
        Ok(true)
    }
}

impl NodeContext {
    pub fn load_or_initialize_identity(&self) -> Result<NodeIdentity, NodeIdentityError> {
        NodeIdentity::load_or_initialize(self)
    }
}

fn status_for_key(signing_key: &SigningKey) -> NodeIdentityStatus {
    let x_only_public_key = signing_key.verifying_key().to_bytes();
    let public_key_hex = hex::encode(x_only_public_key.as_ref());
    let node_id = node_id_for_x_only_public_key(x_only_public_key.as_ref());
    NodeIdentityStatus {
        public_key_hex,
        node_id,
    }
}

pub(crate) fn node_id_for_x_only_public_key(public_key: &[u8]) -> String {
    format!(
        "{NODE_ID_PREFIX}{}",
        hex::encode(&sha256_domain(NODE_ID_DOMAIN, public_key))
    )
}

fn read_private_key(
    context: &NodeContext,
    path: &Path,
) -> Result<[u8; PRIVATE_KEY_BYTES], NodeIdentityError> {
    crate::node_key::read_private_key(context, path, "identity state has an unexpected file type")
}

fn inspect_existing_state_file(path: &Path, label: &str) -> Result<bool, NodeIdentityError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err(NodeIdentityError::State(format!(
                    "{label} has an unexpected file type"
                )));
            }
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn reject_public_companion(state_dir: &Path) -> Result<(), NodeIdentityError> {
    let path = state_dir.join(IDENTITY_PUBLIC_FILE);
    match fs::symlink_metadata(path) {
        Ok(_) => Err(NodeIdentityError::State(
            "identity.pub is an unsupported identity-state extra".to_string(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn cleanup_identity_temps(state_dir: &Path) -> Result<(), NodeIdentityError> {
    for entry in fs::read_dir(state_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(".identity.key.tmp-") {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_dir() {
            return Err(NodeIdentityError::State(
                "identity temporary path is a directory".to_string(),
            ));
        }
        fs::remove_file(entry.path())?;
    }
    Ok(())
}

struct IdentityLock {
    file: fs::File,
}

impl IdentityLock {
    fn acquire(context: &NodeContext) -> Result<Self, NodeIdentityError> {
        let path = context.state_dir().join(IDENTITY_LOCK_FILE);
        let mut options = crate::util::fs::no_follow_open_options();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(NodeIdentityError::State(
                "identity lock is a symlink".to_string(),
            ));
        }
        context.validate_private_file(&path)?;
        file.lock_exclusive()?;
        Ok(Self { file })
    }
}

impl Drop for IdentityLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(debug_assertions)]
    use crate::domain::NodeConfig;
    use crate::test_support::node_context;

    #[cfg(debug_assertions)]
    use k256::schnorr::{signature::hazmat::PrehashVerifier, VerifyingKey};
    use serde::Deserialize;
    #[cfg(debug_assertions)]
    use std::sync::Arc;
    #[cfg(debug_assertions)]
    use std::thread;

    #[derive(Deserialize)]
    struct IdentityVectors {
        format_version: u8,
        curve: String,
        hash: String,
        domain_separator_hex: String,
        node_id_hash_input: String,
        private_key_encoding: String,
        public_key_encoding: String,
        signing_algorithm: String,
        normalization: String,
        identity_file: String,
        public_identity_file: String,
        node_id_prefix: String,
        vectors: Vec<IdentityVector>,
    }

    #[derive(Deserialize)]
    struct IdentityVector {
        input_scalar_hex: String,
        normalized_private_key_hex: String,
        x_only_public_key_hex: String,
        node_id: String,
    }

    #[cfg(debug_assertions)]
    fn vectors() -> IdentityVectors {
        toml::from_str(include_str!("../tests/fixtures/node_identity_vectors.toml")).unwrap()
    }

    #[test]
    fn corrected_vectors_match_normalized_scalar_x_only_key_and_node_id() {
        let fixture = vectors();
        assert_eq!(fixture.format_version, 2);
        assert_eq!(fixture.curve, "secp256k1");
        assert_eq!(fixture.hash, "SHA-256");
        assert_eq!(
            fixture.domain_separator_hex,
            "6f6d616b7572652f6e6f64652d69642f763100"
        );
        assert!(fixture.node_id_hash_input.contains("x_only_public_key"));
        assert_eq!(
            fixture.private_key_encoding,
            "normalized-scalar-32-byte-big-endian-hex"
        );
        assert_eq!(fixture.public_key_encoding, "x-only-bip340-hex-lowercase");
        assert_eq!(fixture.signing_algorithm, "BIP-340-Schnorr");
        assert_eq!(
            fixture.normalization,
            "even-y: d if y(dG) is even, otherwise n-d"
        );
        assert_eq!(fixture.identity_file, "identity.key");
        assert_eq!(fixture.public_identity_file, "none");
        assert_eq!(fixture.node_id_prefix, NODE_ID_PREFIX);
        assert_eq!(fixture.vectors.len(), 3);
        for vector in fixture.vectors {
            let signing_key =
                SigningKey::from_slice(&hex::decode(&vector.input_scalar_hex).unwrap()).unwrap();
            assert_eq!(
                hex::encode(signing_key.to_bytes().as_ref()),
                vector.normalized_private_key_hex
            );
            let status = status_for_key(&signing_key);
            assert_eq!(status.public_key_hex, vector.x_only_public_key_hex);
            assert_eq!(status.node_id, vector.node_id);
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn imported_odd_y_scalar_is_normalized_once_and_reopens_stably() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = node_context(tmp.path());
        let scalar =
            hex::decode("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140")
                .unwrap();
        let imported = NodeIdentity::import(&context, &scalar).unwrap();
        let status = imported.public_status().clone();
        assert_eq!(
            fs::read(context.identity_path()).unwrap(),
            vec![0; 31].into_iter().chain([1]).collect::<Vec<_>>()
        );
        assert!(!context.state_dir().join("identity.pub").exists());
        assert_eq!(
            NodeIdentity::load_or_initialize(&context)
                .unwrap()
                .public_status(),
            &status
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn first_initialization_is_single_file_and_reopens_stably() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = node_context(tmp.path());
        let first = NodeIdentity::load_or_initialize(&context).unwrap();
        let first_status = first.public_status().clone();
        let reopened = NodeIdentity::load_or_initialize(&context).unwrap();
        assert_eq!(&first_status, reopened.public_status());
        assert_eq!(
            fs::read(context.identity_path()).unwrap().len(),
            PRIVATE_KEY_BYTES
        );
        assert!(!context.state_dir().join("identity.pub").exists());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn concurrent_first_initialization_converges_on_one_identity() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = Arc::new(node_context(tmp.path()));
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let context = Arc::clone(&context);
                thread::spawn(move || {
                    NodeIdentity::load_or_initialize(&context)
                        .unwrap()
                        .public_status()
                        .clone()
                })
            })
            .collect();
        let statuses: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(statuses.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn malformed_or_non_normalized_existing_keys_fail_closed_without_regeneration() {
        let cases = [
            vec![0u8; 31],
            vec![0u8; 32],
            vec![0xffu8; 32],
            hex::decode("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140")
                .unwrap(),
        ];
        for bytes in cases {
            let tmp = tempfile::TempDir::new().unwrap();
            let context = node_context(tmp.path());
            context.ensure_state_directory().unwrap();
            fs::write(context.identity_path(), &bytes).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(context.identity_path(), fs::Permissions::from_mode(0o600))
                    .unwrap();
            }
            assert!(NodeIdentity::load_or_initialize(&context).is_err());
            assert_eq!(fs::read(context.identity_path()).unwrap(), bytes);
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn identity_pub_is_an_unsupported_extra_not_mismatch_state() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = node_context(tmp.path());
        context.ensure_state_directory().unwrap();
        fs::write(context.state_dir().join("identity.pub"), b"unsupported").unwrap();
        assert!(NodeIdentity::load_or_initialize(&context).is_err());
        assert!(!context.identity_path().exists());
    }

    #[cfg(all(unix, debug_assertions))]
    #[test]
    fn insecure_permissions_and_symlinks_fail_closed() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let tmp = tempfile::TempDir::new().unwrap();
        let context = node_context(tmp.path());
        context.ensure_state_directory().unwrap();
        fs::write(context.identity_path(), [1u8; 32]).unwrap();
        fs::set_permissions(context.identity_path(), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(NodeIdentity::load_or_initialize(&context).is_err());
        fs::remove_file(context.identity_path()).unwrap();
        let outside = tmp.path().join("outside.key");
        fs::write(&outside, [1u8; 32]).unwrap();
        symlink(&outside, context.identity_path()).unwrap();
        assert!(NodeIdentity::load_or_initialize(&context).is_err());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn interrupted_temps_and_write_failures_are_handled_without_replacement() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = node_context(tmp.path());
        context.ensure_state_directory().unwrap();
        let stale = context.state_dir().join(".identity.key.tmp-stale");
        fs::write(&stale, [7u8; 32]).unwrap();
        fs::create_dir(context.identity_path()).unwrap();
        assert!(NodeIdentity::load_or_initialize(&context).is_err());
        assert!(!stale.exists());
        assert!(context.identity_path().is_dir());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn typed_direct_signing_verifies_without_double_hashing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = node_context(tmp.path());
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let direct = DirectEnvelopePrehash::from_canonical_bytes(br#"{"a":1}"#);
        let direct_signature = identity.sign_direct_envelope(direct).unwrap();
        let verifying_key = VerifyingKey::from_slice(
            &hex::decode(&identity.public_status().public_key_hex).unwrap(),
        )
        .unwrap();
        let direct_signature = Signature::from_slice(&direct_signature.to_bytes()).unwrap();
        verifying_key
            .verify_prehash(direct.as_bytes(), &direct_signature)
            .unwrap();
    }

    #[cfg(debug_assertions)]
    #[test]
    fn public_surfaces_contain_no_private_material() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = node_context(tmp.path());
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let private = fs::read(context.identity_path()).unwrap();
        fs::write(
            context.config_path(),
            NodeConfig::default().to_toml().unwrap(),
        )
        .unwrap();
        let history = tmp.path().join(".history");
        fs::create_dir(&history).unwrap();
        fs::write(
            history.join("runs.sqlite"),
            identity.public_status().node_id.as_bytes(),
        )
        .unwrap();
        for path in [
            context.config_path().to_path_buf(),
            history.join("runs.sqlite"),
        ] {
            let contents = fs::read(path).unwrap();
            assert!(!contents
                .windows(private.len())
                .any(|window| window == private));
        }
        assert_eq!(identity.public_status().public_key_hex.len(), 64);
        assert!(hex::is_lower(&identity.public_status().public_key_hex));
        assert!(!format!("{:?}", identity.public_status()).contains("private"));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn existing_database_without_identity_fails_closed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = node_context(tmp.path());
        context.ensure_state_directory().unwrap();
        fs::write(context.database_path(), b"database placeholder").unwrap();
        assert!(NodeIdentity::load_or_initialize(&context).is_err());
    }
}

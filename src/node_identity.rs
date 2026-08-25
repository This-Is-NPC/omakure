use crate::node::{write_atomic_new, NodeContext, NodeError};
use crate::node_registry::RegistryError;
use fs2::FileExt;
use k256::elliptic_curve::Generate;
use k256::schnorr::{signature::hazmat::PrehashSigner, Signature, SigningKey};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use thiserror::Error;

const IDENTITY_PRIVATE_BYTES: usize = 32;
const NODE_ID_PREFIX: &str = "omk1_";
const NODE_ID_DOMAIN: &[u8] = b"omakure/node-id/v1\0";
const DIRECT_ENVELOPE_DOMAIN: &[u8] = b"omakure/direct-envelope/v1\0";

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
    #[error("prehash must be exactly 32 bytes")]
    InvalidPrehash,
    #[error("node trust registry error: {0}")]
    Registry(#[from] RegistryError),
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
        Self(sha256_domain(DIRECT_ENVELOPE_DOMAIN, bytes))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventId([u8; 32]);

impl EventId {
    /// Construct an event id that was computed by the NIP-01 serializer.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NodeIdentityError> {
        Ok(Self(
            bytes
                .try_into()
                .map_err(|_| NodeIdentityError::InvalidPrehash)?,
        ))
    }

    /// Hash NIP-01 serialized event bytes into the explicit event-id prehash.
    pub fn from_nip01_serialized(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
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
    context: NodeContext,
}

pub struct RotationPreparation {
    identity: NodeIdentity,
}

impl RotationPreparation {
    pub fn status(&self) -> &NodeIdentityStatus {
        &self.identity.status
    }
}

pub struct ResetPreparation;

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
            return Err(NodeIdentityError::State(
                "node state is not initialized".to_string(),
            ));
        }
        reject_public_companion(context.state_dir())?;
        let identity_path = context.identity_path();
        if !inspect_existing_state_file(&identity_path, "identity.key")? {
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
        Ok(Self::from_signing_key(context, signing_key))
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
        let identity_exists = inspect_existing_state_file(&identity_path, "identity.key")?;
        let database_exists = inspect_existing_state_file(&context.database_path(), "node.sqlite")?;
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
            write_atomic_new(&identity_path, normalized.as_ref(), 0o600)?;
            context.validate_private_file(&identity_path)?;
            signing_key
        };

        let identity = Self::from_signing_key(context, signing_key);
        if created_identity {
            context.open_trust_registry_for_initialization(identity.public_status())?;
        } else {
            context.open_trust_registry(identity.public_status())?;
        }
        Ok(identity)
    }

    fn from_signing_key(context: &NodeContext, signing_key: SigningKey) -> Self {
        let status = status_for_key(&signing_key);
        Self {
            signing_key,
            status,
            context: context.clone(),
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

    /// Sign an explicit 32-byte NIP-01 event id without hashing it again.
    pub fn sign_event_id(&self, event_id: EventId) -> Result<Bip340Signature, NodeIdentityError> {
        self.sign_prehash(&event_id.0)
    }

    pub(crate) fn sign_transport_certificate(
        &self,
        body: &[u8],
    ) -> Result<Bip340Signature, NodeIdentityError> {
        self.sign_prehash(&sha256_domain(b"omakure/transport-cert/v1\0", body))
    }

    pub(crate) fn sign_discovery(&self, body: &[u8]) -> Result<Bip340Signature, NodeIdentityError> {
        self.sign_prehash(&sha256_domain(b"omakure/lan-beacon/v1\0", body))
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

    pub fn prepare_rotation(&self) -> RotationPreparation {
        RotationPreparation {
            identity: Self::from_signing_key(&self.context, SigningKey::generate()),
        }
    }

    pub fn prepare_reset(&self) -> ResetPreparation {
        ResetPreparation
    }

    pub fn execute_reset(
        context: &NodeContext,
        _preparation: ResetPreparation,
    ) -> Result<(), NodeIdentityError> {
        context.ensure_state_directory()?;
        let _lock = IdentityLock::acquire(context)?;
        let path = context.identity_path();
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() =>
            {
                Err(NodeIdentityError::State(
                    "identity state has an unexpected file type".to_string(),
                ))
            }
            Ok(_) => {
                context.validate_private_file(&path)?;
                fs::remove_file(path)?;
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
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
                .map(|name| name != ".identity.lock" && name != ".node.lifecycle.lock")
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

    pub fn load_existing_identity(&self) -> Result<NodeIdentity, NodeIdentityError> {
        NodeIdentity::load_existing(self)
    }
}

fn status_for_key(signing_key: &SigningKey) -> NodeIdentityStatus {
    let x_only_public_key = signing_key.verifying_key().to_bytes();
    let public_key_hex = encode_hex(x_only_public_key.as_ref());
    let node_id = node_id_for_x_only_public_key(x_only_public_key.as_ref());
    NodeIdentityStatus {
        public_key_hex,
        node_id,
    }
}

pub(crate) fn node_id_for_x_only_public_key(public_key: &[u8]) -> String {
    format!(
        "{NODE_ID_PREFIX}{}",
        encode_hex(&sha256_domain(NODE_ID_DOMAIN, public_key))
    )
}

fn sha256_domain(domain: &[u8], bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes);
    digest.finalize().into()
}

fn read_private_key(
    context: &NodeContext,
    path: &Path,
) -> Result<[u8; IDENTITY_PRIVATE_BYTES], NodeIdentityError> {
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
        return Err(NodeIdentityError::State(
            "identity state has an unexpected file type".to_string(),
        ));
    }
    context.validate_private_file(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    bytes.try_into().map_err(|_| NodeIdentityError::InvalidKey)
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
    let path = state_dir.join("identity.pub");
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
        let path = context.state_dir().join(".identity.lock");
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
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

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(debug_assertions)]
    use crate::domain::NodeConfig;
    #[cfg(debug_assertions)]
    use crate::node::{NodePathOverrides, NodePlatform};
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

    fn vectors() -> IdentityVectors {
        toml::from_str(include_str!("../tests/fixtures/node_identity_vectors.toml")).unwrap()
    }

    fn decode_hex(value: &str) -> Vec<u8> {
        (0..value.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&value[index..index + 2], 16).unwrap())
            .collect()
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
                SigningKey::from_slice(&decode_hex(&vector.input_scalar_hex)).unwrap();
            assert_eq!(
                encode_hex(signing_key.to_bytes().as_ref()),
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
        let context = test_context(tmp.path());
        let scalar = decode_hex("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140");
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
        let context = test_context(tmp.path());
        let first = NodeIdentity::load_or_initialize(&context).unwrap();
        let first_status = first.public_status().clone();
        let reopened = NodeIdentity::load_or_initialize(&context).unwrap();
        assert_eq!(&first_status, reopened.public_status());
        assert_eq!(
            fs::read(context.identity_path()).unwrap().len(),
            IDENTITY_PRIVATE_BYTES
        );
        assert!(!context.state_dir().join("identity.pub").exists());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn concurrent_first_initialization_converges_on_one_identity() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = Arc::new(test_context(tmp.path()));
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
            decode_hex("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140"),
        ];
        for bytes in cases {
            let tmp = tempfile::TempDir::new().unwrap();
            let context = test_context(tmp.path());
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
        let context = test_context(tmp.path());
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
        let context = test_context(tmp.path());
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
        let context = test_context(tmp.path());
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
    fn typed_direct_and_event_signing_verify_without_double_hashing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = test_context(tmp.path());
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let direct = DirectEnvelopePrehash::from_canonical_bytes(br#"{"a":1}"#);
        let direct_signature = identity.sign_direct_envelope(direct).unwrap();
        let event_id = EventId::from_bytes(&[7u8; 32]).unwrap();
        let event_signature = identity.sign_event_id(event_id).unwrap();
        let verifying_key =
            VerifyingKey::from_slice(&decode_hex(&identity.public_status().public_key_hex))
                .unwrap();
        let direct_signature = Signature::from_slice(&direct_signature.to_bytes()).unwrap();
        let event_signature = Signature::from_slice(&event_signature.to_bytes()).unwrap();
        verifying_key
            .verify_prehash(direct.as_bytes(), &direct_signature)
            .unwrap();
        verifying_key
            .verify_prehash(event_id.as_bytes(), &event_signature)
            .unwrap();
        assert_ne!(direct_signature.to_bytes(), event_signature.to_bytes());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn public_surfaces_contain_no_private_material() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = test_context(tmp.path());
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
        assert!(identity
            .public_status()
            .public_key_hex
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase()));
        assert!(!format!("{:?}", identity.public_status()).contains("private"));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn rotation_and_reset_are_explicit_hooks() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = test_context(tmp.path());
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        let current = identity.public_status().clone();
        let rotation = identity.prepare_rotation();
        assert_ne!(rotation.status(), &current);
        let reset = identity.prepare_reset();
        assert!(context.identity_path().is_file());
        NodeIdentity::execute_reset(&context, reset).unwrap();
        assert!(!context.identity_path().exists());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn existing_database_without_identity_fails_closed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let context = test_context(tmp.path());
        context.ensure_state_directory().unwrap();
        fs::write(context.database_path(), b"database placeholder").unwrap();
        assert!(NodeIdentity::load_or_initialize(&context).is_err());
    }
}

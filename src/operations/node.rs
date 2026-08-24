use crate::domain::{NodeConfig, NodeConfigError};
use crate::node::{NodeContext, NodeError, NodePathOverrides};
use crate::node_identity::{NodeIdentity, NodeIdentityError};
use crate::node_registry::{
    NodeRegistry, PeerRecord, PeerRegistration, PeerRole, PeerSource, PeerState, RegistryError,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read};

use super::{OperationError, OperationErrorCode, OperationResult};

const PUBLIC_PEER_LIMIT: usize = 256;
const MAX_NODE_CONFIG_BYTES: usize = 64 * 1024;

pub fn resolve_context(overrides: NodePathOverrides) -> OperationResult<NodeContext> {
    NodeContext::resolve(overrides).map_err(map_node_error)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeStatus {
    pub initialized: bool,
    pub identity: Option<PublicIdentity>,
    pub config: Option<PublicNodeConfig>,
    pub trust: TrustSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicIdentity {
    pub node_id: String,
    pub public_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicNodeConfig {
    pub display_name: String,
    pub api_bind: String,
    pub network_mode: String,
    pub relays: Vec<String>,
    pub static_peers: Vec<String>,
    pub max_message_bytes: u64,
    pub enrollment: String,
    pub allow_remote_cues: bool,
    pub allow_baseline_push: bool,
    pub organization_id: String,
    pub discovery_secret_configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TrustSummary {
    pub registry_initialized: bool,
    pub peer_count: usize,
    pub active_peer_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeInitializationResult {
    pub state_dir_created: bool,
    pub config_created: bool,
    pub identity_created: bool,
    pub registry_created: bool,
    pub status: NodeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicPeer {
    pub node_id: String,
    pub public_key: String,
    pub role: String,
    pub state: String,
    pub capabilities: Vec<String>,
    pub added_at: String,
    pub updated_at: String,
    pub last_seen: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ManualTrustRequest {
    pub node_id: String,
    pub public_key: String,
    pub role: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub actor: String,
    pub reason: String,
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CapabilityUpdateRequest {
    pub node_id: String,
    pub capabilities: Vec<String>,
    pub actor: String,
    pub reason: String,
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RevocationRequest {
    pub node_id: String,
    pub actor: String,
    pub reason: String,
    pub confirmed: bool,
}

pub fn initialize_node(
    context: &NodeContext,
    config: &NodeConfig,
) -> OperationResult<NodeInitializationResult> {
    let state_was_present = context
        .validate_existing_state_directory()
        .map_err(map_node_error)?;
    let identity_was_present = path_is_present(&context.identity_path(), "identity.key")?;
    let registry_was_present = path_is_present(&context.database_path(), "node.sqlite")?;
    let initialization = context.initialize(config).map_err(map_node_error)?;
    let _identity = context
        .load_or_initialize_identity()
        .map_err(map_identity_error)?;
    let status = public_node_status(context)?;
    Ok(NodeInitializationResult {
        state_dir_created: initialization.state_dir_created && !state_was_present,
        config_created: initialization.config_created,
        identity_created: !identity_was_present,
        registry_created: !registry_was_present,
        status,
    })
}

/// Inspect node state without creating a directory, identity, lock, or
/// registry. Corrupt or inconsistent state is surfaced instead of being
/// replaced with a fresh identity.
pub fn public_node_status(context: &NodeContext) -> OperationResult<NodeStatus> {
    let config = read_public_config(context)?;
    let state_present = context
        .validate_existing_state_directory()
        .map_err(map_node_error)?;
    if !state_present {
        return Ok(NodeStatus {
            initialized: false,
            identity: None,
            config,
            trust: TrustSummary {
                registry_initialized: false,
                peer_count: 0,
                active_peer_count: 0,
            },
        });
    }

    let identity_present = path_is_present(&context.identity_path(), "identity.key")?;
    let registry_present = path_is_present(&context.database_path(), "node.sqlite")?;
    let public_companion = context.state_dir().join("identity.pub");
    if path_is_present(&public_companion, "identity.pub")? {
        return Err(registry_error("unsupported identity state extra"));
    }
    if identity_present != registry_present {
        return Err(registry_error(
            "node identity and trust registry state are inconsistent",
        ));
    }
    if !identity_present {
        return Ok(NodeStatus {
            initialized: false,
            identity: None,
            config,
            trust: TrustSummary {
                registry_initialized: false,
                peer_count: 0,
                active_peer_count: 0,
            },
        });
    }

    let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())
        .map_err(map_registry_error)?;
    let counts = registry.peer_counts().map_err(map_registry_error)?;
    Ok(NodeStatus {
        initialized: config.is_some(),
        identity: Some(public_identity(identity)),
        config,
        trust: TrustSummary {
            registry_initialized: true,
            peer_count: counts.total,
            active_peer_count: counts.active,
        },
    })
}

pub fn list_trusted_peers(context: &NodeContext) -> OperationResult<Vec<PublicPeer>> {
    let registry = open_initialized_registry(context)?;
    registry
        .peers_limited(PUBLIC_PEER_LIMIT)
        .map_err(map_registry_error)
        .map(|peers| peers.into_iter().map(public_peer).collect())
}

pub fn import_manual_trust(
    context: &NodeContext,
    request: ManualTrustRequest,
) -> OperationResult<PublicPeer> {
    require_confirmation(request.confirmed)?;
    let registry = open_initialized_registry(context)?;
    let registration = PeerRegistration {
        node_id: request.node_id,
        public_key: request.public_key,
        role: parse_role(&request.role)?,
        capabilities: request.capabilities,
        source: PeerSource::Manual,
        actor: request.actor,
        reason: request.reason,
    };
    registry
        .import_manual_peer(registration)
        .map_err(map_registry_error)
        .map(public_peer)
}

pub fn update_peer_capabilities(
    context: &NodeContext,
    request: CapabilityUpdateRequest,
) -> OperationResult<PublicPeer> {
    require_confirmation(request.confirmed)?;
    let registry = open_initialized_registry(context)?;
    registry
        .update_peer_capabilities(
            &request.node_id,
            request.capabilities,
            &request.actor,
            &request.reason,
        )
        .map_err(map_registry_error)
        .map(public_peer)
}

pub fn revoke_peer(
    context: &NodeContext,
    request: RevocationRequest,
) -> OperationResult<PublicPeer> {
    require_confirmation(request.confirmed)?;
    let registry = open_initialized_registry(context)?;
    registry
        .revoke_peer(&request.node_id, &request.actor, &request.reason)
        .map_err(map_registry_error)
        .map(public_peer)
}

fn open_initialized_registry(context: &NodeContext) -> OperationResult<NodeRegistry> {
    let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
    NodeRegistry::open_existing(context, identity.public_status()).map_err(map_registry_error)
}

fn read_public_config(context: &NodeContext) -> OperationResult<Option<PublicNodeConfig>> {
    let Some(file) = context.open_public_file().map_err(map_node_error)? else {
        return Ok(None);
    };
    let metadata = file.metadata().map_err(map_io_error)?;
    if metadata.len() > MAX_NODE_CONFIG_BYTES as u64 {
        return Err(registry_error("node configuration exceeds maximum size"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_NODE_CONFIG_BYTES as u64) + 1)
        .read_to_end(&mut bytes)
        .map_err(map_io_error)?;
    if bytes.len() > MAX_NODE_CONFIG_BYTES {
        return Err(registry_error("node configuration exceeds maximum size"));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| registry_error("node configuration is invalid or corrupt"))?;
    let config = NodeConfig::parse(&text)
        .map_err(|_| registry_error("node configuration is invalid or corrupt"))?;
    Ok(Some(public_config(config)))
}

fn path_is_present(path: &std::path::Path, label: &str) -> OperationResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            Err(registry_error(format!(
                "{label} has an unexpected file type"
            )))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(map_io_error(error)),
    }
}

fn public_identity(identity: NodeIdentity) -> PublicIdentity {
    PublicIdentity {
        node_id: identity.public_status().node_id.clone(),
        public_key: identity.public_status().public_key_hex.clone(),
    }
}

fn public_config(config: NodeConfig) -> PublicNodeConfig {
    PublicNodeConfig {
        display_name: config.node.display_name,
        api_bind: config.api.bind,
        network_mode: config.network.mode,
        relays: config.network.relays,
        static_peers: config.network.static_peers,
        max_message_bytes: config.network.max_message_bytes,
        enrollment: config.trust.enrollment,
        allow_remote_cues: config.trust.allow_remote_cues,
        allow_baseline_push: config.trust.allow_baseline_push,
        organization_id: config.organization.id,
        discovery_secret_configured: !config.organization.discovery_secret_ref.is_empty(),
    }
}

fn public_peer(peer: PeerRecord) -> PublicPeer {
    PublicPeer {
        node_id: peer.node_id,
        public_key: peer.public_key,
        role: role_string(peer.role),
        state: state_string(peer.state),
        capabilities: peer.capabilities,
        added_at: peer.added_at,
        updated_at: peer.updated_at,
        last_seen: peer.last_seen,
        source: source_string(peer.source),
    }
}

fn parse_role(value: &str) -> OperationResult<PeerRole> {
    match value {
        "conductor" => Ok(PeerRole::Conductor),
        "performer" => Ok(PeerRole::Performer),
        _ => Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "role must be conductor or performer",
        )),
    }
}

fn role_string(role: PeerRole) -> String {
    match role {
        PeerRole::Conductor => "conductor",
        PeerRole::Performer => "performer",
    }
    .to_string()
}

fn state_string(state: PeerState) -> String {
    state.to_string()
}

fn source_string(source: PeerSource) -> String {
    match source {
        PeerSource::Manual => "manual",
        PeerSource::Bundle => "bundle",
        PeerSource::Recovery => "recovery",
    }
    .to_string()
}

fn require_confirmation(confirmed: bool) -> OperationResult<()> {
    if confirmed {
        Ok(())
    } else {
        Err(OperationError::new(
            OperationErrorCode::Forbidden,
            "explicit confirmation is required for trust mutation",
        ))
    }
}

fn map_config_error(error: NodeConfigError) -> OperationError {
    OperationError::new(OperationErrorCode::InvalidInput, error.to_string())
}

fn map_node_error(error: NodeError) -> OperationError {
    match error {
        NodeError::Config(error) => map_config_error(error),
        NodeError::InvalidPath { .. }
        | NodeError::TestOverrideOutsideTestMode
        | NodeError::IncompleteTestOverrides => {
            OperationError::new(OperationErrorCode::InvalidInput, error.to_string())
        }
        NodeError::InsecurePath(_)
        | NodeError::UnsafePath(_)
        | NodeError::UnexpectedFileType(_)
        | NodeError::ExistingConfig(_) => registry_error("node state is invalid or insecure"),
        NodeError::TestModeUnavailable => registry_error("node test mode is unavailable"),
        NodeError::Io(_) => OperationError::new(OperationErrorCode::IoFailed, error.to_string()),
    }
}

fn map_identity_error(error: NodeIdentityError) -> OperationError {
    match error {
        NodeIdentityError::Node(error) => map_node_error(error),
        NodeIdentityError::Registry(error) => map_registry_error(error),
        NodeIdentityError::InvalidKey | NodeIdentityError::State(_) => {
            registry_error("node identity state is invalid or insecure")
        }
        NodeIdentityError::Io(_)
        | NodeIdentityError::Signing
        | NodeIdentityError::InvalidPrehash => {
            OperationError::new(OperationErrorCode::IoFailed, error.to_string())
        }
    }
}

fn map_registry_error(error: RegistryError) -> OperationError {
    match error {
        RegistryError::InvalidInput(error) => {
            OperationError::new(OperationErrorCode::InvalidInput, error)
        }
        RegistryError::Duplicate(error) | RegistryError::Revoked(error) => {
            OperationError::new(OperationErrorCode::Conflict, error)
        }
        RegistryError::InvalidTransition { from, to } => OperationError::new(
            OperationErrorCode::Conflict,
            format!("invalid trust transition from {from} to {to}"),
        ),
        RegistryError::Unchanged(error) => OperationError::new(OperationErrorCode::Conflict, error),
        RegistryError::NotFound(error) => OperationError::new(OperationErrorCode::NotFound, error),
        RegistryError::InvalidSchema(_) | RegistryError::Corrupt(_) => {
            registry_error("node trust registry is invalid or corrupt")
        }
        RegistryError::Io(error) => {
            OperationError::new(OperationErrorCode::IoFailed, error.to_string())
        }
        RegistryError::Sqlite(_) => registry_error("node trust registry is unavailable"),
        RegistryError::Node(error) => map_node_error(error),
        RegistryError::SelfTrust => {
            OperationError::new(OperationErrorCode::Conflict, "peer cannot trust itself")
        }
    }
}

fn map_io_error(error: io::Error) -> OperationError {
    OperationError::new(OperationErrorCode::IoFailed, error.to_string())
}

fn registry_error(message: impl Into<String>) -> OperationError {
    OperationError::new(OperationErrorCode::RegistryInvalid, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NodePathOverrides, NodePlatform};
    use tempfile::TempDir;

    fn context(temp: &TempDir) -> NodeContext {
        NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::new(
                Some(temp.path().join("state")),
                Some(temp.path().join("node.toml")),
            ),
            true,
            None,
            None,
            None,
        )
        .unwrap()
    }

    fn peer_request(identity: &NodeIdentity) -> ManualTrustRequest {
        let key = k256::schnorr::SigningKey::from_slice(&[3; 32]).unwrap();
        let public_key = key
            .verifying_key()
            .to_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let node_id =
            crate::node_identity::node_id_for_x_only_public_key(&key.verifying_key().to_bytes());
        assert_ne!(node_id, identity.public_status().node_id);
        ManualTrustRequest {
            node_id,
            public_key,
            role: "performer".into(),
            capabilities: vec!["remote-run".into()],
            actor: "operator".into(),
            reason: "approved manually".into(),
            confirmed: true,
        }
    }

    #[test]
    fn status_is_observational_and_mutations_require_evidence() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        let before = public_node_status(&context).unwrap();
        assert!(!before.initialized);
        assert!(!context.state_dir().exists());

        let initialized = initialize_node(&context, &NodeConfig::default()).unwrap();
        assert!(initialized.status.initialized);
        let identity = NodeIdentity::load_existing(&context).unwrap();
        let mut request = peer_request(&identity);
        request.confirmed = false;
        let error = import_manual_trust(&context, request).unwrap_err();
        assert_eq!(error.code, OperationErrorCode::Forbidden);
        assert!(list_trusted_peers(&context).unwrap().is_empty());
    }

    #[test]
    fn status_treats_missing_config_parent_as_uninitialized() {
        let temp = TempDir::new().unwrap();
        let context = NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::new(
                Some(temp.path().join("state")),
                Some(temp.path().join("missing/node.toml")),
            ),
            true,
            None,
            None,
            None,
        )
        .unwrap();

        let status = public_node_status(&context).unwrap();
        assert!(!status.initialized);
        assert!(status.identity.is_none());
        assert!(status.config.is_none());
    }

    #[test]
    fn manual_import_update_and_revoke_are_public_and_replay_safe() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        initialize_node(&context, &NodeConfig::default()).unwrap();
        let identity = NodeIdentity::load_existing(&context).unwrap();
        let request = peer_request(&identity);
        let node_id = request.node_id.clone();
        let peer = import_manual_trust(&context, request.clone()).unwrap();
        assert_eq!(peer.state, "active");
        let registry = NodeIdentity::load_existing(&context)
            .unwrap()
            .open_trust_registry()
            .unwrap();
        let audit = registry.audit_events().unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].actor, "operator");
        assert_eq!(audit[0].reason, "approved manually");
        assert!(import_manual_trust(&context, request).is_err());
        let updated = update_peer_capabilities(
            &context,
            CapabilityUpdateRequest {
                node_id: node_id.clone(),
                capabilities: vec!["notifications".into()],
                actor: "operator".into(),
                reason: "narrowed".into(),
                confirmed: true,
            },
        )
        .unwrap();
        assert_eq!(updated.capabilities, vec!["notifications"]);
        let revoked = revoke_peer(
            &context,
            RevocationRequest {
                node_id: node_id.clone(),
                actor: "operator".into(),
                reason: "retired".into(),
                confirmed: true,
            },
        )
        .unwrap();
        assert_eq!(revoked.state, "revoked");
        assert!(revoke_peer(
            &context,
            RevocationRequest {
                node_id,
                actor: "operator".into(),
                reason: "replay".into(),
                confirmed: true,
            },
        )
        .is_err());
    }

    #[test]
    fn status_fails_closed_on_corrupt_registry_without_replacing_identity() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        initialize_node(&context, &NodeConfig::default()).unwrap();
        let private_before = std::fs::read(context.identity_path()).unwrap();
        std::fs::write(context.database_path(), b"not a sqlite database").unwrap();

        let error = public_node_status(&context).unwrap_err();
        assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
        assert_eq!(
            std::fs::read(context.identity_path()).unwrap(),
            private_before
        );
    }

    #[test]
    fn status_redacts_malformed_config_values() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        initialize_node(&context, &NodeConfig::default()).unwrap();
        let secret = "relay-user-super-secret-value";
        let malformed = NodeConfig::default()
            .to_toml()
            .unwrap()
            .replace("mode = \"direct\"", "mode = \"nostr\"")
            .replace(
                "relays = []",
                &format!("relays = [\"wss://user:{secret}@relay.example.test\"]"),
            );
        std::fs::write(context.config_path(), malformed).unwrap();

        let error = public_node_status(&context).unwrap_err();
        assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
        assert_eq!(error.message, "node configuration is invalid or corrupt");
        assert!(!error.message.contains(secret));
    }

    #[test]
    fn status_rejects_oversized_config_before_parsing() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        initialize_node(&context, &NodeConfig::default()).unwrap();
        std::fs::write(
            context.config_path(),
            format!(
                "{}\n#{}",
                NodeConfig::default().to_toml().unwrap(),
                "x".repeat(MAX_NODE_CONFIG_BYTES)
            ),
        )
        .unwrap();

        let error = public_node_status(&context).unwrap_err();
        assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
        assert_eq!(error.message, "node configuration exceeds maximum size");
    }

    #[cfg(unix)]
    #[test]
    fn status_rejects_insecure_public_config_mode_before_reading() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        initialize_node(&context, &NodeConfig::default()).unwrap();
        std::fs::set_permissions(
            context.config_path(),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        let error = public_node_status(&context).unwrap_err();
        assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
        assert_eq!(error.message, "node state is invalid or insecure");
    }

    #[cfg(unix)]
    #[test]
    fn status_rejects_final_and_intermediate_config_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        initialize_node(&context, &NodeConfig::default()).unwrap();

        let outside = temp.path().join("outside.toml");
        std::fs::write(&outside, NodeConfig::default().to_toml().unwrap()).unwrap();
        std::fs::remove_file(context.config_path()).unwrap();
        symlink(&outside, context.config_path()).unwrap();
        let error = public_node_status(&context).unwrap_err();
        assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
        assert_eq!(error.message, "node state is invalid or insecure");

        std::fs::remove_file(context.config_path()).unwrap();
        let real_parent = temp.path().join("real-config");
        let link_parent = temp.path().join("linked-config");
        std::fs::create_dir(&real_parent).unwrap();
        let real_config = real_parent.join("node.toml");
        std::fs::write(&real_config, NodeConfig::default().to_toml().unwrap()).unwrap();
        symlink(&real_parent, &link_parent).unwrap();
        let linked_context = NodeContext::resolve_for(
            NodePlatform::Linux,
            NodePathOverrides::new(
                Some(context.state_dir().to_path_buf()),
                Some(link_parent.join("node.toml")),
            ),
            true,
            None,
            None,
            None,
        )
        .unwrap();
        let error = public_node_status(&linked_context).unwrap_err();
        assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
        assert_eq!(error.message, "node state is invalid or insecure");
    }
}

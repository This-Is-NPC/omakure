use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::discovery::configured_discovery_status;
use super::errors::{map_identity_error, map_node_error, map_registry_error, registry_error};
use crate::domain::NodeConfig;
use crate::node::{
    NodeContext, DATABASE_FILE, IDENTITY_KEY_FILE, IDENTITY_PUBLIC_FILE,
    TRANSPORT_CERTIFICATE_FILE, TRANSPORT_KEY_FILE,
};
use crate::node_identity::NodeIdentity;
use crate::node_registry::NodeRegistry;
use crate::node_transport::LocalTransport;
use serde::Serialize;
use std::fs;
use std::io::{self, Read};

pub(super) const MAX_NODE_CONFIG_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeStatus {
    pub initialized: bool,
    pub identity: Option<PublicIdentity>,
    pub config: Option<PublicNodeConfig>,
    pub trust: TrustSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<crate::direct_service::TransportStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discovery: Option<crate::discovery::DiscoveryStatus>,
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
    pub static_peers: Vec<String>,
    pub direct_bind: Option<String>,
    pub enrollment: String,
    pub allow_remote_cues: bool,
    pub allow_baseline_push: bool,
    pub organization_id: String,
    pub discovery_secret_configured: bool,
    pub discovery_enabled: bool,
    pub discovery_port: u16,
    pub discovery_broadcast: bool,
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
pub struct NodeResetResult {
    pub state_removed: bool,
    pub trust_removed: bool,
    pub identity_removed: bool,
}

pub fn initialize_node(
    context: &NodeContext,
    config: &NodeConfig,
) -> OperationResult<NodeInitializationResult> {
    let lifecycle = context.acquire_lifecycle_lock().map_err(map_node_error)?;
    context
        .validate_existing_state_directory()
        .map_err(map_node_error)?;
    initialize_node_locked(context, config, lifecycle.state_was_present())
}

/// Initialize through an exposed control surface without waiting behind the
/// long-lived node-service lifecycle lock. Genuine first initialization callers
/// should use `initialize_node`, which preserves serialized convergence.
pub fn initialize_node_nonblocking(
    context: &NodeContext,
    config: &NodeConfig,
) -> OperationResult<NodeInitializationResult> {
    let lifecycle = context
        .try_acquire_lifecycle_lock()
        .map_err(map_node_error)?;
    context
        .validate_existing_state_directory()
        .map_err(map_node_error)?;
    initialize_node_locked(context, config, lifecycle.state_was_present())
}

pub(crate) fn initialize_node_locked(
    context: &NodeContext,
    config: &NodeConfig,
    state_was_present: bool,
) -> OperationResult<NodeInitializationResult> {
    let _state_contents_present = context
        .validate_existing_state_contents()
        .map_err(map_node_error)?;
    let identity_was_present = path_is_present(&context.identity_path(), IDENTITY_KEY_FILE)?;
    let registry_was_present = path_is_present(&context.database_path(), DATABASE_FILE)?;
    if state_was_present
        && (identity_was_present || registry_was_present)
        && read_node_config(context)?.is_none()
    {
        return Err(registry_error("node configuration is missing"));
    }
    let initialization = context.initialize(config).map_err(map_node_error)?;
    let identity = context
        .load_or_initialize_identity()
        .map_err(map_identity_error)?;
    let transport_key_was_present =
        path_is_present(&context.transport_key_path(), TRANSPORT_KEY_FILE)?;
    let transport_certificate_was_present = path_is_present(
        &context.transport_certificate_path(),
        TRANSPORT_CERTIFICATE_FILE,
    )?;
    let first_machine_creation = !identity_was_present
        && !registry_was_present
        && !transport_key_was_present
        && !transport_certificate_was_present;
    if !first_machine_creation && (!identity_was_present || !registry_was_present) {
        return Err(registry_error("node machine state is incomplete"));
    }
    let provision = if first_machine_creation {
        LocalTransport::provision_new(context, &identity)
    } else {
        LocalTransport::load_existing(context, &identity)
    };
    provision.map_err(|error| {
        OperationError::new(
            OperationErrorCode::RegistryInvalid,
            format!("transport provisioning failed: {error}"),
        )
    })?;
    let status = public_node_status(context)?;
    Ok(NodeInitializationResult {
        state_dir_created: !state_was_present,
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
    let configured_discovery = configured_discovery_status(config.as_ref());
    let state_present = context
        .validate_existing_state_contents()
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
            transport: None,
            discovery: Some(configured_discovery),
        });
    }

    let identity_present = path_is_present(&context.identity_path(), IDENTITY_KEY_FILE)?;
    let registry_present = path_is_present(&context.database_path(), DATABASE_FILE)?;
    let public_companion = context.state_dir().join(IDENTITY_PUBLIC_FILE);
    if path_is_present(&public_companion, IDENTITY_PUBLIC_FILE)? {
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
            transport: None,
            discovery: Some(configured_discovery),
        });
    }

    let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
    let registry = NodeRegistry::open_health_observational(context, identity.public_status())
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
        transport: None,
        discovery: Some(configured_discovery),
    })
}

pub fn reset_node(context: &NodeContext, confirmed: bool) -> OperationResult<NodeResetResult> {
    if !confirmed {
        return Err(OperationError::new(
            OperationErrorCode::Forbidden,
            "explicit confirmation is required for node factory reset",
        ));
    }
    let state_exists = match std::fs::symlink_metadata(context.state_dir()) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(map_node_error(error.into())),
    };
    if !state_exists {
        return Ok(NodeResetResult {
            state_removed: false,
            trust_removed: false,
            identity_removed: false,
        });
    }
    let _lifecycle = context
        .try_acquire_lifecycle_lock()
        .map_err(map_node_error)?;
    if !context
        .validate_existing_state_directory()
        .map_err(map_node_error)?
    {
        return Ok(NodeResetResult {
            state_removed: false,
            trust_removed: false,
            identity_removed: false,
        });
    }
    let had_identity = path_is_present(&context.identity_path(), IDENTITY_KEY_FILE)?;
    let had_registry = path_is_present(&context.database_path(), DATABASE_FILE)?;
    let removed = NodeIdentity::execute_factory_reset(context).map_err(map_identity_error)?;
    Ok(NodeResetResult {
        state_removed: removed,
        trust_removed: removed && had_registry,
        identity_removed: removed && had_identity,
    })
}

/// The trust store, for the one caller outside this module that needs it.
///
/// `BaselinePublisher::create` asks the registry whether this node already
/// conducts anyone, so creating a publisher key needs a registry handle. It is
/// exposed here rather than opened by the CLI so the identity load, the error
/// mapping, and the security validation stay in one place.
pub fn open_registry_for_baseline(context: &NodeContext) -> OperationResult<NodeRegistry> {
    open_initialized_registry(context)
}

pub(super) fn open_initialized_registry(context: &NodeContext) -> OperationResult<NodeRegistry> {
    let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
    NodeRegistry::open_existing(context, identity.public_status()).map_err(map_registry_error)
}

fn read_public_config(context: &NodeContext) -> OperationResult<Option<PublicNodeConfig>> {
    Ok(read_node_config(context)?.map(public_config))
}

pub fn load_node_config(context: &NodeContext) -> OperationResult<NodeConfig> {
    read_node_config(context)?.ok_or_else(|| registry_error("node configuration is missing"))
}

pub(super) fn read_node_config(context: &NodeContext) -> OperationResult<Option<NodeConfig>> {
    let Some(file) = context.open_public_file().map_err(map_node_error)? else {
        return Ok(None);
    };
    let metadata = file.metadata().map_err(super::super::io_error)?;
    if metadata.len() > MAX_NODE_CONFIG_BYTES as u64 {
        return Err(registry_error("node configuration exceeds maximum size"));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_NODE_CONFIG_BYTES as u64) + 1)
        .read_to_end(&mut bytes)
        .map_err(super::super::io_error)?;
    if bytes.len() > MAX_NODE_CONFIG_BYTES {
        return Err(registry_error("node configuration exceeds maximum size"));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| registry_error("node configuration is invalid or corrupt"))?;
    let config = NodeConfig::parse(&text)
        .map_err(|_| registry_error("node configuration is invalid or corrupt"))?;
    Ok(Some(config))
}

pub(super) fn path_is_present(path: &std::path::Path, label: &str) -> OperationResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            Err(registry_error(format!(
                "{label} has an unexpected file type"
            )))
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(super::super::io_error(error)),
    }
}

fn public_identity(identity: NodeIdentity) -> PublicIdentity {
    PublicIdentity {
        node_id: identity.public_status().node_id.clone(),
        public_key: identity.public_status().public_key_hex.clone(),
    }
}

pub(super) fn public_config(config: NodeConfig) -> PublicNodeConfig {
    PublicNodeConfig {
        display_name: config.node.display_name,
        api_bind: config.api.bind,
        static_peers: config.network.static_peers,
        direct_bind: config.network.direct_bind,
        enrollment: config.trust.enrollment,
        allow_remote_cues: config.trust.allow_remote_cues,
        allow_baseline_push: config.trust.allow_baseline_push,
        organization_id: config.organization.id,
        discovery_secret_configured: !config.organization.discovery_secret_ref.is_empty(),
        discovery_enabled: config.discovery.enabled,
        discovery_port: config.discovery.port,
        discovery_broadcast: config.discovery.broadcast,
    }
}

use crate::domain::{NodeConfig, NodeConfigError};
use crate::enrollment::{self, EnrollmentError, EnrollmentRole, ManualEnrollmentRequest};
use crate::node::{NodeContext, NodeError, NodePathOverrides};
use crate::node_identity::{NodeIdentity, NodeIdentityError};
use crate::node_registry::{
    NodeRegistry, PeerRecord, PeerRegistration, PeerRole, PeerSource, PeerState, RegistryError,
};
use crate::node_transport::LocalTransport;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<crate::direct_service::TransportStatus>,
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
    pub direct_bind: Option<String>,
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
pub struct NodeResetResult {
    pub state_removed: bool,
    pub trust_removed: bool,
    pub identity_removed: bool,
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
    #[serde(default)]
    pub transport_certificate: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ManualEnrollmentApprovalRequest {
    pub request_hex: String,
    pub transport_certificate: String,
    pub code: String,
    pub actor: String,
    pub reason: String,
    pub confirmed: bool,
    pub expected_node_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ManualEnrollmentRejectionRequest {
    pub node_id: String,
    pub actor: String,
    pub reason: String,
    pub confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManualEnrollmentResult {
    pub pairing_id: String,
    pub request_id: String,
    pub request_hex: String,
    pub code: String,
    pub state: String,
    pub reciprocal_request_hex: Option<String>,
    pub reciprocal_code: Option<String>,
}

pub fn initialize_node(
    context: &NodeContext,
    config: &NodeConfig,
) -> OperationResult<NodeInitializationResult> {
    let state_was_present = context
        .validate_existing_state_directory()
        .map_err(map_node_error)?;
    let _lifecycle = context.acquire_lifecycle_lock().map_err(map_node_error)?;
    initialize_node_locked(context, config, state_was_present)
}

/// Initialize through an exposed control surface without waiting behind the
/// long-lived node-service lifecycle lock. Genuine first initialization callers
/// should use `initialize_node`, which preserves serialized convergence.
pub fn initialize_node_nonblocking(
    context: &NodeContext,
    config: &NodeConfig,
) -> OperationResult<NodeInitializationResult> {
    let state_was_present = context
        .validate_existing_state_directory()
        .map_err(map_node_error)?;
    let _lifecycle = context
        .try_acquire_lifecycle_lock()
        .map_err(map_node_error)?;
    initialize_node_locked(context, config, state_was_present)
}

pub(crate) fn initialize_node_locked(
    context: &NodeContext,
    config: &NodeConfig,
    state_was_present: bool,
) -> OperationResult<NodeInitializationResult> {
    let _state_contents_present = context
        .validate_existing_state_contents()
        .map_err(map_node_error)?;
    let identity_was_present = path_is_present(&context.identity_path(), "identity.key")?;
    let registry_was_present = path_is_present(&context.database_path(), "node.sqlite")?;
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
        path_is_present(&context.transport_key_path(), "transport.key")?;
    let transport_certificate_was_present =
        path_is_present(&context.transport_certificate_path(), "transport.cert")?;
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
            transport: None,
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
        transport: None,
    })
}

pub fn reset_node(context: &NodeContext, confirmed: bool) -> OperationResult<NodeResetResult> {
    if !confirmed {
        return Err(OperationError::new(
            OperationErrorCode::Forbidden,
            "explicit confirmation is required for node factory reset",
        ));
    }
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
    let _lifecycle = context
        .try_acquire_lifecycle_lock()
        .map_err(map_node_error)?;
    let had_identity = path_is_present(&context.identity_path(), "identity.key")?;
    let had_registry = path_is_present(&context.database_path(), "node.sqlite")?;
    let removed = NodeIdentity::execute_factory_reset(context).map_err(map_identity_error)?;
    Ok(NodeResetResult {
        state_removed: removed,
        trust_removed: removed && had_registry,
        identity_removed: removed && had_identity,
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
    let certificate = request
        .transport_certificate
        .as_deref()
        .map(decode_transport_certificate)
        .transpose()?;
    registry
        .import_manual_peer_with_transport(registration, certificate.as_deref())
        .map_err(map_registry_error)
        .map(public_peer)
}

fn decode_transport_certificate(value: &str) -> OperationResult<Vec<u8>> {
    if value.len() != crate::direct_transport::MAX_CERTIFICATE_BYTES * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "transport certificate must be lowercase hexadecimal bytes",
        ));
    }
    let bytes: Vec<u8> = (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16).map_err(|_| {
                OperationError::new(
                    OperationErrorCode::InvalidInput,
                    "transport certificate is not valid hexadecimal",
                )
            })
        })
        .collect::<OperationResult<Vec<_>>>()?;
    crate::direct_transport::TransportCertificate::from_bytes(&bytes).map_err(|error| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("transport certificate is invalid: {error}"),
        )
    })?;
    Ok(bytes)
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

pub fn manual_enrollment_enabled(context: &NodeContext) -> OperationResult<()> {
    let config = read_node_config(context)?
        .ok_or_else(|| registry_error("node configuration is missing"))?;
    if config.trust.enrollment != "manual" {
        return Err(OperationError::new(
            OperationErrorCode::EnrollmentDisabled,
            "manual enrollment is not enabled",
        ));
    }
    Ok(())
}

pub fn stage_manual_enrollment(
    context: &NodeContext,
    request: &ManualEnrollmentRequest,
    transport_certificate: &[u8],
) -> OperationResult<PublicPeer> {
    manual_enrollment_enabled(context)?;
    request
        .verify(enrollment::now_seconds())
        .map_err(map_enrollment_error)?;
    let registry = open_initialized_registry(context)?;
    registry
        .stage_manual_enrollment(
            request,
            transport_certificate,
            "authenticated-untrusted",
            "authenticated manual enrollment request",
            enrollment::now_seconds(),
        )
        .map_err(map_registry_error)
        .map(public_peer)
}

pub fn stage_manual_enrollment_hex(
    context: &NodeContext,
    request_hex: &str,
    transport_certificate_hex: &str,
) -> OperationResult<PublicPeer> {
    let request_bytes = decode_request(request_hex)?;
    let request = ManualEnrollmentRequest::decode(&request_bytes).map_err(map_enrollment_error)?;
    let certificate = decode_fixed_hex(
        transport_certificate_hex,
        crate::direct_transport::MAX_CERTIFICATE_BYTES,
        "transport certificate",
    )?;
    stage_manual_enrollment(context, &request, &certificate)
}

pub fn approve_manual_enrollment(
    context: &NodeContext,
    request: ManualEnrollmentApprovalRequest,
) -> OperationResult<PublicPeer> {
    require_confirmation(request.confirmed)?;
    manual_enrollment_enabled(context)?;
    let request_bytes = decode_request(&request.request_hex)?;
    let enrollment_request =
        ManualEnrollmentRequest::decode(&request_bytes).map_err(map_enrollment_error)?;
    if request
        .expected_node_id
        .as_deref()
        .is_some_and(|node_id| node_id != enrollment_request.proposer_node_id.as_str())
    {
        return Err(OperationError::new(
            OperationErrorCode::EnrollmentMismatch,
            "enrollment path node ID does not match request identity",
        ));
    }
    let certificate = decode_fixed_hex(
        &request.transport_certificate,
        crate::direct_transport::MAX_CERTIFICATE_BYTES,
        "transport certificate",
    )?;
    let code = decode_fixed_hex(&request.code, enrollment::CODE_BYTES, "approval code")?;
    let registry = open_initialized_registry(context)?;
    registry
        .approve_manual_enrollment(
            &enrollment_request,
            &certificate,
            &code,
            &request.actor,
            &request.reason,
            enrollment::now_seconds(),
        )
        .map_err(map_registry_error)
        .map(public_peer)
}

pub fn reject_manual_enrollment(
    context: &NodeContext,
    request: ManualEnrollmentRejectionRequest,
) -> OperationResult<PublicPeer> {
    require_confirmation(request.confirmed)?;
    manual_enrollment_enabled(context)?;
    let registry = open_initialized_registry(context)?;
    registry
        .reject_manual_enrollment(&request.node_id, &request.actor, &request.reason)
        .map_err(|error| {
            if matches!(error, RegistryError::InvalidTransition { .. }) {
                OperationError::new(OperationErrorCode::EnrollmentDenied, error.to_string())
            } else {
                map_registry_error(error)
            }
        })
        .map(public_peer)
}

pub fn request_manual_enrollment(
    context: &NodeContext,
    endpoint: std::net::SocketAddr,
    role: &str,
    capabilities: Vec<String>,
    lifetime_seconds: u64,
) -> OperationResult<ManualEnrollmentResult> {
    manual_enrollment_enabled(context)?;
    let role = parse_enrollment_role(role)?;
    let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
    let transport = LocalTransport::load_existing(context, &identity).map_err(|error| {
        OperationError::new(
            OperationErrorCode::RegistryInvalid,
            format!("transport state is invalid or insecure: {error}"),
        )
    })?;
    let offer = enrollment::ManualEnrollmentRequest::create(
        &identity,
        *transport.certificate().transport_public(),
        role,
        capabilities,
        enrollment::now_seconds(),
        lifetime_seconds,
    )
    .map_err(map_enrollment_error)?;
    let reciprocal = crate::direct_service::request_manual_enrollment(
        endpoint,
        context,
        &offer.request.encode(),
    )
    .map_err(map_direct_enrollment_error)?;
    if reciprocal.is_none() {
        return Err(OperationError::new(
            OperationErrorCode::EnrollmentDenied,
            "remote node did not stage the manual enrollment request",
        ));
    }
    let (reciprocal_request_hex, reciprocal_code) = reciprocal
        .map(|(request, code)| {
            (
                enrollment::hex_bytes(&request),
                enrollment::hex_bytes(&code),
            )
        })
        .unzip();
    Ok(ManualEnrollmentResult {
        pairing_id: offer.request.pairing_id_hex(),
        request_id: offer.request.request_id_hex(),
        request_hex: offer.request_hex(),
        code: offer.code_hex(),
        state: "pending".to_string(),
        reciprocal_request_hex,
        reciprocal_code,
    })
}

pub fn list_pending_enrollments(context: &NodeContext) -> OperationResult<Vec<PublicPeer>> {
    manual_enrollment_enabled(context)?;
    Ok(list_trusted_peers(context)?
        .into_iter()
        .filter(|peer| peer.state == "pending" && peer.source == "manual")
        .collect())
}

fn decode_request(value: &str) -> OperationResult<Vec<u8>> {
    if value.is_empty() || value.len() > enrollment::MAX_REQUEST_BYTES * 2 {
        return Err(OperationError::new(
            OperationErrorCode::EnrollmentInvalid,
            "manual enrollment request bytes are invalid",
        ));
    }
    if !value.len().is_multiple_of(2)
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(OperationError::new(
            OperationErrorCode::EnrollmentInvalid,
            "manual enrollment request bytes must be lowercase hexadecimal",
        ));
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16).map_err(|_| {
                OperationError::new(
                    OperationErrorCode::EnrollmentInvalid,
                    "manual enrollment request bytes are invalid",
                )
            })
        })
        .collect()
}

fn decode_fixed_hex(value: &str, bytes: usize, label: &str) -> OperationResult<Vec<u8>> {
    enrollment::parse_hex(value, bytes).map_err(|_| {
        OperationError::new(
            OperationErrorCode::EnrollmentInvalid,
            format!("{label} must be lowercase hexadecimal bytes"),
        )
    })
}

fn map_enrollment_error(error: EnrollmentError) -> OperationError {
    let code = match error {
        EnrollmentError::Expired => OperationErrorCode::EnrollmentExpired,
        EnrollmentError::Replay => OperationErrorCode::EnrollmentReplay,
        EnrollmentError::IdentityMismatch => OperationErrorCode::EnrollmentMismatch,
        EnrollmentError::Invalid | EnrollmentError::TooLarge => {
            OperationErrorCode::EnrollmentInvalid
        }
    };
    OperationError::new(code, error.to_string())
}

fn map_direct_enrollment_error(error: crate::direct_service::DirectServiceError) -> OperationError {
    let code = match error {
        crate::direct_service::DirectServiceError::Protocol(ref error) => match error.code() {
            crate::direct_transport::ProtocolErrorCode::UnsupportedVersion => {
                OperationErrorCode::TransportUnsupportedVersion
            }
            crate::direct_transport::ProtocolErrorCode::InvalidFrame => {
                OperationErrorCode::TransportInvalidFrame
            }
            crate::direct_transport::ProtocolErrorCode::MessageTooLarge => {
                OperationErrorCode::TransportMessageTooLarge
            }
            crate::direct_transport::ProtocolErrorCode::HandshakeFailed => {
                OperationErrorCode::TransportHandshakeFailed
            }
            crate::direct_transport::ProtocolErrorCode::IdentityMismatch => {
                OperationErrorCode::TransportIdentityMismatch
            }
            crate::direct_transport::ProtocolErrorCode::NotEnrolled => {
                OperationErrorCode::TransportNotEnrolled
            }
            crate::direct_transport::ProtocolErrorCode::Revoked => {
                OperationErrorCode::TransportRevoked
            }
            crate::direct_transport::ProtocolErrorCode::Expired => {
                OperationErrorCode::TransportExpired
            }
            crate::direct_transport::ProtocolErrorCode::Replay => {
                OperationErrorCode::TransportReplay
            }
            crate::direct_transport::ProtocolErrorCode::RateLimited => {
                OperationErrorCode::TransportRateLimited
            }
            crate::direct_transport::ProtocolErrorCode::Internal => {
                OperationErrorCode::TransportInternal
            }
        },
        _ => OperationErrorCode::TransportInternal,
    };
    OperationError::new(code, error.to_string())
}

fn open_initialized_registry(context: &NodeContext) -> OperationResult<NodeRegistry> {
    let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
    NodeRegistry::open_existing(context, identity.public_status()).map_err(map_registry_error)
}

fn read_public_config(context: &NodeContext) -> OperationResult<Option<PublicNodeConfig>> {
    Ok(read_node_config(context)?.map(public_config))
}

pub fn load_node_config(context: &NodeContext) -> OperationResult<NodeConfig> {
    read_node_config(context)?.ok_or_else(|| registry_error("node configuration is missing"))
}

fn read_node_config(context: &NodeContext) -> OperationResult<Option<NodeConfig>> {
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
    Ok(Some(config))
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
        direct_bind: config.network.direct_bind,
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

fn parse_enrollment_role(value: &str) -> OperationResult<EnrollmentRole> {
    match value {
        "conductor" => Ok(EnrollmentRole::Conductor),
        "performer" => Ok(EnrollmentRole::Performer),
        _ => Err(OperationError::new(
            OperationErrorCode::EnrollmentInvalid,
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
        NodeError::LifecycleBusy => OperationError::new(
            OperationErrorCode::Conflict,
            "node service is active; stop it before changing node state",
        ),
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
        RegistryError::Sqlite(_) => registry_error("node trust registry is invalid or corrupt"),
        RegistryError::Node(error) => map_node_error(error),
        RegistryError::AuditCapacity => {
            registry_error("node transport audit capacity is exhausted")
        }
        RegistryError::SelfTrust => {
            OperationError::new(OperationErrorCode::Conflict, "peer cannot trust itself")
        }
        RegistryError::EnrollmentReplay => OperationError::new(
            OperationErrorCode::EnrollmentReplay,
            "manual enrollment request was replayed",
        ),
        RegistryError::EnrollmentConflict => OperationError::new(
            OperationErrorCode::Conflict,
            "manual enrollment conflicts with existing trust state",
        ),
        RegistryError::EnrollmentCapacity => {
            registry_error("manual enrollment replay capacity is exhausted")
        }
        RegistryError::EnrollmentMismatch => OperationError::new(
            OperationErrorCode::EnrollmentMismatch,
            "manual enrollment evidence does not match staged state",
        ),
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
    use rusqlite::Connection;
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

    fn fail_enrollment_audits(context: &NodeContext, enabled: bool) {
        let connection = Connection::open(context.database_path()).unwrap();
        if enabled {
            connection
                .execute_batch(
                    "DROP TRIGGER enrollment_audits_no_update;
                     CREATE TRIGGER enrollment_audits_no_update
                     BEFORE INSERT ON enrollment_audits
                     BEGIN SELECT RAISE(ABORT, 'injected enrollment audit failure'); END;",
                )
                .unwrap();
        } else {
            connection
                .execute_batch(
                    "DROP TRIGGER enrollment_audits_no_update;
                     CREATE TRIGGER enrollment_audits_no_update
                     BEFORE UPDATE ON enrollment_audits
                     BEGIN SELECT RAISE(ABORT, 'enrollment audits are append-only'); END;",
                )
                .unwrap();
        }
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
            transport_certificate: None,
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
    fn initialization_does_not_recreate_deleted_transport_state() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        initialize_node(&context, &NodeConfig::default()).unwrap();
        fs::remove_file(context.transport_key_path()).unwrap();
        fs::remove_file(context.transport_certificate_path()).unwrap();

        assert!(initialize_node(&context, &NodeConfig::default()).is_err());
        assert!(!context.transport_key_path().exists());
        assert!(!context.transport_certificate_path().exists());
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
        let registry = context
            .open_trust_registry(
                NodeIdentity::load_existing(&context)
                    .unwrap()
                    .public_status(),
            )
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
    fn manual_enrollment_stages_requires_code_and_promotes_atomically() {
        let target_temp = TempDir::new().unwrap();
        let candidate_temp = TempDir::new().unwrap();
        let target = context(&target_temp);
        let candidate = context(&candidate_temp);
        let mut target_config = NodeConfig::default();
        target_config.trust.enrollment = "manual".into();
        initialize_node(&target, &target_config).unwrap();
        initialize_node(&candidate, &NodeConfig::default()).unwrap();

        let candidate_identity = NodeIdentity::load_existing(&candidate).unwrap();
        let candidate_transport =
            crate::node_transport::LocalTransport::load_existing(&candidate, &candidate_identity)
                .unwrap();
        let offer = ManualEnrollmentRequest::create(
            &candidate_identity,
            *candidate_transport.certificate().transport_public(),
            EnrollmentRole::Performer,
            vec!["remote-run".into()],
            enrollment::now_seconds(),
            300,
        )
        .unwrap();
        let certificate_hex = candidate_transport
            .certificate()
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();

        fail_enrollment_audits(&target, true);
        let stage_error = stage_manual_enrollment(
            &target,
            &offer.request,
            candidate_transport.certificate().as_bytes(),
        )
        .unwrap_err();
        assert_eq!(stage_error.code, OperationErrorCode::RegistryInvalid);
        assert!(list_pending_enrollments(&target).unwrap().is_empty());
        fail_enrollment_audits(&target, false);

        let pending = stage_manual_enrollment(
            &target,
            &offer.request,
            candidate_transport.certificate().as_bytes(),
        )
        .unwrap();
        assert_eq!(pending.state, "pending");
        assert_eq!(list_pending_enrollments(&target).unwrap().len(), 1);

        fail_enrollment_audits(&target, true);
        let denied = approve_manual_enrollment(
            &target,
            ManualEnrollmentApprovalRequest {
                request_hex: offer.request_hex(),
                transport_certificate: certificate_hex.clone(),
                code: [0u8; enrollment::CODE_BYTES]
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
                actor: "operator".into(),
                reason: "wrong code".into(),
                confirmed: true,
                expected_node_id: None,
            },
        )
        .unwrap_err();
        assert_eq!(denied.code, OperationErrorCode::RegistryInvalid);
        assert_eq!(list_pending_enrollments(&target).unwrap().len(), 1);
        fail_enrollment_audits(&target, false);

        let approved = approve_manual_enrollment(
            &target,
            ManualEnrollmentApprovalRequest {
                request_hex: offer.request_hex(),
                transport_certificate: certificate_hex,
                code: offer
                    .code
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
                actor: "operator".into(),
                reason: "approved manually".into(),
                confirmed: true,
                expected_node_id: Some(offer.request.proposer_node_id.clone()),
            },
        )
        .unwrap();
        assert_eq!(approved.state, "active");
        assert!(list_pending_enrollments(&target).unwrap().is_empty());
        assert_eq!(list_trusted_peers(&target).unwrap()[0].state, "active");
    }

    #[test]
    fn manual_enrollment_rejection_audit_failure_is_atomic() {
        let target_temp = TempDir::new().unwrap();
        let candidate_temp = TempDir::new().unwrap();
        let target = context(&target_temp);
        let candidate = context(&candidate_temp);
        let mut target_config = NodeConfig::default();
        target_config.trust.enrollment = "manual".into();
        initialize_node(&target, &target_config).unwrap();
        initialize_node(&candidate, &NodeConfig::default()).unwrap();

        let candidate_identity = NodeIdentity::load_existing(&candidate).unwrap();
        let candidate_transport =
            crate::node_transport::LocalTransport::load_existing(&candidate, &candidate_identity)
                .unwrap();
        let offer = ManualEnrollmentRequest::create(
            &candidate_identity,
            *candidate_transport.certificate().transport_public(),
            EnrollmentRole::Performer,
            vec!["remote-run".into()],
            enrollment::now_seconds(),
            300,
        )
        .unwrap();
        let pending = stage_manual_enrollment(
            &target,
            &offer.request,
            candidate_transport.certificate().as_bytes(),
        )
        .unwrap();
        assert_eq!(pending.state, "pending");

        fail_enrollment_audits(&target, true);
        let error = reject_manual_enrollment(
            &target,
            ManualEnrollmentRejectionRequest {
                node_id: offer.request.proposer_node_id.clone(),
                actor: "operator".into(),
                reason: "reject test".into(),
                confirmed: true,
            },
        )
        .unwrap_err();
        assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
        assert_eq!(list_pending_enrollments(&target).unwrap().len(), 1);
        fail_enrollment_audits(&target, false);
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

    #[test]
    fn concurrent_service_initialization_converges_without_duplicate_state() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let temp = TempDir::new().unwrap();
        let context = Arc::new(context(&temp));
        let barrier = Arc::new(Barrier::new(8));
        let handles = (0..8)
            .map(|_| {
                let context = Arc::clone(&context);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    initialize_node(&context, &NodeConfig::default()).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        let identity = results[0].status.identity.clone().unwrap();
        assert!(results.iter().all(|result| {
            result.status.identity.as_ref() == Some(&identity)
                && result.status.trust.peer_count == 0
        }));
        assert!(context.identity_path().is_file());
        assert!(context.database_path().is_file());
    }

    #[test]
    fn factory_reset_requires_confirmation_and_removes_identity_and_trust_only() {
        let temp = TempDir::new().unwrap();
        let context = context(&temp);
        initialize_node(&context, &NodeConfig::default()).unwrap();
        let identity_before = NodeIdentity::load_existing(&context)
            .unwrap()
            .public_status()
            .clone();

        let denied = reset_node(&context, false).unwrap_err();
        assert_eq!(denied.code, OperationErrorCode::Forbidden);
        assert!(context.identity_path().exists());

        let result = reset_node(&context, true).unwrap();
        assert!(result.state_removed);
        assert!(result.identity_removed);
        assert!(result.trust_removed);
        assert!(context.state_dir().is_dir());
        assert!(!context.identity_path().exists());
        assert!(!context.database_path().exists());
        assert!(context.config_path().is_file());

        initialize_node(&context, &NodeConfig::default()).unwrap();
        let identity_after = NodeIdentity::load_existing(&context)
            .unwrap()
            .public_status()
            .clone();
        assert_ne!(identity_before, identity_after);
        assert!(list_trusted_peers(&context).unwrap().is_empty());
    }

    #[test]
    fn reset_and_initialize_race_preserves_identity_registry_pairing() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let temp = TempDir::new().unwrap();
        let context = Arc::new(context(&temp));
        initialize_node(&context, &NodeConfig::default()).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let reset_context = Arc::clone(&context);
        let reset_barrier = Arc::clone(&barrier);
        let reset = thread::spawn(move || {
            reset_barrier.wait();
            reset_node(&reset_context, true)
        });
        let init_context = Arc::clone(&context);
        let init_barrier = Arc::clone(&barrier);
        let init = thread::spawn(move || {
            init_barrier.wait();
            initialize_node(&init_context, &NodeConfig::default())
        });

        let reset_result = reset.join().unwrap();
        let init_result = init.join().unwrap();
        assert!(
            reset_result.is_ok()
                || reset_result.as_ref().unwrap_err().code == OperationErrorCode::Conflict
        );
        assert!(init_result.is_ok());

        let identity_exists = context.identity_path().is_file();
        let registry_exists = context.database_path().is_file();
        assert_eq!(identity_exists, registry_exists);
        if identity_exists {
            let identity = NodeIdentity::load_existing(&context).unwrap();
            NodeRegistry::open_existing(&context, identity.public_status()).unwrap();
        }
    }
}

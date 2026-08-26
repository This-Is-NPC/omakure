use crate::domain::{NodeConfig, NodeConfigError};
use crate::enrollment::{self, EnrollmentError, EnrollmentRole, ManualEnrollmentRequest};
use crate::node::{
    NodeContext, NodeError, NodePathOverrides, PrivateFileCommitStatus, PrivateTokenLease,
};
use crate::node_identity::{NodeIdentity, NodeIdentityError};
use crate::node_registry::{
    NodeRegistry, PeerRecord, PeerRegistration, PeerRole, PeerSource, PeerState, RegistryError,
};
use crate::node_transport::LocalTransport;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::Duration;
use subtle::ConstantTimeEq;

use super::{OperationError, OperationErrorCode, OperationResult};

const PUBLIC_PEER_LIMIT: usize = 256;
const MAX_NODE_CONFIG_BYTES: usize = 64 * 1024;
const SIGNED_BUNDLE_ACTOR: &str = "signed-bundle-installer";
const SIGNED_BUNDLE_REASON: &str = "unattended signed enrollment bundle";

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
    #[serde(default, skip_serializing_if = "is_false")]
    pub cleanup_pending: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SignedBundleApplyRequest {
    pub bundle_hex: String,
    pub bootstrap_token: String,
    pub bootstrap_nonce: String,
    pub bootstrap_token_path: Option<PathBuf>,
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
            discovery: Some(configured_discovery),
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

pub fn signed_bundle_enrollment_enabled(context: &NodeContext) -> OperationResult<NodeConfig> {
    let config = load_node_config(context)?;
    if config.trust.enrollment != "signed-bundle" {
        return Err(OperationError::new(
            OperationErrorCode::EnrollmentDisabled,
            "signed-bundle enrollment is not enabled",
        ));
    }
    Ok(config)
}

pub fn apply_signed_bundle(
    context: &NodeContext,
    request: SignedBundleApplyRequest,
) -> OperationResult<PublicPeer> {
    apply_signed_bundle_with_actor(context, request, SIGNED_BUNDLE_ACTOR)
}

pub fn apply_signed_bundle_authenticated(
    context: &NodeContext,
    request: SignedBundleApplyRequest,
    token_id: &str,
) -> OperationResult<PublicPeer> {
    let actor = format!("auth-token:{token_id}");
    apply_signed_bundle_with_actor(context, request, &actor)
}

fn apply_signed_bundle_with_actor(
    context: &NodeContext,
    mut request: SignedBundleApplyRequest,
    actor: &str,
) -> OperationResult<PublicPeer> {
    let config = signed_bundle_enrollment_enabled(context)?;
    let nonce = decode_fixed_hex(&request.bootstrap_nonce, 16, "bootstrap nonce")?;
    let state_ready = context
        .validate_existing_state_contents()
        .map_err(map_node_error)?;
    let identity_ready = path_is_present(&context.identity_path(), "identity.key")?;
    let registry_ready = path_is_present(&context.database_path(), "node.sqlite")?;
    if !state_ready || !identity_ready || !registry_ready {
        let _ = initialize_node_nonblocking(context, &config)?;
    }
    let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())
        .map_err(map_registry_error)?;
    let mut token_lease = if let Some(path) = request.bootstrap_token_path.as_deref() {
        recover_private_token_tombstones(context, &registry, &config.organization.id, path)?;
        let lease = context
            .stage_private_bounded_file(path, enrollment::MAX_BOOTSTRAP_TOKEN_BYTES)
            .map_err(map_node_error)?;
        let token = match String::from_utf8(lease.contents().to_vec()) {
            Ok(token) => token,
            Err(_) => {
                let mut lease = Some(lease);
                return Err(restore_token_lease(
                    &mut lease,
                    OperationError::new(
                        OperationErrorCode::EnrollmentDenied,
                        "bootstrap token is invalid",
                    ),
                ));
            }
        };
        request.bootstrap_token = token.trim().to_string();
        Some(lease)
    } else {
        None
    };
    if request.bootstrap_token.len() < 32
        || request.bootstrap_token.len() > enrollment::MAX_BOOTSTRAP_TOKEN_BYTES
        || request
            .bootstrap_token
            .bytes()
            .any(|byte| byte.is_ascii_control())
    {
        return Err(restore_token_lease(
            &mut token_lease,
            OperationError::new(
                OperationErrorCode::EnrollmentDenied,
                "bootstrap token is invalid",
            ),
        ));
    }
    let bundle_bytes = match decode_bundle(&request.bundle_hex) {
        Ok(bytes) => bytes,
        Err(error) => {
            return record_signed_bundle_failure_with_token(
                &registry,
                &mut token_lease,
                None,
                error,
            )
        }
    };
    let bundle = match enrollment::SignedEnrollmentBundle::decode(&bundle_bytes) {
        Ok(bundle) => bundle,
        Err(error) => {
            return record_signed_bundle_failure_with_token(
                &registry,
                &mut token_lease,
                None,
                map_enrollment_error(error),
            )
        }
    };
    let preflight = (|| {
        let authority = config
            .trust
            .authorities
            .iter()
            .find(|authority| authority.key_id == hash_hex(&bundle.authority_key_id))
            .ok_or_else(|| {
                OperationError::new(
                    OperationErrorCode::EnrollmentInvalid,
                    "signed enrollment authority is not configured",
                )
            })?;
        let authority = enrollment::BundleAuthority {
            key_id: enrollment::parse_hex(&authority.key_id, 16)
                .map_err(|_| {
                    OperationError::new(
                        OperationErrorCode::EnrollmentInvalid,
                        "authority key ID is invalid",
                    )
                })?
                .try_into()
                .map_err(|_| {
                    OperationError::new(
                        OperationErrorCode::EnrollmentInvalid,
                        "authority key ID is invalid",
                    )
                })?,
            public_key: enrollment::parse_hex(&authority.public_key, 32)
                .map_err(|_| {
                    OperationError::new(
                        OperationErrorCode::EnrollmentInvalid,
                        "authority public key is invalid",
                    )
                })?
                .try_into()
                .map_err(|_| {
                    OperationError::new(
                        OperationErrorCode::EnrollmentInvalid,
                        "authority public key is invalid",
                    )
                })?,
            revoked: authority.revoked,
        };
        let now = enrollment::now_seconds();
        bundle
            .verify(
                &authority,
                &config.organization.id,
                identity.public_status().node_id.as_str(),
                now,
            )
            .map_err(map_enrollment_error)?;
        let certificate =
            crate::direct_transport::TransportCertificate::from_bytes(&bundle.subject_certificate)
                .map_err(|_| {
                    OperationError::new(
                        OperationErrorCode::EnrollmentInvalid,
                        "signed enrollment certificate is invalid",
                    )
                })?;
        certificate.verify_time(now).map_err(|_| {
            OperationError::new(
                OperationErrorCode::EnrollmentExpired,
                "signed enrollment certificate is expired",
            )
        })?;
        let token_hash = enrollment::hash_bootstrap_token(request.bootstrap_token.as_bytes());
        let nonce_hash = enrollment::hash_bootstrap_nonce(&nonce);
        if hash_hex(&token_hash)
            .as_bytes()
            .ct_eq(config.trust.bootstrap_token_hash.as_bytes())
            .unwrap_u8()
            != 1
            || hash_hex(&nonce_hash)
                .as_bytes()
                .ct_eq(config.trust.bootstrap_nonce_hash.as_bytes())
                .unwrap_u8()
                != 1
        {
            return Err(OperationError::new(
                OperationErrorCode::EnrollmentDenied,
                "bootstrap proof does not match local policy",
            ));
        }
        Ok((now, token_hash, nonce_hash))
    })();
    let (now, token_hash, nonce_hash) = match preflight {
        Ok(value) => value,
        Err(error) => {
            return record_signed_bundle_failure_with_token(
                &registry,
                &mut token_lease,
                Some(&bundle),
                error,
            )
        }
    };
    let mut peer = match registry
        .activate_signed_bundle(
            &bundle,
            actor,
            SIGNED_BUNDLE_REASON,
            now,
            &token_hash,
            &nonce_hash,
        )
        .map_err(map_registry_error)
    {
        Ok(peer) => public_peer(peer),
        Err(error) => {
            return record_signed_bundle_failure_with_token(
                &registry,
                &mut token_lease,
                Some(&bundle),
                error,
            )
        }
    };
    if let Some(lease) = token_lease.take() {
        let bundle_digest: [u8; 32] = Sha256::digest(bundle.encode()).into();
        peer.cleanup_pending = !complete_token_cleanup(
            &registry,
            lease,
            &config.organization.id,
            &token_hash,
            &nonce_hash,
            &bundle.bundle_id,
            &bundle_digest,
        );
    }
    Ok(peer)
}

pub fn apply_signed_bundle_from_local_token(
    context: &NodeContext,
    mut request: SignedBundleApplyRequest,
    token_id: &str,
) -> OperationResult<PublicPeer> {
    let token_path = std::env::var_os("OMAKURE_BOOTSTRAP_TOKEN_FILE")
        .map(PathBuf::from)
        .ok_or_else(|| {
            OperationError::new(
                OperationErrorCode::EnrollmentDenied,
                "local bootstrap token file is not configured",
            )
        })?;
    request.bootstrap_token.clear();
    request.bootstrap_token_path = Some(token_path);
    apply_signed_bundle_authenticated(context, request, token_id)
}

fn restore_token_lease(
    lease: &mut Option<PrivateTokenLease>,
    error: OperationError,
) -> OperationError {
    if let Some(lease) = lease.take() {
        if lease.restore().is_err() {
            return OperationError::new(
                OperationErrorCode::EnrollmentDenied,
                "bootstrap token could not be restored",
            );
        }
    }
    error
}

fn record_signed_bundle_failure_with_token(
    registry: &NodeRegistry,
    lease: &mut Option<PrivateTokenLease>,
    bundle: Option<&enrollment::SignedEnrollmentBundle>,
    error: OperationError,
) -> OperationResult<PublicPeer> {
    record_signed_bundle_failure(registry, bundle, restore_token_lease(lease, error))
}

fn recover_private_token_tombstones(
    context: &NodeContext,
    registry: &NodeRegistry,
    organization: &str,
    path: &std::path::Path,
) -> OperationResult<()> {
    let _lock = context
        .acquire_private_token_lock(path)
        .map_err(map_node_error)?;
    let pending = registry
        .pending_bootstrap_cleanups(
            organization,
            crate::node::PRIVATE_TOKEN_TOMBSTONE_RETRY_LIMIT,
        )
        .map_err(map_registry_error)?;
    let mut completed = vec![false; pending.len()];
    for lease in context
        .list_private_token_tombstones(path, enrollment::MAX_BOOTSTRAP_TOKEN_BYTES)
        .map_err(map_node_error)?
    {
        let token_hash = enrollment::hash_bootstrap_token(lease.contents());
        if let Some((index, cleanup)) = pending
            .iter()
            .enumerate()
            .find(|(_, cleanup)| cleanup.token_hash == token_hash)
        {
            if lease.finish_success() == PrivateFileCommitStatus::CleanupRequired {
                return Err(cleanup_recovery_error());
            }
            registry
                .complete_bootstrap_cleanup(cleanup, None)
                .map_err(|_| cleanup_recovery_error())?;
            completed[index] = true;
        } else if registry
            .bootstrap_proof_consumed(organization)
            .map_err(|_| cleanup_recovery_error())?
        {
            if lease.finish_success() == PrivateFileCommitStatus::CleanupRequired {
                return Err(cleanup_recovery_error());
            }
        } else {
            lease.restore().map_err(|_| cleanup_recovery_error())?;
        }
    }
    for (index, cleanup) in pending.iter().enumerate() {
        if completed[index] {
            continue;
        }
        match fs::symlink_metadata(path) {
            Ok(_) => return Err(cleanup_recovery_error()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(cleanup_recovery_error()),
        }
        registry
            .complete_bootstrap_cleanup(cleanup, None)
            .map_err(|_| cleanup_recovery_error())?;
    }
    Ok(())
}

pub fn recover_local_bootstrap_token_tombstones(context: &NodeContext) -> OperationResult<()> {
    let Some(path) = std::env::var_os("OMAKURE_BOOTSTRAP_TOKEN_FILE").map(PathBuf::from) else {
        return Ok(());
    };
    let result = (|| {
        let config = signed_bundle_enrollment_enabled(context)?;
        let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
        let registry = NodeRegistry::open_existing(context, identity.public_status())
            .map_err(map_registry_error)?;
        recover_private_token_tombstones(context, &registry, &config.organization.id, &path)
    })();
    result.map_err(|_| cleanup_recovery_error())
}

fn complete_token_cleanup(
    registry: &NodeRegistry,
    lease: PrivateTokenLease,
    organization: &str,
    token_hash: &[u8; 32],
    nonce_hash: &[u8; 32],
    bundle_id: &[u8; 16],
    bundle_digest: &[u8; 32],
) -> bool {
    if lease.finish_success() == PrivateFileCommitStatus::CleanupRequired {
        return false;
    }
    let cleanup = crate::node_registry::PendingBootstrapCleanup {
        organization: organization.to_string(),
        token_hash: *token_hash,
        nonce_hash: *nonce_hash,
        bundle_id: *bundle_id,
    };
    match registry.complete_bootstrap_cleanup(&cleanup, Some(bundle_digest)) {
        Ok(()) => true,
        Err(_) => false,
    }
}

fn cleanup_recovery_error() -> OperationError {
    OperationError::new(
        OperationErrorCode::IoFailed,
        "bootstrap token cleanup recovery failed; node service startup was aborted; repair the node state and retry",
    )
}

fn record_signed_bundle_failure(
    registry: &NodeRegistry,
    bundle: Option<&enrollment::SignedEnrollmentBundle>,
    error: OperationError,
) -> OperationResult<PublicPeer> {
    let (request_id, node_id, request_digest) = match bundle {
        Some(bundle) => (
            Some(&bundle.bundle_id),
            bundle.subject_node_id.as_str(),
            Some(Sha256::digest(bundle.encode()).into()),
        ),
        None => (None, registry.local_node_id(), None),
    };
    let event_code = match error.code {
        OperationErrorCode::EnrollmentDenied => "proof_rejected",
        OperationErrorCode::EnrollmentExpired => "expired",
        _ => "malformed",
    };
    registry
        .record_enrollment_audit(
            event_code,
            request_id,
            request_digest.as_ref(),
            node_id,
            "rejected",
            "signed enrollment bundle verification failed",
        )
        .map_err(map_registry_error)?;
    Err(error)
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

fn decode_bundle(value: &str) -> OperationResult<Vec<u8>> {
    if value.is_empty() || value.len() > enrollment::MAX_BUNDLE_BYTES * 2 {
        return Err(OperationError::new(
            OperationErrorCode::EnrollmentInvalid,
            "signed enrollment bundle bytes are invalid",
        ));
    }
    decode_fixed_hex(value, value.len() / 2, "signed enrollment bundle")
}

fn hash_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
        EnrollmentError::AuthorityUnknown => OperationErrorCode::EnrollmentInvalid,
        EnrollmentError::AuthorityRevoked => OperationErrorCode::EnrollmentDenied,
        EnrollmentError::OrganizationMismatch | EnrollmentError::AudienceMismatch => {
            OperationErrorCode::EnrollmentMismatch
        }
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
        discovery_enabled: config.discovery.enabled,
        discovery_port: config.discovery.port,
        discovery_broadcast: config.discovery.broadcast,
    }
}

pub fn public_discovery_status(
    handle: Option<&crate::discovery::DiscoveryStatusHandle>,
    include_addresses: bool,
) -> OperationResult<crate::discovery::DiscoveryStatus> {
    public_discovery_status_with_config(handle, include_addresses, None)
}

pub fn public_discovery_status_with_config(
    handle: Option<&crate::discovery::DiscoveryStatusHandle>,
    include_addresses: bool,
    config: Option<&PublicNodeConfig>,
) -> OperationResult<crate::discovery::DiscoveryStatus> {
    let Some(handle) = handle else {
        return Ok(configured_discovery_status(config));
    };
    handle
        .lock()
        .map_err(|_| {
            OperationError::new(OperationErrorCode::IoFailed, "discovery status unavailable")
        })
        .map(|mut snapshot| {
            snapshot.public_status(include_addresses, crate::enrollment::now_seconds())
        })
}

fn configured_discovery_status(
    config: Option<&PublicNodeConfig>,
) -> crate::discovery::DiscoveryStatus {
    let settings = crate::domain::DiscoverySettings {
        enabled: config.is_some_and(|config| config.discovery_enabled),
        port: config
            .map(|config| config.discovery_port)
            .unwrap_or(crate::discovery::DISCOVERY_PORT),
        multicast_addr: crate::discovery::MULTICAST_GROUP.to_string(),
        broadcast: config.is_some_and(|config| config.discovery_broadcast),
    };
    crate::discovery::DiscoveryService::status_without_service(
        &settings,
        crate::discovery::platform_supported(),
        config.is_some_and(|config| config.discovery_secret_configured),
    )
}

pub fn scan_discovery(
    context: &NodeContext,
    scripts_dir: &std::path::Path,
    wait_seconds: u64,
    include_addresses: bool,
) -> OperationResult<crate::discovery::DiscoveryStatus> {
    let config = load_node_config(context)?;
    let direct_bind = config
        .network
        .direct_bind
        .as_deref()
        .map(str::parse::<std::net::SocketAddr>)
        .transpose()
        .map_err(|_| {
            OperationError::new(OperationErrorCode::InvalidInput, "direct bind is invalid")
        })?;
    if !config.discovery.enabled {
        let public_config = public_config(config.clone());
        return public_discovery_status_with_config(None, include_addresses, Some(&public_config));
    }
    let secret = if config.organization.discovery_secret_ref.is_empty() {
        None
    } else {
        let workspace = crate::workspace::Workspace::new(scripts_dir.to_path_buf());
        workspace.ensure_layout().map_err(|_| {
            OperationError::new(
                OperationErrorCode::DiscoveryInternal,
                "discovery workspace is unavailable",
            )
        })?;
        Some(
            crate::secrets::resolve_secret_value(
                &workspace,
                &config.organization.discovery_secret_ref,
                &crate::secrets::SecretAccess::allow_all(),
            )
            .map_err(|_| {
                OperationError::new(
                    OperationErrorCode::DiscoverySecretMismatch,
                    "discovery secret could not be resolved",
                )
            })?,
        )
    };
    let mut service = crate::discovery::DiscoveryService::start(
        config.discovery,
        context.clone(),
        direct_bind.map(|bind| bind.port()),
        secret,
    )
    .map_err(|error| match error {
        crate::discovery::DiscoveryError::UnsupportedPlatform => OperationError::new(
            OperationErrorCode::DiscoveryUnsupportedPlatform,
            "discovery is unsupported on this platform",
        ),
        crate::discovery::DiscoveryError::SecretInvalid => OperationError::new(
            OperationErrorCode::DiscoverySecretMismatch,
            "discovery secret is invalid",
        ),
        _ => OperationError::new(
            OperationErrorCode::DiscoveryInternal,
            "discovery could not start",
        ),
    })?;
    std::thread::sleep(Duration::from_secs(wait_seconds.clamp(1, 30)));
    let result = public_discovery_status(Some(&service.status()), include_addresses);
    service.stop();
    result
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
        cleanup_pending: false,
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

pub(crate) fn map_node_error(error: NodeError) -> OperationError {
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

pub(crate) fn map_identity_error(error: NodeIdentityError) -> OperationError {
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

pub(crate) fn map_registry_error(error: RegistryError) -> OperationError {
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
        RegistryError::BundleReplay => OperationError::new(
            OperationErrorCode::EnrollmentReplay,
            "signed enrollment bundle was replayed",
        ),
        RegistryError::BundleConflict => OperationError::new(
            OperationErrorCode::Conflict,
            "signed enrollment bundle conflicts with existing trust state",
        ),
        RegistryError::BootstrapProofConsumed => OperationError::new(
            OperationErrorCode::EnrollmentReplay,
            "signed enrollment bootstrap proof was already consumed",
        ),
        RegistryError::ConductorConflict => OperationError::new(
            OperationErrorCode::Conflict,
            "an active conductor already exists",
        ),
        RegistryError::BundleCapacity => {
            registry_error("signed enrollment replay capacity is exhausted")
        }
        RegistryError::BundleRateLimited => OperationError::new(
            OperationErrorCode::EnrollmentRateLimited,
            "signed enrollment bundle rate limit exceeded",
        ),
    }
}

fn map_io_error(error: io::Error) -> OperationError {
    OperationError::new(OperationErrorCode::IoFailed, error.to_string())
}

pub(crate) fn registry_error(message: impl Into<String>) -> OperationError {
    OperationError::new(OperationErrorCode::RegistryInvalid, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{
        set_private_token_fault, NodePathOverrides, NodePlatform, PrivateTokenFault,
    };
    use rusqlite::Connection;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    static TOKEN_FAULT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

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

    fn fail_cleanup_completion_audit(context: &NodeContext, enabled: bool) {
        let connection = Connection::open(context.database_path()).unwrap();
        if enabled {
            connection
                .execute_batch(
                    "DROP TRIGGER enrollment_audits_no_update;
                     CREATE TRIGGER enrollment_audits_no_update
                     BEFORE INSERT ON enrollment_audits
                     WHEN NEW.event_code = 'cleanup_completed'
                     BEGIN SELECT RAISE(ABORT, 'injected cleanup completion audit failure'); END;",
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

    fn write_secure_token(path: &std::path::Path, token: &str) {
        fs::write(path, token.as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    struct SignedBundleFixture {
        _target_temp: TempDir,
        _manager_temp: TempDir,
        target: NodeContext,
        request: SignedBundleApplyRequest,
        token_path: std::path::PathBuf,
        organization: String,
    }

    fn signed_bundle_fixture(
        authority_private: [u8; 32],
        nonce_byte: u8,
        bundle_byte: u8,
    ) -> SignedBundleFixture {
        let target_temp = TempDir::new().unwrap();
        let target = context(&target_temp);
        let manager_temp = TempDir::new().unwrap();
        let manager_context = context(&manager_temp);
        let authority_signing_key =
            k256::schnorr::SigningKey::from_slice(&authority_private).unwrap();
        let token = "t".repeat(32);
        let nonce = [nonce_byte; 16];
        let mut config = NodeConfig::default();
        config.organization.id = "omakure".into();
        config.trust.enrollment = "signed-bundle".into();
        config.trust.bootstrap_token_hash =
            hash_hex(&enrollment::hash_bootstrap_token(token.as_bytes()));
        config.trust.bootstrap_nonce_hash = hash_hex(&enrollment::hash_bootstrap_nonce(&nonce));
        config.trust.authorities = vec![crate::domain::EnrollmentAuthority {
            key_id: hash_hex(&[8; 16]),
            public_key: hash_hex(&authority_signing_key.verifying_key().to_bytes()),
            revoked: false,
        }];
        initialize_node(&target, &config).unwrap();
        let manager = NodeIdentity::load_or_initialize(&manager_context).unwrap();
        let manager_transport = LocalTransport::provision_new(&manager_context, &manager).unwrap();
        let target_identity = NodeIdentity::load_existing(&target).unwrap();
        let now = enrollment::now_seconds();
        let bundle = enrollment::SignedEnrollmentBundle::sign_with_material(
            &authority_private,
            [bundle_byte; enrollment::REQUEST_ID_BYTES],
            [8; enrollment::BUNDLE_AUTHORITY_ID_BYTES],
            "omakure".into(),
            target_identity.public_status().node_id.clone(),
            manager.public_status().node_id.clone(),
            enrollment::parse_hex(&manager.public_status().public_key_hex, 32)
                .unwrap()
                .try_into()
                .unwrap(),
            *manager_transport.certificate().transport_public(),
            *manager_transport.certificate().as_bytes(),
            EnrollmentRole::Conductor,
            vec!["remote-run".into()],
            now,
            now + 600,
        )
        .unwrap();
        let token_path = target_temp.path().join("bootstrap.token");
        write_secure_token(&token_path, &token);
        SignedBundleFixture {
            target,
            request: SignedBundleApplyRequest {
                bundle_hex: hash_hex(&bundle.encode()),
                bootstrap_token: token,
                bootstrap_nonce: hash_hex(&nonce),
                bootstrap_token_path: Some(token_path.clone()),
            },
            token_path,
            organization: "omakure".into(),
            _target_temp: target_temp,
            _manager_temp: manager_temp,
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

    #[test]
    fn signed_bundle_apply_is_target_bound_atomic_and_single_use() {
        let _fault_lock = TOKEN_FAULT_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap();
        set_private_token_fault(PrivateTokenFault::None);
        let target_temp = TempDir::new().unwrap();
        let target = context(&target_temp);
        let authority_private = [2_u8; 32];
        let authority_signing_key =
            k256::schnorr::SigningKey::from_slice(&authority_private).unwrap();
        let token = "t".repeat(32);
        let nonce = [9_u8; 16];
        let mut config = NodeConfig::default();
        config.organization.id = "omakure".into();
        config.trust.enrollment = "signed-bundle".into();
        config.trust.bootstrap_token_hash =
            hash_hex(&enrollment::hash_bootstrap_token(token.as_bytes()));
        config.trust.bootstrap_nonce_hash = hash_hex(&enrollment::hash_bootstrap_nonce(&nonce));
        config.trust.authorities = vec![crate::domain::EnrollmentAuthority {
            key_id: hash_hex(&[8; 16]),
            public_key: hash_hex(&authority_signing_key.verifying_key().to_bytes()),
            revoked: false,
        }];

        let manager_temp = TempDir::new().unwrap();
        let manager_context = context(&manager_temp);
        let manager = NodeIdentity::load_or_initialize(&manager_context).unwrap();
        let manager_transport = LocalTransport::provision_new(&manager_context, &manager).unwrap();
        initialize_node(&target, &config).unwrap();
        let target_identity = NodeIdentity::load_existing(&target).unwrap();
        let now = enrollment::now_seconds();
        let bundle = enrollment::SignedEnrollmentBundle::sign_with_material(
            &authority_private,
            [7; enrollment::REQUEST_ID_BYTES],
            [8; enrollment::BUNDLE_AUTHORITY_ID_BYTES],
            "omakure".into(),
            target_identity.public_status().node_id.clone(),
            manager.public_status().node_id.clone(),
            enrollment::parse_hex(&manager.public_status().public_key_hex, 32)
                .unwrap()
                .try_into()
                .unwrap(),
            *manager_transport.certificate().transport_public(),
            *manager_transport.certificate().as_bytes(),
            EnrollmentRole::Conductor,
            vec!["remote-run".into()],
            now,
            now + 600,
        )
        .unwrap();
        let request = SignedBundleApplyRequest {
            bundle_hex: hash_hex(&bundle.encode()),
            bootstrap_token: token.clone(),
            bootstrap_nonce: hash_hex(&nonce),
            bootstrap_token_path: Some(target_temp.path().join("bootstrap.token")),
        };
        fs::write(
            request.bootstrap_token_path.as_ref().unwrap(),
            token.as_bytes(),
        )
        .unwrap();
        let mut permissions = fs::metadata(request.bootstrap_token_path.as_ref().unwrap())
            .unwrap()
            .permissions();
        #[cfg(unix)]
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o600);
        fs::set_permissions(request.bootstrap_token_path.as_ref().unwrap(), permissions).unwrap();
        let peer = apply_signed_bundle(&target, request.clone()).unwrap();
        assert_eq!(peer.state, "active");
        assert!(!request.bootstrap_token_path.as_ref().unwrap().exists());
        assert_eq!(list_trusted_peers(&target).unwrap().len(), 1);
        let replay = apply_signed_bundle(
            &target,
            SignedBundleApplyRequest {
                bootstrap_token: token,
                bootstrap_token_path: None,
                ..request
            },
        )
        .unwrap_err();
        assert_eq!(replay.code, OperationErrorCode::EnrollmentReplay);
        assert_eq!(list_trusted_peers(&target).unwrap().len(), 1);
    }

    #[test]
    fn signed_bundle_token_consumption_faults_are_recoverable_across_restart() {
        let _fault_lock = TOKEN_FAULT_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap();

        let fixture = signed_bundle_fixture([21; 32], 31, 41);
        set_private_token_fault(PrivateTokenFault::Rename);
        assert!(apply_signed_bundle(&fixture.target, fixture.request.clone()).is_err());
        set_private_token_fault(PrivateTokenFault::None);
        assert!(fixture.token_path.exists());
        assert!(list_trusted_peers(&fixture.target).unwrap().is_empty());

        let fixture = signed_bundle_fixture([22; 32], 32, 42);
        fail_enrollment_audits(&fixture.target, true);
        assert!(apply_signed_bundle(&fixture.target, fixture.request.clone()).is_err());
        fail_enrollment_audits(&fixture.target, false);
        assert!(fixture.token_path.exists());
        assert!(list_trusted_peers(&fixture.target).unwrap().is_empty());

        let fixture = signed_bundle_fixture([23; 32], 33, 43);
        fail_enrollment_audits(&fixture.target, true);
        set_private_token_fault(PrivateTokenFault::Restore);
        assert!(apply_signed_bundle(&fixture.target, fixture.request.clone()).is_err());
        set_private_token_fault(PrivateTokenFault::None);
        fail_enrollment_audits(&fixture.target, false);
        assert!(!fixture.token_path.exists());
        let identity = NodeIdentity::load_existing(&fixture.target).unwrap();
        let registry =
            NodeRegistry::open_existing(&fixture.target, identity.public_status()).unwrap();
        recover_private_token_tombstones(
            &fixture.target,
            &registry,
            &fixture.organization,
            &fixture.token_path,
        )
        .unwrap();
        assert!(fixture.token_path.exists());
        assert!(list_trusted_peers(&fixture.target).unwrap().is_empty());

        let fixture = signed_bundle_fixture([24; 32], 34, 44);
        set_private_token_fault(PrivateTokenFault::Delete);
        let applied = apply_signed_bundle(&fixture.target, fixture.request.clone()).unwrap();
        assert!(applied.cleanup_pending);
        set_private_token_fault(PrivateTokenFault::None);
        assert!(!fixture.token_path.exists());
        assert_eq!(
            fixture
                .target
                .list_private_token_tombstones(
                    &fixture.token_path,
                    enrollment::MAX_BOOTSTRAP_TOKEN_BYTES,
                )
                .unwrap()
                .len(),
            1
        );
        let identity = NodeIdentity::load_existing(&fixture.target).unwrap();
        let registry =
            NodeRegistry::open_existing(&fixture.target, identity.public_status()).unwrap();
        set_private_token_fault(PrivateTokenFault::Delete);
        let recovery_error = recover_private_token_tombstones(
            &fixture.target,
            &registry,
            &fixture.organization,
            &fixture.token_path,
        )
        .unwrap_err();
        assert!(recovery_error
            .message
            .contains("bootstrap token cleanup recovery failed"));
        set_private_token_fault(PrivateTokenFault::None);
        recover_private_token_tombstones(
            &fixture.target,
            &registry,
            &fixture.organization,
            &fixture.token_path,
        )
        .unwrap();
        assert!(!fixture.token_path.exists());
        assert!(fixture
            .target
            .list_private_token_tombstones(
                &fixture.token_path,
                enrollment::MAX_BOOTSTRAP_TOKEN_BYTES,
            )
            .unwrap()
            .is_empty());
        let cleanup_count: i64 = Connection::open(fixture.target.database_path())
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM enrollment_audits WHERE event_code = 'cleanup_completed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cleanup_count, 1);
        set_private_token_fault(PrivateTokenFault::None);
    }

    #[test]
    fn cleanup_completion_failure_leaves_durable_pending_proof() {
        let _fault_lock = TOKEN_FAULT_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap();
        set_private_token_fault(PrivateTokenFault::None);
        let fixture = signed_bundle_fixture([25; 32], 35, 45);
        fail_cleanup_completion_audit(&fixture.target, true);
        let applied = apply_signed_bundle(&fixture.target, fixture.request.clone()).unwrap();
        assert!(applied.cleanup_pending);
        let state: String = Connection::open(fixture.target.database_path())
            .unwrap()
            .query_row(
                "SELECT cleanup_state FROM bootstrap_proofs WHERE consumed_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "pending");
        fail_cleanup_completion_audit(&fixture.target, false);
        recover_private_token_tombstones(
            &fixture.target,
            &NodeRegistry::open_existing(
                &fixture.target,
                NodeIdentity::load_existing(&fixture.target)
                    .unwrap()
                    .public_status(),
            )
            .unwrap(),
            &fixture.organization,
            &fixture.token_path,
        )
        .unwrap();
        let state: String = Connection::open(fixture.target.database_path())
            .unwrap()
            .query_row(
                "SELECT cleanup_state FROM bootstrap_proofs WHERE consumed_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "complete");
    }

    #[test]
    fn signed_bundle_distinct_conductors_have_one_transactional_winner() {
        let _fault_lock = TOKEN_FAULT_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap();
        set_private_token_fault(PrivateTokenFault::None);
        let target_temp = TempDir::new().unwrap();
        let target = context(&target_temp);
        let authority_private = [3_u8; 32];
        let authority = k256::schnorr::SigningKey::from_slice(&authority_private).unwrap();
        let token = "t".repeat(32);
        let nonce = [12_u8; 16];
        let mut config = NodeConfig::default();
        config.organization.id = "omakure".into();
        config.trust.enrollment = "signed-bundle".into();
        config.trust.bootstrap_token_hash =
            hash_hex(&enrollment::hash_bootstrap_token(token.as_bytes()));
        config.trust.bootstrap_nonce_hash = hash_hex(&enrollment::hash_bootstrap_nonce(&nonce));
        config.trust.authorities = vec![crate::domain::EnrollmentAuthority {
            key_id: hash_hex(&[8; 16]),
            public_key: hash_hex(&authority.verifying_key().to_bytes()),
            revoked: false,
        }];
        initialize_node(&target, &config).unwrap();

        let manager_a_temp = TempDir::new().unwrap();
        let manager_b_temp = TempDir::new().unwrap();
        let manager_a_context = context(&manager_a_temp);
        let manager_b_context = context(&manager_b_temp);
        let manager_a = NodeIdentity::load_or_initialize(&manager_a_context).unwrap();
        let manager_b = NodeIdentity::load_or_initialize(&manager_b_context).unwrap();
        let transport_a = LocalTransport::provision_new(&manager_a_context, &manager_a).unwrap();
        let transport_b = LocalTransport::provision_new(&manager_b_context, &manager_b).unwrap();
        let target_id = NodeIdentity::load_existing(&target)
            .unwrap()
            .public_status()
            .node_id
            .clone();
        let make_bundle =
            |bundle_id: [u8; 16], manager: &NodeIdentity, transport: &LocalTransport| {
                enrollment::SignedEnrollmentBundle::sign_with_material(
                    &authority_private,
                    bundle_id,
                    [8; 16],
                    "omakure".into(),
                    target_id.clone(),
                    manager.public_status().node_id.clone(),
                    enrollment::parse_hex(&manager.public_status().public_key_hex, 32)
                        .unwrap()
                        .try_into()
                        .unwrap(),
                    *transport.certificate().transport_public(),
                    *transport.certificate().as_bytes(),
                    EnrollmentRole::Conductor,
                    vec!["remote-run".into()],
                    enrollment::now_seconds(),
                    enrollment::now_seconds() + 600,
                )
                .unwrap()
            };
        let requests = [
            SignedBundleApplyRequest {
                bundle_hex: hash_hex(&make_bundle([13; 16], &manager_a, &transport_a).encode()),
                bootstrap_token: token.clone(),
                bootstrap_nonce: hash_hex(&nonce),
                bootstrap_token_path: None,
            },
            SignedBundleApplyRequest {
                bundle_hex: hash_hex(&make_bundle([14; 16], &manager_b, &transport_b).encode()),
                bootstrap_token: token,
                bootstrap_nonce: hash_hex(&nonce),
                bootstrap_token_path: None,
            },
        ];
        let target_a = target.clone();
        let target_b = target.clone();
        let request_a = requests[0].clone();
        let request_b = requests[1].clone();
        let first = std::thread::spawn(move || apply_signed_bundle(&target_a, request_a));
        let second = std::thread::spawn(move || apply_signed_bundle(&target_b, request_b));
        let results = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(list_trusted_peers(&target).unwrap().len(), 1);
        assert!(results.iter().any(|result| {
            result.as_ref().err().is_some_and(|error| {
                error.code == OperationErrorCode::Conflict
                    || error.code == OperationErrorCode::EnrollmentReplay
            })
        }));
    }
}

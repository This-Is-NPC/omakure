use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::errors::{map_execution_lock_error, map_identity_error, map_registry_error};
use super::require_confirmation;
use super::status::open_initialized_registry;
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::{
    NodeRegistry, PeerRecord, PeerRegistration, PeerRole, PeerSource, PeerState,
};
use crate::util::hex;
use serde::{Deserialize, Serialize};

const PUBLIC_PEER_LIMIT: usize = 256;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_error: Option<String>,
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
    let invalid = || {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            "transport certificate must be lowercase hexadecimal bytes",
        )
    };
    if value.len() != crate::direct_transport::MAX_CERTIFICATE_BYTES * 2 || !hex::is_lower(value) {
        return Err(invalid());
    }
    let bytes = hex::decode(value).ok_or_else(invalid)?;
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
    let _guard = crate::remote_cue::ExecutionGuard::acquire(context, &request.node_id)
        .map_err(map_execution_lock_error)?;
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

/// Revoke a peer, and stop the work it already caused.
///
/// Trust withdrawal is committed independently of the runs database: an
/// unavailable history store must not leave a peer trusted. Existing Cue work
/// is then cancelled and the response reports whether that cleanup was
/// confirmed. Workers perform the same registry check immediately before Cue
/// execution, so a race cannot turn a revoked peer's queued work into a new
/// process.
pub fn revoke_peer(
    context: &NodeContext,
    workspace: &crate::workspace::Workspace,
    request: RevocationRequest,
) -> OperationResult<PublicPeer> {
    require_confirmation(request.confirmed)?;
    let _guard = crate::remote_cue::ExecutionGuard::acquire(context, &request.node_id)
        .map_err(map_execution_lock_error)?;
    let registry = open_initialized_registry(context)?;
    if registry
        .peer(&request.node_id)
        .map_err(map_registry_error)?
        .is_none()
    {
        return Err(OperationError::new(
            OperationErrorCode::NotFound,
            format!("peer was not found: {}", request.node_id),
        ));
    }
    let peer = registry
        .revoke_peer(&request.node_id, &request.actor, &request.reason)
        .map_err(map_registry_error)?;
    let (cleanup_pending, cleanup_error) = match crate::runs::open(workspace) {
        Ok(runs) => match crate::runs::cancel_cue_runs_for_actor(&runs, &request.node_id) {
            Ok(_) => (false, None),
            Err(error) => (true, Some(error.to_string())),
        },
        Err(error) => (true, Some(format!("cannot open runs database: {error}"))),
    };
    let mut result = public_peer(peer);
    result.cleanup_pending = cleanup_pending;
    result.cleanup_error = cleanup_error;
    Ok(result)
}

/// Reconcile Cue rows for every peer the registry records as revoked. This is
/// safe to retry after a crash or a temporary runs-database failure; the worker
/// preflight remains the fail-closed barrier while reconciliation is pending.
pub fn reconcile_revoked_cue_runs(
    context: &NodeContext,
    workspace: &crate::workspace::Workspace,
) -> OperationResult<Vec<String>> {
    let identity = NodeIdentity::load_existing(context).map_err(map_identity_error)?;
    let registry = NodeRegistry::open_existing(context, identity.public_status())
        .map_err(map_registry_error)?;
    let revoked = registry
        .peers()
        .map_err(map_registry_error)?
        .into_iter()
        .filter(|peer| peer.state == PeerState::Revoked)
        .map(|peer| peer.node_id)
        .collect::<Vec<_>>();
    let conn = crate::runs::open(workspace).map_err(|error| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("cannot reconcile revoked Cue runs: {error}"),
        )
    })?;
    let mut cancelled = Vec::new();
    for actor in revoked {
        let _guard = crate::remote_cue::ExecutionGuard::acquire(context, &actor)
            .map_err(map_execution_lock_error)?;
        let rows = crate::runs::cancel_cue_runs_for_actor(&conn, &actor).map_err(|error| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                format!("cannot reconcile revoked Cue runs for {actor}: {error}"),
            )
        })?;
        cancelled.extend(rows);
    }
    Ok(cancelled)
}

pub(super) fn public_peer(peer: PeerRecord) -> PublicPeer {
    PublicPeer {
        node_id: peer.node_id,
        public_key: peer.public_key,
        role: peer.role.as_str().to_string(),
        state: peer.state.as_str().to_string(),
        capabilities: peer.capabilities,
        added_at: peer.added_at,
        updated_at: peer.updated_at,
        last_seen: peer.last_seen,
        source: peer.source.as_str().to_string(),
        cleanup_pending: false,
        cleanup_error: None,
    }
}

fn parse_role(value: &str) -> OperationResult<PeerRole> {
    PeerRole::from_wire(value).ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            "role must be conductor or performer",
        )
    })
}

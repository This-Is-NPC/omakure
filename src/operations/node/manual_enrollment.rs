use super::super::{OperationError, OperationErrorCode, OperationResult};
use super::errors::{
    map_direct_enrollment_error, map_enrollment_error, map_identity_error, map_registry_error,
    registry_error,
};
use super::status::{open_initialized_registry, read_node_config};
use super::trust::{list_trusted_peers, public_peer, PublicPeer};
use super::{decode_fixed_hex, require_confirmation};
use crate::enrollment::{self, EnrollmentRole, ManualEnrollmentRequest};
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::RegistryError;
use crate::node_transport::LocalTransport;
use crate::util::hex;
use serde::{Deserialize, Serialize};

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
        .verify(crate::util::time::unix_seconds())
        .map_err(map_enrollment_error)?;
    let registry = open_initialized_registry(context)?;
    registry
        .stage_manual_enrollment(
            request,
            transport_certificate,
            "authenticated-untrusted",
            "authenticated manual enrollment request",
            crate::util::time::unix_seconds(),
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
            crate::util::time::unix_seconds(),
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
        crate::util::time::unix_seconds(),
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
        .map(|(request, code)| (hex::encode(&request), hex::encode(&code)))
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
    let not_lowercase_hex = || {
        OperationError::new(
            OperationErrorCode::EnrollmentInvalid,
            "manual enrollment request bytes must be lowercase hexadecimal",
        )
    };
    if !hex::is_lower(value) {
        return Err(not_lowercase_hex());
    }
    hex::decode(value).ok_or_else(not_lowercase_hex)
}

fn parse_enrollment_role(value: &str) -> OperationResult<EnrollmentRole> {
    EnrollmentRole::from_wire(value).ok_or_else(|| {
        OperationError::new(
            OperationErrorCode::EnrollmentInvalid,
            "role must be conductor or performer",
        )
    })
}

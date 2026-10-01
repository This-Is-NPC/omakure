use super::bearer::{require_capability, require_scope};
use super::query::{query_pairs, query_value};
use super::respond::{operation_error_response, operation_response, parse_json_body};
use super::state::{ApiCapability, ApiState};
use super::SIGNED_BUNDLE_HTTP_BODY_LIMIT_BYTES;
use crate::auth::AuthContext;
use crate::node_registry::NodeRegistry;
use crate::operations::node as node_ops;
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::util::hex;
use axum::body::Body;
use axum::extract::{Path as AxumPath, RawQuery, State};
use axum::response::Response;
use axum::Extension;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Default, Deserialize)]
struct NodeInitializeBody {}

#[derive(Debug, Deserialize)]
struct ManualTrustBody {
    node_id: String,
    public_key: String,
    #[serde(default)]
    transport_certificate: Option<String>,
    role: String,
    #[serde(default)]
    capabilities: Vec<String>,
    actor: String,
    reason: String,
    confirmed: bool,
}

#[derive(Debug, Deserialize)]
struct NodeCapabilitiesBody {
    #[serde(default)]
    capabilities: Vec<String>,
    actor: String,
    reason: String,
    confirmed: bool,
}

#[derive(Debug, Deserialize)]
struct NodeRevokeBody {
    actor: String,
    reason: String,
    confirmed: bool,
}

/// One Cue, named by the peer it is for and the script it selects.
///
/// There is no argument list and no script content: a Cue selects among code
/// the Performer already declared, which is what stops remote management from
/// being able to introduce code onto a node.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeCueBody {
    peer_node_id: String,
    script: String,
    reason: String,
    #[serde(default = "default_cue_wait_seconds")]
    wait_seconds: u32,
    #[serde(default)]
    cue_id: Option<String>,
}

fn default_cue_wait_seconds() -> u32 {
    120
}

/// One baseline push, as the operator's CLI hands it to its own service.
///
/// The manifest arrives already signed. This route never signs one, and this
/// process holds no path to a publisher key: the operator signs where the key
/// is, and the service only carries what it is given.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeBaselineBody {
    peer_node_id: String,
    /// The signed manifest, lowercase hex.
    manifest: String,
    /// Script bodies in manifest order, lowercase hex. No paths: the manifest
    /// is the only thing that says where a script goes.
    scripts: Vec<String>,
    #[serde(default = "default_baseline_wait_seconds")]
    wait_seconds: u32,
}

fn default_baseline_wait_seconds() -> u32 {
    120
}

#[derive(Debug, Deserialize)]
struct NodeEnrollmentStageBody {
    request_hex: String,
    transport_certificate: String,
}

#[derive(Debug, Deserialize)]
struct NodeEnrollmentApprovalBody {
    request_hex: String,
    transport_certificate: String,
    code: String,
    actor: String,
    reason: String,
    confirmed: bool,
}

#[derive(Debug, Deserialize)]
struct NodeEnrollmentRejectBody {
    actor: String,
    reason: String,
    confirmed: bool,
}

#[derive(Debug, Deserialize)]
struct NodeSignedBundleApplyBody {
    bundle_hex: String,
    bootstrap_nonce: String,
}

pub(super) async fn node_status_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::NodeRead) {
        return response;
    }
    operation_response(node_context().and_then(|context| {
        node_ops::public_node_status(&context).map(|mut status| {
            status.transport = state
                .transport
                .as_ref()
                .and_then(|transport| transport.lock().ok().map(|status| status.clone()));
            status.discovery = node_ops::public_discovery_status_with_config(
                state.discovery.as_ref(),
                false,
                status.config.as_ref(),
            )
            .ok();
            status
        })
    }))
}

pub(super) async fn node_discovery_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::DiscoveryRead) {
        return response;
    }
    let include_addresses = query_pairs(raw_query.as_deref())
        .ok()
        .and_then(|pairs| query_value(&pairs, "include_addresses"))
        .is_some_and(|value| matches!(value.as_str(), "1" | "true"));
    operation_response(node_context().and_then(|context| {
        node_ops::public_node_status(&context).and_then(|status| {
            node_ops::public_discovery_status_with_config(
                state.discovery.as_ref(),
                include_addresses,
                status.config.as_ref(),
            )
        })
    }))
}

pub(super) async fn node_initialize_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::NodeWrite) {
        return response;
    }
    if let Err(error) =
        parse_json_body::<NodeInitializeBody>(body, state.deploy.http.body_limit_bytes).await
    {
        return operation_error_response(error);
    }
    operation_response(node_context().and_then(|context| {
        node_ops::initialize_node_nonblocking(&context, &crate::domain::NodeConfig::default())
    }))
}

/// Thin adapter over the protocol-neutral fleet-status operation.
///
/// It adds no business logic and no new authorization scheme: the existing
/// `node:read` management capability gates it, exactly as it gates
/// `GET /v1/node/status` and `GET /v1/node/peers`. A management call can read
/// this projection; it can never write it, because the only writer is the
/// authenticated node-to-node Health Plane exchange.
pub(super) async fn node_health_handler(
    State(registry): State<Arc<NodeRegistry>>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::NodeRead) {
        return response;
    }
    operation_response(crate::operations::health::fleet_status(&registry))
}

/// Thin adapter over the protocol-neutral Signal feed operation.
///
/// Same posture as `GET /v1/node/health`: no business logic, no new
/// authorization scheme, and no write path. The existing `node:read`
/// management capability gates it, and the only writer of the underlying
/// Signals is the authenticated node-to-node Health Plane exchange plus this
/// node's own append-only trust log.
pub(super) async fn node_signals_handler(
    State(registry): State<Arc<NodeRegistry>>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::NodeRead) {
        return response;
    }
    operation_response(crate::operations::health::signal_feed(&registry))
}

pub(super) async fn node_peers_handler(Extension(auth_ctx): Extension<AuthContext>) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::NodeRead) {
        return response;
    }
    operation_response(node_context().and_then(|context| node_ops::list_trusted_peers(&context)))
}

pub(super) async fn node_enrollments_handler(
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnrollmentRead) {
        return response;
    }
    operation_response(
        node_context().and_then(|context| node_ops::list_pending_enrollments(&context)),
    )
}

pub(super) async fn node_enrollment_stage_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnrollmentWrite) {
        return response;
    }
    let body =
        match parse_json_body::<NodeEnrollmentStageBody>(body, state.deploy.http.body_limit_bytes)
            .await
        {
            Ok(body) => body,
            Err(error) => return operation_error_response(error),
        };
    operation_response(node_context().and_then(|context| {
        node_ops::stage_manual_enrollment_hex(
            &context,
            &body.request_hex,
            &body.transport_certificate,
        )
    }))
}

pub(super) async fn node_enrollment_approve_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(node_id): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnrollmentWrite) {
        return response;
    }
    let body = match parse_json_body::<NodeEnrollmentApprovalBody>(
        body,
        state.deploy.http.body_limit_bytes,
    )
    .await
    {
        Ok(body) => body,
        Err(error) => return operation_error_response(error),
    };
    operation_response(node_context().and_then(|context| {
        node_ops::approve_manual_enrollment(
            &context,
            node_ops::ManualEnrollmentApprovalRequest {
                request_hex: body.request_hex,
                transport_certificate: body.transport_certificate,
                code: body.code,
                actor: body.actor,
                reason: body.reason,
                confirmed: body.confirmed,
                expected_node_id: Some(node_id),
            },
        )
    }))
}

pub(super) async fn node_enrollment_reject_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(node_id): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnrollmentWrite) {
        return response;
    }
    let body =
        match parse_json_body::<NodeEnrollmentRejectBody>(body, state.deploy.http.body_limit_bytes)
            .await
        {
            Ok(body) => body,
            Err(error) => return operation_error_response(error),
        };
    operation_response(node_context().and_then(|context| {
        node_ops::reject_manual_enrollment(
            &context,
            node_ops::ManualEnrollmentRejectionRequest {
                node_id,
                actor: body.actor,
                reason: body.reason,
                confirmed: body.confirmed,
            },
        )
    }))
}

pub(super) async fn node_signed_bundle_apply_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnrollmentWrite) {
        return response;
    }
    let body = match parse_json_body::<NodeSignedBundleApplyBody>(
        body,
        state
            .deploy
            .http
            .body_limit_bytes
            .min(SIGNED_BUNDLE_HTTP_BODY_LIMIT_BYTES),
    )
    .await
    {
        Ok(body) => body,
        Err(error) => return operation_error_response(error),
    };
    operation_response(node_context().and_then(|context| {
        node_ops::apply_signed_bundle_from_local_token(
            &context,
            node_ops::SignedBundleApplyRequest {
                bundle_hex: body.bundle_hex,
                bootstrap_token: String::new(),
                bootstrap_nonce: body.bootstrap_nonce,
                bootstrap_token_path: None,
            },
            &auth_ctx.token_id,
        )
    }))
}

pub(super) async fn node_trust_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "trust:write") {
        return response;
    }
    let body =
        match parse_json_body::<ManualTrustBody>(body, state.deploy.http.body_limit_bytes).await {
            Ok(body) => body,
            Err(error) => return operation_error_response(error),
        };
    operation_response(node_context().and_then(|context| {
        node_ops::import_manual_trust(
            &context,
            node_ops::ManualTrustRequest {
                node_id: body.node_id,
                public_key: body.public_key,
                transport_certificate: body.transport_certificate,
                role: body.role,
                capabilities: body.capabilities,
                actor: body.actor,
                reason: body.reason,
                confirmed: body.confirmed,
            },
        )
    }))
}

pub(super) async fn node_capabilities_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(node_id): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "trust:write") {
        return response;
    }
    let body =
        match parse_json_body::<NodeCapabilitiesBody>(body, state.deploy.http.body_limit_bytes)
            .await
        {
            Ok(body) => body,
            Err(error) => return operation_error_response(error),
        };
    operation_response(node_context().and_then(|context| {
        node_ops::update_peer_capabilities(
            &context,
            node_ops::CapabilityUpdateRequest {
                node_id,
                capabilities: body.capabilities,
                actor: body.actor,
                reason: body.reason,
                confirmed: body.confirmed,
            },
        )
    }))
}

/// Dispatch one Cue over the session this process already holds.
///
/// Deliberately not a second way to authorize anything: every gate is on the
/// receiving node, read from its own registry and config. What this route
/// decides is only whether *this* operator may ask, which is the same
/// `node:write` scope that governs the rest of the node surface.
pub(super) async fn node_cue_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "node:write") {
        return response;
    }
    let body = match parse_json_body::<NodeCueBody>(body, state.deploy.http.body_limit_bytes).await
    {
        Ok(body) => body,
        Err(error) => return operation_error_response(error),
    };
    if body
        .cue_id
        .as_ref()
        .is_some_and(|cue_id| !crate::remote_cue::is_well_formed_cue_id(cue_id))
    {
        return operation_error_response(OperationError::new(
            OperationErrorCode::InvalidInput,
            "cue id must be 32 lowercase hexadecimal characters",
        ));
    }
    let Some(dispatcher) = state.cues.clone() else {
        return operation_error_response(OperationError::new(
            OperationErrorCode::InvalidInput,
            "no direct transport is running, so there is no session to carry a cue",
        ));
    };
    // "No session with that peer" is a different fact from a refusal, and is
    // reported as one so a caller can reach that peer another way instead of
    // reading it as a verdict.
    if !dispatcher.has_session(&body.peer_node_id) {
        return operation_error_response(OperationError::new(
            OperationErrorCode::NotFound,
            "this node holds no session with that peer",
        ));
    }
    let wait = Duration::from_secs(u64::from(body.wait_seconds.min(MAX_CUE_WAIT_SECONDS)));
    // The dispatch blocks on the session thread, so it must not hold a runtime
    // worker for its whole budget.
    let dispatched = tokio::task::spawn_blocking(move || {
        dispatcher.dispatch(
            &body.peer_node_id,
            &body.script,
            &body.reason,
            wait,
            body.cue_id.as_deref(),
        )
    })
    .await;
    let result = match dispatched {
        Ok(Ok(outcome)) => Ok(serde_json::json!({
            "dispatched": true,
            "via": "service",
            "cue_id": outcome.cue_id,
            "expected_run_id": outcome.expected_run_id,
            "answered": outcome.answered,
            "accepted": outcome.accepted,
            "code": outcome.code,
            "outcome_seen": outcome.outcome_seen,
        })),
        Ok(Err(error)) => Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("cue dispatch failed: {error}"),
        )),
        Err(_) => Err(OperationError::new(
            OperationErrorCode::IoFailed,
            "cue dispatch task failed",
        )),
    };
    operation_response(result)
}

/// The ceiling on how long one request may hold a connection waiting.
const MAX_CUE_WAIT_SECONDS: u32 = 600;

/// Hand one already-signed baseline to the session this service holds.
///
/// Under `node:write`, the same scope `POST /v1/node/cues` uses and no new
/// capability. The route decides only whether this operator may ask; every
/// authorization that matters — the gate, the publisher, the organization, the
/// signature, the content hashes — is enforced on the receiving node against
/// facts it reads locally, and nothing here can influence any of them.
pub(super) async fn node_baseline_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "node:write") {
        return response;
    }
    let body =
        match parse_json_body::<NodeBaselineBody>(body, state.deploy.http.body_limit_bytes).await {
            Ok(body) => body,
            Err(error) => return operation_error_response(error),
        };
    let Some(dispatcher) = state.baselines.clone() else {
        return operation_error_response(OperationError::new(
            OperationErrorCode::InvalidInput,
            "no direct transport is running, so there is no session to carry a baseline",
        ));
    };
    // "No session with that peer" is a different fact from a refusal, and is
    // reported as one so a caller can reach that peer another way instead of
    // reading it as a verdict.
    if !dispatcher.has_session(&body.peer_node_id) {
        return operation_error_response(OperationError::new(
            OperationErrorCode::NotFound,
            "this node holds no session with that peer",
        ));
    }
    let (Some(manifest), Some(scripts)) = (
        decode_lower_hex(&body.manifest),
        body.scripts
            .iter()
            .map(|body| decode_lower_hex(body))
            .collect::<Option<Vec<_>>>(),
    ) else {
        return operation_error_response(OperationError::new(
            OperationErrorCode::InvalidInput,
            "the manifest and every script body must be lowercase hex",
        ));
    };
    let wait = Duration::from_secs(u64::from(body.wait_seconds.min(MAX_CUE_WAIT_SECONDS)));
    // The push blocks on the session thread, so it must not hold a runtime
    // worker for its whole budget.
    let pushed = tokio::task::spawn_blocking(move || {
        dispatcher.push_baseline(&body.peer_node_id, &manifest, &scripts, wait)
    })
    .await;
    let result = match pushed {
        Ok(Ok(outcome)) => Ok(serde_json::json!({
            "pushed": true,
            "via": "service",
            "baseline_id": outcome.baseline_id,
            "answered": outcome.answered,
            "accepted": outcome.accepted,
            "code": outcome.code,
        })),
        Ok(Err(error)) => Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("baseline push failed: {error}"),
        )),
        Err(_) => Err(OperationError::new(
            OperationErrorCode::IoFailed,
            "baseline push task failed",
        )),
    };
    operation_response(result)
}

/// The body `POST /v1/node/baseline/rollback` takes.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeBaselineRollbackBody {
    confirmed: bool,
}

/// Put this node back on the baseline it retained before the current one.
///
/// A local act on the machine that holds the scripts, exposed here for the same
/// reason every other local act is: an operator working over the management API
/// should not have to open a shell. It is `node:write` on *this* node's own
/// surface and reaches no peer. Nothing a Conductor can send arrives here: the
/// baseline plane carries exactly two node-to-node kinds and neither one asks a
/// Performer to change which version it runs.
pub(super) async fn node_baseline_rollback_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "node:write") {
        return response;
    }
    let body =
        match parse_json_body::<NodeBaselineRollbackBody>(body, state.deploy.http.body_limit_bytes)
            .await
        {
            Ok(body) => body,
            Err(error) => return operation_error_response(error),
        };
    operation_response(node_context().and_then(|context| {
        let policy = crate::baseline_push::read_policy(&context);
        crate::operations::baseline::rollback_baseline(
            &state.workspace,
            &policy,
            body.confirmed,
            crate::direct_transport::unix_seconds() as i64,
        )
    }))
}

/// Decode lowercase hex, refusing upper case so one artefact has one spelling.
fn decode_lower_hex(value: &str) -> Option<Vec<u8>> {
    if !hex::is_lower(value) {
        return None;
    }
    hex::decode(value)
}

pub(super) async fn node_revoke_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(node_id): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "trust:write") {
        return response;
    }
    let body =
        match parse_json_body::<NodeRevokeBody>(body, state.deploy.http.body_limit_bytes).await {
            Ok(body) => body,
            Err(error) => return operation_error_response(error),
        };
    operation_response(node_context().and_then(|context| {
        node_ops::revoke_peer(
            &context,
            &state.workspace,
            node_ops::RevocationRequest {
                node_id,
                actor: body.actor,
                reason: body.reason,
                confirmed: body.confirmed,
            },
        )
    }))
}

fn node_context() -> OperationResult<crate::node::NodeContext> {
    node_ops::resolve_context(crate::node::NodePathOverrides::default())
}

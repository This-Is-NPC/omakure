use super::audit::AuditRunId;
use crate::cli::json;
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use axum::body::{to_bytes, Body};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

pub(super) async fn parse_json_body<T: for<'de> Deserialize<'de>>(
    body: Body,
    limit_bytes: usize,
) -> OperationResult<T> {
    let bytes = to_bytes(body, limit_bytes).await.map_err(|err| {
        let message = err.to_string();
        if message.contains("length limit") {
            OperationError::new(
                OperationErrorCode::PayloadTooLarge,
                "request body is too large",
            )
        } else {
            OperationError::new(
                OperationErrorCode::InvalidInput,
                format!("invalid request body: {message}"),
            )
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|err| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("invalid JSON request body: {err}"),
        )
    })
}

pub(super) fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json::err_envelope(code, message))).into_response()
}

pub(super) fn operation_response<T: Serialize>(result: OperationResult<T>) -> Response {
    match result {
        Ok(data) => (StatusCode::OK, Json(json::ok_envelope(data))).into_response(),
        Err(err) => operation_error_response(err),
    }
}

pub(super) fn operation_response_with_run_id<T: Serialize>(
    result: OperationResult<T>,
    run_id: Option<String>,
) -> Response {
    attach_audit_run_id(operation_response(result), run_id)
}

pub(super) fn attach_audit_run_id(mut response: Response, run_id: Option<String>) -> Response {
    if let Some(run_id) = run_id {
        response.extensions_mut().insert(AuditRunId(run_id));
    }
    response
}

pub(super) fn operation_error_response(err: OperationError) -> Response {
    let status = match err.code {
        OperationErrorCode::InvalidInput
        | OperationErrorCode::UnsafePath
        | OperationErrorCode::ManifestInvalid
        | OperationErrorCode::EnrollmentInvalid
        | OperationErrorCode::DiscoveryUnsupportedVersion
        | OperationErrorCode::DiscoveryInvalidBeacon
        | OperationErrorCode::DiscoveryMessageTooLarge
        | OperationErrorCode::DiscoveryExpired
        | OperationErrorCode::DiscoveryFuture
        | OperationErrorCode::DiscoverySecretMismatch
        | OperationErrorCode::DiscoveryIdentityMismatch
        | OperationErrorCode::DiscoverySignatureInvalid => StatusCode::BAD_REQUEST,
        OperationErrorCode::Forbidden | OperationErrorCode::EnrollmentDisabled => {
            StatusCode::FORBIDDEN
        }
        OperationErrorCode::NotFound => StatusCode::NOT_FOUND,
        OperationErrorCode::AlreadyExists
        | OperationErrorCode::Conflict
        | OperationErrorCode::NotSynced
        | OperationErrorCode::EnrollmentExpired
        | OperationErrorCode::EnrollmentReplay
        | OperationErrorCode::EnrollmentMismatch
        | OperationErrorCode::EnrollmentDenied
        | OperationErrorCode::EnrollmentRateLimited
        | OperationErrorCode::DiscoveryRateLimited
        | OperationErrorCode::DiscoveryCandidateLimit => StatusCode::CONFLICT,
        OperationErrorCode::UnsupportedScript => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        OperationErrorCode::DiscoveryUnsupportedPlatform => StatusCode::NOT_IMPLEMENTED,
        OperationErrorCode::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        OperationErrorCode::GitFailed
        | OperationErrorCode::IoFailed
        | OperationErrorCode::RegistryInvalid
        | OperationErrorCode::DiscoveryInternal
        | OperationErrorCode::TransportUnsupportedVersion
        | OperationErrorCode::TransportInvalidFrame
        | OperationErrorCode::TransportMessageTooLarge
        | OperationErrorCode::TransportHandshakeFailed
        | OperationErrorCode::TransportIdentityMismatch
        | OperationErrorCode::TransportNotEnrolled
        | OperationErrorCode::TransportRevoked
        | OperationErrorCode::TransportExpired
        | OperationErrorCode::TransportReplay
        | OperationErrorCode::TransportRateLimited
        | OperationErrorCode::TransportInternal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response(status, err.code.as_str(), &err.message)
}

use super::bearer::require_capability;
use super::blocking::run_bounded;
use super::respond::{error_response, operation_error_response};
use super::state::{ApiCapability, ApiState};
use crate::auth::AuthContext;
use crate::cli::json;
use axum::Extension;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

pub(super) async fn list_secrets_metadata_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if !state.deploy.secrets.metadata_endpoint {
        return error_response(
            StatusCode::NOT_FOUND,
            "not_found",
            "secrets metadata endpoint is disabled by policy",
        );
    }
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::SecretsReadMetadata) {
        return response;
    }
    let access = state.policy.secret_access(&auth_ctx);
    // Metadata listing also accepts credentials:use as a read-adjacent scope when
    // secrets:read-metadata is granted; secret_access already folds both scopes.
    let gate = Arc::clone(&state.blocking_operation_gate);
    let metadata = match run_bounded("secret metadata", gate, move || {
        crate::secrets::list_secret_metadata(&state.workspace, &access)
    })
    .await
    {
        Ok(metadata) => metadata,
        Err(err) => return operation_error_response(err),
    };
    (StatusCode::OK, Json(json::ok_envelope(metadata))).into_response()
}

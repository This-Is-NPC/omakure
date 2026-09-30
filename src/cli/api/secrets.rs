use super::bearer::require_capability;
use super::respond::error_response;
use super::state::{ApiCapability, ApiState};
use crate::auth::AuthContext;
use crate::cli::json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;

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
    let metadata = crate::secrets::list_secret_metadata(&state.workspace, &access);
    (StatusCode::OK, Json(json::ok_envelope(metadata))).into_response()
}

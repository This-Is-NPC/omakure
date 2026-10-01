use super::bearer::{require_capability, require_scope};
use super::blocking::{operation_response_bounded, run_bounded};
use super::respond::operation_error_response;
use super::state::{ApiCapability, ApiState};
use crate::auth::{self, AuthContext};
use crate::cli::json;
use crate::operations::config as config_ops;
use crate::operations::core;
use crate::operations::doctor as doctor_ops;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct ReadyResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct AdminStatusResponse {
    ready: bool,
    readiness: AdminReadinessDetails,
    auth: auth::AuthStatus,
}

#[derive(Serialize)]
struct AdminReadinessDetails {
    requires_worker: bool,
    requires_scheduler: bool,
    workers_configured: bool,
    scheduler_configured: bool,
    workers_alive: bool,
    scheduler_alive: bool,
    requires_transport: bool,
    transport_configured: bool,
    transport_alive: bool,
}

pub(super) async fn health() -> Json<serde_json::Value> {
    Json(json::ok_envelope(HealthResponse { status: "ok" }))
}

pub(super) async fn ready_handler(State(state): State<ApiState>) -> Response {
    let ready = state
        .readiness
        .as_ref()
        .map(|gate| gate.is_ready())
        .unwrap_or(true);
    if ready {
        (
            StatusCode::OK,
            Json(json::ok_envelope(ReadyResponse { status: "ready" })),
        )
            .into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json::ok_envelope(ReadyResponse {
                status: "not_ready",
            })),
        )
            .into_response()
    }
}

pub(super) async fn admin_status_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "admin:status") {
        return response;
    }
    let (ready, readiness) = match &state.readiness {
        Some(gate) => (
            gate.is_ready(),
            AdminReadinessDetails {
                requires_worker: gate.requires_worker,
                requires_scheduler: gate.requires_scheduler,
                workers_configured: gate.workers_configured,
                scheduler_configured: gate.scheduler_configured,
                workers_alive: gate.workers_alive.load(Ordering::SeqCst),
                scheduler_alive: gate.scheduler_alive.load(Ordering::SeqCst),
                requires_transport: gate.requires_transport,
                transport_configured: gate.transport_configured,
                transport_alive: gate.transport_alive.load(Ordering::SeqCst),
            },
        ),
        None => (
            true,
            AdminReadinessDetails {
                requires_worker: false,
                requires_scheduler: false,
                workers_configured: false,
                scheduler_configured: false,
                workers_alive: false,
                scheduler_alive: false,
                requires_transport: false,
                transport_configured: false,
                transport_alive: false,
            },
        ),
    };
    let gate = Arc::clone(&state.blocking_operation_gate);
    let auth = match run_bounded("admin status", gate, move || state.auth.status()).await {
        Ok(auth) => auth,
        Err(err) => return operation_error_response(err),
    };
    (
        StatusCode::OK,
        Json(json::ok_envelope(AdminStatusResponse {
            ready,
            readiness,
            auth,
        })),
    )
        .into_response()
}

pub(super) async fn workspace_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::ConfigRead) {
        return response;
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    operation_response_bounded("workspace", gate, move || {
        core::workspace_summary(&state.workspace)
    })
    .await
}

pub(super) async fn config_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::ConfigRead) {
        return response;
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    operation_response_bounded("config", gate, move || {
        config_ops::redacted_config_summary(&state.workspace)
    })
    .await
}

pub(super) async fn doctor_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::ConfigRead) {
        return response;
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    operation_response_bounded("doctor", gate, move || {
        doctor_ops::doctor_report(&state.workspace)
    })
    .await
}

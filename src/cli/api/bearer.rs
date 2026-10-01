use super::audit::{
    AuditRunId, HttpAuditEvent, emit_http_audit_async, mutation_path_run_id, safe_audit_run_id,
};
use super::respond::error_response;
use super::router::HealthPlaneAuthState;
use super::state::{ApiCapability, ApiState};
use crate::auth::{AuthContext, Authenticator};
use crate::policy::DeployPolicy;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

// Concurrent Argon2id verifications are capped by
// `deploy.auth.max_concurrent_verifications` (see
// `policy::DEFAULT_MAX_CONCURRENT_AUTH_VERIFICATIONS`). Each verify is
// memory-hard (~64 MiB); without a bound, unauthenticated requests carrying
// any bearer string could amplify hashing into CPU/memory exhaustion.
// Verifies also run on the blocking pool (see `require_bearer`) so async
// reactor threads never stall — keeping `/v1/health` and `/v1/ready`
// responsive even under an auth flood.

/// Authenticate a presented bearer token without blocking the async runtime.
/// The memory-hard verify runs on the blocking pool under a concurrency permit.
enum AuthAttempt {
    Accepted(AuthContext),
    Rejected,
    Busy,
}

async fn authenticate_off_runtime(
    auth: &Authenticator,
    gate: &Arc<tokio::sync::Semaphore>,
    presented: &str,
) -> AuthAttempt {
    // Hold the permit for the LIFETIME OF THE HASH, not of the request future.
    // `spawn_blocking` is detached: if the client cancels mid-verify the request
    // future is dropped, but the Argon2 task keeps running. Moving an *owned*
    // permit into the blocking closure ties the permit's release to the hash
    // completing, so a "send bearer then reset connection" flood cannot orphan
    // unbounded memory-hard hashes past MAX_CONCURRENT_AUTH_VERIFICATIONS.
    let Ok(permit) = Arc::clone(gate).try_acquire_owned() else {
        return AuthAttempt::Busy;
    };
    let auth = auth.clone();
    let token = presented.to_string();
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        auth.authenticate(&token)
    })
    .await
    {
        Ok(Some(context)) => AuthAttempt::Accepted(context),
        Ok(None) | Err(_) => AuthAttempt::Rejected,
    }
}

fn health_plane_public_path(stripped_path: &str) -> String {
    match stripped_path {
        "/health" => "/v1/node/health".to_string(),
        "/signals" => "/v1/node/signals".to_string(),
        other => other.to_string(),
    }
}

async fn bearer_auth_attempt(
    auth: &Authenticator,
    auth_verification_gate: &Arc<tokio::sync::Semaphore>,
    headers: &HeaderMap,
) -> AuthAttempt {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match presented {
        Some(token) => authenticate_off_runtime(auth, auth_verification_gate, token).await,
        None => AuthAttempt::Rejected,
    }
}

async fn finish_bearer_auth(
    authenticated: AuthAttempt,
    deploy: &DeployPolicy,
    method: String,
    path: String,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    match authenticated {
        AuthAttempt::Accepted(ctx) => {
            if let Some(message) = deploy.deny_reason(&method, &path) {
                return audited_response(
                    HttpAuditEvent {
                        token_id: Some(ctx.token_id),
                        run_id: mutation_path_run_id(&method, &path),
                        method,
                        path,
                        outcome: "forbidden".to_string(),
                        status: StatusCode::FORBIDDEN.as_u16(),
                    },
                    error_response(StatusCode::FORBIDDEN, "forbidden", message),
                )
                .await;
            }
            let token_id = ctx.token_id.clone();
            request.extensions_mut().insert(ctx);
            let response = next.run(request).await;
            let status = response.status().as_u16();
            let run_id = response
                .extensions()
                .get::<AuditRunId>()
                .and_then(|value| safe_audit_run_id(&value.0))
                .or_else(|| mutation_path_run_id(&method, &path));
            let outcome = if (200..400).contains(&status) {
                "ok"
            } else if status == 403 {
                "forbidden"
            } else if status == 401 {
                "unauthorized"
            } else {
                "error"
            };
            audited_response(
                HttpAuditEvent {
                    token_id: Some(token_id),
                    run_id,
                    method,
                    path,
                    outcome: outcome.to_string(),
                    status,
                },
                response,
            )
            .await
        }
        AuthAttempt::Rejected => {
            audited_response(
                HttpAuditEvent {
                    token_id: None,
                    run_id: None,
                    method,
                    path,
                    outcome: "unauthorized".to_string(),
                    status: StatusCode::UNAUTHORIZED.as_u16(),
                },
                error_response(
                    StatusCode::UNAUTHORIZED,
                    "unauthorized",
                    "bearer token required",
                ),
            )
            .await
        }
        AuthAttempt::Busy => {
            audited_response(
                HttpAuditEvent {
                    token_id: None,
                    run_id: None,
                    method,
                    path,
                    outcome: "unavailable".to_string(),
                    status: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                },
                error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "auth_busy",
                    "authentication capacity is temporarily exhausted",
                ),
            )
            .await
        }
    }
}

async fn audited_response(event: HttpAuditEvent, response: Response) -> Response {
    match emit_http_audit_async(event).await {
        Ok(()) => response,
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "audit_unavailable",
            "HTTP audit is unavailable",
        ),
    }
}

pub(super) async fn health_plane_require_bearer(
    auth_state: HealthPlaneAuthState,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = health_plane_public_path(request.uri().path());
    let method = request.method().as_str().to_string();
    let authenticated = bearer_auth_attempt(
        &auth_state.auth,
        &auth_state.auth_verification_gate,
        &headers,
    )
    .await;
    finish_bearer_auth(
        authenticated,
        &auth_state.deploy,
        method,
        path,
        request,
        next,
    )
    .await
}

pub(super) async fn require_bearer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().as_str().to_string();
    if path == "/v1/health" || path == "/v1/ready" {
        return next.run(request).await;
    }

    let authenticated =
        bearer_auth_attempt(&state.auth, &state.auth_verification_gate, &headers).await;
    finish_bearer_auth(authenticated, &state.deploy, method, path, request, next).await
}

pub(super) fn require_capability(
    auth: &AuthContext,
    capability: ApiCapability,
) -> Option<Response> {
    require_scope(auth, capability.as_scope())
}

pub(super) fn require_scope(auth: &AuthContext, scope: &str) -> Option<Response> {
    (!auth.has_scope(scope)).then(|| {
        error_response(
            StatusCode::FORBIDDEN,
            "forbidden",
            "token is not permitted for this operation",
        )
    })
}

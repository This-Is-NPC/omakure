use super::battery::{
    add_battery_handler, inspect_battery_handler, install_battery_script_handler,
    list_batteries_handler, list_battery_scripts_handler, remove_battery_handler,
    sync_battery_handler,
};
use super::bearer::{health_plane_require_bearer, require_bearer};
#[cfg(test)]
use super::boot::auth_verification_gate;
use super::envs::{
    activate_env_handler, create_env_handler, deactivate_env_handler, delete_env_handler,
    delete_env_param_handler, list_envs_handler, patch_env_handler, put_env_handler,
    set_env_param_handler, show_env_handler,
};
use super::node::{
    node_baseline_handler, node_baseline_rollback_handler, node_capabilities_handler,
    node_cue_handler, node_discovery_handler, node_enrollment_approve_handler,
    node_enrollment_reject_handler, node_enrollment_stage_handler, node_enrollments_handler,
    node_health_handler, node_initialize_handler, node_peers_handler, node_revoke_handler,
    node_signals_handler, node_signed_bundle_apply_handler, node_status_handler,
    node_trust_handler,
};
use super::respond::error_response;
use super::runs::{
    cancel_run_handler, dead_letter_run_handler, enqueue_run_handler, list_runs_handler,
    list_traces_handler, queue_stats_handler, show_run_handler,
};
use super::scripts::{
    list_scripts_handler, script_path_handler, search_handler, tree_path_handler, tree_root_handler,
};
use super::secrets::list_secrets_metadata_handler;
#[cfg(test)]
use super::state::MAX_CONCURRENT_BLOCKING_OPERATIONS;
use super::state::{ApiPolicy, ApiState, ReadinessGate};
use super::status::{
    admin_status_handler, config_handler, doctor_handler, health, ready_handler, workspace_handler,
};
use super::SIGNED_BUNDLE_HTTP_BODY_LIMIT_BYTES;
use crate::auth::Authenticator;
use crate::direct_service::TransportStatusHandle;
use crate::node_registry::NodeRegistry;
#[cfg(test)]
use crate::operations::node as node_ops;
use crate::policy::DeployPolicy;
use crate::workspace::Workspace;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::Router;
use std::sync::Arc;

#[cfg(test)]
pub(super) const BODY_LIMIT_BYTES: usize = 1024 * 1024;

/// Auth bundle for Health Plane HTTP routes (`State` is `Arc<NodeRegistry>`).
#[derive(Clone)]
pub(super) struct HealthPlaneAuthState {
    pub(super) auth: Authenticator,
    pub(super) deploy: DeployPolicy,
    pub(super) auth_verification_gate: Arc<tokio::sync::Semaphore>,
}

/// Health Plane read routes mounted under `/v1/node` by `serve_http`.
///
/// Handlers use `State<Arc<NodeRegistry>>`; the registry is opened once by
/// `omakure node serve`, not by `omakure api`.
pub(crate) fn health_plane_router(
    registry: Arc<NodeRegistry>,
    auth: Authenticator,
    deploy: DeployPolicy,
    auth_verification_gate: Arc<tokio::sync::Semaphore>,
    body_limit: usize,
) -> Router {
    let auth_state = HealthPlaneAuthState {
        auth,
        deploy,
        auth_verification_gate,
    };
    Router::new()
        .route("/health", get(node_health_handler))
        .route("/signals", get(node_signals_handler))
        .layer(axum::extract::DefaultBodyLimit::max(body_limit))
        .layer(middleware::from_fn(
            move |request: Request<Body>, next: Next| {
                let auth_state = auth_state.clone();
                async move {
                    let headers = request.headers().clone();
                    health_plane_require_bearer(auth_state, headers, request, next).await
                }
            },
        ))
        .with_state(registry)
}

#[cfg(test)]
pub(super) fn router(workspace: Workspace) -> Router {
    router_with_blocking_gate(
        workspace,
        Arc::new(tokio::sync::Semaphore::new(
            MAX_CONCURRENT_BLOCKING_OPERATIONS,
        )),
    )
}

#[cfg(test)]
pub(super) fn router_with_blocking_gate(
    workspace: Workspace,
    blocking_operation_gate: Arc<tokio::sync::Semaphore>,
) -> Router {
    // Test convenience: wildcard scope plus unrestricted secret refs.
    // Production scope `*` still requires explicit `--secret-ref`.
    let deploy = DeployPolicy::default();
    let auth_gate = auth_verification_gate(&deploy);
    router_with_transport(
        crate::auth::test_credential::authenticator(&["*"]),
        workspace,
        ApiPolicy::with_secret_refs(["*"]),
        deploy,
        None,
        None,
        None,
        None,
        None,
        auth_gate,
        blocking_operation_gate,
        BODY_LIMIT_BYTES,
    )
}

#[cfg(test)]
pub(super) fn router_with_auth(
    auth: Authenticator,
    workspace: Workspace,
    policy: ApiPolicy,
) -> Router {
    router_with_policy(
        auth,
        workspace,
        policy,
        DeployPolicy::default(),
        None,
        BODY_LIMIT_BYTES,
    )
}

#[cfg(test)]
pub(super) fn router_with_deploy(
    auth: Authenticator,
    workspace: Workspace,
    policy: ApiPolicy,
    deploy: DeployPolicy,
) -> Router {
    router_with_policy(auth, workspace, policy, deploy, None, BODY_LIMIT_BYTES)
}

/// Canonical `(method, path)` inventory for the HTTP management API.
///
/// Keep this list in lockstep with `router_with_policy`. Black-box E2E tests
/// parse the markers below so route drift fails the suite without importing
/// the binary crate as a library.
// OMAKURE_HTTP_ROUTE_INVENTORY_START
pub const HTTP_ROUTE_INVENTORY: &[(&str, &str)] = &[
    ("GET", "/v1/health"),
    ("GET", "/v1/ready"),
    ("GET", "/v1/admin/status"),
    ("GET", "/v1/config"),
    ("GET", "/v1/doctor"),
    ("GET", "/v1/workspace"),
    ("GET", "/v1/search"),
    ("GET", "/v1/tree"),
    ("GET", "/v1/tree/*path"),
    ("GET", "/v1/scripts"),
    ("GET", "/v1/scripts/*script_id"),
    ("GET", "/v1/envs"),
    ("POST", "/v1/envs"),
    ("DELETE", "/v1/envs/active"),
    ("GET", "/v1/envs/:name"),
    ("PUT", "/v1/envs/:name"),
    ("PATCH", "/v1/envs/:name"),
    ("DELETE", "/v1/envs/:name"),
    ("POST", "/v1/envs/:name/activate"),
    ("PUT", "/v1/envs/:name/params/:key"),
    ("DELETE", "/v1/envs/:name/params/:key"),
    ("GET", "/v1/runs"),
    ("POST", "/v1/runs"),
    ("GET", "/v1/runs/:run_id"),
    ("GET", "/v1/runs/:run_id/traces"),
    ("POST", "/v1/runs/:run_id/cancel"),
    ("POST", "/v1/runs/:run_id/dead-letter"),
    ("GET", "/v1/queue/stats"),
    ("GET", "/v1/batteries"),
    ("POST", "/v1/batteries"),
    ("GET", "/v1/batteries/:battery_id"),
    ("DELETE", "/v1/batteries/:battery_id"),
    ("GET", "/v1/batteries/:battery_id/scripts"),
    (
        "POST",
        "/v1/batteries/:battery_id/scripts/:script_id/install",
    ),
    ("POST", "/v1/batteries/:battery_id/sync"),
    ("GET", "/v1/secrets"),
    ("GET", "/v1/node/status"),
    ("GET", "/v1/node/discovery"),
    ("POST", "/v1/node/init"),
    ("GET", "/v1/node/health"),
    ("GET", "/v1/node/signals"),
    ("POST", "/v1/node/cues"),
    ("POST", "/v1/node/baselines"),
    ("POST", "/v1/node/baseline/rollback"),
    ("GET", "/v1/node/peers"),
    ("POST", "/v1/node/peers"),
    ("GET", "/v1/node/enrollments"),
    ("POST", "/v1/node/enrollments"),
    ("POST", "/v1/node/enrollments/:node_id/approve"),
    ("POST", "/v1/node/enrollments/:node_id/reject"),
    ("POST", "/v1/node/enrollment/bundle"),
    ("PATCH", "/v1/node/peers/:node_id/capabilities"),
    ("POST", "/v1/node/peers/:node_id/revoke"),
];

// OMAKURE_HTTP_ROUTE_INVENTORY_END

#[cfg(test)]
pub(super) fn router_with_policy(
    auth: Authenticator,
    workspace: Workspace,
    policy: ApiPolicy,
    deploy: DeployPolicy,
    readiness: Option<Arc<ReadinessGate>>,
    body_limit: usize,
) -> Router {
    let auth_gate = auth_verification_gate(&deploy);
    router_with_transport(
        auth,
        workspace,
        policy,
        deploy,
        readiness,
        None,
        None,
        None,
        None,
        auth_gate,
        Arc::new(tokio::sync::Semaphore::new(
            MAX_CONCURRENT_BLOCKING_OPERATIONS,
        )),
        body_limit,
    )
}

// Keep transport and discovery handles explicit so test routers cannot hide
// runtime status behind global state.
#[allow(clippy::too_many_arguments)]
pub(super) fn router_with_transport(
    auth: Authenticator,
    workspace: Workspace,
    policy: ApiPolicy,
    deploy: DeployPolicy,
    readiness: Option<Arc<ReadinessGate>>,
    transport: Option<TransportStatusHandle>,
    discovery: Option<crate::discovery::DiscoveryStatusHandle>,
    cues: Option<crate::direct_service::CueDispatcher>,
    baselines: Option<crate::direct_service::BaselineDispatcher>,
    auth_verification_gate: Arc<tokio::sync::Semaphore>,
    blocking_operation_gate: Arc<tokio::sync::Semaphore>,
    body_limit: usize,
) -> Router {
    let state = ApiState {
        auth,
        workspace,
        policy,
        deploy,
        readiness,
        transport,
        discovery,
        cues,
        baselines,
        auth_verification_gate,
        blocking_operation_gate,
    };
    // Route registration must stay aligned with `HTTP_ROUTE_INVENTORY`.
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/ready", get(ready_handler))
        .route("/v1/admin/status", get(admin_status_handler))
        .route("/v1/config", get(config_handler))
        .route("/v1/doctor", get(doctor_handler))
        .route("/v1/workspace", get(workspace_handler))
        .route("/v1/search", get(search_handler))
        .route("/v1/tree", get(tree_root_handler))
        .route("/v1/tree/*path", get(tree_path_handler))
        .route("/v1/scripts", get(list_scripts_handler))
        .route("/v1/scripts/*script_id", get(script_path_handler))
        .route("/v1/envs", get(list_envs_handler).post(create_env_handler))
        .route("/v1/envs/active", delete(deactivate_env_handler))
        .route(
            "/v1/envs/:name",
            get(show_env_handler)
                .put(put_env_handler)
                .patch(patch_env_handler)
                .delete(delete_env_handler),
        )
        .route("/v1/envs/:name/activate", post(activate_env_handler))
        .route(
            "/v1/envs/:name/params/:key",
            put(set_env_param_handler).delete(delete_env_param_handler),
        )
        .route("/v1/runs", get(list_runs_handler).post(enqueue_run_handler))
        .route("/v1/runs/:run_id", get(show_run_handler))
        .route("/v1/runs/:run_id/traces", get(list_traces_handler))
        .route("/v1/runs/:run_id/cancel", post(cancel_run_handler))
        .route(
            "/v1/runs/:run_id/dead-letter",
            post(dead_letter_run_handler),
        )
        .route("/v1/queue/stats", get(queue_stats_handler))
        .route(
            "/v1/batteries",
            get(list_batteries_handler).post(add_battery_handler),
        )
        .route(
            "/v1/batteries/:battery_id",
            get(inspect_battery_handler).delete(remove_battery_handler),
        )
        .route(
            "/v1/batteries/:battery_id/scripts",
            get(list_battery_scripts_handler),
        )
        .route(
            "/v1/batteries/:battery_id/scripts/:script_id/install",
            post(install_battery_script_handler),
        )
        .route("/v1/batteries/:battery_id/sync", post(sync_battery_handler))
        .route("/v1/secrets", get(list_secrets_metadata_handler))
        .route("/v1/node/status", get(node_status_handler))
        .route("/v1/node/discovery", get(node_discovery_handler))
        .route("/v1/node/init", post(node_initialize_handler))
        .route("/v1/node/cues", post(node_cue_handler))
        .route("/v1/node/baselines", post(node_baseline_handler))
        .route(
            "/v1/node/baseline/rollback",
            post(node_baseline_rollback_handler),
        )
        .route(
            "/v1/node/peers",
            get(node_peers_handler).post(node_trust_handler),
        )
        .route(
            "/v1/node/enrollments",
            get(node_enrollments_handler).post(node_enrollment_stage_handler),
        )
        .route(
            "/v1/node/enrollments/:node_id/approve",
            post(node_enrollment_approve_handler),
        )
        .route(
            "/v1/node/enrollments/:node_id/reject",
            post(node_enrollment_reject_handler),
        )
        .route(
            "/v1/node/enrollment/bundle",
            post(node_signed_bundle_apply_handler).layer(axum::extract::DefaultBodyLimit::max(
                SIGNED_BUNDLE_HTTP_BODY_LIMIT_BYTES,
            )),
        )
        .route(
            "/v1/node/peers/:node_id/capabilities",
            axum::routing::patch(node_capabilities_handler),
        )
        .route("/v1/node/peers/:node_id/revoke", post(node_revoke_handler))
        .fallback(protected_not_found)
        .layer(axum::extract::DefaultBodyLimit::max(body_limit))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ))
        .with_state(state)
}

async fn protected_not_found() -> impl IntoResponse {
    error_response(StatusCode::NOT_FOUND, "not_found", "endpoint not found")
}

#[cfg(test)]
pub(super) fn shared_test_health_registry() -> Arc<NodeRegistry> {
    use crate::domain::NodeConfig;

    use std::sync::OnceLock;

    static REGISTRY: OnceLock<Arc<NodeRegistry>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            let temp = Box::leak(Box::new(tempfile::TempDir::new().expect("tempdir")));
            let context = crate::test_support::node_context(temp.path());
            node_ops::initialize_node_nonblocking(&context, &NodeConfig::default())
                .expect("initialize node");
            Arc::new(
                crate::operations::health::open_observational_registry(&context)
                    .expect("open observational registry"),
            )
        })
        .clone()
}

#[cfg(test)]
pub(super) fn router_with_health_plane(
    auth: Authenticator,
    workspace: Workspace,
    policy: ApiPolicy,
    deploy: DeployPolicy,
    readiness: Option<Arc<ReadinessGate>>,
    registry: Arc<NodeRegistry>,
    body_limit: usize,
) -> Router {
    let api = router_with_policy(
        auth.clone(),
        workspace,
        policy.clone(),
        deploy.clone(),
        readiness,
        body_limit,
    );
    let auth_gate = auth_verification_gate(&deploy);
    let health = health_plane_router(registry, auth, deploy, auth_gate, body_limit);
    api.nest("/v1/node", health)
}

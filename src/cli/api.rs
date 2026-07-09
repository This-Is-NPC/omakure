use crate::cli::args::ApiArgs;
use crate::cli::json;
use crate::operations::battery as battery_ops;
use crate::operations::config as config_ops;
use crate::operations::core;
use crate::operations::doctor as doctor_ops;
use crate::operations::envs as env_ops;
use crate::operations::scripts as scripts_ops;
use crate::operations::search as search_ops;
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::ports::ScriptRepository;
use crate::workspace::Workspace;
use axum::body::{to_bytes, Body};
use axum::extract::{Path as AxumPath, RawQuery, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use subtle::ConstantTimeEq;

const MIN_TOKEN_LEN: usize = 32;
const BODY_LIMIT_BYTES: usize = 1024 * 1024;
const MAX_SEARCH_QUERY_LEN: usize = 256;
const MAX_SEARCH_TAGS: usize = 16;
const MAX_SEARCH_TAG_LEN: usize = 64;

struct ApiState {
    token_digest: [u8; 32],
    workspace: Workspace,
    policy: ApiPolicy,
}

impl Clone for ApiState {
    fn clone(&self) -> Self {
        Self {
            token_digest: self.token_digest,
            workspace: self.workspace.clone_for_executor(),
            policy: self.policy.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiCapability {
    ConfigRead,
    ScriptsRead,
    EnvRead,
    EnvWrite,
    EnvActivate,
    EnvUse,
    SecretProviderUse,
    RunRead,
    RunWrite,
    BatteryRead,
    BatteryWrite,
}

#[derive(Debug, Clone)]
struct ApiPolicy {
    capabilities: Vec<ApiCapability>,
    allowed_secret_refs: Option<Vec<String>>,
}

impl ApiPolicy {
    fn all() -> Self {
        let mut policy = Self::allow([
            ApiCapability::ConfigRead,
            ApiCapability::ScriptsRead,
            ApiCapability::EnvRead,
            ApiCapability::EnvWrite,
            ApiCapability::EnvActivate,
            ApiCapability::EnvUse,
            ApiCapability::SecretProviderUse,
            ApiCapability::RunRead,
            ApiCapability::RunWrite,
            ApiCapability::BatteryRead,
            ApiCapability::BatteryWrite,
        ]);
        policy.allowed_secret_refs = None;
        policy
    }

    fn allow<const N: usize>(capabilities: [ApiCapability; N]) -> Self {
        Self {
            capabilities: capabilities.into(),
            allowed_secret_refs: Some(Vec::new()),
        }
    }

    fn from_config(capabilities: &[String], refs: &[String]) -> Result<Self, ApiConfigError> {
        let mut parsed = Vec::new();
        for capability in capabilities {
            if capability == "all" {
                return Ok(Self::all());
            }
            parsed.push(ApiCapability::from_config_value(capability)?);
        }
        Ok(Self {
            capabilities: parsed,
            allowed_secret_refs: Some(refs.to_vec()),
        })
    }

    #[cfg(test)]
    fn allow_with_secret_refs<const N: usize, const M: usize>(
        capabilities: [ApiCapability; N],
        refs: [&str; M],
    ) -> Self {
        Self {
            capabilities: capabilities.into(),
            allowed_secret_refs: Some(refs.into_iter().map(str::to_string).collect()),
        }
    }

    fn permits(&self, capability: ApiCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    fn secret_access(&self) -> crate::secrets::SecretAccess {
        match &self.allowed_secret_refs {
            None => crate::secrets::SecretAccess::allow_all(),
            Some(refs) => crate::secrets::SecretAccess::new(
                self.permits(ApiCapability::SecretProviderUse)
                    .then_some("secrets:use"),
                refs.iter().cloned(),
            ),
        }
    }
}

impl ApiCapability {
    fn from_config_value(value: &str) -> Result<Self, ApiConfigError> {
        match value {
            "config:read" => Ok(Self::ConfigRead),
            "scripts:read" => Ok(Self::ScriptsRead),
            "env:read" => Ok(Self::EnvRead),
            "env:write" => Ok(Self::EnvWrite),
            "env:activate" => Ok(Self::EnvActivate),
            "env:use" => Ok(Self::EnvUse),
            "secrets:use" => Ok(Self::SecretProviderUse),
            "runs:read" => Ok(Self::RunRead),
            "runs:write" => Ok(Self::RunWrite),
            "batteries:read" => Ok(Self::BatteryRead),
            "batteries:write" => Ok(Self::BatteryWrite),
            _ => Err(ApiConfigError::InvalidCapability(value.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ApiConfigError {
    MissingToken,
    InvalidToken,
    NonLoopbackBind(SocketAddr),
    InvalidCapability(String),
}

impl std::fmt::Display for ApiConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingToken => write!(f, "OMAKURE_API_TOKEN is required"),
            Self::InvalidToken => write!(f, "OMAKURE_API_TOKEN is invalid"),
            Self::NonLoopbackBind(addr) => write!(
                f,
                "refusing to bind {addr}; pass --allow-non-loopback to opt in"
            ),
            Self::InvalidCapability(value) => write!(f, "invalid API capability: {value}"),
        }
    }
}

impl Error for ApiConfigError {}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Deserialize)]
struct EnqueueRunBody {
    script: String,
    #[serde(default)]
    args: Vec<String>,
    env: Option<String>,
    #[serde(default)]
    secret_fields: HashMap<String, String>,
    run_id: Option<String>,
    #[serde(default = "default_actor")]
    actor: String,
    reason: Option<String>,
    #[serde(default)]
    priority: i64,
    timeout_ms: Option<i64>,
    parent_run_id: Option<String>,
    cron_schedule_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RunReasonBody {
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EnvBody {
    name: Option<String>,
    #[serde(default)]
    params: Vec<env_ops::EnvParam>,
}

#[derive(Debug, Deserialize)]
struct EnvParamBody {
    value: String,
}

#[derive(Debug, Deserialize)]
struct AddBatteryBody {
    name: String,
    git_url: String,
    #[serde(default = "default_battery_ref")]
    requested_ref: String,
}

#[derive(Debug, Default, Deserialize)]
struct InstallBatteryScriptBody {
    #[serde(default)]
    force: bool,
}

fn default_actor() -> String {
    "human".to_string()
}

fn default_battery_ref() -> String {
    "main".to_string()
}

pub fn run(scripts_dir: PathBuf, args: ApiArgs) -> Result<(), Box<dyn Error>> {
    validate_bind(args.bind, args.allow_non_loopback)?;
    let token = token_from_env()?;
    let policy = ApiPolicy::from_config(&args.capabilities, &args.secret_refs)?;
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(args.bind).await?;
        axum::serve(listener, router_with_policy(token, workspace, policy)).await?;
        Ok::<(), Box<dyn Error>>(())
    })
}

#[cfg(test)]
fn router(token: String, workspace: Workspace) -> Router {
    router_with_policy(token, workspace, ApiPolicy::all())
}

/// Canonical `(method, path)` inventory for the HTTP management API.
///
/// Keep this list in lockstep with `router_with_policy`. Black-box E2E tests
/// parse the markers below so route drift fails the suite without importing
/// the binary crate as a library.
// OMAKURE_HTTP_ROUTE_INVENTORY_START
#[allow(dead_code)] // consumed by black-box E2E via source markers; kept as router source of truth
pub const HTTP_ROUTE_INVENTORY: &[(&str, &str)] = &[
    ("GET", "/v1/health"),
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
];
// OMAKURE_HTTP_ROUTE_INVENTORY_END

fn router_with_policy(token: String, workspace: Workspace, policy: ApiPolicy) -> Router {
    let state = ApiState {
        token_digest: token_digest_for(&token),
        workspace,
        policy,
    };
    // Route registration must stay aligned with `HTTP_ROUTE_INVENTORY`.
    Router::new()
        .route("/v1/health", get(health))
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
        .fallback(protected_not_found)
        .layer(axum::extract::DefaultBodyLimit::max(BODY_LIMIT_BYTES))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ))
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(json::ok_envelope(HealthResponse { status: "ok" }))
}

async fn workspace_handler(State(state): State<ApiState>) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::ConfigRead) {
        return response;
    }
    operation_response(core::workspace_summary(&state.workspace))
}

async fn config_handler(State(state): State<ApiState>) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::ConfigRead) {
        return response;
    }
    operation_response(config_ops::redacted_config_summary(&state.workspace))
}

async fn doctor_handler(State(state): State<ApiState>) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::ConfigRead) {
        return response;
    }
    operation_response(doctor_ops::doctor_report(&state.workspace))
}

async fn search_handler(State(state): State<ApiState>, RawQuery(raw_query): RawQuery) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::ScriptsRead) {
        return response;
    }
    let request = query_pairs(raw_query.as_deref()).and_then(|pairs| {
        let query = query_value(&pairs, "q")
            .or_else(|| query_value(&pairs, "query"))
            .ok_or_else(|| {
                OperationError::new(
                    OperationErrorCode::InvalidInput,
                    "q query parameter is required",
                )
            })?;
        if query.trim().is_empty() {
            return Err(OperationError::new(
                OperationErrorCode::InvalidInput,
                "q query parameter must not be empty",
            ));
        }
        if query.len() > MAX_SEARCH_QUERY_LEN {
            return Err(OperationError::new(
                OperationErrorCode::InvalidInput,
                "q query parameter is too long",
            ));
        }
        let tags = query_values(&pairs, "tag");
        if tags.len() > MAX_SEARCH_TAGS || tags.iter().any(|tag| tag.len() > MAX_SEARCH_TAG_LEN) {
            return Err(OperationError::new(
                OperationErrorCode::InvalidInput,
                "tag query parameters exceed limits",
            ));
        }
        Ok(search_ops::SearchScriptsRequest {
            query,
            tags,
            refresh: false,
        })
    });
    operation_response(
        request.and_then(|request| search_ops::search_scripts(&state.workspace, request)),
    )
}

async fn list_scripts_handler(
    State(state): State<ApiState>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::ScriptsRead) {
        return response;
    }
    let request = query_pairs(raw_query.as_deref()).map(|pairs| core::ListScriptsRequest {
        tags: query_values(&pairs, "tag"),
    });
    operation_response(request.and_then(|request| core::list_scripts(&state.workspace, request)))
}

async fn describe_script_handler(State(state): State<ApiState>, script_id: String) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(core::describe_script(
        &state.workspace,
        core::DescribeScriptRequest { script: script_id },
    ))
}

async fn script_schema_handler(State(state): State<ApiState>, script_id: String) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(
        core::describe_script(
            &state.workspace,
            core::DescribeScriptRequest { script: script_id },
        )
        .map(|description| description.schema),
    )
}

async fn script_path_handler(
    State(state): State<ApiState>,
    AxumPath(script_id): AxumPath<String>,
) -> Response {
    if let Some(script_id) = script_id.strip_suffix("/content") {
        if !script_id.is_empty() {
            return script_content_handler(State(state), script_id.to_string()).await;
        }
    }
    match script_id.strip_suffix("/schema") {
        Some(script_id) if !script_id.is_empty() => {
            script_schema_handler(State(state), script_id.to_string()).await
        }
        _ => describe_script_handler(State(state), script_id).await,
    }
}

async fn script_content_handler(State(state): State<ApiState>, script_id: String) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(scripts_ops::read_script_content(
        &state.workspace,
        scripts_ops::ReadScriptContentRequest { script: script_id },
    ))
}

async fn tree_root_handler(State(state): State<ApiState>) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(scripts_ops::list_tree(
        &state.workspace,
        scripts_ops::ListTreeRequest { path: None },
    ))
}

async fn tree_path_handler(
    State(state): State<ApiState>,
    AxumPath(path): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(scripts_ops::list_tree(
        &state.workspace,
        scripts_ops::ListTreeRequest { path: Some(path) },
    ))
}

async fn list_envs_handler(State(state): State<ApiState>) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::EnvRead) {
        return response;
    }
    operation_response(env_ops::list_envs(&state.workspace))
}

async fn create_env_handler(State(state): State<ApiState>, body: Body) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::EnvWrite) {
        return response;
    }
    let body = match parse_json_body::<EnvBody>(body).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    let Some(name) = body.name else {
        return operation_error_response(OperationError::new(
            OperationErrorCode::InvalidInput,
            "name is required",
        ));
    };
    operation_response(env_ops::create_env(&state.workspace, &name, &body.params))
}

async fn show_env_handler(
    State(state): State<ApiState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::EnvRead) {
        return response;
    }
    operation_response(env_ops::show_env(&state.workspace, &name))
}

async fn put_env_handler(
    State(state): State<ApiState>,
    AxumPath(name): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::EnvWrite) {
        return response;
    }
    let body = match parse_json_body::<EnvBody>(body).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    let result = match env_ops::replace_env(&state.workspace, &name, &body.params) {
        Err(err) if err.code == OperationErrorCode::NotFound => {
            env_ops::create_env(&state.workspace, &name, &body.params)
        }
        other => other,
    };
    operation_response(result)
}

async fn patch_env_handler(
    State(state): State<ApiState>,
    AxumPath(name): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::EnvWrite) {
        return response;
    }
    let body = match parse_json_body::<EnvBody>(body).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    for param in body.params {
        if let Err(err) = env_ops::set_param(&state.workspace, &name, &param.key, &param.value) {
            return operation_error_response(err);
        }
    }
    operation_response(Ok(()))
}

async fn delete_env_handler(
    State(state): State<ApiState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::EnvWrite) {
        return response;
    }
    operation_response(env_ops::delete_env(&state.workspace, &name))
}

async fn set_env_param_handler(
    State(state): State<ApiState>,
    AxumPath((name, key)): AxumPath<(String, String)>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::EnvWrite) {
        return response;
    }
    let body = match parse_json_body::<EnvParamBody>(body).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    operation_response(env_ops::set_param(
        &state.workspace,
        &name,
        &key,
        &body.value,
    ))
}

async fn delete_env_param_handler(
    State(state): State<ApiState>,
    AxumPath((name, key)): AxumPath<(String, String)>,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::EnvWrite) {
        return response;
    }
    operation_response(env_ops::remove_param(&state.workspace, &name, &key))
}

async fn activate_env_handler(
    State(state): State<ApiState>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::EnvActivate) {
        return response;
    }
    operation_response(env_ops::activate_env(&state.workspace, &name))
}

async fn deactivate_env_handler(State(state): State<ApiState>) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::EnvActivate) {
        return response;
    }
    operation_response(env_ops::deactivate_env(&state.workspace))
}

async fn list_runs_handler(
    State(state): State<ApiState>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::RunRead) {
        return response;
    }
    let request = list_runs_request(raw_query.as_deref());
    operation_response(request.and_then(|request| core::list_runs(&state.workspace, request)))
}

async fn show_run_handler(
    State(state): State<ApiState>,
    AxumPath(run_id): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::RunRead) {
        return response;
    }
    operation_response(core::show_run(
        &state.workspace,
        core::ShowRunRequest { run_id },
    ))
}

async fn list_traces_handler(
    State(state): State<ApiState>,
    AxumPath(run_id): AxumPath<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::RunRead) {
        return response;
    }
    let request = list_traces_request(run_id, raw_query.as_deref());
    operation_response(request.and_then(|request| core::list_traces(&state.workspace, request)))
}

async fn queue_stats_handler(State(state): State<ApiState>) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::RunRead) {
        return response;
    }
    operation_response(core::queue_stats(&state.workspace))
}

async fn enqueue_run_handler(State(state): State<ApiState>, body: Body) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::RunWrite) {
        return response;
    }
    let body = match parse_json_body::<EnqueueRunBody>(body).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    if body.env.is_some() {
        if let Some(response) = require_capability(&state, ApiCapability::EnvUse) {
            return response;
        }
    }
    if !body.secret_fields.is_empty() || args_use_secret_provider(&body.args) {
        if let Some(response) = require_capability(&state, ApiCapability::SecretProviderUse) {
            return response;
        }
    }
    if let Some(response) = require_implicit_secret_capabilities(&state, &body) {
        return response;
    }
    if body
        .secret_fields
        .values()
        .any(|value| !value.starts_with("secret://"))
    {
        return operation_error_response(OperationError::new(
            OperationErrorCode::InvalidInput,
            "queued HTTP secret_fields must use secret:// refs so workers can resolve them without persisting plaintext",
        ));
    }
    operation_response(core::enqueue_run_with_access(
        &state.workspace,
        core::EnqueueRunRequest {
            script: body.script,
            args: body.args,
            env: body.env,
            secret_fields: body.secret_fields.into_iter().collect(),
            run_id: body.run_id,
            actor: body.actor,
            reason: body.reason,
            priority: body.priority,
            timeout_ms: body.timeout_ms,
            parent_run_id: body.parent_run_id,
            cron_schedule_id: body.cron_schedule_id,
        },
        &state.policy.secret_access(),
    ))
}

fn require_implicit_secret_capabilities(
    state: &ApiState,
    body: &EnqueueRunBody,
) -> Option<Response> {
    let description = match core::describe_script(
        &state.workspace,
        core::DescribeScriptRequest {
            script: body.script.clone(),
        },
    ) {
        Ok(description) => description,
        Err(err) => return Some(operation_error_response(err)),
    };
    let repo = crate::adapters::workspace_repository::FsWorkspaceRepository::new(
        state.workspace.scripts_root().to_path_buf(),
    );
    let schema = match repo.read_schema(std::path::Path::new(&description.absolute_path)) {
        Ok(schema) => schema,
        Err(err) => {
            return Some(operation_error_response(OperationError::new(
                OperationErrorCode::InvalidInput,
                err.to_string(),
            )))
        }
    };
    let secret_fields: Vec<_> = schema
        .fields
        .iter()
        .filter(|field| field.is_secret())
        .collect();
    if secret_fields.is_empty() {
        return None;
    }

    if secret_fields.iter().any(|field| field.default.is_some()) {
        if let Some(response) = require_capability(state, ApiCapability::SecretProviderUse) {
            return Some(response);
        }
    }

    let env_file = match body
        .env
        .as_deref()
        .map(|name| crate::operations::envs::env_file_path(&state.workspace, name))
        .transpose()
    {
        Ok(path) => path,
        Err(err) => return Some(operation_error_response(err)),
    };
    let run_env = match crate::adapters::environments::resolve_run_env(
        state.workspace.envs_dir(),
        env_file.as_deref(),
    ) {
        Ok(run_env) => run_env,
        Err(err) => {
            return Some(operation_error_response(OperationError::new(
                OperationErrorCode::InvalidInput,
                err.to_string(),
            )))
        }
    };
    if secret_fields.iter().any(|field| {
        run_env
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case(&field.name))
    }) {
        if let Some(response) = require_capability(state, ApiCapability::EnvUse) {
            return Some(response);
        }
    }

    None
}

fn args_use_secret_provider(args: &[String]) -> bool {
    args.iter().any(|arg| {
        arg.starts_with("secret://")
            || arg
                .split_once('=')
                .map(|(_, value)| value.starts_with("secret://"))
                .unwrap_or(false)
    })
}

async fn cancel_run_handler(
    State(state): State<ApiState>,
    AxumPath(run_id): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::RunWrite) {
        return response;
    }
    let body = match parse_json_body::<RunReasonBody>(body).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    operation_response(core::cancel_run(
        &state.workspace,
        core::CancelRunRequest {
            run_id,
            reason: body.reason,
        },
    ))
}

async fn dead_letter_run_handler(
    State(state): State<ApiState>,
    AxumPath(run_id): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::RunWrite) {
        return response;
    }
    let body = match parse_json_body::<RunReasonBody>(body).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    operation_response(core::dead_letter_run(
        &state.workspace,
        core::DeadLetterRunRequest {
            run_id,
            reason: body.reason,
        },
    ))
}

async fn list_batteries_handler(State(state): State<ApiState>) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::BatteryRead) {
        return response;
    }
    operation_response(battery_ops::list_batteries(&state.workspace))
}

async fn add_battery_handler(State(state): State<ApiState>, body: Body) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::BatteryWrite) {
        return response;
    }
    let body = match parse_json_body::<AddBatteryBody>(body).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    if !body
        .git_url
        .trim()
        .to_ascii_lowercase()
        .starts_with("https://")
    {
        return operation_error_response(OperationError::new(
            OperationErrorCode::InvalidInput,
            "HTTP API battery registration only accepts https git URLs",
        ));
    }
    operation_response(battery_ops::add_battery(
        &state.workspace,
        battery_ops::AddBatteryRequest {
            name: body.name,
            git_url: body.git_url,
            requested_ref: body.requested_ref,
        },
    ))
}

async fn sync_battery_handler(
    State(state): State<ApiState>,
    AxumPath(battery_id): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::BatteryWrite) {
        return response;
    }
    let request = require_https_battery_source(&state.workspace, &battery_id)
        .map(|_| battery_ops::SyncBatteryRequest { name: battery_id });
    operation_response(
        request.and_then(|request| battery_ops::sync_battery_https_only(&state.workspace, request)),
    )
}

async fn inspect_battery_handler(
    State(state): State<ApiState>,
    AxumPath(battery_id): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::BatteryRead) {
        return response;
    }
    let request = require_https_battery_source(&state.workspace, &battery_id)
        .map(|_| battery_ops::InspectBatteryRequest { name: battery_id });
    operation_response(
        request.and_then(|request| battery_ops::inspect_battery(&state.workspace, request)),
    )
}

async fn list_battery_scripts_handler(
    State(state): State<ApiState>,
    AxumPath(battery_id): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::BatteryRead) {
        return response;
    }
    let request = require_https_battery_source(&state.workspace, &battery_id)
        .map(|_| battery_ops::InspectBatteryRequest { name: battery_id });
    operation_response(
        request.and_then(|request| battery_ops::list_battery_scripts(&state.workspace, request)),
    )
}

async fn install_battery_script_handler(
    State(state): State<ApiState>,
    AxumPath((battery_id, script_id)): AxumPath<(String, String)>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::BatteryWrite) {
        return response;
    }
    let body = match parse_json_body::<InstallBatteryScriptBody>(body).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    let request = require_https_battery_source(&state.workspace, &battery_id).map(|_| {
        battery_ops::InstallBatteryScriptRequest {
            battery_name: battery_id,
            script_id,
            force: body.force,
        }
    });
    operation_response(
        request.and_then(|request| battery_ops::install_battery_script(&state.workspace, request)),
    )
}

async fn remove_battery_handler(
    State(state): State<ApiState>,
    AxumPath(battery_id): AxumPath<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_capability(&state, ApiCapability::BatteryWrite) {
        return response;
    }
    let request = query_pairs(raw_query.as_deref()).and_then(|pairs| {
        query_bool(&pairs, "remove_cache").map(|remove_cache| battery_ops::RemoveBatteryRequest {
            name: battery_id,
            remove_cache: remove_cache.unwrap_or(false),
        })
    });
    operation_response(
        request.and_then(|request| battery_ops::remove_battery(&state.workspace, request)),
    )
}

async fn protected_not_found() -> impl IntoResponse {
    error_response(StatusCode::NOT_FOUND, "not_found", "endpoint not found")
}

async fn require_bearer(
    State(state): State<ApiState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    if request.uri().path() == "/v1/health" {
        return next.run(request).await;
    }

    match headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
    {
        Some(value) if valid_authorization(value, &state.token_digest) => next.run(request).await,
        _ => error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "bearer token required",
        ),
    }
}

fn valid_authorization(value: &str, token_digest: &[u8; 32]) -> bool {
    value
        .strip_prefix("Bearer ")
        .is_some_and(|candidate| token_digest_for(candidate).ct_eq(token_digest).into())
}

fn token_digest_for(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

async fn parse_json_body<T: for<'de> Deserialize<'de>>(body: Body) -> OperationResult<T> {
    let bytes = to_bytes(body, BODY_LIMIT_BYTES).await.map_err(|err| {
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

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json::err_envelope(code, message))).into_response()
}

fn require_capability(state: &ApiState, capability: ApiCapability) -> Option<Response> {
    (!state.policy.permits(capability)).then(|| {
        error_response(
            StatusCode::FORBIDDEN,
            "forbidden",
            "token is not permitted for this operation",
        )
    })
}

fn operation_response<T: Serialize>(result: OperationResult<T>) -> Response {
    match result {
        Ok(data) => (StatusCode::OK, Json(json::ok_envelope(data))).into_response(),
        Err(err) => operation_error_response(err),
    }
}

fn operation_error_response(err: OperationError) -> Response {
    let status = match err.code {
        OperationErrorCode::InvalidInput
        | OperationErrorCode::UnsafePath
        | OperationErrorCode::ManifestInvalid => StatusCode::BAD_REQUEST,
        OperationErrorCode::Forbidden => StatusCode::FORBIDDEN,
        OperationErrorCode::NotFound => StatusCode::NOT_FOUND,
        OperationErrorCode::AlreadyExists
        | OperationErrorCode::Conflict
        | OperationErrorCode::NotSynced => StatusCode::CONFLICT,
        OperationErrorCode::UnsupportedScript => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        OperationErrorCode::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        OperationErrorCode::GitFailed
        | OperationErrorCode::IoFailed
        | OperationErrorCode::RegistryInvalid => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response(status, err.code.as_str(), &err.message)
}

fn require_https_battery_source(workspace: &Workspace, battery_id: &str) -> OperationResult<()> {
    let battery = battery_ops::list_batteries(workspace)?
        .into_iter()
        .find(|battery| battery.name == battery_id)
        .ok_or_else(|| {
            OperationError::new(
                OperationErrorCode::NotFound,
                format!("battery '{battery_id}' was not found"),
            )
        })?;
    if battery
        .git_url
        .trim()
        .to_ascii_lowercase()
        .starts_with("https://")
    {
        Ok(())
    } else {
        Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "HTTP API can only operate on Batteries registered with https git URLs",
        ))
    }
}

fn list_runs_request(raw_query: Option<&str>) -> OperationResult<core::ListRunsRequest> {
    let pairs = query_pairs(raw_query)?;
    Ok(core::ListRunsRequest {
        script: query_value(&pairs, "script"),
        actor: query_value(&pairs, "actor"),
        since_ms: query_i64(&pairs, "since_ms")?,
        until_ms: query_i64(&pairs, "until_ms")?,
        success: query_bool(&pairs, "success")?,
        limit: query_i64(&pairs, "limit")?,
        states: query_values(&pairs, "state"),
        state_set: query_value(&pairs, "state_set"),
    })
}

fn list_traces_request(
    run_id: String,
    raw_query: Option<&str>,
) -> OperationResult<core::ListTracesRequest> {
    let pairs = query_pairs(raw_query)?;
    Ok(core::ListTracesRequest {
        run_id,
        level: query_value(&pairs, "level"),
        since_sequence: query_i64(&pairs, "since_sequence")?,
    })
}

fn query_pairs(raw_query: Option<&str>) -> OperationResult<Vec<(String, String)>> {
    match raw_query {
        Some(query) => serde_urlencoded::from_str(query).map_err(|err| {
            OperationError::new(
                OperationErrorCode::InvalidInput,
                format!("invalid query string: {err}"),
            )
        }),
        None => Ok(Vec::new()),
    }
}

fn query_value(pairs: &[(String, String)], key: &str) -> Option<String> {
    query_values(pairs, key).into_iter().next()
}

fn query_values(pairs: &[(String, String)], key: &str) -> Vec<String> {
    pairs
        .iter()
        .filter(|(name, _)| *name == key)
        .map(|(_, value)| value.clone())
        .collect()
}

fn query_i64(pairs: &[(String, String)], key: &str) -> OperationResult<Option<i64>> {
    query_value(pairs, key)
        .map(|value| {
            value.parse::<i64>().map_err(|_| {
                OperationError::new(
                    OperationErrorCode::InvalidInput,
                    format!("invalid integer query parameter: {key}"),
                )
            })
        })
        .transpose()
}

fn query_bool(pairs: &[(String, String)], key: &str) -> OperationResult<Option<bool>> {
    query_value(pairs, key)
        .map(|value| {
            value.parse::<bool>().map_err(|_| {
                OperationError::new(
                    OperationErrorCode::InvalidInput,
                    format!("invalid boolean query parameter: {key}"),
                )
            })
        })
        .transpose()
}

fn token_from_env() -> Result<String, ApiConfigError> {
    let token = env::var("OMAKURE_API_TOKEN").map_err(|_| ApiConfigError::MissingToken)?;
    validate_token(&token)?;
    Ok(token.trim().to_string())
}

fn validate_token(token: &str) -> Result<(), ApiConfigError> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(ApiConfigError::MissingToken);
    }

    let lower = trimmed.to_ascii_lowercase();
    let known_defaults = [
        "changeme",
        "change-me",
        "default",
        "password",
        "secret",
        "token",
        "omakure",
    ];
    if trimmed.len() < MIN_TOKEN_LEN || known_defaults.contains(&lower.as_str()) {
        return Err(ApiConfigError::InvalidToken);
    }

    Ok(())
}

fn validate_bind(addr: SocketAddr, allow_non_loopback: bool) -> Result<(), ApiConfigError> {
    if allow_non_loopback || addr.ip().is_loopback() {
        return Ok(());
    }

    Err(ApiConfigError::NonLoopbackBind(addr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_meta;
    use crate::runs::{self, EnqueueOptions, RunCompletion};
    use axum::body::to_bytes;
    use axum::http::Method;
    use std::process::Command;
    use tempfile::TempDir;
    use tower::ServiceExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    async fn response_json(response: Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), BODY_LIMIT_BYTES)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn workspace_in(dir: &TempDir) -> Workspace {
        let workspace = Workspace::new(dir.path().to_path_buf());
        workspace.ensure_layout().unwrap();
        workspace
    }

    fn write_script(root: &std::path::Path, name: &str) {
        if let Some(parent) = root.join(name).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            root.join(name),
            format!(
                "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {{\"Name\":\"{name}\",\"Description\":\"test script\",\"Tags\":[\"ops\"],\"Fields\":[]}}\n# OMAKURE_SCHEMA_END\necho ok\n"
            ),
        )
        .unwrap();
    }

    fn write_secret_script(root: &std::path::Path, name: &str, default: Option<&str>) {
        let default_line = default
            .map(|value| format!(r#", "Default":"{value}""#))
            .unwrap_or_default();
        std::fs::write(
            root.join(name),
            format!(
                r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {{"Name":"Secret","Fields":[{{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"{default_line}}}]}}
# OMAKURE_SCHEMA_END
echo ok
"#
            ),
        )
        .unwrap();
    }

    fn authed_request(uri: &str) -> Request<Body> {
        Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap()
    }

    fn authed_json_request(uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn authed_json_method_request(method: Method, uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn authed_delete_request(uri: &str) -> Request<Body> {
        Request::builder()
            .method(Method::DELETE)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .body(Body::empty())
            .unwrap()
    }

    fn run_git(args: &[&str], cwd: &std::path::Path) {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn invalid_manifest_repo() -> TempDir {
        let repo = TempDir::new().unwrap();
        run_git(&["init", "-b", "main"], repo.path());
        std::fs::write(repo.path().join("omakure-battery.toml"), "not = [valid").unwrap();
        run_git(&["add", "."], repo.path());
        run_git(
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test User",
                "commit",
                "-m",
                "invalid manifest",
            ],
            repo.path(),
        );
        repo
    }

    fn register_invalid_https_battery_cache(workspace: &Workspace, name: &str) {
        let paths = battery_ops::BatteryPaths::for_workspace(workspace);
        let cache = paths.cache_path_for(name);
        std::fs::create_dir_all(&cache).unwrap();
        run_git(&["init", "-b", "main"], &cache);
        std::fs::write(cache.join("omakure-battery.toml"), "not = [valid").unwrap();
        run_git(&["add", "."], &cache);
        run_git(
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test User",
                "commit",
                "-m",
                "invalid manifest",
            ],
            &cache,
        );
        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&cache)
            .output()
            .unwrap();
        assert!(head.status.success());
        let registry = battery_ops::BatteryRegistry {
            version: battery_ops::REGISTRY_VERSION,
            batteries: vec![battery_ops::BatterySummary {
                name: name.to_string(),
                git_url: format!("https://example.invalid/{name}.git"),
                requested_ref: "main".into(),
                resolved_commit: Some(String::from_utf8_lossy(&head.stdout).trim().to_string()),
                cache_path: paths
                    .cache_path_for(name)
                    .strip_prefix(workspace.root())
                    .unwrap()
                    .to_path_buf(),
                last_synced_at: Some("2026-07-07T00:00:00Z".into()),
            }],
        };
        std::fs::write(
            &paths.registry_path,
            serde_json::to_string_pretty(&registry).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn http_route_inventory_is_non_empty_and_unique() {
        assert!(!HTTP_ROUTE_INVENTORY.is_empty());
        let mut seen = std::collections::BTreeSet::new();
        for entry in HTTP_ROUTE_INVENTORY {
            assert!(
                seen.insert(*entry),
                "duplicate HTTP route inventory entry: {entry:?}"
            );
        }
    }

    #[test]
    fn bind_guard_allows_loopback_by_default() {
        let addr: SocketAddr = "127.0.0.1:7878".parse().unwrap();
        assert!(validate_bind(addr, false).is_ok());
    }

    #[test]
    fn bind_guard_rejects_non_loopback_without_opt_in() {
        let addr: SocketAddr = "0.0.0.0:7878".parse().unwrap();
        assert_eq!(
            validate_bind(addr, false),
            Err(ApiConfigError::NonLoopbackBind(addr))
        );
    }

    #[test]
    fn bind_guard_allows_non_loopback_with_opt_in() {
        let addr: SocketAddr = "0.0.0.0:7878".parse().unwrap();
        assert!(validate_bind(addr, true).is_ok());
    }

    #[test]
    fn token_validation_rejects_empty_short_and_defaults() {
        assert_eq!(validate_token(""), Err(ApiConfigError::MissingToken));
        assert_eq!(validate_token("short"), Err(ApiConfigError::InvalidToken));
        assert_eq!(
            validate_token("changeme"),
            Err(ApiConfigError::InvalidToken)
        );
    }

    #[test]
    fn token_validation_accepts_long_token() {
        assert!(validate_token(TOKEN).is_ok());
    }

    #[tokio::test]
    async fn health_works_without_token() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let response = router(TOKEN.to_string(), workspace)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["data"]["status"], "ok");
    }

    #[tokio::test]
    async fn protected_route_rejects_missing_token() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let response = router(TOKEN.to_string(), workspace)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/scripts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = response_json(response).await;
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn protected_route_rejects_invalid_token() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let response = router(TOKEN.to_string(), workspace)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/scripts")
                    .header(header::AUTHORIZATION, "Bearer wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn protected_route_accepts_valid_token() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/unknown"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn read_endpoints_require_explicit_read_capabilities() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let app = router_with_policy(
            TOKEN.to_string(),
            workspace,
            ApiPolicy::allow([ApiCapability::RunWrite]),
        );

        for uri in [
            "/v1/config",
            "/v1/workspace",
            "/v1/doctor",
            "/v1/scripts",
            "/v1/tree",
            "/v1/runs",
            "/v1/queue/stats",
            "/v1/batteries",
        ] {
            let response = app.clone().oneshot(authed_request(uri)).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "uri: {uri}");
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "forbidden");
        }
    }

    #[tokio::test]
    async fn read_endpoints_accept_matching_read_capabilities() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let app = router_with_policy(
            TOKEN.to_string(),
            workspace,
            ApiPolicy::allow([
                ApiCapability::ConfigRead,
                ApiCapability::ScriptsRead,
                ApiCapability::RunRead,
                ApiCapability::BatteryRead,
            ]),
        );

        for uri in [
            "/v1/config",
            "/v1/workspace",
            "/v1/doctor",
            "/v1/scripts",
            "/v1/tree",
            "/v1/runs",
            "/v1/queue/stats",
            "/v1/batteries",
        ] {
            let response = app.clone().oneshot(authed_request(uri)).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "uri: {uri}");
        }
    }

    #[tokio::test]
    async fn workspace_endpoint_returns_summary() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/workspace"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["data"]["version"], app_meta::APP_VERSION);
    }

    #[tokio::test]
    async fn config_endpoint_returns_full_masked_config() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(workspace.envs_dir().join("dev.conf"), "HOST=localhost\n").unwrap();
        std::fs::write(workspace.envs_active_path(), "dev.conf\n").unwrap();

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/config"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["data"]["version"], app_meta::APP_VERSION);
        assert_eq!(body["data"]["active_env"], "dev.conf");
        assert_eq!(body["data"]["active_env_keys"][0]["key"], "HOST");
        assert_eq!(body["data"]["active_env_keys"][0]["value"], "****");
        assert!(!body.to_string().contains("localhost"));
    }

    #[tokio::test]
    async fn doctor_endpoint_returns_structured_report() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/doctor"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["ok"], true);
        assert!(body["data"]["dependencies"].is_array());
        assert!(body["data"]["workspace_paths"].is_array());
        assert_eq!(body["data"]["schemas"]["total"], 1);
    }

    #[tokio::test]
    async fn scripts_and_schema_endpoints_return_operation_data() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");

        let app = router(TOKEN.to_string(), workspace);
        let list = app
            .clone()
            .oneshot(authed_request("/v1/scripts?tag=ops"))
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_body = response_json(list).await;
        assert_eq!(list_body["data"][0]["relative_path"], "job.sh");

        let show = app
            .clone()
            .oneshot(authed_request("/v1/scripts/job.sh"))
            .await
            .unwrap();
        assert_eq!(show.status(), StatusCode::OK);
        let show_body = response_json(show).await;
        assert_eq!(show_body["data"]["schema"]["name"], "job.sh");

        let schema = app
            .oneshot(authed_request("/v1/scripts/job.sh/schema"))
            .await
            .unwrap();
        assert_eq!(schema.status(), StatusCode::OK);
        let schema_body = response_json(schema).await;
        assert_eq!(schema_body["data"]["tags"][0], "ops");
    }

    #[tokio::test]
    async fn search_endpoint_returns_operation_data() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "deploy.sh");
        search_ops::search_scripts(
            &workspace,
            search_ops::SearchScriptsRequest {
                query: "deploy".into(),
                tags: vec!["ops".into()],
                refresh: true,
            },
        )
        .unwrap();

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/search?q=deploy&tag=ops"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["data"][0]["relative_path"], "deploy.sh");
    }

    #[tokio::test]
    async fn search_endpoint_requires_query() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/search"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "invalid_input");
    }

    #[tokio::test]
    async fn search_endpoint_rejects_empty_and_oversized_query() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let app = router(TOKEN.to_string(), workspace);

        let empty = app
            .clone()
            .oneshot(authed_request("/v1/search?q="))
            .await
            .unwrap();
        assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

        let too_long = "x".repeat(MAX_SEARCH_QUERY_LEN + 1);
        let long = app
            .oneshot(authed_request(&format!("/v1/search?q={too_long}")))
            .await
            .unwrap();
        assert_eq!(long.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn search_endpoint_rejects_excessive_tags() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let app = router(TOKEN.to_string(), workspace);

        let too_many_tags = (0..=MAX_SEARCH_TAGS)
            .map(|idx| format!("tag=t{idx}"))
            .collect::<Vec<_>>()
            .join("&");
        let too_many = app
            .clone()
            .oneshot(authed_request(&format!(
                "/v1/search?q=deploy&{too_many_tags}"
            )))
            .await
            .unwrap();
        assert_eq!(too_many.status(), StatusCode::BAD_REQUEST);

        let too_long_tag = "x".repeat(MAX_SEARCH_TAG_LEN + 1);
        let long = app
            .oneshot(authed_request(&format!(
                "/v1/search?q=deploy&tag={too_long_tag}"
            )))
            .await
            .unwrap();
        assert_eq!(long.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn script_routes_support_nested_paths() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "tools/job.sh");
        std::fs::write(
            workspace.scripts_root().join("tools/secret.sh"),
            r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Default":"schema_secret_default","Arg":"--token"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
        )
        .unwrap();

        let app = router(TOKEN.to_string(), workspace);
        let show = app
            .clone()
            .oneshot(authed_request("/v1/scripts/tools/job.sh"))
            .await
            .unwrap();
        assert_eq!(show.status(), StatusCode::OK);
        let show_body = response_json(show).await;
        assert_eq!(show_body["data"]["relative_path"], "tools/job.sh");

        let schema = app
            .clone()
            .oneshot(authed_request("/v1/scripts/tools/job.sh/schema"))
            .await
            .unwrap();
        assert_eq!(schema.status(), StatusCode::OK);
        let schema_body = response_json(schema).await;
        assert_eq!(schema_body["data"]["name"], "tools/job.sh");

        let secret_schema = app
            .oneshot(authed_request("/v1/scripts/tools/secret.sh/schema"))
            .await
            .unwrap();
        assert_eq!(secret_schema.status(), StatusCode::OK);
        let secret_body = response_json(secret_schema).await;
        assert!(!secret_body.to_string().contains("schema_secret_default"));
    }

    #[tokio::test]
    async fn tree_and_content_endpoints_return_safe_browsing_data() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "tools/job.sh");

        let app = router(TOKEN.to_string(), workspace);
        let tree = app
            .clone()
            .oneshot(authed_request("/v1/tree"))
            .await
            .unwrap();
        assert_eq!(tree.status(), StatusCode::OK);
        let tree_body = response_json(tree).await;
        assert_eq!(tree_body["data"][0]["kind"], "directory");
        assert_eq!(tree_body["data"][0]["relative_path"], "tools");

        let nested = app
            .clone()
            .oneshot(authed_request("/v1/tree/tools"))
            .await
            .unwrap();
        assert_eq!(nested.status(), StatusCode::OK);
        let nested_body = response_json(nested).await;
        assert_eq!(nested_body["data"][0]["relative_path"], "tools/job.sh");

        let content = app
            .oneshot(authed_request("/v1/scripts/tools/job.sh/content"))
            .await
            .unwrap();
        assert_eq!(content.status(), StatusCode::OK);
        let content_body = response_json(content).await;
        assert_eq!(content_body["data"]["relative_path"], "tools/job.sh");
        assert!(content_body["data"]["content"]
            .as_str()
            .unwrap()
            .contains("echo ok"));
    }

    #[tokio::test]
    async fn content_endpoint_rejects_path_traversal() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/scripts/../secret.sh/content"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "unsafe_path");
    }

    #[tokio::test]
    async fn content_endpoint_rejects_absolute_encoded_path() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/scripts/%2Ftmp%2Fsecret.sh/content"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "unsafe_path");
    }

    #[tokio::test]
    async fn tree_and_content_endpoints_reject_metadata_paths() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let app = router(TOKEN.to_string(), workspace);

        for uri in [
            "/v1/tree/.omakure",
            "/v1/tree/.history",
            "/v1/tree/.git",
            "/v1/scripts/.omakure/secret.sh/content",
            "/v1/scripts/.history/secret.sh/content",
            "/v1/scripts/.git/secret.sh/content",
        ] {
            let response = app.clone().oneshot(authed_request(uri)).await.unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "unsafe_path");
        }
    }

    #[tokio::test]
    async fn content_endpoint_error_hides_local_paths() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let root = workspace.root().display().to_string();

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/scripts/../secret.sh/content"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert!(!body.to_string().contains(&root));
    }

    #[tokio::test]
    async fn content_endpoint_maps_unsupported_script_to_415() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(workspace.scripts_root().join("note.txt"), "hello\n").unwrap();

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/scripts/note.txt/content"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "unsupported_script");
    }

    #[tokio::test]
    async fn scripts_query_percent_decodes_values() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/scripts?tag=ops%20team"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn scripts_endpoint_maps_missing_script_to_404() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/scripts/missing.sh"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn env_endpoints_round_trip_and_redact_values() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let app = router(TOKEN.to_string(), workspace.clone_for_executor());

        let create = app
            .clone()
            .oneshot(authed_json_request(
                "/v1/envs",
                r#"{"name":"prod","params":[{"key":"HOST","value":"prod.example.com"},{"key":"API_KEY","value":"super_secret"}]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);

        let show = app
            .clone()
            .oneshot(authed_request("/v1/envs/prod"))
            .await
            .unwrap();
        assert_eq!(show.status(), StatusCode::OK);
        let show_body = response_json(show).await;
        assert_eq!(show_body["data"][0]["key"], "HOST");
        assert_eq!(show_body["data"][1]["key"], "API_KEY");
        assert_eq!(show_body["data"][1]["value"], "****");
        assert!(!show_body.to_string().contains("super_secret"));

        let set = app
            .clone()
            .oneshot(authed_json_method_request(
                Method::PUT,
                "/v1/envs/prod/params/PORT",
                r#"{"value":"443"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(set.status(), StatusCode::OK);

        let activate = app
            .clone()
            .oneshot(authed_json_request("/v1/envs/prod/activate", r#"{}"#))
            .await
            .unwrap();
        assert_eq!(activate.status(), StatusCode::OK);

        let list = app
            .clone()
            .oneshot(authed_request("/v1/envs"))
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_body = response_json(list).await;
        assert_eq!(list_body["data"][0]["name"], "prod");
        assert_eq!(list_body["data"][0]["file"], "prod.conf");
        assert_eq!(list_body["data"][0]["active"], true);

        let remove_param = app
            .clone()
            .oneshot(authed_delete_request("/v1/envs/prod/params/API_KEY"))
            .await
            .unwrap();
        assert_eq!(remove_param.status(), StatusCode::OK);

        let deactivate = app
            .clone()
            .oneshot(authed_delete_request("/v1/envs/active"))
            .await
            .unwrap();
        assert_eq!(deactivate.status(), StatusCode::OK);

        let delete = app
            .oneshot(authed_delete_request("/v1/envs/prod"))
            .await
            .unwrap();
        assert_eq!(delete.status(), StatusCode::OK);
        assert!(!workspace.envs_dir().join("prod.conf").exists());
    }

    #[tokio::test]
    async fn env_endpoints_replace_patch_and_reject_conf_route_names() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let app = router(TOKEN.to_string(), workspace);

        let replace = app
            .clone()
            .oneshot(authed_json_method_request(
                Method::PUT,
                "/v1/envs/dev",
                r#"{"params":[{"key":"HOST","value":"localhost"}]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(replace.status(), StatusCode::OK);

        let patch = app
            .clone()
            .oneshot(authed_json_method_request(
                Method::PATCH,
                "/v1/envs/dev",
                r#"{"params":[{"key":"HOST","value":"127.0.0.1"},{"key":"TOKEN","value":"secret_value"}]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(patch.status(), StatusCode::OK);

        let show = app
            .clone()
            .oneshot(authed_request("/v1/envs/dev"))
            .await
            .unwrap();
        let show_body = response_json(show).await;
        assert_eq!(show_body["data"][0]["value"], "127.0.0.1");
        assert_eq!(show_body["data"][1]["value"], "****");
        assert!(!show_body.to_string().contains("secret_value"));

        let invalid = app
            .oneshot(authed_request("/v1/envs/dev.conf"))
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let invalid_body = response_json(invalid).await;
        assert_eq!(invalid_body["error"]["code"], "invalid_input");
    }

    #[tokio::test]
    async fn env_endpoints_require_specific_policy_capabilities() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        env_ops::create_env(
            &workspace,
            "prod",
            &[env_ops::EnvParam {
                key: "HOST".into(),
                value: "prod.example.com".into(),
            }],
        )
        .unwrap();
        let app = router_with_policy(
            TOKEN.to_string(),
            workspace,
            ApiPolicy::allow([ApiCapability::EnvRead]),
        );

        let read = app
            .clone()
            .oneshot(authed_request("/v1/envs/prod"))
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);

        for request in [
            authed_json_method_request(
                Method::PATCH,
                "/v1/envs/prod",
                r#"{"params":[{"key":"HOST","value":"changed"}]}"#,
            ),
            authed_json_request("/v1/envs/prod/activate", r#"{}"#),
            authed_delete_request("/v1/envs/active"),
        ] {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "forbidden");
        }
    }

    #[tokio::test]
    async fn runs_and_queue_stats_endpoints_return_operation_data() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let conn = runs::open(&workspace).unwrap();
        runs::enqueue(
            &conn,
            workspace
                .scripts_root()
                .join("job.sh")
                .to_string_lossy()
                .as_ref(),
            &[],
            EnqueueOptions {
                run_id: Some("rid-http".into()),
                actor: "agent".into(),
                reason: None,
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
                script_name: None,
                omakure_version: app_meta::APP_VERSION.to_string(),
                trigger: runs::RunTrigger::Manual,
                env_name: None,
                allowed_secret_refs: None,
            },
        )
        .unwrap();

        let app = router(TOKEN.to_string(), workspace);
        let list = app
            .clone()
            .oneshot(authed_request("/v1/runs?state_set=all"))
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_body = response_json(list).await;
        assert_eq!(list_body["data"][0]["run_id"], "rid-http");

        let show = app
            .clone()
            .oneshot(authed_request("/v1/runs/rid-http"))
            .await
            .unwrap();
        assert_eq!(show.status(), StatusCode::OK);
        let show_body = response_json(show).await;
        assert_eq!(show_body["data"]["actor"], "agent");

        let traces = app
            .clone()
            .oneshot(authed_request("/v1/runs/rid-http/traces"))
            .await
            .unwrap();
        assert_eq!(traces.status(), StatusCode::OK);

        let stats = app
            .oneshot(authed_request("/v1/queue/stats"))
            .await
            .unwrap();
        assert_eq!(stats.status(), StatusCode::OK);
        let stats_body = response_json(stats).await;
        assert_eq!(stats_body["data"]["total"], 1);
    }

    #[tokio::test]
    async fn runs_endpoint_maps_invalid_query_to_400() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/runs?state=bad"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "invalid_input");
    }

    #[tokio::test]
    async fn enqueue_run_requires_auth() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/runs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"script":"job.sh"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "unauthorized");
    }

    #[tokio::test]
    async fn enqueue_run_returns_queued_run() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"job","args":["--x"],"run_id":"rid-post","actor":"agent","reason":"api","priority":7,"timeout_ms":1000}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["data"]["run_id"], "rid-post");
        assert_eq!(body["data"]["state"], "queued");
        assert_eq!(body["data"]["actor"], "agent");
        assert_eq!(body["data"]["priority"], 7);
    }

    #[tokio::test]
    async fn enqueue_run_endpoint_redacts_secret_args_in_response_and_storage() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(
            workspace.scripts_root().join("secret.sh"),
            r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
        )
        .unwrap();
        std::fs::write(
            workspace.envs_dir().join("prod.conf"),
            "token=http_secret_value\n",
        )
        .unwrap();

        let response = router(TOKEN.to_string(), workspace.clone_for_executor())
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret.sh","run_id":"rid-http-secret","args":["--token","secret://prod/token"]}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let rendered = body.to_string();
        assert!(rendered.contains("secret://prod/token"));
        assert!(!rendered.contains("http_secret_value"));

        let conn = runs::open(&workspace).unwrap();
        let row = runs::get_run(&conn, "rid-http-secret").unwrap().unwrap();
        assert!(row.args_json.contains("secret://prod/token"));
        assert!(!row.args_json.contains("http_secret_value"));
    }

    #[tokio::test]
    async fn enqueue_run_rejects_plaintext_secret_arg_values() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(
            workspace.scripts_root().join("secret.sh"),
            r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
        )
        .unwrap();

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret.sh","run_id":"rid-http-plain-secret","args":["--token","http_secret_value"]}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "invalid_input");
        assert!(!body.to_string().contains("http_secret_value"));
    }

    #[tokio::test]
    async fn enqueue_run_accepts_env_and_rejects_non_reconstructable_secret_fields() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(
            workspace.scripts_root().join("secret.sh"),
            r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"},{"Name":"MODE","Type":"string","Arg":"--mode"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
        )
        .unwrap();
        std::fs::write(
            workspace.envs_dir().join("prod.conf"),
            "TOKEN=env_secret_value\n",
        )
        .unwrap();

        let env_response = router(TOKEN.to_string(), workspace.clone_for_executor())
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret.sh","run_id":"rid-http-env-secret","args":["--mode","fast"],"env":"prod"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(env_response.status(), StatusCode::OK);
        let env_body = response_json(env_response).await;
        assert!(env_body.to_string().contains("<redacted>"));
        assert!(!env_body.to_string().contains("env_secret_value"));

        let conn = runs::open(&workspace).unwrap();
        let env_row = runs::get_run(&conn, "rid-http-env-secret")
            .unwrap()
            .unwrap();
        assert_eq!(
            runs::get_run_env(&conn, "rid-http-env-secret")
                .unwrap()
                .as_deref(),
            Some("prod")
        );
        assert!(env_row.args_json.contains("<redacted>"));
        assert!(!env_row.args_json.contains("env_secret_value"));
        drop(conn);

        let direct_response = router(TOKEN.to_string(), workspace.clone_for_executor())
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret.sh","run_id":"rid-http-direct-secret","secret_fields":{"TOKEN":"direct_secret_value"}}"#,
            ))
            .await
            .unwrap();

        assert_eq!(direct_response.status(), StatusCode::BAD_REQUEST);
        let direct_body = response_json(direct_response).await;
        assert_eq!(direct_body["error"]["code"], "invalid_input");
        assert!(!direct_body.to_string().contains("direct_secret_value"));
    }

    #[tokio::test]
    async fn enqueue_run_enforces_secret_provider_acl() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        std::fs::write(
            workspace.scripts_root().join("secret.sh"),
            r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
echo ok
"#,
        )
        .unwrap();
        std::fs::write(
            workspace.envs_dir().join("prod.conf"),
            "token=provider_secret\n",
        )
        .unwrap();
        let app = router_with_policy(
            TOKEN.to_string(),
            workspace.clone_for_executor(),
            ApiPolicy::allow_with_secret_refs(
                [ApiCapability::RunWrite, ApiCapability::SecretProviderUse],
                ["secret://prod/other"],
            ),
        );

        let denied = app
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret.sh","run_id":"rid-denied-ref","args":["--token","secret://prod/token"]}"#,
            ))
            .await
            .unwrap();

        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let body = response_json(denied).await;
        assert_eq!(body["error"]["code"], "forbidden");
        assert!(!body.to_string().contains("provider_secret"));
    }

    #[tokio::test]
    async fn enqueue_run_env_and_secret_fields_require_policy_capabilities() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        std::fs::write(
            workspace.envs_dir().join("prod.conf"),
            "TOKEN=env_secret_value\n",
        )
        .unwrap();
        let app = router_with_policy(
            TOKEN.to_string(),
            workspace,
            ApiPolicy::allow([ApiCapability::RunWrite]),
        );

        let plain = app
            .clone()
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"job.sh","run_id":"rid-plain"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(plain.status(), StatusCode::OK);

        for body in [
            r#"{"script":"job.sh","run_id":"rid-env","env":"prod"}"#,
            r#"{"script":"job.sh","run_id":"rid-secret","secret_fields":{"TOKEN":"direct_secret_value"}}"#,
            r#"{"script":"job.sh","run_id":"rid-secret-ref","args":["--token","secret://prod/token"]}"#,
        ] {
            let response = app
                .clone()
                .oneshot(authed_json_request("/v1/runs", body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "forbidden");
        }
    }

    #[tokio::test]
    async fn enqueue_run_implicit_secret_default_requires_secret_capability() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_secret_script(
            workspace.scripts_root(),
            "secret-default.sh",
            Some("schema_secret_value"),
        );
        let app = router_with_policy(
            TOKEN.to_string(),
            workspace,
            ApiPolicy::allow([ApiCapability::RunWrite]),
        );

        let response = app
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret-default.sh","run_id":"rid-default"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "forbidden");
        assert!(!body.to_string().contains("schema_secret_value"));
    }

    #[tokio::test]
    async fn enqueue_run_implicit_active_env_secret_requires_env_capability() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_secret_script(workspace.scripts_root(), "secret-env.sh", None);
        std::fs::write(
            workspace.envs_dir().join("prod.conf"),
            "token=active_secret\n",
        )
        .unwrap();
        std::fs::write(workspace.envs_active_path(), "prod.conf\n").unwrap();
        let app = router_with_policy(
            TOKEN.to_string(),
            workspace,
            ApiPolicy::allow([ApiCapability::RunWrite]),
        );

        let response = app
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"secret-env.sh","run_id":"rid-env-implicit"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "forbidden");
        assert!(!body.to_string().contains("active_secret"));
    }

    #[tokio::test]
    async fn mutating_run_routes_require_run_write_capability() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let conn = runs::open(&workspace).unwrap();
        runs::enqueue(
            &conn,
            "job.sh",
            &[],
            EnqueueOptions {
                run_id: Some("rid-cap".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let app = router_with_policy(
            TOKEN.to_string(),
            workspace,
            ApiPolicy::allow([ApiCapability::EnvRead]),
        );

        for request in [
            authed_json_request("/v1/runs/rid-cap/cancel", r#"{}"#),
            authed_json_request("/v1/runs/rid-cap/dead-letter", r#"{}"#),
        ] {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "forbidden");
        }
    }

    #[tokio::test]
    async fn mutating_battery_routes_require_battery_write_capability() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        battery_ops::add_battery(
            &workspace,
            battery_ops::AddBatteryRequest {
                name: "azure".into(),
                git_url: "https://example.invalid/azure.git".into(),
                requested_ref: "main".into(),
            },
        )
        .unwrap();
        let app = router_with_policy(
            TOKEN.to_string(),
            workspace,
            ApiPolicy::allow([ApiCapability::EnvRead]),
        );

        for request in [
            authed_json_request(
                "/v1/batteries",
                r#"{"name":"new","git_url":"https://example.invalid/new.git"}"#,
            ),
            authed_json_request("/v1/batteries/azure/sync", r#"{}"#),
            authed_json_request("/v1/batteries/azure/scripts/list/install", r#"{}"#),
            authed_delete_request("/v1/batteries/azure"),
        ] {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "forbidden");
        }
    }

    #[tokio::test]
    async fn enqueue_run_maps_invalid_script_to_404() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request(
                "/v1/runs",
                r#"{"script":"missing.sh"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn enqueue_run_rejects_outside_workspace_script() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(outside.path(), "outside.sh");

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request(
                "/v1/runs",
                &format!(
                    r#"{{"script":"{}"}}"#,
                    outside.path().join("outside.sh").display()
                ),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "unsafe_path");
    }

    #[tokio::test]
    async fn malformed_json_returns_envelope() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request("/v1/runs", r#"{"#))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"]["code"], "invalid_input");
    }

    #[tokio::test]
    async fn oversized_json_returns_envelope() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        let body = format!(r#"{{"script":"{}"}}"#, "x".repeat(BODY_LIMIT_BYTES + 1));

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request("/v1/runs", &body))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "payload_too_large");
    }

    #[tokio::test]
    async fn cancel_run_success_and_missing_run() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let conn = runs::open(&workspace).unwrap();
        runs::enqueue(
            &conn,
            workspace
                .scripts_root()
                .join("job.sh")
                .to_string_lossy()
                .as_ref(),
            &[],
            EnqueueOptions {
                run_id: Some("rid-cancel".into()),
                actor: "agent".into(),
                reason: None,
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
                script_name: None,
                omakure_version: app_meta::APP_VERSION.to_string(),
                trigger: runs::RunTrigger::Manual,
                env_name: None,
                allowed_secret_refs: None,
            },
        )
        .unwrap();

        let app = router(TOKEN.to_string(), workspace);
        let cancelled = app
            .clone()
            .oneshot(authed_json_request(
                "/v1/runs/rid-cancel/cancel",
                r#"{"reason":"stop"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(cancelled.status(), StatusCode::OK);
        let cancelled_body = response_json(cancelled).await;
        assert_eq!(cancelled_body["data"]["state"], "cancelled");

        let missing = app
            .oneshot(authed_json_request("/v1/runs/missing/cancel", r#"{}"#))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cancel_run_maps_invalid_transition_to_conflict() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let conn = runs::open(&workspace).unwrap();
        let row = runs::start_inline(
            &conn,
            workspace
                .scripts_root()
                .join("job.sh")
                .to_string_lossy()
                .as_ref(),
            &[],
            "worker:test",
            EnqueueOptions {
                run_id: Some("rid-done".into()),
                actor: "agent".into(),
                reason: None,
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
                script_name: None,
                omakure_version: app_meta::APP_VERSION.to_string(),
                trigger: runs::RunTrigger::Manual,
                env_name: None,
                allowed_secret_refs: None,
            },
        )
        .unwrap();
        runs::complete(
            &conn,
            &row.run_id,
            RunCompletion {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                success: true,
                error: None,
            },
        )
        .unwrap();

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request("/v1/runs/rid-done/cancel", r#"{}"#))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "conflict");
    }

    #[tokio::test]
    async fn dead_letter_run_success_and_invalid_transition() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "job.sh");
        let conn = runs::open(&workspace).unwrap();
        let failed = runs::start_inline(
            &conn,
            workspace
                .scripts_root()
                .join("job.sh")
                .to_string_lossy()
                .as_ref(),
            &[],
            "worker:test",
            EnqueueOptions {
                run_id: Some("rid-failed".into()),
                actor: "agent".into(),
                reason: None,
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
                script_name: None,
                omakure_version: app_meta::APP_VERSION.to_string(),
                trigger: runs::RunTrigger::Manual,
                env_name: None,
                allowed_secret_refs: None,
            },
        )
        .unwrap();
        runs::fail(
            &conn,
            &failed.run_id,
            RunCompletion {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(1),
                success: false,
                error: Some("boom".into()),
            },
        )
        .unwrap();
        runs::enqueue(
            &conn,
            workspace
                .scripts_root()
                .join("job.sh")
                .to_string_lossy()
                .as_ref(),
            &[],
            EnqueueOptions {
                run_id: Some("rid-queued".into()),
                actor: "agent".into(),
                reason: None,
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
                script_name: None,
                omakure_version: app_meta::APP_VERSION.to_string(),
                trigger: runs::RunTrigger::Manual,
                env_name: None,
                allowed_secret_refs: None,
            },
        )
        .unwrap();

        let app = router(TOKEN.to_string(), workspace);
        let promoted = app
            .clone()
            .oneshot(authed_json_request(
                "/v1/runs/rid-failed/dead-letter",
                r#"{"reason":"triaged"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(promoted.status(), StatusCode::OK);
        let promoted_body = response_json(promoted).await;
        assert_eq!(promoted_body["data"]["state"], "dead_letter");

        let invalid = app
            .oneshot(authed_json_request(
                "/v1/runs/rid-queued/dead-letter",
                r#"{}"#,
            ))
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::CONFLICT);
        let invalid_body = response_json(invalid).await;
        assert_eq!(invalid_body["error"]["code"], "conflict");
    }

    #[tokio::test]
    async fn battery_endpoints_require_auth() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/batteries")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn battery_add_and_list_use_operations() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let app = router(TOKEN.to_string(), workspace);
        let add = app
            .clone()
            .oneshot(authed_json_request(
                "/v1/batteries",
                r#"{"name":"azure","git_url":"https://example.invalid/azure.git","requested_ref":"stable"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(add.status(), StatusCode::OK);
        let add_body = response_json(add).await;
        assert_eq!(add_body["data"]["name"], "azure");
        assert_eq!(add_body["data"]["requested_ref"], "stable");

        let list = app.oneshot(authed_request("/v1/batteries")).await.unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_body = response_json(list).await;
        assert_eq!(list_body["data"][0]["name"], "azure");
    }

    #[tokio::test]
    async fn battery_add_rejects_plaintext_http_git_url() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request(
                "/v1/batteries",
                r#"{"name":"plain","git_url":"http://example.invalid/plain.git"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "invalid_input");
    }

    #[tokio::test]
    async fn battery_add_rejects_local_git_url() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request(
                "/v1/batteries",
                r#"{"name":"local","git_url":"/tmp/local-battery.git"}"#,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "invalid_input");
    }

    #[tokio::test]
    async fn battery_sync_invalid_manifest_maps_to_400() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        register_invalid_https_battery_cache(&workspace, "bad");

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_request("/v1/batteries/bad"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "manifest_invalid");
    }

    #[tokio::test]
    async fn battery_http_operations_reject_existing_non_https_sources() {
        let repo = invalid_manifest_repo();
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        battery_ops::add_battery(
            &workspace,
            battery_ops::AddBatteryRequest {
                name: "local".into(),
                git_url: repo.path().display().to_string(),
                requested_ref: "main".into(),
            },
        )
        .unwrap();

        let app = router(TOKEN.to_string(), workspace);
        for request in [
            authed_json_request("/v1/batteries/local/sync", r#"{}"#),
            authed_request("/v1/batteries/local"),
            authed_request("/v1/batteries/local/scripts"),
            authed_json_request("/v1/batteries/local/scripts/list/install", r#"{}"#),
        ] {
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = response_json(response).await;
            assert_eq!(body["error"]["code"], "invalid_input");
        }
    }

    #[tokio::test]
    async fn battery_missing_and_unsynced_errors_are_stable() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        battery_ops::add_battery(
            &workspace,
            battery_ops::AddBatteryRequest {
                name: "azure".into(),
                git_url: "https://example.invalid/azure.git".into(),
                requested_ref: "main".into(),
            },
        )
        .unwrap();

        let app = router(TOKEN.to_string(), workspace);
        let missing = app
            .clone()
            .oneshot(authed_request("/v1/batteries/missing"))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let missing_body = response_json(missing).await;
        assert_eq!(missing_body["error"]["code"], "not_found");

        let unsynced = app
            .clone()
            .oneshot(authed_request("/v1/batteries/azure/scripts"))
            .await
            .unwrap();
        assert_eq!(unsynced.status(), StatusCode::CONFLICT);
        let unsynced_body = response_json(unsynced).await;
        assert_eq!(unsynced_body["error"]["code"], "not_synced");

        let install = app
            .oneshot(authed_json_request(
                "/v1/batteries/azure/scripts/list/install",
                r#"{}"#,
            ))
            .await
            .unwrap();
        assert_eq!(install.status(), StatusCode::CONFLICT);
        let install_body = response_json(install).await;
        assert_eq!(install_body["error"]["code"], "not_synced");
    }

    #[tokio::test]
    async fn battery_sync_missing_maps_to_404() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);

        let response = router(TOKEN.to_string(), workspace)
            .oneshot(authed_json_request("/v1/batteries/missing/sync", r#"{}"#))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn battery_remove_supports_cache_flag() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        battery_ops::add_battery(
            &workspace,
            battery_ops::AddBatteryRequest {
                name: "keep".into(),
                git_url: "https://example.invalid/keep.git".into(),
                requested_ref: "main".into(),
            },
        )
        .unwrap();
        battery_ops::add_battery(
            &workspace,
            battery_ops::AddBatteryRequest {
                name: "drop".into(),
                git_url: "https://example.invalid/drop.git".into(),
                requested_ref: "main".into(),
            },
        )
        .unwrap();
        let paths = battery_ops::BatteryPaths::for_workspace(&workspace);
        std::fs::create_dir_all(paths.cache_path_for("drop")).unwrap();

        let app = router(TOKEN.to_string(), workspace);
        let keep = app
            .clone()
            .oneshot(authed_delete_request("/v1/batteries/keep"))
            .await
            .unwrap();
        assert_eq!(keep.status(), StatusCode::OK);
        let keep_body = response_json(keep).await;
        assert_eq!(keep_body["data"]["cache_removed"], false);

        let drop = app
            .oneshot(authed_delete_request(
                "/v1/batteries/drop?remove_cache=true",
            ))
            .await
            .unwrap();
        assert_eq!(drop.status(), StatusCode::OK);
        let drop_body = response_json(drop).await;
        assert_eq!(drop_body["data"]["cache_removed"], true);
    }
}

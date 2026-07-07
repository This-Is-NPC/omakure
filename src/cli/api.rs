use crate::cli::args::ApiArgs;
use crate::cli::json;
use crate::operations::battery as battery_ops;
use crate::operations::config as config_ops;
use crate::operations::core;
use crate::operations::doctor as doctor_ops;
use crate::operations::scripts as scripts_ops;
use crate::operations::search as search_ops;
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::workspace::Workspace;
use axum::body::{to_bytes, Body};
use axum::extract::{Path as AxumPath, RawQuery, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
}

impl Clone for ApiState {
    fn clone(&self) -> Self {
        Self {
            token_digest: self.token_digest,
            workspace: self.workspace.clone_for_executor(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ApiConfigError {
    MissingToken,
    InvalidToken,
    NonLoopbackBind(SocketAddr),
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
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;

    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(args.bind).await?;
        axum::serve(listener, router(token, workspace)).await?;
        Ok::<(), Box<dyn Error>>(())
    })
}

fn router(token: String, workspace: Workspace) -> Router {
    let state = ApiState {
        token_digest: token_digest_for(&token),
        workspace,
    };
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
    operation_response(core::workspace_summary(&state.workspace))
}

async fn config_handler(State(state): State<ApiState>) -> Response {
    operation_response(config_ops::redacted_config_summary(&state.workspace))
}

async fn doctor_handler(State(state): State<ApiState>) -> Response {
    operation_response(doctor_ops::doctor_report(&state.workspace))
}

async fn search_handler(State(state): State<ApiState>, RawQuery(raw_query): RawQuery) -> Response {
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
    let request = query_pairs(raw_query.as_deref()).map(|pairs| core::ListScriptsRequest {
        tags: query_values(&pairs, "tag"),
    });
    operation_response(request.and_then(|request| core::list_scripts(&state.workspace, request)))
}

async fn describe_script_handler(State(state): State<ApiState>, script_id: String) -> Response {
    operation_response(core::describe_script(
        &state.workspace,
        core::DescribeScriptRequest { script: script_id },
    ))
}

async fn script_schema_handler(State(state): State<ApiState>, script_id: String) -> Response {
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
    operation_response(scripts_ops::read_script_content(
        &state.workspace,
        scripts_ops::ReadScriptContentRequest { script: script_id },
    ))
}

async fn tree_root_handler(State(state): State<ApiState>) -> Response {
    operation_response(scripts_ops::list_tree(
        &state.workspace,
        scripts_ops::ListTreeRequest { path: None },
    ))
}

async fn tree_path_handler(
    State(state): State<ApiState>,
    AxumPath(path): AxumPath<String>,
) -> Response {
    operation_response(scripts_ops::list_tree(
        &state.workspace,
        scripts_ops::ListTreeRequest { path: Some(path) },
    ))
}

async fn list_runs_handler(
    State(state): State<ApiState>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request = list_runs_request(raw_query.as_deref());
    operation_response(request.and_then(|request| core::list_runs(&state.workspace, request)))
}

async fn show_run_handler(
    State(state): State<ApiState>,
    AxumPath(run_id): AxumPath<String>,
) -> Response {
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
    let request = list_traces_request(run_id, raw_query.as_deref());
    operation_response(request.and_then(|request| core::list_traces(&state.workspace, request)))
}

async fn queue_stats_handler(State(state): State<ApiState>) -> Response {
    operation_response(core::queue_stats(&state.workspace))
}

async fn enqueue_run_handler(State(state): State<ApiState>, body: Body) -> Response {
    let body = match parse_json_body::<EnqueueRunBody>(body).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    operation_response(core::enqueue_run(
        &state.workspace,
        core::EnqueueRunRequest {
            script: body.script,
            args: body.args,
            run_id: body.run_id,
            actor: body.actor,
            reason: body.reason,
            priority: body.priority,
            timeout_ms: body.timeout_ms,
            parent_run_id: body.parent_run_id,
            cron_schedule_id: body.cron_schedule_id,
        },
    ))
}

async fn cancel_run_handler(
    State(state): State<ApiState>,
    AxumPath(run_id): AxumPath<String>,
    body: Body,
) -> Response {
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
    operation_response(battery_ops::list_batteries(&state.workspace))
}

async fn add_battery_handler(State(state): State<ApiState>, body: Body) -> Response {
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
    async fn script_routes_support_nested_paths() {
        let dir = TempDir::new().unwrap();
        let workspace = workspace_in(&dir);
        write_script(workspace.scripts_root(), "tools/job.sh");

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
            .oneshot(authed_request("/v1/scripts/tools/job.sh/schema"))
            .await
            .unwrap();
        assert_eq!(schema.status(), StatusCode::OK);
        let schema_body = response_json(schema).await;
        assert_eq!(schema_body["data"]["name"], "tools/job.sh");
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

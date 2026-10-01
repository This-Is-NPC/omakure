use super::audit::safe_audit_run_id;
use super::battery::default_actor;
use super::bearer::{require_capability, require_scope};
use super::query::{query_bool, query_i64, query_pairs, query_value, query_values};
use super::respond::{
    attach_audit_run_id, operation_error_response, operation_response,
    operation_response_with_run_id, parse_json_body,
};
use super::state::{ApiCapability, ApiState};
use crate::auth::AuthContext;
use crate::operations::core;
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::ports::ScriptRepository;
use axum::body::Body;
use axum::extract::{Path as AxumPath, RawQuery, State};
use axum::response::Response;
use axum::Extension;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

async fn run_blocking<T: Send + 'static>(
    gate: Arc<tokio::sync::Semaphore>,
    task: impl FnOnce() -> T + Send + 'static,
) -> Result<T, OperationError> {
    let permit = gate.acquire_owned().await.map_err(|_| {
        OperationError::new(OperationErrorCode::IoFailed, "run operation unavailable")
    })?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        task()
    })
    .await
    .map_err(|_| OperationError::new(OperationErrorCode::IoFailed, "run operation failed"))
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

pub(super) async fn list_runs_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::RunRead) {
        return response;
    }
    let request = match list_runs_request(raw_query.as_deref()) {
        Ok(request) => request,
        Err(err) => return operation_error_response(err),
    };
    let gate = Arc::clone(&state.run_operation_gate);
    match run_blocking(gate, move || core::list_runs(&state.workspace, request)).await {
        Ok(result) => operation_response(result),
        Err(err) => operation_error_response(err),
    }
}

pub(super) async fn show_run_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(run_id): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::RunRead) {
        return response;
    }
    operation_response(core::show_run(
        &state.workspace,
        core::ShowRunRequest { run_id },
    ))
}

pub(super) async fn list_traces_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(run_id): AxumPath<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::RunRead) {
        return response;
    }
    let request = list_traces_request(run_id, raw_query.as_deref());
    operation_response(request.and_then(|request| core::list_traces(&state.workspace, request)))
}

pub(super) async fn queue_stats_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::RunRead) {
        return response;
    }
    operation_response(core::queue_stats(&state.workspace))
}

pub(super) async fn enqueue_run_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "runs:enqueue") {
        return response;
    }
    let body =
        match parse_json_body::<EnqueueRunBody>(body, state.deploy.http.body_limit_bytes).await {
            Ok(body) => body,
            Err(err) => return operation_error_response(err),
        };
    let requested_run_id = body.run_id.as_deref().and_then(safe_audit_run_id);
    if body.env.is_some() {
        if !state.deploy.runs.allow_env_selection {
            return attach_audit_run_id(
                operation_error_response(OperationError::new(
                    OperationErrorCode::Forbidden,
                    "policy runs.allow_env_selection=false",
                )),
                requested_run_id,
            );
        }
        if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnvUse) {
            return attach_audit_run_id(response, requested_run_id);
        }
    }
    let args_use_secret_provider = args_use_secret_provider(&body.args);
    if !state.deploy.runs.allow_secret_fields
        && (!body.secret_fields.is_empty() || args_use_secret_provider)
    {
        return attach_audit_run_id(
            operation_error_response(OperationError::new(
                OperationErrorCode::Forbidden,
                "policy runs.allow_secret_fields=false",
            )),
            requested_run_id,
        );
    }
    if !body.secret_fields.is_empty() || args_use_secret_provider {
        if let Some(response) = require_capability(&auth_ctx, ApiCapability::SecretProviderUse) {
            return attach_audit_run_id(response, requested_run_id);
        }
    }
    let gate = Arc::clone(&state.run_operation_gate);
    let fallback_run_id = requested_run_id.clone();
    match run_blocking(gate, move || {
        enqueue_authorized(state, auth_ctx, body, requested_run_id)
    })
    .await
    {
        Ok(response) => response,
        Err(err) => attach_audit_run_id(operation_error_response(err), fallback_run_id),
    }
}

fn enqueue_authorized(
    state: ApiState,
    auth_ctx: AuthContext,
    body: EnqueueRunBody,
    requested_run_id: Option<String>,
) -> Response {
    if let Some(response) = require_implicit_secret_capabilities(&state, &auth_ctx, &body) {
        return attach_audit_run_id(response, requested_run_id);
    }
    if body
        .secret_fields
        .values()
        .any(|value| !value.starts_with("secret://"))
    {
        return attach_audit_run_id(
            operation_error_response(OperationError::new(
                OperationErrorCode::InvalidInput,
                "queued HTTP secret_fields must use secret:// refs so workers can resolve them without persisting plaintext",
            )),
            requested_run_id,
        );
    }
    let result = core::enqueue_run_with_access(
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
        &state.policy.secret_access(&auth_ctx),
    );
    let run_id = result
        .as_ref()
        .ok()
        .map(|row| row.run_id.clone())
        .or(requested_run_id);
    operation_response_with_run_id(result, run_id)
}

fn require_implicit_secret_capabilities(
    state: &ApiState,
    auth_ctx: &AuthContext,
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
        if !state.deploy.runs.allow_secret_fields {
            return Some(operation_error_response(OperationError::new(
                OperationErrorCode::Forbidden,
                "policy runs.allow_secret_fields=false",
            )));
        }
        if let Some(response) = require_capability(auth_ctx, ApiCapability::SecretProviderUse) {
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
        if let Some(response) = require_capability(auth_ctx, ApiCapability::EnvUse) {
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

pub(super) async fn cancel_run_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(run_id): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "runs:cancel") {
        return response;
    }
    let body =
        match parse_json_body::<RunReasonBody>(body, state.deploy.http.body_limit_bytes).await {
            Ok(body) => body,
            Err(err) => return operation_error_response(err),
        };
    let result = core::cancel_run(
        &state.workspace,
        core::CancelRunRequest {
            run_id: run_id.clone(),
            reason: body.reason,
        },
    );
    operation_response_with_run_id(result, Some(run_id))
}

pub(super) async fn dead_letter_run_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(run_id): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "runs:dead-letter") {
        return response;
    }
    let body =
        match parse_json_body::<RunReasonBody>(body, state.deploy.http.body_limit_bytes).await {
            Ok(body) => body,
            Err(err) => return operation_error_response(err),
        };
    let result = core::dead_letter_run(
        &state.workspace,
        core::DeadLetterRunRequest {
            run_id: run_id.clone(),
            reason: body.reason,
        },
    );
    operation_response_with_run_id(result, Some(run_id))
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

#[cfg(test)]
mod blocking_tests {
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    #[tokio::test(flavor = "current_thread")]
    async fn locked_list_runs_does_not_block_health_request() {
        let dir = tempfile::tempdir().expect("workspace");
        let workspace = crate::test_support::workspace_in(&dir);
        let lock = crate::runs::open(&workspace).expect("runs database");
        lock.execute_batch(
            "PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE; CREATE TABLE hold_lock(id INTEGER)",
        )
        .expect("hold the SQLite write lock");
        let gate = Arc::new(tokio::sync::Semaphore::new(1));
        let app = super::super::router::router_with_run_gate(workspace, Arc::clone(&gate));
        let mut list = tokio::spawn({
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .uri("/v1/runs")
                        .header(
                            header::AUTHORIZATION,
                            format!("Bearer {}", crate::auth::test_credential::token()),
                        )
                        .body(Body::empty())
                        .expect("list request"),
                )
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while gate.available_permits() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("list handler entered the blocking operation");
        assert!(tokio::time::timeout(Duration::from_millis(100), &mut list)
            .await
            .is_err());
        let response = tokio::time::timeout(
            Duration::from_secs(1),
            app.oneshot(
                Request::builder()
                    .uri("/v1/health")
                    .body(Body::empty())
                    .unwrap(),
            ),
        )
        .await
        .expect("health request must remain responsive")
        .expect("health response");
        assert_eq!(response.status(), StatusCode::OK);
        lock.execute_batch("ROLLBACK").expect("release SQLite lock");
        drop(lock);
        let list = tokio::time::timeout(Duration::from_secs(3), list)
            .await
            .expect("list request completed")
            .expect("list task")
            .expect("list response");
        assert_eq!(list.status(), StatusCode::OK);
    }
}

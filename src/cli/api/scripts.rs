use super::bearer::require_capability;
use super::query::{query_pairs, query_value, query_values};
use super::respond::{operation_error_response, operation_response};
use super::state::{ApiCapability, ApiState};
use crate::auth::AuthContext;
use crate::operations::core;
use crate::operations::scripts as scripts_ops;
use crate::operations::search as search_ops;
use crate::operations::{OperationError, OperationErrorCode};
use axum::extract::{Path as AxumPath, RawQuery, State};
use axum::response::Response;
use axum::Extension;

pub(super) const MAX_SEARCH_QUERY_LEN: usize = 256;

pub(super) const MAX_SEARCH_TAGS: usize = 16;

pub(super) const MAX_SEARCH_TAG_LEN: usize = 64;

pub(super) async fn search_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::ScriptsRead) {
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
        Ok(search_ops::SearchScriptsRequest { query, tags })
    });
    let request = match request {
        Ok(request) => request,
        Err(err) => return operation_error_response(err),
    };
    let result =
        tokio::task::spawn_blocking(move || search_ops::search_scripts(&state.workspace, request))
            .await
            .unwrap_or_else(|err| {
                Err(OperationError::new(
                    OperationErrorCode::IoFailed,
                    format!("Search task failed: {err}"),
                ))
            });
    operation_response(result)
}

pub(super) async fn list_scripts_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::ScriptsRead) {
        return response;
    }
    let request = query_pairs(raw_query.as_deref()).map(|pairs| core::ListScriptsRequest {
        tags: query_values(&pairs, "tag"),
    });
    operation_response(request.and_then(|request| core::list_scripts(&state.workspace, request)))
}

async fn describe_script_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    script_id: String,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(core::describe_script(
        &state.workspace,
        core::DescribeScriptRequest { script: script_id },
    ))
}

async fn script_schema_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    script_id: String,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::ScriptsRead) {
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

pub(super) async fn script_path_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(script_id): AxumPath<String>,
) -> Response {
    if let Some(script_id) = script_id.strip_suffix("/content") {
        if !script_id.is_empty() {
            return script_content_handler(
                State(state),
                Extension(auth_ctx),
                script_id.to_string(),
            )
            .await;
        }
    }
    match script_id.strip_suffix("/schema") {
        Some(script_id) if !script_id.is_empty() => {
            script_schema_handler(State(state), Extension(auth_ctx), script_id.to_string()).await
        }
        _ => describe_script_handler(State(state), Extension(auth_ctx), script_id).await,
    }
}

async fn script_content_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    script_id: String,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(scripts_ops::read_script_content(
        &state.workspace,
        scripts_ops::ReadScriptContentRequest { script: script_id },
        state.deploy.scripts.max_content_bytes as u64,
    ))
}

pub(super) async fn tree_root_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(scripts_ops::list_tree(
        &state.workspace,
        scripts_ops::ListTreeRequest { path: None },
        state.deploy.scripts.tree_entry_limit,
    ))
}

pub(super) async fn tree_path_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(path): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::ScriptsRead) {
        return response;
    }
    operation_response(scripts_ops::list_tree(
        &state.workspace,
        scripts_ops::ListTreeRequest { path: Some(path) },
        state.deploy.scripts.tree_entry_limit,
    ))
}

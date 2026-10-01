use super::bearer::require_capability;
use super::blocking::operation_response_bounded;
use super::respond::{operation_error_response, parse_json_body};
use super::state::{ApiCapability, ApiState};
use crate::auth::AuthContext;
use crate::operations::envs as env_ops;
use crate::operations::{OperationError, OperationErrorCode};
use axum::body::Body;
use axum::extract::{Path as AxumPath, State};
use axum::response::Response;
use axum::Extension;
use serde::Deserialize;
use std::sync::Arc;

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

pub(super) async fn list_envs_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnvRead) {
        return response;
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    operation_response_bounded("environment", gate, move || {
        env_ops::list_envs(&state.workspace)
    })
    .await
}

fn env_params_forbid_secret_refs(
    deploy: &crate::policy::DeployPolicy,
    params: &[env_ops::EnvParam],
) -> Option<Response> {
    if deploy.envs.allow_secret_refs {
        return None;
    }
    if params.iter().any(|p| p.value.starts_with("secret://")) {
        return Some(operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.allow_secret_refs=false",
        )));
    }
    None
}

fn env_value_forbid_secret_ref(
    deploy: &crate::policy::DeployPolicy,
    value: &str,
) -> Option<Response> {
    if deploy.envs.allow_secret_refs || !value.starts_with("secret://") {
        return None;
    }
    Some(operation_error_response(OperationError::new(
        OperationErrorCode::Forbidden,
        "policy envs.allow_secret_refs=false",
    )))
}

pub(super) async fn create_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnvWrite) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    let body = match parse_json_body::<EnvBody>(body, state.deploy.http.body_limit_bytes).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    let Some(name) = body.name else {
        return operation_error_response(OperationError::new(
            OperationErrorCode::InvalidInput,
            "name is required",
        ));
    };
    if let Some(response) = env_params_forbid_secret_refs(&state.deploy, &body.params) {
        return response;
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    operation_response_bounded("environment", gate, move || {
        env_ops::create_env(&state.workspace, &name, &body.params)
    })
    .await
}

pub(super) async fn show_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnvRead) {
        return response;
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    operation_response_bounded("environment", gate, move || {
        env_ops::show_env(&state.workspace, &name)
    })
    .await
}

pub(super) async fn put_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(name): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnvWrite) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    let body = match parse_json_body::<EnvBody>(body, state.deploy.http.body_limit_bytes).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    if let Some(response) = env_params_forbid_secret_refs(&state.deploy, &body.params) {
        return response;
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    operation_response_bounded("environment", gate, move || {
        match env_ops::replace_env(&state.workspace, &name, &body.params) {
            Err(err) if err.code == OperationErrorCode::NotFound => {
                env_ops::create_env(&state.workspace, &name, &body.params)
            }
            other => other,
        }
    })
    .await
}

pub(super) async fn patch_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(name): AxumPath<String>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnvWrite) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    let body = match parse_json_body::<EnvBody>(body, state.deploy.http.body_limit_bytes).await {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    if let Some(response) = env_params_forbid_secret_refs(&state.deploy, &body.params) {
        return response;
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    operation_response_bounded("environment", gate, move || {
        for param in body.params {
            env_ops::set_param(&state.workspace, &name, &param.key, &param.value)?;
        }
        Ok(())
    })
    .await
}

pub(super) async fn delete_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnvWrite) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    operation_response_bounded("environment", gate, move || {
        env_ops::delete_env(&state.workspace, &name)
    })
    .await
}

pub(super) async fn set_env_param_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath((name, key)): AxumPath<(String, String)>,
    body: Body,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnvWrite) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    let body = match parse_json_body::<EnvParamBody>(body, state.deploy.http.body_limit_bytes).await
    {
        Ok(body) => body,
        Err(err) => return operation_error_response(err),
    };
    if let Some(response) = env_value_forbid_secret_ref(&state.deploy, &body.value) {
        return response;
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    operation_response_bounded("environment", gate, move || {
        env_ops::set_param(&state.workspace, &name, &key, &body.value)
    })
    .await
}

pub(super) async fn delete_env_param_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath((name, key)): AxumPath<(String, String)>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnvWrite) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    operation_response_bounded("environment", gate, move || {
        env_ops::remove_param(&state.workspace, &name, &key)
    })
    .await
}

pub(super) async fn activate_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnvActivate) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    operation_response_bounded("environment", gate, move || {
        env_ops::activate_env(&state.workspace, &name)
    })
    .await
}

pub(super) async fn deactivate_env_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::EnvActivate) {
        return response;
    }
    if !state.deploy.envs.http_manage {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy envs.http_manage=false",
        ));
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    operation_response_bounded("environment", gate, move || {
        env_ops::deactivate_env(&state.workspace)
    })
    .await
}

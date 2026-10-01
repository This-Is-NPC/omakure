use super::bearer::{require_capability, require_scope};
use super::blocking::{run_bounded, run_bounded_with_join};
use super::query::{query_bool, query_pairs};
use super::respond::{operation_error_response, operation_response, parse_json_body};
use super::state::{ApiCapability, ApiState};
use crate::auth::AuthContext;
use crate::operations::battery as battery_ops;
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::workspace::Workspace;
use axum::body::Body;
use axum::extract::{Path as AxumPath, RawQuery, State};
use axum::response::Response;
use axum::Extension;
use serde::Deserialize;
use serde::Serialize;
use std::sync::Arc;

async fn battery_operation_response<T: Serialize + Send + 'static>(
    gate: Arc<tokio::sync::Semaphore>,
    task: impl FnOnce() -> OperationResult<T> + Send + 'static,
) -> Response {
    let result = run_bounded("battery", gate, task)
        .await
        .and_then(std::convert::identity);
    operation_response(result)
}

#[derive(Debug, Deserialize)]
struct AddBatteryBody {
    name: String,
    git_url: String,
    #[serde(default = "default_battery_ref")]
    requested_ref: String,
    /// Optional `secret://provider/key` for private HTTPS Battery auth.
    #[serde(default)]
    token_ref: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct InstallBatteryScriptBody {
    #[serde(default)]
    force: bool,
}

pub(super) fn default_actor() -> String {
    "human".to_string()
}

fn default_battery_ref() -> String {
    "main".to_string()
}

pub(super) async fn list_batteries_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::BatteryRead) {
        return response;
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    battery_operation_response(gate, move || battery_ops::list_batteries(&state.workspace)).await
}

pub(super) async fn add_battery_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "batteries:add") {
        return response;
    }
    let body =
        match parse_json_body::<AddBatteryBody>(body, state.deploy.http.body_limit_bytes).await {
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
    if let Err(err) = battery_ops::assert_local_battery_allowed(
        state.deploy.sources.allow_local_batteries,
        &body.git_url,
    ) {
        return operation_error_response(err);
    }
    if !state.deploy.sources.allow_https_batteries {
        return operation_error_response(OperationError::new(
            OperationErrorCode::Forbidden,
            "policy sources.allow_https_batteries=false",
        ));
    }
    // SSRF guard (registration): reject literal private/loopback/metadata hosts
    // up front. The resolving check runs at sync time before the actual fetch.
    if let Err(err) = battery_ops::assert_git_url_host_public_literal(&body.git_url) {
        return operation_error_response(err);
    }
    if body
        .token_ref
        .as_ref()
        .is_some_and(|r| !r.trim().is_empty())
    {
        if !state.deploy.sources.allow_private_https_batteries {
            return operation_error_response(OperationError::new(
                OperationErrorCode::Forbidden,
                "policy sources.allow_private_https_batteries=false",
            ));
        }
        if let Some(response) = require_capability(&auth_ctx, ApiCapability::CredentialsUse) {
            return response;
        }
        // Validate ACL only — secret need not exist until sync.
        let access = state.policy.battery_credential_access(&auth_ctx);
        if let Err(err) =
            crate::secrets::check_secret_access(body.token_ref.as_deref().unwrap_or(""), &access)
        {
            return operation_error_response(OperationError::new(
                OperationErrorCode::Forbidden,
                format!("token_ref not usable: {err}"),
            ));
        }
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    battery_operation_response(gate, move || {
        battery_ops::add_battery(
            &state.workspace,
            battery_ops::AddBatteryRequest {
                name: body.name,
                git_url: body.git_url,
                requested_ref: body.requested_ref,
                token_ref: body.token_ref,
            },
        )
    })
    .await
}

pub(super) async fn sync_battery_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(battery_id): AxumPath<String>,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "batteries:sync") {
        return response;
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    let result = run_bounded_with_join(
        "battery",
        gate,
        move || {
            require_https_battery_source(&state.workspace, &battery_id)?;
            let batteries = battery_ops::list_batteries(&state.workspace)?;
            let summary = batteries
                .into_iter()
                .find(|b| b.name == battery_id)
                .ok_or_else(|| {
                    OperationError::new(
                        OperationErrorCode::NotFound,
                        format!("battery '{battery_id}' was not found"),
                    )
                })?;
            if summary.auth.is_some() {
                if !state.deploy.sources.allow_private_https_batteries {
                    return Err(OperationError::new(
                        OperationErrorCode::Forbidden,
                        "policy sources.allow_private_https_batteries=false",
                    ));
                }
                if !auth_ctx.has_scope("credentials:use") {
                    return Err(OperationError::new(
                        OperationErrorCode::Forbidden,
                        "credentials:use scope is required for private HTTPS Battery sync",
                    ));
                }
            }
            let access = state.policy.battery_credential_access(&auth_ctx);
            battery_ops::sync_battery_https_only_with_access(
                &state.workspace,
                battery_ops::SyncBatteryRequest { name: battery_id },
                &access,
            )
        },
        |_| {
            OperationError::new(
                OperationErrorCode::IoFailed,
                "battery sync task failed to complete",
            )
        },
    )
    .await
    .and_then(std::convert::identity);
    operation_response(result)
}

pub(super) async fn inspect_battery_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(battery_id): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::BatteryRead) {
        return response;
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    battery_operation_response(gate, move || {
        require_https_battery_source(&state.workspace, &battery_id)?;
        battery_ops::inspect_battery(
            &state.workspace,
            battery_ops::InspectBatteryRequest { name: battery_id },
        )
    })
    .await
}

pub(super) async fn list_battery_scripts_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(battery_id): AxumPath<String>,
) -> Response {
    if let Some(response) = require_capability(&auth_ctx, ApiCapability::BatteryRead) {
        return response;
    }
    let gate = Arc::clone(&state.blocking_operation_gate);
    battery_operation_response(gate, move || {
        require_https_battery_source(&state.workspace, &battery_id)?;
        battery_ops::list_battery_scripts(
            &state.workspace,
            battery_ops::InspectBatteryRequest { name: battery_id },
        )
    })
    .await
}

pub(super) async fn install_battery_script_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath((battery_id, script_id)): AxumPath<(String, String)>,
    body: Body,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "batteries:install") {
        return response;
    }
    let body =
        match parse_json_body::<InstallBatteryScriptBody>(body, state.deploy.http.body_limit_bytes)
            .await
        {
            Ok(body) => body,
            Err(err) => return operation_error_response(err),
        };
    let gate = Arc::clone(&state.blocking_operation_gate);
    battery_operation_response(gate, move || {
        require_https_battery_source(&state.workspace, &battery_id)?;
        battery_ops::install_battery_script(
            &state.workspace,
            battery_ops::InstallBatteryScriptRequest {
                battery_name: battery_id,
                script_id,
                force: body.force,
            },
        )
    })
    .await
}

pub(super) async fn remove_battery_handler(
    State(state): State<ApiState>,
    Extension(auth_ctx): Extension<AuthContext>,
    AxumPath(battery_id): AxumPath<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    if let Some(response) = require_scope(&auth_ctx, "batteries:remove") {
        return response;
    }
    let request = match query_pairs(raw_query.as_deref()).and_then(|pairs| {
        query_bool(&pairs, "remove_cache").map(|remove_cache| battery_ops::RemoveBatteryRequest {
            name: battery_id,
            remove_cache: remove_cache.unwrap_or(false),
        })
    }) {
        Ok(request) => request,
        Err(err) => return operation_error_response(err),
    };
    let gate = Arc::clone(&state.blocking_operation_gate);
    battery_operation_response(gate, move || {
        battery_ops::remove_battery(&state.workspace, request)
    })
    .await
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

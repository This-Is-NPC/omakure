use super::run_queries::io_error_runs;
use super::script_path::resolve_script_path;
use super::types::EnqueueRunRequest;
use crate::app_meta;
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::runs::{self, EnqueueOptions, RunRow, RunStore};
use crate::workspace::Workspace;

pub fn enqueue_run(workspace: &Workspace, request: EnqueueRunRequest) -> OperationResult<RunRow> {
    enqueue_run_with_access(
        workspace,
        request,
        &crate::secrets::SecretAccess::allow_all(),
    )
}

pub fn enqueue_run_with_access(
    workspace: &Workspace,
    request: EnqueueRunRequest,
    secret_access: &crate::secrets::SecretAccess,
) -> OperationResult<RunRow> {
    let path = resolve_script_path(&request.script, workspace.scripts_root())?;
    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    let env_file = request
        .env
        .as_deref()
        .map(|name| crate::operations::envs::env_file_path(workspace, name))
        .transpose()?;
    let extra_env =
        crate::adapters::environments::resolve_run_env(workspace.envs_dir(), env_file.as_deref())
            .map_err(|err| OperationError::new(OperationErrorCode::InvalidInput, err.to_string()))?;
    crate::secrets::validate_queued_secret_args_reconstructable(
        workspace,
        &canonical,
        &request.args,
    )
    .map_err(|error| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            format!(
                "required field `{}` is missing: {}",
                error.field(),
                error.message()
            ),
        )
    })?;
    let resolved_args = crate::secrets::resolve_args_with_access(
        workspace,
        &canonical,
        &request.args,
        &extra_env,
        &request.secret_fields,
        secret_access,
    )
    .map_err(|error| {
        let message = error.message();
        let code = if matches!(
            &error,
            crate::secrets::SecretArgError::Resolution {
                source: crate::secrets::SecretResolveError::Denied(_),
                ..
            }
        ) {
            OperationErrorCode::Forbidden
        } else {
            OperationErrorCode::InvalidInput
        };
        OperationError::new(
            code,
            format!("required field `{}` is missing: {}", error.field(), message),
        )
    })?;
    let store = RunStore::open(workspace).map_err(io_error_runs)?;
    store
        .enqueue(
            canonical.to_string_lossy().as_ref(),
            &resolved_args.persisted_args,
            EnqueueOptions {
                run_id: request.run_id,
                actor: request.actor,
                reason: request.reason,
                priority: request.priority,
                timeout_ms: request.timeout_ms,
                parent_run_id: request.parent_run_id,
                cron_schedule_id: request.cron_schedule_id,
                script_name: None,
                omakure_version: app_meta::APP_VERSION.to_string(),
                trigger: runs::RunTrigger::Manual,
                env_name: request.env,
                allowed_secret_refs: Some(resolved_args.provider_refs),
                script_content_hash: None,
            },
        )
        .map_err(io_error_runs)
}

/// Enqueue a run that a remote Conductor asked for.
///
/// Separate from `enqueue_run_with_access` rather than a flag on it, because
/// the two guarantees this path owes cannot be optional:
///
/// * the row is `RunTrigger::Cue`, which is what keeps it out of the worker's
///   lease steal and makes the Health Plane report its provenance honestly
///   rather than as `manual`;
/// * secret access is an explicit empty policy, meaning deny-all.
///
/// The second is why this is a function and not a parameter. `None` writes
/// ALLOW-ALL (`runs/`), and a policy *lookup error* also grants allow-all
/// (`run_executor.rs`), so a caller who forgot the field would hand a remote
/// instruction every secret the node holds. Here there is no field to forget:
/// the signature cannot express allow-all.
///
/// A script declaring secret fields is refused at the gate before reaching this
/// point, so the empty policy denies nothing the script legitimately needed.
///
/// `authorized_content_hash` is a required parameter for the same reason the
/// secret policy is not one. It is the bytes gate E authorized, and the executor
/// refuses a Cue-origin run whose recorded hash is missing. Were it a field on
/// the shared request struct, every non-Cue caller would carry a `None` that
/// looks like a default, and the day someone copied one into this path the run
/// would execute unconstrained with nothing red.
pub fn enqueue_cue_run(
    workspace: &Workspace,
    request: EnqueueRunRequest,
    authorized_content_hash: &str,
) -> OperationResult<RunRow> {
    let script_name = request.script.clone();
    let path = resolve_script_path(&request.script, workspace.scripts_root())?;
    let canonical = std::fs::canonicalize(&path).unwrap_or(path);
    // No environment is injected and no secret is resolvable: an empty scope
    // set with an empty allowed-ref set is deny-all.
    let deny_all = crate::secrets::SecretAccess::new(Vec::<String>::new(), Vec::<String>::new());
    let resolved_args = crate::secrets::resolve_args_with_access(
        workspace,
        &canonical,
        &request.args,
        &[],
        &request.secret_fields,
        &deny_all,
    )
    .map_err(|error| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("{}: {}", error.field(), error.message()),
        )
    })?;

    let mut store = RunStore::open(workspace).map_err(io_error_runs)?;
    store
        .enqueue_cue(
            canonical.to_string_lossy().as_ref(),
            &resolved_args.persisted_args,
            EnqueueOptions {
                run_id: request.run_id,
                actor: request.actor,
                reason: request.reason,
                priority: request.priority,
                timeout_ms: request.timeout_ms,
                parent_run_id: None,
                cron_schedule_id: None,
                script_name: Some(script_name),
                omakure_version: app_meta::APP_VERSION.to_string(),
                trigger: runs::RunTrigger::Cue,
                env_name: None,
                allowed_secret_refs: Some(Vec::new()),
                script_content_hash: Some(authorized_content_hash.to_string()),
            },
        )
        .map_err(io_error_runs)
}

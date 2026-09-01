use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::app_meta;
use crate::ports::ScriptRepository;
use crate::runs::{
    self, EnqueueOptions, RunFilters, RunRow, RunState, RunStateSet, RunStats, TraceLevel, TraceRow,
};
use crate::runtime::script_extensions;
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use super::{OperationError, OperationErrorCode, OperationResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub version: String,
    pub workspace_root: PathBuf,
    pub scripts_root: PathBuf,
    pub omakure_dir: PathBuf,
    pub history_dir: PathBuf,
    pub workspace_config: PathBuf,
    pub envs_dir: PathBuf,
    pub envs_active_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ListScriptsRequest {
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptSummary {
    pub absolute_path: String,
    pub relative_path: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub field_count: usize,
    pub schema_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescribeScriptRequest {
    pub script: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptDescription {
    pub absolute_path: String,
    pub relative_path: String,
    pub schema: ScriptSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptSchema {
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub fields: Vec<ScriptField>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptField {
    pub name: String,
    pub prompt: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub order: u32,
    pub required: bool,
    pub arg: Option<String>,
    pub default: Option<String>,
    pub choices: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ListRunsRequest {
    pub script: Option<String>,
    pub actor: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub success: Option<bool>,
    pub limit: Option<i64>,
    pub states: Vec<String>,
    pub state_set: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowRunRequest {
    pub run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListTracesRequest {
    pub run_id: String,
    pub level: Option<String>,
    pub since_sequence: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnqueueRunRequest {
    pub script: String,
    pub args: Vec<String>,
    pub env: Option<String>,
    pub secret_fields: Vec<(String, String)>,
    pub run_id: Option<String>,
    pub actor: String,
    pub reason: Option<String>,
    pub priority: i64,
    pub timeout_ms: Option<i64>,
    pub parent_run_id: Option<String>,
    pub cron_schedule_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelRunRequest {
    pub run_id: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadLetterRunRequest {
    pub run_id: String,
    pub reason: Option<String>,
}

pub fn workspace_summary(workspace: &Workspace) -> OperationResult<WorkspaceSummary> {
    Ok(WorkspaceSummary {
        version: app_meta::APP_VERSION.to_string(),
        workspace_root: workspace.root().to_path_buf(),
        scripts_root: workspace.scripts_root().to_path_buf(),
        omakure_dir: workspace.omakure_dir().to_path_buf(),
        history_dir: workspace.history_dir().to_path_buf(),
        workspace_config: workspace.config_path().to_path_buf(),
        envs_dir: workspace.envs_dir().to_path_buf(),
        envs_active_path: workspace.envs_active_path().to_path_buf(),
    })
}

pub fn list_scripts(
    workspace: &Workspace,
    request: ListScriptsRequest,
) -> OperationResult<Vec<ScriptSummary>> {
    let root = canonical_scripts_root(workspace.scripts_root())?;
    let repo = FsWorkspaceRepository::new(root.clone());
    let mut scripts = repo.list_scripts_recursive().map_err(io_error)?;
    scripts.sort();
    Ok(scripts
        .into_iter()
        .map(|script| build_script_summary(&repo, &root, script))
        .filter(|entry| matches_all_tags(entry, &request.tags))
        .collect())
}

pub fn describe_script(
    workspace: &Workspace,
    request: DescribeScriptRequest,
) -> OperationResult<ScriptDescription> {
    let root = canonical_scripts_root(workspace.scripts_root())?;
    let path = resolve_script_path(&request.script, &root)?;
    let repo = FsWorkspaceRepository::new(root.clone());
    let schema = repo
        .read_schema(&path)
        .map_err(|err| OperationError::new(OperationErrorCode::InvalidInput, err.to_string()))?;
    let absolute_path = std::fs::canonicalize(&path)
        .unwrap_or_else(|_| path.clone())
        .to_string_lossy()
        .to_string();
    let relative_path = logical_relative_path(&path, &root);
    Ok(ScriptDescription {
        absolute_path,
        relative_path,
        schema: script_schema_from_domain(schema),
    })
}

fn canonical_scripts_root(scripts_root: &Path) -> OperationResult<PathBuf> {
    scripts_root.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to canonicalize scripts root: {err}"),
        )
    })
}

fn logical_relative_path(path: &Path, root: &Path) -> String {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path_text = canonical_path.to_string_lossy().replace('\\', "/");
    let root_text = canonical_root
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    path_text
        .strip_prefix(&root_text)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(&path_text)
        .to_string()
}

fn script_schema_from_domain(schema: crate::domain::Schema) -> ScriptSchema {
    let mut fields: Vec<ScriptField> = schema
        .fields
        .into_iter()
        .map(|field| {
            let is_secret = field.is_secret();
            ScriptField {
                name: field.name,
                prompt: field.prompt,
                kind: field.kind,
                order: field.order.unwrap_or(0),
                required: field.required.unwrap_or(false),
                arg: field.arg,
                default: (!is_secret).then_some(field.default).flatten(),
                choices: field.choices,
            }
        })
        .collect();
    fields.sort_by_key(|field| field.order);
    ScriptSchema {
        name: schema.name,
        description: schema.description,
        tags: schema.tags.unwrap_or_default(),
        fields,
    }
}

pub fn list_runs(workspace: &Workspace, request: ListRunsRequest) -> OperationResult<Vec<RunRow>> {
    let states = resolve_states(&request.states, request.state_set.as_deref())?;
    let filters = RunFilters {
        script: request.script,
        actor: request.actor,
        since_ms: request.since_ms,
        until_ms: request.until_ms,
        success: request.success,
        limit: request.limit,
        states,
    };
    let conn = runs::open(workspace).map_err(io_error_string)?;
    runs::query_runs(&conn, &filters).map_err(io_error_string)
}

pub fn show_run(workspace: &Workspace, request: ShowRunRequest) -> OperationResult<RunRow> {
    let conn = runs::open(workspace).map_err(io_error_string)?;
    match runs::get_run(&conn, &request.run_id).map_err(io_error_string)? {
        Some(row) => Ok(row),
        None => Err(OperationError::new(
            OperationErrorCode::NotFound,
            format!("run not found: {}", request.run_id),
        )),
    }
}

pub fn list_traces(
    workspace: &Workspace,
    request: ListTracesRequest,
) -> OperationResult<Vec<TraceRow>> {
    let conn = runs::open(workspace).map_err(io_error_string)?;
    let level = match request.level.as_deref() {
        Some(level) => Some(TraceLevel::from_str(level).map_err(invalid_input)?),
        None => None,
    };
    runs::query_traces(&conn, &request.run_id, level, request.since_sequence)
        .map_err(map_not_found_string)
}

pub fn queue_stats(workspace: &Workspace) -> OperationResult<RunStats> {
    run_stats(workspace)
}

pub fn run_stats(workspace: &Workspace) -> OperationResult<RunStats> {
    let conn = runs::open(workspace).map_err(io_error_string)?;
    runs::stats(&conn).map_err(io_error_string)
}

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
    .map_err(|(field, message)| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("required field `{}` is missing: {}", field, message),
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
    .map_err(|(field, message)| {
        let code = if message.contains("secrets:use") || message.contains("not allowed") {
            OperationErrorCode::Forbidden
        } else {
            OperationErrorCode::InvalidInput
        };
        OperationError::new(
            code,
            format!("required field `{}` is missing: {}", field, message),
        )
    })?;
    let conn = runs::open(workspace).map_err(io_error_string)?;
    runs::enqueue(
        &conn,
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
    .map_err(io_error_string)
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
/// ALLOW-ALL (`runs.rs`), and a policy *lookup error* also grants allow-all
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
    .map_err(|(field, message)| {
        OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("{field}: {message}"),
        )
    })?;

    let mut conn = runs::open(workspace).map_err(io_error_string)?;
    runs::enqueue_cue(
        &mut conn,
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
    .map_err(io_error_string)
}

pub fn cancel_run(workspace: &Workspace, request: CancelRunRequest) -> OperationResult<RunRow> {
    let conn = runs::open(workspace).map_err(io_error_string)?;
    require_run(&conn, &request.run_id)?;
    runs::cancel(&conn, &request.run_id, request.reason, None).map_err(map_transition_error)
}

pub fn dead_letter_run(
    workspace: &Workspace,
    request: DeadLetterRunRequest,
) -> OperationResult<RunRow> {
    let conn = runs::open(workspace).map_err(io_error_string)?;
    require_run(&conn, &request.run_id)?;
    runs::dead_letter(&conn, &request.run_id, request.reason).map_err(map_transition_error)
}

fn build_script_summary(
    repo: &FsWorkspaceRepository,
    root: &Path,
    script: PathBuf,
) -> ScriptSummary {
    let relative_path = logical_relative_path(&script, root);
    let absolute_path = std::fs::canonicalize(&script)
        .unwrap_or_else(|_| script.clone())
        .to_string_lossy()
        .to_string();
    match repo.read_schema(&script) {
        Ok(schema) => ScriptSummary {
            absolute_path,
            relative_path,
            name: Some(schema.name),
            description: schema.description,
            tags: schema.tags.unwrap_or_default(),
            field_count: schema.fields.len(),
            schema_error: None,
        },
        Err(err) => ScriptSummary {
            absolute_path,
            relative_path,
            name: None,
            description: None,
            tags: Vec::new(),
            field_count: 0,
            schema_error: Some(err.to_string()),
        },
    }
}

fn matches_all_tags(entry: &ScriptSummary, required: &[String]) -> bool {
    required
        .iter()
        .all(|tag| entry.tags.iter().any(|entry_tag| entry_tag == tag))
}

fn resolve_script_path(script: &str, scripts_root: &Path) -> OperationResult<PathBuf> {
    if script.trim().is_empty() {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "script is required",
        ));
    }

    let root = scripts_root.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to canonicalize scripts root: {err}"),
        )
    })?;
    let normalized_script = script.replace('\\', "/");
    let has_separator = normalized_script.contains('/');
    let path = PathBuf::from(&normalized_script);
    if (!path.is_absolute() && has_windows_prefix(&normalized_script))
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("script path escapes scripts root: {script}"),
        ));
    }
    // Absolute paths are validated after resolution.  Do not compare their
    // spelling with the canonical root here: Windows may present the same
    // path through an 8.3 or verbatim alias (and may normalize its case).
    // `canonical_script_path` performs the containment check on canonical
    // paths once the candidate exists.
    let candidate = if path.is_absolute() {
        path
    } else if has_separator {
        root.join(path)
    } else {
        root.join(script)
    };
    resolve_with_extensions(candidate, &root)
}

fn has_windows_prefix(path: &str) -> bool {
    path.starts_with("\\\\")
        || (path.as_bytes().get(1).is_some_and(|colon| *colon == b':')
            && path.as_bytes()[0].is_ascii_alphabetic())
}

fn resolve_with_extensions(path: PathBuf, scripts_root: &Path) -> OperationResult<PathBuf> {
    reject_absolute_path_outside_root(&path, scripts_root)?;
    if path.exists() {
        if path.is_file() {
            return canonical_script_path(&path, scripts_root);
        }
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            format!("script is not a file: {}", path.display()),
        ));
    }
    if path.extension().is_some() {
        return Err(OperationError::new(
            OperationErrorCode::NotFound,
            format!("script not found: {}", path.display()),
        ));
    }
    for ext in script_extensions() {
        let mut candidate = path.clone();
        candidate.set_extension(ext);
        if candidate.is_file() {
            return canonical_script_path(&candidate, scripts_root);
        }
    }
    Err(OperationError::new(
        OperationErrorCode::NotFound,
        format!("script not found: {}", path.display()),
    ))
}

fn reject_absolute_path_outside_root(path: &Path, scripts_root: &Path) -> OperationResult<()> {
    if !path.is_absolute() {
        return Ok(());
    }
    let canonical_root = scripts_root.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to canonicalize scripts root: {err}"),
        )
    })?;
    let mut probe = path;
    loop {
        match probe.canonicalize() {
            Ok(canonical) => {
                if canonical.starts_with(&canonical_root) {
                    return Ok(());
                }
                return Err(OperationError::new(
                    OperationErrorCode::UnsafePath,
                    format!("script path escapes scripts root: {}", path.display()),
                ));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let Some(parent) = probe.parent() else {
                    return Err(io_error(err));
                };
                if parent == probe {
                    return Err(io_error(err));
                }
                probe = parent;
            }
            Err(err) => return Err(io_error(err)),
        }
    }
}

fn canonical_script_path(path: &Path, scripts_root: &Path) -> OperationResult<PathBuf> {
    let canonical = path.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to canonicalize script path: {err}"),
        )
    })?;
    let canonical_root = scripts_root.canonicalize().map_err(|err| {
        OperationError::new(
            OperationErrorCode::IoFailed,
            format!("failed to canonicalize scripts root: {err}"),
        )
    })?;
    if canonical.starts_with(&canonical_root) {
        Ok(canonical)
    } else {
        Err(OperationError::new(
            OperationErrorCode::UnsafePath,
            format!("script path escapes scripts root: {}", path.display()),
        ))
    }
}

fn resolve_states(states: &[String], state_set: Option<&str>) -> OperationResult<Vec<RunState>> {
    if !states.is_empty() && state_set.is_some() {
        return Err(OperationError::new(
            OperationErrorCode::InvalidInput,
            "state and state_set are mutually exclusive",
        ));
    }
    if let Some(set) = state_set {
        return RunStateSet::from_str(set)
            .map(|set| set.to_states())
            .map_err(invalid_input);
    }
    if !states.is_empty() {
        return states
            .iter()
            .map(|state| RunState::from_str(state).map_err(invalid_input))
            .collect();
    }
    Ok(RunStateSet::Terminal.to_states())
}

fn require_run(conn: &rusqlite::Connection, run_id: &str) -> OperationResult<()> {
    match runs::get_run(conn, run_id).map_err(io_error_string)? {
        Some(_) => Ok(()),
        None => Err(OperationError::new(
            OperationErrorCode::NotFound,
            format!("run not found: {run_id}"),
        )),
    }
}

fn map_transition_error(message: String) -> OperationError {
    if message.contains("terminal state") || message.contains("only failed or timed_out") {
        OperationError::new(OperationErrorCode::Conflict, message)
    } else {
        OperationError::new(OperationErrorCode::IoFailed, message)
    }
}

fn map_not_found_string(message: String) -> OperationError {
    if message.starts_with("not_found") {
        OperationError::new(OperationErrorCode::NotFound, message)
    } else {
        OperationError::new(OperationErrorCode::IoFailed, message)
    }
}

fn invalid_input(message: String) -> OperationError {
    OperationError::new(OperationErrorCode::InvalidInput, message)
}

fn io_error(err: impl std::error::Error) -> OperationError {
    OperationError::new(OperationErrorCode::IoFailed, err.to_string())
}

fn io_error_string(message: String) -> OperationError {
    OperationError::new(OperationErrorCode::IoFailed, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::{RunCompletion, RunState};
    use tempfile::TempDir;

    fn workspace_in(dir: &TempDir) -> Workspace {
        let ws = Workspace::new(dir.path().to_path_buf());
        ws.ensure_layout().unwrap();
        ws
    }

    fn cue_request() -> EnqueueRunRequest {
        EnqueueRunRequest {
            script: "deploy.sh".into(),
            args: Vec::new(),
            env: None,
            secret_fields: Vec::new(),
            run_id: Some("cue-derived-run-id".into()),
            actor: "conductor".into(),
            reason: Some("contract test".into()),
            priority: 0,
            timeout_ms: None,
            parent_run_id: None,
            cron_schedule_id: None,
        }
    }

    /// Asserted against what landed in the database, not inferred from the call.
    ///
    /// `None` writes ALLOW-ALL and a policy *lookup error* also grants
    /// allow-all, so "we pass an empty vec" is a claim worth checking. If this
    /// ever reads `None`, a remote instruction is receiving every secret the
    /// node holds.
    #[test]
    fn a_cue_run_stores_an_explicit_deny_all_secret_policy() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_script(ws.scripts_root(), "deploy.sh", &[]);

        let row =
            enqueue_cue_run(&ws, cue_request(), "authorized-hash").expect("enqueue the cue run");

        let conn = runs::open(&ws).unwrap();
        assert_eq!(
            runs::get_run_secret_refs(&conn, &row.run_id).unwrap(),
            Some(Vec::new()),
            "an empty policy is deny-all; None would have meant allow-all"
        );
    }

    /// The provenance that keeps it out of the worker lease steal and stops the
    /// Health Plane reporting it as `manual`.
    #[test]
    fn a_cue_run_is_recorded_as_cue_originated() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_script(ws.scripts_root(), "deploy.sh", &[]);

        let row =
            enqueue_cue_run(&ws, cue_request(), "authorized-hash").expect("enqueue the cue run");

        assert_eq!(row.trigger, runs::RunTrigger::Cue);
    }

    /// The caller supplies a run id derived from the cue id, so the primary key
    /// is the durable at-most-once guard rather than a separate dedup store.
    #[test]
    fn the_same_cue_derived_run_id_cannot_be_enqueued_twice() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_script(ws.scripts_root(), "deploy.sh", &[]);

        assert!(enqueue_cue_run(&ws, cue_request(), "authorized-hash").is_ok());
        assert!(
            enqueue_cue_run(&ws, cue_request(), "authorized-hash").is_err(),
            "the primary key is what makes a repeated cue id run at most once"
        );
    }

    /// The ordinary path is unchanged: it still resolves declared secrets.
    #[test]
    fn the_manual_enqueue_path_still_records_its_own_policy() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_script(ws.scripts_root(), "deploy.sh", &[]);

        let row = enqueue_run_with_access(
            &ws,
            EnqueueRunRequest {
                run_id: Some("manual-run".into()),
                ..cue_request()
            },
            &crate::secrets::SecretAccess::allow_all(),
        )
        .expect("enqueue a manual run");

        assert_eq!(row.trigger, runs::RunTrigger::Manual);
    }

    fn write_script(root: &Path, path: &str, tags: &[&str]) {
        let tags_json = if tags.is_empty() {
            String::new()
        } else {
            format!(
                ",\"Tags\":[{}]",
                tags.iter()
                    .map(|tag| format!("\"{tag}\""))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        let script = root.join(path);
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(
            script,
            format!(
                "#!/usr/bin/env bash\n# OMAKURE_SCHEMA_START\n# {{\"Name\":\"{path}\",\"Fields\":[]{tags_json}}}\n# OMAKURE_SCHEMA_END\necho ok\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn workspace_summary_returns_operation_ready_paths() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);

        let summary = workspace_summary(&ws).unwrap();

        assert_eq!(summary.workspace_root, ws.root());
        assert_eq!(summary.omakure_dir, ws.omakure_dir());
        assert_eq!(summary.history_dir, ws.history_dir());
    }

    #[test]
    fn list_scripts_filters_by_tags_and_preserves_schema_errors() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_script(ws.scripts_root(), "deploy.sh", &["ops"]);
        write_script(ws.scripts_root(), "other.sh", &["misc"]);
        std::fs::write(ws.scripts_root().join("broken.sh"), "#!/usr/bin/env bash\n").unwrap();

        let entries = list_scripts(
            &ws,
            ListScriptsRequest {
                tags: vec!["ops".into()],
            },
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, "deploy.sh");
    }

    #[test]
    fn describe_script_returns_schema_payload() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_script(ws.scripts_root(), "deploy.sh", &["ops"]);

        let desc = describe_script(
            &ws,
            DescribeScriptRequest {
                script: "deploy".into(),
            },
        )
        .unwrap();

        assert_eq!(desc.relative_path, "deploy.sh");
        assert_eq!(desc.schema.tags, vec!["ops"]);
    }

    #[test]
    fn script_resolution_rejects_absolute_paths_outside_workspace() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_script(outside.path(), "outside.sh", &[]);

        let err = enqueue_run(
            &ws,
            EnqueueRunRequest {
                script: outside
                    .path()
                    .join("outside.sh")
                    .to_string_lossy()
                    .to_string(),
                args: Vec::new(),
                env: None,
                secret_fields: Vec::new(),
                run_id: None,
                actor: "agent".into(),
                reason: None,
                priority: 0,
                timeout_ms: None,
                parent_run_id: None,
                cron_schedule_id: None,
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[test]
    fn script_resolution_rejects_missing_absolute_paths_outside_workspace() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let ws = workspace_in(&dir);

        let err = describe_script(
            &ws,
            DescribeScriptRequest {
                script: outside
                    .path()
                    .join("missing.sh")
                    .to_string_lossy()
                    .into_owned(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }
    #[test]
    fn script_resolution_accepts_confined_absolute_paths() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_script(ws.scripts_root(), "deploy.sh", &[]);

        let path = ws.scripts_root().join("deploy.sh");
        let description = describe_script(
            &ws,
            DescribeScriptRequest {
                script: path.to_string_lossy().into_owned(),
            },
        )
        .unwrap();

        assert_eq!(description.relative_path, "deploy.sh");
    }

    #[cfg(unix)]
    #[test]
    fn script_resolution_accepts_absolute_paths_through_workspace_alias() {
        use std::os::unix::fs::symlink;

        let real = TempDir::new().unwrap();
        let alias_parent = TempDir::new().unwrap();
        let alias = alias_parent.path().join("workspace");
        symlink(real.path(), &alias).unwrap();
        // Keep the workspace's configured root as the symlink alias. The
        // absolute request therefore has a different spelling from the
        // canonical root, just as an 8.3/verbatim Windows path can.
        let ws = Workspace::new(alias);
        ws.ensure_layout().unwrap();
        write_script(ws.scripts_root(), "deploy.sh", &[]);

        let description = describe_script(
            &ws,
            DescribeScriptRequest {
                script: ws
                    .scripts_root()
                    .join("deploy.sh")
                    .to_string_lossy()
                    .into_owned(),
            },
        )
        .unwrap();

        assert_eq!(description.relative_path, "deploy.sh");
    }

    #[test]
    fn logical_relative_paths_handle_windows_alias_fixtures() {
        let verbatim_root = Path::new(r"\\?\C:\workspace\scripts");
        let verbatim_path = Path::new(r"\\?\C:\workspace\scripts\tools\deploy.cmd");
        let short_root = Path::new(r"C:\PROGRA~1\OMAKURE\scripts");
        let short_path = Path::new(r"C:\PROGRA~1\OMAKURE\scripts\tools\deploy.cmd");

        assert_eq!(
            logical_relative_path(verbatim_path, verbatim_root),
            "tools/deploy.cmd"
        );
        assert_eq!(
            logical_relative_path(short_path, short_root),
            "tools/deploy.cmd"
        );
    }

    #[test]
    fn logical_relative_paths_use_forward_slashes_for_windows_fixtures() {
        let root = Path::new(r"C:\workspace\scripts");
        let path = Path::new(r"C:\workspace\scripts\tools\deploy.sh");

        assert_eq!(logical_relative_path(path, root), "tools/deploy.sh");
    }

    #[cfg(unix)]
    #[test]
    fn list_scripts_matches_a_symlinked_workspace_root() {
        use std::os::unix::fs::symlink;

        let real = TempDir::new().unwrap();
        let alias_parent = TempDir::new().unwrap();
        let alias = alias_parent.path().join("workspace");
        symlink(real.path(), &alias).unwrap();
        let ws = Workspace::new(alias);
        ws.ensure_layout().unwrap();
        write_script(ws.scripts_root(), "tools/deploy.sh", &[]);

        let entries = list_scripts(&ws, ListScriptsRequest::default()).unwrap();

        assert_eq!(entries[0].relative_path, "tools/deploy.sh");
    }

    #[test]
    fn script_resolution_rejects_parent_traversal() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);

        let err = describe_script(
            &ws,
            DescribeScriptRequest {
                script: "../outside.sh".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[cfg(unix)]
    #[test]
    fn script_resolution_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_script(outside.path(), "outside.sh", &[]);
        symlink(
            outside.path().join("outside.sh"),
            ws.scripts_root().join("escape.sh"),
        )
        .unwrap();

        let err = describe_script(
            &ws,
            DescribeScriptRequest {
                script: "escape.sh".into(),
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::UnsafePath);
    }

    #[test]
    fn enqueue_list_show_cancel_and_stats_share_runs_state_machine() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_script(ws.scripts_root(), "job.sh", &[]);

        let row = enqueue_run(
            &ws,
            EnqueueRunRequest {
                script: "job".into(),
                args: vec!["--x".into()],
                env: None,
                secret_fields: Vec::new(),
                run_id: Some("rid-op".into()),
                actor: "agent".into(),
                reason: Some("test".into()),
                priority: 5,
                timeout_ms: Some(1000),
                parent_run_id: None,
                cron_schedule_id: None,
            },
        )
        .unwrap();
        assert_eq!(row.state, RunState::Queued);

        let rows = list_runs(
            &ws,
            ListRunsRequest {
                state_set: Some("all".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);

        let shown = show_run(
            &ws,
            ShowRunRequest {
                run_id: "rid-op".into(),
            },
        )
        .unwrap();
        assert_eq!(shown.actor, "agent");

        let cancelled = cancel_run(
            &ws,
            CancelRunRequest {
                run_id: "rid-op".into(),
                reason: Some("stop".into()),
            },
        )
        .unwrap();
        assert_eq!(cancelled.state, RunState::Cancelled);

        let stats = queue_stats(&ws).unwrap();
        assert_eq!(stats.total, 1);
    }

    #[test]
    fn dead_letter_requires_existing_failed_run() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);
        write_script(ws.scripts_root(), "job.sh", &[]);
        let conn = runs::open(&ws).unwrap();
        let row = runs::start_inline(
            &conn,
            ws.scripts_root().join("job.sh").to_string_lossy().as_ref(),
            &[],
            "worker:test",
            EnqueueOptions {
                run_id: Some("rid-fail".into()),
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
                script_content_hash: None,
            },
        )
        .unwrap();
        runs::fail(
            &conn,
            &row.run_id,
            RunCompletion {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(1),
                success: false,
                error: Some("boom".into()),
            },
        )
        .unwrap();

        let dead = dead_letter_run(
            &ws,
            DeadLetterRunRequest {
                run_id: "rid-fail".into(),
                reason: Some("triaged".into()),
            },
        )
        .unwrap();

        assert_eq!(dead.state, RunState::DeadLetter);
    }

    #[test]
    fn list_traces_reports_missing_run_as_not_found() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);

        let err = list_traces(
            &ws,
            ListTracesRequest {
                run_id: "missing".into(),
                level: Some("debug".into()),
                since_sequence: None,
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::NotFound);
    }

    #[test]
    fn invalid_state_filter_is_operation_error() {
        let dir = TempDir::new().unwrap();
        let ws = workspace_in(&dir);

        let err = list_runs(
            &ws,
            ListRunsRequest {
                states: vec!["bad".into()],
                ..Default::default()
            },
        )
        .unwrap_err();

        assert_eq!(err.code, OperationErrorCode::InvalidInput);
    }
}

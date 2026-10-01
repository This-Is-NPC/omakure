use super::script_path::resolve_script_path;
use super::types::{
    DescribeScriptRequest, ListScriptsRequest, ScriptDescription, ScriptField, ScriptSchema,
    ScriptSummary, WorkspaceSummary,
};
use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::app_meta;
use crate::operations::path::{canonical_relative_path, canonical_scripts_root};
use crate::operations::{OperationError, OperationErrorCode, OperationResult, io_error};
use crate::workspace::Workspace;
use std::path::{Path, PathBuf};

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
    let relative_path = canonical_relative_path(&path, &root);
    Ok(ScriptDescription {
        absolute_path,
        relative_path,
        schema: script_schema_from_domain(schema),
    })
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

fn build_script_summary(
    repo: &FsWorkspaceRepository,
    root: &Path,
    script: PathBuf,
) -> ScriptSummary {
    let relative_path = canonical_relative_path(&script, root);
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

pub(crate) fn matches_all_tags(entry: &ScriptSummary, required: &[String]) -> bool {
    required
        .iter()
        .all(|tag| entry.tags.iter().any(|entry_tag| entry_tag == tag))
}

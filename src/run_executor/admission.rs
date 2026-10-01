use super::{ExecutionResult, ExecutionTerminal};
use crate::runs::{self, RunCompletion, RunRow, RunTrigger};
use crate::workspace::Workspace;
use std::path::{Path, PathBuf};

/// Enqueue-time validation cannot authorize a path forever: a queued or
/// scheduled script may have been replaced with a link into Battery cache.
pub(super) fn execution_script_path(
    workspace: &Workspace,
    row: &RunRow,
) -> Result<PathBuf, ExecutionResult> {
    let path = Path::new(&row.script_path);
    if !path.exists() {
        return Err(script_admission_failure(
            ExecutionTerminal::Errored,
            format!("script not found: {}", row.script_path),
        ));
    }
    let path = crate::operations::core::canonical_script_path(path, workspace.scripts_root())
        .map_err(|error| script_admission_failure(ExecutionTerminal::Errored, error.to_string()))?;
    check_cue_script_unchanged(workspace, row, &path)
        .map_err(|error| script_admission_failure(ExecutionTerminal::Failed, error))?;
    Ok(path)
}

fn script_admission_failure(terminal: ExecutionTerminal, error: String) -> ExecutionResult {
    ExecutionResult {
        terminal,
        completion: RunCompletion {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            success: false,
            error: Some(error),
        },
    }
}

/// Refuse a Cue-origin run whose script is no longer the script it was
/// authorized against.
///
/// The Remote Cue contract declined this third check on the grounds that it
/// only defended against an attacker who could already write to the workspace.
/// A baseline push makes that premise false: a signed baseline replaces scripts
/// legitimately, so a Cue accepted against version N can reach the executor
/// with version N+1 on disk and nobody hostile anywhere in the story.
///
/// Fail-closed in all three directions. A missing hash row, a failed lookup,
/// and an unreadable script each refuse, because none of them is evidence that
/// the bytes are the authorized ones — and the run_secret_refs precedent, where
/// "missing" and "lookup failed" both meant allow-all, is exactly the shape of
/// mistake this must not repeat.
///
/// Scoped to `RunTrigger::Cue`. A manual or scheduled run is started by someone
/// on this machine against whatever is on this machine; there is no earlier
/// authorization for it to have drifted from.
fn check_cue_script_unchanged(
    workspace: &Workspace,
    row: &RunRow,
    script_path: &Path,
) -> Result<(), String> {
    if row.trigger != RunTrigger::Cue {
        return Ok(());
    }
    let recorded = runs::open(workspace)
        .and_then(|conn| runs::get_run_script_hash(&conn, &row.run_id))
        .map_err(|err| format!("authorized script content lookup failed: {err}"))?
        .ok_or_else(|| {
            "no authorized script content was recorded for this remote run".to_string()
        })?;
    match crate::remote_cue::content_hash(script_path) {
        Some(current) if current == recorded => Ok(()),
        Some(_) => Err(
            "the script changed after this remote run was authorized; it was not executed"
                .to_string(),
        ),
        None => Err("the authorized script could not be read at execution time".to_string()),
    }
}

pub(super) fn secret_access_for_row(
    workspace: &Workspace,
    row: &RunRow,
    args: &[String],
) -> Result<crate::secrets::SecretAccess, String> {
    let has_provider_ref = args.iter().any(|arg| {
        arg.starts_with("secret://")
            || arg
                .split_once('=')
                .map(|(_, value)| value.starts_with("secret://"))
                .unwrap_or(false)
    });
    let refs = match runs::open(workspace)
        .and_then(|conn| runs::get_run_secret_refs(&conn, &row.run_id))
    {
        Ok(Some(refs)) => refs,
        Ok(None) if has_provider_ref => {
            return Err("secret provider policy missing for queued run".to_string())
        }
        Ok(None) => return Ok(crate::secrets::SecretAccess::allow_all()),
        Err(err) if has_provider_ref => {
            return Err(format!("secret provider policy lookup failed: {err}"))
        }
        Err(_) => return Ok(crate::secrets::SecretAccess::allow_all()),
    };
    if refs
        .iter()
        .any(|secret_ref| secret_ref == runs::ALLOW_ALL_SECRET_REFS_POLICY)
    {
        Ok(crate::secrets::SecretAccess::allow_all())
    } else {
        Ok(crate::secrets::SecretAccess::new(
            [crate::secrets::SECRETS_USE_SCOPE],
            refs,
        ))
    }
}

pub(super) fn parse_args_json(args_json: &str) -> Vec<String> {
    serde_json::from_str(args_json).unwrap_or_else(|_| Vec::new())
}

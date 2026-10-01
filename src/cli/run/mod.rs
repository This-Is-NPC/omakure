//! `omakure run` — synchronous fast path for one script execution.
//!
//! `omakure run` writes through the same state machine as
//! `omakure queue worker`. The row is inserted in `state='running'` at
//! start (so `history list --state running` sees it immediately) and
//! transitions to `completed`/`failed`/`timed_out` on completion via
//! the shared [`crate::run_executor::execute_with_heartbeat`] helper.

use crate::app_meta;
use crate::cli::args::RunArgs;
use crate::cli::emit::emit_error;
use crate::cli::json::{self, codes};
use crate::operations::core::check_required_fields;
use crate::operations::core::resolve_script_path;
use crate::run_executor::{execute_with_heartbeat, ExecutionResult, ExecutionTerminal};
use crate::runs::{self, EnqueueOptions};
use crate::workspace::Workspace;
use std::error::Error;
use std::path::PathBuf;

pub fn run(
    scripts_dir: PathBuf,
    options: RunArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;

    let script_path = match resolve_script_path(&options.script, workspace.root()) {
        Ok(path) => path,
        Err(err) => return emit_error(json_output, err.code.as_str(), err.to_string()),
    };

    let PreparedRunInputs {
        extra_env,
        resolved_args,
    } = match prepare_run_inputs(&workspace, &script_path, &options, json_output) {
        Ok(inputs) => inputs,
        Err((code, message)) => return emit_error(json_output, code, message),
    };

    let canonical = std::fs::canonicalize(&script_path).unwrap_or_else(|_| script_path.clone());
    let canonical_str = canonical.to_string_lossy().to_string();
    let conn = runs::open(&workspace).map_err(|err| -> Box<dyn Error> { err.into() })?;
    let row = runs::start_inline(
        &conn,
        &canonical_str,
        &resolved_args.persisted_args,
        &format!("inline:{}", std::process::id()),
        EnqueueOptions {
            run_id: options.run_id.clone(),
            actor: options.actor.clone(),
            reason: options.reason.clone(),
            priority: 0,
            timeout_ms: None,
            parent_run_id: options.parent_run_id.clone(),
            cron_schedule_id: None,
            script_name: None,
            omakure_version: app_meta::APP_VERSION.to_string(),
            trigger: crate::runs::RunTrigger::Manual,
            env_name: None,
            allowed_secret_refs: None,
            script_content_hash: None,
        },
    )
    .map_err(|err| -> Box<dyn Error> { err.into() })?;
    drop(conn);
    let mut execution_row = row.clone();
    execution_row.args_json = serde_json::to_string(&resolved_args.execution_args)
        .unwrap_or_else(|_| row.args_json.clone());
    let result = execute_with_heartbeat(&workspace, &execution_row, extra_env, None);

    let final_row = finalize_run(&workspace, &row.run_id, &result);

    if json_output {
        if let Some(row) = final_row {
            json::print_ok(row);
        }
    } else {
        print_human_run_output(&result);
    }
    if let Some(code) = failure_exit_code(&result) {
        std::process::exit(code);
    }
    Ok(())
}

struct PreparedRunInputs {
    extra_env: Vec<(String, String)>,
    resolved_args: crate::secrets::ResolvedArgs,
}

fn prepare_run_inputs(
    workspace: &Workspace,
    script_path: &std::path::Path,
    options: &RunArgs,
    json_output: bool,
) -> Result<PreparedRunInputs, (&'static str, String)> {
    // Layers 2 + 3 of the env-injection precedence table
    // (`docs/internal/env-injection-spec.md` §1): the active managed env, with the
    // optional CLI `--env-file` folded on top (env-file wins per key). The
    // reserved vars `OMAKURE_RUN_ID` / `OMAKURE_SCRIPTS_DIR` (layer 4) are
    // pushed after this inside `execute_with_heartbeat`, stay
    // non-overridable, and are therefore not visible to `$VAR` expansion in
    // `.conf` / `--env-file` values. A missing/unreadable `--env-file` is a
    // hard error.
    let extra_env = crate::adapters::environments::resolve_run_env(
        workspace.envs_dir(),
        options.env_file.as_deref(),
    )
    .map_err(|error| (codes::INVALID_ARGUMENT, error.to_string()))?;
    let direct_secrets = crate::secrets::parse_direct_secrets(&options.secrets)
        .map_err(|error| (codes::INVALID_ARGUMENT, error))?;
    let resolved_args = crate::secrets::resolve_args_with_direct_secrets(
        workspace,
        script_path,
        &options.args,
        &extra_env,
        &direct_secrets,
    )
    .map_err(|(field, message)| missing_required_field_error(&field, &message))?;
    // `--json` implies `--no-prompt`: agents must never block on a TTY.
    if options.no_prompt || json_output {
        check_required_fields(workspace, script_path, &resolved_args.persisted_args)
            .map_err(|(field, message)| missing_required_field_error(&field, &message))?;
    }
    Ok(PreparedRunInputs {
        extra_env,
        resolved_args,
    })
}

fn missing_required_field_error(field: &str, message: &str) -> (&'static str, String) {
    (
        codes::MISSING_REQUIRED_FIELD,
        format!("required field `{field}` is missing: {message}"),
    )
}

fn print_human_run_output(result: &ExecutionResult) {
    if !result.completion.stdout.trim().is_empty() {
        print!("{}", result.completion.stdout);
        if !result.completion.stdout.ends_with('\n') {
            println!();
        }
    }
    if !result.completion.stderr.trim().is_empty() {
        eprint!("{}", result.completion.stderr);
        if !result.completion.stderr.ends_with('\n') {
            eprintln!();
        }
    }
    if let Some(err) = &result.completion.error {
        eprintln!("error: {}", err);
    }
}

fn failure_exit_code(result: &ExecutionResult) -> Option<i32> {
    matches!(
        result.terminal,
        ExecutionTerminal::Failed
            | ExecutionTerminal::TimedOut
            | ExecutionTerminal::Errored
            | ExecutionTerminal::Cancelled
    )
    .then(|| result.completion.exit_code.unwrap_or(1))
}

fn finalize_run(
    workspace: &Workspace,
    run_id: &str,
    result: &crate::run_executor::ExecutionResult,
) -> Option<runs::RunRow> {
    let conn = runs::open(workspace).ok()?;
    let _ = match result.terminal {
        ExecutionTerminal::Completed => runs::complete(&conn, run_id, result.completion.clone()),
        ExecutionTerminal::Failed | ExecutionTerminal::Errored => {
            runs::fail(&conn, run_id, result.completion.clone())
        }
        ExecutionTerminal::TimedOut => runs::time_out(&conn, run_id, result.completion.clone()),
        ExecutionTerminal::Cancelled => {
            // The cancel transition was already recorded by an external
            // caller; just attach the captured output.
            runs::record_cancelled_output(&conn, run_id, result.completion.clone())
        }
    };
    runs::get_run(&conn, run_id).ok().flatten()
}

#[cfg(test)]
mod tests;

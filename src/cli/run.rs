//! `omakure run` — synchronous fast path for one script execution.
//!
//! `omakure run` writes through the same state machine as
//! `omakure queue worker`. The row is inserted in `state='running'` at
//! start (so `history list --state running` sees it immediately) and
//! transitions to `completed`/`failed`/`timed_out` on completion via
//! the shared [`crate::run_executor::execute_with_heartbeat`] helper.

use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::app_meta;
use crate::cli::args::RunArgs;
use crate::cli::json::{self, codes};
use crate::ports::ScriptRepository;
use crate::run_executor::{execute_with_heartbeat, ExecutionTerminal};
use crate::runs::{self, EnqueueOptions};
use crate::runtime::script_extensions;
use crate::workspace::Workspace;
use std::error::Error;
use std::path::{Path, PathBuf};

pub fn run(
    scripts_dir: PathBuf,
    options: RunArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;

    let script_path = match resolve_script_path(&options.script, workspace.root()) {
        Ok(path) => path,
        Err(err) => return emit_error(json_output, codes::NOT_FOUND, err.to_string()),
    };

    // `--json` implies `--no-prompt`: agents must never block on a TTY.
    let no_prompt = options.no_prompt || json_output;
    if no_prompt {
        if let Err((field, message)) =
            check_required_fields(&workspace, &script_path, &options.args)
        {
            return emit_error(
                json_output,
                codes::MISSING_REQUIRED_FIELD,
                format!("required field `{}` is missing: {}", field, message),
            );
        }
    }

    let canonical = std::fs::canonicalize(&script_path).unwrap_or_else(|_| script_path.clone());
    let canonical_str = canonical.to_string_lossy().to_string();
    let conn = runs::open(&workspace).map_err(|err| -> Box<dyn Error> { err.into() })?;
    let row = runs::start_inline(
        &conn,
        &canonical_str,
        &options.args,
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
        },
    )
    .map_err(|err| -> Box<dyn Error> { err.into() })?;
    drop(conn);

    let result = execute_with_heartbeat(&workspace, &row, vec![], None);

    let final_row = finalize_run(&workspace, &row.run_id, &result);

    if json_output {
        if let Some(row) = final_row {
            json::print_ok(row);
        }
        // Match the previous PR #8 surface: a failing script propagates
        // its exit code so the caller's CI/agent loop sees the right
        // signal even under --json.
        if matches!(
            result.terminal,
            ExecutionTerminal::Failed
                | ExecutionTerminal::TimedOut
                | ExecutionTerminal::Errored
                | ExecutionTerminal::Cancelled
        ) {
            std::process::exit(result.completion.exit_code.unwrap_or(1));
        }
        return Ok(());
    }

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
    if matches!(
        result.terminal,
        ExecutionTerminal::Failed
            | ExecutionTerminal::TimedOut
            | ExecutionTerminal::Errored
            | ExecutionTerminal::Cancelled
    ) {
        std::process::exit(result.completion.exit_code.unwrap_or(1));
    }
    Ok(())
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

/// Verify that every required field on the script's schema either has a
/// `--<field>` (or its `Arg` override) on the command line, or is
/// non-required. Returns `Err((field_name, message))` if any required
/// field is missing.
fn check_required_fields(
    workspace: &Workspace,
    script_path: &Path,
    args: &[String],
) -> Result<(), (String, String)> {
    let repo = FsWorkspaceRepository::new(workspace.root().to_path_buf());
    let schema = match repo.read_schema(script_path) {
        Ok(s) => s,
        // No schema means no required-field check is possible. Treat as
        // a permissive pass — the script may have its own validation.
        Err(_) => return Ok(()),
    };
    for field in &schema.fields {
        if !field.required.unwrap_or(false) {
            continue;
        }
        let arg_flag = field
            .arg
            .clone()
            .unwrap_or_else(|| format!("--{}", field.name));
        if !cli_args_contain_flag(args, &arg_flag) {
            return Err((
                field.name.clone(),
                format!("expected `{}` on the command line", arg_flag),
            ));
        }
    }
    Ok(())
}

fn cli_args_contain_flag(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|a| a == flag || a.starts_with(&format!("{}=", flag)))
}

fn emit_error(json_output: bool, code: &str, message: String) -> Result<(), Box<dyn Error>> {
    if json_output {
        json::print_err(code, message);
        std::process::exit(1);
    }
    Err(message.into())
}

pub(crate) fn resolve_script_path(
    script: &str,
    scripts_dir: &Path,
) -> Result<PathBuf, Box<dyn Error>> {
    let has_separator = script.contains('/') || script.contains('\\');
    let path = PathBuf::from(script);

    if path.is_absolute() {
        return resolve_with_extensions(path);
    }

    if has_separator {
        return resolve_with_extensions(scripts_dir.join(path));
    }

    resolve_with_extensions(scripts_dir.join(script))
}

fn resolve_with_extensions(path: PathBuf) -> Result<PathBuf, Box<dyn Error>> {
    if path.exists() {
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!("Script is not a file: {}", path.display()).into());
    }
    if path.extension().is_some() {
        return Err(format!("Script not found: {}", path.display()).into());
    }
    for ext in script_extensions() {
        let mut candidate = path.clone();
        candidate.set_extension(ext);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!("Script not found: {}", path.display()).into())
}

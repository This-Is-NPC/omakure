use crate::adapters::script_runner::MultiScriptRunner;
use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::app_meta;
use crate::cli::args::RunArgs;
use crate::cli::json::{self, codes};
use crate::ports::{ScriptRepository, ScriptRunOutput};
use crate::runs::{self, RunRow};
use crate::runtime::script_extensions;
use crate::use_cases::ScriptService;
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
        if let Err((field, message)) = check_required_fields(&workspace, &script_path, &options.args) {
            return emit_error(
                json_output,
                codes::MISSING_REQUIRED_FIELD,
                format!("required field `{}` is missing: {}", field, message),
            );
        }
    }

    let repo = Box::new(FsWorkspaceRepository::new(workspace.root().to_path_buf()));
    let runner = Box::new(MultiScriptRunner::new());
    let service = ScriptService::new(repo, runner);

    let started_at = runs::current_unix_ms();
    let run_result = service.run_script(&script_path, &options.args);
    let finished_at = runs::current_unix_ms();

    let row = build_run_row(
        &script_path,
        &options.args,
        &run_result,
        started_at,
        finished_at,
        &options,
    );
    record_row(&workspace, &row);

    if json_output {
        let exit_code = row.exit_code.unwrap_or(if row.success { 0 } else { 1 });
        json::print_ok(&row);
        if !row.success {
            std::process::exit(exit_code);
        }
        return Ok(());
    }

    match run_result {
        Ok(output) => {
            let success = output.success;
            let exit_code = output.exit_code.unwrap_or(1);
            print_output(&output);
            if !success {
                std::process::exit(exit_code);
            }
        }
        Err(err) => {
            eprintln!("{}", err);
            return Err(Box::new(err));
        }
    }

    Ok(())
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
    args.iter().any(|a| a == flag || a.starts_with(&format!("{}=", flag)))
}

fn emit_error(
    json_output: bool,
    code: &str,
    message: String,
) -> Result<(), Box<dyn Error>> {
    if json_output {
        json::print_err(code, message);
        std::process::exit(1);
    }
    Err(message.into())
}

/// Insert a row into the run log, ignoring all errors so persistence
/// failures never break a successful script run.
fn record_row(workspace: &Workspace, row: &RunRow) {
    if let Ok(conn) = runs::open(workspace) {
        let _ = runs::insert_run(&conn, row);
    }
}

fn build_run_row(
    script: &Path,
    args: &[String],
    result: &Result<ScriptRunOutput, crate::error::AppError>,
    started_at: i64,
    finished_at: i64,
    options: &RunArgs,
) -> RunRow {
    let canonical = std::fs::canonicalize(script).unwrap_or_else(|_| script.to_path_buf());
    let script_path = canonical.to_string_lossy().to_string();
    let args_json = serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string());
    let duration_ms = (finished_at - started_at).max(0);
    let omakure_version = app_meta::APP_VERSION.to_string();
    let run_id = options
        .run_id
        .clone()
        .unwrap_or_else(runs::generate_run_id);

    match result {
        Ok(output) => RunRow {
            run_id,
            script_path,
            script_name: None,
            args_json,
            actor: options.actor.clone(),
            reason: options.reason.clone(),
            started_at,
            finished_at,
            duration_ms,
            exit_code: output.exit_code,
            success: output.success,
            stdout: output.stdout.clone(),
            stderr: output.stderr.clone(),
            error: None,
            parent_run_id: options.parent_run_id.clone(),
            omakure_version,
        },
        Err(err) => RunRow {
            run_id,
            script_path,
            script_name: None,
            args_json,
            actor: options.actor.clone(),
            reason: options.reason.clone(),
            started_at,
            finished_at,
            duration_ms,
            exit_code: None,
            success: false,
            stdout: String::new(),
            stderr: String::new(),
            error: Some(err.to_string()),
            parent_run_id: options.parent_run_id.clone(),
            omakure_version,
        },
    }
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

fn print_output(output: &ScriptRunOutput) {
    if !output.stdout.trim().is_empty() {
        print!("{}", output.stdout);
        if !output.stdout.ends_with('\n') {
            println!();
        }
    }
    if !output.stderr.trim().is_empty() {
        eprint!("{}", output.stderr);
        if !output.stderr.ends_with('\n') {
            eprintln!();
        }
    }
}

//! `omakure history list|show|tail` — query the SQLite run log.

use crate::cli::args::{HistoryArgs, HistoryCommand, HistoryListArgs, HistoryShowArgs, HistoryTailArgs};
use crate::cli::json::{self, codes};
use crate::runs::{self, current_unix_ms, format_run_timestamp, RunFilters, RunRow};
use crate::workspace::Workspace;
use serde::Serialize;
use std::error::Error;
use std::path::PathBuf;

/// Compact run row used by `history list --json`. Stdout/stderr are
/// omitted to keep payloads small; agents that need them call
/// `history show <run_id>` instead.
#[derive(Debug, Serialize)]
pub struct CompactRun {
    pub run_id: String,
    pub script_path: String,
    pub script_name: Option<String>,
    pub args_json: String,
    pub actor: String,
    pub reason: Option<String>,
    pub started_at: i64,
    pub finished_at: i64,
    pub duration_ms: i64,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub error: Option<String>,
    pub parent_run_id: Option<String>,
    pub omakure_version: String,
}

impl From<RunRow> for CompactRun {
    fn from(r: RunRow) -> Self {
        CompactRun {
            run_id: r.run_id,
            script_path: r.script_path,
            script_name: r.script_name,
            args_json: r.args_json,
            actor: r.actor,
            reason: r.reason,
            started_at: r.started_at,
            finished_at: r.finished_at,
            duration_ms: r.duration_ms,
            exit_code: r.exit_code,
            success: r.success,
            error: r.error,
            parent_run_id: r.parent_run_id,
            omakure_version: r.omakure_version,
        }
    }
}

pub fn run(
    scripts_dir: PathBuf,
    args: HistoryArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(scripts_dir);
    workspace.ensure_layout()?;

    match args.command {
        HistoryCommand::List(opts) => list(&workspace, opts, json_output),
        HistoryCommand::Show(opts) => show(&workspace, opts, json_output),
        HistoryCommand::Tail(opts) => tail(&workspace, opts, json_output),
    }
}

fn list(
    workspace: &Workspace,
    opts: HistoryListArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let success = if opts.success {
        Some(true)
    } else if opts.failure {
        Some(false)
    } else {
        None
    };

    let now = current_unix_ms();
    let since_ms = match opts.since.as_deref().map(parse_duration_to_ms) {
        Some(Ok(d)) => Some(now - d),
        Some(Err(err)) => return emit_error(json_output, codes::INVALID_ARGUMENT, err),
        None => None,
    };
    let until_ms = match opts.until.as_deref().map(parse_duration_to_ms) {
        Some(Ok(d)) => Some(now - d),
        Some(Err(err)) => return emit_error(json_output, codes::INVALID_ARGUMENT, err),
        None => None,
    };

    let filters = RunFilters {
        script: opts.script,
        actor: opts.actor,
        since_ms,
        until_ms,
        success,
        limit: opts.limit,
    };

    let conn = open_or_error(workspace, json_output)?;
    let rows = match runs::query_runs(&conn, &filters) {
        Ok(rows) => rows,
        Err(err) => return emit_error(json_output, codes::INTERNAL, err),
    };

    if json_output {
        let compact: Vec<CompactRun> = rows.into_iter().map(CompactRun::from).collect();
        json::print_ok(compact);
        return Ok(());
    }

    if rows.is_empty() {
        println!("(no runs)");
        return Ok(());
    }
    for row in rows {
        let date = format_run_timestamp(row.started_at);
        let status = if row.error.is_some() {
            "ERROR".to_string()
        } else if row.success {
            "OK".to_string()
        } else {
            format!("FAIL({})", row.exit_code.unwrap_or(-1))
        };
        println!(
            "{}  {:>6}  {}  {}  {}",
            row.run_id, status, date, row.actor, row.script_path
        );
    }
    Ok(())
}

fn show(
    workspace: &Workspace,
    opts: HistoryShowArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let conn = open_or_error(workspace, json_output)?;
    let row = match runs::get_run(&conn, &opts.run_id) {
        Ok(Some(row)) => row,
        Ok(None) => {
            return emit_error(
                json_output,
                codes::NOT_FOUND,
                format!("run not found: {}", opts.run_id),
            );
        }
        Err(err) => return emit_error(json_output, codes::INTERNAL, err),
    };

    if json_output {
        json::print_ok(row);
        return Ok(());
    }

    println!("Run id: {}", row.run_id);
    println!("Script: {}", row.script_path);
    println!("Args: {}", row.args_json);
    println!("Actor: {}", row.actor);
    if let Some(reason) = &row.reason {
        println!("Reason: {}", reason);
    }
    println!("Started at: {}", format_run_timestamp(row.started_at));
    println!("Finished at: {}", format_run_timestamp(row.finished_at));
    println!("Duration: {} ms", row.duration_ms);
    println!("Exit code: {:?}", row.exit_code);
    println!("Success: {}", row.success);
    if !row.stdout.trim().is_empty() {
        println!("--- stdout ---\n{}", row.stdout.trim_end());
    }
    if !row.stderr.trim().is_empty() {
        println!("--- stderr ---\n{}", row.stderr.trim_end());
    }
    if let Some(error) = &row.error {
        println!("--- error ---\n{}", error.trim_end());
    }
    Ok(())
}

fn tail(
    workspace: &Workspace,
    opts: HistoryTailArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    if opts.follow {
        return emit_error(
            json_output,
            codes::NOT_IMPLEMENTED,
            "history tail --follow is not implemented in v1".into(),
        );
    }
    let list_opts = HistoryListArgs {
        script: None,
        actor: None,
        since: None,
        until: None,
        success: false,
        failure: false,
        limit: Some(opts.limit),
    };
    list(workspace, list_opts, json_output)
}

fn open_or_error(
    workspace: &Workspace,
    json_output: bool,
) -> Result<rusqlite::Connection, Box<dyn Error>> {
    match runs::open(workspace) {
        Ok(conn) => Ok(conn),
        Err(err) => Err(emit_error(json_output, codes::INTERNAL, err).unwrap_err()),
    }
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

/// Parse a relative-duration string like `30s`, `15m`, `2h`, `7d` into
/// milliseconds. Returns an error message string on parse failure.
pub fn parse_duration_to_ms(s: &str) -> Result<i64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty duration".into());
    }
    let (digits, unit) = s.split_at(s.len() - 1);
    let unit_char = unit.chars().next().ok_or("missing unit")?;
    let value: i64 = digits
        .parse()
        .map_err(|_| format!("invalid duration value: {}", s))?;
    let multiplier = match unit_char {
        's' => 1_000_i64,
        'm' => 60 * 1_000,
        'h' => 60 * 60 * 1_000,
        'd' => 24 * 60 * 60 * 1_000,
        _ => return Err(format!("invalid duration unit: {}", unit_char)),
    };
    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("duration overflow: {}", s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_seconds_minutes_hours_days() {
        assert_eq!(parse_duration_to_ms("30s").unwrap(), 30_000);
        assert_eq!(parse_duration_to_ms("15m").unwrap(), 900_000);
        assert_eq!(parse_duration_to_ms("2h").unwrap(), 7_200_000);
        assert_eq!(parse_duration_to_ms("7d").unwrap(), 7 * 24 * 3_600 * 1_000);
    }

    #[test]
    fn parse_duration_rejects_garbage() {
        assert!(parse_duration_to_ms("").is_err());
        assert!(parse_duration_to_ms("abc").is_err());
        assert!(parse_duration_to_ms("10x").is_err());
        assert!(parse_duration_to_ms("h").is_err());
    }
}

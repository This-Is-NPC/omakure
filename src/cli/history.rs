//! `omakure history list|show|tail|stats|traces` — query the SQLite run log.

use crate::cli::args::{
    HistoryArgs, HistoryCommand, HistoryListArgs, HistoryShowArgs, HistoryTailArgs,
    HistoryTracesArgs,
};
use crate::cli::json::{self, codes};
use crate::runs::{
    self, format_run_timestamp, query_traces, RunFilters, RunRow, RunState, RunStateSet, TraceLevel,
};
use crate::workspace::Workspace;
use serde::Serialize;
use std::error::Error;
use std::path::PathBuf;
use std::str::FromStr;

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
    pub state: String,
    pub priority: i64,
    pub enqueued_at: i64,
    pub worker_id: Option<String>,
    pub lease_until: Option<i64>,
    pub timeout_ms: Option<i64>,
    pub cron_schedule_id: Option<String>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub exit_code: Option<i32>,
    pub success: Option<bool>,
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
            state: r.state.as_str().to_string(),
            priority: r.priority,
            enqueued_at: r.enqueued_at,
            worker_id: r.worker_id,
            lease_until: r.lease_until,
            timeout_ms: r.timeout_ms,
            cron_schedule_id: r.cron_schedule_id,
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
        HistoryCommand::Stats => stats(&workspace, json_output),
        HistoryCommand::Traces(opts) => traces(&workspace, opts, json_output),
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

    let states = match resolve_state_filter(&opts.state, opts.state_set.as_deref()) {
        Ok(states) => states,
        Err(err) => return emit_error(json_output, codes::INVALID_ARGUMENT, err),
    };

    let now = runs::current_unix_ms();
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
        states,
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
        let date = format_run_timestamp(row.started_at.unwrap_or(row.enqueued_at));
        println!(
            "{}  {:>11}  {}  {}  {}",
            row.run_id,
            row.state.as_str(),
            date,
            row.actor,
            row.script_path
        );
    }
    Ok(())
}

/// Resolve the user-supplied `--state` and `--state-set` flags into a
/// concrete list of [`RunState`] values for [`RunFilters::states`].
///
/// Default (neither flag): the [`RunStateSet::Terminal`] set, so v0.1
/// callers see no behavior change.
fn resolve_state_filter(
    states: &[String],
    state_set: Option<&str>,
) -> Result<Vec<RunState>, String> {
    if !states.is_empty() && state_set.is_some() {
        return Err("--state and --state-set are mutually exclusive".to_string());
    }
    if let Some(set) = state_set {
        let parsed: RunStateSet = set.parse()?;
        return Ok(parsed.to_states());
    }
    if !states.is_empty() {
        let mut out = Vec::with_capacity(states.len());
        for s in states {
            out.push(s.parse::<RunState>()?);
        }
        return Ok(out);
    }
    Ok(RunStateSet::Terminal.to_states())
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
    println!("State: {}", row.state.as_str());
    println!("Args: {}", row.args_json);
    println!("Actor: {}", row.actor);
    if let Some(reason) = &row.reason {
        println!("Reason: {}", reason);
    }
    println!("Priority: {}", row.priority);
    println!("Enqueued at: {}", format_run_timestamp(row.enqueued_at));
    if let Some(started_at) = row.started_at {
        println!("Started at: {}", format_run_timestamp(started_at));
    }
    if let Some(finished_at) = row.finished_at {
        println!("Finished at: {}", format_run_timestamp(finished_at));
    }
    if let Some(duration_ms) = row.duration_ms {
        println!("Duration: {} ms", duration_ms);
    }
    if let Some(exit_code) = row.exit_code {
        println!("Exit code: {}", exit_code);
    }
    if let Some(success) = row.success {
        println!("Success: {}", success);
    }
    if let Some(timeout_ms) = row.timeout_ms {
        println!("Timeout: {} ms", timeout_ms);
    }
    if let Some(worker_id) = &row.worker_id {
        println!("Worker: {}", worker_id);
    }
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
        state: Vec::new(),
        state_set: None,
    };
    list(workspace, list_opts, json_output)
}

fn stats(workspace: &Workspace, json_output: bool) -> Result<(), Box<dyn Error>> {
    let conn = open_or_error(workspace, json_output)?;
    let stats = match runs::stats(&conn) {
        Ok(s) => s,
        Err(err) => return emit_error(json_output, codes::INTERNAL, err),
    };
    if json_output {
        json::print_ok(stats);
        return Ok(());
    }
    println!("Total runs: {}", stats.total);
    println!("--- by state ---");
    let mut states: Vec<_> = stats.counts_by_state.iter().collect();
    states.sort_by(|a, b| a.0.cmp(b.0));
    for (state, count) in states {
        println!("  {:<12} {}", state, count);
    }
    println!("--- by actor ---");
    let mut actors: Vec<_> = stats.counts_by_actor.iter().collect();
    actors.sort_by(|a, b| a.0.cmp(b.0));
    for (actor, count) in actors {
        println!("  {:<12} {}", actor, count);
    }
    Ok(())
}

fn traces(
    workspace: &Workspace,
    opts: HistoryTracesArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let level_min = match opts.level.as_deref() {
        None => None,
        Some(s) => match TraceLevel::from_str(s) {
            Ok(l) => Some(l),
            Err(err) => return emit_error(json_output, codes::INVALID_ARGUMENT, err),
        },
    };

    let conn = open_or_error(workspace, json_output)?;
    let traces = match query_traces(&conn, &opts.run_id, level_min, opts.since_sequence) {
        Ok(rows) => rows,
        Err(err) if err.starts_with("not_found") => {
            return emit_error(
                json_output,
                codes::NOT_FOUND,
                format!("run not found: {}", opts.run_id),
            );
        }
        Err(err) => return emit_error(json_output, codes::INTERNAL, err),
    };

    if json_output {
        json::print_ok(traces);
        return Ok(());
    }

    if traces.is_empty() {
        println!("(no traces)");
        return Ok(());
    }
    for t in traces {
        let when = format_run_timestamp(t.timestamp);
        let data = t.data_json.as_deref().unwrap_or("");
        if data.is_empty() {
            println!("[{}] #{:<4} {:<5} {}", when, t.sequence, t.level, t.message);
        } else {
            println!(
                "[{}] #{:<4} {:<5} {}  {}",
                when, t.sequence, t.level, t.message, data
            );
        }
    }
    Ok(())
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

fn emit_error(json_output: bool, code: &str, message: String) -> Result<(), Box<dyn Error>> {
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

    #[test]
    fn resolve_state_filter_default_is_terminal_set() {
        let resolved = resolve_state_filter(&[], None).unwrap();
        let expected = RunStateSet::Terminal.to_states();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn resolve_state_filter_state_set_in_flight() {
        let resolved = resolve_state_filter(&[], Some("in_flight")).unwrap();
        assert!(resolved.contains(&RunState::Queued));
        assert!(resolved.contains(&RunState::Running));
        assert!(!resolved.contains(&RunState::Completed));
    }

    #[test]
    fn resolve_state_filter_explicit_states() {
        let resolved = resolve_state_filter(&["queued".into(), "running".into()], None).unwrap();
        assert_eq!(resolved, vec![RunState::Queued, RunState::Running]);
    }

    #[test]
    fn resolve_state_filter_invalid_value_returns_error() {
        let err = resolve_state_filter(&["bogus".into()], None).unwrap_err();
        assert!(err.contains("invalid run state"));
    }

    #[test]
    fn resolve_state_filter_mutually_exclusive_with_state_set() {
        let err = resolve_state_filter(&["queued".into()], Some("terminal")).unwrap_err();
        assert!(err.contains("mutually exclusive"));
    }

    #[test]
    fn resolve_state_filter_invalid_state_set_returns_error() {
        let err = resolve_state_filter(&[], Some("bogus")).unwrap_err();
        assert!(err.contains("invalid state-set"));
    }
}

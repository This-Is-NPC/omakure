//! `omakure history list|show|tail|stats|traces` — query the SQLite run log.

use crate::cli::args::{
    HistoryArgs, HistoryCommand, HistoryListArgs, HistoryShowArgs, HistoryTailArgs,
    HistoryTracesArgs,
};
use crate::cli::json::{self, codes};
use crate::operations::core::{self, ListRunsRequest, ListTracesRequest, ShowRunRequest};
use crate::operations::{OperationError, OperationErrorCode};
use crate::runs::{self, format_run_timestamp, RunRow, RunStats, TraceRow};
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
    pub state: String,
    pub priority: i64,
    pub enqueued_at: i64,
    pub worker_id: Option<String>,
    pub lease_until: Option<i64>,
    pub timeout_ms: Option<i64>,
    pub cron_schedule_id: Option<String>,
    pub trigger: String,
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
            trigger: r.trigger.as_str().to_string(),
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

    let request = ListRunsRequest {
        script: opts.script,
        actor: opts.actor,
        since_ms,
        until_ms,
        success,
        limit: opts.limit,
        states: opts.state,
        state_set: opts.state_set,
    };

    let rows = match core::list_runs(workspace, request) {
        Ok(rows) => rows,
        Err(err) => return emit_operation_error(json_output, err),
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
        println!("{}", format_list_row(&row));
    }
    Ok(())
}

fn format_list_row(row: &RunRow) -> String {
    let date = format_run_timestamp(row.started_at.unwrap_or(row.enqueued_at));
    format!(
        "{}  {:>11}  {}  {}  {}",
        row.run_id,
        row.state.as_str(),
        date,
        row.actor,
        row.script_path
    )
}

/// Resolve the user-supplied `--state` and `--state-set` flags into concrete
/// run states. Retained as characterization coverage for the operation-backed
/// adapter migration.
///
/// Default (neither flag): the terminal set, so v0.1 callers see no behavior
/// change.
#[cfg(test)]
fn resolve_state_filter(
    states: &[String],
    state_set: Option<&str>,
) -> Result<Vec<crate::runs::RunState>, String> {
    if !states.is_empty() && state_set.is_some() {
        return Err("--state and --state-set are mutually exclusive".to_string());
    }
    if let Some(set) = state_set {
        let parsed: crate::runs::RunStateSet = set.parse()?;
        return Ok(parsed.to_states());
    }
    if !states.is_empty() {
        let mut out = Vec::with_capacity(states.len());
        for s in states {
            out.push(s.parse::<crate::runs::RunState>()?);
        }
        return Ok(out);
    }
    Ok(crate::runs::RunStateSet::Terminal.to_states())
}

fn show(
    workspace: &Workspace,
    opts: HistoryShowArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let row = match core::show_run(
        workspace,
        ShowRunRequest {
            run_id: opts.run_id,
        },
    ) {
        Ok(row) => row,
        Err(err) => return emit_operation_error(json_output, err),
    };

    if json_output {
        json::print_ok(row);
        return Ok(());
    }

    for line in format_show_lines(&row) {
        println!("{}", line);
    }
    Ok(())
}

fn format_show_lines(row: &RunRow) -> Vec<String> {
    let mut lines = vec![
        format!("Run id: {}", row.run_id),
        format!("Script: {}", row.script_path),
        format!("State: {}", row.state.as_str()),
        format!("Args: {}", row.args_json),
        format!("Actor: {}", row.actor),
    ];
    if let Some(reason) = &row.reason {
        lines.push(format!("Reason: {}", reason));
    }
    lines.push(format!("Priority: {}", row.priority));
    lines.push(format!(
        "Enqueued at: {}",
        format_run_timestamp(row.enqueued_at)
    ));
    if let Some(started_at) = row.started_at {
        lines.push(format!("Started at: {}", format_run_timestamp(started_at)));
    }
    if let Some(finished_at) = row.finished_at {
        lines.push(format!(
            "Finished at: {}",
            format_run_timestamp(finished_at)
        ));
    }
    if let Some(duration_ms) = row.duration_ms {
        lines.push(format!("Duration: {} ms", duration_ms));
    }
    if let Some(exit_code) = row.exit_code {
        lines.push(format!("Exit code: {}", exit_code));
    }
    if let Some(success) = row.success {
        lines.push(format!("Success: {}", success));
    }
    if let Some(timeout_ms) = row.timeout_ms {
        lines.push(format!("Timeout: {} ms", timeout_ms));
    }
    if let Some(worker_id) = &row.worker_id {
        lines.push(format!("Worker: {}", worker_id));
    }
    if !row.stdout.trim().is_empty() {
        lines.push(format!("--- stdout ---\n{}", row.stdout.trim_end()));
    }
    if !row.stderr.trim().is_empty() {
        lines.push(format!("--- stderr ---\n{}", row.stderr.trim_end()));
    }
    if let Some(error) = &row.error {
        lines.push(format!("--- error ---\n{}", error.trim_end()));
    }
    lines
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
    let stats = match core::run_stats(workspace) {
        Ok(s) => s,
        Err(err) => return emit_operation_error(json_output, err),
    };
    if json_output {
        json::print_ok(stats);
        return Ok(());
    }
    for line in format_stats_lines(&stats) {
        println!("{}", line);
    }
    Ok(())
}

fn format_stats_lines(stats: &RunStats) -> Vec<String> {
    let mut lines = vec![
        format!("Total runs: {}", stats.total),
        "--- by state ---".to_string(),
    ];
    let mut states: Vec<_> = stats.counts_by_state.iter().collect();
    states.sort_by(|a, b| a.0.cmp(b.0));
    for (state, count) in states {
        lines.push(format!("  {:<12} {}", state, count));
    }
    lines.push("--- by actor ---".to_string());
    let mut actors: Vec<_> = stats.counts_by_actor.iter().collect();
    actors.sort_by(|a, b| a.0.cmp(b.0));
    for (actor, count) in actors {
        lines.push(format!("  {:<12} {}", actor, count));
    }
    lines
}

fn traces(
    workspace: &Workspace,
    opts: HistoryTracesArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let run_id = opts.run_id;
    let traces = match core::list_traces(
        workspace,
        ListTracesRequest {
            run_id: run_id.clone(),
            level: opts.level,
            since_sequence: opts.since_sequence,
        },
    ) {
        Ok(rows) => rows,
        Err(err) if err.code == OperationErrorCode::NotFound => {
            return emit_error(
                json_output,
                codes::NOT_FOUND,
                format!("run not found: {}", run_id),
            );
        }
        Err(err) => return emit_operation_error(json_output, err),
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
        println!("{}", format_trace_row(&t));
    }
    Ok(())
}

fn format_trace_row(trace: &TraceRow) -> String {
    let when = format_run_timestamp(trace.timestamp);
    let data = trace.data_json.as_deref().unwrap_or("");
    if data.is_empty() {
        format!(
            "[{}] #{:<4} {:<5} {}",
            when, trace.sequence, trace.level, trace.message
        )
    } else {
        format!(
            "[{}] #{:<4} {:<5} {}  {}",
            when, trace.sequence, trace.level, trace.message, data
        )
    }
}

fn emit_error(json_output: bool, code: &str, message: String) -> Result<(), Box<dyn Error>> {
    if json_output {
        json::print_err(code, message);
        std::process::exit(1);
    }
    Err(message.into())
}

fn emit_operation_error(json_output: bool, err: OperationError) -> Result<(), Box<dyn Error>> {
    let code = match err.code {
        OperationErrorCode::InvalidInput => codes::INVALID_ARGUMENT,
        OperationErrorCode::NotFound => codes::NOT_FOUND,
        _ => codes::INTERNAL,
    };
    emit_error(json_output, code, err.message)
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
    use crate::runs::{RunState, RunStateSet};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn sample_row() -> RunRow {
        RunRow {
            run_id: "rid-1".into(),
            script_path: "/scripts/deploy.sh".into(),
            script_name: Some("deploy".into()),
            args_json: r#"["--target","prod"]"#.into(),
            actor: "ai".into(),
            reason: Some("ship it".into()),
            state: RunState::Completed,
            priority: 7,
            enqueued_at: 1_700_000_000_000,
            worker_id: Some("worker-1".into()),
            lease_until: Some(1_700_000_000_500),
            timeout_ms: Some(30_000),
            cron_schedule_id: None,
            trigger: crate::runs::RunTrigger::Manual,
            started_at: Some(1_700_000_000_100),
            finished_at: Some(1_700_000_000_200),
            duration_ms: Some(100),
            exit_code: Some(0),
            success: Some(true),
            stdout: "done\n".into(),
            stderr: "warning\n".into(),
            error: Some("boom\n".into()),
            parent_run_id: Some("parent-1".into()),
            omakure_version: "test".into(),
        }
    }

    fn temp_workspace() -> Workspace {
        let tmp = TempDir::new().unwrap();
        let root = tmp.keep();
        let workspace = Workspace::new(root);
        workspace.ensure_layout().unwrap();
        workspace
    }

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

    #[test]
    fn compact_run_from_preserves_fields() {
        let row = sample_row();
        let compact = CompactRun::from(row.clone());

        assert_eq!(compact.run_id, row.run_id);
        assert_eq!(compact.script_path, row.script_path);
        assert_eq!(compact.actor, row.actor);
        assert_eq!(compact.state, "completed");
        assert_eq!(compact.success, Some(true));
        assert_eq!(compact.error, Some("boom\n".to_string()));
    }

    #[test]
    fn format_list_row_includes_state_actor_and_path() {
        let row = sample_row();
        let line = format_list_row(&row);

        assert!(line.contains("rid-1"));
        assert!(line.contains("completed"));
        assert!(line.contains("ai"));
        assert!(line.contains("/scripts/deploy.sh"));
    }

    #[test]
    fn format_show_lines_includes_optional_sections_and_trimmed_output() {
        let row = sample_row();
        let lines = format_show_lines(&row);
        let joined = lines.join("\n");

        assert!(joined.contains("Run id: rid-1"));
        assert!(joined.contains("Reason: ship it"));
        assert!(joined.contains("Worker: worker-1"));
        assert!(joined.contains("--- stdout ---\ndone"));
        assert!(joined.contains("--- stderr ---\nwarning"));
        assert!(joined.contains("--- error ---\nboom"));
    }

    #[test]
    fn format_show_lines_omits_empty_optional_sections() {
        let mut row = sample_row();
        row.reason = None;
        row.worker_id = None;
        row.timeout_ms = None;
        row.stdout.clear();
        row.stderr.clear();
        row.error = None;

        let joined = format_show_lines(&row).join("\n");

        assert!(!joined.contains("Reason:"));
        assert!(!joined.contains("Worker:"));
        assert!(!joined.contains("Timeout:"));
        assert!(!joined.contains("--- stdout ---"));
        assert!(!joined.contains("--- stderr ---"));
        assert!(!joined.contains("--- error ---"));
    }

    #[test]
    fn format_stats_lines_sorts_state_and_actor_names() {
        let stats = RunStats {
            total: 3,
            counts_by_state: HashMap::from([
                ("running".to_string(), 1),
                ("completed".to_string(), 2),
            ]),
            counts_by_actor: HashMap::from([("human".to_string(), 2), ("ai".to_string(), 1)]),
        };

        let lines = format_stats_lines(&stats);

        assert_eq!(lines[0], "Total runs: 3");
        assert_eq!(lines[1], "--- by state ---");
        assert!(lines[2].contains("completed"));
        assert!(lines[3].contains("running"));
        assert_eq!(lines[4], "--- by actor ---");
        assert!(lines[5].contains("ai"));
        assert!(lines[6].contains("human"));
    }

    #[test]
    fn format_trace_row_handles_with_and_without_data() {
        let no_data = TraceRow {
            trace_id: 1,
            run_id: "rid-1".into(),
            timestamp: 1_700_000_000_000,
            sequence: 1,
            level: "info".into(),
            message: "first".into(),
            data_json: None,
        };
        let with_data = TraceRow {
            trace_id: 2,
            run_id: "rid-1".into(),
            timestamp: 1_700_000_000_001,
            sequence: 2,
            level: "warn".into(),
            message: "second".into(),
            data_json: Some(r#"{"k":1}"#.into()),
        };

        let first = format_trace_row(&no_data);
        let second = format_trace_row(&with_data);

        assert!(first.contains("info"));
        assert!(first.contains("first"));
        assert!(!first.contains("{\"k\":1}"));
        assert!(second.contains("warn"));
        assert!(second.contains("second"));
        assert!(second.contains(r#"{"k":1}"#));
    }

    #[test]
    fn list_rejects_invalid_since_before_opening_db() {
        let workspace = temp_workspace();
        let err = list(
            &workspace,
            HistoryListArgs {
                script: None,
                actor: None,
                since: Some("nope".into()),
                until: None,
                success: false,
                failure: false,
                limit: None,
                state: Vec::new(),
                state_set: None,
            },
            false,
        )
        .unwrap_err();

        assert!(err.to_string().contains("invalid duration"));
    }

    #[test]
    fn tail_follow_returns_not_implemented_error() {
        let workspace = temp_workspace();
        let err = tail(
            &workspace,
            HistoryTailArgs {
                limit: 10,
                follow: true,
            },
            false,
        )
        .unwrap_err();

        assert!(err.to_string().contains("not implemented"));
    }

    fn enqueue_one(workspace: &Workspace) -> String {
        use crate::runs::EnqueueOptions;
        let conn = runs::open(workspace).unwrap();
        let row = runs::enqueue(
            &conn,
            "/scripts/x.sh",
            &[],
            EnqueueOptions {
                actor: "human".into(),
                omakure_version: "test".into(),
                ..Default::default()
            },
        )
        .unwrap();
        row.run_id
    }

    #[test]
    fn run_dispatches_to_subcommands() {
        let workspace = temp_workspace();
        let _id = enqueue_one(&workspace);

        let scripts_dir = workspace.root().to_path_buf();
        run(
            scripts_dir.clone(),
            HistoryArgs {
                command: HistoryCommand::List(HistoryListArgs {
                    script: None,
                    actor: None,
                    since: None,
                    until: None,
                    success: false,
                    failure: false,
                    limit: None,
                    state: Vec::new(),
                    state_set: Some("all".into()),
                }),
            },
            false,
        )
        .unwrap();

        run(
            scripts_dir.clone(),
            HistoryArgs {
                command: HistoryCommand::Stats,
            },
            false,
        )
        .unwrap();

        run(
            scripts_dir.clone(),
            HistoryArgs {
                command: HistoryCommand::Tail(HistoryTailArgs {
                    limit: 5,
                    follow: false,
                }),
            },
            true,
        )
        .unwrap();
    }

    #[test]
    fn list_human_format_prints_runs_and_no_runs() {
        let workspace = temp_workspace();
        // No rows yet — prints "(no runs)".
        list(
            &workspace,
            HistoryListArgs {
                script: None,
                actor: None,
                since: None,
                until: None,
                success: false,
                failure: false,
                limit: None,
                state: Vec::new(),
                state_set: Some("all".into()),
            },
            false,
        )
        .unwrap();

        let _id = enqueue_one(&workspace);
        list(
            &workspace,
            HistoryListArgs {
                script: None,
                actor: None,
                since: None,
                until: None,
                success: false,
                failure: false,
                limit: Some(10),
                state: Vec::new(),
                state_set: Some("all".into()),
            },
            false,
        )
        .unwrap();
    }

    #[test]
    fn list_with_success_failure_filters() {
        let workspace = temp_workspace();
        let _id = enqueue_one(&workspace);
        list(
            &workspace,
            HistoryListArgs {
                script: None,
                actor: None,
                since: Some("1h".into()),
                until: None,
                success: true,
                failure: false,
                limit: None,
                state: Vec::new(),
                state_set: Some("all".into()),
            },
            true,
        )
        .unwrap();
        list(
            &workspace,
            HistoryListArgs {
                script: None,
                actor: None,
                since: None,
                until: Some("1h".into()),
                success: false,
                failure: true,
                limit: None,
                state: Vec::new(),
                state_set: Some("all".into()),
            },
            false,
        )
        .unwrap();
    }

    #[test]
    fn list_rejects_invalid_until_value() {
        let workspace = temp_workspace();
        let err = list(
            &workspace,
            HistoryListArgs {
                script: None,
                actor: None,
                since: None,
                until: Some("nope".into()),
                success: false,
                failure: false,
                limit: None,
                state: Vec::new(),
                state_set: None,
            },
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid duration"));
    }

    #[test]
    fn list_rejects_invalid_state_value() {
        let workspace = temp_workspace();
        let err = list(
            &workspace,
            HistoryListArgs {
                script: None,
                actor: None,
                since: None,
                until: None,
                success: false,
                failure: false,
                limit: None,
                state: vec!["bogus".into()],
                state_set: None,
            },
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid run state"));
    }

    #[test]
    fn show_returns_not_found_for_unknown_id() {
        let workspace = temp_workspace();
        let err = show(
            &workspace,
            HistoryShowArgs {
                run_id: "nope".into(),
            },
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("run not found"));
    }

    #[test]
    fn show_human_and_json_formats_succeed() {
        let workspace = temp_workspace();
        let id = enqueue_one(&workspace);
        show(&workspace, HistoryShowArgs { run_id: id.clone() }, false).unwrap();
        show(&workspace, HistoryShowArgs { run_id: id }, true).unwrap();
    }

    #[test]
    fn stats_human_and_json() {
        let workspace = temp_workspace();
        let _id = enqueue_one(&workspace);
        stats(&workspace, false).unwrap();
        stats(&workspace, true).unwrap();
    }

    #[test]
    fn traces_for_unknown_run_returns_not_found() {
        let workspace = temp_workspace();
        let err = traces(
            &workspace,
            HistoryTracesArgs {
                run_id: "ghost".into(),
                level: None,
                since_sequence: None,
            },
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("run not found"));
    }

    #[test]
    fn traces_returns_empty_for_existing_run() {
        let workspace = temp_workspace();
        let id = enqueue_one(&workspace);
        traces(
            &workspace,
            HistoryTracesArgs {
                run_id: id.clone(),
                level: None,
                since_sequence: None,
            },
            false,
        )
        .unwrap();
        traces(
            &workspace,
            HistoryTracesArgs {
                run_id: id,
                level: Some("info".into()),
                since_sequence: Some(0),
            },
            true,
        )
        .unwrap();
    }

    #[test]
    fn traces_rejects_invalid_level_before_opening_db() {
        let workspace = temp_workspace();
        let err = traces(
            &workspace,
            HistoryTracesArgs {
                run_id: "rid-1".into(),
                level: Some("verbose".into()),
                since_sequence: None,
            },
            false,
        )
        .unwrap_err();

        assert!(err.to_string().contains("invalid trace level"));
    }
}

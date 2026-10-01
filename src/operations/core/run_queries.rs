use super::types::{
    CancelRunRequest, DeadLetterRunRequest, ListRunsRequest, ListTracesRequest, ShowRunRequest,
};
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::runs::{
    self, RunFilters, RunRow, RunState, RunStateSet, RunStats, RunsError, TraceLevel, TraceRow,
};
use crate::workspace::Workspace;
use std::str::FromStr;

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
    runs::query_runs(&conn, &filters).map_err(io_error_runs)
}

pub fn show_run(workspace: &Workspace, request: ShowRunRequest) -> OperationResult<RunRow> {
    let conn = runs::open(workspace).map_err(io_error_string)?;
    match runs::get_run(&conn, &request.run_id).map_err(io_error_runs)? {
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
        .map_err(map_trace_error)
}

pub fn queue_stats(workspace: &Workspace) -> OperationResult<RunStats> {
    run_stats(workspace)
}

pub fn run_stats(workspace: &Workspace) -> OperationResult<RunStats> {
    let conn = runs::open(workspace).map_err(io_error_string)?;
    runs::stats(&conn).map_err(io_error_runs)
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

pub(super) fn resolve_states(
    states: &[String],
    state_set: Option<&str>,
) -> OperationResult<Vec<RunState>> {
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

pub(super) fn require_run(conn: &rusqlite::Connection, run_id: &str) -> OperationResult<()> {
    match runs::get_run(conn, run_id).map_err(io_error_runs)? {
        Some(_) => Ok(()),
        None => Err(OperationError::new(
            OperationErrorCode::NotFound,
            format!("run not found: {run_id}"),
        )),
    }
}

pub(super) fn map_transition_error(error: RunsError) -> OperationError {
    let code = match error {
        RunsError::TerminalState(_) | RunsError::DeadLetterIneligible(_) => {
            OperationErrorCode::Conflict
        }
        _ => OperationErrorCode::IoFailed,
    };
    OperationError::new(code, error.to_string())
}

fn map_trace_error(error: RunsError) -> OperationError {
    let code = match &error {
        RunsError::NotFound(_) => OperationErrorCode::NotFound,
        _ => OperationErrorCode::IoFailed,
    };
    OperationError::new(code, error.to_string())
}

fn invalid_input(message: String) -> OperationError {
    OperationError::new(OperationErrorCode::InvalidInput, message)
}

fn io_error_runs(error: RunsError) -> OperationError {
    io_error_string(error.to_string())
}

pub(super) fn io_error_string(message: String) -> OperationError {
    OperationError::new(OperationErrorCode::IoFailed, message)
}

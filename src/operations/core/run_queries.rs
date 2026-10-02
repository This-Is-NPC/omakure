use super::types::{
    CancelRunRequest, DeadLetterRunRequest, ListRunsRequest, ListTracesRequest, ShowRunRequest,
};
use crate::operations::{OperationError, OperationErrorCode, OperationResult};
use crate::runs::{
    RunFilters, RunRow, RunState, RunStateSet, RunStats, RunStore, RunsError, TraceLevel, TraceRow,
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
    let store = RunStore::open(workspace).map_err(io_error_runs)?;
    store.query_runs(&filters).map_err(io_error_runs)
}

pub fn show_run(workspace: &Workspace, request: ShowRunRequest) -> OperationResult<RunRow> {
    let store = RunStore::open(workspace).map_err(io_error_runs)?;
    store
        .get_run_required(&request.run_id)
        .map_err(map_required_run_error)
}

pub fn list_traces(
    workspace: &Workspace,
    request: ListTracesRequest,
) -> OperationResult<Vec<TraceRow>> {
    let store = RunStore::open(workspace).map_err(io_error_runs)?;
    let level = match request.level.as_deref() {
        Some(level) => Some(TraceLevel::from_str(level).map_err(invalid_input)?),
        None => None,
    };
    store
        .query_traces(&request.run_id, level, request.since_sequence)
        .map_err(map_trace_error)
}

pub fn queue_stats(workspace: &Workspace) -> OperationResult<RunStats> {
    run_stats(workspace)
}

pub fn run_stats(workspace: &Workspace) -> OperationResult<RunStats> {
    let store = RunStore::open(workspace).map_err(io_error_runs)?;
    store.stats().map_err(io_error_runs)
}

pub fn cancel_run(workspace: &Workspace, request: CancelRunRequest) -> OperationResult<RunRow> {
    let store = RunStore::open(workspace).map_err(io_error_runs)?;
    store
        .get_run_required(&request.run_id)
        .map_err(map_required_run_error)?;
    let row = store
        .cancel(&request.run_id, request.reason)
        .map_err(map_transition_error)?;
    reconcile_workflow_terminal(&store, &row)?;
    Ok(row)
}

pub fn dead_letter_run(
    workspace: &Workspace,
    request: DeadLetterRunRequest,
) -> OperationResult<RunRow> {
    let store = RunStore::open(workspace).map_err(io_error_runs)?;
    store
        .get_run_required(&request.run_id)
        .map_err(map_required_run_error)?;
    let row = store
        .dead_letter(&request.run_id, request.reason)
        .map_err(map_transition_error)?;
    reconcile_workflow_terminal(&store, &row)?;
    Ok(row)
}

fn reconcile_workflow_terminal(store: &RunStore, row: &RunRow) -> OperationResult<()> {
    if row.trigger == crate::runs::RunTrigger::Workflow {
        store
            .advance_workflow_for_run(&row.run_id)
            .map_err(io_error_runs)?;
    }
    Ok(())
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

fn map_required_run_error(error: RunsError) -> OperationError {
    match error {
        RunsError::RunNotFound(_) => {
            OperationError::new(OperationErrorCode::NotFound, error.to_string())
        }
        other => io_error_runs(other),
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

pub(super) fn io_error_runs(error: RunsError) -> OperationError {
    io_error_string(error.to_string())
}

pub(super) fn io_error_string(message: String) -> OperationError {
    OperationError::new(OperationErrorCode::IoFailed, message)
}

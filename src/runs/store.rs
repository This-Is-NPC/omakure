use super::enqueue::EnqueueOptions;
use super::lifecycle;
use super::open;
use super::query::{self, RunFilters, RunRow, RunStats};
use super::trace::{self, TraceLevel, TraceRow};
use super::workflow::{WorkflowRun, WorkflowSnapshot};
use super::{ClaimFilters, RunCompletion, RunsError};
use crate::workspace::Workspace;
use rusqlite::Connection;

/// An opaque runs database handle for protocol-neutral operations.
pub(crate) struct RunStore {
    connection: Connection,
}

impl RunStore {
    pub(crate) fn open(workspace: &Workspace) -> Result<Self, RunsError> {
        Ok(Self {
            connection: open::open(workspace)?,
        })
    }

    pub(crate) fn enqueue(
        &self,
        script_path: &str,
        args: &[String],
        options: EnqueueOptions,
    ) -> Result<RunRow, RunsError> {
        super::enqueue(&self.connection, script_path, args, options)
    }

    pub(crate) fn start_workflow(
        &self,
        snapshot: WorkflowSnapshot,
        actor: &str,
    ) -> Result<WorkflowRun, RunsError> {
        super::start_workflow(&self.connection, snapshot, actor)
    }

    pub(crate) fn get_workflow(&self, workflow_id: &str) -> Result<Option<WorkflowRun>, RunsError> {
        super::get_workflow(&self.connection, workflow_id)
    }

    pub(crate) fn advance_workflow_for_run(
        &self,
        run_id: &str,
    ) -> Result<Option<WorkflowRun>, RunsError> {
        super::advance_workflow_for_run(&self.connection, run_id)
    }

    pub(crate) fn recover_workflows(&self) -> Result<Vec<WorkflowRun>, RunsError> {
        super::recover_workflows(&self.connection)
    }

    pub(crate) fn enqueue_cue(
        &mut self,
        script_path: &str,
        args: &[String],
        options: EnqueueOptions,
    ) -> Result<RunRow, RunsError> {
        super::enqueue_cue(&mut self.connection, script_path, args, options)
    }

    pub(crate) fn start_inline(
        &self,
        script_path: &str,
        args: &[String],
        worker_id: &str,
        options: EnqueueOptions,
    ) -> Result<RunRow, RunsError> {
        super::start_inline(&self.connection, script_path, args, worker_id, options)
    }

    pub(crate) fn query_runs(&self, filters: &RunFilters) -> Result<Vec<RunRow>, RunsError> {
        query::query_runs(&self.connection, filters)
    }

    pub(crate) fn get_run_required(&self, run_id: &str) -> Result<RunRow, RunsError> {
        query::get_run_required(&self.connection, run_id)
    }

    pub(crate) fn get_run(&self, run_id: &str) -> Result<Option<RunRow>, RunsError> {
        super::get_run(&self.connection, run_id)
    }

    pub(crate) fn query_traces(
        &self,
        run_id: &str,
        level: Option<TraceLevel>,
        since_sequence: Option<i64>,
    ) -> Result<Vec<TraceRow>, RunsError> {
        trace::query_traces(&self.connection, run_id, level, since_sequence)
    }

    pub(crate) fn insert_trace(
        &mut self,
        run_id: &str,
        level: TraceLevel,
        message: &str,
        data_json: Option<&str>,
    ) -> Result<TraceRow, RunsError> {
        super::insert_trace(&mut self.connection, run_id, level, message, data_json)
    }

    pub(crate) fn stats(&self) -> Result<RunStats, RunsError> {
        query::stats(&self.connection)
    }

    pub(crate) fn cancel(&self, run_id: &str, reason: Option<String>) -> Result<RunRow, RunsError> {
        lifecycle::cancel(&self.connection, run_id, reason, None)
    }

    pub(crate) fn dead_letter(
        &self,
        run_id: &str,
        reason: Option<String>,
    ) -> Result<RunRow, RunsError> {
        lifecycle::dead_letter(&self.connection, run_id, reason)
    }

    pub(crate) fn cancel_cue_runs_for_actor(&self, actor: &str) -> Result<Vec<String>, RunsError> {
        super::cancel_cue_runs_for_actor(&self.connection, actor)
    }

    pub(crate) fn recover_abandoned_cue_runs(&self) -> Result<Vec<String>, RunsError> {
        super::recover_abandoned_cue_runs(&self.connection)
    }

    pub(crate) fn claim_next(
        &self,
        worker_id: &str,
        filters: &ClaimFilters,
    ) -> Result<Option<RunRow>, RunsError> {
        super::claim_next(&self.connection, worker_id, filters)
    }

    pub(crate) fn get_run_env(&self, run_id: &str) -> Result<Option<String>, RunsError> {
        super::get_run_env(&self.connection, run_id)
    }

    pub(crate) fn last_scheduled_fire_ms(
        &self,
        schedule_id: &str,
    ) -> Result<Option<i64>, RunsError> {
        super::last_scheduled_fire_ms(&self.connection, schedule_id)
    }

    pub(crate) fn enqueue_scheduled(
        &self,
        script_path: &str,
        args: &[String],
        options: EnqueueOptions,
    ) -> Result<Option<RunRow>, RunsError> {
        super::enqueue_scheduled(&self.connection, script_path, args, options)
    }

    pub(crate) fn complete(
        &self,
        run_id: &str,
        completion: RunCompletion,
    ) -> Result<(), RunsError> {
        super::complete(&self.connection, run_id, completion)
    }

    pub(crate) fn fail(&self, run_id: &str, completion: RunCompletion) -> Result<(), RunsError> {
        super::fail(&self.connection, run_id, completion)
    }

    pub(crate) fn time_out(
        &self,
        run_id: &str,
        completion: RunCompletion,
    ) -> Result<(), RunsError> {
        super::time_out(&self.connection, run_id, completion)
    }

    pub(crate) fn record_cancelled_output(
        &self,
        run_id: &str,
        completion: RunCompletion,
    ) -> Result<(), RunsError> {
        super::record_cancelled_output(&self.connection, run_id, completion)
    }
}

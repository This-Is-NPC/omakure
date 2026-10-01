use super::enqueue::EnqueueOptions;
use super::lifecycle;
use super::open;
use super::query::{self, RunFilters, RunRow, RunStats};
use super::trace::{self, TraceLevel, TraceRow};
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

    pub(crate) fn enqueue_cue(
        &mut self,
        script_path: &str,
        args: &[String],
        options: EnqueueOptions,
    ) -> Result<RunRow, RunsError> {
        super::enqueue_cue(&mut self.connection, script_path, args, options)
    }

    pub(crate) fn query_runs(&self, filters: &RunFilters) -> Result<Vec<RunRow>, RunsError> {
        query::query_runs(&self.connection, filters)
    }

    pub(crate) fn get_run_required(&self, run_id: &str) -> Result<RunRow, RunsError> {
        query::get_run_required(&self.connection, run_id)
    }

    pub(crate) fn query_traces(
        &self,
        run_id: &str,
        level: Option<TraceLevel>,
        since_sequence: Option<i64>,
    ) -> Result<Vec<TraceRow>, RunsError> {
        trace::query_traces(&self.connection, run_id, level, since_sequence)
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

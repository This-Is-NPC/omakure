use super::lifecycle;
use super::open;
use super::query::{self, RunFilters, RunRow, RunStats};
use super::trace::{self, TraceLevel, TraceRow};
use super::RunsError;
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
}

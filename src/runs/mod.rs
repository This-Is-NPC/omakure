//! SQLite-backed run history with a state machine and structured trace stream.
//!
//! `runs/` is the **only** code path that persists script execution
//! history.

mod enqueue;
mod error;
mod ids;
mod lifecycle;
mod open;
mod query;
mod state;
mod store;
mod trace;
mod workflow;

pub use enqueue::{
    ALLOW_ALL_SECRET_REFS_POLICY, EnqueueOptions, enqueue, enqueue_cue, enqueue_scheduled,
    get_run_env, get_run_script_hash, get_run_secret_refs, start_inline,
};
pub use error::RunsError;
pub use ids::format_run_timestamp;
pub use lifecycle::{
    ClaimFilters, RunCompletion, cancel_cue_runs_for_actor, claim_next, complete, fail, heartbeat,
    record_cancelled_output, recover_abandoned_cue_runs, time_out,
};
pub use open::open;
pub use query::{RunFilters, RunRow, RunStats, get_run, last_scheduled_fire_ms};
pub use state::{RunState, RunStateSet, RunTrigger};
pub(crate) use store::RunStore;
#[cfg(test)]
pub(crate) use trace::query_traces;
pub use trace::{TraceLevel, TraceRow, insert_trace};
#[cfg(all(test, unix))]
pub use workflow::WorkflowState;
pub use workflow::{
    WorkflowRun, WorkflowSnapshot, WorkflowStepSnapshot, advance_workflow_for_run, get_workflow,
    recover_workflows, start_workflow,
};

/// Internal heartbeat lease duration in milliseconds (60 s).
///
/// This is **not** a job timeout. It only governs how long a worker holds a
/// claim before another worker may steal the row. The user-facing per-job
/// `--timeout` is independent of this value.
pub const HEARTBEAT_MS: i64 = 60_000;

#[cfg(test)]
mod tests;

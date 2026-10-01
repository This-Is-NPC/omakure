//! Shared execution helper used by both `omakure run` (synchronous fast
//! path) and `omakure queue worker` (daemon draining the queue).
//!
//! `execute_with_heartbeat` owns the entire lifecycle of a single child
//! process: spawning it with the supplied environment, refreshing the
//! lease in `runs.sqlite` periodically, optionally killing it after a
//! per-job timeout, and reacting to mid-execution cancel by polling the
//! heartbeat call's return value.
//!
//! There is exactly one execution code path; the worker's loop and
//! `omakure run` both call this function so the two surfaces never drift.

use crate::runs::RunCompletion;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

mod admission;
mod environment;
mod lifecycle;
mod pipes;
#[cfg(test)]
mod tests;

pub use lifecycle::{execute_with_heartbeat, execute_with_heartbeat_guarded};

/// Outcome of one [`execute_with_heartbeat`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionTerminal {
    /// The script exited cleanly with `success = true`.
    Completed,
    /// The script exited with a non-zero code or `success = false`.
    Failed,
    /// The watcher killed the script for exceeding `timeout_ms`.
    TimedOut,
    /// The script was killed because the row was cancelled externally,
    /// or the worker was asked to shut down before the script finished.
    Cancelled,
    /// The runner failed to spawn the child or hit an unrecoverable error.
    Errored,
}

/// Captured output and the terminal classification produced by one run.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub terminal: ExecutionTerminal,
    pub completion: RunCompletion,
}

/// Optional cancel flag shared by `omakure queue worker`'s SIGINT handler.
/// `omakure run` does not use it (it passes `None`).
pub type CancelFlag = Arc<AtomicBool>;

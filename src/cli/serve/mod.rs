//! `omakure serve` — cron scheduler daemon.
//!
//! Scans the scripts workspace for scripts whose embedded schema declares a
//! `Schedule` block, computes next fire times, and enqueues runs through
//! the shared `runs::enqueue` state machine with `trigger = Scheduled`.
//!
//! Execution is delegated to the queue worker. By default `serve` also
//! spawns an in-process worker so a single invocation is self-sufficient;
//! pass `--no-worker` when a dedicated `omakure queue worker` is already
//! running.
//!
//! A single PID file at `<workspace>/.omakure/daemon.pid` guards against
//! concurrent daemons. Structured events are appended to
//! `<workspace>/.omakure/daemon.log`.

mod lifecycle;
mod logging;
mod scheduler;

pub use lifecycle::run;
pub(crate) use scheduler::scheduler_tick;

#[cfg(test)]
mod tests;

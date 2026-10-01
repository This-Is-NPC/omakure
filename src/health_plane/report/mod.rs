//! Performer-side Profile and Pulse construction.
//!
//! This module owns *what* a Performer reports and *when*, entirely in terms of
//! the frozen contract. It knows nothing about sockets, Noise sessions, the run
//! log, or the workspace: live facts arrive through [`HealthFactsSource`], which
//! the operations layer implements.
//!
//! Every value it emits is privacy class P0 and is clamped to the frozen
//! grammar before it reaches the wire, because
//! `docs/internal/health-plane-contract.md` requires the sender to redact and the
//! receiver to reject rather than redact.

use super::model::{RunFact, RunnerFact, RuntimeFact};
use serde_json::Value;

mod ids;
mod payload;
mod reporter;
mod sanitize;
#[cfg(test)]
mod tests;

pub use ids::{hex_lower, opaque_run_id, run_signal_id};
pub use payload::{ack_payload, error_payload, signal_encoded_bytes, signal_payload};
pub use reporter::HealthReporter;
pub use sanitize::sanitize_signal_run;

/// The static node facts a Performer reports, before revision assignment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProfileFacts {
    pub agent_version: String,
    pub arch: String,
    /// The derived name of the baseline this node recorded installing, or empty.
    pub baseline_id: String,
    /// The same derivation recomputed over that baseline's paths as they are on
    /// disk now, or empty.
    pub baseline_observed_id: String,
    pub capabilities: Vec<String>,
    pub display_name: String,
    pub distro_id: String,
    pub distro_version: String,
    pub omarchy_channel: String,
    pub omarchy_version: String,
    pub platform: String,
    pub runtimes: Vec<RuntimeFact>,
}

/// The liveness facts a Performer reports, before sequencing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulseFacts {
    pub runner: RunnerFact,
    pub last_run: Option<RunFact>,
    pub uptime_seconds: u64,
}

/// The live facts a Performer reports.
///
/// Implementations read the local node only. Nothing here may consult a peer
/// message: authorization and health are strictly one-directional.
pub trait HealthFactsSource: Send + Sync {
    /// The current static node facts, without `capabilities` or the revision.
    fn profile_facts(&self) -> ProfileFacts;
    /// The current liveness facts.
    fn pulse_facts(&self) -> PulseFacts;

    /// The bounded, newest-first set of runs that already reached a terminal
    /// result in the local run log.
    ///
    /// Implementations read the local run log only and map each row onto the
    /// frozen five-field `run` object. The script path, the arguments, stdout,
    /// stderr, the error text, the actor, and the worker id are privacy class
    /// P1 and never cross this boundary.
    fn terminal_runs(&self, limit: usize) -> Vec<RunFact>;
}

/// One Profile ready to sign, with the revision it was assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileMessage {
    pub payload: Value,
    pub profile_revision: u64,
    /// Whether the facts materially changed since the previous build.
    pub changed: bool,
}

/// One Pulse ready to sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulseMessage {
    pub payload: Value,
    pub sequence: u64,
}

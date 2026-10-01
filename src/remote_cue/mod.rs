//! The receive half of the Remote Cue plane, frozen by
//! `docs/internal/remote-cue-contract.md`.
//!
//! This module decides **whether a node will accept an instruction at all**. It
//! executes nothing, and deliberately holds no path into `run_executor`,
//! `runs::enqueue`, or `runs::start_inline`. The security boundary lands and is
//! certified before any code path can cause work to run.
//!
//! Every authorization input is read from the receiver's own registry and
//! configuration. No field of an inbound message contributes to the decision to
//! accept it, so a Cue asserting its own role or capability is refused whenever
//! the local registry disagrees. The gate logic below is a pure function over
//! locally-read facts precisely so that property is visible rather than
//! asserted.

/// Required too, because a peer that cannot receive an outcome must not be able
/// to create work whose result is unobservable.
pub use crate::domain::CAPABILITY_NOTIFICATIONS;
/// The capability required to send a Cue at all.
pub use crate::domain::CAPABILITY_REMOTE_RUN;

mod codes;
mod dispatch;
mod enqueue;
mod gates;
mod session;

pub use codes::CueCode;
pub use dispatch::content_hash;
pub use enqueue::{CueEnqueueError, derive_run_id};
pub use gates::{
    ExecutionGuard, ExecutionLockError, GateDecision, LocalAuthority, declares_secret_field,
    evaluate_gates, is_declared, is_declared_or_from_declared_battery, is_regular_file,
    is_well_formed_cue_id, is_well_formed_script_name, resolve_in_listing, within_validity_window,
};
pub use session::{CueOutcome, CuePolicy, CueSession, read_policy};

/// The frozen maximum lifetime of a Cue, in seconds.
pub const MAX_LIFETIME_SECONDS: i64 = 300;
/// Frozen per-peer receive limits. The burst is deliberately additive to the
/// sustained minute budget, matching the existing Health Plane convention.
pub const MAX_CUES_PER_MINUTE: usize = 10;
pub const RATE_BURST_ALLOWANCE: usize = 5;
pub const MAX_RETAINED_CUE_RECORDS: usize = 64;
pub const CUE_RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;
pub const MAX_CANONICAL_CUE_DISPATCH: usize = 512;

/// The two kinds of the Cue plane, frozen by the contract.
pub const KIND_DISPATCH: &str = "cue_dispatch";
pub const KIND_ACK: &str = "cue_ack";

/// The frozen upper bound on a Cue's human-readable reason.
pub const MAX_REASON_BYTES: usize = 128;

#[cfg(test)]
mod tests;

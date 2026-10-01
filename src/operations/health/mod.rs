//! Protocol-neutral Health Plane operations.
//!
//! Three responsibilities, all deliberately adapter-free:
//!
//! 1. [`fleet_status`] projects the Conductor-local Health Plane state that the
//!    Wave 2 shared operations own into one bounded, redacted report. The CLI
//!    and the HTTP route both render exactly this value, which is what makes
//!    them return identical status.
//! 2. [`signal_feed`] projects the bounded, newest-first closed Signal feed
//!    the same way, merging the per-Performer inbox with the Conductor-local
//!    lifecycle projection.
//! 3. [`NodeHealthFacts`] reads the live local node so a Performer can report
//!    Profile, Pulse, and `run-completed`. It reads the local node only;
//!    nothing here consults a peer message, and nothing here can mutate trust.
//!
//! Every bound below is transcribed from `docs/internal/health-plane-contract.md` via
//! `crate::health_plane::bounds`. None of them is chosen here.

mod facts;
mod fleet;
mod signals;

pub use facts::NodeHealthFacts;
pub use fleet::{fleet_status, BaselineCounts, FleetStatusReport, PresenceCounts};
pub use signals::{signal_feed, SignalCursor, SignalEntry, SignalFeedReport};

//! Node, baseline and enrollment paired probes.
//!
//! Implementations must exercise shared node operations through both adapters,
//! covering authorized, unauthenticated and forbidden actors where exposed.

pub const CASE_IDS: &[&str] = &[
    "exact.node-init",
    "exact.node-status",
    "exact.node-peers",
    "exact.node-trust",
    "exact.node-capabilities",
    "exact.node-revoke",
    "exact.node-health",
    "exact.node-signals",
    "exact.node-baseline-push",
    "exact.node-baseline-rollback",
    "exact.node-enroll-approve",
    "exact.node-enroll-reject",
    "mismatch.node-discovery",
    "mismatch.node-cue",
    "mismatch.node-enroll-request",
    "mismatch.node-enroll-apply",
];

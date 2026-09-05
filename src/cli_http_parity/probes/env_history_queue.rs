//! Environment, history and queue paired probes.
//!
//! Implementations must use deterministic local workspaces and the shared
//! operations, not route-shape assertions or mocked adapter responses.

pub const CASE_IDS: &[&str] = &[
    "exact.history-list",
    "exact.history-show",
    "exact.history-traces",
    "exact.history-stats",
    "exact.queue-add",
    "exact.queue-cancel",
    "exact.queue-dead-letter",
    "exact.env-list",
    "exact.env-create",
    "exact.env-show",
    "exact.env-replace",
    "exact.env-set",
    "exact.env-remove",
    "exact.env-activate",
    "exact.env-deactivate",
    "exact.env-delete",
];

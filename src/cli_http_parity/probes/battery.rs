//! Battery paired probes, including intentional HTTPS policy divergences.
//!
//! Implementations must exercise battery operations against deterministic local
//! repositories and the real authenticated HTTP routes.

pub const CASE_IDS: &[&str] = &[
    "exact.battery-list",
    "exact.battery-inspect",
    "exact.battery-scripts",
    "exact.battery-remove",
    "mismatch.battery-add",
    "mismatch.battery-sync",
    "mismatch.battery-install",
];

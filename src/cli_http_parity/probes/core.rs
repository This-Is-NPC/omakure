//! Core/script/search/config paired probes.
//!
//! Implementations must call the real CLI adapter and in-process HTTP adapter,
//! then return `ProbeEvidence`; this module intentionally contains no fake
//! observations.

pub const CASE_IDS: &[&str] = &[
    "exact.doctor",
    "exact.scripts",
    "exact.describe",
    "mismatch.config",
    "mismatch.search",
];

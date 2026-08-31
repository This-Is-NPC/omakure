//! Family-owned paired CLI/HTTP probe registrations.
//!
//! Each child module owns a disjoint slice of manifest behavior-case IDs. The
//! integration owner owns `PairedProbeRegistry`; family modules only register
//! real adapter functions through its `register` API.

pub mod battery;
pub mod core;
pub mod env_history_queue;
pub mod node;

/// Case IDs claimed by all family modules. Keep this list partitioned and
/// duplicate-free; `partition_case_ids` is a focused completeness tripwire.
pub fn case_ids() -> Vec<&'static str> {
    battery::CASE_IDS
        .iter()
        .chain(core::CASE_IDS)
        .chain(env_history_queue::CASE_IDS)
        .chain(node::CASE_IDS)
        .copied()
        .collect()
}

pub fn partition_case_ids() -> Result<(), String> {
    let mut ids = case_ids();
    ids.sort_unstable();
    for pair in ids.windows(2) {
        if pair[0] == pair[1] {
            return Err(format!("duplicate probe case {}", pair[0]));
        }
    }
    Ok(())
}

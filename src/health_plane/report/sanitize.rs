use super::{ProfileFacts, PulseFacts};
use crate::health_plane::bounds::{
    BASELINE_ID_HEX_CHARS, CAPABILITY_ALLOWLIST, MAX_AGENT_VERSION_BYTES, MAX_DISPLAY_NAME_BYTES,
    MAX_DISTRO_ID_BYTES, MAX_DISTRO_VERSION_BYTES, MAX_EXIT_CODE, MAX_QUEUE_DEPTH,
    MAX_RUNTIME_COUNT, MAX_SCRIPT_BYTES, MAX_UPTIME_SECONDS, MAX_WORKERS, MIN_EXIT_CODE,
    OPAQUE_ID_HEX_CHARS, RUNTIME_NAMES,
};
use crate::health_plane::model::RunFact;
use crate::util::hex;

/// Force one run fact into the frozen five-field Signal `run` grammar.
///
/// Returns `false` when the run cannot be expressed inside the closed schema,
/// in which case no Signal is produced at all. The sender redacts and the
/// receiver rejects; neither ever stores a value it had to guess at.
pub fn sanitize_signal_run(run: &mut RunFact) -> bool {
    run.script = clamp(&run.script, MAX_SCRIPT_BYTES, "._-", false);
    run.started_at = None;
    run.trigger = None;
    run.exit_code = run
        .exit_code
        .filter(|code| (MIN_EXIT_CODE..=MAX_EXIT_CODE).contains(code));
    run_fact_is_valid(run)
}

/// Whether one run fact already satisfies the frozen `run` grammar.
fn run_fact_is_valid(run: &RunFact) -> bool {
    !run.script.is_empty()
        && run.run_id.len() == OPAQUE_ID_HEX_CHARS
        && hex::is_lower(&run.run_id)
        && run.finished_at >= 1
        && RUN_STATES.contains(&run.state.as_str())
}

/// The five terminal run states the frozen schema permits.
const RUN_STATES: [&str; 5] = [
    "completed",
    "failed",
    "cancelled",
    "timed_out",
    "dead_letter",
];

/// Clamp a granted capability set to the frozen allow-list, sorted and unique.
pub(super) fn sanitize_capabilities(granted: &[String]) -> Vec<String> {
    let mut capabilities: Vec<String> = granted
        .iter()
        .filter(|entry| CAPABILITY_ALLOWLIST.contains(&entry.as_str()))
        .cloned()
        .collect();
    capabilities.sort_unstable();
    capabilities.dedup();
    capabilities
}

pub(super) fn sanitize_profile(facts: &mut ProfileFacts) {
    facts.agent_version = clamp(&facts.agent_version, MAX_AGENT_VERSION_BYTES, ".+-", false);
    if facts.agent_version.is_empty() {
        facts.agent_version = "0".to_string();
    }
    if !["x86_64", "aarch64"].contains(&facts.arch.as_str()) {
        facts.arch = "unknown".to_string();
    }
    clamp_baseline_id(&mut facts.baseline_id);
    clamp_baseline_id(&mut facts.baseline_observed_id);
    // A node that records no baseline has nothing to have observed. Clearing
    // here rather than trusting the caller keeps the pair the receiver's closed
    // schema insists on reachable from any fact source.
    if facts.baseline_id.is_empty() {
        facts.baseline_observed_id.clear();
    }
    facts.display_name = clamp(&facts.display_name, MAX_DISPLAY_NAME_BYTES, " ._-", true);
    while facts.display_name.ends_with(' ') {
        facts.display_name.pop();
    }
    facts.distro_id = clamp(&facts.distro_id, MAX_DISTRO_ID_BYTES, "._-", true).to_lowercase();
    if facts
        .distro_id
        .bytes()
        .next()
        .is_some_and(|byte| !byte.is_ascii_alphanumeric())
    {
        facts.distro_id.clear();
    }
    facts.distro_version = clamp(
        &facts.distro_version,
        MAX_DISTRO_VERSION_BYTES,
        "._+-",
        true,
    );
    facts.omarchy_version = clamp(
        &facts.omarchy_version,
        MAX_DISTRO_VERSION_BYTES,
        "._+-",
        true,
    );
    if !["stable", "dev"].contains(&facts.omarchy_channel.as_str()) {
        facts.omarchy_channel.clear();
    }
    if !["linux", "macos", "windows"].contains(&facts.platform.as_str()) {
        // The closed schema has no "other" platform, and a Performer that
        // cannot name its platform must not silently claim a different one.
        facts.platform = "linux".to_string();
    }
    facts.runtimes.retain(|runtime| {
        RUNTIME_NAMES.contains(&runtime.name.as_str()) && runtime.name.len() <= MAX_SCRIPT_BYTES
    });
    facts.runtimes.sort_by(|left, right| {
        runtime_rank(&left.name)
            .cmp(&runtime_rank(&right.name))
            .then_with(|| left.name.cmp(&right.name))
    });
    facts
        .runtimes
        .dedup_by(|left, right| left.name == right.name);
    facts.runtimes.truncate(MAX_RUNTIME_COUNT);
    for runtime in &mut facts.runtimes {
        runtime.version = clamp(&runtime.version, MAX_DISTRO_VERSION_BYTES, "._+-", true);
        if !runtime.available {
            runtime.version.clear();
        }
    }
}

/// Clear anything that is not the exact derived identity width in lowercase hex.
///
/// A malformed value is cleared rather than truncated: half of a baseline
/// identity is not a shorter identity, and the receiver would read it as a
/// different set rather than as an unreadable one.
fn clamp_baseline_id(value: &mut String) {
    if value.len() != BASELINE_ID_HEX_CHARS || !hex::is_lower(&value) {
        value.clear();
    }
}

pub(super) fn sanitize_pulse(facts: &mut PulseFacts) {
    facts.runner.queue_depth = facts.runner.queue_depth.min(MAX_QUEUE_DEPTH);
    facts.runner.workers_configured = facts.runner.workers_configured.min(MAX_WORKERS);
    facts.runner.workers_busy = facts
        .runner
        .workers_busy
        .min(facts.runner.workers_configured);
    facts.uptime_seconds = facts.uptime_seconds.min(MAX_UPTIME_SECONDS);
    if !["running", "disabled"].contains(&facts.runner.scheduler.as_str()) {
        facts.runner.scheduler = "disabled".to_string();
    }
    if !["idle", "busy", "paused", "degraded", "stopped"].contains(&facts.runner.state.as_str()) {
        facts.runner.state = "degraded".to_string();
    }
    let Some(run) = facts.last_run.as_mut() else {
        return;
    };
    run.script = clamp(&run.script, MAX_SCRIPT_BYTES, "._-", false);
    if !run_fact_is_valid(run) {
        facts.last_run = None;
        return;
    }
    let started = run.started_at.unwrap_or(run.finished_at).max(1);
    run.started_at = Some(started.min(run.finished_at));
    run.exit_code = run
        .exit_code
        .filter(|code| (MIN_EXIT_CODE..=MAX_EXIT_CODE).contains(code));
    let trigger = run.trigger.clone().unwrap_or_default();
    run.trigger = Some(match trigger.as_str() {
        "scheduled" => "scheduled".to_string(),
        "queue" => "queue".to_string(),
        "cue" => "cue".to_string(),
        _ => "manual".to_string(),
    });
}

fn runtime_rank(name: &str) -> usize {
    RUNTIME_NAMES
        .iter()
        .position(|candidate| *candidate == name)
        .unwrap_or(RUNTIME_NAMES.len())
}

/// Force one string into the frozen grammar: ASCII alphanumeric first byte,
/// then alphanumerics plus `extra`, bounded by `max` bytes.
///
/// This is the structural half of P1 enforcement on the sending side. No
/// grammar built from these characters can express a path, a URL, a
/// `secret://` reference, or an address.
fn clamp(value: &str, max: usize, extra: &str, allow_empty: bool) -> String {
    let mut out = String::with_capacity(value.len().min(max));
    for byte in value.bytes() {
        if out.len() == max {
            break;
        }
        let keep = byte.is_ascii_alphanumeric() || extra.as_bytes().contains(&byte);
        if !keep {
            continue;
        }
        if out.is_empty() && !byte.is_ascii_alphanumeric() {
            continue;
        }
        out.push(byte as char);
    }
    if out.is_empty() && !allow_empty {
        return String::new();
    }
    out
}

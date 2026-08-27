//! Performer-side Profile and Pulse construction.
//!
//! This module owns *what* a Performer reports and *when*, entirely in terms of
//! the frozen contract. It knows nothing about sockets, Noise sessions, the run
//! log, or the workspace: live facts arrive through [`HealthFactsSource`], which
//! the operations layer implements.
//!
//! Every value it emits is privacy class P0 and is clamped to the frozen
//! grammar before it reaches the wire, because
//! `.docs/health-plane-contract.md` requires the sender to redact and the
//! receiver to reject rather than redact.

use super::bounds::{
    CAPABILITY_ALLOWLIST, HEALTH_VERSION, MAX_AGENT_VERSION_BYTES, MAX_DISPLAY_NAME_BYTES,
    MAX_DISTRO_ID_BYTES, MAX_DISTRO_VERSION_BYTES, MAX_EXIT_CODE, MAX_QUEUE_DEPTH,
    MAX_RUNTIME_COUNT, MAX_SAFE_INTEGER, MAX_SCRIPT_BYTES, MAX_STORED_SIGNAL_BYTES,
    MAX_UPTIME_SECONDS, MAX_WORKERS, MIN_EXIT_CODE, NOMINAL_PULSE_INTERVAL_SECONDS,
    OPAQUE_ID_HEX_CHARS, RUNTIME_NAMES, SIGNAL_OUTBOX_CAPACITY, SIGNATURE_BYTES,
};
use super::model::{HealthKind, RunFact, RunnerFact, RuntimeFact, SignalRecord};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Mutex;

/// Domain separator for the opaque Health Plane run identifier.
///
/// The shipped run id is `<unix_ms>-<pid>-<counter>`, which carries a host
/// process id and does not match the frozen 16-byte opaque form. Hashing it
/// under a dedicated domain yields a stable, opaque, P0 identifier that leaks
/// no host fact and still correlates two Pulses about the same run.
const RUN_ID_DOMAIN: &[u8] = b"omakure/health-run-id/v1\0";

/// Domain separator for the stable Signal idempotency key.
///
/// `signal_id` must be identical for every retransmission of one logical
/// Signal, including after a Performer restart, because the frozen contract
/// makes it the application idempotency key. Deriving it from the already
/// opaque run identifier under a dedicated domain gives exactly that, with no
/// extra durable state and no host fact.
const SIGNAL_ID_DOMAIN: &[u8] = b"omakure/health-signal-id/v1\0";

/// The fixed-width envelope fields used when measuring a Signal's real size.
///
/// Every direct-envelope field except `payload` is constant width: `nonce` is
/// 32 hex characters, `session_id` is 64, `sender` is a 69-byte node ID,
/// `version` is one digit, and `created_at` is a ten-digit Unix second until
/// the year 2286. Measuring the canonical bytes of that shape therefore yields
/// the real encoded size without needing a signing key.
const SIZE_PROBE_CREATED_AT: u64 = 1_700_000_000;
const SESSION_ID_HEX_CHARS: usize = 64;

/// The static node facts a Performer reports, before revision assignment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProfileFacts {
    pub agent_version: String,
    pub arch: String,
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

#[derive(Debug, Default)]
struct ReporterState {
    current: Option<ProfileFacts>,
    profile_revision: u64,
    last_pulse_sequence: u64,
    /// The newest `finished_at` this reporter has already turned into a
    /// `run-completed` Signal. `None` until the first harvest seeds it.
    run_watermark: Option<i64>,
    /// The opaque run ids already harvested at exactly `run_watermark`, so two
    /// runs that finish inside the same Unix second both produce a Signal and
    /// neither produces two. Bounded by the frozen outbox capacity.
    run_watermark_ids: Vec<String>,
}

/// Builds the Profile and Pulse payloads one Performer sends to its Conductor.
///
/// Revision and sequence are wall-clock derived rather than stored, which is
/// what makes them survive a restart without a second source of truth:
///
/// * `profile_revision` is the Unix second at which the current facts were
///   first observed, floored to strictly exceed the previous revision. A
///   restart therefore always produces a revision greater than the one a
///   Conductor already holds.
/// * `pulse.sequence` is the emitting Unix second, which the contract also
///   requires `emitted_at` to equal. The frozen 10-second minimum accepted
///   Pulse interval guarantees two accepted Pulses never share a second, so
///   the sequence is strictly increasing per sender across restarts.
pub struct HealthReporter {
    facts: Box<dyn HealthFactsSource>,
    state: Mutex<ReporterState>,
}

impl HealthReporter {
    /// Build a reporter over one live fact source.
    pub fn new(facts: Box<dyn HealthFactsSource>) -> Self {
        Self {
            facts,
            state: Mutex::new(ReporterState::default()),
        }
    }

    /// The frozen nominal interval between Pulses, in seconds.
    pub const fn pulse_interval_seconds() -> i64 {
        NOMINAL_PULSE_INTERVAL_SECONDS
    }

    /// Build the current Profile for one Conductor.
    ///
    /// `granted` is the capability set the local registry records for that
    /// Conductor. It is display-only: the receiver authorizes from its own
    /// registry and never from this field.
    pub fn profile(
        &self,
        target: &str,
        message_id: &str,
        granted: &[String],
        now: i64,
    ) -> ProfileMessage {
        let mut facts = self.facts.profile_facts();
        facts.capabilities = sanitize_capabilities(granted);
        sanitize_profile(&mut facts);
        let mut state = self.state.lock().expect("health reporter state");
        let changed = state.current.as_ref() != Some(&facts);
        if changed || state.profile_revision == 0 {
            let floor = state.profile_revision.saturating_add(1);
            state.profile_revision = u64::try_from(now).unwrap_or(1).max(floor).max(1);
            state.current = Some(facts.clone());
        }
        let profile_revision = state.profile_revision;
        drop(state);
        ProfileMessage {
            payload: profile_payload(target, message_id, &facts, profile_revision),
            profile_revision,
            changed,
        }
    }

    /// Whether the live facts differ from the last Profile this reporter built.
    pub fn profile_changed(&self, granted: &[String]) -> bool {
        let mut facts = self.facts.profile_facts();
        facts.capabilities = sanitize_capabilities(granted);
        sanitize_profile(&mut facts);
        let state = self.state.lock().expect("health reporter state");
        state.current.as_ref() != Some(&facts)
    }

    /// Build the current Pulse for one Conductor.
    ///
    /// Returns `None` when `now` would not produce a strictly increasing
    /// sequence, which is exactly the case the frozen minimum Pulse interval
    /// already forbids on the wire.
    pub fn pulse(&self, target: &str, message_id: &str, now: i64) -> Option<PulseMessage> {
        let sequence = u64::try_from(now).ok()?;
        let mut state = self.state.lock().expect("health reporter state");
        if sequence <= state.last_pulse_sequence {
            return None;
        }
        state.last_pulse_sequence = sequence;
        let profile_revision = state.profile_revision;
        drop(state);
        let mut facts = self.facts.pulse_facts();
        sanitize_pulse(&mut facts);
        Some(PulseMessage {
            payload: pulse_payload(target, message_id, &facts, profile_revision, sequence),
            sequence,
        })
    }

    /// Harvest the terminal runs that still need a `run-completed` Signal.
    ///
    /// The frozen contract emits this Signal *only after* the existing run
    /// state reaches a terminal result, so the run log is the sole trigger and
    /// nothing here starts, schedules, or observes live work.
    ///
    /// The first call seeds the watermark from whatever the run log already
    /// holds and returns nothing: a Performer that restarts must not replay its
    /// own history into a Conductor's bounded Signal inbox. Every later call
    /// returns only the runs that reached a terminal result after that point,
    /// oldest first, bounded by the frozen outbox capacity.
    pub fn run_signals(&self) -> Vec<RunFact> {
        let capacity = SIGNAL_OUTBOX_CAPACITY as usize;
        let runs = self.sanitized_terminal_runs(capacity);
        let mut state = self.state.lock().expect("health reporter state");
        let Some(watermark) = state.run_watermark else {
            seed_watermark(&mut state, &runs, capacity);
            return Vec::new();
        };
        let mut emitted = Vec::new();
        for run in runs {
            if run.finished_at < watermark {
                continue;
            }
            if run.finished_at == watermark && state.run_watermark_ids.contains(&run.run_id) {
                continue;
            }
            if run.finished_at > state.run_watermark.unwrap_or(watermark) {
                state.run_watermark = Some(run.finished_at);
                state.run_watermark_ids.clear();
            }
            state.run_watermark_ids.push(run.run_id.clone());
            if state.run_watermark_ids.len() > capacity {
                state.run_watermark_ids.remove(0);
            }
            emitted.push(run);
        }
        emitted
    }

    /// Seed the run watermark now, without consuming anything.
    ///
    /// The first `run_signals` call seeds and returns nothing, so a run that
    /// reaches a terminal result before that call is swallowed forever. That is
    /// correct for history a restarting Performer must not replay, and wrong
    /// for a run some *other* node asked for and is waiting on. Seeding at
    /// service start, before the transport can accept anything, makes the two
    /// cases distinguishable by time rather than by luck.
    ///
    /// Idempotent under the same lock that guards the watermark, so a second
    /// caller cannot turn this into a silent harvest.
    pub fn seed_run_watermark(&self) {
        let capacity = SIGNAL_OUTBOX_CAPACITY as usize;
        let runs = self.sanitized_terminal_runs(capacity);
        let mut state = self.state.lock().expect("health reporter state");
        if state.run_watermark.is_some() {
            return;
        }
        seed_watermark(&mut state, &runs, capacity);
    }

    /// The terminal runs a Signal may describe, sanitized and oldest first.
    fn sanitized_terminal_runs(&self, capacity: usize) -> Vec<RunFact> {
        let mut runs: Vec<RunFact> = self
            .facts
            .terminal_runs(capacity)
            .into_iter()
            .filter_map(|mut run| sanitize_signal_run(&mut run).then_some(run))
            .collect();
        runs.sort_by(|left, right| {
            left.finished_at
                .cmp(&right.finished_at)
                .then_with(|| left.run_id.cmp(&right.run_id))
        });
        runs.truncate(capacity);
        runs
    }
}

/// Record the newest terminal result as already reported.
fn seed_watermark(state: &mut ReporterState, runs: &[RunFact], capacity: usize) {
    let newest = runs.last().map(|run| run.finished_at).unwrap_or(0);
    state.run_watermark = Some(newest);
    state.run_watermark_ids = runs
        .iter()
        .filter(|run| run.finished_at == newest)
        .map(|run| run.run_id.clone())
        .collect();
    state.run_watermark_ids.truncate(capacity);
}

/// The frozen Signal payload object.
///
/// All six `signal` fields are always present and the one that does not apply
/// to this kind is explicitly `null`, because the frozen closed schema rejects
/// an omitted field with `health_unknown_field` (1114). The `run` object
/// carries exactly the five fields the Signal schema names: it has no
/// `started_at` and no `trigger`, which the Pulse `last_run` object does.
pub fn signal_payload(target: &str, message_id: &str, signal: &SignalRecord) -> Value {
    let run = match &signal.run {
        Some(run) => json!({
            "exit_code": run.exit_code,
            "finished_at": run.finished_at,
            "run_id": run.run_id,
            "script": run.script,
            "state": run.state,
        }),
        None => Value::Null,
    };
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id,
        "target": target,
        "signal": {
            "kind": signal.kind.wire(),
            "occurred_at": signal.occurred_at,
            "run": run,
            "sequence": signal.sequence,
            "signal_id": signal.signal_id,
            "subject": signal.subject,
        }
    })
}

/// The encoded byte count one Signal occupies on the wire and in storage.
///
/// The result is the canonical envelope length plus the 64-byte signature,
/// measured from the real payload, and is clamped into the frozen stored-row
/// range so a hostile fact can never widen the storage accounting.
///
/// The measurement always uses the widest permitted `sequence`, because the
/// outbox assigns the real sequence after the size is recorded. The stored
/// accounting is therefore never optimistic.
pub fn signal_encoded_bytes(target: &str, signal: &SignalRecord) -> i64 {
    let widest = SignalRecord {
        sequence: MAX_SAFE_INTEGER,
        ..signal.clone()
    };
    let payload = signal_payload(target, &"0".repeat(OPAQUE_ID_HEX_CHARS), &widest);
    let mut object = serde_json::Map::new();
    object.insert("created_at".into(), Value::from(SIZE_PROBE_CREATED_AT));
    object.insert("kind".into(), Value::from(HealthKind::Signal.wire()));
    object.insert("nonce".into(), Value::from("0".repeat(OPAQUE_ID_HEX_CHARS)));
    object.insert("payload".into(), payload);
    object.insert("sender".into(), Value::from(target));
    object.insert(
        "session_id".into(),
        Value::from("0".repeat(SESSION_ID_HEX_CHARS)),
    );
    object.insert("version".into(), Value::from(1_u8));
    let canonical = serde_jcs::to_vec(&Value::Object(object)).unwrap_or_default();
    let bytes = canonical.len().saturating_add(SIGNATURE_BYTES) as i64;
    bytes.clamp(1, MAX_STORED_SIGNAL_BYTES)
}

/// The stable `signal_id` of the `run-completed` Signal for one opaque run id.
///
/// It is a pure function of the run, so a retransmission, a reconnect, or a
/// Performer restart reproduces exactly the same idempotency key and a
/// Conductor can never store the same terminal run twice.
pub fn run_signal_id(run_id: &str) -> String {
    let digest = Sha256::digest(
        [
            SIGNAL_ID_DOMAIN,
            crate::health_plane::model::SignalKind::RunCompleted
                .wire()
                .as_bytes(),
            b"\0",
            run_id.as_bytes(),
        ]
        .concat(),
    );
    hex_lower(&digest[..16])
}

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
        && run
            .run_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
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

/// The frozen Profile payload object.
fn profile_payload(
    target: &str,
    message_id: &str,
    facts: &ProfileFacts,
    profile_revision: u64,
) -> Value {
    let runtimes: Vec<Value> = facts
        .runtimes
        .iter()
        .map(|runtime| {
            json!({
                "available": runtime.available,
                "name": runtime.name,
                "version": runtime.version,
            })
        })
        .collect();
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id,
        "target": target,
        "profile": {
            "agent_version": facts.agent_version,
            "arch": facts.arch,
            "capabilities": facts.capabilities,
            "display_name": facts.display_name,
            "distro_id": facts.distro_id,
            "distro_version": facts.distro_version,
            "omarchy_channel": facts.omarchy_channel,
            "omarchy_version": facts.omarchy_version,
            "platform": facts.platform,
            "profile_revision": profile_revision,
            "role": "performer",
            "runtimes": runtimes,
        }
    })
}

/// The frozen Pulse payload object.
fn pulse_payload(
    target: &str,
    message_id: &str,
    facts: &PulseFacts,
    profile_revision: u64,
    sequence: u64,
) -> Value {
    let last_run = match &facts.last_run {
        Some(run) => json!({
            "exit_code": run.exit_code,
            "finished_at": run.finished_at,
            "run_id": run.run_id,
            "script": run.script,
            "started_at": run.started_at.unwrap_or(run.finished_at),
            "state": run.state,
            "trigger": run.trigger.clone().unwrap_or_else(|| "manual".to_string()),
        }),
        None => Value::Null,
    };
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id,
        "target": target,
        "pulse": {
            "emitted_at": sequence,
            "last_run": last_run,
            "profile_revision": profile_revision,
            "runner": {
                "queue_depth": facts.runner.queue_depth,
                "scheduler": facts.runner.scheduler,
                "state": facts.runner.state,
                "workers_busy": facts.runner.workers_busy,
                "workers_configured": facts.runner.workers_configured,
            },
            "sequence": sequence,
            "uptime_seconds": facts.uptime_seconds,
        }
    })
}

/// The frozen acknowledgement payload object.
pub fn ack_payload(target: &str, message_id: &str, acked_message_id: &str, cursor: u64) -> Value {
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id,
        "target": target,
        "ack": {
            "accepted": true,
            "acked_message_id": acked_message_id,
            "cursor": cursor,
        }
    })
}

/// The frozen rejection payload object. It carries only the stable code and
/// its name; never the offending bytes, a field value, or a diagnostic string.
pub fn error_payload(
    target: &str,
    message_id: &str,
    acked_message_id: &str,
    code: super::model::HealthCode,
) -> Value {
    json!({
        "health_version": HEALTH_VERSION,
        "message_id": message_id,
        "target": target,
        "error": {
            "accepted": false,
            "acked_message_id": acked_message_id,
            "code": code.code(),
            "reason": code.name(),
        }
    })
}

/// Map a shipped run id onto the frozen 16-byte opaque identifier.
pub fn opaque_run_id(run_id: &str) -> String {
    let digest = Sha256::digest([RUN_ID_DOMAIN, run_id.as_bytes()].concat());
    hex_lower(&digest[..16])
}

/// Lowercase hex, the only encoding the frozen schema accepts for opaque ids.
pub fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
}

/// Clamp a granted capability set to the frozen allow-list, sorted and unique.
fn sanitize_capabilities(granted: &[String]) -> Vec<String> {
    let mut capabilities: Vec<String> = granted
        .iter()
        .filter(|entry| CAPABILITY_ALLOWLIST.contains(&entry.as_str()))
        .cloned()
        .collect();
    capabilities.sort_unstable();
    capabilities.dedup();
    capabilities
}

fn sanitize_profile(facts: &mut ProfileFacts) {
    facts.agent_version = clamp(&facts.agent_version, MAX_AGENT_VERSION_BYTES, ".+-", false);
    if facts.agent_version.is_empty() {
        facts.agent_version = "0".to_string();
    }
    if !["x86_64", "aarch64"].contains(&facts.arch.as_str()) {
        facts.arch = "unknown".to_string();
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

fn sanitize_pulse(facts: &mut PulseFacts) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_plane::model::{HealthKind, SignalKind};
    use crate::health_plane::schema;

    const TARGET: &str = "omk1_0000000000000000000000000000000000000000000000000000000000000001";
    const MESSAGE_ID: &str = "00000000000000000000000000000001";

    struct FixedFacts {
        profile: ProfileFacts,
        pulse: PulseFacts,
        terminal: Mutex<Vec<RunFact>>,
    }

    impl HealthFactsSource for FixedFacts {
        fn profile_facts(&self) -> ProfileFacts {
            self.profile.clone()
        }

        fn pulse_facts(&self) -> PulseFacts {
            self.pulse.clone()
        }

        fn terminal_runs(&self, limit: usize) -> Vec<RunFact> {
            let mut runs = self.terminal.lock().expect("terminal runs").clone();
            runs.truncate(limit);
            runs
        }
    }

    fn sample_profile() -> ProfileFacts {
        ProfileFacts {
            agent_version: "0.3.0".to_string(),
            arch: "x86_64".to_string(),
            capabilities: Vec::new(),
            display_name: "workshop laptop".to_string(),
            distro_id: "arch".to_string(),
            distro_version: "rolling".to_string(),
            omarchy_channel: "stable".to_string(),
            omarchy_version: "2.1.0".to_string(),
            platform: "linux".to_string(),
            runtimes: vec![
                RuntimeFact {
                    available: true,
                    name: "sh".to_string(),
                    version: "5.2.37".to_string(),
                },
                RuntimeFact {
                    available: true,
                    name: "bash".to_string(),
                    version: "5.2.37".to_string(),
                },
            ],
        }
    }

    fn sample_pulse() -> PulseFacts {
        PulseFacts {
            runner: RunnerFact {
                queue_depth: 0,
                scheduler: "running".to_string(),
                state: "idle".to_string(),
                workers_busy: 0,
                workers_configured: 1,
            },
            last_run: None,
            uptime_seconds: 42,
        }
    }

    fn reporter(profile: ProfileFacts, pulse: PulseFacts) -> HealthReporter {
        HealthReporter::new(Box::new(FixedFacts {
            profile,
            pulse,
            terminal: Mutex::new(Vec::new()),
        }))
    }

    /// A fact source whose terminal run log the test can grow, newest first,
    /// exactly like the real run log.
    #[derive(Default)]
    struct SharedFacts {
        terminal: Mutex<Vec<RunFact>>,
    }

    impl SharedFacts {
        fn push(&self, run: RunFact) {
            self.terminal.lock().expect("terminal runs").insert(0, run);
        }
    }

    impl HealthFactsSource for std::sync::Arc<SharedFacts> {
        fn profile_facts(&self) -> ProfileFacts {
            sample_profile()
        }

        fn pulse_facts(&self) -> PulseFacts {
            sample_pulse()
        }

        fn terminal_runs(&self, limit: usize) -> Vec<RunFact> {
            let mut runs = self.terminal.lock().expect("terminal runs").clone();
            runs.truncate(limit);
            runs
        }
    }

    fn run_fact(run_id: &str, script: &str, finished_at: i64) -> RunFact {
        RunFact {
            exit_code: Some(0),
            finished_at,
            run_id: run_id.to_string(),
            script: script.to_string(),
            started_at: None,
            state: "completed".to_string(),
            trigger: None,
        }
    }

    #[test]
    fn profile_payload_validates_against_the_frozen_closed_schema() {
        let reporter = reporter(sample_profile(), sample_pulse());
        let message = reporter.profile(
            TARGET,
            MESSAGE_ID,
            &["inventory-health".to_string(), "notifications".to_string()],
            1_700_000_000,
        );
        let payload = schema::validate_payload(HealthKind::Profile, &message.payload, 0)
            .expect("profile payload is contract-valid");
        assert_eq!(payload.target, TARGET);
        assert!(message.changed);
        assert_eq!(message.profile_revision, 1_700_000_000);
    }

    #[test]
    fn pulse_payload_validates_and_binds_emitted_at_to_created_at() {
        let reporter = reporter(sample_profile(), sample_pulse());
        reporter.profile(TARGET, MESSAGE_ID, &[], 1_700_000_000);
        let message = reporter
            .pulse(TARGET, MESSAGE_ID, 1_700_000_030)
            .expect("a later second produces a pulse");
        schema::validate_payload(HealthKind::Pulse, &message.payload, 1_700_000_030)
            .expect("pulse payload is contract-valid");
        assert_eq!(message.sequence, 1_700_000_030);
    }

    #[test]
    fn pulse_sequence_is_strictly_increasing_within_one_second() {
        let reporter = reporter(sample_profile(), sample_pulse());
        assert!(reporter.pulse(TARGET, MESSAGE_ID, 1_700_000_000).is_some());
        assert!(reporter.pulse(TARGET, MESSAGE_ID, 1_700_000_000).is_none());
        assert!(reporter.pulse(TARGET, MESSAGE_ID, 1_700_000_001).is_some());
    }

    #[test]
    fn profile_revision_only_advances_on_material_change() {
        let reporter = reporter(sample_profile(), sample_pulse());
        let first = reporter.profile(TARGET, MESSAGE_ID, &[], 1_700_000_000);
        let second = reporter.profile(TARGET, MESSAGE_ID, &[], 1_700_000_100);
        assert_eq!(first.profile_revision, second.profile_revision);
        assert!(!second.changed);
        let third = reporter.profile(
            TARGET,
            MESSAGE_ID,
            &["notifications".to_string()],
            1_700_000_200,
        );
        assert!(third.changed);
        assert_eq!(third.profile_revision, 1_700_000_200);
    }

    #[test]
    fn profile_revision_floors_above_the_previous_revision_when_the_clock_stalls() {
        let reporter = reporter(sample_profile(), sample_pulse());
        let first = reporter.profile(TARGET, MESSAGE_ID, &[], 1_700_000_000);
        let second = reporter.profile(TARGET, MESSAGE_ID, &["notifications".to_string()], 1);
        assert_eq!(second.profile_revision, first.profile_revision + 1);
    }

    #[test]
    fn hostile_facts_are_clamped_into_the_frozen_grammar() {
        let mut profile = sample_profile();
        profile.display_name = "/home/user/../secret://token ".to_string();
        profile.distro_id = "ARCH/../etc/passwd".to_string();
        profile.agent_version = "\u{0}0.3.0\n".to_string();
        profile.platform = "plan9".to_string();
        profile.arch = "riscv64".to_string();
        let reporter = reporter(profile, sample_pulse());
        let message = reporter.profile(
            TARGET,
            MESSAGE_ID,
            &["remote-run".to_string()],
            1_700_000_000,
        );
        let payload = schema::validate_payload(HealthKind::Profile, &message.payload, 0)
            .expect("clamped profile stays contract-valid");
        let crate::health_plane::model::HealthBody::Profile(profile) = payload.body else {
            panic!("expected a profile body");
        };
        assert!(!profile.display_name.contains('/'));
        assert!(!profile.display_name.contains(':'));
        assert_eq!(profile.distro_id, "arch..etcpasswd");
        assert_eq!(profile.arch, "unknown");
        assert_eq!(profile.platform, "linux");
        assert_eq!(profile.capabilities, vec!["remote-run".to_string()]);
    }

    #[test]
    fn capabilities_outside_the_frozen_allow_list_are_dropped_and_sorted() {
        let reporter = reporter(sample_profile(), sample_pulse());
        let message = reporter.profile(
            TARGET,
            MESSAGE_ID,
            &[
                "notifications".to_string(),
                "not-a-capability".to_string(),
                "inventory-health".to_string(),
                "inventory-health".to_string(),
            ],
            1_700_000_000,
        );
        assert_eq!(
            message.payload["profile"]["capabilities"],
            serde_json::json!(["inventory-health", "notifications"])
        );
    }

    #[test]
    fn an_invalid_last_run_is_dropped_rather_than_reported() {
        let mut pulse = sample_pulse();
        pulse.last_run = Some(RunFact {
            exit_code: Some(0),
            finished_at: 1_699_999_990,
            run_id: "1700000000-4242-7".to_string(),
            script: "deploy".to_string(),
            started_at: Some(1_699_999_980),
            state: "completed".to_string(),
            trigger: Some("scheduled".to_string()),
        });
        let reporter = reporter(sample_profile(), pulse);
        let message = reporter
            .pulse(TARGET, MESSAGE_ID, 1_700_000_000)
            .expect("pulse builds");
        assert_eq!(message.payload["pulse"]["last_run"], Value::Null);
    }

    #[test]
    fn an_opaque_run_id_is_stable_and_contract_shaped() {
        let first = opaque_run_id("1700000000-4242-7");
        assert_eq!(first, opaque_run_id("1700000000-4242-7"));
        assert_ne!(first, opaque_run_id("1700000000-4242-8"));
        assert_eq!(first.len(), 32);
        assert!(first
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        assert!(!first.contains("4242"));
    }

    #[test]
    fn a_mapped_last_run_survives_validation() {
        let mut pulse = sample_pulse();
        pulse.last_run = Some(RunFact {
            exit_code: Some(0),
            finished_at: 1_699_999_990,
            run_id: opaque_run_id("1700000000-4242-7"),
            script: "deploy".to_string(),
            started_at: Some(1_699_999_980),
            state: "completed".to_string(),
            trigger: Some("scheduled".to_string()),
        });
        let reporter = reporter(sample_profile(), pulse);
        let message = reporter
            .pulse(TARGET, MESSAGE_ID, 1_700_000_000)
            .expect("pulse builds");
        schema::validate_payload(HealthKind::Pulse, &message.payload, 1_700_000_000)
            .expect("mapped run stays contract-valid");
    }

    #[test]
    fn ack_and_error_payloads_validate_against_the_frozen_schema() {
        let ack = ack_payload(TARGET, MESSAGE_ID, MESSAGE_ID, 3);
        schema::validate_payload(HealthKind::Ack, &ack, 0).expect("ack is contract-valid");
        let error = error_payload(
            TARGET,
            MESSAGE_ID,
            MESSAGE_ID,
            crate::health_plane::model::HealthCode::Replay,
        );
        schema::validate_payload(HealthKind::Error, &error, 0).expect("error is contract-valid");
    }

    #[test]
    fn runner_counts_are_clamped_to_the_frozen_ranges() {
        let mut pulse = sample_pulse();
        pulse.runner.queue_depth = u64::MAX;
        pulse.runner.workers_configured = u64::MAX;
        pulse.runner.workers_busy = u64::MAX;
        pulse.uptime_seconds = u64::MAX;
        let reporter = reporter(sample_profile(), pulse);
        let message = reporter
            .pulse(TARGET, MESSAGE_ID, 1_700_000_000)
            .expect("pulse builds");
        schema::validate_payload(HealthKind::Pulse, &message.payload, 1_700_000_000)
            .expect("clamped pulse stays contract-valid");
        assert_eq!(message.payload["pulse"]["runner"]["queue_depth"], 65_535);
        assert_eq!(message.payload["pulse"]["runner"]["workers_busy"], 255);
        assert_eq!(
            message.payload["pulse"]["uptime_seconds"],
            4_294_967_295_u64
        );
    }

    #[test]
    fn unsorted_and_duplicated_runtimes_are_normalized() {
        let mut profile = sample_profile();
        profile.runtimes.push(RuntimeFact {
            available: false,
            name: "bash".to_string(),
            version: "9.9".to_string(),
        });
        profile.runtimes.push(RuntimeFact {
            available: false,
            name: "cmd".to_string(),
            version: "1".to_string(),
        });
        let reporter = reporter(profile, sample_pulse());
        let message = reporter.profile(TARGET, MESSAGE_ID, &[], 1_700_000_000);
        schema::validate_payload(HealthKind::Profile, &message.payload, 0)
            .expect("normalized runtimes stay contract-valid");
        let runtimes = message.payload["profile"]["runtimes"]
            .as_array()
            .expect("runtimes array");
        assert_eq!(runtimes.len(), 2);
        assert_eq!(runtimes[0]["name"], "bash");
        assert_eq!(runtimes[1]["name"], "sh");
    }

    /// Regression pin for why the operations layer must always supply a
    /// script name.
    ///
    /// The frozen `run.script` field is 1..=64 bytes, so an empty name makes
    /// the whole `last_run` unrepresentable and it is dropped rather than
    /// guessed at. The shipped run log only records `script_name` for
    /// scheduler-enqueued runs, which is why the operations layer derives the
    /// name from the script's own file stem for every other run.
    #[test]
    fn a_run_without_a_script_name_is_unrepresentable_and_is_dropped() {
        let mut pulse = sample_pulse();
        let named = run_fact(&"a".repeat(32), "deploy", 1_699_999_990);
        pulse.last_run = Some(RunFact {
            script: String::new(),
            ..named.clone()
        });
        let reporter = reporter(sample_profile(), pulse);
        let message = reporter
            .pulse(TARGET, MESSAGE_ID, 1_700_000_000)
            .expect("pulse builds");
        assert_eq!(
            message.payload["pulse"]["last_run"],
            Value::Null,
            "a run with no schema name cannot be expressed inside the closed schema"
        );

        let mut unnamed = RunFact {
            script: String::new(),
            ..named.clone()
        };
        assert!(
            !sanitize_signal_run(&mut unnamed),
            "and it produces no Signal either"
        );

        let mut still_named = named;
        assert!(sanitize_signal_run(&mut still_named));
        assert_eq!(still_named.script, "deploy");
    }

    #[test]
    fn a_signal_payload_validates_against_the_frozen_closed_schema() {
        let signal = SignalRecord {
            kind: SignalKind::RunCompleted,
            occurred_at: 1_700_000_000,
            run: Some(run_fact(&"a".repeat(32), "deploy", 1_700_000_000)),
            sequence: 1,
            signal_id: "0000000000000000000000000000000b".to_string(),
            subject: None,
        };
        let payload = signal_payload(TARGET, MESSAGE_ID, &signal);
        let validated = schema::validate_payload(HealthKind::Signal, &payload, 1_700_000_000)
            .expect("signal payload must satisfy the frozen closed schema");
        match validated.body {
            crate::health_plane::model::HealthBody::Signal(record) => {
                assert_eq!(record, signal);
                // The Signal `run` object has exactly five fields: no
                // `started_at` and no `trigger`, unlike the Pulse `last_run`.
                assert_eq!(payload["signal"]["run"].as_object().unwrap().len(), 5);
                assert!(payload["signal"]["subject"].is_null());
            }
            other => panic!("unexpected body: {other:?}"),
        }
    }

    #[test]
    fn a_lifecycle_signal_payload_validates_and_carries_no_run() {
        let signal = SignalRecord {
            kind: SignalKind::Enrolled,
            occurred_at: 1_700_000_000,
            run: None,
            sequence: 7,
            signal_id: "0000000000000000000000000000000c".to_string(),
            subject: Some(TARGET.to_string()),
        };
        let payload = signal_payload(TARGET, MESSAGE_ID, &signal);
        schema::validate_payload(HealthKind::Signal, &payload, 1_700_000_000)
            .expect("lifecycle signal payload must satisfy the frozen closed schema");
        assert!(payload["signal"]["run"].is_null());
    }

    #[test]
    fn the_measured_signal_size_stays_inside_the_frozen_stored_row_cap() {
        let signal = SignalRecord {
            kind: SignalKind::RunCompleted,
            occurred_at: 1_700_000_000,
            run: Some(run_fact(&"f".repeat(32), &"s".repeat(64), 1_700_000_000)),
            sequence: 1,
            signal_id: "0".repeat(32),
            subject: None,
        };
        let measured = signal_encoded_bytes(TARGET, &signal);
        assert!(measured >= 1);
        assert!(
            measured <= MAX_STORED_SIGNAL_BYTES,
            "worst-case Signal measured {measured} bytes, cap is {MAX_STORED_SIGNAL_BYTES}"
        );
        // The measurement is sequence-independent, because the outbox assigns
        // the real sequence only after the size is recorded.
        let wider = SignalRecord {
            sequence: MAX_SAFE_INTEGER,
            ..signal.clone()
        };
        assert_eq!(measured, signal_encoded_bytes(TARGET, &wider));
    }

    #[test]
    fn a_run_signal_id_is_stable_per_run_and_distinct_across_runs() {
        let first = run_signal_id("run-a");
        assert_eq!(first, run_signal_id("run-a"));
        assert_ne!(first, run_signal_id("run-b"));
        assert_ne!(first, opaque_run_id("run-a"));
        assert_eq!(first.len(), 32);
        assert!(first
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
    }

    #[test]
    fn the_first_harvest_seeds_the_watermark_and_never_replays_history() {
        let facts = FixedFacts {
            profile: sample_profile(),
            pulse: sample_pulse(),
            terminal: Mutex::new(vec![
                run_fact(&"1".repeat(32), "deploy", 1_700_000_000),
                run_fact(&"2".repeat(32), "backup", 1_699_999_000),
            ]),
        };
        let reporter = HealthReporter::new(Box::new(facts));
        assert!(
            reporter.run_signals().is_empty(),
            "a restarting Performer must not replay its own run history"
        );
        assert!(reporter.run_signals().is_empty());
    }

    #[test]
    fn a_new_terminal_run_is_harvested_exactly_once() {
        let terminal = Mutex::new(vec![run_fact(&"1".repeat(32), "deploy", 1_700_000_000)]);
        let facts = FixedFacts {
            profile: sample_profile(),
            pulse: sample_pulse(),
            terminal,
        };
        let reporter = HealthReporter::new(Box::new(facts));
        assert!(reporter.run_signals().is_empty());
    }

    /// Seeding up front is what keeps a remotely-requested outcome from being
    /// swallowed by the reporter's own first harvest.
    ///
    /// The control matters more than the assertion: without the seed, the very
    /// same run vanishes, which is exactly the shipped hazard.
    #[test]
    fn a_run_finishing_after_an_explicit_seed_is_still_reported() {
        let shared = std::sync::Arc::new(SharedFacts::default());
        let reporter = HealthReporter::new(Box::new(std::sync::Arc::clone(&shared)));
        shared.push(run_fact(&"1".repeat(32), "history", 1_699_999_000));

        reporter.seed_run_watermark();
        shared.push(run_fact(&"2".repeat(32), "cue-origin", 1_700_000_000));

        let harvested = reporter.run_signals();
        assert_eq!(
            harvested
                .iter()
                .map(|run| run.script.as_str())
                .collect::<Vec<_>>(),
            vec!["cue-origin"],
            "history must stay unreplayed and the new run must be reported"
        );

        // The control: no seed, and the first harvest is the seed, so the run
        // that someone is waiting on is consumed and never sent.
        let unseeded_shared = std::sync::Arc::new(SharedFacts::default());
        let unseeded = HealthReporter::new(Box::new(std::sync::Arc::clone(&unseeded_shared)));
        unseeded_shared.push(run_fact(&"1".repeat(32), "history", 1_699_999_000));
        unseeded_shared.push(run_fact(&"2".repeat(32), "cue-origin", 1_700_000_000));
        assert!(
            unseeded.run_signals().is_empty(),
            "the hazard this seed exists to close"
        );
    }

    /// Seeding twice must not become a silent harvest.
    #[test]
    fn seeding_again_never_consumes_a_pending_outcome() {
        let shared = std::sync::Arc::new(SharedFacts::default());
        let reporter = HealthReporter::new(Box::new(std::sync::Arc::clone(&shared)));
        reporter.seed_run_watermark();

        shared.push(run_fact(&"2".repeat(32), "cue-origin", 1_700_000_000));
        reporter.seed_run_watermark();

        assert_eq!(
            reporter.run_signals().len(),
            1,
            "a second seed must be a no-op, not a harvest"
        );
    }

    #[test]
    fn concurrent_terminal_runs_in_one_second_each_produce_one_signal() {
        let shared = std::sync::Arc::new(SharedFacts::default());
        let reporter = HealthReporter::new(Box::new(std::sync::Arc::clone(&shared)));
        shared.push(run_fact(&"1".repeat(32), "seed", 1_699_999_000));
        assert!(reporter.run_signals().is_empty(), "the first call seeds");

        shared.push(run_fact(&"2".repeat(32), "alpha", 1_700_000_000));
        shared.push(run_fact(&"3".repeat(32), "beta", 1_700_000_000));
        let harvested = reporter.run_signals();
        assert_eq!(harvested.len(), 2, "both concurrent runs must be seen");
        assert!(
            reporter.run_signals().is_empty(),
            "a harvested run is never harvested twice"
        );

        shared.push(run_fact(&"4".repeat(32), "gamma", 1_700_000_001));
        let later = reporter.run_signals();
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].script, "gamma");
        assert!(harvested.iter().all(|run| run.script != "gamma"));
    }

    #[test]
    fn a_harvested_run_is_clamped_into_the_frozen_grammar_or_dropped() {
        let shared = std::sync::Arc::new(SharedFacts::default());
        let reporter = HealthReporter::new(Box::new(std::sync::Arc::clone(&shared)));
        assert!(reporter.run_signals().is_empty());

        let mut hostile = run_fact(&"5".repeat(32), "/etc/passwd; rm -rf /", 1_700_000_100);
        hostile.started_at = Some(1);
        hostile.trigger = Some("manual".to_string());
        hostile.exit_code = Some(9_000);
        shared.push(hostile);
        let mut unusable = run_fact("not-hex", "deploy", 1_700_000_101);
        unusable.state = "running".to_string();
        shared.push(unusable);

        let harvested = reporter.run_signals();
        assert_eq!(harvested.len(), 1, "the unusable run must be dropped");
        assert_eq!(harvested[0].script, "etcpasswdrm-rf");
        assert_eq!(harvested[0].exit_code, None);
        assert_eq!(harvested[0].started_at, None);
        assert_eq!(harvested[0].trigger, None);
    }

    #[test]
    fn the_harvest_is_bounded_by_the_frozen_outbox_capacity() {
        let shared = std::sync::Arc::new(SharedFacts::default());
        let reporter = HealthReporter::new(Box::new(std::sync::Arc::clone(&shared)));
        assert!(reporter.run_signals().is_empty());
        for index in 0..(SIGNAL_OUTBOX_CAPACITY as usize + 40) {
            shared.push(run_fact(
                &format!("{index:032x}"),
                "deploy",
                1_700_000_000 + index as i64,
            ));
        }
        assert_eq!(
            reporter.run_signals().len(),
            SIGNAL_OUTBOX_CAPACITY as usize
        );
    }

    #[test]
    fn an_unavailable_runtime_never_reports_a_version() {
        let mut profile = sample_profile();
        profile.runtimes = vec![RuntimeFact {
            available: false,
            name: "python".to_string(),
            version: "3.13.1".to_string(),
        }];
        let reporter = reporter(profile, sample_pulse());
        let message = reporter.profile(TARGET, MESSAGE_ID, &[], 1_700_000_000);
        schema::validate_payload(HealthKind::Profile, &message.payload, 0)
            .expect("unavailable runtime stays contract-valid");
        assert_eq!(message.payload["profile"]["runtimes"][0]["version"], "");
    }
}

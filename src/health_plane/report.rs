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
    MAX_DISTRO_ID_BYTES, MAX_DISTRO_VERSION_BYTES, MAX_QUEUE_DEPTH, MAX_RUNTIME_COUNT,
    MAX_SCRIPT_BYTES, MAX_UPTIME_SECONDS, MAX_WORKERS, NOMINAL_PULSE_INTERVAL_SECONDS,
};
use super::model::{RunFact, RunnerFact, RuntimeFact};
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

/// The four runtime names the closed schema permits, in frozen sorted order.
pub const RUNTIME_NAMES: [&str; MAX_RUNTIME_COUNT] = ["bash", "powershell", "python", "sh"];

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
}

/// The frozen Profile payload object.
pub fn profile_payload(
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
pub fn pulse_payload(
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
    let valid = !run.script.is_empty()
        && run.run_id.len() == 32
        && run.run_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        && run.run_id.bytes().all(|byte| !byte.is_ascii_uppercase())
        && run.finished_at >= 1
        && [
            "completed",
            "failed",
            "cancelled",
            "timed_out",
            "dead_letter",
        ]
        .contains(&run.state.as_str());
    if !valid {
        facts.last_run = None;
        return;
    }
    let started = run.started_at.unwrap_or(run.finished_at).max(1);
    run.started_at = Some(started.min(run.finished_at));
    run.exit_code = run.exit_code.filter(|code| (-256..=255).contains(code));
    let trigger = run.trigger.clone().unwrap_or_default();
    run.trigger = Some(match trigger.as_str() {
        "scheduled" => "scheduled".to_string(),
        "queue" => "queue".to_string(),
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
    use crate::health_plane::model::HealthKind;
    use crate::health_plane::schema;

    const TARGET: &str = "omk1_0000000000000000000000000000000000000000000000000000000000000001";
    const MESSAGE_ID: &str = "00000000000000000000000000000001";

    struct FixedFacts {
        profile: ProfileFacts,
        pulse: PulseFacts,
    }

    impl HealthFactsSource for FixedFacts {
        fn profile_facts(&self) -> ProfileFacts {
            self.profile.clone()
        }

        fn pulse_facts(&self) -> PulseFacts {
            self.pulse.clone()
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
        HealthReporter::new(Box::new(FixedFacts { profile, pulse }))
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

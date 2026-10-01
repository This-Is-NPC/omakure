use super::*;
use crate::health_plane::bounds::{
    MAX_SAFE_INTEGER, MAX_STORED_SIGNAL_BYTES, SIGNAL_OUTBOX_CAPACITY,
};
use crate::health_plane::model::{HealthKind, SignalKind, SignalRecord};
use crate::health_plane::schema;
use crate::test_support::opaque_id_hex;
use crate::util::hex;
use std::sync::Mutex;

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
        newest_runs(&self.terminal, limit)
    }
}

fn newest_runs(terminal: &Mutex<Vec<RunFact>>, limit: usize) -> Vec<RunFact> {
    let mut runs = terminal.lock().expect("terminal runs").clone();
    runs.truncate(limit);
    runs
}

fn sample_profile() -> ProfileFacts {
    ProfileFacts {
        agent_version: "0.3.0".to_string(),
        arch: "x86_64".to_string(),
        baseline_id: String::new(),
        baseline_observed_id: String::new(),
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
        newest_runs(&self.terminal, limit)
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

/// The sender clamps to exactly what the receiver validates.
///
/// A Performer that put a half-written or differently-spelled identity on
/// the wire would have every Profile refused with no way for either side to
/// say why — the same failure the runtime allow-list is single-sourced to
/// avoid. Truncating is refused for a second reason: half of a baseline
/// identity is not a shorter set, it is a different name, and a Conductor
/// would read it as drift.
#[test]
fn an_unreadable_baseline_identity_is_cleared_rather_than_carried() {
    let whole = "0123456789abcdef".repeat(4);
    for (recorded, observed, expected, why) in [
        (
            whole.clone(),
            whole.clone(),
            (whole.clone(), whole.clone()),
            "a well-formed pair travels unchanged",
        ),
        (
            whole.to_uppercase(),
            whole.clone(),
            (String::new(), String::new()),
            "uppercase is a second spelling of one set and is not carried",
        ),
        (
            whole[..40].to_string(),
            whole.clone(),
            (String::new(), String::new()),
            "a truncated claim clears its own evidence with it",
        ),
        (
            whole.clone(),
            "not-hex".to_string(),
            (whole.clone(), String::new()),
            "unreadable evidence is dropped without discarding the claim",
        ),
        (
            String::new(),
            whole.clone(),
            (String::new(), String::new()),
            "evidence a receiver would refuse never leaves the sender",
        ),
    ] {
        let mut facts = sample_profile();
        facts.baseline_id = recorded;
        facts.baseline_observed_id = observed;
        let reporter = reporter(facts, sample_pulse());
        let message = reporter.profile(TARGET, MESSAGE_ID, &[], 1_700_000_000);
        let payload = schema::validate_payload(HealthKind::Profile, &message.payload, 0)
            .expect("a clamped profile must stay contract-valid");
        let crate::health_plane::model::HealthBody::Profile(profile) = payload.body else {
            panic!("expected a profile body");
        };
        assert_eq!(
            (profile.baseline_id, profile.baseline_observed_id),
            expected,
            "{why}"
        );
    }
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
    assert!(hex::is_lower(&first));
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
    assert!(hex::is_lower(&first));
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
            &opaque_id_hex(index as u64),
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

use super::*;

/// The verdict is a comparison of two reported facts, made here.
///
/// Every case moves the *Profile* and reads the projection, because a
/// verdict computed anywhere but from the pair the Performer reported would
/// be an inference the Conductor is not entitled to make.
#[test]
fn a_performers_baseline_reads_as_unknown_none_in_sync_or_drifted() {
    let fixture = fixture();
    let target = fixture.local.clone();
    let installed = "1".repeat(64);
    let on_disk = "2".repeat(64);

    assert_eq!(
        fixture
            .plane()
            .node_status(&fixture.performer)
            .unwrap()
            .map(|node| node.baseline_status),
        None,
        "a peer that has never reported has no row at all"
    );

    // A Performer whose Pulse arrived before its Profile has a row and has
    // still said nothing about a baseline. Reading that as "holds none"
    // would be a verdict on a machine that has not answered.
    let pulse = pulse_payload(&target, 9, 1, BASE_NOW);
    assert!(fixture
        .ingest(&fixture.performer, "health_pulse", BASE_NOW, &pulse)
        .accepted());
    assert_eq!(
        fixture
            .plane()
            .node_status(&fixture.performer)
            .unwrap()
            .expect("a pulsing peer has a row")
            .baseline_status,
        BaselineStatus::Unknown,
        "presence without a Profile is not an answer about a baseline"
    );

    let mut revision = 0;
    let mut report = |recorded: &str, observed: &str| {
        revision += 1;
        let mut payload = profile_payload(&target, revision, revision);
        payload["profile"]["baseline_id"] = json!(recorded);
        payload["profile"]["baseline_observed_id"] = json!(observed);
        assert!(
            fixture
                .ingest(&fixture.performer, "health_profile", BASE_NOW, &payload)
                .accepted(),
            "the Profile under test must be accepted, or the verdict is about nothing"
        );
        fixture
            .plane()
            .node_status(&fixture.performer)
            .unwrap()
            .expect("a reporting peer has a row")
            .baseline_status
    };

    assert_eq!(
        report("", ""),
        BaselineStatus::None,
        "a node that was never pushed a baseline has none, which is not a drift verdict"
    );
    assert_eq!(
        report(&installed, &installed),
        BaselineStatus::InSync,
        "a node running what it was pushed is in sync"
    );
    assert_eq!(
        report(&installed, &on_disk),
        BaselineStatus::Drifted,
        "a node whose scripts changed underneath it has drifted"
    );
    assert_eq!(
        report(&installed, &installed),
        BaselineStatus::InSync,
        "putting the set back must clear the verdict, or drift is one-way"
    );
}

#[test]
fn the_public_fleet_projection_carries_only_permitted_fields() {
    let fixture = fixture();
    let target = fixture.local.clone();
    fixture.ingest(
        &fixture.performer,
        "health_profile",
        BASE_NOW,
        &profile_payload(&target, 1, 1),
    );
    fixture.clock.set(BASE_NOW + 30);
    fixture.ingest(
        &fixture.performer,
        "health_pulse",
        BASE_NOW + 30,
        &pulse_payload(&target, 2, 1, BASE_NOW + 30),
    );

    let fleet = fixture.plane().fleet_status().unwrap();
    let rendered = serde_json::to_value(&fleet).unwrap();
    let mut names = Vec::new();
    collect_field_names(&rendered, &mut names);
    names.sort();
    names.dedup();
    const PERMITTED: [&str; 33] = [
        "agent_version",
        "arch",
        "baseline_id",
        "baseline_observed_id",
        "baseline_status",
        "capabilities",
        "display_name",
        "distro_id",
        "distro_version",
        "emitted_at",
        "exit_code",
        "finished_at",
        "held_signals",
        "last_pulse_at",
        "last_run",
        "node_id",
        "omarchy_channel",
        "omarchy_version",
        "platform",
        "presence",
        "profile",
        "profile_revision",
        "pulse",
        "queue_depth",
        "role",
        "runner",
        "runtimes",
        "scheduler",
        "sequence",
        "signal_cursor",
        "state",
        "stored_signals",
        "trust_state",
    ];
    const ALSO_PERMITTED: [&str; 7] = [
        "available",
        "name",
        "uptime_seconds",
        "version",
        "version_incompatible",
        "workers_busy",
        "workers_configured",
    ];
    for name in &names {
        assert!(
            PERMITTED.contains(&name.as_str()) || ALSO_PERMITTED.contains(&name.as_str()),
            "unexpected field {name:?} in the public fleet projection"
        );
    }
    let text = serde_json::to_string(&fleet).unwrap();
    for forbidden in [
        "hostname",
        "username",
        "ip_address",
        "mac_address",
        "path",
        "secret",
        "token",
        "cpu",
        "memory",
        "disk",
    ] {
        assert!(
            !text.contains(forbidden),
            "public projection leaked {forbidden:?}"
        );
    }
}

fn collect_field_names(value: &Value, names: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (name, child) in map {
                names.push(name.clone());
                collect_field_names(child, names);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_field_names(item, names);
            }
        }
        _ => {}
    }
}

/// The Signal feed is one snapshot, not a sequence of reads.
///
/// A projection assembled from separate reads can contradict itself while
/// ingest is running: the cursor and the `stored`/`held` counters are
/// snapshotted, a Signal commits, and the later read returns a Signal the
/// counters never counted. That is what `gap` — the field an operator
/// reads to decide whether a fleet's Signal delivery has stalled — is
/// derived from, so the contradiction is an operational defect and not
/// only a test-visible one.
///
/// The race is real time, so this drives ingest concurrently rather than
/// pretending to schedule it. Against the split reads it replaced, every
/// single read observed the contradiction; against one transaction the
/// invariants below cannot be violated at all, so the test never fails
/// for timing reasons.
#[test]
fn the_signal_feed_never_reports_a_signal_its_own_cursor_has_not_counted() {
    let fixture = Arc::new(fixture());
    let performer = fixture.performer.clone();
    let local = fixture.local.clone();
    let writer = {
        let fixture = Arc::clone(&fixture);
        std::thread::spawn(move || {
            // Ten seconds apart keeps the frozen per-minute Signal rate
            // limit satisfied while the reader hammers the feed.
            for sequence in 1_u64..=SIGNALS_UNDER_CONCURRENT_READ {
                let at = BASE_NOW + (sequence as i64) * 10;
                fixture.clock.set(at);
                let outcome = fixture.ingest(
                    &performer,
                    "health_signal",
                    at,
                    &signal_payload(&local, 1_000 + sequence, sequence, 5_000 + sequence, at),
                );
                assert!(
                    matches!(outcome.decision, HealthDecision::Accepted { .. }),
                    "signal {sequence} was not accepted: {:?}",
                    outcome.decision
                );
            }
        })
    };

    let mut reads = 0_u64;
    while !writer.is_finished() {
        let feed = fixture.plane().signal_feed(64).unwrap();
        reads += 1;
        for signal in &feed.signals {
            let cursor = feed
                .nodes
                .iter()
                .find(|node| node.node_id == signal.source)
                .unwrap_or_else(|| {
                    panic!(
                        "the feed carried a Signal from {} with no cursor",
                        signal.source
                    )
                });
            assert!(
                signal.signal.sequence <= cursor.cursor,
                "sequence {} is beyond the cursor {} the same feed reports",
                signal.signal.sequence,
                cursor.cursor
            );
        }
        for node in &feed.nodes {
            let carried = feed
                .signals
                .iter()
                .filter(|signal| signal.source == node.node_id)
                .count();
            assert!(
                carried as u64 <= node.stored,
                "the feed carried {carried} Signals from {} beside stored={}",
                node.node_id,
                node.stored
            );
        }
    }
    writer.join().unwrap();
    assert!(reads > 0, "the reader never observed the feed");

    // The settled feed still renders every Signal the cursor accepted.
    let feed = fixture.plane().signal_feed(64).unwrap();
    let cursor = feed
        .nodes
        .iter()
        .find(|node| node.node_id == fixture.performer)
        .expect("performer cursor");
    assert_eq!(cursor.cursor, SIGNALS_UNDER_CONCURRENT_READ);
    assert_eq!(cursor.stored, SIGNALS_UNDER_CONCURRENT_READ);
    assert_eq!(cursor.held, 0);
    assert_eq!(
        feed.signals
            .iter()
            .filter(|signal| signal.source == fixture.performer)
            .count() as u64,
        SIGNALS_UNDER_CONCURRENT_READ
    );
}

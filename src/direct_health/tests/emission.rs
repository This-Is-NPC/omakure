use super::*;

#[test]
fn a_performer_sends_profile_first_then_pulse_at_the_frozen_cadence() {
    let fixture = fixture();
    let mut session = fixture.conductor_session();

    let profile = session.tick().expect("profile on connect");
    let (kind, payload) = decode(&fixture, &profile);
    assert_eq!(kind, "health_profile");
    assert_eq!(payload["profile"]["role"], "performer");
    assert_eq!(payload["target"], fixture.conductor);
    // Display-only echo of what this Conductor granted us locally.
    assert_eq!(
        payload["profile"]["capabilities"],
        serde_json::json!(["inventory-health"])
    );

    // Nothing more leaves the node until the Profile is acknowledged.
    assert!(session.tick().is_none());
    ack(&mut session, &payload);

    let pulse = session.tick().expect("pulse immediately after the profile");
    let (kind, payload) = decode(&fixture, &pulse);
    assert_eq!(kind, "health_pulse");
    assert_eq!(payload["pulse"]["sequence"], BASE_NOW);
    assert_eq!(payload["pulse"]["emitted_at"], BASE_NOW);
    ack(&mut session, &payload);

    // One second before the frozen interval: silence.
    fixture
        .clock
        .set(BASE_NOW + HealthReporter::pulse_interval_seconds() - 1);
    assert!(session.tick().is_none());

    // Exactly at the frozen interval: the next Pulse.
    fixture
        .clock
        .set(BASE_NOW + HealthReporter::pulse_interval_seconds());
    let pulse = session.tick().expect("pulse at the frozen cadence");
    let (_, payload) = decode(&fixture, &pulse);
    assert_eq!(
        payload["pulse"]["sequence"],
        BASE_NOW + HealthReporter::pulse_interval_seconds()
    );
}

#[test]
fn a_material_profile_change_re_emits_a_profile_with_a_higher_revision() {
    let fixture = fixture();
    let mut session = fixture.conductor_session();
    let (_, first) = decode(&fixture, &session.tick().expect("first profile"));
    ack(&mut session, &first);
    let revision = first["profile"]["profile_revision"].as_u64().unwrap();

    // No change: only Pulses flow, however many ticks pass.
    for step in 0..3 {
        fixture
            .clock
            .set(BASE_NOW + HealthReporter::pulse_interval_seconds() * step);
        if let Some(encoded) = session.tick() {
            let (kind, payload) = decode(&fixture, &encoded);
            assert_eq!(kind, "health_pulse", "unexpected {kind} without a change");
            ack(&mut session, &payload);
        }
    }

    *fixture.facts.display_name.lock().unwrap() = "workbench".to_string();
    fixture.clock.advance(1);
    let (kind, second) = decode(&fixture, &session.tick().expect("profile after change"));
    assert_eq!(kind, "health_profile");
    assert_eq!(second["profile"]["display_name"], "workbench");
    assert!(
        second["profile"]["profile_revision"].as_u64().unwrap() > revision,
        "a material change must strictly advance profile_revision"
    );
}

#[test]
fn an_unacknowledged_profile_retries_finitely_then_is_superseded() {
    let fixture = fixture();
    let mut session = fixture.conductor_session();
    assert!(session.tick().is_some(), "first profile attempt");

    // Walk the frozen 5-second acknowledgement timeout plus the 1/2/4
    // second backoff one second at a time, never acknowledging, and stop
    // when the send is finally dropped.
    let mut retries = 0;
    let mut elapsed = 0;
    while session.pending.is_some() && elapsed < 120 {
        elapsed += 1;
        fixture.clock.set(BASE_NOW + elapsed);
        if session.tick().is_some() {
            retries += 1;
        }
    }
    assert_eq!(
        retries, MAX_RETRIES,
        "exactly the frozen retry count, then the send is dropped"
    );
    assert!(
        session.pending.is_none(),
        "the final retry must leave nothing queued"
    );
    // 5 s timeout, then 5+1, 5+2, 5+4: the frozen backoff, and finite.
    assert_eq!(elapsed, 6 + 7 + 9 + 5);

    // A dropped Profile must not re-arm itself: an unreachable Conductor
    // can never be turned into an unbounded Profile loop.
    fixture
        .clock
        .set(BASE_NOW + elapsed + HealthReporter::pulse_interval_seconds());
    let next = session.tick().expect("the schedule continues with a Pulse");
    let (kind, _) = decode(&fixture, &next);
    assert_eq!(kind, "health_pulse");
}

#[test]
fn revoking_the_conductor_stops_emission_on_the_next_tick() {
    let fixture = fixture();
    let mut session = fixture.conductor_session();
    let (_, profile) = decode(&fixture, &session.tick().expect("first profile"));
    ack(&mut session, &profile);

    fixture
        .registry
        .revoke_peer(
            &fixture.conductor,
            "direct-health-tests",
            "revoked mid-session",
        )
        .expect("revoke the conductor");

    fixture
        .clock
        .advance(HealthReporter::pulse_interval_seconds());
    assert!(
        session.tick().is_none(),
        "a revoked Conductor must stop receiving health immediately"
    );
    assert!(
        session.authorization().0 == LocalRole::None,
        "a revoked peer must project no local role at all"
    );
}

#[test]
fn a_registry_failure_is_reported_rather_than_passed_off_as_a_drop() {
    let fixture = fixture();
    let mut session = fixture.session(&fixture.conductor, fixture.conductor_key);
    let payload = serde_json::json!({
        "health_version": 1,
        "message_id": opaque_id_hex(78),
        "pulse": {
            "emitted_at": BASE_NOW,
            "last_run": null,
            "profile_revision": 1,
            "runner": {
                "queue_depth": 0,
                "scheduler": "running",
                "state": "idle",
                "workers_busy": 0,
                "workers_configured": 1
            },
            "sequence": 1,
            "uptime_seconds": 1
        },
        "target": fixture.identity.public_status().node_id,
    });
    let encoded = crate::direct_transport::sign_health_envelope(
        &fixture.conductor_identity,
        "health_pulse",
        &SESSION_ID,
        [0x4e; 16],
        payload,
        BASE_NOW as u64,
    )
    .expect("sign the health message")
    .encoded();
    // Every operation opens the registry afresh, so a database that stops
    // being one mid-session is the shape of a registry error the ingest
    // path can meet: not a decision, a failure to reach one.
    std::fs::write(fixture.registry.path(), b"not a database").unwrap();

    match session.handle_envelope(&encoded) {
        HealthOutcome::Failed { kind, error } => {
            assert_eq!(kind, "health_pulse");
            assert!(
                error.contains("SQLite"),
                "the failure must carry the registry error, got {error:?}"
            );
        }
        other => panic!("a registry failure must not read as {other:?}"),
    }
}

#[test]
fn a_readable_health_message_after_revocation_is_audited_without_state_mutation() {
    let fixture = fixture();
    let mut session = fixture.session(&fixture.conductor, fixture.conductor_key);
    fixture
        .registry
        .revoke_peer(
            &fixture.conductor,
            "direct-health-tests",
            "revoked before queued message",
        )
        .expect("revoke the conductor");
    let before = fixture
        .registry
        .health_peer_states()
        .expect("read Health Plane state before ingest");
    let payload = serde_json::json!({
        "health_version": 1,
        "message_id": opaque_id_hex(77),
        "pulse": {
            "emitted_at": BASE_NOW,
            "last_run": null,
            "profile_revision": 1,
            "runner": {
                "queue_depth": 0,
                "scheduler": "running",
                "state": "idle",
                "workers_busy": 0,
                "workers_configured": 1
            },
            "sequence": 1,
            "uptime_seconds": 1
        },
        "target": fixture.identity.public_status().node_id,
    });
    let encoded = crate::direct_transport::sign_health_envelope(
        &fixture.conductor_identity,
        "health_pulse",
        &SESSION_ID,
        [0x4d; 16],
        payload,
        BASE_NOW as u64,
    )
    .expect("sign the queued health message")
    .encoded();

    assert_eq!(
        session.handle_envelope(&encoded),
        HealthOutcome::Handled,
        "revoked Health traffic is dropped without a reply"
    );
    assert_eq!(
        fixture
            .registry
            .health_peer_states()
            .expect("read Health Plane state after ingest"),
        before,
        "a revoked message must not mutate durable Health Plane state"
    );
    let audit = fixture.registry.health_audit_events(10).unwrap();
    assert_eq!(
        audit.len(),
        1,
        "one readable message must create one audit row"
    );
    assert_eq!(audit[0].event_code, "health_pulse");
    assert_eq!(audit[0].node_id, fixture.conductor);
    assert_eq!(audit[0].message_kind, "health_pulse");
    assert_eq!(audit[0].outcome, "rejected");
    assert_eq!(audit[0].error_code, Some(HealthCode::Revoked.code()));
}

#[test]
fn a_performer_peer_never_receives_profile_or_pulse_from_this_node() {
    let fixture = fixture();
    let (_, _, performer_key) = peer_identity(12);
    let mut session = fixture.session(&fixture.performer, performer_key);
    for step in 0..4 {
        fixture
            .clock
            .set(BASE_NOW + HealthReporter::pulse_interval_seconds() * step);
        assert!(
            session.tick().is_none(),
            "this node is the Conductor for that peer and must never report to it"
        );
    }
}

#[test]
fn an_unsupported_version_error_opens_the_frozen_backoff() {
    let fixture = fixture();
    let mut session = fixture.conductor_session();
    let (_, profile) = decode(&fixture, &session.tick().expect("first profile"));
    let acked = profile["message_id"].as_str().unwrap();
    session.absorb_reply(
        &serde_json::json!({
            "error": {
                "accepted": false,
                "acked_message_id": acked,
                "code": HealthCode::UnsupportedVersion.code(),
                "reason": HealthCode::UnsupportedVersion.name(),
            }
        }),
        Some(HealthKind::Error),
        true,
    );
    fixture
        .clock
        .advance(VERSION_INCOMPATIBLE_BACKOFF_SECONDS - 1);
    assert!(
        session.tick().is_none(),
        "the frozen 300-second version backoff must silence this node"
    );
    fixture.clock.advance(1);
    assert!(
        session.tick().is_some(),
        "the node retries once the frozen backoff expires"
    );
}

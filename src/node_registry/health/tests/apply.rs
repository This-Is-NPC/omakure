use super::*;

#[test]
fn authorization_projection_reports_role_and_capabilities_without_mutating() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let before = fixture.registry.audit_events().unwrap().len();

    let authorization = fixture
        .registry
        .health_authorization(&node_id)
        .unwrap()
        .expect("projection");
    assert_eq!(authorization.state, PeerState::Active);
    assert_eq!(authorization.role, PeerRole::Performer);
    assert_eq!(
        authorization.capabilities,
        vec!["inventory-health".to_string(), "notifications".to_string()]
    );
    // Reading authorization creates nothing, not even an audit row.
    assert_eq!(fixture.registry.audit_events().unwrap().len(), before);
    assert!(fixture.registry.health_peer_states().unwrap().is_empty());

    let (unknown, _, _) = peer_identity(77);
    assert!(fixture
        .registry
        .health_authorization(&unknown)
        .unwrap()
        .is_none());
}

#[test]
fn profile_and_pulse_keep_exactly_one_latest_row_per_peer() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 1, 1),
            BASE_NOW
        ),
        accepted(0)
    );
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 2, 2),
            BASE_NOW + 5
        ),
        accepted(0)
    );
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &pulse(&local, 3, 1, BASE_NOW + 10),
            BASE_NOW + 10
        ),
        accepted(0)
    );
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &pulse(&local, 4, 2, BASE_NOW + 30),
            BASE_NOW + 30
        ),
        accepted(0)
    );

    let connection = Connection::open(fixture.registry.path()).unwrap();
    for table in ["health_profiles", "health_pulses"] {
        let rows: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 1, "{table} retains exactly the latest row");
    }
    let snapshot = fixture
        .registry
        .health_node_snapshot(&node_id, BASE_NOW + 30)
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(snapshot.profile.unwrap().profile_revision, 2);
    assert_eq!(snapshot.pulse.unwrap().sequence, 2);
    assert_eq!(snapshot.state.last_pulse_at, Some(BASE_NOW + 30));
}

#[test]
fn duplicates_replays_and_regressions_are_rejected_without_mutation() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    apply(
        &fixture.registry,
        &node_id,
        &profile(&local, 1, 2),
        BASE_NOW,
    );
    // Same message ID.
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 1, 3),
            BASE_NOW + 1
        ),
        HealthDecision::Rejected(HealthCode::Replay)
    );
    // Fresh message ID but an equal or lower revision.
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 2, 2),
            BASE_NOW + 2
        ),
        HealthDecision::Rejected(HealthCode::Replay)
    );
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 3, 1),
            BASE_NOW + 3
        ),
        HealthDecision::Rejected(HealthCode::Replay)
    );
    let snapshot = fixture
        .registry
        .health_node_snapshot(&node_id, BASE_NOW + 3)
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(snapshot.profile.unwrap().profile_revision, 2);

    apply(
        &fixture.registry,
        &node_id,
        &pulse(&local, 10, 5, BASE_NOW + 20),
        BASE_NOW + 20,
    );
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &pulse(&local, 11, 5, BASE_NOW + 40),
            BASE_NOW + 40
        ),
        HealthDecision::Rejected(HealthCode::Replay)
    );
}

#[test]
fn signal_cursor_accepts_in_order_holds_gaps_and_refuses_far_future() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 1, 1, 101, BASE_NOW),
            BASE_NOW
        ),
        accepted(1)
    );
    // A gap inside the 32-entry reorder window is held; the cursor stalls.
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 2, 3, 103, BASE_NOW + 10),
            BASE_NOW + 10
        ),
        HealthDecision::Held { cursor: 1 }
    );
    // Beyond the window it is refused outright and never buffered.
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 3, 40, 140, BASE_NOW + 20),
            BASE_NOW + 20
        ),
        HealthDecision::Rejected(HealthCode::Reordered)
    );
    // Filling the gap advances the cursor across the promoted Signal.
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 4, 2, 102, BASE_NOW + 30),
            BASE_NOW + 30
        ),
        accepted(3)
    );
    let signals = fixture
        .registry
        .health_signals(&node_id, 64, BASE_NOW + 30)
        .unwrap();
    assert_eq!(
        signals
            .iter()
            .map(|entry| entry.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    // A held Signal that never fills its gap expires without moving the cursor.
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 5, 6, 106, BASE_NOW + 40),
            BASE_NOW + 40
        ),
        HealthDecision::Held { cursor: 3 }
    );
    let report = fixture.registry.health_prune(BASE_NOW + 200).unwrap();
    assert_eq!(report.expired_held_signals, 1);
    assert_eq!(
        fixture
            .registry
            .health_peer_states()
            .unwrap()
            .first()
            .unwrap()
            .cursor,
        3
    );
}

#[test]
fn signal_identity_is_idempotent_across_fresh_message_ids() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 1, 1, 101, BASE_NOW),
            BASE_NOW
        ),
        accepted(1)
    );
    // A resend reuses signal_id and sequence with a fresh message_id.
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 2, 1, 101, BASE_NOW + 10),
            BASE_NOW + 10
        ),
        HealthDecision::Rejected(HealthCode::Replay)
    );
    // A different sequence carrying an already stored signal_id is also a replay.
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 3, 2, 101, BASE_NOW + 20),
            BASE_NOW + 20
        ),
        HealthDecision::Rejected(HealthCode::Replay)
    );
    assert_eq!(
        fixture
            .registry
            .health_signals(&node_id, 64, BASE_NOW + 20)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn state_and_cursor_survive_a_restart() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    apply(
        &fixture.registry,
        &node_id,
        &profile(&local, 1, 4),
        BASE_NOW,
    );
    apply(
        &fixture.registry,
        &node_id,
        &pulse(&local, 2, 9, BASE_NOW + 5),
        BASE_NOW + 5,
    );
    apply(
        &fixture.registry,
        &node_id,
        &signal(&local, 3, 1, 101, BASE_NOW + 10),
        BASE_NOW + 10,
    );

    let reopened = reopen(&fixture);
    let state = &reopened.health_peer_states().unwrap()[0];
    assert_eq!(state.cursor, 1);
    assert_eq!(state.last_profile_revision, 4);
    assert_eq!(state.last_pulse_sequence, 9);
    assert_eq!(state.stored_signals, 1);
    // The replay key survives too, so a restart cannot reopen a replay window.
    assert_eq!(
        apply(&reopened, &node_id, &profile(&local, 1, 5), BASE_NOW + 20),
        HealthDecision::Rejected(HealthCode::Replay)
    );
}

#[test]
fn a_failed_apply_rolls_back_completely_and_preserves_evidence() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    let connection = Connection::open(fixture.registry.path()).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER health_profiles_injected_failure
                 BEFORE INSERT ON health_profiles
                 BEGIN SELECT RAISE(ABORT, 'injected health storage failure'); END;",
        )
        .unwrap();

    assert!(fixture
        .registry
        .apply_health_message(HealthApplyRequest {
            sender: &node_id,
            payload: &profile(&local, 1, 1),
            created_at: BASE_NOW,
            now: BASE_NOW,
            message_bytes: 1_327,
        })
        .is_err());

    // Nothing partial survives: no peer row, no replay key, no audit row.
    assert!(fixture.registry.health_peer_states().unwrap().is_empty());
    let replays: i64 = connection
        .query_row("SELECT COUNT(*) FROM health_replay_keys", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(replays, 0);
    assert!(fixture.registry.health_audit_events(10).unwrap().is_empty());
    // Trust and identity evidence is untouched by the failure.
    let trusted: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM trusted_peers WHERE state = 'active'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trusted, 1);

    connection
        .execute_batch("DROP TRIGGER health_profiles_injected_failure;")
        .unwrap();
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 1, 1),
            BASE_NOW
        ),
        accepted(0)
    );
}

#[test]
fn concurrent_ingest_applies_each_message_exactly_once() {
    let fixture = Arc::new(fixture());
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    let mut handles = Vec::new();
    for index in 0..8_u64 {
        let fixture = Arc::clone(&fixture);
        let node_id = node_id.clone();
        let local = local.clone();
        handles.push(std::thread::spawn(move || {
            apply(
                &fixture.registry,
                &node_id,
                &profile(&local, 500, 1 + index),
                BASE_NOW,
            )
        }));
    }
    let decisions: Vec<HealthDecision> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    let accepted_count = decisions
        .iter()
        .filter(|decision| matches!(decision, HealthDecision::Accepted { .. }))
        .count();
    assert_eq!(
        accepted_count, 1,
        "one shared message_id may be applied exactly once"
    );
    let connection = Connection::open(fixture.registry.path()).unwrap();
    let replays: i64 = connection
        .query_row("SELECT COUNT(*) FROM health_replay_keys", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(replays, 1);
    let profiles: i64 = connection
        .query_row("SELECT COUNT(*) FROM health_profiles", [], |row| row.get(0))
        .unwrap();
    assert_eq!(profiles, 1);
}

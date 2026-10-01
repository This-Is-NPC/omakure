use super::*;

#[test]
fn signal_inbox_and_storage_stay_inside_their_frozen_bounds_under_flood() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    let mut now = BASE_NOW;
    let mut accepted_count = 0_u64;
    let mut queue_full = 0_u64;
    for index in 1..=100_u64 {
        now += 7;
        let decision = apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 1_000 + index, index, 2_000 + index, now),
            now,
        );
        match decision {
            HealthDecision::Accepted { .. } => accepted_count += 1,
            HealthDecision::Rejected(HealthCode::QueueFull) => queue_full += 1,
            // Once the inbox is full the cursor stalls, so later sequences
            // eventually leave the 32-entry reorder window. Both outcomes
            // are bounded rejections that store nothing.
            HealthDecision::Rejected(HealthCode::RateLimited)
            | HealthDecision::Rejected(HealthCode::Reordered) => {}
            other => panic!("unexpected decision {other:?}"),
        }
    }
    assert_eq!(accepted_count, SIGNAL_INBOX_CAPACITY as u64);
    assert!(queue_full > 0, "the inbox bound must reject the overflow");
    let connection = Connection::open(fixture.registry.path()).unwrap();
    let stored: i64 = connection
        .query_row("SELECT COUNT(*) FROM health_signals", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, SIGNAL_INBOX_CAPACITY);

    let peers = fixture.registry.health_peer_states().unwrap().len() as i64;
    let bytes = fixture.registry.health_storage_bytes().unwrap();
    assert!(bytes <= peers * WORST_CASE_BYTES_PER_PERFORMER + MAX_AUDIT_ROWS * AUDIT_ROW_BYTES);
    assert!(bytes < STORAGE_CEILING_BYTES);
}

#[test]
fn frozen_per_row_caps_multiply_out_to_the_frozen_ceiling() {
    assert_eq!(
        MAX_STORED_PROFILE_BYTES
            + MAX_STORED_PULSE_BYTES
            + SIGNAL_INBOX_CAPACITY * MAX_STORED_SIGNAL_BYTES,
        WORST_CASE_BYTES_PER_PERFORMER
    );
    assert_eq!(
        MAX_PERFORMERS_PER_CONDUCTOR * WORST_CASE_BYTES_PER_PERFORMER
            + MAX_REPLAY_ROWS * REPLAY_ROW_BYTES
            + MAX_AUDIT_ROWS * AUDIT_ROW_BYTES,
        STORAGE_CEILING_BYTES
    );
}

#[test]
fn rate_limits_bound_a_flood_from_one_peer() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    // Profiles are capped per hour.
    let mut profiles = 0;
    for index in 1..=30_u64 {
        match apply(
            &fixture.registry,
            &node_id,
            &profile(&local, index, index),
            BASE_NOW,
        ) {
            HealthDecision::Accepted { .. } => profiles += 1,
            HealthDecision::Rejected(HealthCode::RateLimited) => {}
            other => panic!("unexpected decision {other:?}"),
        }
    }
    assert_eq!(profiles, MAX_PROFILES_PER_PEER_PER_HOUR);

    // Pulses honour the minimum accepted interval.
    let fixture = super::fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &pulse(&local, 1, 1, BASE_NOW),
            BASE_NOW
        ),
        accepted(0)
    );
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &pulse(&local, 2, 2, BASE_NOW + 9),
            BASE_NOW + 9
        ),
        HealthDecision::Rejected(HealthCode::RateLimited)
    );
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &pulse(&local, 3, 2, BASE_NOW + 10),
            BASE_NOW + 10
        ),
        accepted(0)
    );
}

#[test]
fn freshness_boundaries_are_inclusive_exactly_where_the_contract_says() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    let now = BASE_NOW + 1_000;
    assert_eq!(
        apply_at(
            &fixture.registry,
            &node_id,
            &profile(&local, 1, 1),
            now - MAX_AGE_SECONDS,
            now
        ),
        accepted(0)
    );
    assert_eq!(
        apply_at(
            &fixture.registry,
            &node_id,
            &profile(&local, 2, 2),
            now - MAX_AGE_SECONDS - 1,
            now
        ),
        HealthDecision::Rejected(HealthCode::Stale)
    );
    assert_eq!(
        apply_at(
            &fixture.registry,
            &node_id,
            &profile(&local, 3, 3),
            now + MAX_FUTURE_SKEW_SECONDS,
            now
        ),
        accepted(0)
    );
    assert_eq!(
        apply_at(
            &fixture.registry,
            &node_id,
            &profile(&local, 4, 4),
            now + MAX_FUTURE_SKEW_SECONDS + 1,
            now
        ),
        HealthDecision::Rejected(HealthCode::Future)
    );
}

#[test]
fn authorization_rejections_precede_freshness() {
    let fixture = fixture();
    let local = fixture.registry.local_node_id().to_string();
    let (stranger, _, _) = peer_identity(9);
    let conductor = trust(&fixture.registry, 2, PeerRole::Conductor, &[]);
    let limited = trust(&fixture.registry, 3, PeerRole::Performer, &[]);
    let future = BASE_NOW + MAX_FUTURE_SKEW_SECONDS + 1;

    assert_eq!(
        apply_at(
            &fixture.registry,
            &stranger,
            &profile(&local, 1, 1),
            future,
            BASE_NOW
        ),
        HealthDecision::Rejected(HealthCode::Revoked)
    );
    assert_eq!(
        apply_at(
            &fixture.registry,
            &conductor,
            &profile(&local, 2, 1),
            future,
            BASE_NOW
        ),
        HealthDecision::Rejected(HealthCode::WrongRole)
    );
    assert_eq!(
        apply_at(
            &fixture.registry,
            &limited,
            &profile(&local, 3, 1),
            future,
            BASE_NOW
        ),
        HealthDecision::Rejected(HealthCode::MissingCapability)
    );
    assert!(fixture.registry.health_peer_states().unwrap().is_empty());
}

#[test]
fn rate_rejection_precedes_message_replay_without_consuming_the_replay_key() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();
    let first = pulse(&local, 1, 1, BASE_NOW);

    assert_eq!(
        apply(&fixture.registry, &node_id, &first, BASE_NOW),
        accepted(0)
    );
    assert_eq!(
        apply(&fixture.registry, &node_id, &first, BASE_NOW + 1),
        HealthDecision::Rejected(HealthCode::RateLimited)
    );
    assert_eq!(
        apply(&fixture.registry, &node_id, &first, BASE_NOW + 10),
        HealthDecision::Rejected(HealthCode::Replay)
    );
    let connection = Connection::open(fixture.registry.path()).unwrap();
    let replay_keys: i64 = connection
        .query_row("SELECT COUNT(*) FROM health_replay_keys", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(replay_keys, 1);
    assert_eq!(
        fixture.registry.health_peer_states().unwrap()[0].last_pulse_sequence,
        1
    );
}

#[test]
fn message_replay_precedes_signal_ordering() {
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

    let out_of_order = signal(
        &local,
        1,
        crate::health_plane::bounds::REORDER_BUFFER_ENTRIES + 1,
        10,
        BASE_NOW,
    );
    assert_eq!(
        apply(&fixture.registry, &node_id, &out_of_order, BASE_NOW),
        HealthDecision::Rejected(HealthCode::Replay)
    );
    let fresh_id = signal(
        &local,
        2,
        crate::health_plane::bounds::REORDER_BUFFER_ENTRIES + 1,
        11,
        BASE_NOW,
    );
    assert_eq!(
        apply(&fixture.registry, &node_id, &fresh_id, BASE_NOW),
        HealthDecision::Rejected(HealthCode::Reordered)
    );
    let connection = Connection::open(fixture.registry.path()).unwrap();
    let signals: i64 = connection
        .query_row("SELECT COUNT(*) FROM health_signals", [], |row| row.get(0))
        .unwrap();
    assert_eq!(signals, 0);
}

#[test]
fn unauthorized_and_revoked_peers_cannot_mutate_any_health_state() {
    let fixture = fixture();
    let local = fixture.registry.local_node_id().to_string();
    let performer = performer(&fixture.registry);
    let limited = trust(&fixture.registry, 2, PeerRole::Performer, &["remote-run"]);
    let conductor = trust(&fixture.registry, 3, PeerRole::Conductor, &[]);
    let (stranger, _, _) = peer_identity(9);

    // A peer with no registry row at all.
    assert_eq!(
        apply(
            &fixture.registry,
            &stranger,
            &profile(&local, 1, 1),
            BASE_NOW
        ),
        HealthDecision::Rejected(HealthCode::Revoked)
    );
    // A trusted peer without the required capability.
    assert_eq!(
        apply(
            &fixture.registry,
            &limited,
            &profile(&local, 2, 1),
            BASE_NOW
        ),
        HealthDecision::Rejected(HealthCode::MissingCapability)
    );
    // A Conductor cannot report health.
    assert_eq!(
        apply(
            &fixture.registry,
            &conductor,
            &profile(&local, 3, 1),
            BASE_NOW
        ),
        HealthDecision::Rejected(HealthCode::WrongRole)
    );
    // A Performer cannot acknowledge.
    let ack = HealthPayload {
        message_id: opaque_id_hex(4),
        target: local.clone(),
        body: HealthBody::Ack(crate::health_plane::model::AckBody {
            accepted: true,
            acked_message_id: opaque_id_hex(1),
            cursor: 0,
        }),
    };
    assert_eq!(
        apply(&fixture.registry, &performer, &ack, BASE_NOW),
        HealthDecision::Rejected(HealthCode::WrongRole)
    );

    assert!(fixture.registry.health_peer_states().unwrap().is_empty());
    let connection = Connection::open(fixture.registry.path()).unwrap();
    for table in ["health_profiles", "health_pulses", "health_signals"] {
        let rows: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "{table} must stay empty");
    }
    // Trust rows are unchanged: nothing was created and nothing reactivated.
    let trusted: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM trusted_peers WHERE state = 'active'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trusted, 3);
}

#[test]
fn the_two_hundred_fifty_seventh_peer_is_refused_without_changing_trust() {
    let fixture = fixture();
    let local = fixture.registry.local_node_id().to_string();
    let mut peers = Vec::new();
    for seed in 0..(MAX_PERFORMERS_PER_CONDUCTOR as u32 + 1) {
        peers.push(trust(
            &fixture.registry,
            seed,
            PeerRole::Performer,
            &["inventory-health"],
        ));
    }
    for (index, node_id) in peers.iter().take(peers.len() - 1).enumerate() {
        assert_eq!(
            apply(
                &fixture.registry,
                node_id,
                &profile(&local, 10_000 + index as u64, 1),
                BASE_NOW
            ),
            accepted(0)
        );
    }
    assert_eq!(
        fixture.registry.health_peer_states().unwrap().len(),
        MAX_PERFORMERS_PER_CONDUCTOR as usize
    );
    assert_eq!(
        apply(
            &fixture.registry,
            peers.last().unwrap(),
            &profile(&local, 99_999, 1),
            BASE_NOW
        ),
        HealthDecision::Rejected(HealthCode::QueueFull)
    );
    assert_eq!(
        fixture.registry.health_peer_states().unwrap().len(),
        MAX_PERFORMERS_PER_CONDUCTOR as usize
    );
}

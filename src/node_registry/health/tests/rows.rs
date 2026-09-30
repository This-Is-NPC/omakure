use super::*;

#[test]
fn a_corrupt_health_row_is_quarantined_and_audited_without_disabling_the_peer() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    apply(
        &fixture.registry,
        &node_id,
        &profile(&local, 1, 1),
        BASE_NOW,
    );
    apply(
        &fixture.registry,
        &node_id,
        &pulse(&local, 2, 1, BASE_NOW + 5),
        BASE_NOW + 5,
    );
    let connection = Connection::open(fixture.registry.path()).unwrap();
    connection
        .execute("UPDATE health_profiles SET runtimes = 'not-json'", [])
        .unwrap();

    let fleet = fixture
        .registry
        .health_node_snapshot(&node_id, BASE_NOW + 10)
        .unwrap()
        .unwrap();
    assert!(fleet.snapshot.profile.is_none());

    let snapshot = fixture
        .registry
        .health_peer_snapshot(&node_id, BASE_NOW + 10)
        .unwrap()
        .unwrap();
    assert!(snapshot.profile.is_none(), "the corrupt row is quarantined");
    assert!(snapshot.pulse.is_some(), "the healthy row still reads");
    let audit = fixture.registry.health_audit_events(10).unwrap();
    assert!(audit.iter().any(|event| {
        event.event_code == "corrupt_row"
            && event.error_code == Some(HealthCode::CorruptState.code())
    }));
    let rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM health_profiles", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    // The peer keeps reporting; only the single bad row was discarded.
    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 3, 2),
            BASE_NOW + 20
        ),
        accepted(0)
    );
}

#[test]
fn deferred_corrupt_health_cleanup_keeps_newer_replacements_without_audit() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    apply(
        &fixture.registry,
        &node_id,
        &profile(&local, 1, 1),
        BASE_NOW,
    );
    apply(
        &fixture.registry,
        &node_id,
        &pulse(&local, 2, 1, BASE_NOW + 5),
        BASE_NOW + 5,
    );

    let mut connection = Connection::open(fixture.registry.path()).unwrap();
    connection
        .execute(
            "UPDATE health_profiles SET runtimes = 'not-json'
                 WHERE node_id = ?1",
            [&node_id],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE health_pulses SET last_run = 'not-json'
                 WHERE node_id = ?1",
            [&node_id],
        )
        .unwrap();

    let corrupt = {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .unwrap();
        let state = load_peer_state(&transaction, &node_id).unwrap().unwrap();
        let (_, corrupt) = fleet_peer_in(&transaction, state).unwrap();
        transaction.commit().unwrap();
        corrupt
    };
    assert!(corrupt.iter().any(|row| {
        matches!(
            row.identity,
            CorruptHealthIdentity::Profile {
                profile_revision: 1
            }
        )
    }));
    assert!(corrupt
        .iter()
        .any(|row| { matches!(row.identity, CorruptHealthIdentity::Pulse { sequence: 1 }) }));
    drop(connection);

    // These writes happen after observation but before deferred cleanup.
    apply(
        &fixture.registry,
        &node_id,
        &profile(&local, 3, 2),
        BASE_NOW + 10,
    );
    apply(
        &fixture.registry,
        &node_id,
        &pulse(&local, 4, 2, BASE_NOW + 15),
        BASE_NOW + 15,
    );
    cleanup_corrupt_health_rows(&fixture.registry, &corrupt, BASE_NOW + 20).unwrap();

    let connection = Connection::open(fixture.registry.path()).unwrap();
    let profile_revision: i64 = connection
        .query_row(
            "SELECT profile_revision FROM health_profiles WHERE node_id = ?1",
            [&node_id],
            |row| row.get(0),
        )
        .unwrap();
    let sequence: i64 = connection
        .query_row(
            "SELECT sequence FROM health_pulses WHERE node_id = ?1",
            [&node_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(profile_revision, 2);
    assert_eq!(sequence, 2);
    assert!(fixture
        .registry
        .health_audit_events(100)
        .unwrap()
        .iter()
        .all(|event| event.event_code != "corrupt_row"));
}

#[test]
fn observational_fleet_snapshot_quarantines_corrupt_profile_without_error() {
    let fixture = fixture();
    let identity = NodeIdentity::load_existing(&fixture.context).unwrap();
    let observational =
        NodeRegistry::open_health_observational(&fixture.context, identity.public_status())
            .unwrap();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    apply(
        &fixture.registry,
        &node_id,
        &profile(&local, 1, 1),
        BASE_NOW,
    );
    let connection = Connection::open(fixture.registry.path()).unwrap();
    connection
        .execute("UPDATE health_profiles SET runtimes = 'not-json'", [])
        .unwrap();

    let fleet = observational
        .health_fleet_snapshot(BASE_NOW + 10)
        .expect("corrupt cleanup must not fail on the observational connection");
    let peer = fleet
        .iter()
        .find(|peer| peer.snapshot.state.node_id == node_id)
        .expect("fleet peer");
    assert!(
        peer.snapshot.profile.is_none(),
        "the corrupt row is quarantined"
    );

    let rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM health_profiles", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    let audit = fixture.registry.health_audit_events(10).unwrap();
    assert!(audit.iter().any(|event| {
        event.event_code == "corrupt_row"
            && event.error_code == Some(HealthCode::CorruptState.code())
    }));
}

use super::*;

#[test]
fn revocation_stops_ingest_and_purges_derived_state_without_touching_trust() {
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
        &signal(&local, 2, 1, 101, BASE_NOW + 5),
        BASE_NOW + 5,
    );
    assert_eq!(fixture.registry.health_peer_states().unwrap().len(), 1);

    fixture
        .registry
        .revoke_peer(&node_id, "operator", "lost device")
        .unwrap();

    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &profile(&local, 3, 2),
            BASE_NOW + 10
        ),
        HealthDecision::Rejected(HealthCode::Revoked)
    );
    let purged = fixture
        .registry
        .health_purge_revoked(BASE_NOW + 20)
        .unwrap();
    assert_eq!(purged, vec![node_id.clone()]);
    assert!(fixture.registry.health_peer_states().unwrap().is_empty());

    let connection = Connection::open(fixture.registry.path()).unwrap();
    for table in ["health_profiles", "health_pulses", "health_signals"] {
        let rows: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "{table} must be purged");
    }
    // The revocation itself is retained: health cleanup never rewrites trust.
    let revocations: i64 = connection
        .query_row("SELECT COUNT(*) FROM revocations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(revocations, 1);
    let trust_state: String = connection
        .query_row(
            "SELECT state FROM trusted_peers WHERE node_id = ?1",
            params![node_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(trust_state, "revoked");
}

#[test]
fn pruning_enforces_signal_replay_and_audit_retention() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    apply(
        &fixture.registry,
        &node_id,
        &signal(&local, 1, 1, 101, BASE_NOW),
        BASE_NOW,
    );
    let later = BASE_NOW + SIGNAL_RETENTION_SECONDS + 1;
    let report = fixture.registry.health_prune(later).unwrap();
    assert_eq!(report.expired_signals, 1);
    assert!(report.expired_replay_keys >= 1);
    assert!(fixture
        .registry
        .health_signals(&node_id, 64, later)
        .unwrap()
        .is_empty());

    // The replay security floor keeps young keys even past their retention.
    apply(
        &fixture.registry,
        &node_id,
        &signal(&local, 2, 2, 102, later),
        later,
    );
    let connection = Connection::open(fixture.registry.path()).unwrap();
    connection
        .execute(
            "UPDATE health_replay_keys SET expires_at = first_seen + 1",
            [],
        )
        .unwrap();
    let report = fixture
        .registry
        .health_prune(later + REPLAY_SECURITY_FLOOR_SECONDS - 1)
        .unwrap();
    assert_eq!(report.expired_replay_keys, 0);
    let report = fixture
        .registry
        .health_prune(later + REPLAY_SECURITY_FLOOR_SECONDS)
        .unwrap();
    assert_eq!(report.expired_replay_keys, 1);
}

#[test]
fn version_incompatibility_expires_without_operator_action() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    apply(
        &fixture.registry,
        &node_id,
        &profile(&local, 1, 1),
        BASE_NOW,
    );
    fixture
        .registry
        .mark_health_version_incompatible(&node_id, BASE_NOW)
        .unwrap();
    assert!(
        fixture.registry.health_peer_states().unwrap()[0].version_incompatible,
        "the peer must be marked"
    );
    fixture
        .registry
        .health_prune(BASE_NOW + VERSION_INCOMPATIBLE_EXPIRY_SECONDS - 1)
        .unwrap();
    assert!(fixture.registry.health_peer_states().unwrap()[0].version_incompatible);
    fixture
        .registry
        .health_prune(BASE_NOW + VERSION_INCOMPATIBLE_EXPIRY_SECONDS)
        .unwrap();
    assert!(!fixture.registry.health_peer_states().unwrap()[0].version_incompatible);
}

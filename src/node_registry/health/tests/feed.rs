use super::*;

#[test]
fn pure_health_reads_succeed_while_writer_is_reserved() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let mut writer = Connection::open(fixture.registry.path()).unwrap();
    super::super::super::open::configure_connection(&mut writer).unwrap();
    let writer_transaction = writer
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();

    assert!(fixture.registry.health_fleet_snapshot(BASE_NOW).is_ok());
    assert!(
        fixture
            .registry
            .health_node_snapshot(&node_id, BASE_NOW)
            .is_ok()
    );
    assert!(fixture.registry.health_signal_feed(16, BASE_NOW).is_ok());

    writer_transaction.rollback().unwrap();
}

#[test]
fn a_corrupt_signal_in_feed_is_quarantined_after_snapshot() {
    let fixture = fixture();
    let node_id = performer(&fixture.registry);
    let local = fixture.registry.local_node_id().to_string();

    assert_eq!(
        apply(
            &fixture.registry,
            &node_id,
            &signal(&local, 1, 1, 101, BASE_NOW),
            BASE_NOW,
        ),
        accepted(1)
    );
    let connection = Connection::open(fixture.registry.path()).unwrap();
    connection
        .execute("UPDATE health_signals SET run = 'not-json'", [])
        .unwrap();

    let feed = fixture
        .registry
        .health_signal_feed(16, BASE_NOW + 1)
        .unwrap();
    assert!(feed.signals.is_empty(), "the corrupt Signal is hidden");
    let rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM health_signals", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    let audit = fixture.registry.health_audit_events(10).unwrap();
    assert!(audit.iter().any(|event| {
        event.event_code == "corrupt_row"
            && event.error_code == Some(HealthCode::CorruptState.code())
    }));
}

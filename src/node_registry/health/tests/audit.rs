use super::*;

#[test]
fn audit_rows_carry_only_redacted_metadata() {
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
        &profile(&local, 1, 2),
        BASE_NOW + 1,
    );
    let events = fixture.registry.health_audit_events(10).unwrap();
    assert_eq!(events.len(), 2);
    for event in &events {
        assert_eq!(event.node_id, node_id);
        assert_eq!(event.message_kind, "health_profile");
        assert!(event.byte_count > 0);
        assert!(matches!(event.outcome.as_str(), "accepted" | "rejected"));
        let rendered = format!("{event:?}");
        for forbidden in [
            "workshop-laptop",
            "arch",
            "rolling",
            "bash",
            "5.2.37",
            "secret://",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "audit row leaked {forbidden:?}: {rendered}"
            );
        }
    }
    assert_eq!(
        events
            .iter()
            .find(|event| event.outcome == "rejected")
            .unwrap()
            .error_code,
        Some(HealthCode::Replay.code())
    );
}

#[test]
fn transport_audit_survives_main_file_copy_while_observational_connection_is_open() {
    let fixture = fixture();
    let identity = NodeIdentity::load_existing(&fixture.context).unwrap();
    let observational =
        NodeRegistry::open_health_observational(&fixture.context, identity.public_status())
            .unwrap();
    observational
        .health_fleet_snapshot(BASE_NOW)
        .expect("observational connection must be query-only before writer audit");
    let writer = NodeRegistry::open_existing(&fixture.context, identity.public_status()).unwrap();
    let local = writer.local_node_id().to_string();
    writer
        .record_transport_audit(crate::node_registry::TransportAudit {
            event_type: "unsupported_downgrade",
            node_id: &local,
            session_id: None,
            direction: None,
            byte_count: 0,
            outcome: "rejected",
            error_code: Some(1001),
            cue: None,
        })
        .unwrap();
    let snapshot = fixture._temp.path().join("node.sqlite.snapshot");
    std::fs::copy(fixture.registry.path(), &snapshot).unwrap();

    let connection = Connection::open(&snapshot).unwrap();
    let rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM transport_audit WHERE error_code = 1001",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1);
    observational
        .health_fleet_snapshot(BASE_NOW)
        .expect("observational connection must stay open after main-file copy");
}

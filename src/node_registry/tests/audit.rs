use super::*;

#[test]
fn cue_rate_limit_is_durable_and_allows_the_frozen_burst() {
    let temp = TempDir::new().unwrap();
    let node_context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&node_context).unwrap();
    let registry = NodeRegistry::open(&node_context, identity.public_status()).unwrap();
    let peer = identity.public_status().node_id.clone();

    for _ in 0..(crate::remote_cue::MAX_CUES_PER_MINUTE + crate::remote_cue::RATE_BURST_ALLOWANCE) {
        assert!(registry.consume_cue_rate(&peer, 100).unwrap());
    }
    assert!(!registry.consume_cue_rate(&peer, 101).unwrap());
    drop(registry);
    drop(identity);
    let identity = NodeIdentity::load_or_initialize(&node_context).unwrap();
    let registry = NodeRegistry::open(&node_context, identity.public_status()).unwrap();
    assert!(!registry.consume_cue_rate(&peer, 101).unwrap());
    assert!(registry.consume_cue_rate(&peer, 160).unwrap());
}

#[test]
fn cue_audit_persists_correlation_and_leaves_plain_rows_null() {
    let temp = TempDir::new().unwrap();
    let node_context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&node_context).unwrap();
    let registry = NodeRegistry::open(&node_context, identity.public_status()).unwrap();
    let cue_id = "0123456789abcdef0123456789abcdef";

    registry
        .record_cue_transport_audit(
            "cue_rejected",
            &identity.public_status().node_id,
            None,
            None,
            0,
            "rejected",
            Some(1206),
            Some(cue_id),
            Some("deploy.sh"),
            Some("approved by operator"),
        )
        .unwrap();
    registry
        .record_transport_audit(
            "plain_event",
            &identity.public_status().node_id,
            None,
            None,
            0,
            "accepted",
            None,
        )
        .unwrap();

    let connection = Connection::open(node_context.database_path()).unwrap();
    let stored: (String, String, String) = connection
        .query_row(
            "SELECT cue_id, cue_script, cue_reason FROM transport_audit
                 WHERE cue_id = ?1",
            [cue_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        stored,
        (
            cue_id.into(),
            "deploy.sh".into(),
            "approved by operator".into()
        )
    );
    let plain_nulls: (Option<String>, Option<String>, Option<String>) = connection
        .query_row(
            "SELECT cue_id, cue_script, cue_reason FROM transport_audit
                 WHERE event_type = 'plain_event'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(plain_nulls, (None, None, None));
}

#[test]
fn transport_audit_validation_keeps_cue_precedence_and_metadata_errors() {
    let temp = TempDir::new().unwrap();
    let node_context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&node_context).unwrap();
    let registry = NodeRegistry::open(&node_context, identity.public_status()).unwrap();
    let node_id = identity.public_status().node_id.clone();

    let invalid_event = registry
        .record_transport_audit("", &node_id, None, Some(2), 0, "rejected", None)
        .unwrap_err();
    assert_eq!(
        invalid_event.to_string(),
        "node registry input is invalid: transport event type is empty, oversized, or contains control characters"
    );

    let incomplete_cue = registry
        .record_cue_transport_audit(
            "",
            &node_id,
            None,
            Some(2),
            0,
            "rejected",
            None,
            Some("bad"),
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(
        incomplete_cue.to_string(),
        "node registry input is invalid: Cue audit correlation must be complete"
    );

    let invalid_direction = registry
        .record_cue_transport_audit(
            "cue_rejected",
            &node_id,
            None,
            Some(2),
            0,
            "rejected",
            None,
            None,
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(
        invalid_direction.to_string(),
        "node registry input is invalid: transport audit metadata is invalid"
    );
}

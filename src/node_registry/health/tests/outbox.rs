use super::*;

#[test]
fn outbox_is_bounded_drops_the_oldest_and_retires_on_acknowledgement() {
    let fixture = fixture();
    let conductor = trust(&fixture.registry, 5, PeerRole::Conductor, &[]);

    for index in 1..=(SIGNAL_OUTBOX_CAPACITY as u64 + 4) {
        fixture
            .registry
            .health_enqueue_signal(
                SignalEnqueueRequest {
                    target_node_id: &conductor,
                    signal_id: &opaque_id_hex(3_000 + index),
                    kind: SignalKind::Enrolled,
                    occurred_at: BASE_NOW + index as i64,
                    subject: Some(&conductor),
                    run: None,
                    message_bytes: 777,
                },
                BASE_NOW + index as i64,
            )
            .unwrap();
    }
    let outbox = fixture.registry.health_outbox(64).unwrap();
    assert_eq!(outbox.len(), SIGNAL_OUTBOX_CAPACITY as usize);
    assert_eq!(fixture.registry.health_signals_dropped().unwrap(), 4);
    assert_eq!(outbox.first().unwrap().sequence, 5);

    // Re-queuing the same signal_id is refused; idempotency is by signal_id.
    assert!(matches!(
        fixture.registry.health_enqueue_signal(
            SignalEnqueueRequest {
                target_node_id: &conductor,
                signal_id: &opaque_id_hex(3_000 + SIGNAL_OUTBOX_CAPACITY as u64),
                kind: SignalKind::Enrolled,
                occurred_at: BASE_NOW,
                subject: Some(&conductor),
                run: None,
                message_bytes: 777,
            },
            BASE_NOW,
        ),
        Err(RegistryError::Duplicate(_))
    ));

    let target = outbox.first().unwrap().signal_id.clone();
    assert!(
        fixture
            .registry
            .health_mark_signal_sent(&target, &opaque_id_hex(7_777), BASE_NOW + 100)
            .unwrap()
    );
    let local = fixture.registry.local_node_id().to_string();
    let ack = HealthPayload {
        message_id: opaque_id_hex(8_888),
        target: local,
        body: HealthBody::Ack(crate::domain::health_plane::model::AckBody {
            accepted: true,
            acked_message_id: opaque_id_hex(7_777),
            cursor: 5,
        }),
    };
    assert_eq!(
        apply(&fixture.registry, &conductor, &ack, BASE_NOW + 100),
        accepted(5)
    );
    let outbox = fixture.registry.health_outbox(64).unwrap();
    assert_eq!(outbox.len(), SIGNAL_OUTBOX_CAPACITY as usize - 1);
    assert!(outbox.iter().all(|entry| entry.signal_id != target));
}

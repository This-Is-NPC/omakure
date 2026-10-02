use super::*;

#[test]
fn a_terminal_run_becomes_exactly_one_bounded_redacted_signal() {
    let fixture = fixture();
    grant_notifications(&fixture);
    let mut session = fixture.conductor_session();
    settle(&fixture, &mut session);

    // Nothing has finished yet, so the feed is silent.
    fixture.clock.advance(1);
    assert!(session.tick().is_none());

    fixture
        .facts
        .finish_run(&"a".repeat(32), "deploy", BASE_NOW + 1);
    fixture.clock.advance(1);
    let encoded = session.tick().expect("run-completed signal");
    let (kind, payload) = decode(&fixture, &encoded);
    assert_eq!(kind, "health_signal");
    assert_eq!(payload["target"], fixture.conductor);
    assert_eq!(payload["signal"]["kind"], "run-completed");
    assert_eq!(payload["signal"]["sequence"], 1);
    assert!(payload["signal"]["subject"].is_null());
    assert_eq!(payload["signal"]["occurred_at"], BASE_NOW + 1);
    assert_eq!(payload["signal"]["run"]["finished_at"], BASE_NOW + 1);
    assert_eq!(payload["signal"]["run"]["script"], "deploy");
    assert_eq!(payload["signal"]["run"]["state"], "completed");
    assert_eq!(payload["signal"]["run"].as_object().unwrap().len(), 5);
    assert!(
        encoded.len() <= HealthKind::Signal.max_encoded_bytes(),
        "signal envelope exceeded the frozen per-kind cap"
    );

    // The same terminal run never produces a second outbox entry.
    assert_eq!(session.plane().outbox(64).expect("outbox").len(), 1);
    fixture.clock.advance(1);
    assert!(session.tick().is_none(), "one in-flight message at a time");
    assert_eq!(session.plane().outbox(64).expect("outbox").len(), 1);
}

#[test]
fn a_performer_without_the_notifications_capability_never_emits_a_signal() {
    let fixture = fixture();
    let mut session = fixture.conductor_session();
    settle(&fixture, &mut session);
    fixture
        .facts
        .finish_run(&"b".repeat(32), "deploy", BASE_NOW + 1);
    for _ in 0..5 {
        fixture.clock.advance(1);
        assert!(
            session.tick().is_none(),
            "a Signal must never leave a node whose Conductor granted no notifications"
        );
    }
    assert!(
        session.plane().outbox(64).expect("outbox").is_empty(),
        "nothing is even queued without the frozen capability"
    );
}

#[test]
fn an_unacknowledged_signal_is_retried_from_the_outbox_and_then_bounded() {
    let fixture = fixture();
    grant_notifications(&fixture);
    let mut session = fixture.conductor_session();
    settle(&fixture, &mut session);
    fixture
        .facts
        .finish_run(&"c".repeat(32), "deploy", BASE_NOW + 1);
    fixture.clock.advance(1);
    let (_, first) = decode(&fixture, &session.tick().expect("first attempt"));
    let signal_id = first["signal"]["signal_id"].as_str().unwrap().to_string();
    let mut message_ids = vec![first["message_id"].as_str().unwrap().to_string()];

    // Frozen backoff: 1 s, then 2 s, each after the 5 s acknowledgement
    // timeout. Three attempts in total, then the entry is retained rather
    // than resent forever.
    for backoff in [RETRY_BACKOFF_SECONDS[0], RETRY_BACKOFF_SECONDS[1]] {
        fixture.clock.advance(ACK_TIMEOUT_SECONDS + backoff);
        let (kind, retry) = decode(&fixture, &session.tick().expect("retry"));
        assert_eq!(kind, "health_signal");
        assert_eq!(
            retry["signal"]["signal_id"], signal_id,
            "a resend reuses the frozen idempotency key"
        );
        assert_eq!(retry["signal"]["sequence"], 1, "and the same sequence");
        let message_id = retry["message_id"].as_str().unwrap().to_string();
        assert!(
            !message_ids.contains(&message_id),
            "a resend must use a fresh message_id"
        );
        message_ids.push(message_id);
    }
    assert_eq!(message_ids.len(), MAX_RETRIES as usize);

    fixture
        .clock
        .advance(ACK_TIMEOUT_SECONDS + RETRY_BACKOFF_SECONDS[2]);
    assert!(
        session.tick().is_none(),
        "the frozen three-attempt bound must stop the resend loop"
    );
    let outbox = session.plane().outbox(64).expect("outbox");
    assert_eq!(outbox.len(), 1, "the Signal is retained, not dropped");
    assert_eq!(outbox[0].attempts, MAX_RETRIES);
    assert_eq!(
        outbox[0].expires_at - outbox[0].enqueued_at,
        crate::health_plane::bounds::SIGNAL_RETENTION_SECONDS
    );
}

#[test]
fn an_exhausted_signal_is_resent_on_the_next_session() {
    let fixture = fixture();
    grant_notifications(&fixture);
    let signal_id;
    let mut message_ids: Vec<String> = Vec::new();
    {
        let mut session = fixture.conductor_session();
        settle(&fixture, &mut session);
        fixture
            .facts
            .finish_run(&"f".repeat(32), "deploy", BASE_NOW + 1);
        fixture.clock.advance(1);
        let (_, first) = decode(&fixture, &session.tick().expect("first attempt"));
        signal_id = first["signal"]["signal_id"].as_str().unwrap().to_string();
        message_ids.push(first["message_id"].as_str().unwrap().to_string());

        // Spend the frozen three attempts for this session.
        for backoff in [RETRY_BACKOFF_SECONDS[0], RETRY_BACKOFF_SECONDS[1]] {
            fixture.clock.advance(ACK_TIMEOUT_SECONDS + backoff);
            let (_, retry) = decode(&fixture, &session.tick().expect("retry"));
            message_ids.push(retry["message_id"].as_str().unwrap().to_string());
        }
        fixture
            .clock
            .advance(ACK_TIMEOUT_SECONDS + RETRY_BACKOFF_SECONDS[2]);
        assert!(
            session.tick().is_none(),
            "the frozen bound is three attempts per session"
        );
        let outbox = session.plane().outbox(64).expect("outbox");
        assert_eq!(outbox.len(), 1, "the Signal is retained, never dropped");
        assert_eq!(outbox[0].attempts, MAX_RETRIES);
    }
    assert_eq!(message_ids.len(), MAX_RETRIES as usize);

    // A new session re-arms the delivery budget exactly once, which is the
    // frozen "resent on the next session" rule.
    let mut next = fixture.conductor_session();
    settle(&fixture, &mut next);
    fixture.clock.advance(1);
    let (kind, resent) = decode(&fixture, &next.tick().expect("resend on the next session"));
    assert_eq!(kind, "health_signal");
    assert_eq!(
        resent["signal"]["signal_id"], signal_id,
        "a resend reuses the frozen idempotency key"
    );
    assert_eq!(resent["signal"]["sequence"], 1, "and the same sequence");
    let resent_message_id = resent["message_id"].as_str().unwrap().to_string();
    assert!(
        !message_ids.contains(&resent_message_id),
        "a resend must use a fresh message_id"
    );

    // Re-armed, not widened: the new session gets three attempts, not four,
    // and the outbox still holds exactly one bounded entry.
    let outbox = next.plane().outbox(64).expect("outbox");
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].attempts, 1, "one attempt spent in this session");
    assert_eq!(
        outbox[0].expires_at - outbox[0].enqueued_at,
        crate::health_plane::bounds::SIGNAL_RETENTION_SECONDS,
        "re-arming never extends the frozen 7-day retention"
    );

    // The budget is re-armed on connect, never mid-session: this session
    // still stops after its own three attempts.
    let mut sent = 1;
    for backoff in [RETRY_BACKOFF_SECONDS[0], RETRY_BACKOFF_SECONDS[1]] {
        fixture.clock.advance(ACK_TIMEOUT_SECONDS + backoff);
        if next.tick().is_some() {
            sent += 1;
        }
    }
    fixture
        .clock
        .advance(ACK_TIMEOUT_SECONDS + RETRY_BACKOFF_SECONDS[2]);
    assert!(next.tick().is_none());
    assert_eq!(sent, MAX_RETRIES, "still three attempts inside one session");
    assert_eq!(
        next.plane().outbox(64).expect("outbox")[0].attempts,
        MAX_RETRIES
    );
}

#[test]
fn a_new_session_resends_the_same_logical_signal_after_a_reconnect() {
    let fixture = fixture();
    grant_notifications(&fixture);
    let signal_id;
    let first_message_id;
    {
        let mut session = fixture.conductor_session();
        settle(&fixture, &mut session);
        fixture
            .facts
            .finish_run(&"d".repeat(32), "deploy", BASE_NOW + 1);
        fixture.clock.advance(1);
        let (_, payload) = decode(&fixture, &session.tick().expect("first attempt"));
        signal_id = payload["signal"]["signal_id"].as_str().unwrap().to_string();
        first_message_id = payload["message_id"].as_str().unwrap().to_string();
    }

    // A brand new session over a brand new reporter: the durable outbox is
    // the only thing that carries the Signal across the reconnect.
    let mut session = fixture.conductor_session();
    settle(&fixture, &mut session);
    fixture.clock.advance(1);
    let (kind, resent) = decode(&fixture, &session.tick().expect("resend"));
    assert_eq!(kind, "health_signal");
    assert_eq!(resent["signal"]["signal_id"], signal_id);
    assert_eq!(resent["signal"]["sequence"], 1);
    assert_ne!(resent["message_id"], first_message_id.as_str());
    assert_eq!(
        session.plane().outbox(64).expect("outbox").len(),
        1,
        "a reconnect must not duplicate the queued Signal"
    );
}

#[test]
fn the_signal_send_rate_stays_inside_the_frozen_per_minute_bound() {
    let fixture = fixture();
    grant_notifications(&fixture);
    let mut session = fixture.conductor_session();
    settle(&fixture, &mut session);
    for index in 0..(MAX_SIGNALS_PER_PEER_PER_MINUTE as usize + 4) {
        fixture
            .facts
            .finish_run(&opaque_id_hex(index as u64), "deploy", BASE_NOW + 1);
    }

    let mut sent = 0;
    // One in-flight message at a time, so each accepted Signal is
    // acknowledged before the next is offered. The window is one minute.
    for _ in 0..120 {
        fixture.clock.advance(1);
        let Some(encoded) = session.tick() else {
            continue;
        };
        let (kind, payload) = decode(&fixture, &encoded);
        ack(&mut session, &payload);
        if kind == "health_signal" {
            sent += 1;
        }
        if fixture.clock.unix_seconds() >= BASE_NOW + RATE_MINUTE_WINDOW_SECONDS {
            break;
        }
    }
    assert!(
        sent <= MAX_SIGNALS_PER_PEER_PER_MINUTE,
        "sent {sent} Signals in one minute; the frozen bound is {MAX_SIGNALS_PER_PEER_PER_MINUTE}"
    );
    assert!(sent > 0, "the feed must actually drain");
}

#[test]
fn revoking_the_conductor_stops_the_signal_feed_on_the_next_tick() {
    let fixture = fixture();
    grant_notifications(&fixture);
    let mut session = fixture.conductor_session();
    settle(&fixture, &mut session);
    fixture
        .facts
        .finish_run(&"e".repeat(32), "deploy", BASE_NOW + 1);
    fixture.clock.advance(1);
    assert!(session.tick().is_some(), "the Signal leaves while trusted");

    fixture
        .registry
        .revoke_peer(
            &fixture.conductor,
            "direct-health-tests",
            "revoked during the certification",
        )
        .expect("revoke conductor");
    for _ in 0..5 {
        fixture.clock.advance(ACK_TIMEOUT_SECONDS + 8);
        assert!(
            session.tick().is_none(),
            "a revoked Conductor must never receive another Signal"
        );
    }
}

#[test]
fn a_non_health_envelope_is_left_to_the_existing_steady_state_behavior() {
    let fixture = fixture();
    let mut session = fixture.conductor_session();
    let probe = crate::direct_transport::sign_probe(
        &fixture.identity,
        &SESSION_ID,
        [0x11; 16],
        BASE_NOW as u64,
    )
    .expect("sign probe");
    assert_eq!(
        session.handle_envelope(&probe.encoded()),
        HealthOutcome::NotHealth
    );
}

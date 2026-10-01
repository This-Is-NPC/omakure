use super::super::bounds::{
    MAX_AGE_SECONDS, MAX_CANONICAL_PROFILE, MAX_FUTURE_SKEW_SECONDS, PRESENCE_ONLINE_SECONDS,
    PRESENCE_STALE_SECONDS,
};
use super::*;

#[test]
fn the_frozen_registry_schema_version_matches_the_shipped_one() {
    assert_eq!(
        bounds::REGISTRY_SCHEMA_VERSION,
        crate::node_registry::SCHEMA_VERSION
    );
}

#[test]
fn a_syntactically_impossible_sender_is_dropped_without_a_reply() {
    let fixture = fixture();
    let outcome = fixture.ingest(
        "not-a-node-id",
        "health_profile",
        BASE_NOW,
        &profile_payload(&fixture.local, 1, 1),
    );
    assert_eq!(outcome.code(), Some(HealthCode::InvalidMessage));
    assert_eq!(outcome.reply, HealthReply::None);
    assert!(fixture.plane().audit_events(10).unwrap().is_empty());
    assert!(fixture.plane().fleet_status().unwrap().is_empty());
}

#[test]
fn a_complete_reporting_sequence_is_accepted_and_acknowledged() {
    let fixture = fixture();
    let plane = fixture.plane();

    let profile = fixture.ingest(
        &fixture.performer,
        "health_profile",
        BASE_NOW,
        &profile_payload(&fixture.local, 1, 1),
    );
    assert!(profile.accepted());
    assert_eq!(
        profile.reply,
        HealthReply::Ack {
            acked_message_id: opaque_id_hex(1),
            cursor: 0
        }
    );

    fixture.clock.set(BASE_NOW + 30);
    let pulse = fixture.ingest(
        &fixture.performer,
        "health_pulse",
        BASE_NOW + 30,
        &pulse_payload(&fixture.local, 2, 1, BASE_NOW + 30),
    );
    assert!(pulse.accepted());

    fixture.clock.set(BASE_NOW + 60);
    let signal = fixture.ingest(
        &fixture.performer,
        "health_signal",
        BASE_NOW + 60,
        &signal_payload(&fixture.local, 3, 1, 101, BASE_NOW + 60),
    );
    assert_eq!(
        signal.reply,
        HealthReply::Ack {
            acked_message_id: opaque_id_hex(3),
            cursor: 1
        }
    );

    let fleet = plane.fleet_status().unwrap();
    assert_eq!(fleet.len(), 1);
    let node = &fleet[0];
    assert_eq!(node.node_id, fixture.performer);
    assert_eq!(node.role, "performer");
    assert_eq!(node.trust_state, "active");
    assert_eq!(node.presence, Presence::Online);
    assert_eq!(node.signal_cursor, 1);
    assert_eq!(node.stored_signals, 1);
    assert_eq!(node.profile.as_ref().unwrap().profile_revision, 1);
    assert_eq!(node.pulse.as_ref().unwrap().sequence, 1);
    assert!(!node.version_incompatible);
    assert_eq!(plane.signals(&fixture.performer, 64).unwrap().len(), 1);
}

#[test]
fn presence_boundaries_are_exact_and_derived_only_from_the_last_pulse() {
    let fixture = fixture();
    assert_eq!(Presence::derive(None, BASE_NOW), Presence::Unknown);
    fixture.ingest(
        &fixture.performer,
        "health_pulse",
        BASE_NOW,
        &pulse_payload(&fixture.local, 1, 1, BASE_NOW),
    );
    for (offset, expected) in [
        (0, Presence::Online),
        (PRESENCE_ONLINE_SECONDS, Presence::Online),
        (PRESENCE_ONLINE_SECONDS + 1, Presence::Stale),
        (PRESENCE_STALE_SECONDS, Presence::Stale),
        (PRESENCE_STALE_SECONDS + 1, Presence::Offline),
    ] {
        fixture.clock.set(BASE_NOW + offset);
        let node = fixture
            .plane()
            .node_status(&fixture.performer)
            .unwrap()
            .unwrap();
        assert_eq!(node.presence, expected, "at offset {offset}");
    }
}

/// The two baseline fields are a claim and its evidence, and the closed
/// schema is what keeps both readable.
///
/// The drift verdict is a comparison of these two strings, so a receiver
/// that accepted a half-width identity would compare a truncated name
/// against a whole one and read that as drift; and one that accepted
/// evidence without a claim would store a verdict no set on disk could
/// justify.
#[test]
fn a_profile_carrying_an_unreadable_baseline_pair_is_refused() {
    let fixture = fixture();
    let target = fixture.local.clone();
    let identity = "a".repeat(64);

    let mut accepted = profile_payload(&target, 1, 1);
    accepted["profile"]["baseline_id"] = json!(identity);
    accepted["profile"]["baseline_observed_id"] = json!(identity);
    assert!(
        fixture
            .ingest(&fixture.performer, "health_profile", BASE_NOW, &accepted)
            .accepted(),
        "a Performer reporting the set it holds must be accepted"
    );

    for (recorded, observed, why) in [
        (
            identity[..63].to_string(),
            identity.clone(),
            "an identity one character short is not a shorter identity",
        ),
        (
            identity.clone(),
            identity.to_uppercase(),
            "uppercase hex names the same bytes and must still be refused, \
                 because two spellings of one set would read as drift",
        ),
        (
            String::new(),
            identity.clone(),
            "evidence without a claim is a verdict no record justifies",
        ),
    ] {
        let mut payload = profile_payload(&target, 2, 2);
        payload["profile"]["baseline_id"] = json!(recorded);
        payload["profile"]["baseline_observed_id"] = json!(observed);
        assert_eq!(
            fixture
                .ingest(&fixture.performer, "health_profile", BASE_NOW, &payload)
                .code(),
            Some(HealthCode::InvalidMessage),
            "{why}"
        );
    }
}

#[test]
fn every_contracted_rejection_produces_its_stable_code() {
    let fixture = fixture();
    let target = fixture.local.clone();

    // An envelope kind outside the closed set.
    assert_eq!(
        fixture.code(
            &fixture.performer,
            "health_inventory",
            BASE_NOW,
            &profile_payload(&target, 1, 1)
        ),
        HealthCode::UnknownField
    );
    // Oversize, checked before parsing.
    assert_eq!(
        fixture
            .plane()
            .ingest(InboundHealthMessage {
                sender: &fixture.performer,
                kind: "health_profile",
                created_at: BASE_NOW,
                canonical_len: MAX_CANONICAL_PROFILE + 1,
                payload: &profile_payload(&target, 1, 1),
            })
            .unwrap()
            .code()
            .unwrap(),
        HealthCode::MessageTooLarge
    );
    // Missing and unknown fields.
    let mut payload = profile_payload(&target, 1, 1);
    payload.as_object_mut().unwrap().remove("health_version");
    assert_eq!(
        fixture.code(&fixture.performer, "health_profile", BASE_NOW, &payload),
        HealthCode::UnknownField
    );
    let mut payload = profile_payload(&target, 1, 1);
    payload.as_object_mut().unwrap()["profile"]
        .as_object_mut()
        .unwrap()
        .insert("hostname".to_string(), json!("workshop.local"));
    assert_eq!(
        fixture.code(&fixture.performer, "health_profile", BASE_NOW, &payload),
        HealthCode::UnknownField
    );
    // An unsupported schema version.
    let mut payload = profile_payload(&target, 1, 1);
    payload.as_object_mut().unwrap()["health_version"] = json!(2);
    assert_eq!(
        fixture.code(&fixture.performer, "health_profile", BASE_NOW, &payload),
        HealthCode::UnsupportedVersion
    );
    // Grammar, secret references, and floating point numbers.
    let mut payload = profile_payload(&target, 1, 1);
    payload.as_object_mut().unwrap()["profile"]
        .as_object_mut()
        .unwrap()["display_name"] = json!("/etc/shadow");
    assert_eq!(
        fixture.code(&fixture.performer, "health_profile", BASE_NOW, &payload),
        HealthCode::InvalidMessage
    );
    let mut payload = profile_payload(&target, 1, 1);
    payload.as_object_mut().unwrap()["profile"]
        .as_object_mut()
        .unwrap()["distro_version"] = json!("secret://vault/token");
    assert_eq!(
        fixture.code(&fixture.performer, "health_profile", BASE_NOW, &payload),
        HealthCode::InvalidMessage
    );
    let mut payload = pulse_payload(&target, 1, 1, BASE_NOW);
    payload.as_object_mut().unwrap()["pulse"]
        .as_object_mut()
        .unwrap()["uptime_seconds"] = json!(1.5);
    assert_eq!(
        fixture.code(&fixture.performer, "health_pulse", BASE_NOW, &payload),
        HealthCode::InvalidMessage
    );
    // A third party's node ID as the target.
    let (other, _, _) = peer_identity(40);
    assert_eq!(
        fixture.code(
            &fixture.performer,
            "health_profile",
            BASE_NOW,
            &profile_payload(&other, 1, 1)
        ),
        HealthCode::WrongTarget
    );
    // Role and capability.
    assert_eq!(
        fixture.code(
            &fixture.conductor,
            "health_profile",
            BASE_NOW,
            &profile_payload(&target, 1, 1)
        ),
        HealthCode::WrongRole
    );
    assert_eq!(
        fixture.code(
            &fixture.limited,
            "health_profile",
            BASE_NOW,
            &profile_payload(&target, 2, 1)
        ),
        HealthCode::MissingCapability
    );
    // An identity the registry has never seen.
    let (stranger, _, _) = peer_identity(41);
    assert_eq!(
        fixture.code(
            &stranger,
            "health_profile",
            BASE_NOW,
            &profile_payload(&target, 3, 1)
        ),
        HealthCode::Revoked
    );
    // Freshness, inclusive at exactly the frozen boundaries.
    assert_eq!(
        fixture.code(
            &fixture.performer,
            "health_profile",
            BASE_NOW - MAX_AGE_SECONDS - 1,
            &profile_payload(&target, 4, 1)
        ),
        HealthCode::Stale
    );
    assert_eq!(
        fixture.code(
            &fixture.performer,
            "health_profile",
            BASE_NOW + MAX_FUTURE_SKEW_SECONDS + 1,
            &profile_payload(&target, 5, 1)
        ),
        HealthCode::Future
    );
    // Replay and reordering.
    assert!(
        fixture
            .ingest(
                &fixture.performer,
                "health_profile",
                BASE_NOW,
                &profile_payload(&target, 6, 1)
            )
            .accepted()
    );
    assert_eq!(
        fixture.code(
            &fixture.performer,
            "health_profile",
            BASE_NOW,
            &profile_payload(&target, 6, 2)
        ),
        HealthCode::Replay
    );
    assert_eq!(
        fixture.code(
            &fixture.performer,
            "health_signal",
            BASE_NOW,
            &signal_payload(&target, 7, 99, 199, BASE_NOW)
        ),
        HealthCode::Reordered
    );

    // Nothing above created or reactivated a trust row.
    let authorization = fixture.plane().authorization(&stranger).unwrap();
    assert!(authorization.is_none());
}

#[test]
fn a_health_error_is_only_sent_to_an_authorized_target_bound_peer() {
    let fixture = fixture();
    let target = fixture.local.clone();

    // Trust, role, and capability failures are dropped and audited only.
    for (sender, payload) in [
        (&fixture.conductor, profile_payload(&target, 1, 1)),
        (&fixture.limited, profile_payload(&target, 2, 1)),
    ] {
        let outcome = fixture.ingest(sender, "health_profile", BASE_NOW, &payload);
        assert_eq!(outcome.reply, HealthReply::None);
    }
    // A failure after authorization carries the stable code back.
    let outcome = fixture.ingest(
        &fixture.performer,
        "health_profile",
        BASE_NOW - MAX_AGE_SECONDS - 1,
        &profile_payload(&target, 3, 1),
    );
    assert_eq!(
        outcome.reply,
        HealthReply::Error {
            acked_message_id: opaque_id_hex(3),
            code: HealthCode::Stale
        }
    );
    // The rejections are audited with their stable codes and nothing else.
    let audit = fixture.plane().audit_events(10).unwrap();
    assert!(audit.iter().all(|event| event.outcome != "accepted"));
    assert!(
        audit
            .iter()
            .any(|event| event.error_code == Some(HealthCode::Stale.code()))
    );
}

#[test]
fn an_unsupported_version_replies_only_when_the_peer_is_addressed_and_authorized() {
    let fixture = fixture();
    let target = fixture.local.clone();
    let mut payload = profile_payload(&target, 1, 1);
    payload.as_object_mut().unwrap()["health_version"] = json!(2);

    // The peer must first be tracked for the projection to record anything.
    assert!(
        fixture
            .ingest(
                &fixture.performer,
                "health_profile",
                BASE_NOW,
                &profile_payload(&target, 9, 1)
            )
            .accepted()
    );

    let outcome = fixture.ingest(&fixture.performer, "health_profile", BASE_NOW, &payload);
    assert_eq!(outcome.code(), Some(HealthCode::UnsupportedVersion));
    assert_eq!(
        outcome.reply,
        HealthReply::Error {
            acked_message_id: opaque_id_hex(1),
            code: HealthCode::UnsupportedVersion
        }
    );
    let node = fixture
        .plane()
        .node_status(&fixture.performer)
        .unwrap()
        .unwrap();
    assert!(node.version_incompatible);

    // A message addressed elsewhere gets no reply at all.
    let (other, _, _) = peer_identity(42);
    let mut payload = profile_payload(&other, 2, 1);
    payload.as_object_mut().unwrap()["health_version"] = json!(3);
    let outcome = fixture.ingest(&fixture.performer, "health_profile", BASE_NOW, &payload);
    assert_eq!(outcome.code(), Some(HealthCode::UnsupportedVersion));
    assert_eq!(outcome.reply, HealthReply::None);

    // Trust and transport state are untouched by a version mismatch.
    let authorization = fixture
        .plane()
        .authorization(&fixture.performer)
        .unwrap()
        .unwrap();
    assert_eq!(authorization.state, PeerState::Active);
    assert_eq!(authorization.role, PeerRole::Performer);
}

#[test]
fn a_held_signal_acknowledges_the_unchanged_cursor() {
    let fixture = fixture();
    let target = fixture.local.clone();
    assert!(
        fixture
            .ingest(
                &fixture.performer,
                "health_signal",
                BASE_NOW,
                &signal_payload(&target, 1, 1, 101, BASE_NOW)
            )
            .accepted()
    );
    fixture.clock.set(BASE_NOW + 20);
    let held = fixture.ingest(
        &fixture.performer,
        "health_signal",
        BASE_NOW + 20,
        &signal_payload(&target, 2, 3, 103, BASE_NOW + 20),
    );
    assert_eq!(held.decision, HealthDecision::Held { cursor: 1 });
    assert_eq!(
        held.reply,
        HealthReply::Ack {
            acked_message_id: opaque_id_hex(2),
            cursor: 1
        }
    );
    // The stalled cursor never exposes a hole to a reader.
    assert_eq!(
        fixture
            .plane()
            .signals(&fixture.performer, 64)
            .unwrap()
            .iter()
            .map(|signal| signal.sequence)
            .collect::<Vec<_>>(),
        vec![1]
    );
}

use super::*;

/// One baseline written on a session, with the budget the caller asked for
/// and the channel it is waiting on.
fn outbound_baseline_waiting(
    baseline_id: &str,
    budget: Duration,
) -> (
    OutboundBaseline,
    std::sync::mpsc::Receiver<BaselinePushOutcome>,
) {
    let (reply, answers) = std::sync::mpsc::sync_channel(1);
    let pending = PendingBaseline {
        manifest: Vec::new(),
        bodies: Vec::new(),
        baseline_id: baseline_id.to_string(),
        deadline: Instant::now() + budget,
        reply,
    };
    (OutboundBaseline::new(pending), answers)
}

/// The `baseline_ack` a Performer sends back, signed by its own identity
/// against the session the handshake established.
fn signed_baseline_ack(
    peer: &crate::node_identity::NodeIdentity,
    session_id: &[u8; 32],
    baseline_id: &str,
    accepted: bool,
) -> Vec<u8> {
    let mut payload = serde_json::json!({
        "version": 1,
        "baseline_id": baseline_id,
        "accepted": accepted,
    });
    if !accepted {
        payload["error"] = serde_json::json!({
            "code": crate::baseline_push::BaselineCode::InstallFailed.code(),
        });
    }
    crate::direct_transport::sign_baseline_envelope(
        peer,
        crate::baseline_push::KIND_ACK,
        session_id,
        [9u8; 16],
        payload,
        unix_seconds(),
    )
    .expect("sign the baseline ack")
    .encoded()
}

/// The ordinary case, so the late-ack tests below cannot pass by breaking
/// the timely path they are measured against.
#[test]
fn a_baseline_ack_inside_the_budget_answers_the_caller() {
    let temp = tempfile::tempdir().expect("workspace");
    let (peer, peer_node_id, peer_key) = test_peer_identity(&temp);
    let session_id = [3u8; 32];
    let (mut in_flight, answers) = outbound_baseline_waiting("a1b2c3", Duration::from_secs(60));

    let ack = signed_baseline_ack(&peer, &session_id, "a1b2c3", true);
    assert_eq!(
        in_flight.absorb_ack(&ack, &peer_node_id, &peer_key, &session_id),
        BaselineAckMatch::Answered
    );
    let outcome = answers.try_recv().expect("the caller must be answered");
    assert!(outcome.answered && outcome.accepted, "outcome: {outcome:?}");
    assert_eq!(outcome.code, 0, "outcome: {outcome:?}");
}

/// A `baseline_ack` that misses the caller's budget is still this node's
/// own answer to its own push.
///
/// The budget bounds how long the *caller* waits. Nothing about it says the
/// Performer will stay quiet, and on a slow link the ack has arrived a
/// minute or two later with the baseline installed. A session that had
/// forgotten the id hands that ack to the receive half, which judges
/// `baseline_push` messages and can only call a `baseline_ack` malformed --
/// so the Conductor's own audit table records `baseline_rejected` /
/// `invalid_message` for a baseline the Performer accepted.
#[test]
fn a_baseline_ack_after_the_budget_is_recognized_rather_than_taken_for_a_stranger() {
    let temp = tempfile::tempdir().expect("workspace");
    let (peer, peer_node_id, peer_key) = test_peer_identity(&temp);
    let session_id = [5u8; 32];
    let (mut in_flight, answers) = outbound_baseline_waiting("d4e5f6", Duration::ZERO);

    in_flight.expire_if_due();
    let given_up = answers
        .try_recv()
        .expect("an expired budget must stop the caller waiting");
    assert!(
        !given_up.answered,
        "the caller was told the push was answered: {given_up:?}"
    );

    let ack = signed_baseline_ack(&peer, &session_id, "d4e5f6", true);
    assert_eq!(
        in_flight.absorb_ack(&ack, &peer_node_id, &peer_key, &session_id),
        BaselineAckMatch::Late {
            accepted: true,
            code: 0
        },
        "the ack for this node's own baseline was not recognized after the budget"
    );
    assert!(
        answers.try_recv().is_err(),
        "the caller was answered twice: the first answer is the only one it read"
    );
}

/// A late refusal carries its code, so the audited outcome is the
/// Performer's, not a guess.
#[test]
fn a_late_baseline_refusal_keeps_the_code_the_performer_sent() {
    let temp = tempfile::tempdir().expect("workspace");
    let (peer, peer_node_id, peer_key) = test_peer_identity(&temp);
    let session_id = [6u8; 32];
    let (mut in_flight, _answers) = outbound_baseline_waiting("0a0b0c", Duration::ZERO);
    in_flight.expire_if_due();

    let ack = signed_baseline_ack(&peer, &session_id, "0a0b0c", false);
    assert_eq!(
        in_flight.absorb_ack(&ack, &peer_node_id, &peer_key, &session_id),
        BaselineAckMatch::Late {
            accepted: false,
            code: crate::baseline_push::BaselineCode::InstallFailed.code()
        }
    );
}

/// An ack for someone else's baseline is not an answer to this one, and
/// keeping the id around to spot a late ack must not turn into matching
/// everything that arrives.
#[test]
fn an_ack_for_a_different_baseline_is_never_taken_as_this_ones_answer() {
    let temp = tempfile::tempdir().expect("workspace");
    let (peer, peer_node_id, peer_key) = test_peer_identity(&temp);
    let session_id = [7u8; 32];
    let (mut in_flight, _answers) = outbound_baseline_waiting("111111", Duration::ZERO);
    in_flight.expire_if_due();

    let ack = signed_baseline_ack(&peer, &session_id, "222222", true);
    assert_eq!(
        in_flight.absorb_ack(&ack, &peer_node_id, &peer_key, &session_id),
        BaselineAckMatch::Other
    );
}

#[test]
fn baseline_ack_rejects_wrong_kind_identity_and_session() {
    let temp = tempfile::tempdir().expect("workspace");
    let (peer, peer_node_id, peer_key) = test_peer_identity(&temp);
    let session_id = [23u8; 32];
    let (mut in_flight, answers) = outbound_baseline_waiting("111111", Duration::from_secs(60));
    let ack = signed_baseline_ack(&peer, &session_id, "111111", true);
    let wrong_kind = crate::direct_transport::sign_baseline_envelope(
        &peer,
        crate::baseline_push::KIND_PUSH,
        &session_id,
        [11u8; 16],
        serde_json::json!({"baseline_id": "111111", "accepted": true}),
        unix_seconds(),
    )
    .expect("sign wrong kind")
    .encoded();
    assert_unmatched_acks(
        AckFixtures {
            valid: &ack,
            wrong_kind: &wrong_kind,
            peer_node_id: &peer_node_id,
            peer_key: &peer_key,
            session_id: &session_id,
        },
        BaselineAckMatch::Other,
        &answers,
        |body, node_id, key, session| in_flight.absorb_ack(body, node_id, key, session),
    );
}

/// The "one baseline in flight per session" bound is about unanswered bytes
/// on the wire, not about remembering an id: a slot kept only so a late ack
/// can be recognized must not hold the next push behind it for the rest of
/// the session.
#[test]
fn a_slot_kept_only_for_correlation_does_not_bar_the_next_push() {
    let (mut waiting, _waiting_answers) =
        outbound_baseline_waiting("333333", Duration::from_secs(60));
    waiting.expire_if_due();
    assert!(
        !waiting.is_answered(),
        "a baseline still inside its budget must hold the queue"
    );

    let (mut spent, _spent_answers) = outbound_baseline_waiting("444444", Duration::ZERO);
    spent.expire_if_due();
    assert!(
        spent.is_answered(),
        "a slot whose caller has been answered must let the next baseline go out"
    );
}

/// And the same hole on the path that supplies the code.
///
/// `push_baseline` had the identical shape: it checked the manifest and the
/// body count, then enqueued onto whatever session existed. Shipping a
/// signed script set to a machine the fleet has just disowned is the worse
/// of the two, because the Performer installs it and keeps it.
#[test]
fn a_baseline_is_refused_for_a_peer_this_node_revoked_even_with_a_live_session() {
    let temp = tempfile::TempDir::new().expect("temporary node root");
    let (state, peer_node_id, _sockets) = revoked_peer_with_a_standing_session(&temp);
    let dispatcher = BaselineDispatcher {
        state: Arc::clone(&state),
    };
    assert!(
        dispatcher.has_session(&peer_node_id),
        "the dispatcher must still see a session, or the refusal proves nothing"
    );

    // Deliberately not a valid manifest: the gate has to refuse before the
    // manifest is even looked at, so a caller cannot learn whether its
    // bytes parsed by asking about a peer it is no longer allowed to reach.
    let error = dispatcher
        .push_baseline(
            &peer_node_id,
            b"not-a-manifest",
            &[],
            Duration::from_millis(50),
        )
        .expect_err("a baseline push to a revoked peer must be refused");
    match &error {
        DirectServiceError::PeerNotActive {
            peer_node_id: named,
            state,
            protocol,
        } => {
            assert_eq!(named, &peer_node_id);
            assert_eq!(*state, "revoked");
            assert_eq!(*protocol, TransportError::Revoked);
        }
        other => panic!("expected a refusal naming the revoked peer, got {other}"),
    }
    assert!(
        state.baseline_outbox.lock().unwrap().is_empty(),
        "a refused baseline must never reach the session thread's outbox"
    );
}

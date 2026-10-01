use super::*;

#[test]
fn resolve_cue_id_honors_a_supplied_idempotency_key() {
    let known = "0123456789abcdef0123456789abcdef";
    assert_eq!(resolve_cue_id(Some(known)).expect("well-formed id"), known);
    assert!(resolve_cue_id(Some("not-hex")).is_err());
    let minted = resolve_cue_id(None).expect("mint");
    assert_eq!(minted.len(), 32);
    assert!(crate::remote_cue::is_well_formed_cue_id(&minted));
}

/// One Cue written on a session, with the channel its answer goes back on.
fn outbound_cue_waiting(
    cue_id: &str,
) -> (OutboundCue, std::sync::mpsc::Receiver<CueDispatchOutcome>) {
    let (reply, answers) = std::sync::mpsc::sync_channel(1);
    let pending = PendingCue {
        cue_id: cue_id.to_string(),
        script: "declared.sh".to_string(),
        reason: "unit test".to_string(),
        expected_run_id: "run".to_string(),
        deadline: Instant::now() + Duration::from_secs(60),
        reply,
    };
    (OutboundCue::new(pending), answers)
}

/// The `cue_ack` a Performer sends back, signed by its own identity against
/// the session the handshake established.
fn signed_cue_ack(
    peer: &crate::node_identity::NodeIdentity,
    session_id: &[u8; 32],
    cue_id: &str,
    accepted: bool,
) -> Vec<u8> {
    let mut payload = serde_json::json!({
        "version": 1,
        "cue_id": cue_id,
        "accepted": accepted,
    });
    if !accepted {
        payload["error"] = serde_json::json!({
            "code": crate::remote_cue::CueCode::ScriptUnresolvable.code(),
        });
    }
    crate::direct_transport::sign_cue_envelope(
        peer,
        crate::remote_cue::KIND_ACK,
        session_id,
        [11u8; 16],
        payload,
        unix_seconds(),
    )
    .expect("sign the cue ack")
    .encoded()
}

#[test]
fn a_local_cue_enqueue_failure_is_visible_in_transport_status() {
    let temp = tempfile::tempdir().expect("node root");
    let context = crate::test_support::node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).expect("initialize identity");
    let state = ConnectionState::new(
        context,
        &identity,
        &[],
        Arc::new(AtomicBool::new(false)),
        true,
        Arc::new(AdmissionController {
            state: Mutex::new(AdmissionState::default()),
        }),
        None,
        None,
    );
    let error = DirectServiceError::CueEnqueueFailed {
        error: crate::remote_cue::CueEnqueueError::Failed(
            crate::operations::OperationErrorCode::IoFailed,
        ),
    };

    state.record_direct_error("peer", &error);

    assert_eq!(
        state.status.lock().unwrap().last_errors.get("peer"),
        Some(&"cue_enqueue_failed".to_string())
    );
    assert_eq!(
        error.to_string(),
        "direct transport Cue enqueue failed locally: io_failed"
    );
}

/// An accepted Cue's ack belongs to the Cue that sent it.
///
/// It does not *finish* the exchange -- the outcome still arrives later as
/// an ordinary Signal -- but it is this slot's envelope, and reporting
/// otherwise handed it to the receive half, which judges `cue_dispatch`
/// messages and can only read a `cue_ack` as a malformed one. The result
/// was `cue_rejected` / `invalid_message` written into the *Conductor's*
/// own audit table for every Cue its Performer accepted and ran. Measured
/// on two real nodes: one accepted dispatch, one new 1211 row.
#[test]
fn an_accepted_cue_ack_is_not_mistaken_for_a_stranger() {
    let temp = tempfile::tempdir().expect("workspace");
    let (peer, peer_node_id, peer_key) = test_peer_identity(&temp);
    let session_id = [13u8; 32];
    let (mut in_flight, answers) = outbound_cue_waiting("cue-accepted");

    let ack = signed_cue_ack(&peer, &session_id, "cue-accepted", true);
    assert_eq!(
        in_flight.absorb_ack(&ack, &peer_node_id, &peer_key, &session_id),
        CueAckMatch::Accepted,
        "the ack for this node's own Cue was reported as somebody else's"
    );
    assert!(
        answers.try_recv().is_err(),
        "an acceptance must not answer the caller: the outcome has not been read back yet"
    );
}

/// A refusal is the end of the exchange, and answers the caller.
#[test]
fn a_refused_cue_ack_answers_the_caller_and_ends_the_exchange() {
    let temp = tempfile::tempdir().expect("workspace");
    let (peer, peer_node_id, peer_key) = test_peer_identity(&temp);
    let session_id = [17u8; 32];
    let (mut in_flight, answers) = outbound_cue_waiting("cue-refused");

    let ack = signed_cue_ack(&peer, &session_id, "cue-refused", false);
    assert_eq!(
        in_flight.absorb_ack(&ack, &peer_node_id, &peer_key, &session_id),
        CueAckMatch::Refused
    );
    let outcome = answers.try_recv().expect("the caller must be answered");
    assert!(
        outcome.answered && !outcome.accepted,
        "outcome: {outcome:?}"
    );
    assert_eq!(
        outcome.code,
        crate::remote_cue::CueCode::ScriptUnresolvable.code(),
        "outcome: {outcome:?}"
    );
}

/// An ack for someone else's Cue is not an answer to this one, and
/// classifying acceptances must not turn into matching everything.
#[test]
fn an_ack_for_a_different_cue_is_never_taken_as_this_ones_answer() {
    let temp = tempfile::tempdir().expect("workspace");
    let (peer, peer_node_id, peer_key) = test_peer_identity(&temp);
    let session_id = [19u8; 32];
    let (mut in_flight, _answers) = outbound_cue_waiting("cue-mine");

    let ack = signed_cue_ack(&peer, &session_id, "cue-theirs", true);
    assert_eq!(
        in_flight.absorb_ack(&ack, &peer_node_id, &peer_key, &session_id),
        CueAckMatch::Other
    );
}

#[test]
fn cue_ack_rejects_wrong_kind_identity_and_session() {
    let temp = tempfile::tempdir().expect("workspace");
    let (peer, peer_node_id, peer_key) = test_peer_identity(&temp);
    let session_id = [23u8; 32];
    let (mut in_flight, answers) = outbound_cue_waiting("cue-mine");
    let ack = signed_cue_ack(&peer, &session_id, "cue-mine", true);
    let wrong_kind = crate::direct_transport::sign_cue_envelope(
        &peer,
        crate::remote_cue::KIND_DISPATCH,
        &session_id,
        [11u8; 16],
        serde_json::json!({"cue_id": "cue-mine", "accepted": true}),
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
        CueAckMatch::Other,
        &answers,
        |body, node_id, key, session| in_flight.absorb_ack(body, node_id, key, session),
    );
}

/// A Cue must not reach a peer this node has revoked.
///
/// This is the live two-VM failure: `node revoke` on the Conductor, and the
/// Conductor then dispatched a Cue that the revoked Performer accepted and
/// ran (`accepted:true, code:0`). Every receiving gate is fail-closed
/// against the *receiver's* registry, and the revoked node is never told it
/// was revoked, so it went on seeing an active Conductor and was right to.
/// The sender is the only place this can be enforced.
///
/// Delete the `require_active_peer` call in `dispatch` and this returns
/// `Ok` with `answered: false` after the budget instead of refusing: the
/// instruction was queued for the session and only the fake peer's silence
/// stopped it.
#[test]
fn a_cue_is_refused_for_a_peer_this_node_revoked_even_with_a_live_session() {
    let temp = tempfile::TempDir::new().expect("temporary node root");
    let (state, peer_node_id, _sockets) = revoked_peer_with_a_standing_session(&temp);
    let dispatcher = CueDispatcher {
        state: Arc::clone(&state),
    };
    assert!(
        dispatcher.has_session(&peer_node_id),
        "the dispatcher must still see a session, or the refusal proves nothing"
    );

    let error = dispatcher
        .dispatch(
            &peer_node_id,
            "cue-ok.sh",
            "why",
            Duration::from_millis(50),
            None,
        )
        .expect_err("a Cue to a revoked peer must be refused");
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
        state.outbox.lock().unwrap().is_empty(),
        "a refused Cue must never reach the session thread's outbox"
    );
}

/// A peer this node never trusted is refused too, and said to be absent.
///
/// The distinction is the operator's: `not_enrolled` says "you have not set
/// this up", `revoked` says "you took it away". Collapsing them would make
/// a typo in a node id look like a revocation.
#[test]
fn a_cue_to_an_unknown_peer_is_refused_as_not_enrolled() {
    let temp = tempfile::TempDir::new().expect("temporary node root");
    let (state, _peer_node_id, _sockets) = revoked_peer_with_a_standing_session(&temp);
    let stranger_root = temp.path().join("stranger");
    std::fs::create_dir_all(&stranger_root).expect("stranger root");
    let stranger =
        NodeIdentity::load_or_initialize(&crate::test_support::node_context(&stranger_root))
            .expect("initialize a stranger identity")
            .public_status()
            .node_id
            .clone();
    let dispatcher = CueDispatcher { state };
    let error = dispatcher
        .dispatch(
            &stranger,
            "cue-ok.sh",
            "why",
            Duration::from_millis(50),
            None,
        )
        .expect_err("a Cue to an unknown peer must be refused");
    match &error {
        DirectServiceError::PeerNotActive {
            peer_node_id: named,
            state,
            protocol,
        } => {
            assert_eq!(named, &stranger);
            assert_eq!(*state, "absent");
            assert_eq!(*protocol, TransportError::NotEnrolled);
        }
        other => panic!("expected a not-enrolled refusal, got {other}"),
    }
}

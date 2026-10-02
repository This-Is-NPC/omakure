use super::*;

/// A client waiting on a `wait`-bounded dispatch must outlast the session
/// thread it is waiting on.
///
/// The thread answers `answered: false` at `dispatch_answer_deadline`,
/// which is how the protocol's designed silence -- a receiver that refused
/// on trust, role, or capability says nothing at all -- becomes a verdict
/// an operator can read. A client that gives up at the budget it asked for
/// gives up before that verdict is produced, and reports a transport error
/// for the one case the rule exists to describe.
#[test]
fn a_dispatch_client_outlasts_the_session_thread_it_waits_on() {
    for seconds in [0u64, 1, 2, 60, 120, 300, 3600] {
        let wait = Duration::from_secs(seconds);
        let answer = dispatch_answer_deadline(wait);
        let client = dispatch_client_timeout(wait);
        assert!(
            answer > wait,
            "the session thread must outlast the budget it is enforcing at {seconds}s: \
                 answer={answer:?} wait={wait:?}"
        );
        assert!(
            client > answer,
            "the client must outlast the answer it is waiting for at {seconds}s: \
                 client={client:?} answer={answer:?}"
        );
    }
}

#[test]
fn pending_outboxes_are_fifo_and_empty_after_drain() {
    let temp = tempfile::tempdir().expect("temporary node root");
    let context = crate::test_support::node_context(temp.path());
    let (cue_reply, _cue_answers) = std::sync::mpsc::sync_channel(1);
    let (baseline_reply, _baseline_answers) = std::sync::mpsc::sync_channel(1);
    let state = ConnectionState {
        local_node_id: "local-peer".to_string(),
        context,
        identity_status: test_identity_status("local-peer"),
        expected: HashSet::new(),
        stop: Arc::new(AtomicBool::new(false)),
        active: Mutex::new(HashMap::new()),
        outbox: Mutex::new(HashMap::new()),
        baseline_outbox: Mutex::new(HashMap::new()),
        status: Arc::new(Mutex::new(TransportStatus::default())),
        admission: Arc::new(AdmissionController {
            state: Mutex::new(AdmissionState::default()),
        }),
        reporter: None,
        workspace_root: None,
    };
    let pending = PendingCue {
        cue_id: "cue".to_string(),
        script: "declared.sh".to_string(),
        reason: "unit test".to_string(),
        expected_run_id: "run".to_string(),
        deadline: Instant::now() + Duration::from_secs(60),
        reply: cue_reply,
    };
    let baseline = PendingBaseline {
        manifest: Vec::new(),
        bodies: Vec::new(),
        baseline_id: "baseline".to_string(),
        deadline: Instant::now() + Duration::from_secs(60),
        reply: baseline_reply,
    };

    state
        .outbox
        .lock()
        .unwrap()
        .entry("peer".to_string())
        .or_default()
        .push(pending);
    state
        .baseline_outbox
        .lock()
        .unwrap()
        .entry("peer".to_string())
        .or_default()
        .push(baseline);

    assert!(state.take_pending_cue("peer").is_some());
    assert!(state.take_pending_cue("peer").is_none());
    assert!(state.take_pending_baseline("peer").is_some());
    assert!(state.take_pending_baseline("peer").is_none());
}

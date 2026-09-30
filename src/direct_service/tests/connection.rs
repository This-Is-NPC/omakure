use super::*;

#[test]
fn static_peer_dial_ownership_is_deterministic_for_both_node_id_orderings() {
    fn state_for(local_node_id: &str) -> (tempfile::TempDir, ConnectionState) {
        let temp = tempfile::TempDir::new().expect("node root");
        let state = ConnectionState {
            local_node_id: local_node_id.to_string(),
            context: crate::test_support::node_context(temp.path()),
            identity_status: test_identity_status(local_node_id),
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
        (temp, state)
    }

    let (_lower_root, lower) = state_for("omk1_a");
    let (_higher_root, higher) = state_for("omk1_b");
    assert!(
        lower.should_initiate("omk1_b"),
        "the lexicographically lower node ID must own the dial"
    );
    assert!(
        !higher.should_initiate("omk1_a"),
        "the lexicographically higher node ID must remain listener-only"
    );
}

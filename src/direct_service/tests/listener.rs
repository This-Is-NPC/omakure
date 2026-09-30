use super::*;

#[test]
fn inbound_registration_does_not_use_static_peers_as_an_allowlist() {
    let expected = HashSet::from(["expected-peer".to_string()]);
    let status = Arc::new(Mutex::new(TransportStatus {
        enabled: true,
        listening: true,
        expected_peer_count: 1,
        connected_peer_count: 0,
        expected_connected_peer_count: 0,
        peers: Vec::new(),
        last_errors: BTreeMap::new(),
    }));
    let temp = tempfile::TempDir::new().expect("temporary node root");
    let state = Arc::new(ConnectionState {
        local_node_id: "local-peer".to_string(),
        context: crate::test_support::node_context(temp.path()),
        identity_status: test_identity_status("local-peer"),
        expected,
        stop: Arc::new(AtomicBool::new(false)),
        active: Mutex::new(HashMap::new()),
        outbox: Mutex::new(HashMap::new()),
        baseline_outbox: Mutex::new(HashMap::new()),
        status: Arc::clone(&status),
        admission: Arc::new(AdmissionController {
            state: Mutex::new(AdmissionState::default()),
        }),
        reporter: None,
        workspace_root: None,
    });
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    let claim = state
        .register(
            "trusted-but-not-static",
            ConnectionDirection::Responder,
            [7; 32],
            &server,
        )
        .unwrap();
    let status = status.lock().unwrap().clone();
    assert_eq!(status.connected_peer_count, 1);
    assert_eq!(status.expected_connected_peer_count, 0);
    drop(claim);
    drop(client);
}

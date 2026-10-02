use super::*;

#[test]
fn retry_backoff_keeps_the_opening_ladder_then_holds_at_the_ceiling() {
    for (failures, expected) in RETRY_BACKOFF.iter().enumerate() {
        assert_eq!(retry_backoff(failures), *expected);
    }
    assert_eq!(retry_backoff(RETRY_BACKOFF.len()), Duration::from_secs(8));
    assert_eq!(retry_backoff(usize::MAX), RETRY_BACKOFF_CEILING);
    let mut previous = Duration::ZERO;
    for failures in 0..64 {
        let delay = retry_backoff(failures);
        assert!(delay >= previous, "backoff shrank at {failures} failures");
        assert!(
            delay <= RETRY_BACKOFF_CEILING,
            "backoff passed the ceiling at {failures} failures"
        );
        previous = delay;
    }
}

/// A dial that fails on this node's own state must open no connection.
///
/// `connect_and_hold` used to open the socket first and only afterwards
/// reserve admission, load the identity, load the transport material, open
/// the registry, and build the handshake. Every one of those returns
/// early, so a fault entirely on this side left the peer holding an
/// accepted connection that never spoke: the peer charges that stray to
/// its own admission controller and records it in its audit trail as the
/// dialer's misbehaviour.
///
/// The dialer retries without bound now, so a persistent local fault
/// produced one such stray per redial forever rather than three in total.
///
/// Each case below is a real local fault -- no budget, material that has
/// gone away, a registry file that will not open -- and each pins itself
/// to the step it means to exercise before dialing, so a case cannot
/// quietly start failing one step earlier than its name claims. The sixth
/// step, `local.handshake`, has no case: it cannot be driven to fail once
/// `LocalTransport::load_existing` has accepted the key material, so it is
/// covered by sitting between the cases below and the first write.
///
/// Restore the old order and every case reddens.
#[test]
fn a_local_failure_before_the_first_write_opens_no_connection() {
    let _test_lock = RESOLVER_TEST_LOCK.lock().unwrap();

    use tempfile::TempDir;

    fn fresh_state(context: &NodeContext) -> Arc<ConnectionState> {
        Arc::new(ConnectionState {
            local_node_id: "local-peer".to_string(),
            context: context.clone(),
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
        })
    }

    /// Dial a listener that never accepts, and report both the failure and
    /// whether the kernel ever queued a connection for it. A listener is
    /// handed the completed connection by the kernel whether or not it
    /// calls `accept`, and it keeps it even after the dialer hangs up, so
    /// asking once afterwards is enough.
    fn dial_and_report(
        context: &NodeContext,
        state: &Arc<ConnectionState>,
    ) -> (TransportError, bool) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the observing listener");
        let address = listener.local_addr().expect("listener address");
        listener
            .set_nonblocking(true)
            .expect("set the listener non-blocking");
        let resolver = Resolver::start().expect("start the resolver");
        let peer = StaticPeer {
            node_id: "zzzz-remote-peer".to_string(),
            endpoint: address.to_string(),
        };
        let error = error_to_transport(
            connect_and_hold(&peer, context, state, &resolver)
                .expect_err("the local fault must fail the dial"),
        );
        resolver.shutdown();
        let opened = match listener.accept() {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
            Err(error) => panic!("the observing listener could not be polled: {error}"),
        };
        (error, opened)
    }

    // Step 1: no admission budget left for a dial.
    let temp = TempDir::new().expect("temporary node root");
    let context = crate::test_support::node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).expect("initialize the identity");
    LocalTransport::provision_new(&context, &identity).expect("provision transport material");
    let state = fresh_state(&context);
    let mut held = Vec::new();
    while let Some(reservation) = state.admission.reserve_dial() {
        held.push(reservation);
    }
    assert!(
        !held.is_empty(),
        "the admission controller refused the very first dial reservation, so this \
             case would prove nothing about a budget that had been spent"
    );
    let (error, opened) = dial_and_report(&context, &state);
    assert_eq!(error, TransportError::RateLimited);
    assert!(
        !opened,
        "a dial with no admission budget opened a connection and abandoned it"
    );
    drop(held);

    // Step 2: this node's identity is not on disk.
    let temp = TempDir::new().expect("temporary node root");
    let context = crate::test_support::node_context(temp.path());
    assert!(
        NodeIdentity::load_existing(&context).is_err(),
        "this case must fail on the identity load"
    );
    let (error, opened) = dial_and_report(&context, &fresh_state(&context));
    assert_eq!(error, TransportError::Internal);
    assert!(
        !opened,
        "a dial by a node that cannot load its own identity opened a connection \
             and abandoned it"
    );

    // Step 3: the transport key has gone away.
    let temp = TempDir::new().expect("temporary node root");
    let context = crate::test_support::node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).expect("initialize the identity");
    LocalTransport::provision_new(&context, &identity).expect("provision transport material");
    std::fs::remove_file(context.transport_key_path()).expect("remove the transport key");
    let identity = NodeIdentity::load_existing(&context)
        .expect("this case must reach the transport load, so the identity must still load");
    assert!(
        LocalTransport::load_existing(&context, &identity).is_err(),
        "this case must fail on the transport load"
    );
    let (error, opened) = dial_and_report(&context, &fresh_state(&context));
    assert_eq!(error, TransportError::Internal);
    assert!(
        !opened,
        "a dial by a node whose transport material has gone away opened a \
             connection and abandoned it"
    );

    // Step 4: the registry will not open.
    let temp = TempDir::new().expect("temporary node root");
    let context = crate::test_support::node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).expect("initialize the identity");
    LocalTransport::provision_new(&context, &identity).expect("provision transport material");
    // A directory where the registry file belongs. Docker creates exactly
    // this when a bind mount names a file that does not exist yet, so it
    // is a fault this fleet can really meet rather than an invented one.
    std::fs::remove_file(context.database_path()).expect("clear the registry path");
    std::fs::create_dir_all(context.database_path()).expect("occupy the registry path");
    let identity = NodeIdentity::load_existing(&context)
        .expect("this case must reach the registry open, so the identity must still load");
    LocalTransport::load_existing(&context, &identity)
        .expect("this case must reach the registry open, so the transport must still load");
    assert!(
        NodeRegistry::open_existing(&context, identity.public_status()).is_err(),
        "this case must fail on the registry open"
    );
    let (error, opened) = dial_and_report(&context, &fresh_state(&context));
    assert_eq!(error, TransportError::Internal);
    assert!(
        !opened,
        "a dial by a node whose registry will not open opened a connection and \
             abandoned it"
    );
}

/// The ceiling is only defensible if a peer that comes back is redialed
/// while the fleet still counts it Online.
#[test]
fn retry_ceiling_redials_within_the_presence_window() {
    let worst_case = RETRY_BACKOFF_CEILING + RETRY_JITTER_MAX + CONNECT_TIMEOUT + HANDSHAKE_TIMEOUT;
    let online = u64::try_from(crate::health_plane::bounds::PRESENCE_ONLINE_SECONDS)
        .expect("the Online window is a positive number of seconds");
    let online = Duration::from_secs(online);
    assert!(
        worst_case < online,
        "a redial can take {worst_case:?}, past the {online:?} Online window"
    );
}

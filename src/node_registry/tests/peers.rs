use super::*;

/// Direction one: a node that signs baselines may not take Conductor
/// authority over anyone, on any path that records a Performer.
///
/// Every entry point is exercised rather than the shared helper, because
/// the helper being right is worth nothing if one path forgets to call it.
#[test]
fn a_baseline_publisher_is_refused_conductor_authority_on_every_path() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
    crate::baseline_publisher::BaselinePublisher::create(&context, &registry).unwrap();

    assert!(matches!(
        registry.register_pending_with_transport(registration(&identity, 3), None),
        Err(RegistryError::PublisherConductorConflict)
    ));
    assert!(matches!(
        registry.import_manual_peer_with_transport(registration(&identity, 4), None),
        Err(RegistryError::PublisherConductorConflict)
    ));

    let now = crate::util::time::unix_seconds();
    let (_remote_context, remote) = remote_identity(&temp);
    let certificate = remote_certificate(&remote, now);
    let offer = ManualEnrollmentRequest::create(
        &remote,
        REMOTE_TRANSPORT_PUBLIC,
        crate::enrollment::EnrollmentRole::Performer,
        vec!["remote-run".to_string()],
        now,
        600,
    )
    .unwrap();
    assert!(matches!(
        registry.stage_manual_enrollment(
            &offer.request,
            certificate.as_bytes(),
            "operator",
            "staging a performer",
            now,
        ),
        Err(RegistryError::PublisherConductorConflict)
    ));

    let bundle = SignedEnrollmentBundle::sign_with_material(
        &[5u8; 32],
        [2; crate::enrollment::REQUEST_ID_BYTES],
        [8; crate::enrollment::BUNDLE_AUTHORITY_ID_BYTES],
        "omakure".to_string(),
        identity.public_status().node_id.clone(),
        remote.public_status().node_id.clone(),
        crate::enrollment::parse_hex(&remote.public_status().public_key_hex, 32)
            .unwrap()
            .try_into()
            .unwrap(),
        REMOTE_TRANSPORT_PUBLIC,
        *certificate.as_bytes(),
        crate::enrollment::EnrollmentRole::Performer,
        vec!["remote-run".to_string()],
        now,
        now + 600,
    )
    .unwrap();
    assert!(matches!(
        registry.activate_signed_bundle(
            &bundle,
            "operator",
            "activating a performer",
            now,
            &[1; 32],
            &[2; 32],
        ),
        Err(RegistryError::PublisherConductorConflict)
    ));

    assert!(
        registry
            .peers()
            .unwrap()
            .iter()
            .all(|peer| peer.role == PeerRole::Conductor),
        "no refused path may have left a Performer behind"
    );
}

/// Direction two, and the other order: a node that already conducts someone
/// cannot become a publisher — and the refused attempt leaves no key.
#[test]
fn a_conductor_is_refused_a_publisher_key_and_keeps_none() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
    let performer = registry
        .register_pending_with_transport(registration(&identity, 3), None)
        .unwrap();
    assert_eq!(performer.role, PeerRole::Performer);

    assert!(
        crate::baseline_publisher::BaselinePublisher::create(&context, &registry).is_err(),
        "a node that conducts a Performer must not become a publisher"
    );
    assert!(
        !context.publisher_key_path().exists(),
        "a refused create must not leave a key on disk for a later load to find"
    );
    assert!(
        context.validate_existing_state_contents().unwrap(),
        "and must not leave a stray temporary file in the state directory"
    );
}

/// The refusal is about the combination, not about publishers.
///
/// Without this the whole rule could have been "a publisher records no
/// peers", which would be a different and much blunter thing.
#[test]
fn a_publisher_may_still_record_the_conductor_it_answers_to() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
    crate::baseline_publisher::BaselinePublisher::create(&context, &registry).unwrap();

    let mut conductor = registration(&identity, 3);
    conductor.role = PeerRole::Conductor;
    let peer = registry
        .import_manual_peer_with_transport(conductor, None)
        .unwrap();
    assert_eq!(peer.state, PeerState::Active);
}

/// Only revocation ends a Conductor relationship, so only revocation frees
/// the node to publish.
#[test]
fn a_performer_blocks_publishing_until_it_is_revoked() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
    let performer = registry
        .register_pending_with_transport(registration(&identity, 3), None)
        .unwrap();

    registry
        .transition_peer(
            &performer.node_id,
            PeerState::Suspended,
            "operator",
            "paused",
        )
        .unwrap();
    assert!(
        crate::baseline_publisher::BaselinePublisher::create(&context, &registry).is_err(),
        "a suspended Performer can be reactivated, so it still counts"
    );

    registry
        .revoke_peer(&performer.node_id, "operator", "decommissioned")
        .unwrap();
    crate::baseline_publisher::BaselinePublisher::create(&context, &registry)
        .expect("a revoked Performer is terminal and no longer blocks publishing");
}

#[test]
fn full_transition_graph_and_revocation_precedence() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
    let peer = registry
        .register_pending_with_transport(registration(&identity, 3), None)
        .unwrap();
    assert_eq!(peer.state, PeerState::Pending);
    assert!(
        registry
            .transition_peer(&peer.node_id, PeerState::Active, "", "reason")
            .is_err()
    );
    assert_eq!(
        registry
            .transition_peer(&peer.node_id, PeerState::Active, "operator", "approve")
            .unwrap()
            .state,
        PeerState::Active
    );
    assert_eq!(
        registry
            .transition_peer(&peer.node_id, PeerState::Suspended, "operator", "pause")
            .unwrap()
            .state,
        PeerState::Suspended
    );
    assert_eq!(
        registry
            .transition_peer(&peer.node_id, PeerState::Active, "operator", "resume")
            .unwrap()
            .state,
        PeerState::Active
    );
    assert_eq!(
        registry
            .revoke_peer(&peer.node_id, "operator", "retire")
            .unwrap()
            .state,
        PeerState::Revoked
    );
    assert!(matches!(
        registry.transition_peer(&peer.node_id, PeerState::Active, "operator", "resurrect"),
        Err(RegistryError::Revoked(_))
    ));
    assert_eq!(registry.revocations().unwrap().len(), 1);
    assert_eq!(registry.audit_events().unwrap().len(), 5);
}

#[test]
fn rejects_self_duplicates_invalid_capabilities_and_bad_transitions() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
    let mut self_registration = registration(&identity, 5);
    self_registration.node_id = identity.public_status().node_id.clone();
    self_registration.public_key = identity.public_status().public_key_hex.clone();
    assert!(matches!(
        registry.register_pending_with_transport(self_registration, None),
        Err(RegistryError::SelfTrust)
    ));
    let peer = registry
        .register_pending_with_transport(registration(&identity, 7), None)
        .unwrap();
    assert!(matches!(
        registry.register_pending_with_transport(registration(&identity, 7), None),
        Err(RegistryError::Duplicate(_))
    ));
    assert!(
        registry
            .transition_peer(&peer.node_id, PeerState::Suspended, "operator", "bad")
            .is_ok()
    );
    assert!(matches!(
        registry.transition_peer(&peer.node_id, PeerState::Suspended, "operator", "again"),
        Err(RegistryError::InvalidTransition { .. })
    ));
    let mut unsupported = registration(&identity, 9);
    unsupported.capabilities = vec!["not-supported".to_string()];
    assert!(
        registry
            .register_pending_with_transport(unsupported, None)
            .is_err()
    );
}

#[test]
fn transaction_failure_does_not_leave_partial_peer_or_audit() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let registry = NodeRegistry::open(&context, identity.public_status()).unwrap();
    let peer = registry
        .register_pending_with_transport(registration(&identity, 11), None)
        .unwrap();
    assert!(
        registry
            .transition_peer(&peer.node_id, PeerState::Active, "operator", "\0")
            .is_err()
    );
    assert_eq!(
        registry.peer(&peer.node_id).unwrap().unwrap().state,
        PeerState::Pending
    );
    assert_eq!(registry.audit_events().unwrap().len(), 1);
}

#[test]
fn concurrent_register_operations_are_serialized() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).unwrap();
    let registry = Arc::new(NodeRegistry::open(&context, identity.public_status()).unwrap());
    let registrations = (2..18)
        .map(|scalar| registration(&identity, scalar))
        .collect::<Vec<_>>();
    let threads = (2..18)
        .zip(registrations)
        .map(|(_, registration)| {
            let registry = Arc::clone(&registry);
            thread::spawn(move || registry.register_pending_with_transport(registration, None))
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap().unwrap();
    }
    assert_eq!(registry.peers().unwrap().len(), 16);
}

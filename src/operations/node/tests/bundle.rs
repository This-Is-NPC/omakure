use super::*;

#[test]
fn local_bundle_apply_requires_a_configured_token_path_before_inspecting_bundle() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let error = apply_signed_bundle_from_local_token(
        &context,
        SignedBundleApplyRequest {
            bundle_hex: "invalid".into(),
            bootstrap_token: "untrusted".into(),
            bootstrap_nonce: "invalid".into(),
            bootstrap_token_path: None,
        },
        "test-token",
        None,
    )
    .unwrap_err();
    assert_eq!(error.code, OperationErrorCode::EnrollmentDenied);
    assert_eq!(
        error.message,
        "local bootstrap token file is not configured"
    );
}

#[test]
fn startup_recovery_without_a_token_path_does_not_open_node_state() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    recover_local_bootstrap_token_tombstones(&context, None).unwrap();
    assert!(!context.database_path().exists());
}

/// A recovery abort must carry the cause it was given.
#[test]
fn a_failed_cleanup_recovery_reports_what_actually_failed() {
    let cause = OperationError::new(
        OperationErrorCode::InvalidInput,
        "signed-bundle enrollment is not enabled",
    );
    let message = cleanup_recovery_error(&cause).to_string();
    assert!(
        message.contains("signed-bundle enrollment is not enabled"),
        "the abort must carry its cause, not discard it: {message}"
    );
}

#[test]
fn signed_bundle_apply_is_target_bound_atomic_and_single_use() {
    let _fault_lock = TOKEN_FAULT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap();
    set_private_token_fault(PrivateTokenFault::None);
    let target_temp = TempDir::new().unwrap();
    let target = node_context(target_temp.path());
    let authority_private = [2_u8; 32];
    let authority_signing_key = k256::schnorr::SigningKey::from_slice(&authority_private).unwrap();
    let token = "t".repeat(32);
    let nonce = [9_u8; 16];
    let mut config = NodeConfig::default();
    config.organization.id = "omakure".into();
    config.trust.enrollment = "signed-bundle".into();
    config.trust.bootstrap_token_hash =
        hex::encode(&enrollment::hash_bootstrap_token(token.as_bytes()));
    config.trust.bootstrap_nonce_hash = hex::encode(&enrollment::hash_bootstrap_nonce(&nonce));
    config.trust.authorities = vec![crate::domain::EnrollmentAuthority {
        key_id: hex::encode(&[8; 16]),
        public_key: hex::encode(&authority_signing_key.verifying_key().to_bytes()),
        revoked: false,
    }];

    let manager_temp = TempDir::new().unwrap();
    let manager_context = node_context(manager_temp.path());
    let manager = NodeIdentity::load_or_initialize(&manager_context).unwrap();
    let manager_transport = LocalTransport::provision_new(&manager_context, &manager).unwrap();
    initialize_node(&target, &config).unwrap();
    let target_identity = NodeIdentity::load_existing(&target).unwrap();
    let now = crate::util::time::unix_seconds();
    let bundle = enrollment::SignedEnrollmentBundle::sign_with_material(
        &authority_private,
        [7; enrollment::REQUEST_ID_BYTES],
        [8; enrollment::BUNDLE_AUTHORITY_ID_BYTES],
        "omakure".into(),
        target_identity.public_status().node_id.clone(),
        manager.public_status().node_id.clone(),
        enrollment::parse_hex(&manager.public_status().public_key_hex, 32)
            .unwrap()
            .try_into()
            .unwrap(),
        *manager_transport.certificate().transport_public(),
        *manager_transport.certificate().as_bytes(),
        EnrollmentRole::Conductor,
        vec!["remote-run".into()],
        now,
        now + 600,
    )
    .unwrap();
    let request = SignedBundleApplyRequest {
        bundle_hex: hex::encode(&bundle.encode()),
        bootstrap_token: token.clone(),
        bootstrap_nonce: hex::encode(&nonce),
        bootstrap_token_path: Some(target_temp.path().join("bootstrap.token")),
    };
    fs::write(
        request.bootstrap_token_path.as_ref().unwrap(),
        token.as_bytes(),
    )
    .unwrap();
    let permissions = fs::metadata(request.bootstrap_token_path.as_ref().unwrap())
        .unwrap()
        .permissions();
    #[cfg(unix)]
    let mut permissions = permissions;
    #[cfg(unix)]
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o600);
    fs::set_permissions(request.bootstrap_token_path.as_ref().unwrap(), permissions).unwrap();
    let peer = apply_signed_bundle(&target, request.clone()).unwrap();
    assert_eq!(peer.state, "active");
    assert!(!request.bootstrap_token_path.as_ref().unwrap().exists());
    assert_eq!(list_trusted_peers(&target).unwrap().len(), 1);
    let replay = apply_signed_bundle(
        &target,
        SignedBundleApplyRequest {
            bootstrap_token: token,
            bootstrap_token_path: None,
            ..request
        },
    )
    .unwrap_err();
    assert_eq!(replay.code, OperationErrorCode::EnrollmentReplay);
    assert_eq!(list_trusted_peers(&target).unwrap().len(), 1);
}

#[test]
fn signed_bundle_proof_failure_restores_token_before_retry() {
    let _fault_lock = TOKEN_FAULT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap();
    set_private_token_fault(PrivateTokenFault::None);
    let fixture = signed_bundle_fixture([26; 32], 36, 46);
    write_secure_token(&fixture.token_path, &"x".repeat(32));

    let error = apply_signed_bundle(&fixture.target, fixture.request.clone()).unwrap_err();
    assert_eq!(error.code, OperationErrorCode::EnrollmentDenied);
    assert_eq!(error.message, "bootstrap proof does not match local policy");
    assert_eq!(
        fs::read_to_string(&fixture.token_path).unwrap(),
        "x".repeat(32)
    );
    assert!(list_trusted_peers(&fixture.target).unwrap().is_empty());

    write_secure_token(&fixture.token_path, &fixture.request.bootstrap_token);
    let applied = apply_signed_bundle(&fixture.target, fixture.request).unwrap();
    assert_eq!(applied.state, "active");
    assert!(!fixture.token_path.exists());
}

#[test]
fn signed_bundle_token_consumption_faults_are_recoverable_across_restart() {
    let _fault_lock = TOKEN_FAULT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap();

    let fixture = signed_bundle_fixture([21; 32], 31, 41);
    set_private_token_fault(PrivateTokenFault::Rename);
    assert!(apply_signed_bundle(&fixture.target, fixture.request.clone()).is_err());
    set_private_token_fault(PrivateTokenFault::None);
    assert!(fixture.token_path.exists());
    assert!(list_trusted_peers(&fixture.target).unwrap().is_empty());

    let fixture = signed_bundle_fixture([22; 32], 32, 42);
    fail_enrollment_audits(&fixture.target, true);
    assert!(apply_signed_bundle(&fixture.target, fixture.request.clone()).is_err());
    fail_enrollment_audits(&fixture.target, false);
    assert!(fixture.token_path.exists());
    assert!(list_trusted_peers(&fixture.target).unwrap().is_empty());

    let fixture = signed_bundle_fixture([23; 32], 33, 43);
    fail_enrollment_audits(&fixture.target, true);
    set_private_token_fault(PrivateTokenFault::Restore);
    assert!(apply_signed_bundle(&fixture.target, fixture.request.clone()).is_err());
    set_private_token_fault(PrivateTokenFault::None);
    fail_enrollment_audits(&fixture.target, false);
    assert!(!fixture.token_path.exists());
    let identity = NodeIdentity::load_existing(&fixture.target).unwrap();
    let registry = NodeRegistry::open_existing(&fixture.target, identity.public_status()).unwrap();
    recover_private_token_tombstones(
        &fixture.target,
        &registry,
        &fixture.organization,
        &fixture.token_path,
    )
    .unwrap();
    assert!(fixture.token_path.exists());
    assert!(list_trusted_peers(&fixture.target).unwrap().is_empty());

    let fixture = signed_bundle_fixture([24; 32], 34, 44);
    set_private_token_fault(PrivateTokenFault::Delete);
    let applied = apply_signed_bundle(&fixture.target, fixture.request.clone()).unwrap();
    assert!(applied.cleanup_pending);
    set_private_token_fault(PrivateTokenFault::None);
    assert!(!fixture.token_path.exists());
    assert_eq!(
        fixture
            .target
            .list_private_token_tombstones(
                &fixture.token_path,
                enrollment::MAX_BOOTSTRAP_TOKEN_BYTES,
            )
            .unwrap()
            .len(),
        1
    );
    let identity = NodeIdentity::load_existing(&fixture.target).unwrap();
    let registry = NodeRegistry::open_existing(&fixture.target, identity.public_status()).unwrap();
    set_private_token_fault(PrivateTokenFault::Delete);
    let recovery_error = recover_private_token_tombstones(
        &fixture.target,
        &registry,
        &fixture.organization,
        &fixture.token_path,
    )
    .unwrap_err();
    assert!(
        recovery_error
            .message
            .contains("bootstrap token cleanup recovery failed")
    );
    set_private_token_fault(PrivateTokenFault::None);
    recover_private_token_tombstones(
        &fixture.target,
        &registry,
        &fixture.organization,
        &fixture.token_path,
    )
    .unwrap();
    assert!(!fixture.token_path.exists());
    assert!(
        fixture
            .target
            .list_private_token_tombstones(
                &fixture.token_path,
                enrollment::MAX_BOOTSTRAP_TOKEN_BYTES,
            )
            .unwrap()
            .is_empty()
    );
    let cleanup_count: i64 = Connection::open(fixture.target.database_path())
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM enrollment_audits WHERE event_code = 'cleanup_completed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cleanup_count, 1);
    set_private_token_fault(PrivateTokenFault::None);
}

#[test]
fn cleanup_completion_failure_leaves_durable_pending_proof() {
    let _fault_lock = TOKEN_FAULT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap();
    set_private_token_fault(PrivateTokenFault::None);
    let fixture = signed_bundle_fixture([25; 32], 35, 45);
    fail_cleanup_completion_audit(&fixture.target, true);
    let applied = apply_signed_bundle(&fixture.target, fixture.request.clone()).unwrap();
    assert!(applied.cleanup_pending);
    let state: String = Connection::open(fixture.target.database_path())
        .unwrap()
        .query_row(
            "SELECT cleanup_state FROM bootstrap_proofs WHERE consumed_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "pending");
    fail_cleanup_completion_audit(&fixture.target, false);
    recover_private_token_tombstones(
        &fixture.target,
        &NodeRegistry::open_existing(
            &fixture.target,
            NodeIdentity::load_existing(&fixture.target)
                .unwrap()
                .public_status(),
        )
        .unwrap(),
        &fixture.organization,
        &fixture.token_path,
    )
    .unwrap();
    let state: String = Connection::open(fixture.target.database_path())
        .unwrap()
        .query_row(
            "SELECT cleanup_state FROM bootstrap_proofs WHERE consumed_at IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "complete");
}

#[test]
fn signed_bundle_distinct_conductors_have_one_transactional_winner() {
    let _fault_lock = TOKEN_FAULT_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap();
    set_private_token_fault(PrivateTokenFault::None);
    let target_temp = TempDir::new().unwrap();
    let target = node_context(target_temp.path());
    let authority_private = [3_u8; 32];
    let authority = k256::schnorr::SigningKey::from_slice(&authority_private).unwrap();
    let token = "t".repeat(32);
    let nonce = [12_u8; 16];
    let mut config = NodeConfig::default();
    config.organization.id = "omakure".into();
    config.trust.enrollment = "signed-bundle".into();
    config.trust.bootstrap_token_hash =
        hex::encode(&enrollment::hash_bootstrap_token(token.as_bytes()));
    config.trust.bootstrap_nonce_hash = hex::encode(&enrollment::hash_bootstrap_nonce(&nonce));
    config.trust.authorities = vec![crate::domain::EnrollmentAuthority {
        key_id: hex::encode(&[8; 16]),
        public_key: hex::encode(&authority.verifying_key().to_bytes()),
        revoked: false,
    }];
    initialize_node(&target, &config).unwrap();

    let manager_a_temp = TempDir::new().unwrap();
    let manager_b_temp = TempDir::new().unwrap();
    let manager_a_context = node_context(manager_a_temp.path());
    let manager_b_context = node_context(manager_b_temp.path());
    let manager_a = NodeIdentity::load_or_initialize(&manager_a_context).unwrap();
    let manager_b = NodeIdentity::load_or_initialize(&manager_b_context).unwrap();
    let transport_a = LocalTransport::provision_new(&manager_a_context, &manager_a).unwrap();
    let transport_b = LocalTransport::provision_new(&manager_b_context, &manager_b).unwrap();
    let target_id = NodeIdentity::load_existing(&target)
        .unwrap()
        .public_status()
        .node_id
        .clone();
    let make_bundle = |bundle_id: [u8; 16], manager: &NodeIdentity, transport: &LocalTransport| {
        enrollment::SignedEnrollmentBundle::sign_with_material(
            &authority_private,
            bundle_id,
            [8; 16],
            "omakure".into(),
            target_id.clone(),
            manager.public_status().node_id.clone(),
            enrollment::parse_hex(&manager.public_status().public_key_hex, 32)
                .unwrap()
                .try_into()
                .unwrap(),
            *transport.certificate().transport_public(),
            *transport.certificate().as_bytes(),
            EnrollmentRole::Conductor,
            vec!["remote-run".into()],
            crate::util::time::unix_seconds(),
            crate::util::time::unix_seconds() + 600,
        )
        .unwrap()
    };
    let requests = [
        SignedBundleApplyRequest {
            bundle_hex: hex::encode(&make_bundle([13; 16], &manager_a, &transport_a).encode()),
            bootstrap_token: token.clone(),
            bootstrap_nonce: hex::encode(&nonce),
            bootstrap_token_path: None,
        },
        SignedBundleApplyRequest {
            bundle_hex: hex::encode(&make_bundle([14; 16], &manager_b, &transport_b).encode()),
            bootstrap_token: token,
            bootstrap_nonce: hex::encode(&nonce),
            bootstrap_token_path: None,
        },
    ];
    let target_a = target.clone();
    let target_b = target.clone();
    let request_a = requests[0].clone();
    let request_b = requests[1].clone();
    let first = std::thread::spawn(move || apply_signed_bundle(&target_a, request_a));
    let second = std::thread::spawn(move || apply_signed_bundle(&target_b, request_b));
    let results = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(list_trusted_peers(&target).unwrap().len(), 1);
    assert!(results.iter().any(|result| {
        result.as_ref().err().is_some_and(|error| {
            error.code == OperationErrorCode::Conflict
                || error.code == OperationErrorCode::EnrollmentReplay
        })
    }));
}

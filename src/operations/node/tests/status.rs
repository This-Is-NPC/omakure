use super::*;

#[test]
fn status_is_observational_and_mutations_require_evidence() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let before = public_node_status(&context).unwrap();
    assert!(!before.initialized);
    assert!(!context.state_dir().exists());

    let initialized = initialize_node(&context, &NodeConfig::default()).unwrap();
    assert!(initialized.status.initialized);
    let identity = NodeIdentity::load_existing(&context).unwrap();
    let mut request = peer_request(&identity);
    request.confirmed = false;
    let error = import_manual_trust(&context, request).unwrap_err();
    assert_eq!(error.code, OperationErrorCode::Forbidden);
    assert!(list_trusted_peers(&context).unwrap().is_empty());
}

/// An abort must say what failed, not only that something did.
///
/// These three variants collapsed into one opaque sentence, and it cost a
/// real debugging session on a provisioned machine: the node refused to
/// start, the operator had root, and `node state is invalid or insecure`
/// named neither the file nor the problem. The reason was sitting in the
/// error that was being discarded.
#[test]
fn a_refused_node_path_says_which_path_and_why() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    context.ensure_state_directory().expect("state dir");

    // A directory where a file belongs: `UnexpectedFileType`, which used
    // to be reported as the same sentence as every other refusal.
    std::fs::create_dir(context.state_dir().join("identity.key")).expect("decoy");

    let error = public_node_status(&context).expect_err("a directory is not an identity");
    let message = error.to_string();
    assert!(
        message.contains("identity.key"),
        "the refusal must name the entry it refused: {message}"
    );
    assert_ne!(
        message, "node state is invalid or insecure",
        "the opaque sentence is what this test exists to prevent"
    );
}

#[test]
fn initialization_does_not_recreate_deleted_transport_state() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    initialize_node(&context, &NodeConfig::default()).unwrap();
    fs::remove_file(context.transport_key_path()).unwrap();
    fs::remove_file(context.transport_certificate_path()).unwrap();

    assert!(initialize_node(&context, &NodeConfig::default()).is_err());
    assert!(!context.transport_key_path().exists());
    assert!(!context.transport_certificate_path().exists());
}

/// Applying a bundle while this process already serves must not be
/// reported as a lifecycle conflict.
///
/// `node serve` holds the lifecycle lock for its whole life. The apply path
/// re-initializes only when the state looks incomplete, and that branch
/// takes the lock *non-blocking* — so a state directory that merely looks
/// wrong turns "your state has a stray entry" into "lifecycle busy", which
/// sends the reader after the wrong problem entirely.
///
/// Measured, not assumed: today it already reports
/// `node state is invalid or insecure`, because
/// `validate_existing_state_contents` *errors* on a stray entry rather than
/// returning `false`, and that error propagates before the re-init branch
/// is reached. So the hazard is not live and no `_locked` variant is
/// needed. This pins the diagnosis so it stays that way.
///
/// The one shape that would still misreport is the state directory being
/// deleted out from under a running service — `Ok(false)` rather than an
/// error, so the re-init branch runs and the non-blocking lock fails.
/// Left alone deliberately: a state directory that vanishes mid-serve is
/// not a case worth carrying a code path for.
#[test]
fn a_stray_state_entry_is_not_reported_as_a_lifecycle_conflict() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    let mut config = crate::domain::NodeConfig::default();
    config.trust.enrollment = "signed-bundle".to_string();
    config.organization.id = "stray-diagnosis".to_string();
    config.trust.authorities = vec![crate::domain::EnrollmentAuthority {
        key_id: "0".repeat(32),
        public_key: "0".repeat(64),
        revoked: false,
    }];
    config.trust.bootstrap_token_hash = "0".repeat(64);
    config.trust.bootstrap_nonce_hash = "0".repeat(64);
    initialize_node_nonblocking(&context, &config).expect("initialize");
    std::fs::write(
        context.config_path(),
        toml::to_string(&config).expect("serialize config"),
    )
    .expect("write config");

    // Hold the lock the way a serving process does.
    let _serving = context.acquire_lifecycle_lock().expect("hold the lock");

    std::fs::write(context.state_dir().join("stray.txt"), b"x").expect("stray");

    let error = apply_signed_bundle(
        &context,
        SignedBundleApplyRequest {
            bundle_hex: String::new(),
            bootstrap_token: "irrelevant".into(),
            bootstrap_token_path: None,
            bootstrap_nonce: "00".repeat(16),
        },
    )
    .expect_err("a stray state entry must not be applied over");
    let message = error.to_string().to_lowercase();
    assert!(
        message.contains("state"),
        "the reader must be sent at the state problem: {message}"
    );
    assert!(
        !message.contains("busy") && !message.contains("lifecycle"),
        "and not at the lock: {message}"
    );
}

#[test]
fn status_treats_missing_config_parent_as_uninitialized() {
    let temp = TempDir::new().unwrap();
    let context = NodeContext::resolve_for(
        NodePlatform::current(),
        NodePathOverrides::new(
            Some(temp.path().join("state")),
            Some(temp.path().join("missing/node.toml")),
        ),
        true,
        None,
        None,
        None,
    )
    .unwrap();

    let status = public_node_status(&context).unwrap();
    assert!(!status.initialized);
    assert!(status.identity.is_none());
    assert!(status.config.is_none());
}

#[test]
fn status_fails_closed_on_corrupt_registry_without_replacing_identity() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    initialize_node(&context, &NodeConfig::default()).unwrap();
    let private_before = std::fs::read(context.identity_path()).unwrap();
    std::fs::write(context.database_path(), b"not a sqlite database").unwrap();

    let error = public_node_status(&context).unwrap_err();
    assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
    assert_eq!(
        std::fs::read(context.identity_path()).unwrap(),
        private_before
    );
}

#[test]
fn status_redacts_malformed_config_values() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    initialize_node(&context, &NodeConfig::default()).unwrap();
    let secret = "static-peer-secret-value";
    let malformed = NodeConfig::default().to_toml().unwrap().replace(
        "static_peers = []",
        &format!("static_peers = [\"{secret}\"]"),
    );
    std::fs::write(context.config_path(), malformed).unwrap();

    let error = public_node_status(&context).unwrap_err();
    assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
    assert_eq!(error.message, "node configuration is invalid or corrupt");
    assert!(!error.message.contains(secret));
}

#[test]
fn status_rejects_oversized_config_before_parsing() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    initialize_node(&context, &NodeConfig::default()).unwrap();
    std::fs::write(
        context.config_path(),
        format!(
            "{}\n#{}",
            NodeConfig::default().to_toml().unwrap(),
            "x".repeat(MAX_NODE_CONFIG_BYTES)
        ),
    )
    .unwrap();

    let error = public_node_status(&context).unwrap_err();
    assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
    assert_eq!(error.message, "node configuration exceeds maximum size");
}

#[cfg(unix)]
#[test]
fn status_rejects_insecure_public_config_mode_before_reading() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    initialize_node(&context, &NodeConfig::default()).unwrap();
    std::fs::set_permissions(
        context.config_path(),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();

    let error = public_node_status(&context).unwrap_err();
    assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
    // The refusal has to lead somewhere. An operator who reads this must be
    // able to get from it to the fix without reading the source.
    let path = context.config_path().display().to_string();
    assert!(
        error.message.contains(&path)
            && error.message.contains("0644")
            && error.message.contains("0640")
            && error.message.contains("chmod 640"),
        "the refusal must name the file, the mode, and the remedy: {}",
        error.message
    );
}

#[cfg(unix)]
#[test]
fn status_rejects_final_and_intermediate_config_symlinks() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    initialize_node(&context, &NodeConfig::default()).unwrap();

    let outside = temp.path().join("outside.toml");
    std::fs::write(&outside, NodeConfig::default().to_toml().unwrap()).unwrap();
    std::fs::remove_file(context.config_path()).unwrap();
    symlink(&outside, context.config_path()).unwrap();
    let error = public_node_status(&context).unwrap_err();
    assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
    // O_NOFOLLOW refuses the final symlink. The refusal names the path it
    // refused, which is what tells the operator which file to look at.
    assert!(
        error
            .message
            .contains(&context.config_path().display().to_string())
            && error.message.contains("could not be opened securely"),
        "the refusal must name the file it refused: {}",
        error.message
    );

    std::fs::remove_file(context.config_path()).unwrap();
    let real_parent = temp.path().join("real-config");
    let link_parent = temp.path().join("linked-config");
    std::fs::create_dir(&real_parent).unwrap();
    let real_config = real_parent.join("node.toml");
    std::fs::write(&real_config, NodeConfig::default().to_toml().unwrap()).unwrap();
    symlink(&real_parent, &link_parent).unwrap();
    let linked_context = NodeContext::resolve_for(
        NodePlatform::current(),
        NodePathOverrides::new(
            Some(context.state_dir().to_path_buf()),
            Some(link_parent.join("node.toml")),
        ),
        true,
        None,
        None,
        None,
    )
    .unwrap();
    let error = public_node_status(&linked_context).unwrap_err();
    assert_eq!(error.code, OperationErrorCode::RegistryInvalid);
    // A symlinked ancestor is refused for a different reason than a
    // symlinked config file, and the operator has to be told which one
    // they hit: the two are repaired differently.
    assert!(
        error.message.contains(&link_parent.display().to_string()),
        "the refusal must name the unsafe ancestor: {}",
        error.message
    );
}

#[test]
fn concurrent_service_initialization_converges_without_duplicate_state() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let temp = TempDir::new().unwrap();
    let context = Arc::new(node_context(temp.path()));
    let barrier = Arc::new(Barrier::new(8));
    let handles = (0..8)
        .map(|_| {
            let context = Arc::clone(&context);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                initialize_node(&context, &NodeConfig::default()).unwrap()
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    let identity = results[0].status.identity.clone().unwrap();
    assert!(results.iter().all(|result| {
        result.status.identity.as_ref() == Some(&identity) && result.status.trust.peer_count == 0
    }));
    assert!(context.identity_path().is_file());
    assert!(context.database_path().is_file());
}

#[test]
fn factory_reset_requires_confirmation_and_removes_identity_and_trust_only() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    initialize_node(&context, &NodeConfig::default()).unwrap();
    let identity_before = NodeIdentity::load_existing(&context)
        .unwrap()
        .public_status()
        .clone();

    let denied = reset_node(&context, false).unwrap_err();
    assert_eq!(denied.code, OperationErrorCode::Forbidden);
    assert!(context.identity_path().exists());

    let result = reset_node(&context, true).unwrap();
    assert!(result.state_removed);
    assert!(result.identity_removed);
    assert!(result.trust_removed);
    assert!(context.state_dir().is_dir());
    assert!(!context.identity_path().exists());
    assert!(!context.database_path().exists());
    assert!(context.config_path().is_file());

    initialize_node(&context, &NodeConfig::default()).unwrap();
    let identity_after = NodeIdentity::load_existing(&context)
        .unwrap()
        .public_status()
        .clone();
    assert_ne!(identity_before, identity_after);
    assert!(list_trusted_peers(&context).unwrap().is_empty());
}

#[test]
fn reset_and_initialize_race_preserves_identity_registry_pairing() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let temp = TempDir::new().unwrap();
    let context = Arc::new(node_context(temp.path()));
    initialize_node(&context, &NodeConfig::default()).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let reset_context = Arc::clone(&context);
    let reset_barrier = Arc::clone(&barrier);
    let reset = thread::spawn(move || {
        reset_barrier.wait();
        reset_node(&reset_context, true)
    });
    let init_context = Arc::clone(&context);
    let init_barrier = Arc::clone(&barrier);
    let init = thread::spawn(move || {
        init_barrier.wait();
        initialize_node(&init_context, &NodeConfig::default())
    });

    let reset_result = reset.join().unwrap();
    let init_result = init.join().unwrap();
    assert!(
        reset_result.is_ok()
            || reset_result.as_ref().unwrap_err().code == OperationErrorCode::Conflict
    );
    assert!(init_result.is_ok());

    let identity_exists = context.identity_path().is_file();
    let registry_exists = context.database_path().is_file();
    assert_eq!(identity_exists, registry_exists);
    if identity_exists {
        let identity = NodeIdentity::load_existing(&context).unwrap();
        NodeRegistry::open_existing(&context, identity.public_status()).unwrap();
    }
}

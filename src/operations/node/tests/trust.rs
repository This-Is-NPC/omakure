use super::*;

#[test]
fn cue_execution_lock_prepare_failure_preserves_operation_error() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    fs::write(context.state_dir(), "not a directory").unwrap();

    assert!(matches!(
        crate::remote_cue::ExecutionGuard::acquire(&context, "omk1_test"),
        Err(crate::remote_cue::ExecutionLockError::Prepare(
            crate::node::NodeError::UnexpectedFileType(_)
        ))
    ));

    let error = update_peer_capabilities(
        &context,
        CapabilityUpdateRequest {
            node_id: "omk1_test".into(),
            capabilities: Vec::new(),
            actor: "operator".into(),
            reason: "test".into(),
            confirmed: true,
        },
    )
    .unwrap_err();
    assert_eq!(error.code, OperationErrorCode::IoFailed);
    assert_eq!(
        error.message,
        format!(
            "cannot prepare Cue execution lock: node path has unexpected file type: {}",
            context.state_dir().display()
        )
    );
}

/// A conflict must say which conflict it is.
///
/// "This peer is already trusted" and "this peer was revoked and cannot be
/// resurrected" want opposite things from an operator: leave it alone, or
/// issue the machine a new identity. Both arrived as `Conflict` carrying
/// the inner node id as the entire message, so the refusal read as nothing
/// but the id the caller had just typed. Measured on a real fleet: after
/// `node revoke`, re-trusting the same peer answered
/// `{"code":"conflict","message":"omk1_709c1c..."}` and nothing else.
#[test]
fn a_trust_conflict_says_whether_the_peer_is_already_trusted_or_revoked() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    initialize_node(&context, &NodeConfig::default()).unwrap();
    let identity = NodeIdentity::load_existing(&context).unwrap();
    let request = peer_request(&identity);
    let node_id = request.node_id.clone();
    import_manual_trust(&context, request.clone()).expect("first trust");

    // Already trusted.
    let duplicate = import_manual_trust(&context, request.clone()).unwrap_err();
    assert_eq!(duplicate.code, OperationErrorCode::Conflict);
    assert!(
        duplicate.message.contains(&node_id) && duplicate.message.contains("already exists"),
        "a duplicate must say the peer is already there: {}",
        duplicate.message
    );

    revoke_peer(
        &context,
        &crate::workspace::Workspace::new(temp.path().join("workspace")),
        RevocationRequest {
            node_id: node_id.clone(),
            actor: "operator".into(),
            reason: "lost device".into(),
            confirmed: true,
        },
    )
    .expect("revoke");

    // Revoked, which is a different conflict with a different remedy.
    let revoked = import_manual_trust(&context, request).unwrap_err();
    assert_eq!(revoked.code, OperationErrorCode::Conflict);
    assert!(
        revoked.message.contains("revocation"),
        "a revoked peer's refusal must name the revocation, not just the id: {}",
        revoked.message
    );
    assert!(
        revoked.message.contains("new identity"),
        "the refusal must say what the operator can actually do: {}",
        revoked.message
    );
    assert_ne!(
        revoked.message, duplicate.message,
        "two opposite conflicts must not produce the same sentence"
    );
}

#[test]
fn manual_import_update_and_revoke_are_public_and_replay_safe() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    initialize_node(&context, &NodeConfig::default()).unwrap();
    let identity = NodeIdentity::load_existing(&context).unwrap();
    let request = peer_request(&identity);
    let node_id = request.node_id.clone();
    let peer = import_manual_trust(&context, request.clone()).unwrap();
    assert_eq!(peer.state, "active");
    let registry = context
        .open_trust_registry(
            NodeIdentity::load_existing(&context)
                .unwrap()
                .public_status(),
        )
        .unwrap();
    let audit = registry.audit_events().unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].actor, "operator");
    assert_eq!(audit[0].reason, "approved manually");
    assert!(import_manual_trust(&context, request).is_err());
    let updated = update_peer_capabilities(
        &context,
        CapabilityUpdateRequest {
            node_id: node_id.clone(),
            capabilities: vec!["notifications".into()],
            actor: "operator".into(),
            reason: "narrowed".into(),
            confirmed: true,
        },
    )
    .unwrap();
    assert_eq!(updated.capabilities, vec!["notifications"]);
    let workspace = crate::workspace::Workspace::new(temp.path().join("workspace"));
    let revoked = revoke_peer(
        &context,
        &workspace,
        RevocationRequest {
            node_id: node_id.clone(),
            actor: "operator".into(),
            reason: "retired".into(),
            confirmed: true,
        },
    )
    .unwrap();
    assert_eq!(revoked.state, "revoked");
    assert!(
        revoke_peer(
            &context,
            &workspace,
            RevocationRequest {
                node_id,
                actor: "operator".into(),
                reason: "replay".into(),
                confirmed: true,
            },
        )
        .is_err()
    );
}

#[test]
fn revocation_succeeds_with_pending_cleanup_when_runs_storage_is_unavailable() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    initialize_node(&context, &NodeConfig::default()).unwrap();
    let identity = NodeIdentity::load_existing(&context).unwrap();
    let request = peer_request(&identity);
    let node_id = request.node_id.clone();
    import_manual_trust(&context, request).unwrap();

    let workspace = crate::workspace::Workspace::new(temp.path().join("workspace"));
    workspace.ensure_layout().unwrap();
    let history = workspace.history_dir().to_path_buf();
    let history_backup = temp.path().join("history-backup");
    fs::rename(&history, &history_backup).unwrap();
    fs::write(&history, "injected runs storage failure").unwrap();

    let revoked = revoke_peer(
        &context,
        &workspace,
        RevocationRequest {
            node_id,
            actor: "operator".into(),
            reason: "lost device".into(),
            confirmed: true,
        },
    )
    .expect("trust withdrawal must not depend on runs storage");
    assert_eq!(revoked.state, "revoked");
    assert!(revoked.cleanup_pending);
    assert!(
        revoked
            .cleanup_error
            .as_deref()
            .is_some_and(|error| error.contains("runs database"))
    );
}

#[test]
fn revoked_cue_cleanup_reconciles_after_runs_storage_returns() {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
    initialize_node(&context, &NodeConfig::default()).unwrap();
    let identity = NodeIdentity::load_existing(&context).unwrap();
    let request = peer_request(&identity);
    let node_id = request.node_id.clone();
    import_manual_trust(&context, request).unwrap();

    let workspace = crate::workspace::Workspace::new(temp.path().join("workspace"));
    workspace.ensure_layout().unwrap();
    let conn = crate::runs::open(&workspace).unwrap();
    let row = crate::runs::enqueue(
        &conn,
        "/workspace/deploy.sh",
        &[],
        crate::runs::EnqueueOptions {
            actor: node_id.clone(),
            trigger: crate::runs::RunTrigger::Cue,
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);

    let history = workspace.history_dir().to_path_buf();
    let history_backup = temp.path().join("history-backup");
    fs::rename(&history, &history_backup).unwrap();
    fs::write(&history, "injected runs storage failure").unwrap();
    let revoked = revoke_peer(
        &context,
        &workspace,
        RevocationRequest {
            node_id: node_id.clone(),
            actor: "operator".into(),
            reason: "lost device".into(),
            confirmed: true,
        },
    )
    .unwrap();
    assert!(revoked.cleanup_pending);

    let error = reconcile_revoked_cue_runs(&context, &workspace).unwrap_err();
    assert_eq!(error.code, OperationErrorCode::IoFailed);
    assert!(
        error
            .message
            .starts_with("cannot reconcile revoked Cue runs: Create history dir failed: ")
    );

    fs::remove_file(&history).unwrap();
    fs::rename(&history_backup, &history).unwrap();
    let reconciled = reconcile_revoked_cue_runs(&context, &workspace).unwrap();
    assert_eq!(reconciled, vec![row.run_id.clone()]);
    let conn = crate::runs::open(&workspace).unwrap();
    assert_eq!(
        crate::runs::get_run(&conn, &row.run_id)
            .unwrap()
            .unwrap()
            .state,
        crate::runs::RunState::Cancelled
    );
}

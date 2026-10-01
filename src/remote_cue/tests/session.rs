use super::super::enqueue::classify_enqueue_error;
use super::*;
use crate::test_support::configured_node_context;

#[test]
fn duplicate_cache_retains_reportable_ack_and_silent_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let (identity, registry) = identity_and_registry(dir.path());
    let workspace = workspace_with_declared_script(dir.path());
    let mut session = cue_session(&registry, &identity, [7u8; 32], workspace);

    session.pending_reply = Some(vec![1, 2, 3]);
    session.remember_cue("0123456789abcdef0123456789abcdef", 100);
    session.pending_reply = None;
    session.remember_cue("fedcba9876543210fedcba9876543210", 100);

    assert_eq!(session.cue_records[0].reply, Some(vec![1, 2, 3]));
    assert_eq!(session.cue_records[1].reply, None);
}

// -----------------------------------------------------------------------
// Durable at-most-once, across sessions
// -----------------------------------------------------------------------

/// A node context with an identity and an open registry, in `root`.
fn identity_and_registry(
    root: &std::path::Path,
) -> (
    crate::node_identity::NodeIdentity,
    crate::node_registry::NodeRegistry,
) {
    let context = configured_node_context(root);
    let identity =
        crate::node_identity::NodeIdentity::load_or_initialize(&context).expect("identity");
    let registry =
        crate::node_registry::NodeRegistry::open_existing(&context, identity.public_status())
            .expect("registry");
    (identity, registry)
}

/// A workspace holding the one script the Performer declares.
fn workspace_with_declared_script(root: &std::path::Path) -> crate::workspace::Workspace {
    let workspace = crate::workspace::Workspace::new(root.join("workspace"));
    workspace.ensure_layout().expect("workspace layout");
    let script = workspace.scripts_root().join("deploy.sh");
    std::fs::create_dir_all(script.parent().expect("scripts root")).expect("scripts root");
    std::fs::write(
        &script,
        "#!/usr/bin/env bash\n\
             # OMAKURE_SCHEMA_START\n\
             # {\"Name\":\"deploy.sh\",\"Fields\":[]}\n\
             # OMAKURE_SCHEMA_END\n\
             echo ok\n",
    )
    .expect("write the declared script");
    workspace
}

fn cue_session<'a>(
    registry: &'a crate::node_registry::NodeRegistry,
    identity: &'a crate::node_identity::NodeIdentity,
    session_id: [u8; 32],
    workspace: crate::workspace::Workspace,
) -> CueSession<'a> {
    CueSession::new(
        registry,
        identity,
        "omk1_0000000000000000000000000000000000000000000000000000000000000000",
        [3u8; 32],
        session_id,
        CuePolicy {
            enabled: true,
            declared_scripts: vec!["deploy.sh".to_string()],
            declared_batteries: Vec::new(),
        },
        Some(workspace),
    )
}

/// The same Cue arriving after the node session is restarted must not run a
/// second time.
///
/// `a_new_session_does_not_inherit_the_seen_set` establishes that the
/// in-session guard dies with the connection, and `seen_cue_ids` above says
/// durable at-most-once is `runs.run_id`. Nothing exercised the sentence.
/// The unit test on `enqueue_cue_run` proves SQLite refuses a repeated run
/// id; it does not prove `CueSession` derives that id from the cue id,
/// reaches the insert, or reports the refusal as `Duplicate` rather than as
/// something that reads like a fault in the script.
///
/// This is the only shape in which the primary key is the sole guard: a
/// Conductor whose session drops mid-Cue and redispatches is exactly the
/// duplicate the in-session set cannot see.
#[test]
fn the_same_cue_id_on_a_restarted_session_does_not_enqueue_a_second_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (identity, registry) = identity_and_registry(dir.path());
    let cue_id = "0123456789abcdef0123456789abcdef";

    let first = cue_session(
        &registry,
        &identity,
        [7u8; 32],
        workspace_with_declared_script(dir.path()),
    );
    let enqueued = first
        .enqueue_accepted(cue_id, "deploy.sh", "first delivery", "authorized-hash")
        .expect("the first delivery of a cue id must enqueue its run");
    assert_eq!(
        enqueued,
        derive_run_id(cue_id),
        "the run id must be derived from the cue id, or the primary key guards nothing"
    );
    drop(first);
    // Close and reopen both node-owned state handles, as a service restart
    // does. The run database is reopened by the enqueue operation itself.
    drop(registry);
    drop(identity);
    let (identity, registry) = identity_and_registry(dir.path());

    // A different session id, so `seen_cue_ids` is empty and cannot be the
    // thing that refuses this.
    let second = cue_session(
        &registry,
        &identity,
        [11u8; 32],
        crate::workspace::Workspace::new(dir.path().join("workspace")),
    );
    assert_eq!(
        second.enqueue_accepted(cue_id, "deploy.sh", "redelivery", "authorized-hash"),
        Err(CueEnqueueError::Duplicate),
        "a redelivered cue id must be refused as a duplicate, not as a script fault"
    );

    let workspace = crate::workspace::Workspace::new(dir.path().join("workspace"));
    let runs = crate::operations::core::list_runs(
        &workspace,
        crate::operations::core::ListRunsRequest {
            script: None,
            actor: None,
            since_ms: None,
            until_ms: None,
            success: None,
            limit: None,
            states: Vec::new(),
            // Every state: the run is still queued, and a filter that
            // happened to exclude it would make the count below pass by
            // finding nothing rather than by finding one.
            state_set: Some("all".to_string()),
        },
    )
    .expect("list the runs the workspace holds");
    let ids: Vec<&str> = runs.iter().map(|run| run.run_id.as_str()).collect();
    assert_eq!(
        ids,
        vec![enqueued.as_str()],
        "two deliveries of one cue id must leave exactly one run"
    );
}

#[test]
fn the_same_cue_id_on_one_session_is_a_duplicate_after_the_first_enqueue() {
    let dir = tempfile::tempdir().unwrap();
    let (identity, registry) = identity_and_registry(dir.path());
    let session = cue_session(
        &registry,
        &identity,
        [7u8; 32],
        workspace_with_declared_script(dir.path()),
    );
    let cue_id = "0123456789abcdef0123456789abcdef";

    session
        .enqueue_accepted(cue_id, "deploy.sh", "first delivery", "authorized-hash")
        .expect("the first delivery must enqueue");
    assert_eq!(
        session.enqueue_accepted(cue_id, "deploy.sh", "retry", "authorized-hash"),
        Err(CueEnqueueError::Duplicate)
    );
}

#[test]
fn a_unique_cue_id_creates_a_distinct_durable_run() {
    let dir = tempfile::tempdir().unwrap();
    let (identity, registry) = identity_and_registry(dir.path());
    let session = cue_session(
        &registry,
        &identity,
        [7u8; 32],
        workspace_with_declared_script(dir.path()),
    );

    let first = session
        .enqueue_accepted(
            "0123456789abcdef0123456789abcdef",
            "deploy.sh",
            "first",
            "authorized-hash",
        )
        .expect("first Cue must create a run");
    let second = session
        .enqueue_accepted(
            "fedcba9876543210fedcba9876543210",
            "deploy.sh",
            "second",
            "authorized-hash",
        )
        .expect("a distinct Cue must create a distinct run");

    assert_ne!(first, second);
    let workspace = crate::workspace::Workspace::new(dir.path().join("workspace"));
    let runs = crate::operations::core::list_runs(
        &workspace,
        crate::operations::core::ListRunsRequest {
            state_set: Some("all".to_string()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(runs.len(), 2, "distinct Cue ids must create two runs");
    assert!(
        runs.iter()
            .all(|run| run.trigger == crate::runs::RunTrigger::Cue)
    );
}

#[test]
fn a_non_duplicate_enqueue_failure_keeps_its_stable_operation_error() {
    let dir = tempfile::tempdir().unwrap();
    let (identity, registry) = identity_and_registry(dir.path());
    let workspace = workspace_with_declared_script(dir.path());

    // Make opening runs.sqlite fail without changing the script or the
    // authorization inputs. This is a local storage fault, not a run-id
    // conflict.
    std::fs::remove_dir_all(workspace.history_dir()).unwrap();
    std::fs::write(workspace.history_dir(), "not a directory").unwrap();

    let session = cue_session(&registry, &identity, [7u8; 32], workspace);
    assert_eq!(
        session.enqueue_accepted(
            "0123456789abcdef0123456789abcdef",
            "deploy.sh",
            "storage fault",
            "authorized-hash",
        ),
        Err(CueEnqueueError::Failed(
            crate::operations::OperationErrorCode::IoFailed
        )),
        "a storage failure must not be reported as a duplicate"
    );
}

#[test]
fn only_the_run_id_uniqueness_error_is_a_duplicate() {
    let run_id_conflict = crate::operations::OperationError::new(
        crate::operations::OperationErrorCode::IoFailed,
        "Insert run failed: UNIQUE constraint failed: runs.run_id",
    );
    assert_eq!(
        classify_enqueue_error(run_id_conflict),
        CueEnqueueError::Duplicate
    );

    let unrelated_conflict = crate::operations::OperationError::new(
        crate::operations::OperationErrorCode::IoFailed,
        "Insert run failed: UNIQUE constraint failed: run_script_hashes.run_id",
    );
    assert_eq!(
        classify_enqueue_error(unrelated_conflict),
        CueEnqueueError::Failed(crate::operations::OperationErrorCode::IoFailed)
    );
}

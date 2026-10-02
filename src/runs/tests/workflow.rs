use super::super::workflow::WorkflowState;
use super::*;

fn snapshot() -> WorkflowSnapshot {
    WorkflowSnapshot {
        battery_id: "maintenance".into(),
        battery_version: "1.2.3".into(),
        battery_commit: "0123456789abcdef".into(),
        workflow_name: "update".into(),
        steps: vec![
            WorkflowStepSnapshot {
                name: "prepare".into(),
                script_path: "maintenance/prepare.sh".into(),
                content_hash: "first-hash".into(),
            },
            WorkflowStepSnapshot {
                name: "install".into(),
                script_path: "maintenance/install.sh".into(),
                content_hash: "second-hash".into(),
            },
        ],
    }
}

#[test]
fn workflow_advances_exactly_once_and_preserves_approved_hashes() {
    let conn = Connection::open_in_memory().unwrap();
    init_schema(&conn).unwrap();
    let started = start_workflow(&conn, snapshot(), "human").unwrap();
    assert_eq!(started.state, WorkflowState::Running);
    assert_eq!(started.battery_version, "1.2.3");
    assert_eq!(started.battery_commit, "0123456789abcdef");
    assert_eq!(started.steps.len(), 2);
    assert_eq!(started.steps[1].run_id, None);
    let first_id = started.steps[0].run_id.clone().unwrap();
    assert_eq!(
        get_run_script_hash(&conn, &first_id).unwrap().as_deref(),
        Some("first-hash")
    );
    assert_eq!(
        get_run_secret_refs(&conn, &first_id).unwrap(),
        Some(Vec::new())
    );
    assert_eq!(
        get_run(&conn, &first_id).unwrap().unwrap().trigger,
        RunTrigger::Workflow
    );

    let claimed = claim_next(&conn, "worker", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    assert_eq!(claimed.run_id, first_id);
    complete(&conn, &first_id, ok_completion()).unwrap();
    let advanced = advance_workflow_for_run(&conn, &first_id).unwrap().unwrap();
    let second_id = advanced.steps[1].run_id.clone().unwrap();
    assert_ne!(first_id, second_id);
    assert_eq!(advanced.current_step, 1);
    assert_eq!(
        get_run_script_hash(&conn, &second_id).unwrap().as_deref(),
        Some("second-hash")
    );
    assert_eq!(
        get_run(&conn, &second_id)
            .unwrap()
            .unwrap()
            .parent_run_id
            .as_deref(),
        Some(first_id.as_str())
    );

    let repeated = advance_workflow_for_run(&conn, &first_id).unwrap().unwrap();
    assert_eq!(
        repeated.steps[1].run_id.as_deref(),
        Some(second_id.as_str())
    );
    assert_eq!(recover_workflows(&conn).unwrap().len(), 0);

    claim_next(&conn, "worker", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    complete(&conn, &second_id, ok_completion()).unwrap();
    let done = recover_workflows(&conn).unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].state, WorkflowState::Completed);
    assert!(done[0].finished_at.is_some());
    assert!(recover_workflows(&conn).unwrap().is_empty());
}

#[test]
fn workflow_stops_after_failure_and_cannot_queue_later_step() {
    let conn = Connection::open_in_memory().unwrap();
    init_schema(&conn).unwrap();
    let started = start_workflow(&conn, snapshot(), "human").unwrap();
    let first_id = started.steps[0].run_id.clone().unwrap();
    claim_next(&conn, "worker", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    fail(&conn, &first_id, fail_completion()).unwrap();
    let stopped = recover_workflows(&conn).unwrap();
    assert_eq!(stopped[0].state, WorkflowState::Failed);
    assert_eq!(stopped[0].steps[1].run_id, None);
    assert!(
        advance_workflow_for_run(&conn, &first_id)
            .unwrap()
            .unwrap()
            .steps[1]
            .run_id
            .is_none()
    );
}

#[test]
fn invalid_workflow_is_not_partially_persisted() {
    let conn = Connection::open_in_memory().unwrap();
    init_schema(&conn).unwrap();
    let mut invalid = snapshot();
    invalid.steps[1].content_hash.clear();
    assert!(start_workflow(&conn, invalid, "human").is_err());
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM workflow_runs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn workflow_start_rolls_back_when_approved_hash_cannot_be_stored() {
    let conn = Connection::open_in_memory().unwrap();
    init_schema(&conn).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_workflow_hash BEFORE INSERT ON run_script_hashes
         BEGIN SELECT RAISE(ABORT, 'hash write rejected'); END;",
    )
    .unwrap();

    assert!(start_workflow(&conn, snapshot(), "human").is_err());
    for table in ["workflow_runs", "workflow_steps", "runs"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table} must roll back with the hash write");
    }
}

#[test]
fn workflow_resumes_after_database_reopen_without_repeating_completed_step() {
    let workspace = scratch_workspace("workflow_restart");
    let conn = open(&workspace).unwrap();
    let started = start_workflow(&conn, snapshot(), "human").unwrap();
    let first_id = started.steps[0].run_id.clone().unwrap();
    claim_next(&conn, "worker", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    complete(&conn, &first_id, ok_completion()).unwrap();
    drop(conn);

    let reopened = open(&workspace).unwrap();
    let recovered = recover_workflows(&reopened).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].current_step, 1);
    assert_eq!(
        recovered[0].steps[0].run_id.as_deref(),
        Some(first_id.as_str())
    );
    let second_id = recovered[0].steps[1].run_id.clone().unwrap();
    assert!(recover_workflows(&reopened).unwrap().is_empty());
    let count: i64 = reopened
        .query_row(
            "SELECT COUNT(*) FROM runs WHERE run_id IN (?1, ?2)",
            params![first_id, second_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
    drop(reopened);
    fs::remove_dir_all(workspace.root()).unwrap();
}

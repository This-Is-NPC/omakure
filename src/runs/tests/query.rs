use super::*;

// -----------------------------------------------------------------
// Filters
// -----------------------------------------------------------------

#[test]
fn run_filters_default_returns_terminal_only() {
    let ws = scratch_workspace("filters_default");
    let conn = open(&ws).expect("open");
    // Enqueue two rows; claim the FIRST and complete it. The second
    // stays queued, so RunFilters::default() (terminal-only) returns
    // exactly the completed one.
    let _other = enqueue(&conn, "/x/other.sh", &[], enqueue_opts()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let _later = enqueue(&conn, "/x/later.sh", &[], enqueue_opts()).unwrap();
    let claimed = claim_next(&conn, "w", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    complete(&conn, &claimed.run_id, ok_completion()).unwrap();
    let rows = query_runs(&conn, &RunFilters::default()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].run_id, claimed.run_id);
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn run_filters_all_returns_every_state() {
    let ws = scratch_workspace("filters_all");
    let conn = open(&ws).expect("open");
    enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    enqueue(&conn, "/x/b.sh", &[], enqueue_opts()).unwrap();
    let claimed = claim_next(&conn, "w", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    complete(&conn, &claimed.run_id, ok_completion()).unwrap();
    let filters = RunFilters {
        states: RunStateSet::All.to_states(),
        ..Default::default()
    };
    let rows = query_runs(&conn, &filters).unwrap();
    assert_eq!(rows.len(), 2);
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn run_filters_state_specific_running() {
    let ws = scratch_workspace("filters_running");
    let conn = open(&ws).expect("open");
    enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    enqueue(&conn, "/x/b.sh", &[], enqueue_opts()).unwrap();
    claim_next(&conn, "w", &ClaimFilters::default()).unwrap();
    let filters = RunFilters {
        states: vec![RunState::Running],
        ..Default::default()
    };
    let rows = query_runs(&conn, &filters).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, RunState::Running);
    let _ = fs::remove_dir_all(ws.root());
}

// -----------------------------------------------------------------
// Stats
// -----------------------------------------------------------------

#[test]
fn scheduled_queries_cover_missing_overlap_completion_errors_and_concurrency() {
    let ws = scratch_workspace("scheduled_queries");
    let conn = open(&ws).expect("open");
    let schedule_id = "/scripts/job.sh@*/5 * * * * *";

    assert_eq!(
        last_scheduled_fire_ms(&conn, schedule_id).unwrap(),
        None,
        "a schedule with no rows has no last fire"
    );
    assert!(
        !has_live_scheduled_run(&conn, schedule_id).unwrap(),
        "a schedule with no rows does not overlap"
    );

    let row = enqueue(
        &conn,
        "/scripts/job.sh",
        &[],
        EnqueueOptions {
            run_id: Some("scheduled-query-row".into()),
            cron_schedule_id: Some(schedule_id.into()),
            trigger: RunTrigger::Scheduled,
            ..enqueue_opts()
        },
    )
    .unwrap();
    assert_eq!(
        last_scheduled_fire_ms(&conn, schedule_id).unwrap(),
        Some(row.enqueued_at)
    );
    assert!(has_live_scheduled_run(&conn, schedule_id).unwrap());

    let db_path = runs_db_path(&ws);
    let mut readers = Vec::new();
    for _ in 0..3 {
        let path = db_path.clone();
        let schedule_id = schedule_id.to_string();
        readers.push(std::thread::spawn(move || {
            let conn = open_connection(&path).expect("open reader");
            (
                last_scheduled_fire_ms(&conn, &schedule_id).unwrap(),
                has_live_scheduled_run(&conn, &schedule_id).unwrap(),
            )
        }));
    }
    for reader in readers {
        assert_eq!(
            reader.join().unwrap(),
            (Some(row.enqueued_at), true),
            "concurrent readers observe one scheduler state"
        );
    }

    claim_next(&conn, "scheduler-worker", &ClaimFilters::default())
        .unwrap()
        .expect("scheduled row is claimable");
    assert!(has_live_scheduled_run(&conn, schedule_id).unwrap());
    complete(&conn, &row.run_id, ok_completion()).unwrap();
    assert!(
        !has_live_scheduled_run(&conn, schedule_id).unwrap(),
        "terminal rows do not block the next fire"
    );

    let broken = Connection::open_in_memory().expect("in-memory connection");
    let last_error = last_scheduled_fire_ms(&broken, schedule_id).unwrap_err();
    assert!(last_error
        .to_string()
        .contains("Query last scheduled fire failed"));
    let live_error = has_live_scheduled_run(&broken, schedule_id).unwrap_err();
    assert!(live_error
        .to_string()
        .contains("Query live scheduled run failed"));

    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn stats_counts_per_state_and_actor() {
    let ws = scratch_workspace("stats");
    let conn = open(&ws).expect("open");
    enqueue(
        &conn,
        "/x/a.sh",
        &[],
        EnqueueOptions {
            actor: "ai".into(),
            ..enqueue_opts()
        },
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    enqueue(
        &conn,
        "/x/b.sh",
        &[],
        EnqueueOptions {
            actor: "ai".into(),
            ..enqueue_opts()
        },
    )
    .unwrap();
    let claimed = claim_next(&conn, "w", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    complete(&conn, &claimed.run_id, ok_completion()).unwrap();

    let s = stats(&conn).unwrap();
    assert_eq!(s.total, 2);
    assert_eq!(s.counts_by_state.get("queued").copied(), Some(1));
    assert_eq!(s.counts_by_state.get("completed").copied(), Some(1));
    // Every legal state must be present (zero when absent).
    assert_eq!(s.counts_by_state.get("dead_letter").copied(), Some(0));
    assert_eq!(s.counts_by_actor.get("ai").copied(), Some(2));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn get_run_returns_none_for_unknown_id() {
    let ws = scratch_workspace("unknown_id");
    let conn = open(&ws).expect("open");
    assert!(get_run(&conn, "missing").unwrap().is_none());
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn query_runs_applies_all_filters() {
    let ws = scratch_workspace("query_filters");
    let conn = open(&ws).expect("open");
    let now = unix_millis();

    let r1 = enqueue(&conn, "/scripts/alpha.sh", &[], enqueue_opts()).unwrap();
    let r2 = enqueue(
        &conn,
        "/scripts/beta.sh",
        &[],
        EnqueueOptions {
            actor: "ai".into(),
            omakure_version: "test".into(),
            ..Default::default()
        },
    )
    .unwrap();
    let claim = ClaimFilters::default();
    claim_next(&conn, "w1", &claim).unwrap();
    complete(&conn, &r1.run_id, ok_completion()).unwrap();
    claim_next(&conn, "w1", &claim).unwrap();
    fail(&conn, &r2.run_id, fail_completion()).unwrap();

    let by_script = query_runs(
        &conn,
        &RunFilters {
            script: Some("alpha".into()),
            states: RunStateSet::All.to_states(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(by_script.iter().any(|r| r.script_path.contains("alpha")));
    assert!(!by_script.iter().any(|r| r.script_path.contains("beta")));

    let by_actor = query_runs(
        &conn,
        &RunFilters {
            actor: Some("ai".into()),
            states: RunStateSet::All.to_states(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(by_actor.iter().all(|r| r.actor == "ai"));

    let recent = query_runs(
        &conn,
        &RunFilters {
            since_ms: Some(now - 60_000),
            until_ms: Some(now + 60_000),
            states: RunStateSet::All.to_states(),
            limit: Some(10),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!recent.is_empty());

    let only_success = query_runs(
        &conn,
        &RunFilters {
            success: Some(true),
            states: RunStateSet::All.to_states(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(only_success.iter().all(|r| r.success == Some(true)));

    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn row_to_run_rejects_invalid_state_string() {
    let ws = scratch_workspace("invalid_state_row");
    let conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/scripts/x.sh", &[], enqueue_opts()).unwrap();
    // Tamper with the state column to a value RunState::from_str rejects.
    conn.execute(
        "UPDATE runs SET state = 'not_a_state' WHERE run_id = ?",
        params![&row.run_id],
    )
    .unwrap();

    let err = query_runs(
        &conn,
        &RunFilters {
            states: vec![],
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("invalid run state")
            || err.to_string().contains("Row query_runs failed")
    );

    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn missing_run_table_preserves_query_error_sources_and_text() {
    let conn = Connection::open_in_memory().unwrap();
    let cases = [
        (
            get_run(&conn, "missing").unwrap_err(),
            "Prepare get_run failed",
        ),
        (
            last_scheduled_fire_ms(&conn, "schedule").unwrap_err(),
            "Query last scheduled fire failed",
        ),
        (
            query_runs(&conn, &RunFilters::default()).unwrap_err(),
            "Prepare query_runs failed",
        ),
        (stats(&conn).unwrap_err(), "Prepare state stats failed"),
    ];
    for (error, operation) in cases {
        match &error {
            RunsError::Sqlite {
                operation: actual,
                source,
            } => {
                assert_eq!(*actual, operation);
                assert_eq!(source.to_string(), "no such table: runs");
            }
            other => panic!("expected SQLite error, got {other:?}"),
        }
        assert_eq!(
            error.to_string(),
            format!("{operation}: no such table: runs")
        );
    }
}

use super::*;

// -----------------------------------------------------------------
// Schema
// -----------------------------------------------------------------

#[test]
fn open_creates_db_with_state_column() {
    let ws = scratch_workspace("open_creates");
    let conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    let loaded = get_run(&conn, &row.run_id).unwrap().unwrap();
    assert_eq!(loaded.state, RunState::Queued);
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn enqueue_rolls_back_row_when_metadata_write_fails() {
    let ws = scratch_workspace("enqueue_metadata_atomic");
    let conn = open(&ws).expect("open");
    conn.execute_batch(
        "CREATE TRIGGER reject_run_secret_refs
             BEFORE INSERT ON run_secret_refs
             BEGIN
                 SELECT RAISE(ABORT, 'injected metadata failure');
             END;",
    )
    .expect("install metadata failure trigger");
    let mut opts = enqueue_opts();
    opts.allowed_secret_refs = Some(vec!["secret://env/TOKEN".into()]);
    let error = enqueue(&conn, "/x/atomic.sh", &[], opts).unwrap_err();
    assert!(error.contains("injected metadata failure"));
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM runs WHERE script_path = '/x/atomic.sh'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "failed metadata must roll back the run row");
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn open_idempotent_on_new_schema() {
    let ws = scratch_workspace("open_idempotent");
    let _ = open(&ws).expect("first open");
    let conn = open(&ws).expect("second open");
    let rows = query_runs(&conn, &RunFilters::default()).unwrap();
    assert!(rows.is_empty());
    let _ = fs::remove_dir_all(ws.root());
}

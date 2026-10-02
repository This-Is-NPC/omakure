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
    assert!(matches!(
        &error,
        RunsError::Sqlite {
            operation: "Set run secret ref failed",
            ..
        }
    ));
    assert_eq!(
        error.to_string(),
        "Set run secret ref failed: injected metadata failure"
    );
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

#[test]
fn blocked_history_directory_keeps_filesystem_source_and_text() {
    let ws = scratch_workspace("blocked_history");
    fs::remove_dir_all(ws.history_dir()).unwrap();
    fs::write(ws.history_dir(), "blocking file").unwrap();
    let error = open(&ws).unwrap_err();
    match &error {
        RunsError::Filesystem { operation, source } => {
            assert_eq!(*operation, "Create history dir failed");
            assert_eq!(source.kind(), std::io::ErrorKind::AlreadyExists);
            assert_eq!(error.to_string(), format!("{operation}: {source}"));
        }
        other => panic!("expected filesystem error, got {other:?}"),
    }
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn database_open_and_schema_errors_retain_sqlite_sources() {
    let dir = tempfile::TempDir::new().unwrap();
    let error = open_connection(dir.path()).unwrap_err();
    match &error {
        RunsError::DatabaseOpen(crate::util::sqlite::WalOpenError::Sqlite {
            operation,
            source,
        }) => {
            assert_eq!(operation, "Open runs db failed");
            assert_eq!(error.to_string(), format!("{operation}: {source}"));
        }
        other => panic!("expected database open error, got {other:?}"),
    }

    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE runs (run_id TEXT PRIMARY KEY)")
        .unwrap();
    let error = init_schema(&conn).unwrap_err();
    match &error {
        RunsError::Sqlite { operation, source } => {
            assert_eq!(*operation, "Init runs db failed");
            assert_eq!(error.to_string(), format!("{operation}: {source}"));
        }
        other => panic!("expected schema error, got {other:?}"),
    }
}

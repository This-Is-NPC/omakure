use super::*;

#[test]
fn enqueue_input_errors_have_typed_variants_and_existing_text() {
    let conn = Connection::open_in_memory().unwrap();
    let scheduled = EnqueueOptions {
        trigger: RunTrigger::Manual,
        ..enqueue_opts()
    };
    let error = enqueue_scheduled(&conn, "/x/a.sh", &[], scheduled).unwrap_err();
    assert!(matches!(error, RunsError::InvalidEnqueue(_)));
    assert_eq!(
        error.to_string(),
        "Scheduled enqueue requires RunTrigger::Scheduled"
    );

    let scheduled = EnqueueOptions {
        trigger: RunTrigger::Scheduled,
        ..enqueue_opts()
    };
    let error = enqueue_scheduled(&conn, "/x/a.sh", &[], scheduled).unwrap_err();
    assert!(matches!(error, RunsError::InvalidEnqueue(_)));
    assert_eq!(
        error.to_string(),
        "Scheduled enqueue requires cron_schedule_id"
    );

    let cue = EnqueueOptions {
        trigger: RunTrigger::Manual,
        ..enqueue_opts()
    };
    let mut conn = conn;
    let error = enqueue_cue(&mut conn, "/x/a.sh", &[], cue).unwrap_err();
    assert!(matches!(error, RunsError::InvalidEnqueue(_)));
    assert_eq!(error.to_string(), "Cue enqueue requires RunTrigger::Cue");
}

#[test]
fn missing_run_table_retains_sqlite_source_and_operation_text() {
    let conn = Connection::open_in_memory().unwrap();
    let error = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap_err();
    match &error {
        RunsError::Sqlite { operation, source } => {
            assert_eq!(*operation, "Insert run failed");
            assert_eq!(source.to_string(), "no such table: runs");
        }
        other => panic!("expected SQLite error, got {other:?}"),
    }
    assert_eq!(error.to_string(), "Insert run failed: no such table: runs");
}

use super::*;

// -----------------------------------------------------------------
// Trace storage
// -----------------------------------------------------------------

#[test]
fn insert_trace_assigns_monotonic_sequence() {
    let ws = scratch_workspace("trace_sequence");
    let mut conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    let t1 = insert_trace(&mut conn, &row.run_id, TraceLevel::Info, "first", None).unwrap();
    let t2 = insert_trace(&mut conn, &row.run_id, TraceLevel::Warn, "second", None).unwrap();
    assert_eq!(t1.sequence, 1);
    assert_eq!(t2.sequence, 2);
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn insert_trace_unknown_run_returns_not_found() {
    let ws = scratch_workspace("trace_unknown");
    let mut conn = open(&ws).expect("open");
    let err = insert_trace(&mut conn, "missing", TraceLevel::Info, "x", None).unwrap_err();
    assert!(matches!(err, RunsError::NotFound(ref id) if id == "missing"));
    assert_eq!(err.to_string(), "not_found: missing");
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn insert_trace_under_concurrency_no_duplicates() {
    let ws = scratch_workspace("trace_concurrent");
    let conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    drop(conn);
    let db_path = runs_db_path(&ws);
    let mut handles = Vec::new();
    // Modest concurrency: we want to prove the monotonic-sequence
    // contract under contention, not stress SQLite's writer queue.
    const PER_THREAD: usize = 10;
    const THREADS: usize = 3;
    for _ in 0..THREADS {
        let path = db_path.clone();
        let run_id = row.run_id.clone();
        handles.push(std::thread::spawn(move || {
            let mut conn = open_connection(&path).expect("open per-thread");
            conn.busy_timeout(std::time::Duration::from_secs(15))
                .unwrap();
            for i in 0..PER_THREAD {
                insert_trace(
                    &mut conn,
                    &run_id,
                    TraceLevel::Info,
                    &format!("event {}", i),
                    None,
                )
                .unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let conn = open_connection(&db_path).unwrap();
    let traces = query_traces(&conn, &row.run_id, None, None).unwrap();
    assert_eq!(traces.len(), THREADS * PER_THREAD);
    let mut seqs: Vec<i64> = traces.iter().map(|t| t.sequence).collect();
    seqs.sort();
    let mut deduped = seqs.clone();
    deduped.dedup();
    assert_eq!(seqs.len(), deduped.len(), "sequences must be unique");
    assert_eq!(*seqs.first().unwrap(), 1);
    assert_eq!(*seqs.last().unwrap(), (THREADS * PER_THREAD) as i64);
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn insert_trace_retries_after_busy_timeout_without_duplicates() {
    let ws = scratch_workspace("trace_busy_retry");
    let mut conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    let db_path = runs_db_path(&ws);
    let ready = std::sync::Arc::new(std::sync::Barrier::new(2));
    let lock_ready = ready.clone();
    let locker = std::thread::spawn(move || {
        let locker = open_connection(&db_path).expect("open locker");
        locker.execute_batch("BEGIN IMMEDIATE").unwrap();
        lock_ready.wait();
        std::thread::sleep(Duration::from_millis(2_250));
        locker.execute_batch("ROLLBACK").unwrap();
    });

    ready.wait();
    insert_trace(&mut conn, &row.run_id, TraceLevel::Info, "after lock", None).unwrap();
    locker.join().unwrap();

    let traces = query_traces(&conn, &row.run_id, None, None).unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].sequence, 1);
    assert_eq!(traces[0].message, "after lock");
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn query_traces_filters_by_level_min() {
    let ws = scratch_workspace("trace_level");
    let mut conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    insert_trace(&mut conn, &row.run_id, TraceLevel::Debug, "d", None).unwrap();
    insert_trace(&mut conn, &row.run_id, TraceLevel::Info, "i", None).unwrap();
    insert_trace(&mut conn, &row.run_id, TraceLevel::Warn, "w", None).unwrap();
    insert_trace(&mut conn, &row.run_id, TraceLevel::Error, "e", None).unwrap();

    let warn_and_above = query_traces(&conn, &row.run_id, Some(TraceLevel::Warn), None).unwrap();
    let levels: Vec<&str> = warn_and_above.iter().map(|t| t.level.as_str()).collect();
    assert_eq!(levels, vec!["warn", "error"]);
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn query_traces_filters_by_since_sequence() {
    let ws = scratch_workspace("trace_since");
    let mut conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    for i in 0..5 {
        insert_trace(
            &mut conn,
            &row.run_id,
            TraceLevel::Info,
            &format!("e{}", i),
            None,
        )
        .unwrap();
    }
    let since = query_traces(&conn, &row.run_id, None, Some(2)).unwrap();
    let seqs: Vec<i64> = since.iter().map(|t| t.sequence).collect();
    assert_eq!(seqs, vec![3, 4, 5]);
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn query_traces_unknown_run_returns_not_found() {
    let ws = scratch_workspace("trace_q_unknown");
    let conn = open(&ws).expect("open");
    let err = query_traces(&conn, "missing", None, None).unwrap_err();
    assert!(matches!(err, RunsError::NotFound(ref id) if id == "missing"));
    assert_eq!(err.to_string(), "not_found: missing");
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn delete_run_cascades_to_traces() {
    let ws = scratch_workspace("trace_cascade");
    let mut conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    insert_trace(&mut conn, &row.run_id, TraceLevel::Info, "x", None).unwrap();
    conn.execute("DELETE FROM runs WHERE run_id = ?", params![row.run_id])
        .unwrap();
    // The traces table must be empty after the cascade.
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM run_traces", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    let _ = fs::remove_dir_all(ws.root());
}

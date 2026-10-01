use super::*;

// -----------------------------------------------------------------
// Transitions
// -----------------------------------------------------------------

#[test]
fn enqueue_then_claim_then_complete_happy_path() {
    let ws = scratch_workspace("happy_path");
    let mut conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &["--foo".into()], enqueue_opts()).unwrap();
    assert_eq!(row.state, RunState::Queued);
    assert!(row.started_at.is_none());

    let claimed = claim_next(&conn, "worker-1", &ClaimFilters::default())
        .unwrap()
        .expect("claimed");
    assert_eq!(claimed.run_id, row.run_id);
    assert_eq!(claimed.state, RunState::Running);
    assert!(claimed.started_at.is_some());
    assert_eq!(claimed.worker_id.as_deref(), Some("worker-1"));

    complete(&conn, &claimed.run_id, ok_completion()).unwrap();
    let loaded = get_run(&conn, &row.run_id).unwrap().unwrap();
    assert_eq!(loaded.state, RunState::Completed);
    assert_eq!(loaded.success, Some(true));
    assert_eq!(loaded.exit_code, Some(0));

    // Suppress unused warning for `mut conn` (insert_trace path uses it
    // elsewhere).
    let _ = &mut conn;
    let _ = fs::remove_dir_all(ws.root());
}

/// The blocker this wave exists to close.
///
/// A queued job whose worker died should be re-run: nobody saw a result. A
/// remote instruction must not be, because the side effect may already have
/// happened and the caller cannot tell it happened twice.
///
/// Revoking a peer reaches the work it already caused, and stops there.
///
/// The second half is the one worth having: a peer's node id can appear on
/// locally-initiated work too, and revoking trust in a peer is not a
/// licence to cancel what this node's owner started.
#[test]
fn revoking_a_peer_cancels_its_cue_runs_and_nothing_else() {
    let ws = scratch_workspace("cue_revocation_cancels");
    let conn = open(&ws).expect("open");
    let peer = "omk1_peer";

    let queued = enqueue(
        &conn,
        "/x/deploy.sh",
        &[],
        EnqueueOptions {
            trigger: RunTrigger::Cue,
            actor: peer.into(),
            ..enqueue_opts()
        },
    )
    .unwrap();
    let running = enqueue(
        &conn,
        "/x/deploy.sh",
        &[],
        EnqueueOptions {
            trigger: RunTrigger::Cue,
            actor: peer.into(),
            ..enqueue_opts()
        },
    )
    .unwrap();
    // Local work carrying the same actor name, and a Cue from someone else.
    let local = enqueue(
        &conn,
        "/x/deploy.sh",
        &[],
        EnqueueOptions {
            actor: peer.into(),
            ..enqueue_opts()
        },
    )
    .unwrap();
    let other_peer = enqueue(
        &conn,
        "/x/deploy.sh",
        &[],
        EnqueueOptions {
            trigger: RunTrigger::Cue,
            actor: "omk1_other".into(),
            ..enqueue_opts()
        },
    )
    .unwrap();
    conn.execute(
        "UPDATE runs SET state = 'running', started_at = ?1 WHERE run_id = ?2",
        rusqlite::params![unix_millis(), running.run_id],
    )
    .unwrap();

    let cancelled = cancel_cue_runs_for_actor(&conn, peer).unwrap();
    assert_eq!(
        cancelled.len(),
        2,
        "both the queued and the running Cue run must be cancelled"
    );

    for run_id in [&queued.run_id, &running.run_id] {
        assert_eq!(
            get_run(&conn, run_id).unwrap().unwrap().state,
            RunState::Cancelled,
            "a revoked peer's in-flight Cue must not survive the revocation"
        );
    }
    for run_id in [&local.run_id, &other_peer.run_id] {
        assert_eq!(
            get_run(&conn, run_id).unwrap().unwrap().state,
            RunState::Queued,
            "revocation must not reach work it was not asked to stop"
        );
    }
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn cue_revocation_failure_is_atomic_and_is_not_reported_as_cleanup_success() {
    let ws = scratch_workspace("cue_revocation_fault");
    let conn = open(&ws).expect("open");
    let peer = "omk1_peer";
    let running = enqueue(
        &conn,
        "/x/running.sh",
        &[],
        EnqueueOptions {
            actor: peer.into(),
            trigger: RunTrigger::Cue,
            ..enqueue_opts()
        },
    )
    .unwrap();
    let queued = enqueue(
        &conn,
        "/x/queued.sh",
        &[],
        EnqueueOptions {
            actor: peer.into(),
            trigger: RunTrigger::Cue,
            ..enqueue_opts()
        },
    )
    .unwrap();
    claim_next(&conn, "worker", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER cue_cancel_fault
             BEFORE UPDATE OF state ON runs
             WHEN NEW.trigger = 'Cue' AND NEW.state = 'cancelled'
             BEGIN SELECT RAISE(ABORT, 'injected Cue cancellation failure'); END",
    )
    .unwrap();

    let error = cancel_cue_runs_for_actor(&conn, peer).unwrap_err();
    assert!(matches!(
        &error,
        RunsError::Sqlite {
            operation: "Read cancelled Cue runs failed",
            ..
        }
    ));
    assert!(error
        .to_string()
        .contains("injected Cue cancellation failure"));
    assert_eq!(
        get_run(&conn, &running.run_id).unwrap().unwrap().state,
        RunState::Running
    );
    assert_eq!(
        get_run(&conn, &queued.run_id).unwrap().unwrap().state,
        RunState::Queued
    );
    let _ = fs::remove_dir_all(ws.root());
}

/// Without the exclusion this test fails by *succeeding* — `claim_next`
/// hands the row back and the script runs a second time.
#[test]
fn a_crashed_cue_run_is_never_reclaimed_by_a_worker() {
    let ws = scratch_workspace("cue_no_lease_steal");
    let conn = open(&ws).expect("open");

    let row = enqueue(
        &conn,
        "/x/deploy.sh",
        &[],
        EnqueueOptions {
            trigger: RunTrigger::Cue,
            ..enqueue_opts()
        },
    )
    .unwrap();
    claim_next(&conn, "w", &ClaimFilters::default())
        .unwrap()
        .expect("the first claim runs it once");

    // The worker dies. Its lease lapses.
    conn.execute(
        "UPDATE runs SET lease_until = ?1 WHERE run_id = ?2",
        rusqlite::params![unix_millis() - HEARTBEAT_MS - 1, row.run_id],
    )
    .unwrap();

    assert!(
        claim_next(&conn, "w2", &ClaimFilters::default())
            .unwrap()
            .is_none(),
        "a lapsed Cue-origin lease must not be stolen; the script would run twice"
    );
    let _ = fs::remove_dir_all(ws.root());
}

/// The full blocker scenario the plan named, end to end.
///
/// A Cue runs, the worker dies mid-flight, the databases are closed and
/// reopened to model a real restart, and recovery runs. The row must end
/// terminal and the side effect must have happened exactly once.
///
/// The side effect is counted with a file the "script" appends to, so the
/// assertion is about observable work rather than about row states agreeing
/// with each other.
#[test]
fn a_crashed_cue_run_recovers_terminal_with_its_effect_seen_exactly_once() {
    let ws = scratch_workspace("cue_recovery");
    let effects = ws.root().join("effects.log");

    // One "execution": the claim, then the side effect.
    {
        let conn = open(&ws).expect("open");
        let row = enqueue(
            &conn,
            "/x/deploy.sh",
            &[],
            EnqueueOptions {
                run_id: Some("run-from-cue".into()),
                trigger: RunTrigger::Cue,
                ..enqueue_opts()
            },
        )
        .unwrap();
        claim_next(&conn, "w", &ClaimFilters::default())
            .unwrap()
            .expect("claimed once");
        fs::write(&effects, "ran\n").unwrap();

        // The worker dies holding the lease.
        conn.execute(
            "UPDATE runs SET lease_until = ?1 WHERE run_id = ?2",
            rusqlite::params![unix_millis() - HEARTBEAT_MS - 1, row.run_id],
        )
        .unwrap();
    }

    // Restart: everything reopened from disk.
    let conn = open(&ws).expect("reopen");

    assert!(
        claim_next(&conn, "w2", &ClaimFilters::default())
            .unwrap()
            .is_none(),
        "a restarted worker must not pick the run up again"
    );

    let recovered = recover_abandoned_cue_runs(&conn).unwrap();
    assert_eq!(recovered, vec!["run-from-cue".to_string()]);

    let loaded = get_run(&conn, "run-from-cue").unwrap().unwrap();
    assert_eq!(
        loaded.state,
        RunState::Failed,
        "the row must reach a terminal state or the Conductor waits forever"
    );
    assert!(loaded.error.unwrap_or_default().contains("at most once"));

    assert!(
        claim_next(&conn, "w3", &ClaimFilters::default())
            .unwrap()
            .is_none(),
        "recovery must leave an abandoned Cue terminal, never claimable"
    );
    assert!(
        recover_abandoned_cue_runs(&conn).unwrap().is_empty(),
        "terminal recovery must be idempotent"
    );

    assert_eq!(
        fs::read_to_string(&effects).unwrap(),
        "ran\n",
        "the side effect must have happened exactly once"
    );
    let _ = fs::remove_dir_all(ws.root());
}

/// Recovery is scoped: it must not resolve a live run, nor a queued one,
/// nor an ordinary crashed job the worker is entitled to retry.
#[test]
fn recovery_touches_only_abandoned_cue_runs() {
    let ws = scratch_workspace("cue_recovery_scope");
    let conn = open(&ws).expect("open");

    // A live Cue run, lease still valid.
    enqueue(
        &conn,
        "/x/live.sh",
        &[],
        EnqueueOptions {
            run_id: Some("live-cue".into()),
            trigger: RunTrigger::Cue,
            ..enqueue_opts()
        },
    )
    .unwrap();
    claim_next(&conn, "w", &ClaimFilters::default())
        .unwrap()
        .unwrap();

    // A crashed ordinary job, which the worker may retry itself.
    let queued = enqueue(&conn, "/x/queued.sh", &[], enqueue_opts()).unwrap();
    claim_next(&conn, "w", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    conn.execute(
        "UPDATE runs SET lease_until = ?1 WHERE run_id = ?2",
        rusqlite::params![unix_millis() - HEARTBEAT_MS - 1, queued.run_id],
    )
    .unwrap();

    assert!(
        recover_abandoned_cue_runs(&conn).unwrap().is_empty(),
        "recovery must not resolve a live cue run or an ordinary crashed job"
    );
    assert_eq!(
        get_run(&conn, "live-cue").unwrap().unwrap().state,
        RunState::Running
    );
    let _ = fs::remove_dir_all(ws.root());
}

/// The control: an ordinary queued job *is* still re-claimed, so the
/// exclusion above is narrow rather than a blanket change to the worker.
#[test]
fn a_crashed_queued_run_is_still_reclaimed() {
    let ws = scratch_workspace("queued_lease_steal");
    let conn = open(&ws).expect("open");

    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    claim_next(&conn, "w", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    conn.execute(
        "UPDATE runs SET lease_until = ?1 WHERE run_id = ?2",
        rusqlite::params![unix_millis() - HEARTBEAT_MS - 1, row.run_id],
    )
    .unwrap();

    assert!(
        claim_next(&conn, "w2", &ClaimFilters::default())
            .unwrap()
            .is_some(),
        "queued work must still recover from a dead worker"
    );
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn fail_transitions_running_to_failed() {
    let ws = scratch_workspace("fail_path");
    let conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    let _ = claim_next(&conn, "w", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    fail(&conn, &row.run_id, fail_completion()).unwrap();
    let loaded = get_run(&conn, &row.run_id).unwrap().unwrap();
    assert_eq!(loaded.state, RunState::Failed);
    assert_eq!(loaded.success, Some(false));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn complete_rejects_queued_row() {
    let ws = scratch_workspace("complete_rejects");
    let conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    let err = complete(&conn, &row.run_id, ok_completion()).unwrap_err();
    assert!(matches!(
        &err,
        RunsError::IllegalTransition {
            from: RunState::Queued,
            to: RunState::Completed
        }
    ));
    assert_eq!(
        err.to_string(),
        "illegal transition: cannot move queued -> completed; row must be in 'running'"
    );
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn cancel_queued_transitions_to_cancelled_immediately() {
    let ws = scratch_workspace("cancel_queued");
    let conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    let after = cancel(&conn, &row.run_id, Some("ux".into()), None).unwrap();
    assert_eq!(after.state, RunState::Cancelled);
    assert_eq!(after.reason.as_deref(), Some("ux"));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn cancel_running_transitions_to_cancelled() {
    let ws = scratch_workspace("cancel_running");
    let conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    let _ = claim_next(&conn, "w", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    let after = cancel(&conn, &row.run_id, Some("kill".into()), None).unwrap();
    assert_eq!(after.state, RunState::Cancelled);
    assert_eq!(after.success, Some(false));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn cancel_terminal_returns_error() {
    let ws = scratch_workspace("cancel_terminal");
    let conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    let _ = claim_next(&conn, "w", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    complete(&conn, &row.run_id, ok_completion()).unwrap();
    let err = cancel(&conn, &row.run_id, None, None).unwrap_err();
    assert!(matches!(
        &err,
        RunsError::TerminalState(RunState::Completed)
    ));
    assert_eq!(
        err.to_string(),
        "cannot cancel run in terminal state 'completed'"
    );
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn dead_letter_only_succeeds_on_failed_or_timed_out() {
    let ws = scratch_workspace("dead_letter_paths");
    let conn = open(&ws).expect("open");

    // failed -> dead_letter ok
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    claim_next(&conn, "w", &ClaimFilters::default()).unwrap();
    fail(&conn, &row.run_id, fail_completion()).unwrap();
    let after = dead_letter(&conn, &row.run_id, Some("chronic".into())).unwrap();
    assert_eq!(after.state, RunState::DeadLetter);
    assert_eq!(after.reason.as_deref(), Some("chronic"));

    // timed_out -> dead_letter ok
    let row = enqueue(&conn, "/x/b.sh", &[], enqueue_opts()).unwrap();
    claim_next(&conn, "w", &ClaimFilters::default()).unwrap();
    time_out(&conn, &row.run_id, fail_completion()).unwrap();
    let after = dead_letter(&conn, &row.run_id, None).unwrap();
    assert_eq!(after.state, RunState::DeadLetter);

    // completed -> dead_letter rejected
    let row = enqueue(&conn, "/x/c.sh", &[], enqueue_opts()).unwrap();
    claim_next(&conn, "w", &ClaimFilters::default()).unwrap();
    complete(&conn, &row.run_id, ok_completion()).unwrap();
    let err = dead_letter(&conn, &row.run_id, None).unwrap_err();
    assert!(matches!(
        &err,
        RunsError::DeadLetterIneligible(RunState::Completed)
    ));
    assert_eq!(err.to_string(), "cannot promote run in state 'completed' to dead_letter; only failed or timed_out rows are eligible");

    let _ = fs::remove_dir_all(ws.root());
}

// -----------------------------------------------------------------
// claim_next behavior
// -----------------------------------------------------------------

#[test]
fn claim_next_returns_none_when_empty() {
    let ws = scratch_workspace("claim_empty");
    let conn = open(&ws).expect("open");
    assert!(claim_next(&conn, "w", &ClaimFilters::default())
        .unwrap()
        .is_none());
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn claim_next_can_exclude_cues_for_context_free_workers() {
    let ws = scratch_workspace("claim_excludes_cues");
    let conn = open(&ws).expect("open");
    let cue = enqueue(
        &conn,
        "/x/remote.sh",
        &[],
        EnqueueOptions {
            actor: "omk1_peer".into(),
            trigger: RunTrigger::Cue,
            ..enqueue_opts()
        },
    )
    .unwrap();
    let local = enqueue(&conn, "/x/local.sh", &[], enqueue_opts()).unwrap();

    let claimed = claim_next(
        &conn,
        "generic-worker",
        &ClaimFilters {
            exclude_cues: true,
            ..ClaimFilters::default()
        },
    )
    .unwrap()
    .unwrap();

    assert_eq!(claimed.run_id, local.run_id);
    assert_eq!(
        get_run(&conn, &cue.run_id).unwrap().unwrap().state,
        RunState::Queued
    );
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn claim_next_allows_only_one_running_cue_per_actor() {
    let ws = scratch_workspace("claim_cue_actor_bound");
    let conn = open(&ws).expect("open");
    let peer = "omk1_peer";
    let first = enqueue(
        &conn,
        "/x/first.sh",
        &[],
        EnqueueOptions {
            actor: peer.into(),
            trigger: RunTrigger::Cue,
            ..enqueue_opts()
        },
    )
    .unwrap();
    let second = enqueue(
        &conn,
        "/x/second.sh",
        &[],
        EnqueueOptions {
            actor: peer.into(),
            trigger: RunTrigger::Cue,
            ..enqueue_opts()
        },
    )
    .unwrap();

    assert_eq!(
        claim_next(&conn, "w1", &ClaimFilters::default())
            .unwrap()
            .unwrap()
            .run_id,
        first.run_id
    );
    assert!(
        claim_next(&conn, "w2", &ClaimFilters::default())
            .unwrap()
            .is_none(),
        "a second Cue from the same peer must wait while the first is running"
    );

    let other_peer = enqueue(
        &conn,
        "/x/other.sh",
        &[],
        EnqueueOptions {
            actor: "omk1_other".into(),
            trigger: RunTrigger::Cue,
            ..enqueue_opts()
        },
    )
    .unwrap();
    assert_eq!(
        claim_next(&conn, "w3", &ClaimFilters::default())
            .unwrap()
            .unwrap()
            .run_id,
        other_peer.run_id,
        "the bound is per peer, not a global Cue worker limit"
    );
    assert_eq!(
        get_run(&conn, &second.run_id).unwrap().unwrap().state,
        RunState::Queued
    );
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn claim_next_orders_by_priority_then_enqueued_at() {
    let ws = scratch_workspace("claim_order");
    let conn = open(&ws).expect("open");
    let low = enqueue(
        &conn,
        "/x/low.sh",
        &[],
        EnqueueOptions {
            priority: 0,
            ..enqueue_opts()
        },
    )
    .unwrap();
    // Sleep one ms so enqueued_at differs.
    std::thread::sleep(std::time::Duration::from_millis(2));
    let high = enqueue(
        &conn,
        "/x/high.sh",
        &[],
        EnqueueOptions {
            priority: 10,
            ..enqueue_opts()
        },
    )
    .unwrap();
    let claimed = claim_next(&conn, "w", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    assert_eq!(claimed.run_id, high.run_id, "higher priority must win");
    let claimed2 = claim_next(&conn, "w", &ClaimFilters::default())
        .unwrap()
        .unwrap();
    assert_eq!(claimed2.run_id, low.run_id);
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn claim_next_reclaims_expired_lease() {
    let ws = scratch_workspace("claim_reclaim");
    let conn = open(&ws).expect("open");
    // Insert a row directly in `running` with an already-expired lease.
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    conn.execute(
        "UPDATE runs SET state='running', worker_id='dead', started_at=?, lease_until=?
                WHERE run_id=?",
        params![unix_millis() - 100_000, unix_millis() - 50_000, row.run_id],
    )
    .unwrap();
    let reclaimed = claim_next(&conn, "fresh", &ClaimFilters::default())
        .unwrap()
        .expect("expired lease must be reclaimable");
    assert_eq!(reclaimed.run_id, row.run_id);
    assert_eq!(reclaimed.worker_id.as_deref(), Some("fresh"));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn claim_next_does_not_reclaim_fresh_lease() {
    let ws = scratch_workspace("claim_no_steal");
    let conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    // Claim once with worker A, leaving a fresh lease in the future.
    claim_next(&conn, "A", &ClaimFilters::default()).unwrap();
    // Worker B must NOT be able to steal it.
    assert!(claim_next(&conn, "B", &ClaimFilters::default())
        .unwrap()
        .is_none());
    // The original row must still be owned by A.
    let loaded = get_run(&conn, &row.run_id).unwrap().unwrap();
    assert_eq!(loaded.worker_id.as_deref(), Some("A"));
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn claim_next_actor_filter() {
    let ws = scratch_workspace("claim_actor");
    let conn = open(&ws).expect("open");
    enqueue(
        &conn,
        "/x/a.sh",
        &[],
        EnqueueOptions {
            actor: "agent-sp".into(),
            ..enqueue_opts()
        },
    )
    .unwrap();
    let other = enqueue(
        &conn,
        "/x/b.sh",
        &[],
        EnqueueOptions {
            actor: "agent-rj".into(),
            ..enqueue_opts()
        },
    )
    .unwrap();
    let claimed = claim_next(
        &conn,
        "w",
        &ClaimFilters {
            actor: Some("agent-rj".into()),
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(claimed.run_id, other.run_id);
    let _ = fs::remove_dir_all(ws.root());
}

// -----------------------------------------------------------------
// Heartbeat
// -----------------------------------------------------------------

#[test]
fn heartbeat_extends_lease_for_owner() {
    let ws = scratch_workspace("hb_owner");
    let conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    claim_next(&conn, "owner", &ClaimFilters::default()).unwrap();
    let lease_before = get_run(&conn, &row.run_id)
        .unwrap()
        .unwrap()
        .lease_until
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let state = heartbeat(&conn, &row.run_id, "owner").unwrap();
    assert_eq!(state, Some(RunState::Running));
    let lease_after = get_run(&conn, &row.run_id)
        .unwrap()
        .unwrap()
        .lease_until
        .unwrap();
    assert!(lease_after >= lease_before);
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn heartbeat_returns_cancelled_when_external_cancel() {
    let ws = scratch_workspace("hb_cancel");
    let conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
    claim_next(&conn, "owner", &ClaimFilters::default()).unwrap();
    cancel(&conn, &row.run_id, Some("user".into()), None).unwrap();
    let state = heartbeat(&conn, &row.run_id, "owner").unwrap();
    assert_eq!(state, Some(RunState::Cancelled));
    let _ = fs::remove_dir_all(ws.root());
}

// -----------------------------------------------------------------
// Concurrency: claim_next under N threads
// -----------------------------------------------------------------

#[test]
fn claim_next_under_concurrency_no_duplicates() {
    let ws = scratch_workspace("concurrency");
    let conn = open(&ws).expect("open");
    // Seed N queued jobs.
    const N: usize = 20;
    for i in 0..N {
        enqueue(&conn, &format!("/x/job_{}.sh", i), &[], enqueue_opts()).unwrap();
    }
    let db_path = runs_db_path(&ws);
    let mut handles = Vec::new();
    for w in 0..4 {
        let path = db_path.clone();
        handles.push(std::thread::spawn(move || {
            let conn = open_connection(&path).expect("open per-thread");
            let mut claimed = Vec::new();
            let worker_id = format!("worker-{}", w);
            while let Some(row) = claim_next(&conn, &worker_id, &ClaimFilters::default()).unwrap() {
                claimed.push(row.run_id);
            }
            claimed
        }));
    }
    let mut all_claims: Vec<String> = Vec::new();
    for h in handles {
        all_claims.extend(h.join().unwrap());
    }
    assert_eq!(all_claims.len(), N, "every job claimed exactly once");
    let mut sorted = all_claims.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), N, "no duplicates");
    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn claim_next_honours_script_filter() {
    let ws = scratch_workspace("claim_script");
    let conn = open(&ws).expect("open");
    enqueue(&conn, "/scripts/alpha.sh", &[], enqueue_opts()).unwrap();
    enqueue(&conn, "/scripts/beta.sh", &[], enqueue_opts()).unwrap();

    let claimed = claim_next(
        &conn,
        "w1",
        &ClaimFilters {
            script: Some("beta".into()),
            ..Default::default()
        },
    )
    .unwrap()
    .unwrap();
    assert!(claimed.script_path.contains("beta"));

    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn dead_letter_preserves_existing_reason_when_no_new() {
    let ws = scratch_workspace("dl_keep_reason");
    let conn = open(&ws).expect("open");
    let row = enqueue(&conn, "/scripts/x.sh", &[], enqueue_opts()).unwrap();
    claim_next(&conn, "w1", &ClaimFilters::default()).unwrap();
    fail(&conn, &row.run_id, fail_completion()).unwrap();
    // Manually set a reason so the (Some, None) merge branch is exercised.
    conn.execute(
        "UPDATE runs SET reason = 'first failure' WHERE run_id = ?",
        params![&row.run_id],
    )
    .unwrap();

    let promoted = dead_letter(&conn, &row.run_id, None).unwrap();
    assert_eq!(promoted.state, RunState::DeadLetter);
    assert_eq!(promoted.reason.as_deref(), Some("first failure"));

    let _ = fs::remove_dir_all(ws.root());
}

#[test]
fn missing_run_and_missing_table_preserve_lifecycle_error_kinds_and_text() {
    let ws = scratch_workspace("lifecycle_errors");
    let conn = open(&ws).unwrap();
    let missing = complete(&conn, "absent", ok_completion()).unwrap_err();
    assert!(matches!(&missing, RunsError::RunNotFound(id) if id == "absent"));
    assert_eq!(missing.to_string(), "run not found: absent");

    let empty = Connection::open_in_memory().unwrap();
    let sqlite = heartbeat(&empty, "absent", "worker").unwrap_err();
    match &sqlite {
        RunsError::Sqlite { operation, source } => {
            assert_eq!(*operation, "Heartbeat failed");
            assert_eq!(source.to_string(), "no such table: runs");
        }
        other => panic!("expected SQLite error, got {other:?}"),
    }
    assert_eq!(sqlite.to_string(), "Heartbeat failed: no such table: runs");
    let _ = fs::remove_dir_all(ws.root());
}

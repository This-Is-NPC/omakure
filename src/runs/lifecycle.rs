use super::query::{get_run, RunRow};
use super::state::{RunState, RunTrigger};
use super::RunsError;
use super::HEARTBEAT_MS;
use crate::util::time::unix_millis;
use rusqlite::{params, Connection, OptionalExtension};

/// Filters used by [`claim_next`] to scope a worker to a subset of jobs.
#[derive(Debug, Clone, Default)]
pub struct ClaimFilters {
    pub actor: Option<String>,
    pub script: Option<String>,
    pub exclude_cues: bool,
}

/// Claim the next eligible job atomically, transitioning it from
/// `queued` (or `running` with an expired lease) to `running` and stamping
/// the worker id, started_at, and a fresh `lease_until = now + HEARTBEAT_MS`.
///
/// Implemented as a single SQLite `UPDATE ... RETURNING run_id` statement
/// so two concurrent workers (or threads) never claim the same row.
pub fn claim_next(
    conn: &Connection,
    worker_id: &str,
    filters: &ClaimFilters,
) -> Result<Option<RunRow>, RunsError> {
    let now = unix_millis();
    // Build the inner SELECT with optional filters. The outer UPDATE always
    // sets state='running'.
    // A Cue-origin run is claimable once, like anything else, but is never
    // lease-stolen afterwards.
    //
    // Re-claiming an expired-lease `running` row is right for a queued job: the
    // worker died, nobody saw a result, run it again. It is wrong for a remote
    // instruction, because the side effect may well have happened and the caller
    // has no way to know it happened twice. That silently turns at-most-once
    // into at-least-once on the one path where the guarantee was promised.
    //
    // The exclusion therefore belongs to the lease-steal branch alone. A crashed
    // Cue-origin row is resolved to a terminal state by recovery instead,
    // without re-executing.
    let mut where_clauses = vec![format!(
        "(state = 'queued' OR (state = 'running' AND lease_until IS NOT NULL \
          AND lease_until < :now AND trigger <> '{}'))",
        RunTrigger::Cue.as_str()
    )];
    where_clauses.push(format!(
        "NOT (trigger = '{}' AND EXISTS (\
            SELECT 1 FROM runs AS active_cue \
             WHERE active_cue.trigger = '{}' \
               AND active_cue.actor = runs.actor \
               AND active_cue.state = 'running'))",
        RunTrigger::Cue.as_str(),
        RunTrigger::Cue.as_str(),
    ));
    let mut named_params: Vec<(&str, Box<dyn rusqlite::ToSql>)> = Vec::new();
    named_params.push((":now", Box::new(now)));
    named_params.push((":worker_id", Box::new(worker_id.to_string())));
    named_params.push((":lease", Box::new(now + HEARTBEAT_MS)));
    if let Some(actor) = &filters.actor {
        where_clauses.push("actor = :actor".to_string());
        named_params.push((":actor", Box::new(actor.clone())));
    }
    if let Some(script) = &filters.script {
        where_clauses.push(
            "(script_path = :script OR script_path LIKE :script_like OR script_name LIKE :script_like)"
                .to_string(),
        );
        named_params.push((":script", Box::new(script.clone())));
        named_params.push((":script_like", Box::new(format!("%{}%", script))));
    }
    if filters.exclude_cues {
        where_clauses.push(format!("trigger <> '{}'", RunTrigger::Cue.as_str()));
    }

    let sql = format!(
        "UPDATE runs
            SET state = 'running',
                started_at = COALESCE(started_at, :now),
                worker_id = :worker_id,
                lease_until = :lease
          WHERE run_id = (
              SELECT run_id FROM runs
               WHERE {}
               ORDER BY
                   CASE WHEN state = 'queued' THEN 0 ELSE 1 END,
                   priority DESC,
                   enqueued_at ASC
               LIMIT 1
          )
          RETURNING run_id",
        where_clauses.join(" AND ")
    );

    let claimed_id: Option<String> = {
        let mut stmt = conn.prepare(&sql).map_err(|source| RunsError::Sqlite {
            operation: "Prepare claim_next failed",
            source,
        })?;
        let params_ref: Vec<(&str, &dyn rusqlite::ToSql)> = named_params
            .iter()
            .map(|(name, value)| (*name, value.as_ref()))
            .collect();
        stmt.query_row(&params_ref[..], |row| row.get::<_, String>(0))
            .optional()
            .map_err(|source| RunsError::Sqlite {
                operation: "Claim next failed",
                source,
            })?
    };

    match claimed_id {
        Some(id) => get_run(conn, &id),
        None => Ok(None),
    }
}

/// Refresh the heartbeat lease on a `running` row currently held by
/// `worker_id`. Returns the row's current state on success, or `None` if
/// the row is no longer owned by `worker_id` (e.g. cancelled or stolen).
///
/// Callers use the returned state to detect mid-execution cancel: if it is
/// not [`RunState::Running`], the worker should kill the script.
pub fn heartbeat(
    conn: &Connection,
    run_id: &str,
    worker_id: &str,
) -> Result<Option<RunState>, RunsError> {
    let now = unix_millis();
    let updated = conn
        .execute(
            "UPDATE runs
                SET lease_until = ?
              WHERE run_id = ? AND worker_id = ? AND state = 'running'",
            params![now + HEARTBEAT_MS, run_id, worker_id],
        )
        .map_err(|source| RunsError::Sqlite {
            operation: "Heartbeat failed",
            source,
        })?;
    if updated == 0 {
        // Either the row was reclaimed/cancelled, or terminated already.
        let row = get_run(conn, run_id)?;
        return Ok(row.map(|r| r.state));
    }
    Ok(Some(RunState::Running))
}

/// Captured outcome of a script execution. Returned by the shared
/// execution helper and consumed by [`complete`] / [`fail`] / etc.
#[derive(Debug, Clone)]
pub struct RunCompletion {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub error: Option<String>,
}

fn finalize(
    conn: &Connection,
    run_id: &str,
    target: RunState,
    completion: &RunCompletion,
) -> Result<(), RunsError> {
    let now = unix_millis();
    // Look up the row first so we can compute duration_ms relative to its
    // started_at and reject illegal transitions.
    let row = get_run(conn, run_id)?.ok_or_else(|| RunsError::RunNotFound(run_id.to_string()))?;
    if !matches!(row.state, RunState::Running) {
        return Err(RunsError::IllegalTransition {
            from: row.state,
            to: target,
        });
    }
    let started = row.started_at.unwrap_or(now);
    let duration_ms = (now - started).max(0);
    conn.execute(
        "UPDATE runs
            SET state = ?, finished_at = ?, duration_ms = ?, exit_code = ?, success = ?,
                stdout = ?, stderr = ?, error = ?, lease_until = NULL
          WHERE run_id = ?",
        params![
            target.as_str(),
            now,
            duration_ms,
            completion.exit_code,
            completion.success as i64,
            completion.stdout,
            completion.stderr,
            completion.error,
            run_id,
        ],
    )
    .map_err(|source| RunsError::Sqlite {
        operation: "Finalize run failed",
        source,
    })?;
    Ok(())
}

/// Mark a `running` row as `completed`.
pub fn complete(
    conn: &Connection,
    run_id: &str,
    completion: RunCompletion,
) -> Result<(), RunsError> {
    finalize(conn, run_id, RunState::Completed, &completion)
}

/// Mark a `running` row as `failed`.
pub fn fail(conn: &Connection, run_id: &str, completion: RunCompletion) -> Result<(), RunsError> {
    finalize(conn, run_id, RunState::Failed, &completion)
}

/// Resolve Cue-origin runs abandoned by a crashed worker, without re-running.
///
/// A Cue-origin row is excluded from the worker lease steal on purpose, so a
/// crash leaves it `running` with a lapsed lease and nothing will ever pick it
/// up again. That is the correct trade — running a remote instruction twice is
/// worse than not knowing whether it finished — but the row still has to reach
/// a terminal state, or the Conductor waits forever and the node reports a run
/// that is permanently in flight.
///
/// Each such row becomes `failed` with an explicit reason. `failed` rather than
/// `cancelled` because nobody cancelled it, and rather than `completed` because
/// nobody observed a result. The honest answer is that the outcome is unknown,
/// and of the shipped terminal states `failed` is the one that does not claim
/// otherwise.
///
/// Returns the run ids it resolved.
pub fn recover_abandoned_cue_runs(conn: &Connection) -> Result<Vec<String>, RunsError> {
    let now = unix_millis();
    let mut statement = conn
        .prepare(
            "UPDATE runs
                SET state = 'failed',
                    finished_at = ?1,
                    duration_ms = MAX(0, ?1 - COALESCE(started_at, ?1)),
                    exit_code = NULL,
                    success = 0,
                    stdout = '',
                    stderr = '',
                    error = ?2,
                    lease_until = NULL
              WHERE state = 'running'
                AND trigger = ?3
                AND lease_until IS NOT NULL
                AND lease_until < ?1
              RETURNING run_id",
        )
        .map_err(|source| RunsError::Sqlite {
            operation: "Prepare cue recovery failed",
            source,
        })?;
    let recovered = statement
        .query_map(
            params![
                now,
                "the worker holding this remote run stopped; it was not re-run because a remote instruction must execute at most once",
                RunTrigger::Cue.as_str()
            ],
            |row| row.get(0),
        )
        .map_err(|source| RunsError::Sqlite {
            operation: "Query cue recovery failed",
            source,
        })?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|source| RunsError::Sqlite {
            operation: "Read cue recovery failed",
            source,
        })?;
    Ok(recovered)
}

/// Cancel every unfinished Cue-origin run this peer caused in one atomic SQL
/// statement. A failure aborts the statement and is returned; no row is
/// silently skipped. The executor heartbeat kills a child as soon as its
/// running row leaves `running`.
///
/// Scoped to `trigger = 'cue'` on purpose. A revoked peer's name may also
/// appear on locally-initiated work, and revoking a peer is not a licence to
/// cancel what this node's owner started.
pub fn cancel_cue_runs_for_actor(conn: &Connection, actor: &str) -> Result<Vec<String>, RunsError> {
    let now = unix_millis();
    let mut statement = conn
        .prepare(
            "UPDATE runs
                SET state = 'cancelled',
                    finished_at = ?1,
                    duration_ms = CASE
                        WHEN state = 'queued' THEN 0
                        ELSE MAX(0, ?1 - COALESCE(started_at, ?1))
                    END,
                    success = 0,
                    error = CASE
                        WHEN state = 'running' THEN
                            COALESCE(error, 'cancelled because the peer was revoked')
                        ELSE error
                    END,
                    reason = 'the peer that asked for this run was revoked',
                    lease_until = NULL
              WHERE trigger = ?2
                AND actor = ?3
                AND state IN ('queued', 'running')
              RETURNING run_id",
        )
        .map_err(|source| RunsError::Sqlite {
            operation: "Prepare cue revocation failed",
            source,
        })?;
    let cancelled = statement
        .query_map(params![now, RunTrigger::Cue.as_str(), actor], |row| {
            row.get(0)
        })
        .map_err(|source| RunsError::Sqlite {
            operation: "Cancel cue runs failed",
            source,
        })?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|source| RunsError::Sqlite {
            operation: "Read cancelled Cue runs failed",
            source,
        })?;
    Ok(cancelled)
}

/// Mark a `running` row as `timed_out` after the worker killed the
/// process for exceeding its `--timeout`.
pub fn time_out(
    conn: &Connection,
    run_id: &str,
    completion: RunCompletion,
) -> Result<(), RunsError> {
    finalize(conn, run_id, RunState::TimedOut, &completion)
}

/// Cancel a row. Behavior depends on its current state:
///
/// - `queued`: instantly transitions to `cancelled` and writes the optional
///   reason. Sets a synthetic finished_at so listings show it as terminal.
/// - `running`: transitions to `cancelled` and writes the supplied
///   `completion` (the worker provides the partial stdout/stderr captured
///   before kill).
/// - any terminal state: returns an error so the caller can surface
///   `error.code = "invalid_argument"` to the user.
pub fn cancel(
    conn: &Connection,
    run_id: &str,
    reason: Option<String>,
    completion: Option<RunCompletion>,
) -> Result<RunRow, RunsError> {
    let row = get_run(conn, run_id)?.ok_or_else(|| RunsError::RunNotFound(run_id.to_string()))?;
    let now = unix_millis();
    match row.state {
        RunState::Queued => {
            conn.execute(
                "UPDATE runs
                    SET state = 'cancelled', finished_at = ?, duration_ms = 0, success = 0,
                        reason = COALESCE(?, reason)
                  WHERE run_id = ?",
                params![now, reason, run_id],
            )
            .map_err(|source| RunsError::Sqlite {
                operation: "Cancel queued run failed",
                source,
            })?;
        }
        RunState::Running => {
            let completion = completion.unwrap_or(RunCompletion {
                stdout: row.stdout.clone(),
                stderr: row.stderr.clone(),
                exit_code: None,
                success: false,
                error: Some("cancelled by user".to_string()),
            });
            let started = row.started_at.unwrap_or(now);
            let duration_ms = (now - started).max(0);
            conn.execute(
                "UPDATE runs
                    SET state = 'cancelled', finished_at = ?, duration_ms = ?,
                        exit_code = ?, success = 0, stdout = ?, stderr = ?,
                        error = ?, reason = COALESCE(?, reason),
                        lease_until = NULL
                  WHERE run_id = ?",
                params![
                    now,
                    duration_ms,
                    completion.exit_code,
                    completion.stdout,
                    completion.stderr,
                    completion.error,
                    reason,
                    run_id,
                ],
            )
            .map_err(|source| RunsError::Sqlite {
                operation: "Cancel running run failed",
                source,
            })?;
        }
        terminal => {
            return Err(RunsError::TerminalState(terminal));
        }
    }
    get_run(conn, run_id)?.ok_or_else(|| RunsError::RunNotFoundAfterCancel(run_id.to_string()))
}

/// Mark a `cancelled` row produced by mid-execution cancel as needing
/// the worker's captured output. Used internally by the worker after it
/// detects an external cancel via [`heartbeat`] and kills the script.
pub fn record_cancelled_output(
    conn: &Connection,
    run_id: &str,
    completion: RunCompletion,
) -> Result<(), RunsError> {
    let row = get_run(conn, run_id)?.ok_or_else(|| RunsError::RunNotFound(run_id.to_string()))?;
    let now = unix_millis();
    let started = row.started_at.unwrap_or(now);
    let duration_ms = (now - started).max(0);
    conn.execute(
        "UPDATE runs
            SET stdout = ?, stderr = ?, error = COALESCE(?, error),
                exit_code = ?, finished_at = ?, duration_ms = ?,
                lease_until = NULL
          WHERE run_id = ? AND state = 'cancelled'",
        params![
            completion.stdout,
            completion.stderr,
            completion.error,
            completion.exit_code,
            now,
            duration_ms,
            run_id,
        ],
    )
    .map_err(|source| RunsError::Sqlite {
        operation: "Record cancelled output failed",
        source,
    })?;
    Ok(())
}

/// Promote a `failed` or `timed_out` row into `dead_letter`. Any other
/// state is rejected.
pub fn dead_letter(
    conn: &Connection,
    run_id: &str,
    reason: Option<String>,
) -> Result<RunRow, RunsError> {
    let row = get_run(conn, run_id)?.ok_or_else(|| RunsError::RunNotFound(run_id.to_string()))?;
    if !matches!(row.state, RunState::Failed | RunState::TimedOut) {
        return Err(RunsError::DeadLetterIneligible(row.state));
    }
    let merged_reason = match (row.reason.as_deref(), reason.as_deref()) {
        (Some(existing), Some(new)) => Some(format!("{}\n{}", existing, new)),
        (None, Some(new)) => Some(new.to_string()),
        (Some(existing), None) => Some(existing.to_string()),
        (None, None) => None,
    };
    conn.execute(
        "UPDATE runs SET state = 'dead_letter', reason = ? WHERE run_id = ?",
        params![merged_reason, run_id],
    )
    .map_err(|source| RunsError::Sqlite {
        operation: "Dead-letter run failed",
        source,
    })?;
    get_run(conn, run_id)?.ok_or_else(|| RunsError::RunNotFoundAfterDeadLetter(run_id.to_string()))
}

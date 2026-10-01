use super::RunsError;
use super::state::{RunState, RunStateSet, RunTrigger};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// RunRow
// ---------------------------------------------------------------------------

/// One row of the `runs` table.
///
/// Field names are stable: this struct is serialized as-is into the
/// `--json` envelope returned by `omakure run --json` and
/// `omakure history show`. Renaming any field is a breaking change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRow {
    pub run_id: String,
    pub script_path: String,
    pub script_name: Option<String>,
    pub args_json: String,
    pub actor: String,
    pub reason: Option<String>,

    /// State machine column. See [`RunState`].
    pub state: RunState,
    /// Higher value picked first by the worker claim query.
    pub priority: i64,
    /// Unix ms when the row was created (queued or inline-started).
    pub enqueued_at: i64,
    /// Id of the worker process currently holding the job's lease, if any.
    pub worker_id: Option<String>,
    /// Unix ms; the worker keeps this in the future while the script runs.
    /// If it expires while `state = 'running'`, another worker may steal it.
    pub lease_until: Option<i64>,
    /// Per-row execution timeout in ms; null means no timeout.
    pub timeout_ms: Option<i64>,
    /// Provenance tag for rows enqueued by the omakure cron scheduler.
    /// Format: `<canonical-script-path>@<cron-expr>`.
    pub cron_schedule_id: Option<String>,
    /// Origin of the run: `Manual` when a human enqueued it, `Scheduled`
    /// when the cron scheduler did. Defaults to `Manual` for pre-scheduler rows.
    #[serde(default)]
    pub trigger: RunTrigger,

    /// Unix ms when execution began. Null while `state = 'queued'`.
    pub started_at: Option<i64>,
    /// Unix ms when execution finished. Null until terminal.
    pub finished_at: Option<i64>,
    /// Wall-clock duration in ms. Null until terminal.
    pub duration_ms: Option<i64>,
    /// Process exit code. Null until terminal.
    pub exit_code: Option<i32>,
    /// True iff the script terminated with `success`. Null until terminal.
    pub success: Option<bool>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
    pub parent_run_id: Option<String>,
    pub omakure_version: String,
}

/// Filters for [`query_runs`]. All filters are AND-combined; `None`
/// fields are ignored. Default filters return only **terminal** rows
/// ordered by `started_at DESC`.
#[derive(Debug, Clone)]
pub struct RunFilters {
    pub script: Option<String>,
    pub actor: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    /// `Some(true)` filters successes only, `Some(false)` failures only,
    /// `None` returns both.
    pub success: Option<bool>,
    pub limit: Option<i64>,
    /// Filter by state. Empty vec means "no state filter" — every state
    /// is returned. Default value is the [`RunStateSet::Terminal`] set, which
    /// returns completed runs only.
    pub states: Vec<RunState>,
}

impl Default for RunFilters {
    fn default() -> Self {
        Self {
            script: None,
            actor: None,
            since_ms: None,
            until_ms: None,
            success: None,
            limit: None,
            states: RunStateSet::Terminal.to_states(),
        }
    }
}

// ---------------------------------------------------------------------------
// Aggregations
// ---------------------------------------------------------------------------

/// Output of [`stats`]. Counts are per-state and per-actor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunStats {
    pub counts_by_state: HashMap<String, i64>,
    pub counts_by_actor: HashMap<String, i64>,
    pub total: i64,
}

/// Fetch one run by id, or `None` if it does not exist.
pub fn get_run(conn: &Connection, run_id: &str) -> Result<Option<RunRow>, RunsError> {
    let mut stmt = conn
        .prepare(
            "SELECT run_id, script_path, script_name, args_json, actor, reason,
                    state, priority, enqueued_at, worker_id, lease_until, timeout_ms,
                    cron_schedule_id, trigger,
                    started_at, finished_at, duration_ms, exit_code, success,
                    stdout, stderr, error, parent_run_id, omakure_version
             FROM runs WHERE run_id = ?",
        )
        .map_err(|source| RunsError::Sqlite {
            operation: "Prepare get_run failed",
            source,
        })?;
    let row = stmt
        .query_row([run_id], row_to_run)
        .optional()
        .map_err(|source| RunsError::Sqlite {
            operation: "Query get_run failed",
            source,
        })?;
    Ok(row)
}

/// Fetch one run by id, returning a typed error when it does not exist.
pub(super) fn get_run_required(conn: &Connection, run_id: &str) -> Result<RunRow, RunsError> {
    get_run(conn, run_id)?.ok_or_else(|| RunsError::RunNotFound(run_id.to_owned()))
}

/// Return the most recent enqueue time for a schedule, or `None` when it has
/// never produced a run.
///
/// Schedule state is intentionally derived from run rows rather than a second
/// mutable cursor, so a successful enqueue and the scheduler's next scan
/// cannot disagree about which fire was last recorded.
pub fn last_scheduled_fire_ms(
    conn: &Connection,
    schedule_id: &str,
) -> Result<Option<i64>, RunsError> {
    conn.query_row(
        "SELECT MAX(enqueued_at) FROM runs WHERE cron_schedule_id = ?",
        [schedule_id],
        |row| row.get::<_, Option<i64>>(0),
    )
    .map_err(|source| RunsError::Sqlite {
        operation: "Query last scheduled fire failed",
        source,
    })
}

/// Return whether a schedule currently has a queued or running row.
///
/// This is the scheduler's overlap guard. Terminal rows never block a later
/// fire, while either in-flight state does, including a row that was queued
/// but has not yet been claimed by a worker.
pub fn has_live_scheduled_run(conn: &Connection, schedule_id: &str) -> Result<bool, RunsError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM runs
             WHERE cron_schedule_id = ? AND state IN (?, ?)",
            params![
                schedule_id,
                RunState::Queued.as_str(),
                RunState::Running.as_str()
            ],
            |row| row.get(0),
        )
        .map_err(|source| RunsError::Sqlite {
            operation: "Query live scheduled run failed",
            source,
        })?;
    Ok(count > 0)
}

/// Query rows matching the supplied filters. In-flight rows are surfaced
/// first (by `enqueued_at DESC` so the most recently queued / running rows
/// appear at the top), then terminal rows by `started_at DESC`.
pub fn query_runs(conn: &Connection, filters: &RunFilters) -> Result<Vec<RunRow>, RunsError> {
    let mut sql = String::from(
        "SELECT run_id, script_path, script_name, args_json, actor, reason,
                state, priority, enqueued_at, worker_id, lease_until, timeout_ms,
                cron_schedule_id, trigger,
                started_at, finished_at, duration_ms, exit_code, success,
                stdout, stderr, error, parent_run_id, omakure_version
         FROM runs",
    );
    let mut where_clauses: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(script) = &filters.script {
        where_clauses.push("(script_path = ? OR script_path LIKE ? OR script_name LIKE ?)".into());
        params.push(Box::new(script.clone()));
        params.push(Box::new(format!("%{}%", script)));
        params.push(Box::new(format!("%{}%", script)));
    }
    if let Some(actor) = &filters.actor {
        where_clauses.push("actor = ?".into());
        params.push(Box::new(actor.clone()));
    }
    if let Some(since) = filters.since_ms {
        where_clauses.push("(started_at >= ? OR (started_at IS NULL AND enqueued_at >= ?))".into());
        params.push(Box::new(since));
        params.push(Box::new(since));
    }
    if let Some(until) = filters.until_ms {
        where_clauses.push("(started_at <= ? OR (started_at IS NULL AND enqueued_at <= ?))".into());
        params.push(Box::new(until));
        params.push(Box::new(until));
    }
    if let Some(success) = filters.success {
        where_clauses.push("success = ?".into());
        params.push(Box::new(success as i64));
    }
    if !filters.states.is_empty() {
        let placeholders: Vec<&str> = filters.states.iter().map(|_| "?").collect();
        where_clauses.push(format!("state IN ({})", placeholders.join(",")));
        for state in &filters.states {
            params.push(Box::new(state.as_str().to_string()));
        }
    }

    if !where_clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_clauses.join(" AND "));
    }
    // In-flight rows (queued/running) sort to the top so history consumers
    // screen and `history list --state-set all` always show the live work
    // first. Within each group we order by enqueued_at DESC (live) and
    // started_at DESC (terminal) so the most recent rows are at the top.
    sql.push_str(
        " ORDER BY \
         CASE WHEN state IN ('queued','running') THEN 0 ELSE 1 END, \
         COALESCE(started_at, enqueued_at) DESC",
    );
    if let Some(limit) = filters.limit {
        sql.push_str(&format!(" LIMIT {}", limit));
    }

    let mut stmt = conn.prepare(&sql).map_err(|source| RunsError::Sqlite {
        operation: "Prepare query_runs failed",
        source,
    })?;
    let rows = stmt
        .query_map(
            params_from_iter(params.iter().map(|p| p.as_ref())),
            row_to_run,
        )
        .map_err(|source| RunsError::Sqlite {
            operation: "Query query_runs failed",
            source,
        })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|source| RunsError::Sqlite {
            operation: "Row query_runs failed",
            source,
        })?);
    }
    Ok(out)
}

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRow> {
    let state_str: String = row.get(6)?;
    let state = state_str.parse::<RunState>().map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Text,
            Box::<dyn std::error::Error + Send + Sync>::from(err),
        )
    })?;
    let trigger_str: String = row.get(13)?;
    let trigger = trigger_str.parse::<RunTrigger>().map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            13,
            rusqlite::types::Type::Text,
            Box::<dyn std::error::Error + Send + Sync>::from(err),
        )
    })?;
    let success_int: Option<i64> = row.get(18)?;
    Ok(RunRow {
        run_id: row.get(0)?,
        script_path: row.get(1)?,
        script_name: row.get(2)?,
        args_json: row.get(3)?,
        actor: row.get(4)?,
        reason: row.get(5)?,
        state,
        priority: row.get(7)?,
        enqueued_at: row.get(8)?,
        worker_id: row.get(9)?,
        lease_until: row.get(10)?,
        timeout_ms: row.get(11)?,
        cron_schedule_id: row.get(12)?,
        trigger,
        started_at: row.get(14)?,
        finished_at: row.get(15)?,
        duration_ms: row.get(16)?,
        exit_code: row.get(17)?,
        success: success_int.map(|v| v != 0),
        stdout: row.get(19)?,
        stderr: row.get(20)?,
        error: row.get(21)?,
        parent_run_id: row.get(22)?,
        omakure_version: row.get(23)?,
    })
}

/// Aggregate counts per state and per actor.
pub fn stats(conn: &Connection) -> Result<RunStats, RunsError> {
    let mut counts_by_state: HashMap<String, i64> = HashMap::new();
    let mut counts_by_actor: HashMap<String, i64> = HashMap::new();
    let mut total: i64 = 0;

    let mut state_stmt = conn
        .prepare("SELECT state, COUNT(*) FROM runs GROUP BY state")
        .map_err(|source| RunsError::Sqlite {
            operation: "Prepare state stats failed",
            source,
        })?;
    let state_rows = state_stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|source| RunsError::Sqlite {
            operation: "Query state stats failed",
            source,
        })?;
    for entry in state_rows {
        let (state, count) = entry.map_err(|source| RunsError::Sqlite {
            operation: "Read state row",
            source,
        })?;
        total += count;
        counts_by_state.insert(state, count);
    }
    // Make sure every legal state has an entry (zero when absent) so
    // dashboards can render a stable layout.
    for state in RunState::all() {
        counts_by_state
            .entry(state.as_str().to_string())
            .or_insert(0);
    }

    let mut actor_stmt = conn
        .prepare("SELECT actor, COUNT(*) FROM runs GROUP BY actor")
        .map_err(|source| RunsError::Sqlite {
            operation: "Prepare actor stats failed",
            source,
        })?;
    let actor_rows = actor_stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|source| RunsError::Sqlite {
            operation: "Query actor stats failed",
            source,
        })?;
    for entry in actor_rows {
        let (actor, count) = entry.map_err(|source| RunsError::Sqlite {
            operation: "Read actor row",
            source,
        })?;
        counts_by_actor.insert(actor, count);
    }

    Ok(RunStats {
        counts_by_state,
        counts_by_actor,
        total,
    })
}

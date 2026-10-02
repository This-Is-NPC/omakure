use super::HEARTBEAT_MS;
use super::RunsError;
use super::ids::generate_run_id;
use super::query::{RunRow, has_live_scheduled_run};
use super::state::{RunState, RunTrigger};
use crate::util::time::unix_millis;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

// ---------------------------------------------------------------------------
// Generic insert / read
// ---------------------------------------------------------------------------

/// Insert a fully-formed run row. The caller is responsible for generating
/// `run_id` (typically via [`generate_run_id`]) and setting `state` to a
/// legal value. Used by [`enqueue`] and [`start_inline`] internally and
/// remains exposed for tests / future use.
pub fn insert_run(conn: &Connection, row: &RunRow) -> Result<(), RunsError> {
    conn.execute(
        "INSERT INTO runs (
            run_id, script_path, script_name, args_json, actor, reason,
            state, priority, enqueued_at, worker_id, lease_until, timeout_ms,
            cron_schedule_id, trigger,
            started_at, finished_at, duration_ms, exit_code, success,
            stdout, stderr, error, parent_run_id, omakure_version
         ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![
            row.run_id,
            row.script_path,
            row.script_name,
            row.args_json,
            row.actor,
            row.reason,
            row.state.as_str(),
            row.priority,
            row.enqueued_at,
            row.worker_id,
            row.lease_until,
            row.timeout_ms,
            row.cron_schedule_id,
            row.trigger.as_str(),
            row.started_at,
            row.finished_at,
            row.duration_ms,
            row.exit_code,
            row.success.map(|b| b as i64),
            row.stdout,
            row.stderr,
            row.error,
            row.parent_run_id,
            row.omakure_version,
        ],
    )
    .map_err(|source| RunsError::Sqlite {
        operation: "Insert run failed",
        source,
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// State machine helpers
// ---------------------------------------------------------------------------

/// Options that producers may set when calling [`enqueue`].
#[derive(Debug, Clone, Default)]
pub struct EnqueueOptions {
    pub run_id: Option<String>,
    pub actor: String,
    pub reason: Option<String>,
    pub priority: i64,
    pub timeout_ms: Option<i64>,
    pub parent_run_id: Option<String>,
    pub cron_schedule_id: Option<String>,
    pub script_name: Option<String>,
    pub omakure_version: String,
    pub trigger: RunTrigger,
    pub env_name: Option<String>,
    pub allowed_secret_refs: Option<Vec<String>>,
    /// The exact script bytes this run was authorized against.
    ///
    /// Only a Cue-origin run carries one, and for such a run the executor
    /// treats its absence as a refusal rather than as "no opinion". Written in
    /// the same call as the row so no window exists in which a Cue-origin run
    /// is claimable without the hash that constrains it.
    pub script_content_hash: Option<String>,
}

pub const ALLOW_ALL_SECRET_REFS_POLICY: &str = "__omakure_allow_all_secret_refs__";

/// Insert a fresh `state='queued'` row and its access metadata atomically.
/// Returns the inserted [`RunRow`].
pub fn enqueue(
    conn: &Connection,
    script_path: &str,
    args: &[String],
    opts: EnqueueOptions,
) -> Result<RunRow, RunsError> {
    let transaction =
        Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(|source| {
            RunsError::Sqlite {
                operation: "Begin enqueue failed",
                source,
            }
        })?;
    let row = enqueue_in_transaction(&transaction, script_path, args, opts)?;
    transaction.commit().map_err(|source| RunsError::Sqlite {
        operation: "Commit enqueue failed",
        source,
    })?;
    Ok(row)
}

pub(super) fn enqueue_in_transaction(
    conn: &Connection,
    script_path: &str,
    args: &[String],
    opts: EnqueueOptions,
) -> Result<RunRow, RunsError> {
    let now = unix_millis();
    let row = RunRow {
        run_id: opts.run_id.unwrap_or_else(generate_run_id),
        script_path: script_path.to_string(),
        script_name: opts.script_name,
        args_json: serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string()),
        actor: if opts.actor.is_empty() {
            "human".to_string()
        } else {
            opts.actor
        },
        reason: opts.reason,
        state: RunState::Queued,
        priority: opts.priority,
        enqueued_at: now,
        worker_id: None,
        lease_until: None,
        timeout_ms: opts.timeout_ms,
        cron_schedule_id: opts.cron_schedule_id,
        trigger: opts.trigger,
        started_at: None,
        finished_at: None,
        duration_ms: None,
        exit_code: None,
        success: None,
        stdout: String::new(),
        stderr: String::new(),
        error: None,
        parent_run_id: opts.parent_run_id,
        omakure_version: opts.omakure_version,
    };
    insert_run(conn, &row)?;
    if let Some(env_name) = opts.env_name.as_deref() {
        set_run_env(conn, &row.run_id, env_name)?;
    }
    match opts.allowed_secret_refs.as_deref() {
        Some(refs) => set_run_secret_refs(conn, &row.run_id, refs)?,
        None => set_run_secret_refs(
            conn,
            &row.run_id,
            &[ALLOW_ALL_SECRET_REFS_POLICY.to_string()],
        )?,
    }
    if let Some(hash) = opts.script_content_hash.as_deref() {
        set_run_script_hash(conn, &row.run_id, hash)?;
    }
    Ok(row)
}

/// Atomically claim an eligible scheduled fire and enqueue its run.
///
/// The immediate transaction serializes concurrent schedulers: after one
/// scheduler acquires the write lock and inserts the queued row, the next
/// scheduler observes the live row and returns `Ok(None)`. Any SQLite error
/// aborts the transaction and is returned, so an unreadable schedule state
/// can never be interpreted as an empty queue.
pub fn enqueue_scheduled(
    conn: &Connection,
    script_path: &str,
    args: &[String],
    opts: EnqueueOptions,
) -> Result<Option<RunRow>, RunsError> {
    if opts.trigger != RunTrigger::Scheduled {
        return Err(RunsError::InvalidEnqueue(
            "Scheduled enqueue requires RunTrigger::Scheduled",
        ));
    }
    let schedule_id = opts
        .cron_schedule_id
        .as_deref()
        .ok_or(RunsError::InvalidEnqueue(
            "Scheduled enqueue requires cron_schedule_id",
        ))?;
    let transaction =
        Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(|source| {
            RunsError::Sqlite {
                operation: "Begin scheduled enqueue failed",
                source,
            }
        })?;
    if has_live_scheduled_run(&transaction, schedule_id)? {
        return Ok(None);
    }

    let now = unix_millis();
    let row = RunRow {
        run_id: opts.run_id.unwrap_or_else(generate_run_id),
        script_path: script_path.to_string(),
        script_name: opts.script_name,
        args_json: serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string()),
        actor: if opts.actor.is_empty() {
            "human".to_string()
        } else {
            opts.actor
        },
        reason: opts.reason,
        state: RunState::Queued,
        priority: opts.priority,
        enqueued_at: now,
        worker_id: None,
        lease_until: None,
        timeout_ms: opts.timeout_ms,
        cron_schedule_id: opts.cron_schedule_id,
        trigger: opts.trigger,
        started_at: None,
        finished_at: None,
        duration_ms: None,
        exit_code: None,
        success: None,
        stdout: String::new(),
        stderr: String::new(),
        error: None,
        parent_run_id: opts.parent_run_id,
        omakure_version: opts.omakure_version,
    };
    insert_run(&transaction, &row)?;
    if let Some(env_name) = opts.env_name.as_deref() {
        set_run_env(&transaction, &row.run_id, env_name)?;
    }
    match opts.allowed_secret_refs.as_deref() {
        Some(refs) => set_run_secret_refs(&transaction, &row.run_id, refs)?,
        None => set_run_secret_refs(
            &transaction,
            &row.run_id,
            &[ALLOW_ALL_SECRET_REFS_POLICY.to_string()],
        )?,
    }
    if let Some(hash) = opts.script_content_hash.as_deref() {
        set_run_script_hash(&transaction, &row.run_id, hash)?;
    }
    transaction.commit().map_err(|source| RunsError::Sqlite {
        operation: "Commit scheduled enqueue failed",
        source,
    })?;
    Ok(Some(row))
}

/// Enqueue a Cue in one run-database transaction so the run row and its
/// deny-all/hash metadata become visible together. Revocation races are closed
/// by the worker's registry preflight immediately before execution.
pub fn enqueue_cue(
    conn: &mut Connection,
    script_path: &str,
    args: &[String],
    opts: EnqueueOptions,
) -> Result<RunRow, RunsError> {
    if opts.trigger != RunTrigger::Cue {
        return Err(RunsError::InvalidEnqueue(
            "Cue enqueue requires RunTrigger::Cue",
        ));
    }
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|source| RunsError::Sqlite {
            operation: "Begin Cue enqueue failed",
            source,
        })?;
    let now = unix_millis();
    let row = RunRow {
        run_id: opts.run_id.unwrap_or_else(generate_run_id),
        script_path: script_path.to_string(),
        script_name: opts.script_name,
        args_json: serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string()),
        actor: if opts.actor.is_empty() {
            "human".to_string()
        } else {
            opts.actor
        },
        reason: opts.reason,
        state: RunState::Queued,
        priority: opts.priority,
        enqueued_at: now,
        worker_id: None,
        lease_until: None,
        timeout_ms: opts.timeout_ms,
        cron_schedule_id: opts.cron_schedule_id,
        trigger: opts.trigger,
        started_at: None,
        finished_at: None,
        duration_ms: None,
        exit_code: None,
        success: None,
        stdout: String::new(),
        stderr: String::new(),
        error: None,
        parent_run_id: opts.parent_run_id,
        omakure_version: opts.omakure_version,
    };
    insert_run(&transaction, &row)?;
    match opts.allowed_secret_refs.as_deref() {
        Some(refs) => set_run_secret_refs(&transaction, &row.run_id, refs)?,
        None => set_run_secret_refs(
            &transaction,
            &row.run_id,
            &[ALLOW_ALL_SECRET_REFS_POLICY.to_string()],
        )?,
    }
    if let Some(hash) = opts.script_content_hash.as_deref() {
        set_run_script_hash(&transaction, &row.run_id, hash)?;
    }
    transaction.commit().map_err(|source| RunsError::Sqlite {
        operation: "Commit Cue enqueue failed",
        source,
    })?;
    Ok(row)
}

/// Insert a row directly in `state='running'`, skipping the queued step.
/// Used by the synchronous `omakure run` fast path so the row is visible
/// to `history list --state running` immediately.
pub fn start_inline(
    conn: &Connection,
    script_path: &str,
    args: &[String],
    worker_id: &str,
    opts: EnqueueOptions,
) -> Result<RunRow, RunsError> {
    let now = unix_millis();
    let row = RunRow {
        run_id: opts.run_id.unwrap_or_else(generate_run_id),
        script_path: script_path.to_string(),
        script_name: opts.script_name,
        args_json: serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string()),
        actor: if opts.actor.is_empty() {
            "human".to_string()
        } else {
            opts.actor
        },
        reason: opts.reason,
        state: RunState::Running,
        priority: opts.priority,
        enqueued_at: now,
        worker_id: Some(worker_id.to_string()),
        lease_until: Some(now + HEARTBEAT_MS),
        timeout_ms: opts.timeout_ms,
        cron_schedule_id: opts.cron_schedule_id,
        trigger: opts.trigger,
        started_at: Some(now),
        finished_at: None,
        duration_ms: None,
        exit_code: None,
        success: None,
        stdout: String::new(),
        stderr: String::new(),
        error: None,
        parent_run_id: opts.parent_run_id,
        omakure_version: opts.omakure_version,
    };
    insert_run(conn, &row)?;
    if let Some(env_name) = opts.env_name.as_deref() {
        set_run_env(conn, &row.run_id, env_name)?;
    }
    match opts.allowed_secret_refs.as_deref() {
        Some(refs) => set_run_secret_refs(conn, &row.run_id, refs)?,
        None => set_run_secret_refs(
            conn,
            &row.run_id,
            &[ALLOW_ALL_SECRET_REFS_POLICY.to_string()],
        )?,
    }
    if let Some(hash) = opts.script_content_hash.as_deref() {
        set_run_script_hash(conn, &row.run_id, hash)?;
    }
    Ok(row)
}

pub fn set_run_env(conn: &Connection, run_id: &str, env_name: &str) -> Result<(), RunsError> {
    conn.execute(
        "INSERT INTO run_envs (run_id, env_name) VALUES (?, ?) \
         ON CONFLICT(run_id) DO UPDATE SET env_name = excluded.env_name",
        params![run_id, env_name],
    )
    .map_err(|source| RunsError::Sqlite {
        operation: "Set run env failed",
        source,
    })?;
    Ok(())
}

pub fn get_run_env(conn: &Connection, run_id: &str) -> Result<Option<String>, RunsError> {
    conn.query_row(
        "SELECT env_name FROM run_envs WHERE run_id = ?",
        [run_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|source| RunsError::Sqlite {
        operation: "Get run env failed",
        source,
    })
}

pub fn set_run_secret_refs(
    conn: &Connection,
    run_id: &str,
    refs: &[String],
) -> Result<(), RunsError> {
    conn.execute("DELETE FROM run_secret_refs WHERE run_id = ?", [run_id])
        .map_err(|source| RunsError::Sqlite {
            operation: "Clear run secret refs failed",
            source,
        })?;
    if refs.is_empty() {
        conn.execute(
            "INSERT OR IGNORE INTO run_secret_refs (run_id, secret_ref) VALUES (?, '')",
            [run_id],
        )
        .map_err(|source| RunsError::Sqlite {
            operation: "Set run secret ref policy failed",
            source,
        })?;
        return Ok(());
    }
    for secret_ref in refs {
        conn.execute(
            "INSERT OR IGNORE INTO run_secret_refs (run_id, secret_ref) VALUES (?, ?)",
            params![run_id, secret_ref],
        )
        .map_err(|source| RunsError::Sqlite {
            operation: "Set run secret ref failed",
            source,
        })?;
    }
    Ok(())
}

pub fn get_run_secret_refs(
    conn: &Connection,
    run_id: &str,
) -> Result<Option<Vec<String>>, RunsError> {
    let mut stmt = conn
        .prepare("SELECT secret_ref FROM run_secret_refs WHERE run_id = ? ORDER BY secret_ref")
        .map_err(|source| RunsError::Sqlite {
            operation: "Prepare run secret refs failed",
            source,
        })?;
    let rows = stmt
        .query_map([run_id], |row| row.get(0))
        .map_err(|source| RunsError::Sqlite {
            operation: "Query run secret refs failed",
            source,
        })?;
    let mut refs = Vec::new();
    let mut has_policy = false;
    for row in rows {
        let secret_ref: String = row.map_err(|source| RunsError::Sqlite {
            operation: "Row run secret refs failed",
            source,
        })?;
        has_policy = true;
        if !secret_ref.is_empty() {
            refs.push(secret_ref);
        }
    }
    if !has_policy {
        Ok(None)
    } else {
        Ok(Some(refs))
    }
}

/// Record the script bytes a run was authorized against.
///
/// `INSERT` rather than `INSERT OR REPLACE`: the authorized content of a run is
/// decided once, when the row is created. A path that could overwrite it would
/// let whatever wrote second decide what the executor compares against, which
/// is the entire property this table exists to hold.
pub fn set_run_script_hash(
    conn: &Connection,
    run_id: &str,
    content_hash: &str,
) -> Result<(), RunsError> {
    conn.execute(
        "INSERT INTO run_script_hashes (run_id, content_hash) VALUES (?, ?)",
        params![run_id, content_hash],
    )
    .map(|_| ())
    .map_err(|source| RunsError::Sqlite {
        operation: "Set run script hash failed",
        source,
    })
}

/// The script bytes a run was authorized against, if any were recorded.
pub fn get_run_script_hash(conn: &Connection, run_id: &str) -> Result<Option<String>, RunsError> {
    conn.query_row(
        "SELECT content_hash FROM run_script_hashes WHERE run_id = ?",
        [run_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|source| RunsError::Sqlite {
        operation: "Query run script hash failed",
        source,
    })
}

use super::RunsError;
use crate::util::sqlite::{OPEN_RETRY_DELAYS, WalDatabase};
use crate::workspace::Workspace;
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

const RUNS_DATABASE: WalDatabase = WalDatabase {
    name: "runs",
    busy_timeout: Duration::from_millis(2_000),
    wal_retry_delays: &OPEN_RETRY_DELAYS,
};

static RUNS_OPEN_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn runs_open_lock() -> &'static Mutex<()> {
    &RUNS_OPEN_LOCK
}

// ---------------------------------------------------------------------------
// Open / schema
// ---------------------------------------------------------------------------

/// Open the run-log database for `workspace`, creating it and its schema if
/// necessary.
pub fn open(workspace: &Workspace) -> Result<Connection, RunsError> {
    let history_dir = workspace.history_dir();
    fs::create_dir_all(history_dir).map_err(|source| RunsError::Filesystem {
        operation: "Create history dir failed",
        source,
    })?;
    let _open_guard = runs_open_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let db_path = runs_db_path(workspace);
    let conn = open_connection_inner(&db_path)?;
    init_schema(&conn)?;
    Ok(conn)
}

/// Path to the SQLite run log inside `workspace`.
pub fn runs_db_path(workspace: &Workspace) -> PathBuf {
    workspace.history_dir().join("runs.sqlite")
}

#[cfg(test)]
pub(super) fn open_connection(db_path: &Path) -> Result<Connection, RunsError> {
    let _open_guard = runs_open_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_connection_inner(db_path)
}

fn open_connection_inner(db_path: &Path) -> Result<Connection, RunsError> {
    let conn = RUNS_DATABASE
        .open(db_path)
        .map_err(RunsError::DatabaseOpen)?;
    // ON DELETE CASCADE on run_traces requires foreign keys to be enforced
    // explicitly: SQLite ships with foreign_keys=OFF for backward
    // compatibility.
    conn.execute_batch("PRAGMA foreign_keys = ON")
        .map_err(|source| RunsError::Sqlite {
            operation: "Enable foreign keys failed",
            source,
        })?;
    Ok(conn)
}

/// Initialize the `runs` and `run_traces` tables and indexes. Idempotent
/// (uses `CREATE TABLE IF NOT EXISTS`), so safe to call on every open.
pub fn init_schema(conn: &Connection) -> Result<(), RunsError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS runs (
            run_id TEXT PRIMARY KEY,
            script_path TEXT NOT NULL,
            script_name TEXT,
            args_json TEXT NOT NULL,
            actor TEXT NOT NULL,
            reason TEXT,
            state TEXT NOT NULL,
            priority INTEGER NOT NULL DEFAULT 0,
            enqueued_at INTEGER NOT NULL,
            worker_id TEXT,
            lease_until INTEGER,
            timeout_ms INTEGER,
            cron_schedule_id TEXT,
            trigger TEXT NOT NULL DEFAULT 'Manual',
            started_at INTEGER,
            finished_at INTEGER,
            duration_ms INTEGER,
            exit_code INTEGER,
            success INTEGER,
            stdout TEXT NOT NULL DEFAULT '',
            stderr TEXT NOT NULL DEFAULT '',
            error TEXT,
            parent_run_id TEXT,
            omakure_version TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS run_envs (
            run_id TEXT PRIMARY KEY,
            env_name TEXT NOT NULL,
            FOREIGN KEY(run_id) REFERENCES runs(run_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS run_secret_refs (
            run_id TEXT NOT NULL,
            secret_ref TEXT NOT NULL,
            PRIMARY KEY(run_id, secret_ref),
            FOREIGN KEY(run_id) REFERENCES runs(run_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS run_script_hashes (
            run_id TEXT PRIMARY KEY,
            content_hash TEXT NOT NULL,
            FOREIGN KEY(run_id) REFERENCES runs(run_id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS workflow_runs (
            workflow_id TEXT PRIMARY KEY,
            battery_id TEXT NOT NULL,
            battery_version TEXT NOT NULL,
            battery_commit TEXT NOT NULL,
            workflow_name TEXT NOT NULL,
            actor TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('running', 'completed', 'failed', 'cancelled')),
            current_step INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            finished_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS workflow_steps (
            workflow_id TEXT NOT NULL,
            step_index INTEGER NOT NULL,
            name TEXT NOT NULL,
            script_path TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            run_id TEXT UNIQUE,
            PRIMARY KEY(workflow_id, step_index),
            FOREIGN KEY(workflow_id) REFERENCES workflow_runs(workflow_id) ON DELETE CASCADE,
            FOREIGN KEY(run_id) REFERENCES runs(run_id)
        );
        CREATE INDEX IF NOT EXISTS idx_workflow_runs_state ON workflow_runs(state);
        CREATE INDEX IF NOT EXISTS idx_runs_started_at ON runs(started_at DESC);
        CREATE INDEX IF NOT EXISTS idx_runs_script_path ON runs(script_path);
        CREATE INDEX IF NOT EXISTS idx_runs_actor ON runs(actor);
        CREATE INDEX IF NOT EXISTS idx_runs_state ON runs(state);
        CREATE INDEX IF NOT EXISTS idx_runs_state_priority_enqueued
            ON runs(state, priority DESC, enqueued_at ASC);
        CREATE INDEX IF NOT EXISTS idx_runs_cron_schedule
            ON runs(cron_schedule_id, enqueued_at DESC);

        CREATE TABLE IF NOT EXISTS run_traces (
            trace_id INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            sequence INTEGER NOT NULL,
            level TEXT NOT NULL,
            message TEXT NOT NULL,
            data_json TEXT,
            FOREIGN KEY(run_id) REFERENCES runs(run_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_traces_run_id ON run_traces(run_id, sequence);",
    )
    .map_err(|source| RunsError::Sqlite {
        operation: "Init runs db failed",
        source,
    })
}

//! SQLite-backed run history with a state machine and structured trace stream.
//!
//! `runs.rs` is the **only** code path that persists script execution
//! history. The legacy `history.rs` JSON-file format has been removed; there
//! is no shim, no fallback, and no migration.
//!
//! On first open against a workspace, two destructive cleanups run:
//!
//! 1. Every top-level `*.json` file in `<workspace>/.history/` is unlinked
//!    (legacy cleanup from the v0.1 AI surface release).
//! 2. If the existing `runs` table lacks the `state` column (i.e. it was
//!    written by the v0.1 schema), the table is dropped and recreated with
//!    the new schema. The state-machine release ships with zero
//!    released-version users on the v0.1 schema, so this destructive
//!    rebuild is acceptable.
//!
//! Subdirectories and other files (notably `runs.sqlite` itself and
//! `search-index.sqlite`) are left untouched by the legacy cleanup. The
//! schema rebuild only drops and recreates the `runs` and `run_traces`
//! tables — not the database file.

use crate::workspace::Workspace;
use rusqlite::{
    params, params_from_iter, Connection, ErrorCode, OptionalExtension, TransactionBehavior,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Internal heartbeat lease duration in milliseconds (60 s).
///
/// This is **not** a job timeout. It only governs how long a worker holds a
/// claim before another worker may steal the row. The user-facing per-job
/// `--timeout` is independent of this value.
pub const HEARTBEAT_MS: i64 = 60_000;

// ---------------------------------------------------------------------------
// RunState enum
// ---------------------------------------------------------------------------

/// Final, closed set of legal values for the `runs.state` column.
///
/// Adding a new variant is a deliberate breaking change to the AI contract.
/// The state set is intentionally small and has no `paused`, `retrying`,
/// `scheduled`, `expired`, `zombie`, or `blocked` member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    DeadLetter,
}

impl RunState {
    /// Stable string representation written into the `state` column and
    /// returned in JSON envelopes. Renaming any of these strings is a
    /// breaking change.
    pub fn as_str(&self) -> &'static str {
        match self {
            RunState::Queued => "queued",
            RunState::Running => "running",
            RunState::Completed => "completed",
            RunState::Failed => "failed",
            RunState::Cancelled => "cancelled",
            RunState::TimedOut => "timed_out",
            RunState::DeadLetter => "dead_letter",
        }
    }

    /// True for any state from which no further transition is allowed
    /// (apart from the explicit `failed|timed_out -> dead_letter` promotion
    /// handled by [`dead_letter`]).
    #[allow(dead_code)] // exposed for future trigger-rule callers
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunState::Completed
                | RunState::Failed
                | RunState::Cancelled
                | RunState::TimedOut
                | RunState::DeadLetter
        )
    }

    /// All seven legal values, in stable order.
    pub fn all() -> &'static [RunState] {
        &[
            RunState::Queued,
            RunState::Running,
            RunState::Completed,
            RunState::Failed,
            RunState::Cancelled,
            RunState::TimedOut,
            RunState::DeadLetter,
        ]
    }
}

impl fmt::Display for RunState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RunState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(RunState::Queued),
            "running" => Ok(RunState::Running),
            "completed" => Ok(RunState::Completed),
            "failed" => Ok(RunState::Failed),
            "cancelled" => Ok(RunState::Cancelled),
            "timed_out" => Ok(RunState::TimedOut),
            "dead_letter" => Ok(RunState::DeadLetter),
            other => Err(format!(
                "invalid run state '{}': expected one of queued, running, completed, failed, cancelled, timed_out, dead_letter",
                other
            )),
        }
    }
}

impl Serialize for RunState {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RunState {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Shorthand for groups of states used by `--state-set` on `history list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStateSet {
    InFlight,
    Terminal,
    All,
}

impl RunStateSet {
    pub fn to_states(self) -> Vec<RunState> {
        match self {
            RunStateSet::InFlight => vec![RunState::Queued, RunState::Running],
            RunStateSet::Terminal => vec![
                RunState::Completed,
                RunState::Failed,
                RunState::Cancelled,
                RunState::TimedOut,
                RunState::DeadLetter,
            ],
            RunStateSet::All => RunState::all().to_vec(),
        }
    }
}

impl FromStr for RunStateSet {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "in_flight" => Ok(RunStateSet::InFlight),
            "terminal" => Ok(RunStateSet::Terminal),
            "all" => Ok(RunStateSet::All),
            other => Err(format!(
                "invalid state-set '{}': expected one of in_flight, terminal, all",
                other
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// RunTrigger
// ---------------------------------------------------------------------------

/// Provenance of a run row: did a human launch it, did the scheduler, or did an
/// authorized Conductor?
///
/// `Cue` is not cosmetic. It is the discriminator that keeps a remotely
/// initiated run out of the lease-steal path, and without it the Health Plane
/// reports such a run as `manual` — a false audit record in exactly the feature
/// whose purpose is distributed audit outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RunTrigger {
    #[default]
    Manual,
    Scheduled,
    Cue,
}

impl RunTrigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunTrigger::Manual => "Manual",
            RunTrigger::Scheduled => "Scheduled",
            RunTrigger::Cue => "Cue",
        }
    }
}

impl fmt::Display for RunTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RunTrigger {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Manual" => Ok(RunTrigger::Manual),
            "Scheduled" => Ok(RunTrigger::Scheduled),
            "Cue" => Ok(RunTrigger::Cue),
            other => Err(format!(
                "invalid run trigger '{}': expected Manual, Scheduled or Cue",
                other
            )),
        }
    }
}

impl Serialize for RunTrigger {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RunTrigger {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

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
/// ordered by `started_at DESC` (the v0.1 default), so existing callers
/// keep their previous semantics.
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
    /// is returned. Default value is the [`RunStateSet::Terminal`] set so
    /// that callers from v0.1 keep their "completed runs only" behavior.
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
// Trace types
// ---------------------------------------------------------------------------

/// Allowed `--level` values for [`omakure trace`](crate::cli::trace).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TraceLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl TraceLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            TraceLevel::Debug => "debug",
            TraceLevel::Info => "info",
            TraceLevel::Warn => "warn",
            TraceLevel::Error => "error",
        }
    }
}

impl FromStr for TraceLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "debug" => Ok(TraceLevel::Debug),
            "info" => Ok(TraceLevel::Info),
            "warn" => Ok(TraceLevel::Warn),
            "error" => Ok(TraceLevel::Error),
            other => Err(format!(
                "invalid trace level '{}': expected one of debug, info, warn, error",
                other
            )),
        }
    }
}

/// One row of the `run_traces` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceRow {
    pub trace_id: i64,
    pub run_id: String,
    pub timestamp: i64,
    pub sequence: i64,
    pub level: String,
    pub message: String,
    pub data_json: Option<String>,
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

// ---------------------------------------------------------------------------
// Open / schema
// ---------------------------------------------------------------------------

/// Open the run-log database for `workspace`, creating it if necessary,
/// running any pending schema setup, and (on first open against a workspace
/// that still contains legacy state) cleaning it up.
///
/// Two destructive cleanups may run on first open:
///
/// 1. Every top-level `*.json` file under `<workspace>/.history/` is
///    deleted (legacy v0.1 cleanup).
/// 2. If the existing `runs` table lacks the `state` column it is dropped
///    and recreated with the new schema.
pub fn open(workspace: &Workspace) -> Result<Connection, String> {
    let history_dir = workspace.history_dir();
    fs::create_dir_all(history_dir).map_err(|err| format!("Create history dir failed: {}", err))?;
    cleanup_legacy_json_files(history_dir);
    let db_path = runs_db_path(workspace);
    let conn = open_connection(&db_path)?;
    rebuild_legacy_schema_if_needed(&conn)?;
    init_schema(&conn)?;
    Ok(conn)
}

/// Path to the SQLite run log inside `workspace`.
pub fn runs_db_path(workspace: &Workspace) -> PathBuf {
    workspace.history_dir().join("runs.sqlite")
}

fn open_connection(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Create runs db folder failed: {}", err))?;
    }
    let conn = Connection::open(db_path).map_err(|err| format!("Open runs db failed: {}", err))?;
    conn.busy_timeout(std::time::Duration::from_millis(2_000))
        .map_err(|err| format!("Runs db busy timeout failed: {}", err))?;
    let _journal_mode: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .map_err(|err| format!("Enable WAL failed: {}", err))?;
    // ON DELETE CASCADE on run_traces requires foreign keys to be enforced
    // explicitly: SQLite ships with foreign_keys=OFF for backward
    // compatibility.
    conn.execute_batch("PRAGMA foreign_keys = ON")
        .map_err(|err| format!("Enable foreign keys failed: {}", err))?;
    Ok(conn)
}

/// Detect a v0.1-shaped `runs` table (no `state` column) and drop it so
/// [`init_schema`] can recreate it with the new layout. Idempotent: if
/// the table is already on the new shape, this is a no-op.
fn rebuild_legacy_schema_if_needed(conn: &Connection) -> Result<(), String> {
    let has_runs_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='runs'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|err| format!("Inspect runs table failed: {}", err))?
        .is_some();
    if !has_runs_table {
        return Ok(());
    }
    let mut stmt = conn
        .prepare("PRAGMA table_info(runs)")
        .map_err(|err| format!("Inspect runs schema failed: {}", err))?;
    let mut has_state_column = false;
    let mut has_trigger_column = false;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|err| format!("Read runs schema rows failed: {}", err))?;
    for col in rows {
        let name = col.map_err(|err| format!("Read schema row failed: {}", err))?;
        match name.as_str() {
            "state" => has_state_column = true,
            "trigger" => has_trigger_column = true,
            _ => {}
        }
    }
    drop(stmt);
    if !has_state_column || !has_trigger_column {
        eprintln!(
            "omakure: rebuilding runs.sqlite schema (legacy layout detected; existing rows will be dropped)"
        );
        conn.execute_batch("DROP TABLE IF EXISTS run_traces; DROP TABLE IF EXISTS runs;")
            .map_err(|err| format!("Drop legacy runs table failed: {}", err))?;
    }
    Ok(())
}

/// Initialize the `runs` and `run_traces` tables and indexes. Idempotent
/// (uses `CREATE TABLE IF NOT EXISTS`), so safe to call on every open.
pub fn init_schema(conn: &Connection) -> Result<(), String> {
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
    .map_err(|err| format!("Init runs db failed: {}", err))
}

// ---------------------------------------------------------------------------
// Generic insert / read
// ---------------------------------------------------------------------------

/// Insert a fully-formed run row. The caller is responsible for generating
/// `run_id` (typically via [`generate_run_id`]) and setting `state` to a
/// legal value. Used by [`enqueue`] and [`start_inline`] internally and
/// remains exposed for tests / future use.
pub fn insert_run(conn: &Connection, row: &RunRow) -> Result<(), String> {
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
    .map_err(|err| format!("Insert run failed: {}", err))?;
    Ok(())
}

/// Fetch one run by id, or `None` if it does not exist.
pub fn get_run(conn: &Connection, run_id: &str) -> Result<Option<RunRow>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT run_id, script_path, script_name, args_json, actor, reason,
                    state, priority, enqueued_at, worker_id, lease_until, timeout_ms,
                    cron_schedule_id, trigger,
                    started_at, finished_at, duration_ms, exit_code, success,
                    stdout, stderr, error, parent_run_id, omakure_version
             FROM runs WHERE run_id = ?",
        )
        .map_err(|err| format!("Prepare get_run failed: {}", err))?;
    let row = stmt
        .query_row([run_id], row_to_run)
        .optional()
        .map_err(|err| format!("Query get_run failed: {}", err))?;
    Ok(row)
}

/// Query rows matching the supplied filters. In-flight rows are surfaced
/// first (by `enqueued_at DESC` so the most recently queued / running rows
/// appear at the top), then terminal rows by `started_at DESC`.
pub fn query_runs(conn: &Connection, filters: &RunFilters) -> Result<Vec<RunRow>, String> {
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

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| format!("Prepare query_runs failed: {}", err))?;
    let rows = stmt
        .query_map(
            params_from_iter(params.iter().map(|p| p.as_ref())),
            row_to_run,
        )
        .map_err(|err| format!("Query query_runs failed: {}", err))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|err| format!("Row query_runs failed: {}", err))?);
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

/// Insert a fresh `state='queued'` row. Returns the inserted [`RunRow`].
pub fn enqueue(
    conn: &Connection,
    script_path: &str,
    args: &[String],
    opts: EnqueueOptions,
) -> Result<RunRow, String> {
    let now = current_unix_ms();
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

/// Insert a row directly in `state='running'`, skipping the queued step.
/// Used by the synchronous `omakure run` fast path so the row is visible
/// to `history list --state running` immediately.
pub fn start_inline(
    conn: &Connection,
    script_path: &str,
    args: &[String],
    worker_id: &str,
    opts: EnqueueOptions,
) -> Result<RunRow, String> {
    let now = current_unix_ms();
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

pub fn set_run_env(conn: &Connection, run_id: &str, env_name: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO run_envs (run_id, env_name) VALUES (?, ?) \
         ON CONFLICT(run_id) DO UPDATE SET env_name = excluded.env_name",
        params![run_id, env_name],
    )
    .map_err(|err| format!("Set run env failed: {}", err))?;
    Ok(())
}

pub fn get_run_env(conn: &Connection, run_id: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT env_name FROM run_envs WHERE run_id = ?",
        [run_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|err| format!("Get run env failed: {}", err))
}

pub fn set_run_secret_refs(conn: &Connection, run_id: &str, refs: &[String]) -> Result<(), String> {
    conn.execute("DELETE FROM run_secret_refs WHERE run_id = ?", [run_id])
        .map_err(|err| format!("Clear run secret refs failed: {}", err))?;
    if refs.is_empty() {
        conn.execute(
            "INSERT OR IGNORE INTO run_secret_refs (run_id, secret_ref) VALUES (?, '')",
            [run_id],
        )
        .map_err(|err| format!("Set run secret ref policy failed: {}", err))?;
        return Ok(());
    }
    for secret_ref in refs {
        conn.execute(
            "INSERT OR IGNORE INTO run_secret_refs (run_id, secret_ref) VALUES (?, ?)",
            params![run_id, secret_ref],
        )
        .map_err(|err| format!("Set run secret ref failed: {}", err))?;
    }
    Ok(())
}

pub fn get_run_secret_refs(conn: &Connection, run_id: &str) -> Result<Option<Vec<String>>, String> {
    let mut stmt = conn
        .prepare("SELECT secret_ref FROM run_secret_refs WHERE run_id = ? ORDER BY secret_ref")
        .map_err(|err| format!("Prepare run secret refs failed: {}", err))?;
    let rows = stmt
        .query_map([run_id], |row| row.get(0))
        .map_err(|err| format!("Query run secret refs failed: {}", err))?;
    let mut refs = Vec::new();
    let mut has_policy = false;
    for row in rows {
        let secret_ref: String =
            row.map_err(|err| format!("Row run secret refs failed: {}", err))?;
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
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO run_script_hashes (run_id, content_hash) VALUES (?, ?)",
        params![run_id, content_hash],
    )
    .map(|_| ())
    .map_err(|err| format!("Set run script hash failed: {}", err))
}

/// The script bytes a run was authorized against, if any were recorded.
pub fn get_run_script_hash(conn: &Connection, run_id: &str) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT content_hash FROM run_script_hashes WHERE run_id = ?",
        [run_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|err| format!("Query run script hash failed: {}", err))
}

/// Filters used by [`claim_next`] to scope a worker to a subset of jobs.
#[derive(Debug, Clone, Default)]
pub struct ClaimFilters {
    pub actor: Option<String>,
    pub script: Option<String>,
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
) -> Result<Option<RunRow>, String> {
    let now = current_unix_ms();
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
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|err| format!("Prepare claim_next failed: {}", err))?;
        let params_ref: Vec<(&str, &dyn rusqlite::ToSql)> = named_params
            .iter()
            .map(|(name, value)| (*name, value.as_ref()))
            .collect();
        stmt.query_row(&params_ref[..], |row| row.get::<_, String>(0))
            .optional()
            .map_err(|err| format!("Claim next failed: {}", err))?
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
) -> Result<Option<RunState>, String> {
    let now = current_unix_ms();
    let updated = conn
        .execute(
            "UPDATE runs
                SET lease_until = ?
              WHERE run_id = ? AND worker_id = ? AND state = 'running'",
            params![now + HEARTBEAT_MS, run_id, worker_id],
        )
        .map_err(|err| format!("Heartbeat failed: {}", err))?;
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
) -> Result<(), String> {
    let now = current_unix_ms();
    // Look up the row first so we can compute duration_ms relative to its
    // started_at and reject illegal transitions.
    let row = get_run(conn, run_id)?.ok_or_else(|| format!("run not found: {}", run_id))?;
    if !matches!(row.state, RunState::Running) {
        return Err(format!(
            "illegal transition: cannot move {} -> {}; row must be in 'running'",
            row.state.as_str(),
            target.as_str()
        ));
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
    .map_err(|err| format!("Finalize run failed: {}", err))?;
    Ok(())
}

/// Mark a `running` row as `completed`.
pub fn complete(conn: &Connection, run_id: &str, completion: RunCompletion) -> Result<(), String> {
    finalize(conn, run_id, RunState::Completed, &completion)
}

/// Mark a `running` row as `failed`.
pub fn fail(conn: &Connection, run_id: &str, completion: RunCompletion) -> Result<(), String> {
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
pub fn recover_abandoned_cue_runs(conn: &Connection) -> Result<Vec<String>, String> {
    let now = current_unix_ms();
    let mut statement = conn
        .prepare(
            "SELECT run_id FROM runs
              WHERE state = 'running'
                AND trigger = ?1
                AND lease_until IS NOT NULL
                AND lease_until < ?2",
        )
        .map_err(|err| format!("Prepare cue recovery failed: {}", err))?;
    let ids: Vec<String> = statement
        .query_map(params![RunTrigger::Cue.as_str(), now], |row| row.get(0))
        .map_err(|err| format!("Query cue recovery failed: {}", err))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|err| format!("Read cue recovery failed: {}", err))?;
    drop(statement);

    for run_id in &ids {
        finalize(
            conn,
            run_id,
            RunState::Failed,
            &RunCompletion {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: None,
                success: false,
                error: Some(
                    "the worker holding this remote run stopped; it was not re-run because a \
                     remote instruction must execute at most once"
                        .to_string(),
                ),
            },
        )?;
    }
    Ok(ids)
}

/// Cancel every unfinished Cue-origin run this peer caused.
///
/// Withdrawing trust must reach work already in flight, not just future
/// instructions. Cancelling the row is the whole mechanism: the executor's
/// heartbeat already kills the child as soon as the row leaves `running`, so
/// there is no new cancel plumbing and no second way to stop a run.
///
/// Scoped to `trigger = 'cue'` on purpose. A revoked peer's name may also
/// appear on locally-initiated work, and revoking a peer is not a licence to
/// cancel what this node's own owner started.
pub fn cancel_cue_runs_for_actor(conn: &Connection, actor: &str) -> Result<Vec<String>, String> {
    let mut statement = conn
        .prepare(
            "SELECT run_id FROM runs
              WHERE trigger = ?1
                AND actor = ?2
                AND state IN ('queued', 'running')",
        )
        .map_err(|err| format!("Prepare cue revocation failed: {}", err))?;
    let ids: Vec<String> = statement
        .query_map(params![RunTrigger::Cue.as_str(), actor], |row| row.get(0))
        .map_err(|err| format!("Query cue revocation failed: {}", err))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|err| format!("Read cue revocation failed: {}", err))?;
    drop(statement);

    let mut cancelled = Vec::new();
    for run_id in ids {
        if cancel(
            conn,
            &run_id,
            Some("the peer that asked for this run was revoked".to_string()),
            None,
        )
        .is_ok()
        {
            cancelled.push(run_id);
        }
    }
    Ok(cancelled)
}

/// Mark a `running` row as `timed_out` after the worker killed the
/// process for exceeding its `--timeout`.
pub fn time_out(conn: &Connection, run_id: &str, completion: RunCompletion) -> Result<(), String> {
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
) -> Result<RunRow, String> {
    let row = get_run(conn, run_id)?.ok_or_else(|| format!("run not found: {}", run_id))?;
    let now = current_unix_ms();
    match row.state {
        RunState::Queued => {
            conn.execute(
                "UPDATE runs
                    SET state = 'cancelled', finished_at = ?, duration_ms = 0, success = 0,
                        reason = COALESCE(?, reason)
                  WHERE run_id = ?",
                params![now, reason, run_id],
            )
            .map_err(|err| format!("Cancel queued run failed: {}", err))?;
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
            .map_err(|err| format!("Cancel running run failed: {}", err))?;
        }
        terminal => {
            return Err(format!(
                "cannot cancel run in terminal state '{}'",
                terminal.as_str()
            ));
        }
    }
    get_run(conn, run_id)?.ok_or_else(|| format!("run not found after cancel: {}", run_id))
}

/// Mark a `cancelled` row produced by mid-execution cancel as needing
/// the worker's captured output. Used internally by the worker after it
/// detects an external cancel via [`heartbeat`] and kills the script.
pub fn record_cancelled_output(
    conn: &Connection,
    run_id: &str,
    completion: RunCompletion,
) -> Result<(), String> {
    let row = get_run(conn, run_id)?.ok_or_else(|| format!("run not found: {}", run_id))?;
    let now = current_unix_ms();
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
    .map_err(|err| format!("Record cancelled output failed: {}", err))?;
    Ok(())
}

/// Promote a `failed` or `timed_out` row into `dead_letter`. Any other
/// state is rejected.
pub fn dead_letter(
    conn: &Connection,
    run_id: &str,
    reason: Option<String>,
) -> Result<RunRow, String> {
    let row = get_run(conn, run_id)?.ok_or_else(|| format!("run not found: {}", run_id))?;
    if !matches!(row.state, RunState::Failed | RunState::TimedOut) {
        return Err(format!(
            "cannot promote run in state '{}' to dead_letter; only failed or timed_out rows are eligible",
            row.state.as_str()
        ));
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
    .map_err(|err| format!("Dead-letter run failed: {}", err))?;
    get_run(conn, run_id)?.ok_or_else(|| format!("run not found after dead_letter: {}", run_id))
}

/// Aggregate counts per state and per actor.
pub fn stats(conn: &Connection) -> Result<RunStats, String> {
    let mut counts_by_state: HashMap<String, i64> = HashMap::new();
    let mut counts_by_actor: HashMap<String, i64> = HashMap::new();
    let mut total: i64 = 0;

    let mut state_stmt = conn
        .prepare("SELECT state, COUNT(*) FROM runs GROUP BY state")
        .map_err(|err| format!("Prepare state stats failed: {}", err))?;
    let state_rows = state_stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|err| format!("Query state stats failed: {}", err))?;
    for entry in state_rows {
        let (state, count) = entry.map_err(|err| format!("Read state row: {}", err))?;
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
        .map_err(|err| format!("Prepare actor stats failed: {}", err))?;
    let actor_rows = actor_stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|err| format!("Query actor stats failed: {}", err))?;
    for entry in actor_rows {
        let (actor, count) = entry.map_err(|err| format!("Read actor row: {}", err))?;
        counts_by_actor.insert(actor, count);
    }

    Ok(RunStats {
        counts_by_state,
        counts_by_actor,
        total,
    })
}

// ---------------------------------------------------------------------------
// Trace storage
// ---------------------------------------------------------------------------

const TRACE_RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(25), Duration::from_millis(100)];

enum TraceInsertError {
    NotFound(String),
    Sqlite {
        operation: &'static str,
        error: rusqlite::Error,
    },
}

impl TraceInsertError {
    fn is_retryable(&self) -> bool {
        matches!(self, Self::Sqlite { error, .. }
        if matches!(
            error.sqlite_error_code(),
            Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
        ))
    }

    fn into_message(self) -> String {
        match self {
            Self::NotFound(run_id) => format!("not_found: {run_id}"),
            Self::Sqlite { operation, error } => format!("{operation}: {error}"),
        }
    }
}

fn insert_trace_once(
    conn: &mut Connection,
    run_id: &str,
    level: TraceLevel,
    message: &str,
    data_json: Option<&str>,
) -> Result<TraceRow, TraceInsertError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| TraceInsertError::Sqlite {
            operation: "Begin trace tx failed",
            error,
        })?;

    let exists: bool = tx
        .query_row(
            "SELECT 1 FROM runs WHERE run_id = ? LIMIT 1",
            params![run_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| TraceInsertError::Sqlite {
            operation: "Lookup run for trace failed",
            error,
        })?
        .is_some();
    if !exists {
        return Err(TraceInsertError::NotFound(run_id.to_string()));
    }

    let next_seq: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM run_traces WHERE run_id = ?",
            params![run_id],
            |row| row.get(0),
        )
        .map_err(|error| TraceInsertError::Sqlite {
            operation: "Compute next sequence failed",
            error,
        })?;
    let now = current_unix_ms();
    tx.execute(
        "INSERT INTO run_traces (run_id, timestamp, sequence, level, message, data_json)
             VALUES (?,?,?,?,?,?)",
        params![run_id, now, next_seq, level.as_str(), message, data_json],
    )
    .map_err(|error| TraceInsertError::Sqlite {
        operation: "Insert trace failed",
        error,
    })?;
    let trace_id = tx.last_insert_rowid();
    tx.commit().map_err(|error| TraceInsertError::Sqlite {
        operation: "Commit trace tx failed",
        error,
    })?;

    Ok(TraceRow {
        trace_id,
        run_id: run_id.to_string(),
        timestamp: now,
        sequence: next_seq,
        level: level.as_str().to_string(),
        message: message.to_string(),
        data_json: data_json.map(|s| s.to_string()),
    })
}

/// Insert one trace event tied to `run_id`. Assigns a monotonic per-run
/// `sequence` inside a SQLite transaction so two concurrent inserts for
/// the same run never collide. Returns the newly inserted [`TraceRow`].
///
/// A busy/locked transaction is retried twice with short backoff after the
/// connection's normal busy timeout. Each retry starts a fresh transaction;
/// non-busy SQLite errors are returned immediately.
///
/// Returns an error message describing `not_found` when the parent run
/// does not exist; the CLI maps this to `error.code = "not_found"`.
pub fn insert_trace(
    conn: &mut Connection,
    run_id: &str,
    level: TraceLevel,
    message: &str,
    data_json: Option<&str>,
) -> Result<TraceRow, String> {
    let mut retry = 0;
    loop {
        match insert_trace_once(conn, run_id, level, message, data_json) {
            Ok(trace) => return Ok(trace),
            Err(error) if error.is_retryable() && retry < TRACE_RETRY_DELAYS.len() => {
                std::thread::sleep(TRACE_RETRY_DELAYS[retry]);
                retry += 1;
            }
            Err(error) => return Err(error.into_message()),
        }
    }
}

/// Query trace rows for `run_id`, ordered by `sequence ASC`.
///
/// `level_min` filters to entries whose level is >= the supplied minimum
/// (e.g. `Warn` returns warn and error). `since_sequence` returns only
/// entries with `sequence > since_sequence`.
///
/// Returns an `Err("not_found: ...")` when the parent run does not exist.
pub fn query_traces(
    conn: &Connection,
    run_id: &str,
    level_min: Option<TraceLevel>,
    since_sequence: Option<i64>,
) -> Result<Vec<TraceRow>, String> {
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM runs WHERE run_id = ? LIMIT 1",
            params![run_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|err| format!("Lookup run for traces failed: {}", err))?
        .is_some();
    if !exists {
        return Err(format!("not_found: {}", run_id));
    }

    let mut sql = String::from(
        "SELECT trace_id, run_id, timestamp, sequence, level, message, data_json
           FROM run_traces WHERE run_id = ?",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(run_id.to_string())];
    if let Some(min) = level_min {
        // Compare against the SQL-stored level via a CASE expression so we
        // do not have to maintain numeric ranks in the schema.
        sql.push_str(
            " AND CASE level
                       WHEN 'debug' THEN 0
                       WHEN 'info'  THEN 1
                       WHEN 'warn'  THEN 2
                       WHEN 'error' THEN 3
                       ELSE 1
                  END >= ?",
        );
        params.push(Box::new(min as i64));
    }
    if let Some(since) = since_sequence {
        sql.push_str(" AND sequence > ?");
        params.push(Box::new(since));
    }
    sql.push_str(" ORDER BY sequence ASC");

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| format!("Prepare query_traces failed: {}", err))?;
    let rows = stmt
        .query_map(params_from_iter(params.iter().map(|p| p.as_ref())), |row| {
            Ok(TraceRow {
                trace_id: row.get(0)?,
                run_id: row.get(1)?,
                timestamp: row.get(2)?,
                sequence: row.get(3)?,
                level: row.get(4)?,
                message: row.get(5)?,
                data_json: row.get(6)?,
            })
        })
        .map_err(|err| format!("Query traces failed: {}", err))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|err| format!("Trace row failed: {}", err))?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Misc helpers
// ---------------------------------------------------------------------------

/// Generate a synthetic, sortable run id of the form
/// `<unix_ms>-<pid>-<counter>`.
///
/// The counter is process-local and monotonic so two ids generated within
/// the same millisecond by the same process never collide. Across
/// processes, the `<pid>` segment provides uniqueness.
pub fn generate_run_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ms = current_unix_ms();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{}-{}", ms, std::process::id(), counter)
}

/// Current Unix time in milliseconds. Saturates at 0 if the system clock
/// is set to a value before 1970.
pub fn current_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Format a Unix-millisecond timestamp as `YYYY-MM-DD HH:MM` (UTC).
/// Used by history CLI and API consumers.
pub fn format_run_timestamp(timestamp_ms: i64) -> String {
    let mut ms = timestamp_ms;
    if ms < 0 {
        ms = 0;
    }
    let seconds = ms / 1000;
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;

    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        year, month, day, hour, minute
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

/// Delete every top-level `*.json` file inside `history_dir` and ignore
/// errors. Subdirectories and non-`.json` files are left untouched.
fn cleanup_legacy_json_files(history_dir: &Path) {
    let entries = match fs::read_dir(history_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let _ = fs::remove_file(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_workspace(label: &str) -> Workspace {
        let dir = std::env::temp_dir().join(format!(
            "omakure_runs_test_{}_{}_{}_{}",
            label,
            std::process::id(),
            current_unix_ms(),
            // Local atomic disambiguates two helpers spinning up workspaces
            // in the same millisecond.
            unique_seq()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp workspace");
        let ws = Workspace::new(dir);
        ws.ensure_layout().expect("ensure layout");
        ws
    }

    fn unique_seq() -> u64 {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    fn enqueue_opts() -> EnqueueOptions {
        EnqueueOptions {
            actor: "human".into(),
            omakure_version: "test".into(),
            ..Default::default()
        }
    }

    fn ok_completion() -> RunCompletion {
        RunCompletion {
            stdout: "out".into(),
            stderr: "".into(),
            exit_code: Some(0),
            success: true,
            error: None,
        }
    }

    fn fail_completion() -> RunCompletion {
        RunCompletion {
            stdout: "".into(),
            stderr: "boom".into(),
            exit_code: Some(2),
            success: false,
            error: None,
        }
    }

    // -----------------------------------------------------------------
    // Schema
    // -----------------------------------------------------------------

    #[test]
    fn open_creates_db_with_state_column() {
        let ws = unique_workspace("open_creates");
        let conn = open(&ws).expect("open");
        let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
        let loaded = get_run(&conn, &row.run_id).unwrap().unwrap();
        assert_eq!(loaded.state, RunState::Queued);
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn open_recreates_table_when_legacy_schema_detected() {
        let ws = unique_workspace("legacy_rebuild");
        // Manually create a legacy v0.1 table layout: no `state` column.
        let db_path = runs_db_path(&ws);
        {
            let conn = Connection::open(&db_path).expect("open legacy");
            conn.execute_batch(
                "CREATE TABLE runs (
                    run_id TEXT PRIMARY KEY,
                    script_path TEXT NOT NULL,
                    args_json TEXT NOT NULL,
                    actor TEXT NOT NULL,
                    started_at INTEGER NOT NULL,
                    finished_at INTEGER NOT NULL,
                    duration_ms INTEGER NOT NULL,
                    success INTEGER NOT NULL,
                    omakure_version TEXT NOT NULL
                );
                INSERT INTO runs VALUES('legacy', '/x/a.sh', '[]', 'human', 1, 2, 1, 1, 'old');",
            )
            .expect("seed legacy");
        }
        let conn = open(&ws).expect("open after legacy detection");
        // The legacy row should be gone.
        assert!(get_run(&conn, "legacy").unwrap().is_none());
        // The new schema must have the state column.
        let row = enqueue(&conn, "/x/b.sh", &[], enqueue_opts()).unwrap();
        let loaded = get_run(&conn, &row.run_id).unwrap().unwrap();
        assert_eq!(loaded.state, RunState::Queued);
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn open_idempotent_on_new_schema() {
        let ws = unique_workspace("open_idempotent");
        let _ = open(&ws).expect("first open");
        let conn = open(&ws).expect("second open");
        let rows = query_runs(&conn, &RunFilters::default()).unwrap();
        assert!(rows.is_empty());
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn open_deletes_top_level_json_files_only() {
        let ws = unique_workspace("cleanup_legacy");
        let history = ws.history_dir().to_path_buf();
        fs::write(history.join("legit.json"), "{}").unwrap();
        fs::write(history.join("keep.txt"), "keep").unwrap();
        fs::create_dir_all(history.join("subdir")).unwrap();
        fs::write(history.join("subdir").join("nested.json"), "{}").unwrap();

        let _conn = open(&ws).expect("open");

        assert!(!history.join("legit.json").exists());
        assert!(history.join("keep.txt").exists());
        assert!(history.join("subdir").join("nested.json").exists());
        assert!(history.join("runs.sqlite").exists());
        let _ = fs::remove_dir_all(ws.root());
    }

    // -----------------------------------------------------------------
    // RunState parsing
    // -----------------------------------------------------------------

    #[test]
    fn run_state_round_trip_through_str() {
        for state in RunState::all() {
            let parsed: RunState = state.as_str().parse().unwrap();
            assert_eq!(parsed, *state);
        }
    }

    #[test]
    fn run_state_invalid_string_returns_helpful_error() {
        let err = "paused".parse::<RunState>().unwrap_err();
        assert!(err.contains("invalid run state"));
        assert!(err.contains("queued"));
    }

    #[test]
    fn state_set_in_flight_and_terminal() {
        let in_flight = RunStateSet::InFlight.to_states();
        assert!(in_flight.contains(&RunState::Queued));
        assert!(in_flight.contains(&RunState::Running));
        let terminal = RunStateSet::Terminal.to_states();
        assert!(terminal.contains(&RunState::Completed));
        assert!(terminal.contains(&RunState::DeadLetter));
        assert!(!terminal.contains(&RunState::Queued));
    }

    // -----------------------------------------------------------------
    // Transitions
    // -----------------------------------------------------------------

    #[test]
    fn enqueue_then_claim_then_complete_happy_path() {
        let ws = unique_workspace("happy_path");
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
        let ws = unique_workspace("cue_revocation_cancels");
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
            rusqlite::params![current_unix_ms(), running.run_id],
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

    /// Without the exclusion this test fails by *succeeding* — `claim_next`
    /// hands the row back and the script runs a second time.
    #[test]
    fn a_crashed_cue_run_is_never_reclaimed_by_a_worker() {
        let ws = unique_workspace("cue_no_lease_steal");
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
            rusqlite::params![current_unix_ms() - HEARTBEAT_MS - 1, row.run_id],
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
        let ws = unique_workspace("cue_recovery");
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
                rusqlite::params![current_unix_ms() - HEARTBEAT_MS - 1, row.run_id],
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
        let ws = unique_workspace("cue_recovery_scope");
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
            rusqlite::params![current_unix_ms() - HEARTBEAT_MS - 1, queued.run_id],
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
        let ws = unique_workspace("queued_lease_steal");
        let conn = open(&ws).expect("open");

        let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
        claim_next(&conn, "w", &ClaimFilters::default())
            .unwrap()
            .unwrap();
        conn.execute(
            "UPDATE runs SET lease_until = ?1 WHERE run_id = ?2",
            rusqlite::params![current_unix_ms() - HEARTBEAT_MS - 1, row.run_id],
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
        let ws = unique_workspace("fail_path");
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
        let ws = unique_workspace("complete_rejects");
        let conn = open(&ws).expect("open");
        let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
        let err = complete(&conn, &row.run_id, ok_completion()).unwrap_err();
        assert!(err.contains("illegal transition"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn cancel_queued_transitions_to_cancelled_immediately() {
        let ws = unique_workspace("cancel_queued");
        let conn = open(&ws).expect("open");
        let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
        let after = cancel(&conn, &row.run_id, Some("ux".into()), None).unwrap();
        assert_eq!(after.state, RunState::Cancelled);
        assert_eq!(after.reason.as_deref(), Some("ux"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn cancel_running_transitions_to_cancelled() {
        let ws = unique_workspace("cancel_running");
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
        let ws = unique_workspace("cancel_terminal");
        let conn = open(&ws).expect("open");
        let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
        let _ = claim_next(&conn, "w", &ClaimFilters::default())
            .unwrap()
            .unwrap();
        complete(&conn, &row.run_id, ok_completion()).unwrap();
        let err = cancel(&conn, &row.run_id, None, None).unwrap_err();
        assert!(err.contains("terminal state"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn dead_letter_only_succeeds_on_failed_or_timed_out() {
        let ws = unique_workspace("dead_letter_paths");
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
        assert!(err.contains("only failed or timed_out"));

        let _ = fs::remove_dir_all(ws.root());
    }

    // -----------------------------------------------------------------
    // claim_next behavior
    // -----------------------------------------------------------------

    #[test]
    fn claim_next_returns_none_when_empty() {
        let ws = unique_workspace("claim_empty");
        let conn = open(&ws).expect("open");
        assert!(claim_next(&conn, "w", &ClaimFilters::default())
            .unwrap()
            .is_none());
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn claim_next_orders_by_priority_then_enqueued_at() {
        let ws = unique_workspace("claim_order");
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
        let ws = unique_workspace("claim_reclaim");
        let conn = open(&ws).expect("open");
        // Insert a row directly in `running` with an already-expired lease.
        let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
        conn.execute(
            "UPDATE runs SET state='running', worker_id='dead', started_at=?, lease_until=?
                WHERE run_id=?",
            params![
                current_unix_ms() - 100_000,
                current_unix_ms() - 50_000,
                row.run_id
            ],
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
        let ws = unique_workspace("claim_no_steal");
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
        let ws = unique_workspace("claim_actor");
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
        let ws = unique_workspace("hb_owner");
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
        let ws = unique_workspace("hb_cancel");
        let conn = open(&ws).expect("open");
        let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
        claim_next(&conn, "owner", &ClaimFilters::default()).unwrap();
        cancel(&conn, &row.run_id, Some("user".into()), None).unwrap();
        let state = heartbeat(&conn, &row.run_id, "owner").unwrap();
        assert_eq!(state, Some(RunState::Cancelled));
        let _ = fs::remove_dir_all(ws.root());
    }

    // -----------------------------------------------------------------
    // Filters
    // -----------------------------------------------------------------

    #[test]
    fn run_filters_default_returns_terminal_only() {
        let ws = unique_workspace("filters_default");
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
        let ws = unique_workspace("filters_all");
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
        let ws = unique_workspace("filters_running");
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
    fn stats_counts_per_state_and_actor() {
        let ws = unique_workspace("stats");
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

    // -----------------------------------------------------------------
    // Concurrency: claim_next under N threads
    // -----------------------------------------------------------------

    #[test]
    fn claim_next_under_concurrency_no_duplicates() {
        let ws = unique_workspace("concurrency");
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
                while let Some(row) =
                    claim_next(&conn, &worker_id, &ClaimFilters::default()).unwrap()
                {
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

    // -----------------------------------------------------------------
    // Trace storage
    // -----------------------------------------------------------------

    #[test]
    fn insert_trace_assigns_monotonic_sequence() {
        let ws = unique_workspace("trace_sequence");
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
        let ws = unique_workspace("trace_unknown");
        let mut conn = open(&ws).expect("open");
        let err = insert_trace(&mut conn, "missing", TraceLevel::Info, "x", None).unwrap_err();
        assert!(err.starts_with("not_found"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn insert_trace_under_concurrency_no_duplicates() {
        let ws = unique_workspace("trace_concurrent");
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
        let ws = unique_workspace("trace_busy_retry");
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
        let ws = unique_workspace("trace_level");
        let mut conn = open(&ws).expect("open");
        let row = enqueue(&conn, "/x/a.sh", &[], enqueue_opts()).unwrap();
        insert_trace(&mut conn, &row.run_id, TraceLevel::Debug, "d", None).unwrap();
        insert_trace(&mut conn, &row.run_id, TraceLevel::Info, "i", None).unwrap();
        insert_trace(&mut conn, &row.run_id, TraceLevel::Warn, "w", None).unwrap();
        insert_trace(&mut conn, &row.run_id, TraceLevel::Error, "e", None).unwrap();

        let warn_and_above =
            query_traces(&conn, &row.run_id, Some(TraceLevel::Warn), None).unwrap();
        let levels: Vec<&str> = warn_and_above.iter().map(|t| t.level.as_str()).collect();
        assert_eq!(levels, vec!["warn", "error"]);
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn query_traces_filters_by_since_sequence() {
        let ws = unique_workspace("trace_since");
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
        let ws = unique_workspace("trace_q_unknown");
        let conn = open(&ws).expect("open");
        let err = query_traces(&conn, "missing", None, None).unwrap_err();
        assert!(err.starts_with("not_found"));
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn delete_run_cascades_to_traces() {
        let ws = unique_workspace("trace_cascade");
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

    // -----------------------------------------------------------------
    // Misc helpers
    // -----------------------------------------------------------------

    #[test]
    fn generate_run_id_is_monotonic_within_process() {
        let a = generate_run_id();
        let b = generate_run_id();
        let c = generate_run_id();
        assert_ne!(a, b);
        assert_ne!(b, c);
        let counter_of = |s: &str| s.rsplit('-').next().unwrap().parse::<u64>().unwrap();
        assert!(counter_of(&b) > counter_of(&a));
        assert!(counter_of(&c) > counter_of(&b));
    }

    #[test]
    fn format_run_timestamp_known_value() {
        assert_eq!(format_run_timestamp(1705321800000), "2024-01-15 12:30");
    }

    #[test]
    fn format_run_timestamp_zero_and_negative() {
        assert_eq!(format_run_timestamp(0), "1970-01-01 00:00");
        assert_eq!(format_run_timestamp(-1000), "1970-01-01 00:00");
    }

    #[test]
    fn get_run_returns_none_for_unknown_id() {
        let ws = unique_workspace("unknown_id");
        let conn = open(&ws).expect("open");
        assert!(get_run(&conn, "missing").unwrap().is_none());
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn run_state_is_terminal_classification() {
        assert!(!RunState::Queued.is_terminal());
        assert!(!RunState::Running.is_terminal());
        for terminal in [
            RunState::Completed,
            RunState::Failed,
            RunState::Cancelled,
            RunState::TimedOut,
            RunState::DeadLetter,
        ] {
            assert!(terminal.is_terminal(), "{:?}", terminal);
        }
    }

    #[test]
    fn run_state_display_matches_as_str() {
        for state in RunState::all() {
            assert_eq!(format!("{}", state), state.as_str());
        }
    }

    #[test]
    fn run_state_serde_roundtrip() {
        for state in RunState::all() {
            let json = serde_json::to_string(state).unwrap();
            let parsed: RunState = serde_json::from_str(&json).unwrap();
            assert_eq!(*state, parsed);
        }
        // Invalid state string deserializes as error.
        let err: Result<RunState, _> = serde_json::from_str("\"bogus\"");
        assert!(err.is_err());
    }

    #[test]
    fn query_runs_applies_all_filters() {
        let ws = unique_workspace("query_filters");
        let conn = open(&ws).expect("open");
        let now = current_unix_ms();

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
    fn claim_next_honours_script_filter() {
        let ws = unique_workspace("claim_script");
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
        let ws = unique_workspace("dl_keep_reason");
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
    fn row_to_run_rejects_invalid_state_string() {
        let ws = unique_workspace("invalid_state_row");
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
        assert!(err.contains("invalid run state") || err.contains("Row query_runs failed"));

        let _ = fs::remove_dir_all(ws.root());
    }
}

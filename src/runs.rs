//! SQLite-backed run history.
//!
//! `runs.rs` is the **only** code path that persists script execution
//! history. The legacy `history.rs` JSON-file format has been removed in
//! the same release; there is no shim, no fallback, and no migration.
//!
//! On first open against a workspace, every top-level `*.json` file in
//! `<workspace>/.history/` is unlinked to clean up the legacy layout.
//! Subdirectories and other files (notably `runs.sqlite` itself and
//! `search-index.sqlite`) are left untouched.

use crate::workspace::Workspace;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
    pub started_at: i64,
    pub finished_at: i64,
    pub duration_ms: i64,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
    pub parent_run_id: Option<String>,
    pub omakure_version: String,
}

/// Filters for [`query_runs`]. All filters are AND-combined; `None`
/// fields are ignored. Default filters return every row ordered by
/// `started_at DESC` with no limit.
#[derive(Debug, Clone, Default)]
pub struct RunFilters {
    pub script: Option<String>,
    pub actor: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    /// `Some(true)` filters successes only, `Some(false)` failures only,
    /// `None` returns both.
    pub success: Option<bool>,
    pub limit: Option<i64>,
}

/// Open the run-log database for `workspace`, creating it if necessary,
/// running any pending schema setup, and (on first open against a workspace
/// that still contains legacy `.history/*.json` files) unlinking them.
///
/// The legacy cleanup is intentionally narrow: it only removes top-level
/// files in `history_dir()` whose extension is exactly `json`. It never
/// descends into subdirectories and never touches `runs.sqlite`,
/// `search-index.sqlite`, or anything inside `.omaken/`.
pub fn open(workspace: &Workspace) -> Result<Connection, String> {
    let history_dir = workspace.history_dir();
    fs::create_dir_all(history_dir)
        .map_err(|err| format!("Create history dir failed: {}", err))?;
    cleanup_legacy_json_files(history_dir);
    let db_path = runs_db_path(workspace);
    let conn = open_connection(&db_path)?;
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
    let conn = Connection::open(db_path)
        .map_err(|err| format!("Open runs db failed: {}", err))?;
    conn.busy_timeout(std::time::Duration::from_millis(500))
        .map_err(|err| format!("Runs db busy timeout failed: {}", err))?;
    let _journal_mode: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .map_err(|err| format!("Enable WAL failed: {}", err))?;
    Ok(conn)
}

/// Initialize the `runs` table and indexes. Idempotent (uses
/// `CREATE TABLE IF NOT EXISTS`), so safe to call on every open.
pub fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS runs (
            run_id TEXT PRIMARY KEY,
            script_path TEXT NOT NULL,
            script_name TEXT,
            args_json TEXT NOT NULL,
            actor TEXT NOT NULL,
            reason TEXT,
            started_at INTEGER NOT NULL,
            finished_at INTEGER NOT NULL,
            duration_ms INTEGER NOT NULL,
            exit_code INTEGER,
            success INTEGER NOT NULL,
            stdout TEXT NOT NULL,
            stderr TEXT NOT NULL,
            error TEXT,
            parent_run_id TEXT,
            omakure_version TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_runs_started_at ON runs(started_at DESC);
        CREATE INDEX IF NOT EXISTS idx_runs_script_path ON runs(script_path);
        CREATE INDEX IF NOT EXISTS idx_runs_actor ON runs(actor);",
    )
    .map_err(|err| format!("Init runs db failed: {}", err))
}

/// Insert a new run row. The caller is responsible for generating
/// `run_id` (typically via [`generate_run_id`]).
pub fn insert_run(conn: &Connection, row: &RunRow) -> Result<(), String> {
    conn.execute(
        "INSERT INTO runs (
            run_id, script_path, script_name, args_json, actor, reason,
            started_at, finished_at, duration_ms, exit_code, success,
            stdout, stderr, error, parent_run_id, omakure_version
         ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![
            row.run_id,
            row.script_path,
            row.script_name,
            row.args_json,
            row.actor,
            row.reason,
            row.started_at,
            row.finished_at,
            row.duration_ms,
            row.exit_code,
            row.success as i64,
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

/// Query rows matching the supplied filters, ordered by `started_at DESC`.
pub fn query_runs(conn: &Connection, filters: &RunFilters) -> Result<Vec<RunRow>, String> {
    let mut sql = String::from(
        "SELECT run_id, script_path, script_name, args_json, actor, reason,
                started_at, finished_at, duration_ms, exit_code, success,
                stdout, stderr, error, parent_run_id, omakure_version
         FROM runs",
    );
    let mut where_clauses: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(script) = &filters.script {
        // Match either an exact script_path or a substring of script_name.
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
        where_clauses.push("started_at >= ?".into());
        params.push(Box::new(since));
    }
    if let Some(until) = filters.until_ms {
        where_clauses.push("started_at <= ?".into());
        params.push(Box::new(until));
    }
    if let Some(success) = filters.success {
        where_clauses.push("success = ?".into());
        params.push(Box::new(success as i64));
    }

    if !where_clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_clauses.join(" AND "));
    }
    sql.push_str(" ORDER BY started_at DESC");
    if let Some(limit) = filters.limit {
        sql.push_str(&format!(" LIMIT {}", limit));
    }

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| format!("Prepare query_runs failed: {}", err))?;
    let rows = stmt
        .query_map(params_from_iter(params.iter().map(|p| p.as_ref())), row_to_run)
        .map_err(|err| format!("Query query_runs failed: {}", err))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|err| format!("Row query_runs failed: {}", err))?);
    }
    Ok(out)
}

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRow> {
    let success_int: i64 = row.get(10)?;
    Ok(RunRow {
        run_id: row.get(0)?,
        script_path: row.get(1)?,
        script_name: row.get(2)?,
        args_json: row.get(3)?,
        actor: row.get(4)?,
        reason: row.get(5)?,
        started_at: row.get(6)?,
        finished_at: row.get(7)?,
        duration_ms: row.get(8)?,
        exit_code: row.get(9)?,
        success: success_int != 0,
        stdout: row.get(11)?,
        stderr: row.get(12)?,
        error: row.get(13)?,
        parent_run_id: row.get(14)?,
        omakure_version: row.get(15)?,
    })
}

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
/// Used by the TUI history screen.
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
///
/// This is the destructive cleanup of legacy `.history/*.json` files. It
/// runs once on first [`open`] against a workspace; subsequent calls are
/// natural no-ops because there is nothing left to delete.
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
            "omakure_runs_test_{}_{}_{}",
            label,
            std::process::id(),
            current_unix_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp workspace");
        let ws = Workspace::new(dir);
        ws.ensure_layout().expect("ensure layout");
        ws
    }

    fn sample_row(run_id: &str, script_path: &str, actor: &str, started_at: i64, success: bool) -> RunRow {
        RunRow {
            run_id: run_id.to_string(),
            script_path: script_path.to_string(),
            script_name: Some("sample".into()),
            args_json: "[]".into(),
            actor: actor.to_string(),
            reason: None,
            started_at,
            finished_at: started_at + 10,
            duration_ms: 10,
            exit_code: Some(if success { 0 } else { 1 }),
            success,
            stdout: "".into(),
            stderr: "".into(),
            error: None,
            parent_run_id: None,
            omakure_version: "test".into(),
        }
    }

    #[test]
    fn open_creates_db_and_init_schema() {
        let ws = unique_workspace("open_creates_db");
        let conn = open(&ws).expect("open");
        // Inserting should work after init.
        insert_run(&conn, &sample_row("r1", "/x/a.sh", "human", 100, true)).expect("insert");
        let row = get_run(&conn, "r1").expect("get_run").expect("row exists");
        assert_eq!(row.run_id, "r1");
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn insert_run_round_trip() {
        let ws = unique_workspace("round_trip");
        let conn = open(&ws).expect("open");
        let row = sample_row("rt", "/x/script.sh", "ai", 1000, true);
        insert_run(&conn, &row).expect("insert");
        let loaded = get_run(&conn, "rt").expect("get").expect("row");
        assert_eq!(loaded.run_id, row.run_id);
        assert_eq!(loaded.script_path, row.script_path);
        assert_eq!(loaded.actor, "ai");
        assert!(loaded.success);
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn query_runs_orders_by_started_at_desc() {
        let ws = unique_workspace("order_desc");
        let conn = open(&ws).expect("open");
        insert_run(&conn, &sample_row("a", "/x/a.sh", "human", 100, true)).unwrap();
        insert_run(&conn, &sample_row("b", "/x/b.sh", "human", 300, true)).unwrap();
        insert_run(&conn, &sample_row("c", "/x/c.sh", "human", 200, true)).unwrap();
        let rows = query_runs(&conn, &RunFilters::default()).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].run_id, "b");
        assert_eq!(rows[1].run_id, "c");
        assert_eq!(rows[2].run_id, "a");
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn query_runs_filters_by_actor() {
        let ws = unique_workspace("filter_actor");
        let conn = open(&ws).expect("open");
        insert_run(&conn, &sample_row("h", "/x/a.sh", "human", 100, true)).unwrap();
        insert_run(&conn, &sample_row("i", "/x/b.sh", "ai", 200, true)).unwrap();
        let filters = RunFilters {
            actor: Some("ai".into()),
            ..Default::default()
        };
        let rows = query_runs(&conn, &filters).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].actor, "ai");
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn query_runs_filters_by_script_substring_and_path() {
        let ws = unique_workspace("filter_script");
        let conn = open(&ws).expect("open");
        insert_run(&conn, &sample_row("a", "/scripts/deploy.sh", "human", 1, true)).unwrap();
        insert_run(&conn, &sample_row("b", "/scripts/test.sh", "human", 2, true)).unwrap();
        let filters = RunFilters {
            script: Some("deploy".into()),
            ..Default::default()
        };
        let rows = query_runs(&conn, &filters).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].run_id, "a");
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn query_runs_filters_by_since_until_and_success() {
        let ws = unique_workspace("filter_time_success");
        let conn = open(&ws).expect("open");
        insert_run(&conn, &sample_row("a", "/x/a.sh", "human", 100, true)).unwrap();
        insert_run(&conn, &sample_row("b", "/x/b.sh", "human", 200, false)).unwrap();
        insert_run(&conn, &sample_row("c", "/x/c.sh", "human", 300, true)).unwrap();
        let filters = RunFilters {
            since_ms: Some(150),
            until_ms: Some(250),
            success: Some(false),
            ..Default::default()
        };
        let rows = query_runs(&conn, &filters).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].run_id, "b");
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn query_runs_respects_limit() {
        let ws = unique_workspace("limit");
        let conn = open(&ws).expect("open");
        for i in 0..5 {
            insert_run(&conn, &sample_row(&format!("r{i}"), "/x/a.sh", "human", i, true)).unwrap();
        }
        let filters = RunFilters {
            limit: Some(2),
            ..Default::default()
        };
        let rows = query_runs(&conn, &filters).unwrap();
        assert_eq!(rows.len(), 2);
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn open_deletes_top_level_json_files_only() {
        let ws = unique_workspace("cleanup_legacy");
        let history = ws.history_dir().to_path_buf();
        // Seed legacy files alongside files we must NOT touch.
        fs::write(history.join("legit.json"), "{}").unwrap();
        fs::write(history.join("another.json"), "{}").unwrap();
        fs::write(history.join("keep.txt"), "keep").unwrap();
        fs::create_dir_all(history.join("subdir")).unwrap();
        fs::write(history.join("subdir").join("nested.json"), "{}").unwrap();
        // Pre-existing search-index.sqlite must survive.
        fs::write(history.join("search-index.sqlite"), b"\x00").unwrap();

        let _conn = open(&ws).expect("open");

        assert!(!history.join("legit.json").exists(), "legit.json must be deleted");
        assert!(
            !history.join("another.json").exists(),
            "another.json must be deleted"
        );
        assert!(history.join("keep.txt").exists(), "keep.txt must survive");
        assert!(
            history.join("subdir").join("nested.json").exists(),
            "nested files must survive"
        );
        assert!(
            history.join("search-index.sqlite").exists(),
            "search-index.sqlite must survive"
        );
        assert!(history.join("runs.sqlite").exists(), "runs.sqlite must be created");
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn open_is_idempotent_after_cleanup() {
        let ws = unique_workspace("cleanup_idempotent");
        let history = ws.history_dir().to_path_buf();
        fs::write(history.join("legacy.json"), "{}").unwrap();
        let _ = open(&ws).expect("first open");
        assert!(!history.join("legacy.json").exists());
        // Second open with no legacy files: must succeed and not error.
        let conn = open(&ws).expect("second open");
        // The DB must still be functional.
        let rows = query_runs(&conn, &RunFilters::default()).unwrap();
        assert!(rows.is_empty());
        let _ = fs::remove_dir_all(ws.root());
    }

    #[test]
    fn generate_run_id_is_monotonic_within_process() {
        let a = generate_run_id();
        let b = generate_run_id();
        let c = generate_run_id();
        assert_ne!(a, b);
        assert_ne!(b, c);
        // Counter monotonicity: when split by '-', the third segment must
        // be strictly increasing within the same process.
        let counter_of = |s: &str| {
            s.rsplit('-').next().unwrap().parse::<u64>().unwrap()
        };
        assert!(counter_of(&b) > counter_of(&a));
        assert!(counter_of(&c) > counter_of(&b));
    }

    #[test]
    fn format_run_timestamp_known_value() {
        // 2024-01-15 12:30 UTC = 1705321800000 ms
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
}

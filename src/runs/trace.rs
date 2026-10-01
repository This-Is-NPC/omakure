use super::RunsError;
use crate::util::time::unix_millis;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::time::Duration;

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
// Trace storage
// ---------------------------------------------------------------------------

const TRACE_RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(25), Duration::from_millis(100)];

fn insert_trace_once(
    conn: &mut Connection,
    run_id: &str,
    level: TraceLevel,
    message: &str,
    data_json: Option<&str>,
) -> Result<TraceRow, RunsError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| RunsError::Sqlite {
            operation: "Begin trace tx failed",
            source: error,
        })?;

    let exists: bool = tx
        .query_row(
            "SELECT 1 FROM runs WHERE run_id = ? LIMIT 1",
            params![run_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| RunsError::Sqlite {
            operation: "Lookup run for trace failed",
            source: error,
        })?
        .is_some();
    if !exists {
        return Err(RunsError::NotFound(run_id.to_string()));
    }

    let next_seq: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM run_traces WHERE run_id = ?",
            params![run_id],
            |row| row.get(0),
        )
        .map_err(|error| RunsError::Sqlite {
            operation: "Compute next sequence failed",
            source: error,
        })?;
    let now = unix_millis();
    tx.execute(
        "INSERT INTO run_traces (run_id, timestamp, sequence, level, message, data_json)
             VALUES (?,?,?,?,?,?)",
        params![run_id, now, next_seq, level.as_str(), message, data_json],
    )
    .map_err(|error| RunsError::Sqlite {
        operation: "Insert trace failed",
        source: error,
    })?;
    let trace_id = tx.last_insert_rowid();
    tx.commit().map_err(|error| RunsError::Sqlite {
        operation: "Commit trace tx failed",
        source: error,
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
/// Returns [`RunsError::NotFound`] when the parent run
/// does not exist; the CLI maps this to `error.code = "not_found"`.
pub fn insert_trace(
    conn: &mut Connection,
    run_id: &str,
    level: TraceLevel,
    message: &str,
    data_json: Option<&str>,
) -> Result<TraceRow, RunsError> {
    let mut retry = 0;
    loop {
        match insert_trace_once(conn, run_id, level, message, data_json) {
            Ok(trace) => return Ok(trace),
            Err(error) if error.is_retryable() && retry < TRACE_RETRY_DELAYS.len() => {
                std::thread::sleep(TRACE_RETRY_DELAYS[retry]);
                retry += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Query trace rows for `run_id`, ordered by `sequence ASC`.
///
/// `level_min` filters to entries whose level is >= the supplied minimum
/// (e.g. `Warn` returns warn and error). `since_sequence` returns only
/// entries with `sequence > since_sequence`.
///
/// Returns [`RunsError::NotFound`] when the parent run does not exist.
pub fn query_traces(
    conn: &Connection,
    run_id: &str,
    level_min: Option<TraceLevel>,
    since_sequence: Option<i64>,
) -> Result<Vec<TraceRow>, RunsError> {
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM runs WHERE run_id = ? LIMIT 1",
            params![run_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|err| RunsError::Sqlite {
            operation: "Lookup run for traces failed",
            source: err,
        })?
        .is_some();
    if !exists {
        return Err(RunsError::NotFound(run_id.to_string()));
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

    let mut stmt = conn.prepare(&sql).map_err(|err| RunsError::Sqlite {
        operation: "Prepare query_traces failed",
        source: err,
    })?;
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
        .map_err(|err| RunsError::Sqlite {
            operation: "Query traces failed",
            source: err,
        })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|err| RunsError::Sqlite {
            operation: "Trace row failed",
            source: err,
        })?);
    }
    Ok(out)
}

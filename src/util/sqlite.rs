use rusqlite::{Connection, ErrorCode};
use std::fs;
use std::path::Path;
use std::time::Duration;

/// Bounded waits for a lock met while a connection is being opened.
///
/// The journal-mode handshake a fresh connection performs can report
/// `SQLITE_BUSY` or `SQLITE_LOCKED` outright while another process is opening
/// or closing the same file, and the busy handler is not consulted for it.
/// These delays cover that cross-process handoff without treating other SQL
/// failures as recoverable.
pub const OPEN_RETRY_DELAYS: [Duration; 5] = [
    Duration::from_millis(10),
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
];

/// A lock another connection holds right now, as opposed to any other error.
pub fn is_lock_contention(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

/// How one workspace database is opened in WAL mode.
pub struct WalDatabase {
    /// Lowercase name used in error messages, e.g. `runs`.
    pub name: &'static str,
    pub busy_timeout: Duration,
    /// Waits between attempts to enable WAL while another connection holds a
    /// lock; empty fails on the first contended attempt.
    pub wal_retry_delays: &'static [Duration],
}

impl WalDatabase {
    /// Open `path`, creating its parent directory, and switch it to WAL.
    pub fn open(&self, path: &Path) -> Result<Connection, String> {
        let name = self.name;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("Create {name} db folder failed: {err}"))?;
        }
        let conn = Connection::open(path).map_err(|err| format!("Open {name} db failed: {err}"))?;
        conn.busy_timeout(self.busy_timeout)
            .map_err(|err| format!("{} db busy timeout failed: {err}", capitalized(name)))?;
        let mut delays = self.wal_retry_delays.iter();
        let _journal_mode: String = loop {
            match conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0)) {
                Ok(mode) => break mode,
                Err(err) if is_lock_contention(&err) => match delays.next() {
                    Some(delay) => std::thread::sleep(*delay),
                    None => return Err(format!("Enable WAL failed: {err}")),
                },
                Err(err) => return Err(format!("Enable WAL failed: {err}")),
            }
        };
        Ok(conn)
    }
}

fn capitalized(name: &str) -> String {
    let mut chars = name.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_parent_and_enables_wal() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nested").join("test.sqlite");
        let database = WalDatabase {
            name: "test",
            busy_timeout: Duration::from_millis(100),
            wal_retry_delays: &[],
        };

        let conn = database.open(&path).unwrap();

        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn capitalized_uppercases_only_the_first_letter() {
        assert_eq!(capitalized("runs"), "Runs");
        assert_eq!(capitalized("search"), "Search");
        assert_eq!(capitalized(""), "");
    }
}

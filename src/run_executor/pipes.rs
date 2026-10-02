use std::io::{BufRead, BufReader, Read};
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// Heartbeat tick interval. Short enough to detect external cancel
/// quickly, but long enough not to thrash SQLite.
pub(super) const HEARTBEAT_TICK_MS: u64 = 250;

/// Maximum time we wait for a pipe to flush after the child has exited.
/// Bounded so a leaked grandchild process holding the pipe write end
/// open cannot deadlock the executor.
pub(super) const PIPE_DRAIN_BUDGET_MS: u64 = 200;

pub(super) fn spawn_pipe_reader_to_channel<R: Read + Send + 'static>(
    handle: R,
    tx: Sender<String>,
) {
    thread::spawn(move || {
        let reader = BufReader::new(handle);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                // Receiver dropped — main path moved on; abandon the
                // reader. The thread terminates when the pipe closes,
                // which on a clean exit is immediate and on an orphan
                // happens whenever the orphan eventually exits.
                break;
            }
        }
    });
}

pub(super) fn drain_channel(rx: &std::sync::mpsc::Receiver<String>, budget: Duration) -> String {
    let deadline = Instant::now() + budget;
    let mut out = String::new();
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                out.push_str(&line);
                out.push('\n');
            }
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    out
}

use super::connection::ConnectionState;
use crate::direct_transport::TransportError;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Items waiting for the session thread that can carry them, by peer node id.
pub(super) type Outbox<T> = Mutex<HashMap<String, Vec<T>>>;

pub(super) fn push_pending<T>(
    outbox: &Outbox<T>,
    peer_node_id: &str,
    pending: T,
) -> Result<(), TransportError> {
    outbox
        .lock()
        .map_err(|_| TransportError::Internal)?
        .entry(peer_node_id.to_string())
        .or_default()
        .push(pending);
    Ok(())
}

pub(super) fn take_pending<T>(outbox: &Outbox<T>, peer_node_id: &str) -> Option<T> {
    let mut outbox = outbox.lock().ok()?;
    let queue = outbox.get_mut(peer_node_id)?;
    if queue.is_empty() {
        return None;
    }
    Some(queue.remove(0))
}

/// When the session thread stops waiting for a peer's answer to a `wait`-bounded
/// dispatch, and answers the caller itself.
///
/// A little past the budget the caller asked for, so the answer the thread is
/// about to send wins over the budget that asked for it.
pub fn dispatch_answer_deadline(wait: Duration) -> Duration {
    wait + crate::direct_health::TICK * 2
}

/// How long a client must be prepared to wait for that answer.
///
/// `answered: false` is a verdict, not a failure: a receiver that refused on
/// trust, role, or capability says nothing at all, by design, and the session
/// thread turns that silence into the answer at `dispatch_answer_deadline`. A
/// client that gives up at the budget it asked for gives up *before* the answer
/// to that budget exists, so the one outcome the silence rule is built to
/// report arrives as an opaque transport error instead. A client has to outlast
/// the thread it is waiting on.
pub fn dispatch_client_timeout(wait: Duration) -> Duration {
    dispatch_answer_deadline(wait) + crate::direct_health::TICK
}

/// Fail everything still queued for a peer when its session ends.
pub(super) struct OutboxGuard<'a> {
    pub(super) state: &'a Arc<ConnectionState>,
    pub(super) peer_node_id: &'a str,
}

impl Drop for OutboxGuard<'_> {
    fn drop(&mut self) {
        self.state.drain_outbox(self.peer_node_id);
    }
}

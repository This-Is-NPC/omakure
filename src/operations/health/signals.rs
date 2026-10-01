use crate::health_plane::bounds::{SIGNAL_INBOX_CAPACITY, SIGNAL_RETENTION_SECONDS};
use crate::health_plane::model::SignalRecord;
use crate::health_plane::HealthPlane;
use crate::node_registry::NodeRegistry;
use crate::operations::node::map_registry_error;
use crate::operations::OperationResult;
use serde::Serialize;
use std::collections::HashSet;

/// One entry in the bounded, newest-first Signal feed.
///
/// Every field is privacy class P0. `source` is either the canonical node ID of
/// the Performer that reported the Signal over the wire, or `local` for the two
/// lifecycle kinds this Conductor decided itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignalEntry {
    pub source: String,
    #[serde(flatten)]
    pub signal: SignalRecord,
}

/// The per-Performer cursor state the frozen ordering rules produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignalCursor {
    pub node_id: String,
    /// The highest contiguously accepted Signal sequence for this Performer.
    pub cursor: u64,
    /// Signals the cursor has accepted and that are visible in the feed.
    pub stored: u64,
    /// Signals waiting in the bounded reorder buffer behind a gap.
    pub held: u64,
    /// Whether this Performer's feed is currently stalled behind a gap. The
    /// cursor never moves backwards and never skips, so a gap holds the feed
    /// rather than admitting a hole.
    pub gap: bool,
}

/// The bounded, newest-first Signal read surface.
///
/// This is a small closed feed, not history and not an event bus: exactly three
/// Signal kinds, one bounded page, one frozen retention window, no
/// subscriptions, no webhooks, and no filters that could turn it into a query
/// engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SignalFeedReport {
    /// The reading node's own canonical node ID.
    pub local_node_id: String,
    /// The UTC Unix second the feed was derived at.
    pub observed_at: i64,
    /// The frozen Signal retention window, in seconds.
    pub retention_seconds: i64,
    /// The frozen bound on how many Signals one read may return.
    pub limit: usize,
    /// Whether any Performer feed is currently stalled behind a gap.
    pub gap: bool,
    /// Per-Performer cursor state, ordered by node ID.
    pub cursors: Vec<SignalCursor>,
    /// The bounded page, newest first.
    pub signals: Vec<SignalEntry>,
}

/// The Conductor-local Signal feed.
///
/// Read-only, bounded, and adapter-free: `omakure node signals --json` and
/// `GET /v1/node/signals` render exactly this value, which is what makes them
/// return identical results.
///
/// Both halves come from the Wave 2 shared operations. Remote Signals are the
/// bounded per-Performer inbox the ingest path filled; local `enrolled` and
/// `revoked` Signals are projected from the append-only trust audit, so they
/// survive the revocation cleanup that deletes every Health Plane row for a
/// peer that is no longer actively trusted.
pub fn signal_feed(registry: &NodeRegistry) -> OperationResult<SignalFeedReport> {
    let plane = HealthPlane::new(registry);
    let limit = SIGNAL_INBOX_CAPACITY as usize;
    // One snapshot for the cursors, the Signals, and the trust log the
    // local lifecycle Signals are projected from. Read separately, the
    // report could contradict itself: ingest commits between the counter
    // read and the Signal read, and the feed then shows a Signal beside a
    // cursor that has not counted it. `gap` is derived from those same
    // counters, and it is the field an operator reads to decide whether a
    // fleet's Signal delivery has stalled.
    let feed = plane.signal_feed(limit).map_err(map_registry_error)?;
    let observed_at = feed.observed_at;
    let mut entries: Vec<SignalEntry> = Vec::new();
    let mut cursors: Vec<SignalCursor> = Vec::new();
    for signal in feed.local {
        entries.push(SignalEntry {
            source: LOCAL_SIGNAL_SOURCE.to_string(),
            signal,
        });
    }
    // The feed shows the *actively trusted* fleet, exactly like the
    // fleet-status projection: a peer whose trust was revoked, suspended,
    // or replaced stops appearing at once, which is what makes a
    // revocation change the operator's view immediately. The retained rows
    // are removed for good by the frozen revocation cleanup.
    let mut active: HashSet<String> = HashSet::new();
    for node in feed.nodes {
        if node.trust_state != "active" {
            continue;
        }
        active.insert(node.node_id.clone());
        cursors.push(SignalCursor {
            node_id: node.node_id,
            cursor: node.cursor,
            stored: node.stored,
            held: node.held,
            gap: node.held > 0,
        });
    }
    cursors.sort_by(|left, right| left.node_id.cmp(&right.node_id));
    for entry in feed.signals {
        // The bounded page is already restricted to actively trusted
        // peers by the read itself; this repeats the decision here so the
        // rule stays visible where the projection is assembled.
        if !active.contains(&entry.source) {
            continue;
        }
        entries.push(SignalEntry {
            source: entry.source,
            signal: entry.signal,
        });
    }
    reduce_to_newest(&mut entries, limit);
    Ok(SignalFeedReport {
        local_node_id: registry.local_node_id().to_string(),
        observed_at,
        retention_seconds: SIGNAL_RETENTION_SECONDS,
        limit,
        gap: cursors.iter().any(|cursor| cursor.gap),
        cursors,
        signals: entries,
    })
}

/// The `source` marker for a Signal this node decided itself.
const LOCAL_SIGNAL_SOURCE: &str = "local";

/// Order newest first and keep one bounded page.
///
/// The tiebreak on `signal_id` gives a total order, so two Signals that share
/// a second still render deterministically for both adapters.
fn reduce_to_newest(entries: &mut Vec<SignalEntry>, limit: usize) {
    entries.sort_by(|left, right| {
        right
            .signal
            .occurred_at
            .cmp(&left.signal.occurred_at)
            .then_with(|| right.signal.signal_id.cmp(&left.signal.signal_id))
    });
    entries.truncate(limit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health_plane::model::SignalKind;

    #[test]
    fn equal_signal_timestamps_use_the_signal_id_tiebreak() {
        let mut entries = vec![
            SignalEntry {
                source: "peer".to_string(),
                signal: SignalRecord {
                    kind: SignalKind::RunCompleted,
                    occurred_at: 1_700_000_000,
                    run: None,
                    sequence: 1,
                    signal_id: "a".to_string(),
                    subject: None,
                },
            },
            SignalEntry {
                source: "peer".to_string(),
                signal: SignalRecord {
                    kind: SignalKind::RunCompleted,
                    occurred_at: 1_700_000_000,
                    run: None,
                    sequence: 2,
                    signal_id: "b".to_string(),
                    subject: None,
                },
            },
        ];

        reduce_to_newest(&mut entries, 2);

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.signal.signal_id.as_str())
                .collect::<Vec<_>>(),
            ["b", "a"]
        );
    }
}

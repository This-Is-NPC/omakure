//! Conductor-local `enrolled` and `revoked` Signals.
//!
//! The frozen contract carries `run-completed` from a Performer to its
//! Conductor over the wire, but `enrolled` and `revoked` are decided *by the
//! Conductor itself*: it is the node that activates trust and the node that
//! revokes it. Their authoritative record is therefore the append-only trust
//! audit the registry already keeps, and this module **projects** that record
//! into the closed Signal vocabulary.
//!
//! Projecting rather than persisting is deliberate and buys three properties
//! the frozen contract needs, none of which a second write path would give:
//!
//! * **Exactly once, by construction.** One trust transition is one audit row
//!   is one Signal. A restart, a repeated read, or a duplicate CLI invocation
//!   cannot produce a second Signal, because nothing is ever written.
//! * **Revocation-safe.** `docs/internal/health-plane-contract.md` makes Health Plane
//!   state derived and disposable, and revocation cleanup deletes every Health
//!   Plane row for a peer that is no longer actively trusted. A `revoked`
//!   Signal stored in that state would delete itself. The audit log is not
//!   Health Plane state, is append-only, and survives.
//! * **No trust mutation.** The Health Plane may write only Health Plane rows
//!   and Health Plane audit rows. This module writes nothing at all, so no
//!   code path from a Signal can reach identity, trust, or revocation.
//!
//! The transition view carries only the audit row id, affected `node_id`,
//! transition, and timestamp. The `actor` and `reason` columns are privacy
//! class P1 and cannot enter the projection or reach a Signal.

use super::bounds::{MAX_SAFE_INTEGER, SIGNAL_INBOX_CAPACITY, SIGNAL_RETENTION_SECONDS};
use super::model::{LifecycleState, LifecycleTransition, SignalKind, SignalRecord};
use crate::util::hex;
use sha2::{Digest, Sha256};

/// Domain separator for the stable Conductor-local `signal_id`.
///
/// It is derived from the audit row identity so the same transition always
/// yields the same idempotency key, exactly like the run-derived key on the
/// Performer side.
const LOCAL_SIGNAL_ID_DOMAIN: &[u8] = b"omakure/health-local-signal-id/v1\0";

/// Project one bounded page of Conductor-local lifecycle Signals.
///
/// `events` is the newest-first trust-transition view the registry
/// returns. The result is newest first, bounded by the frozen per-peer Signal
/// capacity, and contains nothing older than the frozen Signal retention
/// window: the Health Plane keeps a small bounded feed, never history.
pub fn project<'a>(
    events: impl IntoIterator<Item = LifecycleTransition<'a>>,
    now: i64,
    limit: usize,
) -> Vec<SignalRecord> {
    let limit = limit.min(SIGNAL_INBOX_CAPACITY as usize);
    let floor = now.saturating_sub(SIGNAL_RETENTION_SECONDS);
    let mut signals = Vec::with_capacity(limit);
    for event in events {
        if signals.len() == limit {
            break;
        }
        let Some(kind) = lifecycle_kind(&event) else {
            continue;
        };
        let Some(occurred_at) = event.occurred_at else {
            continue;
        };
        if occurred_at < floor || occurred_at < 1 {
            continue;
        }
        // The audit row id is already a unique, monotonic, per-node-local
        // ordinal, so it is the Signal `sequence` directly. Nothing derives a
        // second counter that a restart could disagree with.
        let Ok(sequence) = u64::try_from(event.id) else {
            continue;
        };
        if !(1..=MAX_SAFE_INTEGER).contains(&sequence) {
            continue;
        }
        signals.push(SignalRecord {
            kind,
            occurred_at,
            run: None,
            sequence,
            signal_id: local_signal_id(event.id, kind, event.node_id),
            subject: Some(event.node_id.to_string()),
        });
    }
    signals
}

/// The closed Signal kind one trust transition maps onto, if any.
///
/// A transition *into* active trust is `enrolled`; a transition *into*
/// revocation is `revoked`. A row whose `from_state` already equals the target
/// is not a transition and produces nothing, so a repeated write can never
/// manufacture a second lifecycle Signal.
fn lifecycle_kind(event: &LifecycleTransition<'_>) -> Option<SignalKind> {
    match event.to_state? {
        LifecycleState::Active if event.from_state != Some(LifecycleState::Active) => {
            Some(SignalKind::Enrolled)
        }
        LifecycleState::Revoked if event.from_state != Some(LifecycleState::Revoked) => {
            Some(SignalKind::Revoked)
        }
        _ => None,
    }
}

/// The stable idempotency key of one Conductor-local lifecycle Signal.
fn local_signal_id(audit_id: i64, kind: SignalKind, node_id: &str) -> String {
    let digest = Sha256::digest(
        [
            LOCAL_SIGNAL_ID_DOMAIN,
            &audit_id.to_be_bytes()[..],
            kind.wire().as_bytes(),
            b"\0",
            node_id.as_bytes(),
        ]
        .concat(),
    );
    hex::encode(&digest[..16])
}

#[cfg(test)]
mod tests {
    use super::*;

    const PEER: &str = "omk1_0000000000000000000000000000000000000000000000000000000000000001";
    const OTHER: &str = "omk1_0000000000000000000000000000000000000000000000000000000000000002";
    const NOW: i64 = 1_700_000_000;

    fn event(
        id: i64,
        node_id: &'static str,
        from_state: Option<LifecycleState>,
        to_state: Option<LifecycleState>,
        occurred_at: i64,
    ) -> LifecycleTransition<'static> {
        LifecycleTransition {
            id,
            node_id,
            from_state,
            to_state,
            occurred_at: Some(occurred_at),
        }
    }

    #[test]
    fn activation_and_revocation_project_onto_the_two_local_signal_kinds() {
        let events = [
            event(
                2,
                PEER,
                Some(LifecycleState::Active),
                Some(LifecycleState::Revoked),
                NOW - 10,
            ),
            event(
                1,
                PEER,
                Some(LifecycleState::Other),
                Some(LifecycleState::Active),
                NOW - 20,
            ),
        ];
        let signals = project(events.iter().copied(), NOW, 64);
        assert_eq!(signals.len(), 2);
        assert_eq!(signals[0].kind, SignalKind::Revoked);
        assert_eq!(signals[0].sequence, 2);
        assert_eq!(signals[0].subject.as_deref(), Some(PEER));
        assert!(signals[0].run.is_none());
        assert_eq!(signals[1].kind, SignalKind::Enrolled);
        assert_eq!(signals[1].sequence, 1);
    }

    #[test]
    fn a_non_transition_row_produces_no_signal() {
        let events = [
            event(
                3,
                PEER,
                Some(LifecycleState::Active),
                Some(LifecycleState::Active),
                NOW,
            ),
            event(
                4,
                PEER,
                Some(LifecycleState::Revoked),
                Some(LifecycleState::Revoked),
                NOW,
            ),
            event(5, PEER, Some(LifecycleState::Other), None, NOW),
            event(
                6,
                PEER,
                Some(LifecycleState::Other),
                Some(LifecycleState::Other),
                NOW,
            ),
        ];
        assert!(project(events.iter().copied(), NOW, 64).is_empty());
    }

    #[test]
    fn a_transition_without_a_valid_timestamp_produces_no_signal() {
        let mut invalid = event(
            7,
            PEER,
            Some(LifecycleState::Other),
            Some(LifecycleState::Active),
            NOW,
        );
        invalid.occurred_at = None;
        assert!(project([invalid], NOW, 1).is_empty());
    }

    #[test]
    fn the_signal_id_is_stable_per_row_and_distinct_across_rows() {
        let one = event(
            7,
            PEER,
            Some(LifecycleState::Other),
            Some(LifecycleState::Active),
            NOW,
        );
        let again = event(
            7,
            PEER,
            Some(LifecycleState::Other),
            Some(LifecycleState::Active),
            NOW,
        );
        let other_row = event(
            8,
            PEER,
            Some(LifecycleState::Other),
            Some(LifecycleState::Active),
            NOW,
        );
        let other_node = event(
            7,
            OTHER,
            Some(LifecycleState::Other),
            Some(LifecycleState::Active),
            NOW,
        );
        let first = project([one], NOW, 64).remove(0).signal_id;
        assert_eq!(first, project([again], NOW, 64).remove(0).signal_id);
        assert_ne!(first, project([other_row], NOW, 64).remove(0).signal_id);
        assert_ne!(first, project([other_node], NOW, 64).remove(0).signal_id);
        assert_eq!(first.len(), 32);
        assert!(crate::util::hex::is_lower(&first));
    }

    #[test]
    fn the_feed_is_bounded_by_capacity_and_by_the_frozen_retention_window() {
        let mut events = Vec::new();
        for id in 1..=200 {
            events.push(event(
                201 - id,
                PEER,
                Some(LifecycleState::Other),
                Some(LifecycleState::Active),
                NOW - id,
            ));
        }
        assert_eq!(
            project(events.iter().copied(), NOW, 1_000).len(),
            SIGNAL_INBOX_CAPACITY as usize
        );

        let expired = [
            event(
                9,
                PEER,
                Some(LifecycleState::Other),
                Some(LifecycleState::Active),
                NOW - SIGNAL_RETENTION_SECONDS - 1,
            ),
            event(
                10,
                PEER,
                Some(LifecycleState::Other),
                Some(LifecycleState::Active),
                NOW - SIGNAL_RETENTION_SECONDS,
            ),
        ];
        let retained = project(expired.iter().copied(), NOW, 64);
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].sequence, 10);
    }
}

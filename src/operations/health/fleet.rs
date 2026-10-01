use crate::health_plane::bounds::MAX_PERFORMERS_PER_CONDUCTOR;
use crate::health_plane::model::Presence;
use crate::health_plane::{BaselineStatus, FleetNode, HealthPlane};
use crate::node_registry::{NodeRegistry, PeerState};
use crate::operations::OperationResult;
use crate::operations::node::map_registry_error;
use serde::Serialize;
use std::collections::HashSet;

/// The bounded, redacted fleet-status projection.
///
/// This is current status only. It carries no chart series, no alert rule, no
/// arbitrary host inventory, no raw log, and no history: the Health Plane
/// stores exactly one Profile and one Pulse per Performer, and this report
/// shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FleetStatusReport {
    /// The reporting node's own canonical node ID.
    pub local_node_id: String,
    /// The UTC Unix second the presence projection was derived at.
    pub observed_at: i64,
    /// Presence counts across every actively trusted peer.
    pub presence: PresenceCounts,
    /// Baseline verdicts across the same peers, so "which machines drifted" is
    /// one read rather than a scan of every row.
    pub baselines: BaselineCounts,
    /// One row per actively trusted peer, ordered by node ID.
    pub nodes: Vec<FleetNode>,
}

/// Baseline verdict totals, derived from the same stored Profiles the rows show.
///
/// `unknown` and `none` are separate totals on purpose: a fleet with ten
/// machines that have never reported and a fleet with ten that hold no baseline
/// are different situations, and one number covering both would tell an
/// operator to go looking in the wrong place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct BaselineCounts {
    pub in_sync: usize,
    pub drifted: usize,
    pub none: usize,
    pub unknown: usize,
    pub total: usize,
}

/// Presence totals derived from the frozen Pulse-age windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct PresenceCounts {
    pub online: usize,
    pub stale: usize,
    pub offline: usize,
    pub unknown: usize,
    pub total: usize,
}

fn collect_active_fleet_nodes(
    plane: &HealthPlane<'_, NodeRegistry>,
    registry: &NodeRegistry,
    observed_at: i64,
    mut nodes: Vec<FleetNode>,
) -> OperationResult<Vec<FleetNode>> {
    // The fleet is the set of *actively trusted* peers. A peer whose trust
    // was revoked, suspended, or replaced is no longer part of it, so its
    // retained Health Plane row must not keep reporting a presence: that
    // is what makes a revocation change the operator's view immediately.
    // The decision uses the `trust_state` the Wave 2 projection already
    // computed from the local registry; nothing is re-derived here.
    nodes.retain(|node| node.trust_state == "active");

    // A peer that has never reported has no Health Plane row at all, so the
    // shared projection cannot see it. Enumerate the trusted peers and fill
    // in the never-seen ones, deciding trust *only* through the Wave 2
    // read-only authorization projection and deriving presence *only*
    // through the Wave 2 presence rule.
    let seen: HashSet<String> = nodes.iter().map(|node| node.node_id.clone()).collect();
    let candidates = registry
        .peers_limited(MAX_PERFORMERS_PER_CONDUCTOR as usize)
        .map_err(map_registry_error)?;
    for candidate in candidates {
        if seen.contains(&candidate.node_id) {
            continue;
        }
        let Some(authorization) = plane
            .authorization(&candidate.node_id)
            .map_err(map_registry_error)?
        else {
            continue;
        };
        if authorization.state != PeerState::Active {
            continue;
        }
        nodes.push(FleetNode {
            node_id: authorization.node_id,
            role: authorization.role.as_str().to_string(),
            capabilities: authorization.capabilities,
            trust_state: "active".to_string(),
            presence: Presence::derive(None, observed_at),
            last_pulse_at: None,
            baseline_status: BaselineStatus::Unknown,
            profile: None,
            pulse: None,
            signal_cursor: 0,
            stored_signals: 0,
            held_signals: 0,
            version_incompatible: false,
        });
    }
    nodes.sort_by(|left, right| left.node_id.cmp(&right.node_id));
    Ok(nodes)
}

fn tally_fleet_counts(nodes: &[FleetNode]) -> (PresenceCounts, BaselineCounts) {
    let mut presence = PresenceCounts {
        total: nodes.len(),
        ..PresenceCounts::default()
    };
    let mut baselines = BaselineCounts {
        total: nodes.len(),
        ..BaselineCounts::default()
    };
    for node in nodes {
        match node.presence {
            Presence::Online => presence.online += 1,
            Presence::Stale => presence.stale += 1,
            Presence::Offline => presence.offline += 1,
            Presence::Unknown => presence.unknown += 1,
        }
        match node.baseline_status {
            BaselineStatus::InSync => baselines.in_sync += 1,
            BaselineStatus::Drifted => baselines.drifted += 1,
            BaselineStatus::None => baselines.none += 1,
            BaselineStatus::Unknown => baselines.unknown += 1,
        }
    }
    (presence, baselines)
}

/// The Conductor-local fleet-status projection.
///
/// The operation is read-only and derives presence through the Wave 2 shared
/// operations; it never reads a Health Plane table directly and never
/// re-implements authorization.
pub fn fleet_status(registry: &NodeRegistry) -> OperationResult<FleetStatusReport> {
    let plane = HealthPlane::new(registry);
    let observed_at = plane.now();
    let nodes = plane.fleet_status().map_err(map_registry_error)?;
    let nodes = collect_active_fleet_nodes(&plane, registry, observed_at, nodes)?;
    let (presence, baselines) = tally_fleet_counts(&nodes);
    Ok(FleetStatusReport {
        local_node_id: registry.local_node_id().to_string(),
        observed_at,
        presence,
        baselines,
        nodes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_counts_default_to_zero() {
        let counts = PresenceCounts::default();
        assert_eq!(counts.total, 0);
        assert_eq!(
            counts.online + counts.stale + counts.offline + counts.unknown,
            0
        );
    }
}

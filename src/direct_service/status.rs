use super::connection::ActiveConnection;
use super::error::DirectServiceError;
use crate::direct_transport::TransportError;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticPeer {
    pub node_id: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TransportPeerStatus {
    pub node_id: String,
    pub state: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct TransportStatus {
    pub enabled: bool,
    pub listening: bool,
    pub expected_peer_count: usize,
    pub connected_peer_count: usize,
    pub expected_connected_peer_count: usize,
    pub peers: Vec<TransportPeerStatus>,
    pub last_errors: BTreeMap<String, String>,
}

pub type TransportStatusHandle = Arc<Mutex<TransportStatus>>;

pub fn parse_static_peer(value: &str) -> Result<StaticPeer, DirectServiceError> {
    let Some((node_id, endpoint)) = value.split_once('@') else {
        return Err(DirectServiceError::Protocol(TransportError::InvalidFrame));
    };
    if node_id.is_empty() || endpoint.is_empty() || endpoint.contains('@') {
        return Err(DirectServiceError::Protocol(TransportError::InvalidFrame));
    }
    Ok(StaticPeer {
        node_id: node_id.to_string(),
        endpoint: endpoint.to_string(),
    })
}

pub(super) fn validate_static_peers(peers: &[StaticPeer]) -> Result<(), DirectServiceError> {
    let mut node_ids = HashSet::new();
    let mut endpoints = HashSet::new();
    for peer in peers {
        if !node_ids.insert(&peer.node_id) || !endpoints.insert(&peer.endpoint) {
            return Err(DirectServiceError::Protocol(TransportError::InvalidFrame));
        }
    }
    Ok(())
}

pub(super) fn refresh_status(
    status: &TransportStatusHandle,
    expected: &HashSet<String>,
    active: &HashMap<String, ActiveConnection>,
) {
    let Ok(mut status) = status.lock() else {
        return;
    };
    let mut node_ids = expected.iter().cloned().collect::<Vec<_>>();
    node_ids.extend(active.keys().filter(|id| !expected.contains(*id)).cloned());
    node_ids.sort();
    node_ids.dedup();
    status.connected_peer_count = active.len();
    status.expected_connected_peer_count = expected
        .iter()
        .filter(|node_id| active.contains_key(*node_id))
        .count();
    status.peers = node_ids
        .into_iter()
        .map(|node_id| TransportPeerStatus {
            state: if active.contains_key(&node_id) {
                "connected"
            } else {
                "disconnected"
            },
            node_id,
        })
        .collect();
}

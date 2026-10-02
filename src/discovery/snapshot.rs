use super::*;
use crate::domain::DiscoverySettings;
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiscoveryCandidate {
    pub node_id: String,
    pub direct_port: u16,
    pub last_seen: u64,
    pub expires_at: u64,
    pub sequence: u64,
    pub identity_verified: bool,
    pub secret_proof_verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiscoveryStatus {
    pub enabled: bool,
    pub supported: bool,
    pub listening: bool,
    pub multicast: bool,
    pub broadcast: bool,
    pub secret_configured: bool,
    pub candidate_count: usize,
    pub accepted_datagrams: u64,
    pub dropped_datagrams: u64,
    pub limits: DiscoveryLimits,
    pub candidates: Vec<DiscoveryCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DiscoveryLimits {
    pub datagram_bytes: usize,
    pub secret_bytes: usize,
    pub interfaces: usize,
    pub source_entries: usize,
    pub candidates: usize,
    pub addresses_per_node: usize,
    pub global_datagrams_per_second: usize,
    pub source_datagrams_per_second: usize,
}

impl Default for DiscoveryLimits {
    fn default() -> Self {
        Self {
            datagram_bytes: MAX_DATAGRAM_BYTES,
            secret_bytes: MAX_DISCOVERY_SECRET_BYTES,
            interfaces: MAX_INTERFACES,
            source_entries: MAX_SOURCE_ENTRIES,
            candidates: MAX_CANDIDATES,
            addresses_per_node: MAX_ADDRESSES_PER_NODE,
            global_datagrams_per_second: MAX_GLOBAL_DATAGRAMS_PER_SECOND,
            source_datagrams_per_second: MAX_SOURCE_DATAGRAMS_PER_SECOND,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct CandidateRecord {
    node_id: String,
    source_ip: IpAddr,
    direct_port: u16,
    last_seen: u64,
    expires_at: u64,
    sequence: u64,
    beacon_id: [u8; BEACON_ID_BYTES],
}

#[derive(Debug, Clone)]
pub(super) struct SourceRate {
    seen_at: VecDeque<Instant>,
    last_seen: Instant,
}

#[derive(Debug)]
pub struct DiscoverySnapshot {
    pub(super) status: DiscoveryStatus,
    pub(super) candidates: HashMap<(String, IpAddr, u16), CandidateRecord>,
    pub(super) sources: HashMap<IpAddr, SourceRate>,
    global_seen_at: VecDeque<Instant>,
}

impl DiscoverySnapshot {
    pub(super) fn new(
        settings: &DiscoverySettings,
        supported: bool,
        listening: bool,
        multicast: bool,
        broadcast: bool,
        secret: bool,
    ) -> Self {
        Self {
            status: DiscoveryStatus {
                enabled: settings.enabled,
                supported,
                listening,
                multicast,
                broadcast,
                secret_configured: secret,
                candidate_count: 0,
                accepted_datagrams: 0,
                dropped_datagrams: 0,
                limits: DiscoveryLimits::default(),
                candidates: Vec::new(),
                last_error: None,
            },
            candidates: HashMap::new(),
            sources: HashMap::new(),
            global_seen_at: VecDeque::new(),
        }
    }

    pub fn public_status(&mut self, include_addresses: bool, now: u64) -> DiscoveryStatus {
        self.prune(now, Instant::now());
        let mut candidates = self
            .candidates
            .values()
            .map(|candidate| DiscoveryCandidate {
                node_id: candidate.node_id.clone(),
                direct_port: candidate.direct_port,
                last_seen: candidate.last_seen,
                expires_at: candidate.expires_at,
                sequence: candidate.sequence,
                identity_verified: true,
                secret_proof_verified: self.status.secret_configured,
                address: include_addresses.then(|| {
                    SocketAddr::new(candidate.source_ip, candidate.direct_port).to_string()
                }),
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|a, b| a.node_id.cmp(&b.node_id).then(a.address.cmp(&b.address)));
        self.status.candidate_count = candidates.len();
        self.status.candidates = candidates;
        self.status.clone()
    }

    pub(super) fn accept(&mut self, beacon: Beacon, source_ip: IpAddr, now: u64, instant: Instant) {
        self.prune(now, instant);
        let key = (beacon.node_id.clone(), source_ip, beacon.direct_port);
        if let Some(existing) = self.candidates.get_mut(&key) {
            if (beacon.issued_at, beacon.sequence) <= (existing.last_seen, existing.sequence) {
                return;
            }
            existing.last_seen = beacon.issued_at;
            existing.expires_at = beacon.expires_at;
            existing.sequence = beacon.sequence;
            existing.beacon_id = beacon.beacon_id;
        } else {
            let node_addresses = self
                .candidates
                .values()
                .filter(|candidate| candidate.node_id == beacon.node_id)
                .count();
            if node_addresses >= MAX_ADDRESSES_PER_NODE {
                self.status.dropped_datagrams = self.status.dropped_datagrams.saturating_add(1);
                self.status.last_error = Some(DiscoveryError::CandidateLimit.to_string());
                return;
            }
            if self.candidates.len() >= MAX_CANDIDATES {
                self.status.dropped_datagrams = self.status.dropped_datagrams.saturating_add(1);
                return;
            }
            self.candidates.insert(
                key,
                CandidateRecord {
                    node_id: beacon.node_id,
                    source_ip,
                    direct_port: beacon.direct_port,
                    last_seen: beacon.issued_at,
                    expires_at: beacon.expires_at,
                    sequence: beacon.sequence,
                    beacon_id: beacon.beacon_id,
                },
            );
        }
        self.status.accepted_datagrams = self.status.accepted_datagrams.saturating_add(1);
    }

    pub(super) fn admit_source(&mut self, source: IpAddr, now: Instant) -> bool {
        self.prune_rates(now);
        if self.global_seen_at.len() >= MAX_GLOBAL_DATAGRAMS_PER_SECOND {
            return false;
        }
        if !self.sources.contains_key(&source) && self.sources.len() >= MAX_SOURCE_ENTRIES {
            return false;
        }
        let source_rate = self.sources.entry(source).or_insert_with(|| SourceRate {
            seen_at: VecDeque::new(),
            last_seen: now,
        });
        if source_rate.seen_at.len() >= MAX_SOURCE_DATAGRAMS_PER_SECOND {
            return false;
        }
        source_rate.seen_at.push_back(now);
        source_rate.last_seen = now;
        self.global_seen_at.push_back(now);
        true
    }

    pub(super) fn prune(&mut self, now: u64, instant: Instant) {
        self.candidates
            .retain(|_, candidate| candidate.expires_at > now);
        self.prune_rates(instant);
        self.status.candidate_count = self.candidates.len();
    }

    fn prune_rates(&mut self, now: Instant) {
        while self
            .global_seen_at
            .front()
            .is_some_and(|seen| now.duration_since(*seen) >= RATE_WINDOW)
        {
            self.global_seen_at.pop_front();
        }
        self.sources.retain(|_, source| {
            while source
                .seen_at
                .front()
                .is_some_and(|seen| now.duration_since(*seen) >= RATE_WINDOW)
            {
                source.seen_at.pop_front();
            }
            now.duration_since(source.last_seen) < SOURCE_RETENTION
        });
    }
}

pub type DiscoveryStatusHandle = Arc<Mutex<DiscoverySnapshot>>;

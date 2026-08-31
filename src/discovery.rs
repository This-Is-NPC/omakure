//! Bounded, trust-neutral LAN discovery.
//!
//! Beacons are signed evidence about a possible direct endpoint. This module
//! never opens a trust/session registry and never authorizes a peer.

use crate::domain::DiscoverySettings;
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use k256::schnorr::{signature::hazmat::PrehashVerifier, Signature, VerifyingKey};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const BEACON_MAGIC: &[u8; 4] = b"OMKB";
pub const BEACON_VERSION: u8 = 1;
pub const BEACON_KIND: u8 = 1;
pub const BEACON_CONTRACT_ID: &[u8] = b"omakure/lan-discovery/v1";
pub const BEACON_SIGNATURE_DOMAIN: &[u8] = b"omakure/lan-beacon/v1\0";
pub const DISCOVERY_PROOF_DOMAIN: &[u8] = b"omakure/lan-discovery-proof/v1\0";
pub const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 42, 99);
pub const DISCOVERY_PORT: u16 = 38_383;
pub const MAX_DATAGRAM_BYTES: usize = 512;
pub const MAX_BEACON_BYTES_WITHOUT_PROOF: usize = 215;
pub const MAX_BEACON_BYTES_WITH_PROOF: usize = 247;
pub const MAX_SOURCE_ENTRIES: usize = 256;
pub const MAX_CANDIDATES: usize = 256;
pub const MAX_ADDRESSES_PER_NODE: usize = 8;
pub const MAX_GLOBAL_DATAGRAMS_PER_SECOND: usize = 64;
pub const MAX_SOURCE_DATAGRAMS_PER_SECOND: usize = 8;
pub const MAX_DISCOVERY_SECRET_BYTES: usize = 256;
pub const BEACON_INTERVAL: Duration = Duration::from_secs(3);
pub const BEACON_LIFETIME_SECONDS: u64 = 15;
pub const FUTURE_SKEW_SECONDS: u64 = 5;

const PROOF_FLAG: u16 = 1;
const HEADER_BYTES: usize = 8;
const UNSIGNED_BYTES: usize = 151;
const NODE_ID_BYTES: usize = 69;
const IDENTITY_BYTES: usize = 32;
const BEACON_ID_BYTES: usize = 16;
const PROOF_BYTES: usize = 32;
const SIGNATURE_BYTES: usize = 64;
const RECEIVE_BATCH_LIMIT: usize = 32;
const RATE_WINDOW: Duration = Duration::from_secs(1);
const SOURCE_RETENTION: Duration = Duration::from_secs(60);
const MAX_INTERFACES: usize = 16;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DiscoveryError {
    #[error("unsupported_version")]
    UnsupportedVersion,
    #[error("invalid_beacon")]
    InvalidBeacon,
    #[error("message_too_large")]
    MessageTooLarge,
    #[error("expired")]
    Expired,
    #[error("future")]
    Future,
    #[error("secret_mismatch")]
    SecretMismatch,
    #[error("identity_mismatch")]
    IdentityMismatch,
    #[error("signature_invalid")]
    SignatureInvalid,
    #[error("rate_limited")]
    RateLimited,
    #[error("candidate_limit")]
    CandidateLimit,
    #[error("secret_invalid")]
    SecretInvalid,
    #[error("platform_unsupported")]
    UnsupportedPlatform,
    #[error("internal")]
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beacon {
    pub node_id: String,
    pub identity_xonly: [u8; IDENTITY_BYTES],
    pub beacon_id: [u8; BEACON_ID_BYTES],
    pub direct_port: u16,
    pub issued_at: u64,
    pub expires_at: u64,
    pub sequence: u64,
    pub discovery_proof: Option<[u8; PROOF_BYTES]>,
    pub signature: [u8; SIGNATURE_BYTES],
}

impl Beacon {
    pub fn create(
        identity: &NodeIdentity,
        direct_port: u16,
        beacon_id: [u8; BEACON_ID_BYTES],
        sequence: u64,
        issued_at: u64,
        secret: Option<&[u8]>,
    ) -> Result<Self, DiscoveryError> {
        if direct_port == 0 {
            return Err(DiscoveryError::InvalidBeacon);
        }
        validate_secret(secret)?;
        let identity_key = hex_decode(&identity.public_status().public_key_hex)
            .ok_or(DiscoveryError::IdentityMismatch)?;
        let identity_xonly: [u8; IDENTITY_BYTES] = identity_key
            .try_into()
            .map_err(|_| DiscoveryError::IdentityMismatch)?;
        let expires_at = issued_at.saturating_add(BEACON_LIFETIME_SECONDS);
        let node_id = identity.public_status().node_id.clone();
        let mut beacon = Self {
            node_id,
            identity_xonly,
            beacon_id,
            direct_port,
            issued_at,
            expires_at,
            sequence,
            discovery_proof: None,
            signature: [0; SIGNATURE_BYTES],
        };
        if let Some(secret) = secret {
            // The proof is computed over the final flags value, before the
            // proof bytes themselves are appended to the signed body.
            beacon.discovery_proof = Some([0; PROOF_BYTES]);
            beacon.discovery_proof = Some(hmac_sha256(secret, &beacon.proof_input()));
        }
        let signature = identity
            .sign_discovery(&beacon.unsigned_bytes())
            .map_err(|_| DiscoveryError::Internal)?;
        beacon.signature = signature.to_bytes();
        Ok(beacon)
    }

    pub fn encode(&self) -> Result<Vec<u8>, DiscoveryError> {
        self.validate_shape()?;
        let mut encoded = self.unsigned_bytes();
        encoded.extend_from_slice(&self.signature);
        Ok(encoded)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, DiscoveryError> {
        if bytes.len() > MAX_DATAGRAM_BYTES {
            return Err(DiscoveryError::MessageTooLarge);
        }
        if bytes.len() < HEADER_BYTES {
            return Err(DiscoveryError::InvalidBeacon);
        }
        if &bytes[..4] != BEACON_MAGIC {
            return Err(DiscoveryError::InvalidBeacon);
        }
        if bytes[4] != BEACON_VERSION {
            return Err(DiscoveryError::UnsupportedVersion);
        }
        if bytes[5] != BEACON_KIND {
            return Err(DiscoveryError::InvalidBeacon);
        }
        let flags = u16::from_be_bytes([bytes[6], bytes[7]]);
        if flags & !PROOF_FLAG != 0 {
            return Err(DiscoveryError::InvalidBeacon);
        }
        let expected =
            UNSIGNED_BYTES + SIGNATURE_BYTES + usize::from(flags == PROOF_FLAG) * PROOF_BYTES;
        if bytes.len() != expected {
            return Err(if bytes.len() > MAX_BEACON_BYTES_WITH_PROOF {
                DiscoveryError::MessageTooLarge
            } else {
                DiscoveryError::InvalidBeacon
            });
        }
        let mut cursor = HEADER_BYTES;
        let node_id_bytes = &bytes[cursor..cursor + NODE_ID_BYTES];
        cursor += NODE_ID_BYTES;
        let node_id = std::str::from_utf8(node_id_bytes)
            .map_err(|_| DiscoveryError::InvalidBeacon)?
            .to_string();
        if !valid_node_id(&node_id) {
            return Err(DiscoveryError::InvalidBeacon);
        }
        let identity_xonly: [u8; IDENTITY_BYTES] = bytes[cursor..cursor + IDENTITY_BYTES]
            .try_into()
            .map_err(|_| DiscoveryError::InvalidBeacon)?;
        cursor += IDENTITY_BYTES;
        let beacon_id: [u8; BEACON_ID_BYTES] = bytes[cursor..cursor + BEACON_ID_BYTES]
            .try_into()
            .map_err(|_| DiscoveryError::InvalidBeacon)?;
        cursor += BEACON_ID_BYTES;
        let direct_port = u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]);
        cursor += 2;
        let issued_at = read_u64(bytes, &mut cursor)?;
        let expires_at = read_u64(bytes, &mut cursor)?;
        let sequence = read_u64(bytes, &mut cursor)?;
        let discovery_proof = if flags == PROOF_FLAG {
            let proof: [u8; PROOF_BYTES] = bytes[cursor..cursor + PROOF_BYTES]
                .try_into()
                .map_err(|_| DiscoveryError::InvalidBeacon)?;
            cursor += PROOF_BYTES;
            Some(proof)
        } else {
            None
        };
        let signature: [u8; SIGNATURE_BYTES] = bytes[cursor..cursor + SIGNATURE_BYTES]
            .try_into()
            .map_err(|_| DiscoveryError::InvalidBeacon)?;
        let beacon = Self {
            node_id,
            identity_xonly,
            beacon_id,
            direct_port,
            issued_at,
            expires_at,
            sequence,
            discovery_proof,
            signature,
        };
        beacon.validate_shape()?;
        Ok(beacon)
    }

    pub fn verify(&self, now: u64, secret: Option<&[u8]>) -> Result<(), DiscoveryError> {
        self.validate_shape()?;
        validate_secret(secret)?;
        if self.issued_at > now.saturating_add(FUTURE_SKEW_SECONDS) {
            return Err(DiscoveryError::Future);
        }
        if now >= self.expires_at {
            return Err(DiscoveryError::Expired);
        }
        if let Some(secret) = secret {
            let proof = self.discovery_proof.ok_or(DiscoveryError::SecretMismatch)?;
            if !constant_time_eq(&proof, &hmac_sha256(secret, &self.proof_input())) {
                return Err(DiscoveryError::SecretMismatch);
            }
        } else if self.discovery_proof.is_some() {
            return Err(DiscoveryError::SecretMismatch);
        }
        let expected_node_id = node_id_for_key(&self.identity_xonly);
        if self.node_id != expected_node_id {
            return Err(DiscoveryError::IdentityMismatch);
        }
        let key = VerifyingKey::from_slice(&self.identity_xonly)
            .map_err(|_| DiscoveryError::IdentityMismatch)?;
        let signature =
            Signature::from_slice(&self.signature).map_err(|_| DiscoveryError::SignatureInvalid)?;
        let digest =
            Sha256::digest([BEACON_SIGNATURE_DOMAIN, self.unsigned_bytes().as_slice()].concat());
        key.verify_prehash(&digest, &signature)
            .map_err(|_| DiscoveryError::SignatureInvalid)
    }

    fn validate_shape(&self) -> Result<(), DiscoveryError> {
        if !valid_node_id(&self.node_id)
            || self.direct_port == 0
            || self.expires_at <= self.issued_at
            || self.expires_at - self.issued_at > BEACON_LIFETIME_SECONDS
            || self
                .discovery_proof
                .is_some_and(|proof| proof == [0; PROOF_BYTES])
        {
            return Err(DiscoveryError::InvalidBeacon);
        }
        if node_id_for_key(&self.identity_xonly) != self.node_id {
            return Err(DiscoveryError::IdentityMismatch);
        }
        Ok(())
    }

    fn unsigned_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(UNSIGNED_BYTES + PROOF_BYTES);
        bytes.extend_from_slice(BEACON_MAGIC);
        bytes.push(BEACON_VERSION);
        bytes.push(BEACON_KIND);
        bytes.extend_from_slice(&(u16::from(self.discovery_proof.is_some())).to_be_bytes());
        bytes.extend_from_slice(self.node_id.as_bytes());
        bytes.extend_from_slice(&self.identity_xonly);
        bytes.extend_from_slice(&self.beacon_id);
        bytes.extend_from_slice(&self.direct_port.to_be_bytes());
        bytes.extend_from_slice(&self.issued_at.to_be_bytes());
        bytes.extend_from_slice(&self.expires_at.to_be_bytes());
        bytes.extend_from_slice(&self.sequence.to_be_bytes());
        if let Some(proof) = self.discovery_proof {
            bytes.extend_from_slice(&proof);
        }
        bytes
    }

    fn proof_input(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(UNSIGNED_BYTES);
        bytes.extend_from_slice(DISCOVERY_PROOF_DOMAIN);
        let unsigned = self.unsigned_bytes();
        bytes.extend_from_slice(&unsigned[..UNSIGNED_BYTES]);
        bytes
    }
}

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
struct CandidateRecord {
    node_id: String,
    source_ip: IpAddr,
    direct_port: u16,
    last_seen: u64,
    expires_at: u64,
    sequence: u64,
    beacon_id: [u8; BEACON_ID_BYTES],
}

#[derive(Debug, Clone)]
struct SourceRate {
    seen_at: VecDeque<Instant>,
    last_seen: Instant,
}

#[derive(Debug)]
pub struct DiscoverySnapshot {
    status: DiscoveryStatus,
    candidates: HashMap<(String, IpAddr, u16), CandidateRecord>,
    sources: HashMap<IpAddr, SourceRate>,
    global_seen_at: VecDeque<Instant>,
}

impl DiscoverySnapshot {
    fn new(
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

    fn accept(&mut self, beacon: Beacon, source_ip: IpAddr, now: u64, instant: Instant) {
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

    fn admit_source(&mut self, source: IpAddr, now: Instant) -> bool {
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

    fn prune(&mut self, now: u64, instant: Instant) {
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

pub struct DiscoveryService {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    status: DiscoveryStatusHandle,
}

impl DiscoveryService {
    pub fn start(
        settings: DiscoverySettings,
        context: NodeContext,
        direct_port: Option<u16>,
        secret: Option<String>,
    ) -> Result<Self, DiscoveryError> {
        if !settings.enabled {
            return Ok(Self::disabled(settings, false));
        }
        if !platform_supported() {
            return Err(DiscoveryError::UnsupportedPlatform);
        }
        validate_secret(secret.as_deref().map(str::as_bytes))?;
        if settings.port != DISCOVERY_PORT || settings.multicast_addr != MULTICAST_GROUP.to_string()
        {
            return Err(DiscoveryError::InvalidBeacon);
        }
        let interfaces = local_ipv4_interfaces();
        if interfaces.is_empty() {
            return Err(DiscoveryError::UnsupportedPlatform);
        }
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, settings.port))
            .map_err(|_| DiscoveryError::Internal)?;
        socket
            .set_nonblocking(true)
            .map_err(|_| DiscoveryError::Internal)?;
        let multicast_addr = settings
            .multicast_addr
            .parse::<Ipv4Addr>()
            .map_err(|_| DiscoveryError::InvalidBeacon)?;
        let mut multicast = false;
        for interface in &interfaces {
            if socket
                .join_multicast_v4(&multicast_addr, &interface.address)
                .is_ok()
            {
                multicast = true;
            }
        }
        let send_sockets = interfaces
            .iter()
            .filter_map(|interface| {
                let sender = UdpSocket::bind((interface.address, 0)).ok()?;
                sender.set_nonblocking(true).ok()?;
                let broadcast = if settings.broadcast && interface.broadcast.is_some() {
                    sender.set_broadcast(true).is_ok()
                } else {
                    false
                };
                Some(InterfaceSocket {
                    socket: sender,
                    address: interface.broadcast,
                    broadcast,
                })
            })
            .collect::<Vec<_>>();
        let broadcast = settings.broadcast && send_sockets.iter().any(|sender| sender.broadcast);
        if !multicast && !broadcast {
            return Err(DiscoveryError::UnsupportedPlatform);
        }
        let identity =
            NodeIdentity::load_existing(&context).map_err(|_| DiscoveryError::Internal)?;
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(DiscoverySnapshot::new(
            &settings,
            true,
            true,
            multicast,
            broadcast,
            secret.is_some(),
        )));
        let stop_for_thread = Arc::clone(&stop);
        let status_for_thread = Arc::clone(&status);
        let handle = thread::Builder::new()
            .name("omakure-lan-discovery".to_string())
            .spawn(move || {
                discovery_loop(
                    socket,
                    settings,
                    identity,
                    direct_port,
                    secret,
                    send_sockets,
                    multicast_addr,
                    multicast,
                    broadcast,
                    stop_for_thread,
                    status_for_thread,
                )
            })
            .map_err(|_| DiscoveryError::Internal)?;
        Ok(Self {
            stop,
            handle: Some(handle),
            status,
        })
    }

    pub fn disabled(settings: DiscoverySettings, supported: bool) -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(true)),
            handle: None,
            status: Arc::new(Mutex::new(DiscoverySnapshot::new(
                &settings, supported, false, false, false, false,
            ))),
        }
    }

    pub fn status_without_service(
        settings: &DiscoverySettings,
        supported: bool,
        secret_configured: bool,
    ) -> DiscoveryStatus {
        DiscoverySnapshot::new(settings, supported, false, false, false, secret_configured).status
    }

    pub fn status(&self) -> DiscoveryStatusHandle {
        Arc::clone(&self.status)
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        if let Ok(mut status) = self.status.lock() {
            status.status.listening = false;
        }
    }
}

impl Drop for DiscoveryService {
    fn drop(&mut self) {
        self.stop();
    }
}

// These inputs are the complete owned lifecycle state; grouping them would
// obscure which values are process-only and which are protocol configuration.
#[allow(clippy::too_many_arguments)]
fn discovery_loop(
    socket: UdpSocket,
    settings: DiscoverySettings,
    identity: NodeIdentity,
    direct_port: Option<u16>,
    secret: Option<String>,
    send_sockets: Vec<InterfaceSocket>,
    multicast_addr: Ipv4Addr,
    multicast: bool,
    broadcast: bool,
    stop: Arc<AtomicBool>,
    status: DiscoveryStatusHandle,
) {
    let mut buffer = [0_u8; MAX_DATAGRAM_BYTES];
    let mut beacon_id = [0_u8; BEACON_ID_BYTES];
    OsRng.fill_bytes(&mut beacon_id);
    let mut sequence = 0_u64;
    let local_node_id = identity.public_status().node_id.clone();
    let mut next_send = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        let now_instant = Instant::now();
        if now_instant >= next_send {
            let now = unix_seconds();
            if let Some(direct_port) = direct_port {
                if let Ok(beacon) = Beacon::create(
                    &identity,
                    direct_port,
                    beacon_id,
                    sequence,
                    now,
                    secret.as_deref().map(str::as_bytes),
                ) {
                    if let Ok(bytes) = beacon.encode() {
                        for sender in &send_sockets {
                            if multicast {
                                let _ = sender.socket.send_to(
                                    &bytes,
                                    SocketAddr::new(IpAddr::V4(multicast_addr), settings.port),
                                );
                            }
                            if broadcast && sender.broadcast {
                                let _ = sender.socket.send_to(
                                    &bytes,
                                    SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), settings.port),
                                );
                                if let Some(address) = sender.address {
                                    let _ = sender.socket.send_to(
                                        &bytes,
                                        SocketAddr::new(IpAddr::V4(address), settings.port),
                                    );
                                }
                            }
                        }
                    }
                }
                sequence = sequence.saturating_add(1);
            }
            next_send = now_instant + BEACON_INTERVAL;
        }

        let mut received = 0;
        while received < RECEIVE_BATCH_LIMIT {
            match socket.recv_from(&mut buffer) {
                Ok((size, source)) => {
                    received += 1;
                    process_datagram(
                        &buffer[..size],
                        source,
                        &secret,
                        &status,
                        Some(&local_node_id),
                    );
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        if let Ok(mut snapshot) = status.lock() {
            snapshot.prune(unix_seconds(), Instant::now());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn process_datagram(
    bytes: &[u8],
    source: SocketAddr,
    secret: &Option<String>,
    status: &DiscoveryStatusHandle,
    local_node_id: Option<&str>,
) {
    let now_instant = Instant::now();
    let now = unix_seconds();
    let Ok(mut snapshot) = status.lock() else {
        return;
    };
    if !snapshot.admit_source(source.ip(), now_instant) {
        snapshot.status.dropped_datagrams = snapshot.status.dropped_datagrams.saturating_add(1);
        snapshot.status.last_error = Some(DiscoveryError::RateLimited.to_string());
        return;
    }
    let beacon = match Beacon::parse(bytes) {
        Ok(beacon) => beacon,
        Err(error) => {
            snapshot.status.dropped_datagrams = snapshot.status.dropped_datagrams.saturating_add(1);
            snapshot.status.last_error = Some(error.to_string());
            return;
        }
    };
    if let Err(error) = beacon.verify(now, secret.as_deref().map(str::as_bytes)) {
        snapshot.status.dropped_datagrams = snapshot.status.dropped_datagrams.saturating_add(1);
        snapshot.status.last_error = Some(error.to_string());
        return;
    }
    if local_node_id.is_some_and(|node_id| node_id == beacon.node_id) {
        return;
    }
    snapshot.accept(beacon, source.ip(), now, now_instant);
}

#[derive(Debug, Clone, Copy)]
struct InterfaceAddress {
    address: Ipv4Addr,
    broadcast: Option<Ipv4Addr>,
}

struct InterfaceSocket {
    socket: UdpSocket,
    address: Option<Ipv4Addr>,
    broadcast: bool,
}

#[cfg(unix)]
fn local_ipv4_interfaces() -> Vec<InterfaceAddress> {
    let mut result = Vec::new();
    let mut list = std::ptr::null_mut();
    // SAFETY: libc owns the linked list until freeifaddrs; each sockaddr is
    // checked for AF_INET before it is read as sockaddr_in.
    let returned = unsafe { libc::getifaddrs(&mut list) };
    if returned != 0 {
        return result;
    }
    let mut current = list;
    while !current.is_null() {
        // SAFETY: current is a node from the list returned by getifaddrs.
        let interface = unsafe { &*current };
        if !interface.ifa_addr.is_null()
            && (interface.ifa_flags & libc::IFF_UP as u32) != 0
            && unsafe { (*interface.ifa_addr).sa_family as i32 } == libc::AF_INET
        {
            // SAFETY: AF_INET guarantees sockaddr_in layout.
            let address = unsafe {
                Ipv4Addr::from(u32::from_be(
                    (*((interface.ifa_addr) as *const libc::sockaddr_in))
                        .sin_addr
                        .s_addr,
                ))
            };
            let broadcast = if !interface.ifa_netmask.is_null() {
                // SAFETY: netmask has the same family and layout as ifa_addr.
                let mask = unsafe {
                    u32::from_be(
                        (*((interface.ifa_netmask) as *const libc::sockaddr_in))
                            .sin_addr
                            .s_addr,
                    )
                };
                Some(Ipv4Addr::from((u32::from(address) & mask) | !mask))
            } else {
                None
            };
            if !result
                .iter()
                .any(|entry: &InterfaceAddress| entry.address == address)
            {
                result.push(InterfaceAddress { address, broadcast });
            }
        }
        // SAFETY: current remains within the list until the final free.
        current = unsafe { (*current).ifa_next };
    }
    // SAFETY: list was returned by getifaddrs and has not been freed.
    unsafe { libc::freeifaddrs(list) };
    result.into_iter().take(MAX_INTERFACES).collect()
}

#[cfg(not(unix))]
fn local_ipv4_interfaces() -> Vec<InterfaceAddress> {
    Vec::new()
}

pub fn platform_supported() -> bool {
    cfg!(any(target_os = "linux", target_os = "macos"))
}

fn validate_secret(secret: Option<&[u8]>) -> Result<(), DiscoveryError> {
    if secret.is_some_and(|value| value.is_empty() || value.len() > MAX_DISCOVERY_SECRET_BYTES) {
        return Err(DiscoveryError::SecretInvalid);
    }
    Ok(())
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, DiscoveryError> {
    let end = cursor.checked_add(8).ok_or(DiscoveryError::InvalidBeacon)?;
    let value = bytes
        .get(*cursor..end)
        .ok_or(DiscoveryError::InvalidBeacon)?;
    *cursor = end;
    Ok(u64::from_be_bytes(
        value
            .try_into()
            .map_err(|_| DiscoveryError::InvalidBeacon)?,
    ))
}

fn valid_node_id(value: &str) -> bool {
    value.len() == NODE_ID_BYTES
        && value.starts_with("omk1_")
        && value[5..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn node_id_for_key(key: &[u8; IDENTITY_BYTES]) -> String {
    let mut input = Vec::with_capacity(18 + IDENTITY_BYTES);
    input.extend_from_slice(b"omakure/node-id/v1\0");
    input.extend_from_slice(key);
    let digest = Sha256::digest(input);
    format!(
        "omk1_{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

fn hmac_sha256(secret: &[u8], message: &[u8]) -> [u8; 32] {
    let mut key = [0_u8; 64];
    if secret.len() > key.len() {
        key[..32].copy_from_slice(&Sha256::digest(secret));
    } else {
        key[..secret.len()].copy_from_slice(secret);
    }
    let mut inner = [0x36_u8; 64];
    let mut outer = [0x5c_u8; 64];
    for index in 0..64 {
        inner[index] ^= key[index];
        outer[index] ^= key[index];
    }
    let mut inner_input = Vec::with_capacity(64 + message.len());
    inner_input.extend_from_slice(&inner);
    inner_input.extend_from_slice(message);
    let inner_hash = Sha256::digest(inner_input);
    let mut outer_input = Vec::with_capacity(64 + 32);
    outer_input.extend_from_slice(&outer);
    outer_input.extend_from_slice(&inner_hash);
    Sha256::digest(outer_input).into()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{NodePathOverrides, NodePlatform};
    use tempfile::TempDir;

    fn test_identity() -> (TempDir, NodeIdentity) {
        let temp = TempDir::new().unwrap();
        let context = NodeContext::resolve_for(
            NodePlatform::current(),
            NodePathOverrides::new(
                Some(temp.path().join("state")),
                Some(temp.path().join("node.toml")),
            ),
            true,
            None,
            None,
            None,
        )
        .unwrap();
        let config = crate::domain::NodeConfig::default();
        context.initialize(&config).unwrap();
        let identity = NodeIdentity::load_or_initialize(&context).unwrap();
        (temp, identity)
    }

    #[test]
    fn beacon_round_trip_verifies_identity_and_optional_secret() {
        let (_temp, identity) = test_identity();
        let beacon =
            Beacon::create(&identity, 7988, [1; 16], 1, 1_700_000_000, Some(b"secret")).unwrap();
        let encoded = beacon.encode().unwrap();
        assert_eq!(encoded.len(), MAX_BEACON_BYTES_WITH_PROOF);
        let parsed = Beacon::parse(&encoded).unwrap();
        parsed.verify(1_700_000_001, Some(b"secret")).unwrap();
        assert_eq!(
            parsed.verify(1_700_000_001, Some(b"wrong")),
            Err(DiscoveryError::SecretMismatch)
        );
        assert_eq!(
            parsed.verify(1_700_000_001, None),
            Err(DiscoveryError::SecretMismatch)
        );
    }

    #[test]
    fn beacon_rejects_mutations_and_expiry() {
        let (_temp, identity) = test_identity();
        let beacon = Beacon::create(&identity, 7988, [2; 16], 1, 1_700_000_000, None).unwrap();
        let mut encoded = beacon.encode().unwrap();
        encoded[4] = 2;
        assert_eq!(
            Beacon::parse(&encoded),
            Err(DiscoveryError::UnsupportedVersion)
        );
        let mut expired = beacon.clone();
        expired.expires_at = expired.issued_at + 1;
        assert_eq!(
            expired.verify(1_700_000_001, None),
            Err(DiscoveryError::Expired)
        );

        let mut signed = beacon.encode().unwrap();
        *signed.last_mut().unwrap() ^= 1;
        let parsed = Beacon::parse(&signed).unwrap();
        assert_eq!(
            parsed.verify(1_700_000_001, None),
            Err(DiscoveryError::SignatureInvalid)
        );
        assert_eq!(
            Beacon::parse(&[0; MAX_DATAGRAM_BYTES + 1]),
            Err(DiscoveryError::MessageTooLarge)
        );

        let future = Beacon::create(&identity, 7988, [3; 16], 1, 1_700_000_100, None).unwrap();
        assert_eq!(
            future.verify(1_700_000_000, None),
            Err(DiscoveryError::Future)
        );
        assert_eq!(
            Beacon::create(&identity, 7988, [7; 16], 1, 1_700_000_000, Some(b"")),
            Err(DiscoveryError::SecretInvalid)
        );
        assert_eq!(
            Beacon::create(
                &identity,
                7988,
                [8; 16],
                1,
                1_700_000_000,
                Some(&vec![b'x'; MAX_DISCOVERY_SECRET_BYTES + 1]),
            ),
            Err(DiscoveryError::SecretInvalid)
        );
    }

    #[test]
    fn admission_and_candidate_storage_are_bounded() {
        let settings = DiscoverySettings::default();
        let mut snapshot = DiscoverySnapshot::new(&settings, true, true, true, true, false);
        let now = Instant::now();
        for _ in 0..MAX_SOURCE_DATAGRAMS_PER_SECOND {
            assert!(snapshot.admit_source(IpAddr::V4(Ipv4Addr::LOCALHOST), now));
        }
        assert!(!snapshot.admit_source(IpAddr::V4(Ipv4Addr::LOCALHOST), now));
        assert!(snapshot.candidates.len() <= MAX_CANDIDATES);
    }

    #[test]
    fn malformed_flood_is_rate_limited_before_identity_work() {
        let settings = DiscoverySettings::default();
        let status = Arc::new(Mutex::new(DiscoverySnapshot::new(
            &settings, true, true, true, true, false,
        )));
        let source = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 40_000);
        for _ in 0..MAX_SOURCE_DATAGRAMS_PER_SECOND {
            process_datagram(&[0; 1], source, &None, &status, None);
        }
        process_datagram(&[0; 1], source, &None, &status, None);
        let snapshot = status.lock().unwrap();
        assert_eq!(snapshot.candidates.len(), 0);
        assert_eq!(snapshot.sources.len(), 1);
        assert_eq!(snapshot.status.last_error.as_deref(), Some("rate_limited"));
        assert_eq!(snapshot.status.dropped_datagrams, 9);
    }

    #[test]
    fn spoof_secret_mismatch_and_stale_beacons_never_become_candidates() {
        let (_temp, identity) = test_identity();
        let now = unix_seconds();
        let valid = Beacon::create(&identity, 7988, [4; 16], 1, now, Some(b"secret"))
            .unwrap()
            .encode()
            .unwrap();
        let settings = DiscoverySettings::default();
        let status = Arc::new(Mutex::new(DiscoverySnapshot::new(
            &settings, true, true, true, true, true,
        )));
        let source = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)), 40_001);

        process_datagram(&valid, source, &Some("wrong".to_string()), &status, None);
        assert!(status.lock().unwrap().candidates.is_empty());

        let mut spoofed = valid.clone();
        spoofed[8..8 + NODE_ID_BYTES].copy_from_slice(
            b"omk1_0000000000000000000000000000000000000000000000000000000000000000",
        );
        process_datagram(&spoofed, source, &Some("secret".to_string()), &status, None);
        assert!(status.lock().unwrap().candidates.is_empty());

        let stale = Beacon::create(
            &identity,
            7988,
            [5; 16],
            2,
            now.saturating_sub(BEACON_LIFETIME_SECONDS + 1),
            Some(b"secret"),
        )
        .unwrap()
        .encode()
        .unwrap();
        process_datagram(&stale, source, &Some("secret".to_string()), &status, None);
        let snapshot = status.lock().unwrap();
        assert!(snapshot.candidates.is_empty());
        assert_eq!(snapshot.status.last_error.as_deref(), Some("expired"));
    }

    #[test]
    fn status_redacts_addresses_by_default_and_expires_candidates() {
        let (_temp, identity) = test_identity();
        let settings = DiscoverySettings::default();
        let mut snapshot = DiscoverySnapshot::new(&settings, true, true, true, true, false);
        let beacon = Beacon::create(&identity, 7988, [6; 16], 1, 1_700_000_000, None).unwrap();
        snapshot.accept(
            beacon,
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
            1_700_000_001,
            Instant::now(),
        );
        let redacted = snapshot.public_status(false, 1_700_000_001);
        assert_eq!(redacted.candidate_count, 1);
        assert!(redacted.candidates[0].address.is_none());
        let detailed = snapshot.public_status(true, 1_700_000_001);
        assert_eq!(
            detailed.candidates[0].address.as_deref(),
            Some("192.0.2.10:7988")
        );
        assert!(snapshot
            .public_status(false, 1_700_000_016)
            .candidates
            .is_empty());
    }

    #[test]
    fn platform_support_is_explicit() {
        assert_eq!(
            platform_supported(),
            cfg!(any(target_os = "linux", target_os = "macos"))
        );
    }
}

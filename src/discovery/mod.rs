//! Bounded, trust-neutral LAN discovery.
//!
//! Beacons are signed evidence about a possible direct endpoint. This module
//! never opens a trust/session registry and never authorizes a peer.

use std::net::Ipv4Addr;
use std::time::Duration;
use thiserror::Error;

mod beacon;
mod service;
mod snapshot;

pub use beacon::Beacon;
pub use service::{DiscoveryService, platform_supported};
pub use snapshot::{
    DiscoveryCandidate, DiscoveryLimits, DiscoverySnapshot, DiscoveryStatus, DiscoveryStatusHandle,
};

#[cfg(test)]
mod tests;

pub const BEACON_MAGIC: &[u8; 4] = b"OMKB";
pub const BEACON_VERSION: u8 = 1;
pub const BEACON_KIND: u8 = 1;
pub const BEACON_SIGNATURE_DOMAIN: &[u8] = b"omakure/lan-beacon/v1\0";
pub const DISCOVERY_PROOF_DOMAIN: &[u8] = b"omakure/lan-discovery-proof/v1\0";
pub const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 42, 99);
pub const DISCOVERY_PORT: u16 = 38_383;
pub const MAX_DATAGRAM_BYTES: usize = 512;
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

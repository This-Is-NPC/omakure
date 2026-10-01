use super::service::process_datagram;
use super::*;
use crate::domain::{DiscoverySettings, NODE_ID_BYTES};
use crate::node_identity::NodeIdentity;
use crate::test_support::node_context;
use crate::util::time::unix_seconds;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tempfile::TempDir;

fn test_identity() -> (TempDir, NodeIdentity) {
    let temp = TempDir::new().unwrap();
    let context = node_context(temp.path());
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
    spoofed[8..8 + NODE_ID_BYTES]
        .copy_from_slice(b"omk1_0000000000000000000000000000000000000000000000000000000000000000");
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

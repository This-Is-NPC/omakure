//! Real-UDP adversarial coverage for the bounded discovery receiver.

#[cfg(unix)]
mod unix {
    use omakure::discovery::{
        Beacon, DiscoveryService, DISCOVERY_PORT, MAX_CANDIDATES, MAX_DATAGRAM_BYTES,
        MAX_DISCOVERY_SECRET_BYTES, MAX_SOURCE_ENTRIES,
    };
    use omakure::domain::NodeConfig;
    use omakure::node::{NodeContext, NodePathOverrides, NodePlatform};
    use omakure::node_identity::NodeIdentity;
    use std::net::UdpSocket;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

    #[test]
    fn receiver_rejects_adversarial_datagrams_without_unbounded_state() {
        let temp = TempDir::new().expect("temporary discovery state");
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
        .expect("resolve node context");
        let mut config = NodeConfig::default();
        config.discovery.enabled = true;
        config.discovery.broadcast = false;
        context.initialize(&config).expect("initialize node");
        let _local_identity =
            NodeIdentity::load_or_initialize(&context).expect("initialize identity");
        let peer_temp = TempDir::new().expect("temporary peer state");
        let peer_context = NodeContext::resolve_for(
            NodePlatform::current(),
            NodePathOverrides::new(
                Some(peer_temp.path().join("state")),
                Some(peer_temp.path().join("node.toml")),
            ),
            true,
            None,
            None,
            None,
        )
        .expect("resolve peer context");
        peer_context
            .initialize(&NodeConfig::default())
            .expect("initialize peer");
        let identity =
            NodeIdentity::load_or_initialize(&peer_context).expect("initialize peer identity");
        let mut service = DiscoveryService::start(
            config.discovery,
            context,
            None,
            Some("discovery-secret".to_string()),
        )
        .expect("start discovery receiver");

        let sender = UdpSocket::bind("127.0.0.1:0").expect("bind UDP sender");
        let endpoint = format!("127.0.0.1:{DISCOVERY_PORT}");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_secs();
        let valid = Beacon::create(&identity, 7988, [1; 16], 1, now, Some(b"discovery-secret"))
            .expect("create valid beacon")
            .encode()
            .expect("encode valid beacon");
        sender
            .send_to(&valid, &endpoint)
            .expect("send valid beacon");

        let mut bad_signature = valid.clone();
        *bad_signature.last_mut().expect("signature byte") ^= 1;
        sender
            .send_to(&bad_signature, &endpoint)
            .expect("send bad signature");
        let wrong_secret = Beacon::create(&identity, 7988, [2; 16], 2, now, Some(b"wrong-secret"))
            .expect("create wrong-secret beacon")
            .encode()
            .expect("encode wrong-secret beacon");
        sender
            .send_to(&wrong_secret, &endpoint)
            .expect("send wrong-secret beacon");
        sender
            .send_to(&[0_u8; 3], &endpoint)
            .expect("send truncated beacon");
        sender
            .send_to(&[0_u8; MAX_DATAGRAM_BYTES + 1], &endpoint)
            .expect("send oversize beacon");
        for _ in 0..32 {
            sender
                .send_to(b"not-a-beacon", &endpoint)
                .expect("send malformed flood datagram");
        }

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let handle = service.status();
            let status = handle
                .lock()
                .expect("discovery status")
                .public_status(false, now);
            if status.accepted_datagrams >= 1 && status.dropped_datagrams >= 5 {
                assert!(status.candidate_count <= MAX_CANDIDATES);
                assert!(status
                    .candidates
                    .iter()
                    .all(|candidate| candidate.address.is_none()));
                assert!(status.limits.datagram_bytes == MAX_DATAGRAM_BYTES);
                assert!(status.limits.source_entries == MAX_SOURCE_ENTRIES);
                assert!(status.limits.source_entries <= MAX_SOURCE_ENTRIES);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "receiver did not process datagrams"
            );
            thread::sleep(Duration::from_millis(20));
        }

        service.stop();
        let status = service.status();
        assert!(
            !status
                .lock()
                .expect("stopped discovery status")
                .public_status(false, now)
                .listening
        );
        assert!(MAX_DISCOVERY_SECRET_BYTES >= "discovery-secret".len());
    }
}

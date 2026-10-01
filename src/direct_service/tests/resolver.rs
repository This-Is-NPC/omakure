use super::*;

#[test]
fn blocked_resolution_returns_when_service_stop_is_requested() {
    let _test_lock = RESOLVER_TEST_LOCK.lock().unwrap();
    let resolver = Resolver::start_with_config(Some(blackhole_resolver_config())).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_resolver = Arc::clone(&stop);
    let resolver_for_request = Arc::clone(&resolver);
    let deadline = Instant::now() + Duration::from_secs(10);
    let started = Instant::now();
    let handle = thread::spawn(move || {
        resolver_for_request.resolve("blocked.invalid:7879", deadline, &stop_for_resolver)
    });
    thread::sleep(Duration::from_millis(30));
    stop.store(true, Ordering::SeqCst);
    resolver.cancel();
    assert!(handle.join().unwrap().is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
    resolver.shutdown();
    assert_eq!(ACTIVE_RESOLVER_TASKS.load(Ordering::SeqCst), 0);
    assert_eq!(ACTIVE_RESOLVER_WORKERS.load(Ordering::SeqCst), 0);
}

#[test]
fn timed_out_resolution_joins_all_async_work() {
    let _test_lock = RESOLVER_TEST_LOCK.lock().unwrap();
    let resolver = Resolver::start_with_config(Some(blackhole_resolver_config())).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let deadline = Instant::now() + Duration::from_millis(50);
    let started = Instant::now();
    assert!(resolver
        .resolve("timeout.invalid:7879", deadline, &stop)
        .is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
    resolver.shutdown();
    assert_eq!(ACTIVE_RESOLVER_TASKS.load(Ordering::SeqCst), 0);
    assert_eq!(ACTIVE_RESOLVER_WORKERS.load(Ordering::SeqCst), 0);
}

#[test]
fn repeated_direct_service_start_stop_does_not_accumulate_resolver_workers() {
    let _test_lock = RESOLVER_TEST_LOCK.lock().unwrap();

    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let context = crate::test_support::node_context(temp.path());
    NodeIdentity::load_or_initialize(&context).unwrap();
    let baseline_workers = ACTIVE_RESOLVER_WORKERS.load(Ordering::SeqCst);
    let baseline_tasks = ACTIVE_RESOLVER_TASKS.load(Ordering::SeqCst);
    for _ in 0..8 {
        let mut service = DirectService::start(
            None,
            &["zzzz@blocked.invalid:7879".to_string()],
            context.clone(),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            ACTIVE_RESOLVER_WORKERS.load(Ordering::SeqCst),
            baseline_workers + 1
        );
        service.stop();
        assert_eq!(
            ACTIVE_RESOLVER_WORKERS.load(Ordering::SeqCst),
            baseline_workers
        );
        assert_eq!(ACTIVE_RESOLVER_TASKS.load(Ordering::SeqCst), baseline_tasks);
    }
}

#[test]
fn static_peer_resolution_observes_an_address_change_on_the_next_attempt() {
    use hickory_resolver::config::NameServerConfig;
    use hickory_resolver::proto::op::{Message, MessageType, ResponseCode};
    use hickory_resolver::proto::rr::{rdata::A, RData, Record, RecordType};
    use hickory_resolver::proto::xfer::Protocol;
    use std::net::{Ipv4Addr, UdpSocket};

    let _test_lock = RESOLVER_TEST_LOCK.lock().unwrap();
    let dns = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    dns.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let dns_address = dns.local_addr().unwrap();
    let address = Arc::new(Mutex::new(Ipv4Addr::new(127, 0, 0, 1)));
    let answer_address = Arc::clone(&address);
    let done = Arc::new(AtomicBool::new(false));
    let stop_dns = Arc::clone(&done);
    let server = thread::spawn(move || {
        let mut buffer = [0u8; 512];
        while !stop_dns.load(Ordering::SeqCst) {
            let (length, client) = match dns.recv_from(&mut buffer) {
                Ok(request) => request,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
                Err(error) => panic!("DNS test server failed: {error}"),
            };
            let request = Message::from_vec(&buffer[..length]).unwrap();
            let mut response = Message::new();
            response
                .set_id(request.id())
                .set_message_type(MessageType::Response)
                .set_response_code(ResponseCode::NoError)
                .add_queries(request.queries().to_vec());
            for query in request.queries() {
                if query.query_type() == RecordType::A {
                    let ip = *answer_address.lock().unwrap();
                    response.add_answer(Record::from_rdata(
                        query.name().clone(),
                        600,
                        RData::A(A::from(ip)),
                    ));
                }
            }
            dns.send_to(&response.to_vec().unwrap(), client).unwrap();
        }
    });

    let config = ResolverConfig::from_parts(
        None,
        Vec::new(),
        vec![NameServerConfig::new(dns_address, Protocol::Udp)],
    );
    let resolver = Resolver::start_with_config(Some(config)).unwrap();
    let stop = AtomicBool::new(false);
    let endpoint = "moving.test.:7879";
    let first = resolver
        .resolve(endpoint, Instant::now() + Duration::from_secs(2), &stop)
        .unwrap();
    assert_eq!(
        first,
        vec![SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
            7879
        )]
    );

    *address.lock().unwrap() = Ipv4Addr::new(127, 0, 0, 2);
    let second = resolver
        .resolve(endpoint, Instant::now() + Duration::from_secs(2), &stop)
        .unwrap();
    assert_eq!(
        second,
        vec![SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)),
            7879
        )]
    );

    resolver.shutdown();
    done.store(true, Ordering::SeqCst);
    server.join().unwrap();
}

fn blackhole_resolver_config() -> ResolverConfig {
    use hickory_resolver::config::NameServerConfig;
    use hickory_resolver::proto::xfer::Protocol;
    ResolverConfig::from_parts(
        None,
        Vec::new(),
        vec![NameServerConfig::new(
            SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1)), 53),
            Protocol::Udp,
        )],
    )
}

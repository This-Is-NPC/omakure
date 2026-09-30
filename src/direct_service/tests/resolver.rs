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

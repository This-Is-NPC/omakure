use super::*;

/// One address is bounded before authentication, not after.
///
/// The `DIRECT_MAX_SOURCE_SESSIONS` check in `promote_session` reads like
/// the thing that decides how many distinct nodes may share an address,
/// and it is not: the pre-auth byte budget refuses the next reservation
/// first, so that check is never the binding limit. Removing it would
/// admit no one and would only leave the impression that sharing a host
/// had been dealt with. It stays because it becomes load-bearing again the
/// moment the byte budget is raised, and this records where the real limit
/// is so the next reader looks at the right one.
#[test]
fn one_address_is_bounded_before_authentication_not_after() {
    let admission = Arc::new(AdmissionController {
        state: Mutex::new(AdmissionState::default()),
    });
    let shared = "198.51.100.4".parse::<IpAddr>().unwrap();
    let start = Instant::now();

    let held = (0..DIRECT_MAX_SOURCE_SESSIONS)
        .map(|index| {
            let mut reservation = admission
                .reserve(shared, start)
                .expect("a distinct node behind the shared address");
            admission
                .migrate_node(&mut reservation, &format!("node-{index}"))
                .expect("the identity is under its own cap");
            reservation
                .promote_session()
                .expect("a distinct identity is admitted a session");
            reservation
        })
        .collect::<Vec<_>>();

    // The arrival rate has rolled, so only a standing per-address cap can
    // refuse the next one.
    let after = start + DIRECT_RATE_WINDOW;
    assert!(
        admission.reserve(shared, after).is_none(),
        "the next node behind the shared address reached the handshake, \
             which would make the post-auth session cap the binding limit"
    );
    let state = admission.state.lock().unwrap();
    let reserved = state
        .sources
        .get(&shared)
        .expect("the shared address still holds its sessions")
        .bytes;
    assert!(
        reserved.saturating_add(ADMISSION_BYTES) > DIRECT_MAX_SOURCE_BYTES,
        "the refusal above was not the pre-auth byte budget, so the limit \
             this test names has moved"
    );
    drop(state);
    drop(held);
}

/// The pre-auth budget is all that stands between an unenrolled stranger
/// and the handshake path, so anything narrowed elsewhere has to leave it
/// exactly this strict. No identity is known anywhere in this test; that
/// is the point.
///
/// Three caps enforce the budget and all three land on four: concurrent
/// handshakes, reserved bytes, and arrival rate. So this freezes the
/// behaviour rather than any one cap, and deleting a single one will not
/// redden it. Deleting the per-address dimension will.
#[test]
fn an_unauthenticated_flood_from_one_address_is_still_refused() {
    let admission = Arc::new(AdmissionController {
        state: Mutex::new(AdmissionState::default()),
    });
    let flooder = "203.0.113.7".parse::<IpAddr>().unwrap();
    let bystander = "203.0.113.8".parse::<IpAddr>().unwrap();
    let start = Instant::now();

    let held = (0..DIRECT_MAX_SOURCE_HANDSHAKES)
        .map(|_| {
            admission
                .reserve(flooder, start)
                .expect("the budget admits its own allowance")
        })
        .collect::<Vec<_>>();
    assert!(
        admission.reserve(flooder, start).is_none(),
        "an unauthenticated flood passed the per-address budget"
    );
    assert!(
        admission.reserve(bystander, start).is_some(),
        "the flood starved an unrelated address"
    );

    // Hanging up does not reopen the door. Arrival rate is budgeted apart
    // from concurrency, so a stranger cannot flood by closing and
    // reopening inside the window.
    drop(held);
    assert!(
        admission.reserve(flooder, start).is_none(),
        "the flood was readmitted by closing its own connections"
    );
    assert!(
        admission
            .reserve(flooder, start + DIRECT_RATE_WINDOW)
            .is_some(),
        "a caller was still refused after the rate window rolled"
    );
}

/// Waiting is not a way around the budget either.
///
/// A stranger that respects the arrival rate exactly -- one handshake per
/// window, never hanging up -- is still bounded by what it is holding, so
/// the rate cap is not the only thing standing in front of the handshake
/// path.
#[test]
fn a_patient_stranger_cannot_hold_more_than_its_address_allowance() {
    let admission = Arc::new(AdmissionController {
        state: Mutex::new(AdmissionState::default()),
    });
    let stranger = "203.0.113.9".parse::<IpAddr>().unwrap();
    let start = Instant::now();

    let held = (0..DIRECT_MAX_SOURCE_HANDSHAKES)
        .map(|window| {
            let now = start + DIRECT_RATE_WINDOW * u32::try_from(window).unwrap();
            admission
                .reserve(stranger, now)
                .unwrap_or_else(|| panic!("the arrival rate refused window {window}"))
        })
        .collect::<Vec<_>>();
    let after = start + DIRECT_RATE_WINDOW * u32::try_from(held.len()).unwrap();
    assert!(
        admission.reserve(stranger, after).is_none(),
        "a stranger that waited out every rate window held more than its \
             address allowance"
    );
    drop(held);
}

/// On one host every address in play is the same address, so a node that
/// charged its own dials to it spent the budget that protects it from
/// strangers on its own outgoing links.
#[test]
fn a_nodes_own_dials_leave_the_inbound_budget_to_its_peers() {
    let admission = Arc::new(AdmissionController {
        state: Mutex::new(AdmissionState::default()),
    });
    let shared = "127.0.0.1".parse::<IpAddr>().unwrap();
    let start = Instant::now();

    let dials = (0..DIRECT_MAX_SOURCE_HANDSHAKES)
        .map(|_| admission.reserve_dial().expect("a dial of this node's own"))
        .collect::<Vec<_>>();
    let inbound = (0..DIRECT_MAX_SOURCE_HANDSHAKES)
        .map(|index| {
            admission
                .reserve(shared, start)
                .unwrap_or_else(|| panic!("inbound peer {index} lost its budget to our dials"))
        })
        .collect::<Vec<_>>();
    assert!(
        admission.reserve(shared, start).is_none(),
        "the per-address budget stopped applying to inbound peers"
    );

    drop(inbound);
    drop(dials);
    let state = admission.state.lock().unwrap();
    assert_eq!(
        state.handshakes, 0,
        "a released dial left global handshake capacity behind"
    );
    assert_eq!(
        state.bytes, 0,
        "a released dial left global byte capacity behind"
    );
}

#[test]
fn admission_limits_each_source_without_starving_another_source() {
    let admission = Arc::new(AdmissionController {
        state: Mutex::new(AdmissionState::default()),
    });
    let first = "127.0.0.1:10001".parse::<SocketAddr>().unwrap();
    let second = "127.0.0.2:10001".parse::<SocketAddr>().unwrap();
    let now = Instant::now();
    let first_reservations = (0..DIRECT_MAX_SOURCE_HANDSHAKES)
        .map(|_| admission.reserve(first.ip(), now).unwrap())
        .collect::<Vec<_>>();
    assert!(admission.reserve(first.ip(), now).is_none());
    let second_one = admission.reserve(second.ip(), now).unwrap();
    drop(first_reservations.into_iter().next());
    assert!(admission
        .reserve(first.ip(), now + DIRECT_RATE_WINDOW)
        .is_some());
    drop(second_one);
}

#[test]
fn admission_reservation_releases_handshake_and_byte_capacity() {
    let admission = Arc::new(AdmissionController {
        state: Mutex::new(AdmissionState::default()),
    });
    let mut reservations = Vec::new();
    let max_bytes_reservations = DIRECT_MAX_BYTES / ADMISSION_BYTES;
    for octet in 1..=max_bytes_reservations {
        let source = IpAddr::V6(std::net::Ipv6Addr::from(octet as u128));
        reservations.push(admission.reserve(source, Instant::now()).unwrap());
    }
    assert!(admission
        .reserve("127.0.0.250".parse::<IpAddr>().unwrap(), Instant::now())
        .is_none());
    drop(reservations);
    assert!(admission
        .reserve("127.0.0.250".parse::<IpAddr>().unwrap(), Instant::now())
        .is_some());
}

#[test]
fn admission_prunes_stale_sources_and_bounds_unique_source_churn() {
    let admission = Arc::new(AdmissionController {
        state: Mutex::new(AdmissionState::default()),
    });
    let start = Instant::now();
    for value in 0..(DIRECT_MAX_SOURCE_ENTRIES * 2) {
        let source = IpAddr::V6(std::net::Ipv6Addr::from(value as u128));
        let _ = admission.reserve(source, start);
    }
    let state = admission.state.lock().unwrap();
    assert_eq!(state.sources.len(), DIRECT_MAX_SOURCE_ENTRIES);
    drop(state);

    let active_source = IpAddr::V6(std::net::Ipv6Addr::from(999_000u128));
    let active = admission.reserve(active_source, start + DIRECT_RATE_WINDOW);
    assert!(active.is_some());
    let state = admission.state.lock().unwrap();
    assert!(state.sources.contains_key(&active_source));
    assert!(state.sources.len() <= DIRECT_MAX_SOURCE_ENTRIES);
    drop(state);
    drop(active);

    let later_source = IpAddr::V6(std::net::Ipv6Addr::from(999_001u128));
    assert!(admission
        .reserve(later_source, start + DIRECT_RATE_WINDOW)
        .is_some());
    let state = admission.state.lock().unwrap();
    assert!(state.sources.len() <= DIRECT_MAX_SOURCE_ENTRIES);
    assert!(state.sources.contains_key(&active_source));
}

#[test]
fn admission_migrates_handshake_capacity_to_the_authenticated_node() {
    let admission = Arc::new(AdmissionController {
        state: Mutex::new(AdmissionState::default()),
    });
    let mut reservations = Vec::new();
    for octet in 1..=DIRECT_MAX_SOURCE_SESSIONS {
        let source = IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, octet as u8));
        let mut reservation = admission.reserve(source, Instant::now()).unwrap();
        admission.migrate_node(&mut reservation, "node-a").unwrap();
        reservations.push(reservation);
    }
    let mut rejected = admission
        .reserve(
            IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 1, 1)),
            Instant::now(),
        )
        .unwrap();
    assert_eq!(
        admission.migrate_node(&mut rejected, "node-a"),
        Err(TransportError::RateLimited)
    );
}

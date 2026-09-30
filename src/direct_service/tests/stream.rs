use super::*;

#[test]
fn partial_transfers_and_interrupts_share_one_deadline() {
    let mut calls = 0;
    let mut timeouts = Vec::new();
    transfer_until(
        3,
        Instant::now() + Duration::from_secs(1),
        |offset, timeout| {
            calls += 1;
            timeouts.push(timeout);
            if calls == 2 {
                assert_eq!(offset, 1);
                Err(io::ErrorKind::Interrupted.into())
            } else {
                Ok(1)
            }
        },
    )
    .unwrap();
    assert_eq!(calls, 4);
    assert!(timeouts.windows(2).all(|pair| pair[1] <= pair[0]));
    let error = transfer_until(1, Instant::now(), |_, _| panic!("expired I/O ran")).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
}

#[test]
fn partial_transfer_cannot_return_success_after_deadline() {
    let error = transfer_until(1, Instant::now() + Duration::from_millis(10), |_, _| {
        thread::sleep(Duration::from_millis(20));
        Ok(1)
    })
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
}

fn trickle_frame_expires(prefix_only: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    let frame = Frame::handshake(1, &[7; 32]).unwrap().encode().unwrap();
    let writer = thread::spawn(move || {
        let bytes = if prefix_only {
            &frame[..4]
        } else {
            client.write_all(&frame[..4]).unwrap();
            &frame[4..]
        };
        for byte in bytes {
            if client.write_all(&[*byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(80));
        }
    });
    let started = Instant::now();
    let error = read_frame(&mut server, started + Duration::from_millis(180)).unwrap_err();
    assert!(matches!(error, DirectServiceError::Io(ref error)
        if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)));
    assert!(started.elapsed() < Duration::from_millis(700));
    drop(server);
    writer.join().unwrap();
    // Capacity is immediately reusable for a complete legitimate frame.
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    let valid = Frame::handshake(1, &[9; 32]).unwrap().encode().unwrap();
    client.write_all(&valid).unwrap();
    assert_eq!(
        read_frame(&mut server, Instant::now() + Duration::from_secs(1)).unwrap(),
        valid
    );
}

#[test]
fn trickled_frame_prefix_expires_without_renewing_deadline() {
    trickle_frame_expires(true);
}

#[test]
fn trickled_frame_body_expires_without_renewing_deadline() {
    trickle_frame_expires(false);
}

#[test]
fn truncated_frames_fail_and_fragmented_valid_frames_succeed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    let valid = Frame::handshake(1, &[3; 32]).unwrap().encode().unwrap();
    let expected = valid.clone();
    let writer = thread::spawn(move || {
        for chunk in valid.chunks(3) {
            client.write_all(chunk).unwrap();
        }
        client.write_all(&[0, 0]).unwrap();
    });
    assert_eq!(
        read_frame(&mut server, Instant::now() + Duration::from_secs(2)).unwrap(),
        expected
    );
    let error = read_frame(&mut server, Instant::now() + Duration::from_secs(2)).unwrap_err();
    assert!(
        matches!(error, DirectServiceError::Io(error) if error.kind() == io::ErrorKind::UnexpectedEof)
    );
    writer.join().unwrap();
}

#[test]
fn initiator_deadline_is_one_ten_second_budget() {
    let started = Instant::now();
    let deadline = initiator_deadline(started);
    assert_eq!(deadline.duration_since(started), CONNECT_TIMEOUT);
    thread::sleep(Duration::from_millis(20));
    let remaining = deadline_timeout(deadline).unwrap();
    assert!(remaining < CONNECT_TIMEOUT);
    assert!(remaining > Duration::from_secs(9));
}

use super::super::session::WriteFault;
use super::*;

#[test]
fn noise_xx_round_trip_and_replay_rejection() {
    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    let first_identity = identity(&first);
    let second_identity = identity(&second);
    let first_private = [11u8; 32];
    let second_private = [13u8; 32];
    let now = 1_700_000_000;
    let first_certificate = TransportCertificate::issue(
        &first_identity,
        x25519_public_from_private(&first_private).unwrap(),
        1,
        now - 1,
        now + 1000,
        [1; 16],
    )
    .unwrap();
    let second_certificate = TransportCertificate::issue(
        &second_identity,
        x25519_public_from_private(&second_private).unwrap(),
        1,
        now - 1,
        now + 1000,
        [2; 16],
    )
    .unwrap();
    let mut initiator =
        NoiseHandshake::new(HandshakeRole::Initiator, first_private, first_certificate).unwrap();
    let mut responder =
        NoiseHandshake::new(HandshakeRole::Responder, second_private, second_certificate).unwrap();
    let message_1 = initiator.write_next().unwrap();
    responder.read_next(&message_1, now).unwrap();
    let message_2 = responder.write_next().unwrap();
    initiator.read_next(&message_2, now).unwrap();
    let message_3 = initiator.write_next().unwrap();
    responder.read_next(&message_3, now).unwrap();
    let mut sender = initiator.into_session().unwrap();
    let mut receiver = responder.into_session().unwrap();
    assert_eq!(sender.session_id(), receiver.session_id());
    let frame = sender.write(ENVELOPE_KIND, b"probe").unwrap();
    assert_eq!(receiver.read(&frame).unwrap().body, b"probe");
    assert_eq!(receiver.read(&frame), Err(TransportError::Replay));
}

/// A refusal has to survive the round trip as itself.
///
/// `ERROR_KIND` and its two-byte body have been in the frame parser since
/// version 1, but nothing ever wrote one, so every refusal after a
/// completed handshake reached the peer as a closed socket and became
/// `internal` -- the one code the contract tells a dialer to retry. This is
/// the writer that closes that gap, and the reader that has to recognise
/// what it wrote rather than call it ordinary traffic.
#[test]
fn a_stated_error_survives_the_round_trip_as_the_code_that_was_sent() {
    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    let first_identity = identity(&first);
    let second_identity = identity(&second);
    let first_private = [11u8; 32];
    let second_private = [13u8; 32];
    let now = 1_700_000_000;
    let first_certificate = TransportCertificate::issue(
        &first_identity,
        x25519_public_from_private(&first_private).unwrap(),
        1,
        now - 1,
        now + 1000,
        [1; 16],
    )
    .unwrap();
    let second_certificate = TransportCertificate::issue(
        &second_identity,
        x25519_public_from_private(&second_private).unwrap(),
        1,
        now - 1,
        now + 1000,
        [2; 16],
    )
    .unwrap();
    let mut initiator =
        NoiseHandshake::new(HandshakeRole::Initiator, first_private, first_certificate).unwrap();
    let mut responder =
        NoiseHandshake::new(HandshakeRole::Responder, second_private, second_certificate).unwrap();
    let message_1 = initiator.write_next().unwrap();
    responder.read_next(&message_1, now).unwrap();
    let message_2 = responder.write_next().unwrap();
    initiator.read_next(&message_2, now).unwrap();
    let message_3 = initiator.write_next().unwrap();
    responder.read_next(&message_3, now).unwrap();
    let mut refuser = responder.into_session().unwrap();
    let mut refused = initiator.into_session().unwrap();

    let frame = refuser.write_error(ProtocolErrorCode::Revoked).unwrap();
    assert!(
        refuser.closed,
        "a session that has stated a refusal must not go on talking"
    );
    let message = refused.read(&frame).unwrap();
    assert_eq!(
        stated_error(&message),
        Some(TransportError::Revoked),
        "the code that was sent is the code the peer must read"
    );

    // Ordinary traffic is not a refusal, and must not be read as one.
    let mut other = refuser;
    other.closed = false;
    let ordinary = other.write(ENVELOPE_KIND, b"probe").unwrap();
    let message = refused.read(&ordinary).unwrap();
    assert_eq!(stated_error(&message), None);
}

#[test]
fn failed_writes_preserve_counters_and_close_on_third_safe_failure() {
    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    let first_identity = identity(&first);
    let second_identity = identity(&second);
    let now = 1_700_000_000;
    let first_private = [21u8; 32];
    let second_private = [23u8; 32];
    let first_certificate = TransportCertificate::issue(
        &first_identity,
        x25519_public_from_private(&first_private).unwrap(),
        1,
        now - 1,
        now + 1000,
        [4; 16],
    )
    .unwrap();
    let second_certificate = TransportCertificate::issue(
        &second_identity,
        x25519_public_from_private(&second_private).unwrap(),
        1,
        now - 1,
        now + 1000,
        [5; 16],
    )
    .unwrap();
    let mut initiator =
        NoiseHandshake::new(HandshakeRole::Initiator, first_private, first_certificate).unwrap();
    let mut responder =
        NoiseHandshake::new(HandshakeRole::Responder, second_private, second_certificate).unwrap();
    let message_1 = initiator.write_next().unwrap();
    responder.read_next(&message_1, now).unwrap();
    let message_2 = responder.write_next().unwrap();
    initiator.read_next(&message_2, now).unwrap();
    let message_3 = initiator.write_next().unwrap();
    responder.read_next(&message_3, now).unwrap();
    let mut sender = initiator.into_session().unwrap();

    for attempt in 0..3 {
        sender.inject_write_fault(WriteFault::BeforeEncryption);
        assert_eq!(
            sender.write(ENVELOPE_KIND, b"fault"),
            Err(TransportError::Internal)
        );
        assert_eq!(sender.send_sequence, 0);
        assert_eq!(sender.sent_messages, 0);
        assert_eq!(sender.sent_bytes, 0);
        assert_eq!(sender.closed, attempt == 2);
    }
}

#[test]
fn rekey_threshold_failures_close_on_third_and_successful_rekey_round_trip() {
    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    let first_identity = identity(&first);
    let second_identity = identity(&second);
    let now = 1_700_000_000;
    let first_private = [31u8; 32];
    let second_private = [33u8; 32];
    let first_certificate = TransportCertificate::issue(
        &first_identity,
        x25519_public_from_private(&first_private).unwrap(),
        1,
        now - 1,
        now + 1000,
        [6; 16],
    )
    .unwrap();
    let second_certificate = TransportCertificate::issue(
        &second_identity,
        x25519_public_from_private(&second_private).unwrap(),
        1,
        now - 1,
        now + 1000,
        [7; 16],
    )
    .unwrap();
    let mut initiator =
        NoiseHandshake::new(HandshakeRole::Initiator, first_private, first_certificate).unwrap();
    let mut responder =
        NoiseHandshake::new(HandshakeRole::Responder, second_private, second_certificate).unwrap();
    let message_1 = initiator.write_next().unwrap();
    responder.read_next(&message_1, now).unwrap();
    let message_2 = responder.write_next().unwrap();
    initiator.read_next(&message_2, now).unwrap();
    let message_3 = initiator.write_next().unwrap();
    responder.read_next(&message_3, now).unwrap();
    let mut sender = initiator.into_session().unwrap();
    let mut receiver = responder.into_session().unwrap();
    sender.sent_messages = REKEY_MESSAGES - 1;
    receiver.received_messages = REKEY_MESSAGES - 1;
    let frame = sender.write(ENVELOPE_KIND, b"threshold").unwrap();
    assert_eq!(receiver.read(&frame).unwrap().body, b"threshold");

    receiver.sent_messages = REKEY_MESSAGES - 1;
    sender.received_messages = REKEY_MESSAGES - 1;
    let frame = receiver.write(ENVELOPE_KIND, b"reverse-threshold").unwrap();
    assert_eq!(sender.read(&frame).unwrap().body, b"reverse-threshold");

    sender.sent_messages = REKEY_MESSAGES - 1;
    let before_sequence = sender.send_sequence;
    for attempt in 0..3 {
        sender.inject_write_fault(WriteFault::BeforeEncryption);
        assert_eq!(
            sender.write(ENVELOPE_KIND, b"fault"),
            Err(TransportError::Internal)
        );
        assert_eq!(sender.closed, attempt == 2);
    }
    assert_eq!(sender.send_sequence, before_sequence);
    assert_eq!(sender.sent_messages, REKEY_MESSAGES - 1);
}

use super::*;
use crate::node_identity::NodeIdentity;
use crate::test_support::node_context;
use crate::util::hex;
use tempfile::TempDir;

mod carriage;
mod session;

fn identity(temp: &TempDir) -> NodeIdentity {
    let context = node_context(temp.path());
    NodeIdentity::load_or_initialize(&context).unwrap()
}

#[test]
fn frame_parser_is_bounded_and_exact() {
    let frame = Frame::handshake(1, &[7; 32]).unwrap().encode().unwrap();
    assert_eq!(Frame::parse(&frame).unwrap().body.len(), 33);
    assert_eq!(
        Frame::parse(&frame[..frame.len() - 1]),
        Err(TransportError::InvalidFrame)
    );
    assert_eq!(
        Frame::handshake(1, &[0; MAX_HANDSHAKE_MESSAGE_BYTES + 1]),
        Err(TransportError::MessageTooLarge)
    );
}

#[test]
fn all_low_order_x25519_encodings_are_rejected() {
    let private = [7u8; 32];
    for public in prohibited_x25519_public_keys() {
        assert_eq!(
            validate_x25519_public(&public),
            Err(TransportError::HandshakeFailed)
        );
        assert_eq!(
            x25519_probe(&private, &public),
            Err(TransportError::HandshakeFailed)
        );
    }
    assert!(x25519_probe(&private, &x25519_public_from_private(&private).unwrap()).is_ok());
}

#[test]
fn certificate_binds_identity_and_transport_key() {
    let temp = TempDir::new().unwrap();
    let identity = identity(&temp);
    let private = [9u8; 32];
    let public = x25519_public_from_private(&private).unwrap();
    let certificate =
        TransportCertificate::issue(&identity, public, 1, 1_700_000_000, 1_700_000_100, [4; 16])
            .unwrap();
    assert_eq!(
        TransportCertificate::from_bytes(certificate.as_bytes()).unwrap(),
        certificate
    );
    let mut mutated = *certificate.as_bytes();
    mutated[109] ^= 1;
    assert!(TransportCertificate::from_bytes(&mutated).is_err());
}

#[test]
fn signed_probe_requires_exact_sender_and_nonce() {
    let temp = TempDir::new().unwrap();
    let identity = identity(&temp);
    let session = [3u8; 32];
    let nonce = [5u8; 16];
    let probe = sign_probe(&identity, &session, nonce, 1_700_000_000).unwrap();
    verify_envelope(
        &probe.encoded(),
        &identity.public_status().node_id,
        &identity_key(&identity),
        "probe",
        &session,
        &nonce,
    )
    .unwrap();
    assert_eq!(
        verify_envelope(
            &probe.encoded(),
            &identity.public_status().node_id,
            &identity_key(&identity),
            "ack",
            &session,
            &nonce,
        ),
        Err(TransportError::IdentityMismatch)
    );
}

fn identity_key(identity: &NodeIdentity) -> [u8; 32] {
    hex::decode_array(&identity.public_status().public_key_hex).unwrap()
}

use super::admission::{AdmissionController, AdmissionState};
use super::baseline::{
    BaselineAckMatch, BaselineDispatcher, BaselinePushOutcome, OutboundBaseline, PendingBaseline,
};
use super::connection::{ConnectionDirection, ConnectionOptions, ConnectionState};
use super::cue::{
    CueAckMatch, CueDispatchOutcome, CueDispatcher, OutboundCue, PendingCue, resolve_cue_id,
};
use super::dial::{connect_and_hold, retry_backoff};
use super::error::DirectServiceError;
use super::outbox::{dispatch_answer_deadline, dispatch_client_timeout};
use super::resolver::{ACTIVE_RESOLVER_TASKS, ACTIVE_RESOLVER_WORKERS, Resolver};
use super::session::error_to_transport;
use super::status::{StaticPeer, TransportStatus};
use super::stream::{deadline_timeout, initiator_deadline, read_frame, transfer_until};
use super::*;
use crate::direct_transport::{Frame, TransportError, unix_seconds};
use crate::node::NodeContext;
use crate::node_identity::NodeIdentity;
use crate::node_registry::NodeRegistry;
use crate::node_transport::LocalTransport;
use crate::util::hex;
use hickory_resolver::config::ResolverConfig;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

mod admission;
mod baseline;
mod connection;
mod cue;
mod dial;
mod listener;
mod outbox;
mod resolver;
mod stream;

struct AckFixtures<'a> {
    valid: &'a [u8],
    wrong_kind: &'a [u8],
    peer_node_id: &'a str,
    peer_key: &'a [u8; 32],
    session_id: &'a [u8; 32],
}

fn assert_unmatched_acks<T: std::fmt::Debug + PartialEq, U>(
    fixtures: AckFixtures<'_>,
    expected: T,
    answers: &std::sync::mpsc::Receiver<U>,
    mut absorb: impl FnMut(&[u8], &str, &[u8; 32], &[u8; 32]) -> T,
) {
    let AckFixtures {
        valid,
        wrong_kind,
        peer_node_id,
        peer_key,
        session_id,
    } = fixtures;
    for (body, node_id, key, session) in [
        (wrong_kind, peer_node_id, peer_key, session_id),
        (valid, "wrong-node", peer_key, session_id),
        (valid, peer_node_id, &[0u8; 32], session_id),
        (valid, peer_node_id, peer_key, &[0u8; 32]),
    ] {
        assert_eq!(&absorb(body, node_id, key, session), &expected);
        assert!(answers.try_recv().is_err());
    }
}

static RESOLVER_TEST_LOCK: Mutex<()> = Mutex::new(());

fn test_identity_status(node_id: &str) -> crate::node_identity::NodeIdentityStatus {
    crate::node_identity::NodeIdentityStatus {
        public_key_hex: "00".repeat(32),
        node_id: node_id.to_string(),
    }
}

/// Build a peer identity and its 32-byte identity key, as the handshake
/// would have established them.
fn test_peer_identity(
    temp: &tempfile::TempDir,
) -> (crate::node_identity::NodeIdentity, String, [u8; 32]) {
    let context = crate::test_support::node_context(temp.path());
    let identity =
        crate::node_identity::NodeIdentity::load_or_initialize(&context).expect("peer identity");
    let (node_id, mut key) = {
        let status = identity.public_status();
        let key: [u8; 32] =
            hex::decode_array(&status.public_key_hex).expect("the identity key is lowercase hex");
        (status.node_id.clone(), key)
    };
    let _ = &mut key;
    (identity, node_id, key)
}

/// A trusted node, a live session with it, and then the trust withdrawn.
///
/// Returns the state the dispatchers read, the peer's node id, and the
/// sockets whose lifetime keeps the registered session "live". The stream
/// is never written to: `register` only wants something it can shut down,
/// and every gate under test refuses before a byte would be produced.
fn revoked_peer_with_a_standing_session(
    temp: &tempfile::TempDir,
) -> (Arc<ConnectionState>, String, (TcpStream, TcpStream)) {
    let context = crate::test_support::node_context(temp.path());
    let identity = NodeIdentity::load_or_initialize(&context).expect("initialize the identity");
    LocalTransport::provision_new(&context, &identity).expect("provision transport material");
    let registry =
        NodeRegistry::open_existing(&context, identity.public_status()).expect("open registry");

    // A real second identity, because the registry validates the key.
    let peer_root = temp.path().join("peer");
    std::fs::create_dir_all(&peer_root).expect("peer root");
    let peer_identity =
        NodeIdentity::load_or_initialize(&crate::test_support::node_context(&peer_root))
            .expect("initialize the peer identity");
    let peer_node_id = peer_identity.public_status().node_id.clone();
    registry
        .import_manual_peer_with_transport(
            crate::node_registry::PeerRegistration {
                node_id: peer_node_id.clone(),
                public_key: peer_identity.public_status().public_key_hex.clone(),
                role: crate::node_registry::PeerRole::Performer,
                capabilities: vec!["notifications".to_string(), "remote-run".to_string()],
                source: crate::node_registry::PeerSource::Manual,
                actor: "test".to_string(),
                reason: "trusted for this test".to_string(),
            },
            None,
        )
        .expect("trust the peer");

    let state = ConnectionState::new(
        context,
        &identity,
        &[],
        ConnectionOptions {
            stop: Arc::new(AtomicBool::new(false)),
            listening: true,
            admission: Arc::new(AdmissionController {
                state: Mutex::new(AdmissionState::default()),
            }),
            reporter: None,
            workspace_root: None,
        },
    );

    // A live session with the peer, exactly as the running service holds
    // one. This is the whole point: the "no session with that peer" guard
    // is satisfied, so it cannot be what refuses the instruction.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let client = TcpStream::connect(listener.local_addr().unwrap()).expect("connect");
    let (server, _) = listener.accept().expect("accept");
    let claim = state
        .register(
            &peer_node_id,
            ConnectionDirection::Responder,
            [9; 32],
            &server,
        )
        .expect("register the session");
    std::mem::forget(claim);
    assert!(
        state.active.lock().unwrap().contains_key(&peer_node_id),
        "the session must be live before trust is withdrawn, or this proves nothing"
    );

    registry
        .revoke_peer(&peer_node_id, "operator", "device retired")
        .expect("revoke the peer");

    (state, peer_node_id, (client, server))
}

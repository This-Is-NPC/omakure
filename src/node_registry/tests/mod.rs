use super::bundle::cleanup_enrollment_replays;
use super::open::{
    configure_connection, database_sidecar_paths, ignore_vanished_private_file, is_transient_lock,
};
use super::*;
use crate::enrollment::{ManualEnrollmentRequest, SignedEnrollmentBundle};
use crate::node::NodeContext;
use crate::node::{NodePathOverrides, NodePlatform};
use crate::node_identity::node_id_for_x_only_public_key;
use crate::node_identity::NodeIdentity;
use crate::test_support::node_context;
use crate::util::hex;
use rusqlite::{Connection, TransactionBehavior};
use std::fs;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

mod audit;
mod bundle;
mod open;
mod peers;
mod validate;

fn registration(identity: &NodeIdentity, scalar: u8) -> PeerRegistration {
    let key = k256::schnorr::SigningKey::from_slice(&[scalar; 32]).unwrap();
    let x_only = key.verifying_key().to_bytes();
    let public_key = hex::encode(&x_only);
    let node_id = node_id_for_x_only_public_key(&x_only);
    assert_ne!(node_id, identity.public_status().node_id);
    PeerRegistration {
        node_id,
        public_key,
        role: PeerRole::Performer,
        capabilities: vec!["remote-run".to_string()],
        source: PeerSource::Manual,
        actor: "operator".to_string(),
        reason: "test decision".to_string(),
    }
}

/// A second identity, to stand in for the peer being trusted.
fn remote_identity(temp: &TempDir) -> (NodeContext, NodeIdentity) {
    let remote_context = NodeContext::resolve_for(
        NodePlatform::current(),
        NodePathOverrides::new(
            Some(temp.path().join("remote-state")),
            Some(temp.path().join("remote-node.toml")),
        ),
        true,
        None,
        None,
        None,
    )
    .unwrap();
    let identity = NodeIdentity::load_or_initialize(&remote_context).unwrap();
    (remote_context, identity)
}

const REMOTE_TRANSPORT_PUBLIC: [u8; 32] = [7; 32];

fn remote_certificate(
    identity: &NodeIdentity,
    now: u64,
) -> crate::direct_transport::TransportCertificate {
    crate::direct_transport::TransportCertificate::issue(
        identity,
        REMOTE_TRANSPORT_PUBLIC,
        1,
        now,
        now + 600,
        [1; 16],
    )
    .unwrap()
}

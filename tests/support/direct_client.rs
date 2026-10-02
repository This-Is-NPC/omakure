#[path = "material.rs"]
mod material;

use super::frame;
use omakure::direct_transport::TransportCertificate;
use omakure::node::{NodeContext, NodePathOverrides, NodePlatform};
use omakure::node_identity::NodeIdentity;
use std::net::TcpStream;
use std::path::Path;

pub fn node_material(workspace: &Path) -> (NodeIdentity, [u8; 32], TransportCertificate) {
    let context = NodeContext::resolve_for(
        NodePlatform::current(),
        NodePathOverrides::new(
            Some(workspace.join(".node-state")),
            Some(workspace.join("node.toml")),
        ),
        true,
        None,
        None,
        None,
    )
    .expect("resolve node context");
    material::load(&context)
}

pub fn read_frame(stream: &mut TcpStream) -> Vec<u8> {
    let mut read_stage = frame::ReadStage::Prefix;
    frame::read_frame(stream, None, Some(&mut read_stage)).unwrap_or_else(|error| {
        let label = match read_stage {
            frame::ReadStage::Prefix => "read frame prefix",
            frame::ReadStage::Body => "read frame body",
        };
        panic!("{label}: {error:?}")
    })
}

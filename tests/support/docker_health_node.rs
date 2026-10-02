#[path = "material.rs"]
mod material;

use omakure::direct_transport::TransportCertificate;
use omakure::node::{NodeContext, NodePathOverrides, NodePlatform};
use omakure::node_identity::NodeIdentity;
use std::path::Path;

pub fn node_material(state_dir: &Path) -> (NodeIdentity, [u8; 32], TransportCertificate) {
    let config_path = state_dir.parent().unwrap_or(Path::new(".")).join(format!(
        "{}-harness-node.toml",
        state_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("harness")
    ));
    if !config_path.exists() {
        std::fs::write(&config_path, "version = 1\n").expect("write harness node config");
    }
    let context = NodeContext::resolve_for(
        NodePlatform::current(),
        NodePathOverrides::new(Some(state_dir.to_path_buf()), Some(config_path)),
        true,
        None,
        None,
        None,
    )
    .expect("resolve the harness node context");
    material::load(&context)
}

use crate::health_ids::peer_identity;
use crate::support::{assert_node_success, run_node};
use std::path::Path;

pub fn trust_peer(
    workspace: &Path,
    seed: u8,
    role: &str,
    capabilities: &[&str],
    audit: (&str, &str),
) -> String {
    let (node_id, public_key) = peer_identity(seed);
    let (actor, reason) = audit;
    let mut args = vec![
        "trust".to_string(),
        "--node-id".to_string(),
        node_id.clone(),
        "--public-key".to_string(),
        public_key,
        "--role".to_string(),
        role.to_string(),
        "--actor".to_string(),
        actor.to_string(),
        "--reason".to_string(),
        reason.to_string(),
        "--confirmed".to_string(),
    ];
    for capability in capabilities {
        args.push("--capability".to_string());
        args.push((*capability).to_string());
    }
    let data = assert_node_success(&run_node(workspace, &args));
    assert_eq!(data["state"], "active");
    node_id
}

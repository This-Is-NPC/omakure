//! Fixtures shared by the crate's inline unit tests.

use crate::node::{NodeContext, NodePathOverrides, NodePlatform};
use std::path::Path;

/// A test-mode node context whose state directory and `node.toml` live under
/// `root`.
pub(crate) fn node_context(root: &Path) -> NodeContext {
    NodeContext::resolve_for(
        NodePlatform::current(),
        NodePathOverrides::new(Some(root.join("state")), Some(root.join("node.toml"))),
        true,
        None,
        None,
        None,
    )
    .expect("resolve the node context")
}

/// [`node_context`] with a minimal `node.toml` already written.
pub(crate) fn configured_node_context(root: &Path) -> NodeContext {
    std::fs::write(root.join("node.toml"), "version = 1\n").expect("write config");
    node_context(root)
}

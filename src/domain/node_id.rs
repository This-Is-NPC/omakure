//! The node ID grammar: `omk1_` followed by the 64 lowercase hexadecimal
//! characters of a SHA-256 digest.

use crate::util::hex;

pub const NODE_ID_PREFIX: &str = "omk1_";
pub const NODE_ID_BYTES: usize = NODE_ID_PREFIX.len() + 64;

pub fn is_node_id(value: &str) -> bool {
    value.len() == NODE_ID_BYTES
        && value
            .strip_prefix(NODE_ID_PREFIX)
            .is_some_and(hex::is_lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_prefix_and_sixty_four_lowercase_hex_characters_are_a_node_id() {
        let digest = "0123456789abcdef".repeat(4);
        assert!(is_node_id(&format!("omk1_{digest}")));
        assert!(!is_node_id(&format!("omk2_{digest}")));
        assert!(!is_node_id(&format!("omk1_{}", digest.to_uppercase())));
        assert!(!is_node_id(&format!("omk1_{digest}0")));
        assert!(!is_node_id("omk1_"));
    }
}

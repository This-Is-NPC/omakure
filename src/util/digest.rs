use sha2::{Digest, Sha256};

/// SHA-256 over `domain` followed by `bytes`, so digests from different
/// constructions can never collide.
pub fn sha256_domain(domain: &[u8], bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes);
    digest.finalize().into()
}

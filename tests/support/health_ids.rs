use sha2::{Digest, Sha256};

pub fn peer_identity(seed: u8) -> (String, String) {
    let key = k256::schnorr::SigningKey::from_slice(&[seed; 32]).expect("test scalar");
    let xonly = key.verifying_key().to_bytes();
    let public_key = omakure::hex::encode(&xonly);
    let mut digest = Sha256::new();
    digest.update(b"omakure/node-id/v1\0");
    digest.update(xonly);
    let hash: [u8; 32] = digest.finalize().into();
    (format!("omk1_{}", omakure::hex::encode(&hash)), public_key)
}

pub fn hex16(seed: u64) -> String {
    format!("{seed:032x}")
}

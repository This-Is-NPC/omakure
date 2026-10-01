use rand::rngs::OsRng;
use rand::TryRngCore;

pub fn fill_bytes(bytes: &mut [u8]) {
    OsRng
        .try_fill_bytes(bytes)
        .expect("system randomness is unavailable");
}

pub fn next_u32() -> u32 {
    let mut bytes = [0u8; 4];
    fill_bytes(&mut bytes);
    u32::from_le_bytes(bytes)
}

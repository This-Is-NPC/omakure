pub fn repeated_hex_16(seed: u8) -> String {
    omakure::hex::encode(&[seed; 16])
}

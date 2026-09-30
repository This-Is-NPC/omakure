use super::errors::TransportError;
use curve25519_dalek::{constants::X25519_BASEPOINT, montgomery::MontgomeryPoint};

pub fn x25519_public_from_private(private: &[u8]) -> Result<[u8; 32], TransportError> {
    let scalar: [u8; 32] = private.try_into().map_err(|_| TransportError::Internal)?;
    let public = X25519_BASEPOINT.mul_clamped(scalar).to_bytes();
    validate_x25519_public(&public)?;
    Ok(public)
}

pub fn x25519_probe(private: &[u8], public: &[u8]) -> Result<[u8; 32], TransportError> {
    let scalar: [u8; 32] = private.try_into().map_err(|_| TransportError::Internal)?;
    let public: [u8; 32] = public
        .try_into()
        .map_err(|_| TransportError::HandshakeFailed)?;
    validate_x25519_public(&public)?;
    let shared = MontgomeryPoint(public).mul_clamped(scalar).to_bytes();
    if shared.iter().fold(0u8, |value, byte| value | byte) == 0 {
        return Err(TransportError::HandshakeFailed);
    }
    Ok(shared)
}

pub fn validate_x25519_public(public: &[u8]) -> Result<(), TransportError> {
    let public: [u8; 32] = public
        .try_into()
        .map_err(|_| TransportError::HandshakeFailed)?;
    if prohibited_x25519_public(&public) {
        return Err(TransportError::HandshakeFailed);
    }
    Ok(())
}

fn prohibited_x25519_public(public: &[u8; 32]) -> bool {
    prohibited_x25519_public_keys()
        .iter()
        .fold(0u8, |found, candidate| {
            let difference = candidate
                .iter()
                .zip(public)
                .fold(0u8, |value, (left, right)| value | (left ^ right));
            found | u8::from(difference == 0)
        })
        != 0
}

pub fn prohibited_x25519_public_keys() -> [[u8; 32]; 7] {
    [
        [0; 32],
        [
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ],
        hex_array("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800"),
        hex_array("5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157"),
        hex_array("ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"),
        hex_array("edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"),
        hex_array("eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"),
    ]
}

const fn hex_array(value: &str) -> [u8; 32] {
    let bytes = value.as_bytes();
    let mut output = [0u8; 32];
    let mut index = 0;
    while index < 32 {
        output[index] = (hex_nibble(bytes[index * 2]) << 4) | hex_nibble(bytes[index * 2 + 1]);
        index += 1;
    }
    output
}

const fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        _ => 0,
    }
}

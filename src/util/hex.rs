//! Lowercase hexadecimal text used by identities, digests, and wire payloads.

const DIGITS: &[u8; 16] = b"0123456789abcdef";

pub fn encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

/// `None` for an odd length or any pair that is not a hexadecimal byte.
pub fn decode(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len() / 2)
        .map(|index| pair(value, index))
        .collect()
}

/// `None` unless `value` is exactly `N` hexadecimal bytes.
pub fn decode_array<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 {
        return None;
    }
    let mut bytes = [0u8; N];
    for (index, slot) in bytes.iter_mut().enumerate() {
        *slot = pair(value, index)?;
    }
    Some(bytes)
}

/// Whether every character is one of `0-9a-f`; vacuously true when empty.
pub fn is_lower(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn pair(value: &str, index: usize) -> Option<u8> {
    u8::from_str_radix(value.get(index * 2..index * 2 + 2)?, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_is_lowercase_and_round_trips() {
        let bytes = [0x00, 0x0f, 0xa5, 0xff];
        assert_eq!(encode(&bytes), "000fa5ff");
        assert_eq!(decode("000fa5ff").unwrap(), bytes);
        assert_eq!(decode_array::<4>("000FA5FF"), Some(bytes));
    }

    #[test]
    fn decode_rejects_odd_length_non_hex_and_split_characters() {
        assert_eq!(decode(""), Some(Vec::new()));
        assert_eq!(decode("abc"), None);
        assert_eq!(decode("zz"), None);
        assert_eq!(decode("aéb"), None);
        assert_eq!(decode_array::<2>("00"), None);
    }

    #[test]
    fn is_lower_accepts_only_lowercase_digits() {
        assert!(is_lower("0123456789abcdef"));
        assert!(is_lower(""));
        assert!(!is_lower("ABCDEF"));
        assert!(!is_lower("0g"));
    }
}

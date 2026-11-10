//! Bytes ⇄ bits, most significant bit first. A bit is a `u8` that is 0 or 1.

pub fn bytes_to_bits(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .flat_map(|byte| (0..8).rev().map(move |i| (byte >> i) & 1))
        .collect()
}

/// Packs bits into bytes; the length must be a multiple of 8.
pub fn bits_to_bytes(bits: &[u8]) -> Vec<u8> {
    debug_assert_eq!(bits.len() % 8, 0);
    bits.as_chunks::<8>()
        .0
        .iter()
        .map(|chunk| chunk.iter().fold(0, |byte, bit| (byte << 1) | bit))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_are_msb_first() {
        assert_eq!(bytes_to_bits(&[0b1010_0001]), [1, 0, 1, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn round_trip() {
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(bits_to_bytes(&bytes_to_bits(&bytes)), bytes);
    }
}

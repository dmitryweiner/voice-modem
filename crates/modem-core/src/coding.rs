//! A block of bytes ⇄ MFSK symbol values: convolutional code, interleaver,
//! scrambler, padding to whole symbols.

use crate::bits::{bits_to_bytes, bytes_to_bits};
use crate::conv;

/// Pseudo-noise bits (a 16-bit Galois LFSR, taps 0xB400, seed 0xACE1). XORed
/// over the coded bits so that repeating data does not become one long tone,
/// which vocoders and voice detectors treat as noise.
fn scramble_bits(len: usize) -> impl Iterator<Item = u8> {
    let mut state: u16 = 0xACE1;
    (0..len).map(move |_| {
        let bit = state & 1;
        state >>= 1;
        if bit != 0 {
            state ^= 0xB400;
        }
        bit as u8
    })
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The interleaver: output bit `i` is input bit `i * stride % len`. The stride
/// is the first number from the golden section of `len` up that is coprime to
/// it, so neighbouring bits on the air are far apart in the code.
fn stride(len: usize) -> usize {
    let mut stride = ((len as f64 * 0.618) as usize).max(1);
    while gcd(stride, len) != 1 {
        stride += 1;
    }
    stride
}

fn coded_bits(bytes: usize) -> usize {
    conv::coded_len(bytes * 8)
}

/// Number of symbols that carry a block of `bytes` bytes.
pub fn block_symbols(bytes: usize, bits_per_symbol: usize) -> usize {
    coded_bits(bytes).div_ceil(bits_per_symbol)
}

/// Encodes a block into symbol values of `bits_per_symbol` bits each.
pub fn encode_block(data: &[u8], bits_per_symbol: usize) -> Vec<usize> {
    let coded = conv::encode(&bytes_to_bits(data));
    let len = coded.len();
    let stride = stride(len);
    let padded = block_symbols(data.len(), bits_per_symbol) * bits_per_symbol;
    let air: Vec<u8> = scramble_bits(padded)
        .enumerate()
        .map(|(i, noise)| {
            if i < len {
                coded[i * stride % len] ^ noise
            } else {
                noise
            }
        })
        .collect();
    air.chunks_exact(bits_per_symbol)
        .map(|chunk| {
            chunk
                .iter()
                .fold(0, |value, &bit| (value << 1) | usize::from(bit))
        })
        .collect()
}

/// Decodes the soft bits of `block_symbols(bytes, …)` symbols into `bytes` bytes.
pub fn decode_block(soft: &[f32], bytes: usize) -> Vec<u8> {
    let len = coded_bits(bytes);
    assert!(soft.len() >= len, "not enough soft bits");
    let stride = stride(len);
    let mut llrs = vec![0.0f32; len];
    for (i, noise) in scramble_bits(len).enumerate() {
        llrs[i * stride % len] = if noise == 0 { soft[i] } else { -soft[i] };
    }
    bits_to_bytes(&conv::decode(&llrs, bytes * 8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_soft(symbols: &[usize], bits_per_symbol: usize) -> Vec<f32> {
        symbols
            .iter()
            .flat_map(|&value| {
                (0..bits_per_symbol)
                    .rev()
                    .map(move |bit| if (value >> bit) & 1 == 0 { 1.0 } else { -1.0 })
            })
            .collect()
    }

    #[test]
    fn symbol_counts() {
        // 5 bytes → 40 bits → (40 + 6) · 2 = 92 coded bits → 23 symbols of 4 bits
        assert_eq!(block_symbols(5, 4), 23);
        // 1 byte → 28 coded bits → 10 symbols of 3 bits, 2 bits of padding
        assert_eq!(block_symbols(1, 3), 10);
        assert_eq!(encode_block(&[0; 5], 4).len(), 23);
        assert_eq!(encode_block(&[0; 1], 3).len(), 10);
    }

    #[test]
    fn round_trip() {
        for len in [0usize, 1, 5, 64, 259] {
            let data: Vec<u8> = (0..len).map(|i| (i * 31 + 7) as u8).collect();
            let symbols = encode_block(&data, 4);
            assert!(symbols.iter().all(|&s| s < 16));
            assert_eq!(decode_block(&to_soft(&symbols, 4), len), data, "len {len}");
        }
    }

    #[test]
    fn the_stride_is_a_permutation() {
        for len in [12usize, 92, 100, 2134, 4144] {
            let stride = stride(len);
            let mut seen = vec![false; len];
            for i in 0..len {
                seen[i * stride % len] = true;
            }
            assert!(seen.iter().all(|&s| s), "len {len}");
        }
    }

    #[test]
    fn zeros_do_not_repeat_one_symbol() {
        let symbols = encode_block(&[0; 64], 4);
        let longest_run = symbols.chunk_by(|a, b| a == b).map(<[usize]>::len).max();
        assert!(longest_run < Some(5), "{longest_run:?}");
        let distinct = symbols
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len();
        assert_eq!(distinct, 16);
    }

    #[test]
    fn survives_a_burst_of_lost_symbols() {
        let data: Vec<u8> = (0..100u8).collect();
        let mut soft = to_soft(&encode_block(&data, 4), 4);
        // 6 symbols in a row (24 coded bits) carry nothing
        for llr in &mut soft[400..424] {
            *llr = 0.0;
        }
        assert_eq!(decode_block(&soft, data.len()), data);
    }
}

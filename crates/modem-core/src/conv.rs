//! Convolutional code K=7, rate 1/2, generators 171 and 133 (octal), with a
//! soft-decision Viterbi decoder. The encoder is flushed with K-1 zero bits,
//! so the decoder starts and ends in state 0.
//!
//! Soft bits are log-likelihood ratios: positive means 0, negative means 1,
//! the magnitude is the confidence. Only relative scale matters.

const TAIL: usize = 6;
const STATES: usize = 1 << TAIL;
const GENERATORS: [u8; 2] = [0o171, 0o133];

/// The two output bits for the 7-bit register (newest bit in the lowest position).
fn outputs(register: u8) -> [u8; 2] {
    GENERATORS.map(|g| ((register & g).count_ones() & 1) as u8)
}

/// Number of coded bits for `info_bits` information bits.
pub const fn coded_len(info_bits: usize) -> usize {
    (info_bits + TAIL) * 2
}

pub fn encode(bits: &[u8]) -> Vec<u8> {
    let mut register = 0u8;
    let mut coded = Vec::with_capacity(coded_len(bits.len()));
    for &bit in bits.iter().chain([0u8; TAIL].iter()) {
        register = ((register << 1) | bit) & 0x7F;
        coded.extend(outputs(register));
    }
    coded
}

/// Decodes `coded_len(info_bits)` soft bits into the most likely information bits.
pub fn decode(llrs: &[f32], info_bits: usize) -> Vec<u8> {
    let steps = info_bits + TAIL;
    assert_eq!(llrs.len(), steps * 2, "wrong number of soft bits");

    // metrics[state]: the best path metric ending in `state` (its 6 newest bits).
    let mut metrics = [f32::NEG_INFINITY; STATES];
    metrics[0] = 0.0;
    // decisions[step] bit `state`: the oldest bit of the surviving predecessor.
    let mut decisions: Vec<u64> = Vec::with_capacity(steps);

    for pair in llrs.as_chunks::<2>().0 {
        let mut next = [f32::NEG_INFINITY; STATES];
        let mut decision = 0u64;
        for (state, best) in next.iter_mut().enumerate() {
            let bit = state & 1;
            for oldest in 0..2 {
                let previous = (state >> 1) | (oldest << (TAIL - 1));
                let [a, b] = outputs(((previous << 1) | bit) as u8);
                let branch = if a == 0 { pair[0] } else { -pair[0] }
                    + if b == 0 { pair[1] } else { -pair[1] };
                let metric = metrics[previous] + branch;
                if metric > *best {
                    *best = metric;
                    decision = (decision & !(1 << state)) | ((oldest as u64) << state);
                }
            }
        }
        metrics = next;
        decisions.push(decision);
    }

    let mut bits = vec![0u8; steps];
    let mut state = 0usize;
    for (bit, decision) in bits.iter_mut().zip(&decisions).rev() {
        *bit = (state & 1) as u8;
        let oldest = ((decision >> state) & 1) as usize;
        state = (state >> 1) | (oldest << (TAIL - 1));
    }
    bits.truncate(info_bits);
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_llrs(coded: &[u8]) -> Vec<f32> {
        coded
            .iter()
            .map(|&b| if b == 0 { 1.0 } else { -1.0 })
            .collect()
    }

    fn message(len: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 7 + i / 3) % 5 % 2) as u8).collect()
    }

    #[test]
    fn known_impulse_response() {
        // A single 1 walks through the register: the outputs spell the generators.
        let coded = encode(&[1]);
        assert_eq!(coded, [1, 1, 0, 1, 0, 0, 1, 1, 1, 1, 1, 0, 1, 1]);
    }

    #[test]
    fn coded_length() {
        assert_eq!(encode(&message(40)).len(), coded_len(40));
        assert_eq!(coded_len(40), 92);
    }

    #[test]
    fn decodes_clean_input() {
        for len in [0, 1, 8, 40, 333] {
            let bits = message(len);
            assert_eq!(decode(&to_llrs(&encode(&bits)), len), bits, "len {len}");
        }
    }

    #[test]
    fn corrects_scattered_hard_errors() {
        let bits = message(200);
        let mut llrs = to_llrs(&encode(&bits));
        // The free distance is 10: errors spaced far apart are all corrected.
        for i in (5..llrs.len()).step_by(40) {
            llrs[i] = -llrs[i];
            llrs[i + 3] = -llrs[i + 3];
        }
        assert_eq!(decode(&llrs, bits.len()), bits);
    }

    #[test]
    fn uses_confidence() {
        let bits = message(64);
        let mut llrs = to_llrs(&encode(&bits));
        // Wrong but unsure bits, fewer than the free distance, lose to the confident rest.
        for llr in &mut llrs[20..28] {
            *llr *= -0.05;
        }
        assert_eq!(decode(&llrs, bits.len()), bits);
    }

    #[test]
    fn erasures_are_filled_in() {
        let bits = message(64);
        let mut llrs = to_llrs(&encode(&bits));
        for llr in llrs.iter_mut().skip(3).step_by(4) {
            *llr = 0.0;
        }
        assert_eq!(decode(&llrs, bits.len()), bits);
    }
}

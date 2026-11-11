//! Low-density parity-check codes for the fast modes: the quasi-cyclic codes
//! of IEEE 802.11n with 648-bit code words, rates 1/2 and 3/4, and a
//! soft-decision decoder (layered normalised min-sum).
//!
//! A code word is the information bits followed by the parity bits. The
//! parity-check matrix is a table of 27 × 27 blocks: each is zero or the
//! identity matrix with its columns rotated. A block of bytes is spread
//! evenly over as many code words as it needs; the information bits a code
//! word has no use for are zeros that are not transmitted (shortening), so a
//! short block is protected better than the rate says.
//!
//! Soft bits are as in `conv`: positive means 0, the magnitude is the
//! confidence, only the relative scale matters.

use crate::bits::{bits_to_bytes, bytes_to_bits};
use crate::coding::scramble_bits;

/// Bits of one code word.
pub const CODE_BITS: usize = 648;
/// Size of the blocks of the parity-check matrix.
const Z: usize = 27;
/// Blocks in a row of the table.
const COLUMNS: usize = CODE_BITS / Z;
/// The decoder gives up after this many passes over the checks.
const MAX_ITERATIONS: usize = 40;
/// Min-sum overestimates the confidence of a check; this corrects it.
const NORMALISATION: f32 = 0.8;

type Row = [i8; COLUMNS];

/// Row `r` of a block with the entry `s` has its one in column `(r + s) mod 27`;
/// −1 is a block of zeros.
#[rustfmt::skip]
const HALF: [Row; 12] = [
    [ 0, -1, -1, -1,  0,  0, -1, -1,  0, -1, -1,  0,  1,  0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1],
    [22,  0, -1, -1, 17, -1,  0,  0, 12, -1, -1, -1, -1,  0,  0, -1, -1, -1, -1, -1, -1, -1, -1, -1],
    [ 6, -1,  0, -1, 10, -1, -1, -1, 24, -1,  0, -1, -1, -1,  0,  0, -1, -1, -1, -1, -1, -1, -1, -1],
    [ 2, -1, -1,  0, 20, -1, -1, -1, 25,  0, -1, -1, -1, -1, -1,  0,  0, -1, -1, -1, -1, -1, -1, -1],
    [23, -1, -1, -1,  3, -1, -1, -1,  0, -1,  9, 11, -1, -1, -1, -1,  0,  0, -1, -1, -1, -1, -1, -1],
    [24, -1, 23,  1, 17, -1,  3, -1, 10, -1, -1, -1, -1, -1, -1, -1, -1,  0,  0, -1, -1, -1, -1, -1],
    [25, -1, -1, -1,  8, -1, -1, -1,  7, 18, -1, -1,  0, -1, -1, -1, -1, -1,  0,  0, -1, -1, -1, -1],
    [13, 24, -1, -1,  0, -1,  8, -1,  6, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,  0,  0, -1, -1, -1],
    [ 7, 20, -1, 16, 22, 10, -1, -1, 23, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,  0,  0, -1, -1],
    [11, -1, -1, -1, 19, -1, -1, -1, 13, -1,  3, 17, -1, -1, -1, -1, -1, -1, -1, -1, -1,  0,  0, -1],
    [25, -1,  8, -1, 23, 18, -1, 14,  9, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,  0,  0],
    [ 3, -1, -1, -1, 16, -1, -1,  2, 25,  5, -1, -1,  1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,  0],
];

#[rustfmt::skip]
const THREE_QUARTERS: [Row; 6] = [
    [16, 17, 22, 24,  9,  3, 14, -1,  4,  2,  7, -1, 26, -1,  2, -1, 21, -1,  1,  0, -1, -1, -1, -1],
    [25, 12, 12,  3,  3, 26,  6, 21, -1, 15, 22, -1, 15, -1,  4, -1, -1, 16, -1,  0,  0, -1, -1, -1],
    [25, 18, 26, 16, 22, 23,  9, -1,  0, -1,  4, -1,  4, -1,  8, 23, 11, -1, -1, -1,  0,  0, -1, -1],
    [ 9,  7,  0,  1, 17, -1, -1,  7,  3, -1,  3, 23, -1, 16, -1, -1, 21, -1,  0, -1, -1,  0,  0, -1],
    [24,  5, 26,  7,  1, -1, -1, 15, 24, 15, -1,  8, -1, 13, -1, 13, -1, 11, -1, -1, -1, -1,  0,  0],
    [ 2,  2, 19, 14, 24,  1, 15, 19, -1, 21, -1,  2, -1, 24, -1,  3, -1,  2,  1, -1, -1, -1, -1,  0],
];

/// Share of a code word that is information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rate {
    Half,
    ThreeQuarters,
}

impl Rate {
    const fn table(self) -> &'static [Row] {
        match self {
            Rate::Half => &HALF,
            Rate::ThreeQuarters => &THREE_QUARTERS,
        }
    }

    /// Parity bits of a code word.
    pub const fn parity_bits(self) -> usize {
        self.table().len() * Z
    }

    /// Information bits of a full code word.
    pub const fn info_bits(self) -> usize {
        CODE_BITS - self.parity_bits()
    }
}

/// The ones of a row of the table: block column and rotation.
fn entries(row: &Row) -> impl Iterator<Item = (usize, usize)> + '_ {
    row.iter()
        .enumerate()
        .filter(|&(_, &shift)| shift >= 0)
        .map(|(column, &shift)| (column, shift as usize))
}

/// Position in the code word of the bit that check `r` of a block row meets in a block.
const fn bit_at(column: usize, shift: usize, r: usize) -> usize {
    column * Z + (r + shift) % Z
}

/// The parity bits for `info`: up to `info_bits` bits, the missing ones are zeros.
pub fn parity(rate: Rate, info: &[u8]) -> Vec<u8> {
    let table = rate.table();
    let (rows, info_columns) = (table.len(), COLUMNS - table.len());
    debug_assert!(info.len() <= rate.info_bits());
    let info_bit = |position: usize| info.get(position).copied().unwrap_or(0);

    // What the information bits add up to in every check.
    let mut sums = vec![[0u8; Z]; rows];
    for (sum, row) in sums.iter_mut().zip(table) {
        for (column, shift) in entries(row).filter(|&(column, _)| column < info_columns) {
            for (r, bit) in sum.iter_mut().enumerate() {
                *bit ^= info_bit(bit_at(column, shift, r));
            }
        }
    }
    // The first parity block meets three block rows, the first and the last
    // with the same rotation; every other parity block meets its own row and
    // the next. Added up over all rows everything cancels but the first block.
    let mut first = [0u8; Z];
    for sum in &sums {
        for (bit, s) in first.iter_mut().zip(sum) {
            *bit ^= s;
        }
    }
    let through_first = |row: &Row, r: usize| match row[info_columns] {
        shift if shift >= 0 => first[(r + shift as usize) % Z],
        _ => 0,
    };
    let mut blocks = vec![first];
    let mut previous = [0u8; Z];
    for (sum, row) in sums.iter().zip(table).take(rows - 1) {
        let mut next = [0u8; Z];
        for (r, bit) in next.iter_mut().enumerate() {
            *bit = sum[r] ^ through_first(row, r) ^ previous[r];
        }
        blocks.push(next);
        previous = next;
    }
    blocks.concat()
}

/// True when the bits of a code word satisfy every check.
fn is_code_word(rate: Rate, bits: impl Fn(usize) -> u8) -> bool {
    rate.table().iter().all(|row| {
        (0..Z).all(|r| {
            entries(row).fold(0, |sum, (column, shift)| {
                sum ^ bits(bit_at(column, shift, r))
            }) == 0
        })
    })
}

/// Decodes a code word of which `used` information bits were transmitted:
/// `soft` holds them and the parity bits after them. Returns the `used`
/// information bits — the most likely ones found, right or not: the caller
/// has a checksum.
pub fn decode(rate: Rate, soft: &[f32], used: usize) -> Vec<u8> {
    let table = rate.table();
    let info_bits = rate.info_bits();
    assert_eq!(
        soft.len(),
        used + rate.parity_bits(),
        "wrong number of soft bits"
    );
    // On the scale of the input: its level is anything.
    let mean = soft.iter().map(|llr| llr.abs()).sum::<f32>() / soft.len() as f32;
    let scale = if mean > 0.0 { 1.0 / mean } else { 1.0 };
    // Bits that were not transmitted are known zeros.
    const KNOWN: f32 = 100.0;
    let mut posterior: Vec<f32> = soft[..used]
        .iter()
        .map(|llr| llr * scale)
        .chain(std::iter::repeat_n(KNOWN, info_bits - used))
        .chain(soft[used..].iter().map(|llr| llr * scale))
        .collect();

    // What each check told each of its bits in the previous pass.
    let mut messages = vec![0.0f32; table.len() * COLUMNS * Z];
    let mut incoming = [0.0f32; COLUMNS];
    for _ in 0..MAX_ITERATIONS {
        for (index, row) in table.iter().enumerate() {
            for r in 0..Z {
                // The two least confident bits of the check and the parity of the signs.
                let (mut least, mut second, mut least_at) = (f32::INFINITY, f32::INFINITY, 0);
                let mut negative = false;
                for (column, shift) in entries(row) {
                    let slot = (index * COLUMNS + column) * Z + r;
                    let value = posterior[bit_at(column, shift, r)] - messages[slot];
                    incoming[column] = value;
                    negative ^= value < 0.0;
                    let magnitude = value.abs();
                    if magnitude < least {
                        (second, least, least_at) = (least, magnitude, column);
                    } else if magnitude < second {
                        second = magnitude;
                    }
                }
                for (column, shift) in entries(row) {
                    let slot = (index * COLUMNS + column) * Z + r;
                    let value = incoming[column];
                    let magnitude = if column == least_at { second } else { least };
                    let message = if negative ^ (value < 0.0) {
                        -NORMALISATION * magnitude
                    } else {
                        NORMALISATION * magnitude
                    };
                    messages[slot] = message;
                    posterior[bit_at(column, shift, r)] = value + message;
                }
            }
        }
        if is_code_word(rate, |position| u8::from(posterior[position] < 0.0)) {
            break;
        }
    }
    posterior[..used]
        .iter()
        .map(|&llr| u8::from(llr < 0.0))
        .collect()
}

/// Information bits of each code word of a block of `bytes` bytes: as many
/// code words as it takes, filled evenly, the first ones a bit more.
fn split(rate: Rate, bytes: usize) -> impl Iterator<Item = usize> {
    let bits = bytes * 8;
    let words = bits.div_ceil(rate.info_bits()).max(1);
    (0..words).map(move |word| bits / words + usize::from(word < bits % words))
}

/// Bits on the air for a block of `bytes` bytes.
pub fn block_bits(rate: Rate, bytes: usize) -> usize {
    split(rate, bytes)
        .map(|used| used + rate.parity_bits())
        .sum()
}

/// Encodes a block of bytes into the bits on the air: code word after code
/// word, each as its information bits and its parity bits, all XORed with
/// the scrambler sequence.
pub fn encode_block(rate: Rate, data: &[u8]) -> Vec<u8> {
    let bits = bytes_to_bits(data);
    let mut air = Vec::with_capacity(block_bits(rate, data.len()));
    let mut taken = 0;
    for used in split(rate, data.len()) {
        let info = &bits[taken..taken + used];
        air.extend_from_slice(info);
        air.extend(parity(rate, info));
        taken += used;
    }
    for (bit, noise) in air.iter_mut().zip(scramble_bits(usize::MAX)) {
        *bit ^= noise;
    }
    air
}

/// Decodes the first `block_bits(rate, bytes)` soft bits into `bytes` bytes.
pub fn decode_block(rate: Rate, soft: &[f32], bytes: usize) -> Vec<u8> {
    let len = block_bits(rate, bytes);
    assert!(soft.len() >= len, "not enough soft bits");
    let plain: Vec<f32> = soft[..len]
        .iter()
        .zip(scramble_bits(usize::MAX))
        .map(|(&llr, noise)| if noise == 0 { llr } else { -llr })
        .collect();
    let mut bits = Vec::with_capacity(bytes * 8);
    let mut taken = 0;
    for used in split(rate, bytes) {
        let len = used + rate.parity_bits();
        bits.extend(decode(rate, &plain[taken..taken + len], used));
        taken += len;
    }
    bits_to_bytes(&bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATES: [Rate; 2] = [Rate::Half, Rate::ThreeQuarters];

    /// Repeatable random numbers for the tests (xorshift64).
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn bits(&mut self, len: usize) -> Vec<u8> {
            (0..len).map(|_| (self.next() >> 33) as u8 & 1).collect()
        }

        /// Uniform in (0, 1].
        fn uniform(&mut self) -> f32 {
            ((self.next() >> 40) as f32 + 1.0) / (1u64 << 24) as f32
        }

        fn gaussian(&mut self) -> f32 {
            (-2.0 * self.uniform().ln()).sqrt() * (std::f32::consts::TAU * self.uniform()).cos()
        }
    }

    /// The bits as a receiver sees them: ±1 with noise of the deviation `sigma`.
    fn received(bits: &[u8], sigma: f32, random: &mut Random) -> Vec<f32> {
        bits.iter()
            .map(|&bit| 1.0 - 2.0 * f32::from(bit) + sigma * random.gaussian())
            .collect()
    }

    fn code_word(rate: Rate, info: &[u8]) -> Vec<u8> {
        let mut word = info.to_vec();
        word.resize(rate.info_bits(), 0);
        word.extend(parity(rate, info));
        word
    }

    #[test]
    fn sizes() {
        assert_eq!(
            RATES.map(|rate| (rate.info_bits(), rate.parity_bits())),
            [(324, 324), (486, 162)]
        );
    }

    #[test]
    fn every_code_word_satisfies_every_check() {
        let mut random = Random(1);
        for rate in RATES {
            for used in [0, 1, 100, rate.info_bits()] {
                let word = code_word(rate, &random.bits(used));
                assert_eq!(word.len(), CODE_BITS);
                assert!(is_code_word(rate, |i| word[i]), "{rate:?}, {used} bits");
                // and no other word does: one wrong bit breaks a check
                for flipped in [0, 323, 324, 647] {
                    assert!(
                        !is_code_word(rate, |i| word[i] ^ u8::from(i == flipped)),
                        "{rate:?}, bit {flipped}"
                    );
                }
            }
        }
    }

    #[test]
    fn decodes_clean_input() {
        let mut random = Random(2);
        for rate in RATES {
            for used in [0, 7, rate.info_bits()] {
                let info = random.bits(used);
                let mut soft = received(&info, 0.0, &mut random);
                soft.extend(received(&parity(rate, &info), 0.0, &mut random));
                assert_eq!(decode(rate, &soft, used), info, "{rate:?}, {used} bits");
            }
        }
    }

    #[test]
    fn corrects_what_noise_does() {
        // ±1 in noise of the deviation σ: Eb/N0 = 1/(2σ² · rate). The codes
        // start to work at about 2 dB and 3.5 dB; a decibel and a half more here.
        let mut random = Random(3);
        for (rate, sigma) in [(Rate::Half, 0.667), (Rate::ThreeQuarters, 0.459)] {
            let mut wrong_before = 0;
            for _ in 0..30 {
                let info = random.bits(rate.info_bits());
                let word = code_word(rate, &info);
                let soft = received(&word, sigma, &mut random);
                wrong_before += soft
                    .iter()
                    .zip(&word)
                    .filter(|&(&llr, &bit)| (llr < 0.0) != (bit == 1))
                    .count();
                assert_eq!(decode(rate, &soft, info.len()), info, "{rate:?}");
            }
            // one bit in fifteen (in seventy at rate 3/4) arrived wrong
            assert!(
                wrong_before > 30 * CODE_BITS / 100,
                "{rate:?}: {wrong_before}"
            );
        }
    }

    #[test]
    fn the_level_of_the_input_does_not_matter() {
        let mut random = Random(4);
        let info = random.bits(200);
        let mut word = info.clone();
        word.extend(parity(Rate::Half, &info));
        let soft = received(&word, 0.7, &mut random);
        for gain in [1e-6, 1.0, 1e6] {
            let scaled: Vec<f32> = soft.iter().map(|llr| llr * gain).collect();
            assert_eq!(decode(Rate::Half, &scaled, 200), info, "gain {gain}");
        }
    }

    #[test]
    fn fills_in_lost_bits() {
        let mut random = Random(5);
        let info = random.bits(324);
        let clean = received(&code_word(Rate::Half, &info), 0.3, &mut random);
        // three bits in ten carry nothing
        let mut soft = clean.clone();
        for llr in &mut soft {
            if random.uniform() < 0.3 {
                *llr = 0.0;
            }
        }
        assert_eq!(decode(Rate::Half, &soft, 324), info);
        // a notch in the spectrum: 30 bits in a row are gone, again and again
        let mut soft = clean;
        for start in (50..CODE_BITS - 30).step_by(112) {
            soft[start..start + 30].fill(0.0);
        }
        assert_eq!(decode(Rate::Half, &soft, 324), info);
    }

    #[test]
    fn a_block_is_spread_evenly_over_its_code_words() {
        // 4 bytes fit one word; 41 bytes are 328 bits: two words of 164
        assert_eq!(block_bits(Rate::Half, 0), 324);
        assert_eq!(block_bits(Rate::Half, 4), 32 + 324);
        assert_eq!(block_bits(Rate::Half, 41), 328 + 2 * 324);
        assert_eq!(split(Rate::Half, 41).collect::<Vec<_>>(), [164, 164]);
        // 100 bytes at rate 3/4: 800 bits in two words of 400
        assert_eq!(block_bits(Rate::ThreeQuarters, 100), 800 + 2 * 162);
        // 1027 bytes, the longest block: 8216 bits in 26 words of 316
        assert_eq!(
            split(Rate::Half, 1027).collect::<Vec<_>>(),
            [vec![316; 26]].concat()
        );
        assert_eq!(split(Rate::Half, 122).sum::<usize>(), 976);
        assert_eq!(
            split(Rate::Half, 122).collect::<Vec<_>>(),
            [244, 244, 244, 244]
        );
    }

    #[test]
    fn blocks_round_trip() {
        let mut random = Random(6);
        for rate in RATES {
            for len in [0usize, 1, 40, 41, 259, 1027] {
                let data: Vec<u8> = (0..len).map(|i| (i * 31 + 7) as u8).collect();
                let air = encode_block(rate, &data);
                assert_eq!(air.len(), block_bits(rate, len));
                let mut soft = received(&air, 0.3, &mut random);
                // whatever follows the block is ignored
                soft.extend([0.5; 13]);
                assert_eq!(decode_block(rate, &soft, len), data, "{rate:?}, {len}");
            }
        }
    }

    #[test]
    fn zeros_do_not_go_out_as_zeros() {
        let air = encode_block(Rate::Half, &[0; 40]);
        let ones = air.iter().filter(|&&bit| bit == 1).count();
        assert!(
            (air.len() * 2 / 5..air.len() * 3 / 5).contains(&ones),
            "{ones}"
        );
    }
}

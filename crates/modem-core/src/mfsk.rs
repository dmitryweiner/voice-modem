//! Non-coherent M-ary FSK: one of M tones per symbol, continuous phase.
//!
//! The receiver only compares tone energies, so it needs neither the carrier
//! phase nor the signal level — what survives vocoders and automatic gain
//! control. Symbol values map to tones in Gray code: a drift into the
//! neighbouring tone costs one bit, not several.
//!
//! In a room the tone of a symbol rings on for many symbols. If every symbol
//! used the same tones, that echo would sit exactly where the receiver looks
//! for the next ones, and beyond a few metres it is louder than the direct
//! sound. So consecutive symbols use different sets of tones, interleaved on
//! one grid: the echo of the last `sets - 1` symbols falls between the tones
//! of the current one.

use std::f32::consts::TAU;

use crate::SAMPLE_RATE;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MfskParams {
    /// Bits carried by one tone; a group has `2^bits_per_tone` tones to choose from.
    pub bits_per_tone: usize,
    /// Tones sounding at once, each chosen from a group of its own: the
    /// groups lie side by side in frequency. More groups carry more bits per
    /// symbol, each tone with a smaller share of the power.
    pub groups: usize,
    /// The grid is shared by this many sets of tones used in turn: symbol
    /// `n` takes every `sets`-th frequency starting from number `n % sets`.
    /// The echo of a symbol then falls between the tones of the next ones.
    pub sets: usize,
    /// Symbol length in samples.
    pub symbol_len: usize,
    /// The lowest frequency of the grid, Hz.
    pub first_tone_hz: f32,
    /// Distance between neighbouring frequencies of the grid, Hz. A multiple
    /// of `SAMPLE_RATE / symbol_len` keeps the tones orthogonal.
    pub spacing_hz: f32,
}

impl MfskParams {
    /// Tones in one group.
    pub const fn tones(&self) -> usize {
        1 << self.bits_per_tone
    }

    pub const fn bits_per_symbol(&self) -> usize {
        self.groups * self.bits_per_tone
    }

    /// Frequency of a tone of a group in symbol number `index` of a block.
    pub fn tone_hz(&self, group: usize, tone: usize, index: usize) -> f32 {
        let slot = (group * self.tones() + tone) * self.sets + index % self.sets;
        self.first_tone_hz + self.spacing_hz * slot as f32
    }

    /// The highest frequency of the grid, Hz.
    pub fn last_tone_hz(&self) -> f32 {
        self.first_tone_hz + self.spacing_hz * (self.groups * self.tones() * self.sets - 1) as f32
    }
}

/// The symbol value carried by a tone: neighbouring tones differ in one bit.
const fn value_of(tone: usize) -> usize {
    tone ^ (tone >> 1)
}

/// The tone that carries a symbol value: the inverse of `value_of`.
const fn tone_of(value: usize) -> usize {
    let mut tone = value;
    let mut shifted = value >> 1;
    while shifted != 0 {
        tone ^= shifted;
        shifted >>= 1;
    }
    tone
}

/// Generates the tones, keeping the phase of each group continuous from symbol to symbol.
pub struct Modulator {
    params: MfskParams,
    /// Level of one tone: the tones of all groups together never exceed the asked amplitude.
    amplitude: f32,
    phases: Vec<f32>,
    /// Number of the next symbol.
    index: usize,
}

impl Modulator {
    pub fn new(params: MfskParams, amplitude: f32) -> Self {
        Self {
            params,
            amplitude: amplitude / params.groups as f32,
            phases: vec![0.0; params.groups],
            index: 0,
        }
    }

    /// Appends the samples of one symbol with the given value to `out`: the
    /// first `bits_per_tone` bits of the value choose the tone of the first group.
    pub fn symbol(&mut self, value: usize, out: &mut Vec<f32>) {
        let params = self.params;
        debug_assert!(value < 1 << params.bits_per_symbol());
        let start = out.len();
        out.resize(start + params.symbol_len, 0.0);
        for (group, phase) in self.phases.iter_mut().enumerate() {
            let part = (value >> ((params.groups - 1 - group) * params.bits_per_tone))
                & (params.tones() - 1);
            let step = TAU * params.tone_hz(group, tone_of(part), self.index) / SAMPLE_RATE as f32;
            for sample in &mut out[start..] {
                *sample += self.amplitude * phase.sin();
                *phase = (*phase + step) % TAU;
            }
        }
        self.index += 1;
    }
}

/// Energy of every tone of symbol number `index` of a block, group after
/// group; `freq_offset_hz` is how far the received signal is shifted up from
/// the nominal frequencies.
pub fn tone_energies(
    params: &MfskParams,
    samples: &[f32],
    index: usize,
    freq_offset_hz: f32,
) -> Vec<f32> {
    debug_assert_eq!(samples.len(), params.symbol_len);
    (0..params.groups * params.tones())
        .map(|slot| {
            let hz = params.tone_hz(slot / params.tones(), slot % params.tones(), index)
                + freq_offset_hz;
            let (rotate_sin, rotate_cos) = (TAU * hz / SAMPLE_RATE as f32).sin_cos();
            let (mut cos, mut sin) = (1.0f32, 0.0f32);
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for &x in samples {
                re += x * cos;
                im += x * sin;
                (cos, sin) = (
                    cos * rotate_cos - sin * rotate_sin,
                    sin * rotate_cos + cos * rotate_sin,
                );
            }
            re * re + im * im
        })
        .collect()
}

/// Appends one soft bit per bit of the symbol, first bit of the value first
/// (positive: 0). Max-log on tone amplitudes within each group: the best tone
/// with the bit clear against the best tone with it set.
pub fn soft_bits(params: &MfskParams, energies: &[f32], out: &mut Vec<f32>) {
    for group in energies.chunks_exact(params.tones()) {
        for bit in (0..params.bits_per_tone).rev() {
            let (mut zero, mut one) = (0.0f32, 0.0f32);
            for (tone, energy) in group.iter().enumerate() {
                let amplitude = energy.sqrt();
                if (value_of(tone) >> bit) & 1 == 0 {
                    zero = zero.max(amplitude);
                } else {
                    one = one.max(amplitude);
                }
            }
            out.push(zero - one);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMS: MfskParams = MfskParams {
        bits_per_tone: 4,
        groups: 1,
        sets: 1,
        symbol_len: 512,
        first_tone_hz: 750.0,
        spacing_hz: 125.0,
    };

    fn hard(llrs: &[f32]) -> usize {
        llrs.iter()
            .fold(0, |value, &llr| (value << 1) | usize::from(llr < 0.0))
    }

    #[test]
    fn tone_frequencies() {
        assert_eq!(PARAMS.tones(), 16);
        assert_eq!(PARAMS.tone_hz(0, 0, 0), 750.0);
        assert_eq!(PARAMS.tone_hz(0, 15, 7), 2625.0);
    }

    const HOPPING: MfskParams = MfskParams {
        spacing_hz: 31.25,
        sets: 4,
        ..PARAMS
    };

    #[test]
    fn sets_of_tones_take_turns_on_one_grid() {
        // symbol 0 uses 750, 875, …; symbol 1 the frequencies one grid step up; symbol 4 is back
        assert_eq!(
            [0, 1, 2, 3, 4].map(|index| HOPPING.tone_hz(0, 0, index)),
            [750.0, 781.25, 812.5, 843.75, 750.0]
        );
        assert_eq!(HOPPING.tone_hz(0, 1, 0) - HOPPING.tone_hz(0, 0, 0), 125.0);
        assert_eq!(HOPPING.tone_hz(0, 15, 3), 2718.75);
    }

    #[test]
    fn every_value_comes_back_in_every_set() {
        let mut modulator = Modulator::new(HOPPING, 0.5);
        for index in 0..64 {
            let value = (index * 7) % 16;
            let mut samples = Vec::new();
            modulator.symbol(value, &mut samples);
            let mut llrs = Vec::new();
            soft_bits(
                &HOPPING,
                &tone_energies(&HOPPING, &samples, index, 0.0),
                &mut llrs,
            );
            assert_eq!(hard(&llrs), value, "symbol {index}");
        }
    }

    #[test]
    fn the_previous_symbols_are_not_where_the_receiver_looks() {
        let mut modulator = Modulator::new(HOPPING, 1.0);
        let mut samples = Vec::new();
        for value in [5, 5, 5, 5, 5] {
            modulator.symbol(value, &mut samples);
        }
        let own = tone_energies(&HOPPING, &samples[..512], 0, 0.0);
        let sent = tone_of(5);
        // the same samples seen as symbols 1–3 would be: nothing on any tone
        for index in 1..4 {
            let other = tone_energies(&HOPPING, &samples[..512], index, 0.0);
            assert!(other.iter().all(|&e| e < own[sent] * 1e-4), "set {index}");
        }
        // four symbols later the same tones are in use again
        assert!(tone_energies(&HOPPING, &samples[4 * 512..], 4, 0.0)[sent] > own[sent] * 0.99);
    }

    #[test]
    fn every_value_comes_back() {
        let mut modulator = Modulator::new(PARAMS, 0.5);
        for value in 0..PARAMS.tones() {
            let mut samples = Vec::new();
            modulator.symbol(value, &mut samples);
            assert_eq!(samples.len(), PARAMS.symbol_len);
            let mut llrs = Vec::new();
            soft_bits(
                &PARAMS,
                &tone_energies(&PARAMS, &samples, 0, 0.0),
                &mut llrs,
            );
            assert_eq!(hard(&llrs), value);
        }
    }

    #[test]
    fn only_the_sent_tone_has_energy() {
        let mut samples = Vec::new();
        Modulator::new(PARAMS, 1.0).symbol(0b0110, &mut samples);
        let energies = tone_energies(&PARAMS, &samples, 0, 0.0);
        let sent = tone_of(0b0110);
        let expected = (PARAMS.symbol_len as f32 / 2.0).powi(2);
        assert!((energies[sent] / expected - 1.0).abs() < 0.01);
        for (tone, &energy) in energies.iter().enumerate() {
            assert!(
                tone == sent || energy < expected * 1e-4,
                "tone {tone}: {energy}"
            );
        }
    }

    #[test]
    fn neighbouring_tones_differ_in_one_bit() {
        for tone in 1..PARAMS.tones() {
            assert_eq!(
                (value_of(tone - 1) ^ value_of(tone)).count_ones(),
                1,
                "tone {tone}"
            );
        }
    }

    #[test]
    fn tones_and_values_map_one_to_one() {
        for value in 0..PARAMS.tones() {
            assert_eq!(value_of(tone_of(value)), value);
        }
    }

    #[test]
    fn phase_is_continuous_between_symbols() {
        let mut samples = Vec::new();
        let mut modulator = Modulator::new(PARAMS, 1.0);
        for value in [3, 12, 0, 15, 7] {
            modulator.symbol(value, &mut samples);
        }
        // The largest step of a 2625 Hz sine at 16 kHz is 2·sin(π·2625/16000) ≈ 0.99.
        let largest = samples
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(largest < 1.0, "{largest}");
    }

    #[test]
    fn follows_a_frequency_offset() {
        let shifted = MfskParams {
            first_tone_hz: PARAMS.first_tone_hz + 60.0,
            ..PARAMS
        };
        let mut samples = Vec::new();
        Modulator::new(shifted, 0.5).symbol(9, &mut samples);
        let mut llrs = Vec::new();
        soft_bits(
            &PARAMS,
            &tone_energies(&PARAMS, &samples, 0, 60.0),
            &mut llrs,
        );
        assert_eq!(hard(&llrs), 9);
    }

    #[test]
    fn confidence_grows_with_the_tone() {
        let mut samples = Vec::new();
        Modulator::new(PARAMS, 0.5).symbol(5, &mut samples);
        let (mut loud, mut quiet) = (Vec::new(), Vec::new());
        soft_bits(
            &PARAMS,
            &tone_energies(&PARAMS, &samples, 0, 0.0),
            &mut loud,
        );
        let half: Vec<f32> = samples.iter().map(|x| x * 0.5).collect();
        soft_bits(&PARAMS, &tone_energies(&PARAMS, &half, 0, 0.0), &mut quiet);
        for (a, b) in loud.iter().zip(&quiet) {
            assert!((a / b - 2.0).abs() < 0.01);
        }
    }

    /// Four tones at once, each one of four: 8 bits per symbol.
    const PARALLEL: MfskParams = MfskParams {
        bits_per_tone: 2,
        groups: 4,
        sets: 4,
        ..HOPPING
    };

    #[test]
    fn groups_lie_side_by_side_on_the_grid() {
        assert_eq!(PARALLEL.bits_per_symbol(), 8);
        // a group takes 4 tones × 4 sets = 16 grid steps of 31.25 Hz
        assert_eq!(
            [0, 1, 2, 3].map(|group| PARALLEL.tone_hz(group, 0, 0)),
            [750.0, 1250.0, 1750.0, 2250.0]
        );
        assert_eq!(PARALLEL.tone_hz(3, 3, 3), PARALLEL.last_tone_hz());
        assert_eq!(PARALLEL.last_tone_hz(), 2718.75);
        assert_eq!(HOPPING.last_tone_hz(), 2718.75);
    }

    #[test]
    fn several_tones_at_once_carry_every_value() {
        let mut modulator = Modulator::new(PARALLEL, 0.5);
        for index in 0..256 {
            let value = (index * 37 + 11) % 256;
            let mut samples = Vec::new();
            modulator.symbol(value, &mut samples);
            let peak = samples.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak <= 0.5 + 1e-6, "peak {peak}");
            let mut llrs = Vec::new();
            soft_bits(
                &PARALLEL,
                &tone_energies(&PARALLEL, &samples, index, 0.0),
                &mut llrs,
            );
            assert_eq!(llrs.len(), 8);
            assert_eq!(hard(&llrs), value, "symbol {index}");
        }
    }

    #[test]
    fn each_of_several_tones_has_its_share_of_the_level() {
        let mut samples = Vec::new();
        Modulator::new(PARALLEL, 1.0).symbol(0, &mut samples);
        let energies = tone_energies(&PARALLEL, &samples, 0, 0.0);
        // amplitude 1/4 each: energy (512/2 / 4)² in the first tone of every group
        for group in 0..4 {
            assert!(
                (energies[group * 4] / 4096.0 - 1.0).abs() < 0.01,
                "group {group}: {}",
                energies[group * 4]
            );
        }
        assert!(
            energies
                .iter()
                .enumerate()
                .all(|(slot, &e)| slot % 4 == 0 || e < 1.0)
        );
    }
}

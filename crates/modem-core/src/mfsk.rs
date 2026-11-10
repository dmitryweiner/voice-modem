//! Non-coherent M-ary FSK: one of M tones per symbol, continuous phase.
//!
//! The receiver only compares tone energies, so it needs neither the carrier
//! phase nor the signal level — what survives vocoders and automatic gain
//! control. Symbol values map to tones in Gray code: a drift into the
//! neighbouring tone costs one bit, not several.

use std::f32::consts::TAU;

use crate::SAMPLE_RATE;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MfskParams {
    /// Bits carried by one symbol; there are `2^bits_per_symbol` tones.
    pub bits_per_symbol: usize,
    /// Symbol length in samples.
    pub symbol_len: usize,
    /// Frequency of the lowest tone, Hz.
    pub first_tone_hz: f32,
    /// Distance between neighbouring tones, Hz. A multiple of
    /// `SAMPLE_RATE / symbol_len` keeps the tones orthogonal.
    pub spacing_hz: f32,
}

impl MfskParams {
    pub const fn tones(&self) -> usize {
        1 << self.bits_per_symbol
    }

    pub fn tone_hz(&self, tone: usize) -> f32 {
        self.first_tone_hz + self.spacing_hz * tone as f32
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

/// Generates the tones, keeping the phase continuous from symbol to symbol.
pub struct Modulator {
    params: MfskParams,
    amplitude: f32,
    phase: f32,
}

impl Modulator {
    pub fn new(params: MfskParams, amplitude: f32) -> Self {
        Self {
            params,
            amplitude,
            phase: 0.0,
        }
    }

    /// Appends the samples of one symbol with the given value to `out`.
    pub fn symbol(&mut self, value: usize, out: &mut Vec<f32>) {
        debug_assert!(value < self.params.tones());
        let step = TAU * self.params.tone_hz(tone_of(value)) / SAMPLE_RATE as f32;
        for _ in 0..self.params.symbol_len {
            out.push(self.amplitude * self.phase.sin());
            self.phase = (self.phase + step) % TAU;
        }
    }
}

/// Energy of every tone in one symbol; `freq_offset_hz` is how far the
/// received signal is shifted up from the nominal frequencies.
pub fn tone_energies(params: &MfskParams, samples: &[f32], freq_offset_hz: f32) -> Vec<f32> {
    debug_assert_eq!(samples.len(), params.symbol_len);
    (0..params.tones())
        .map(|tone| {
            let step = TAU * (params.tone_hz(tone) + freq_offset_hz) / SAMPLE_RATE as f32;
            let (rotate_sin, rotate_cos) = step.sin_cos();
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
/// (positive: 0). Max-log on tone amplitudes: the best tone with the bit clear
/// against the best tone with it set.
pub fn soft_bits(params: &MfskParams, energies: &[f32], out: &mut Vec<f32>) {
    for bit in (0..params.bits_per_symbol).rev() {
        let (mut zero, mut one) = (0.0f32, 0.0f32);
        for (tone, energy) in energies.iter().enumerate() {
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

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMS: MfskParams = MfskParams {
        bits_per_symbol: 4,
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
        assert_eq!(PARAMS.tone_hz(0), 750.0);
        assert_eq!(PARAMS.tone_hz(15), 2625.0);
    }

    #[test]
    fn every_value_comes_back() {
        let mut modulator = Modulator::new(PARAMS, 0.5);
        for value in 0..PARAMS.tones() {
            let mut samples = Vec::new();
            modulator.symbol(value, &mut samples);
            assert_eq!(samples.len(), PARAMS.symbol_len);
            let mut llrs = Vec::new();
            soft_bits(&PARAMS, &tone_energies(&PARAMS, &samples, 0.0), &mut llrs);
            assert_eq!(hard(&llrs), value);
        }
    }

    #[test]
    fn only_the_sent_tone_has_energy() {
        let mut samples = Vec::new();
        Modulator::new(PARAMS, 1.0).symbol(0b0110, &mut samples);
        let energies = tone_energies(&PARAMS, &samples, 0.0);
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
        soft_bits(&PARAMS, &tone_energies(&PARAMS, &samples, 60.0), &mut llrs);
        assert_eq!(hard(&llrs), 9);
    }

    #[test]
    fn confidence_grows_with_the_tone() {
        let mut samples = Vec::new();
        Modulator::new(PARAMS, 0.5).symbol(5, &mut samples);
        let (mut loud, mut quiet) = (Vec::new(), Vec::new());
        soft_bits(&PARAMS, &tone_energies(&PARAMS, &samples, 0.0), &mut loud);
        let half: Vec<f32> = samples.iter().map(|x| x * 0.5).collect();
        soft_bits(&PARAMS, &tone_energies(&PARAMS, &half, 0.0), &mut quiet);
        for (a, b) in loud.iter().zip(&quiet) {
            assert!((a / b - 2.0).abs() < 0.01);
        }
    }
}

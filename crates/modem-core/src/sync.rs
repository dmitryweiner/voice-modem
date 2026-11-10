//! The frame preamble: an up-chirp followed by a down-chirp.
//!
//! A chirp correlates sharply with itself at one position only, whatever the
//! level or the phase, so it marks the start of the frame. A frequency offset
//! moves the correlation peak of an up-chirp earlier and of a down-chirp
//! later by the same amount; the pair gives both the true start and the
//! offset.

use std::f32::consts::TAU;

use crate::SAMPLE_RATE;

/// Length of one chirp, samples (128 ms).
pub const CHIRP_LEN: usize = 2048;
/// Length of the whole preamble, samples.
pub const PREAMBLE_LEN: usize = 2 * CHIRP_LEN;
const LOW_HZ: f32 = 600.0;
const HIGH_HZ: f32 = 2600.0;
/// How fast the chirp sweeps, Hz per sample.
pub const HZ_PER_SAMPLE: f32 = (HIGH_HZ - LOW_HZ) / CHIRP_LEN as f32;
/// The ends of a transmitted chirp are faded over this many samples to avoid clicks.
const RAMP: usize = 32;

/// Phase of the up-chirp at sample `n`, radians.
fn phase(n: usize) -> f32 {
    shifted_phase(n, 0.0)
}

/// Phase at sample `n` of an up-chirp moved up by `shift_hz`.
fn shifted_phase(n: usize, shift_hz: f32) -> f32 {
    // In f64: the phase reaches hundreds of radians and f32 would lose the fraction.
    let n = n as f64;
    let cycles = (f64::from(LOW_HZ + shift_hz) * n
        + f64::from(HIGH_HZ - LOW_HZ) * n * n / (2.0 * CHIRP_LEN as f64))
        / f64::from(SAMPLE_RATE);
    (cycles.fract() * std::f64::consts::TAU) as f32
}

/// Index of the up-chirp sample that has the same frequency as sample `n` of a chirp going `up` or down.
fn position(n: usize, up: bool) -> usize {
    if up { n } else { CHIRP_LEN - 1 - n }
}

/// Appends the preamble to `out`.
pub fn preamble(amplitude: f32, out: &mut Vec<f32>) {
    preamble_shifted(amplitude, 0.0, out);
}

/// The preamble as a receiver tuned `shift_hz` too low hears it; for tests.
pub fn preamble_shifted(amplitude: f32, shift_hz: f32, out: &mut Vec<f32>) {
    for up in [true, false] {
        for n in 0..CHIRP_LEN {
            let edge = n.min(CHIRP_LEN - 1 - n);
            let ramp = if edge < RAMP {
                0.5 - 0.5 * (TAU / 2.0 * edge as f32 / RAMP as f32).cos()
            } else {
                1.0
            };
            out.push(amplitude * ramp * shifted_phase(position(n, up), shift_hz).cos());
        }
    }
}

/// A chirp as a complex exponential, to correlate with.
pub struct Reference {
    re: Vec<f32>,
    im: Vec<f32>,
}

impl Reference {
    pub fn new(up: bool) -> Self {
        let (im, re) = (0..CHIRP_LEN)
            .map(|n| phase(position(n, up)).sin_cos())
            .unzip();
        Self { re, im }
    }

    /// How much of the energy of `window` (`CHIRP_LEN` samples) is this chirp:
    /// 1 for a clean chirp at any level and phase, about `2 / CHIRP_LEN` for noise.
    pub fn metric(&self, window: &[f32]) -> f32 {
        debug_assert_eq!(window.len(), CHIRP_LEN);
        let (mut re, mut im, mut energy) = (0.0f32, 0.0f32, 0.0f32);
        for ((&x, &c), &s) in window.iter().zip(&self.re).zip(&self.im) {
            re += x * c;
            im += x * s;
            energy += x * x;
        }
        if energy < 1e-10 {
            0.0
        } else {
            2.0 * (re * re + im * im) / (energy * CHIRP_LEN as f32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() -> Vec<f32> {
        let mut samples = Vec::new();
        preamble(0.5, &mut samples);
        samples
    }

    #[test]
    fn length_and_level() {
        let samples = clean();
        assert_eq!(samples.len(), PREAMBLE_LEN);
        let peak = samples.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak <= 0.5 && peak > 0.49, "{peak}");
        assert!(samples[0].abs() < 1e-3 && samples[PREAMBLE_LEN - 1].abs() < 1e-3);
    }

    #[test]
    fn each_chirp_matches_its_own_reference_only() {
        let samples = clean();
        let (up, down) = (Reference::new(true), Reference::new(false));
        assert!(up.metric(&samples[..CHIRP_LEN]) > 0.95);
        assert!(down.metric(&samples[CHIRP_LEN..]) > 0.95);
        assert!(down.metric(&samples[..CHIRP_LEN]) < 0.02);
        assert!(up.metric(&samples[CHIRP_LEN..]) < 0.02);
    }

    #[test]
    fn the_metric_ignores_level_and_sign() {
        let up = Reference::new(true);
        let samples = clean();
        let expected = up.metric(&samples[..CHIRP_LEN]);
        for gain in [-1.0, 0.001, 30.0] {
            let scaled: Vec<f32> = samples[..CHIRP_LEN].iter().map(|x| x * gain).collect();
            assert!((up.metric(&scaled) - expected).abs() < 1e-3, "gain {gain}");
        }
    }

    #[test]
    fn the_peak_is_sharp() {
        let mut samples = vec![0.0; 100];
        preamble(0.5, &mut samples);
        let up = Reference::new(true);
        let at = |start: usize| up.metric(&samples[start..start + CHIRP_LEN]);
        assert!(at(100) > 0.95);
        for offset in [12, 16, 24, 40, 80] {
            assert!(at(100 - offset) < 0.08, "−{offset}: {}", at(100 - offset));
            assert!(at(100 + offset) < 0.08, "+{offset}: {}", at(100 + offset));
        }
    }

    #[test]
    fn silence_and_tones_do_not_match() {
        let up = Reference::new(true);
        assert_eq!(up.metric(&[0.0; CHIRP_LEN]), 0.0);
        let tone: Vec<f32> = (0..CHIRP_LEN)
            .map(|n| (TAU * 1500.0 * n as f32 / SAMPLE_RATE as f32).sin())
            .collect();
        assert!(up.metric(&tone) < 0.01, "{}", up.metric(&tone));
    }
}

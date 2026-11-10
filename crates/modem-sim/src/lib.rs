//! Channel models for testing the core: what happens to the sound between
//! the speaker of one device and the microphone of the other.
//!
//! A channel takes a whole recording and returns what the other side hears.
//! Everything is deterministic: noise comes from a seeded generator.
//!
//! Not modelled yet: fading, block loss and time stretching (jitter buffers),
//! vocoders — real codecs are run through ffmpeg by `scripts/codecs.mjs`.

use std::f32::consts::TAU;

use modem_core::SAMPLE_RATE;

pub trait Channel {
    /// What comes out of the channel when `input` goes in.
    fn process(&mut self, input: &[f32]) -> Vec<f32>;
}

/// Channels one after another.
#[derive(Default)]
pub struct Chain(Vec<Box<dyn Channel>>);

impl Chain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn then(mut self, channel: impl Channel + 'static) -> Self {
        self.0.push(Box::new(channel));
        self
    }
}

impl Channel for Chain {
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.0
            .iter_mut()
            .fold(input.to_vec(), |signal, channel| channel.process(&signal))
    }
}

/// Root mean square of a signal.
pub fn rms(signal: &[f32]) -> f32 {
    (signal.iter().map(|x| x * x).sum::<f32>() / signal.len().max(1) as f32).sqrt()
}

/// A small deterministic random generator (SplitMix64).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in (0, 1].
    pub fn uniform(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32 + 1.0) / (1u64 << 24) as f32
    }

    /// Normal with mean 0 and variance 1 (Box–Muller).
    pub fn gaussian(&mut self) -> f32 {
        (-2.0 * self.uniform().ln()).sqrt() * (TAU * self.uniform()).cos()
    }
}

/// Additive white Gaussian noise.
pub struct Awgn {
    sigma: f32,
    rng: Rng,
}

impl Awgn {
    /// Noise with the standard deviation `sigma`.
    pub fn new(sigma: f32, seed: u64) -> Self {
        Self {
            sigma,
            rng: Rng::new(seed),
        }
    }

    /// Noise `snr_db` below a signal with the given RMS. The noise is white
    /// up to half the sample rate, so this is the SNR in the 8 kHz band.
    pub fn with_snr(signal_rms: f32, snr_db: f32, seed: u64) -> Self {
        Self::new(signal_rms / 10f32.powf(snr_db / 20.0), seed)
    }
}

impl Channel for Awgn {
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        input
            .iter()
            .map(|x| x + self.sigma * self.rng.gaussian())
            .collect()
    }
}

/// Multiplies the signal by a constant.
pub struct Gain(pub f32);

impl Channel for Gain {
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        input.iter().map(|x| x * self.0).collect()
    }
}

/// Hard clipping at ±level: an overdriven speaker or microphone.
pub struct Clip(pub f32);

impl Channel for Clip {
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        input.iter().map(|x| x.clamp(-self.0, self.0)).collect()
    }
}

/// Silence before and after the signal.
pub struct Pad {
    pub before: usize,
    pub after: usize,
}

impl Channel for Pad {
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let mut output = vec![0.0; self.before];
        output.extend_from_slice(input);
        output.resize(output.len() + self.after, 0.0);
        output
    }
}

/// Echoes: the direct sound plus delayed, scaled copies (delay in samples, gain).
pub struct Echo(pub Vec<(usize, f32)>);

impl Channel for Echo {
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let longest = self.0.iter().map(|&(delay, _)| delay).max().unwrap_or(0);
        let mut output = input.to_vec();
        output.resize(input.len() + longest, 0.0);
        for &(delay, gain) in &self.0 {
            for (i, x) in input.iter().enumerate() {
                output[i + delay] += gain * x;
            }
        }
        output
    }
}

/// A second-order filter section (RBJ cookbook), direct form 1.
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
}

impl Biquad {
    fn new(high_pass: bool, cutoff_hz: f32, q: f32) -> Self {
        let w = TAU * cutoff_hz / SAMPLE_RATE as f32;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b = if high_pass {
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0]
        } else {
            [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0]
        };
        Self {
            b: b.map(|c| c / a0),
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
        }
    }

    fn process(&self, input: &[f32]) -> Vec<f32> {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
        input
            .iter()
            .map(|&x| {
                let y = self.b[0] * x + self.b[1] * x1 + self.b[2] * x2
                    - self.a[0] * y1
                    - self.a[1] * y2;
                (x2, x1, y2, y1) = (x1, x, y1, y);
                y
            })
            .collect()
    }
}

/// A band-pass filter: fourth-order Butterworth slopes on both sides, like
/// the 300–3400 Hz of a telephone line.
pub struct BandPass {
    sections: Vec<Biquad>,
}

impl BandPass {
    pub fn new(low_hz: f32, high_hz: f32) -> Self {
        // The two sections of a fourth-order Butterworth filter.
        const Q: [f32; 2] = [0.541_196_1, 1.306_563];
        let sections = Q
            .iter()
            .flat_map(|&q| [Biquad::new(true, low_hz, q), Biquad::new(false, high_hz, q)])
            .collect();
        Self { sections }
    }

    pub fn telephone() -> Self {
        Self::new(300.0, 3400.0)
    }
}

impl Channel for BandPass {
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.sections
            .iter()
            .fold(input.to_vec(), |signal, section| section.process(&signal))
    }
}

/// Moves every frequency by the same number of hertz, as a mistuned SSB
/// receiver does. Built on a Hilbert transformer; exact from about 100 Hz to
/// 7900 Hz. The output is the same length and is not delayed.
pub struct FreqShift(pub f32);

impl Channel for FreqShift {
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        const HALF: usize = 127;
        // Odd taps of the ideal Hilbert transformer 2/(πk), Hamming window.
        let taps: Vec<(usize, f32)> = (1..=HALF)
            .step_by(2)
            .map(|k| {
                let window = 0.54 + 0.46 * (TAU / 2.0 * k as f32 / (HALF + 1) as f32).cos();
                (k, 2.0 / (TAU / 2.0 * k as f32) * window)
            })
            .collect();
        let at = |i: isize| {
            if i >= 0 && (i as usize) < input.len() {
                input[i as usize]
            } else {
                0.0
            }
        };
        let step = f64::from(self.0) / f64::from(SAMPLE_RATE);
        (0..input.len())
            .map(|n| {
                let i = n as isize;
                let quadrature: f32 = taps
                    .iter()
                    .map(|&(k, tap)| tap * (at(i - k as isize) - at(i + k as isize)))
                    .sum();
                let (sin, cos) = ((step * n as f64).fract() * std::f64::consts::TAU).sin_cos();
                input[n] * cos as f32 - quadrature * sin as f32
            })
            .collect()
    }
}

/// The receiving sound card runs faster (positive) or slower than the sending
/// one by so many parts per million: the recording is resampled.
pub struct ClockOffset {
    pub ppm: f32,
}

impl Channel for ClockOffset {
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let ratio = 1.0 + f64::from(self.ppm) * 1e-6;
        let len = (input.len() as f64 * ratio).round() as usize;
        let at = |i: isize| {
            if i >= 0 && (i as usize) < input.len() {
                input[i as usize]
            } else {
                0.0
            }
        };
        (0..len)
            .map(|n| {
                let position = n as f64 / ratio;
                let i = position.floor() as isize;
                let t = (position - position.floor()) as f32;
                // Catmull-Rom interpolation through the four nearest samples.
                let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
                p1 + 0.5
                    * t
                    * (p2 - p0
                        + t * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3
                            + t * (3.0 * (p1 - p2) + p3 - p0)))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| (TAU * hz * n as f32 / SAMPLE_RATE as f32).sin())
            .collect()
    }

    /// Amplitude of the `hz` component of the middle part of a signal.
    fn level(signal: &[f32], hz: f32) -> f32 {
        let middle = &signal[signal.len() / 4..signal.len() * 3 / 4];
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, &x) in middle.iter().enumerate() {
            let (sin, cos) = (std::f64::consts::TAU * f64::from(hz) * n as f64
                / f64::from(SAMPLE_RATE))
            .sin_cos();
            re += f64::from(x) * cos;
            im += f64::from(x) * sin;
        }
        (2.0 * (re * re + im * im).sqrt() / middle.len() as f64) as f32
    }

    #[test]
    fn random_numbers_are_repeatable_and_normal() {
        let draw = |seed| {
            let mut rng = Rng::new(seed);
            (0..100_000).map(|_| rng.gaussian()).collect::<Vec<f32>>()
        };
        let values = draw(1);
        assert_eq!(values, draw(1));
        assert_ne!(values, draw(2));
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        assert!(mean.abs() < 0.01, "{mean}");
        assert!((rms(&values) - 1.0).abs() < 0.01, "{}", rms(&values));
        let beyond_two =
            values.iter().filter(|v| v.abs() > 2.0).count() as f32 / values.len() as f32;
        assert!((beyond_two - 0.0455).abs() < 0.003, "{beyond_two}");
    }

    #[test]
    fn noise_has_the_asked_level() {
        let noise = Awgn::new(0.25, 7).process(&vec![0.0; 50_000]);
        assert!((rms(&noise) - 0.25).abs() < 0.005);
        // a sine of amplitude 1 has the RMS 0.707; 6 dB below is 0.354
        let noise = Awgn::with_snr(0.707, 6.02, 7).process(&vec![0.0; 50_000]);
        assert!((rms(&noise) - 0.3535).abs() < 0.005, "{}", rms(&noise));
    }

    #[test]
    fn gain_clip_and_pad() {
        assert_eq!(Gain(0.5).process(&[1.0, -2.0]), [0.5, -1.0]);
        assert_eq!(Clip(0.3).process(&[1.0, -2.0, 0.1]), [0.3, -0.3, 0.1]);
        assert_eq!(
            Pad {
                before: 2,
                after: 1
            }
            .process(&[1.0]),
            [0.0, 0.0, 1.0, 0.0]
        );
    }

    #[test]
    fn echo_adds_delayed_copies() {
        let output = Echo(vec![(2, 0.5), (3, -0.25)]).process(&[1.0, 0.0, 0.0]);
        assert_eq!(output, [1.0, 0.0, 0.5, -0.25, 0.0, 0.0]);
    }

    #[test]
    fn telephone_band() {
        let mut filter = BandPass::telephone();
        let mut pass = |hz| level(&filter.process(&tone(hz, 8000)), hz);
        assert!((pass(1000.0) - 1.0).abs() < 0.02, "{}", pass(1000.0));
        assert!(pass(2500.0) > 0.9);
        // the cutoffs are at −3 dB
        assert!((pass(300.0) - 0.707).abs() < 0.03, "{}", pass(300.0));
        assert!((pass(3400.0) - 0.707).abs() < 0.03, "{}", pass(3400.0));
        assert!(pass(100.0) < 0.02, "{}", pass(100.0));
        assert!(pass(6000.0) < 0.1, "{}", pass(6000.0));
    }

    #[test]
    fn frequency_shift_moves_a_tone() {
        for shift in [50.0, -120.0] {
            // the measured half second holds a whole number of periods of every difference
            let output = FreqShift(shift).process(&tone(1000.0, 16_000));
            assert_eq!(output.len(), 16_000);
            assert!(
                (level(&output, 1000.0 + shift) - 1.0).abs() < 0.02,
                "{shift}: {}",
                level(&output, 1000.0 + shift)
            );
            // neither the original nor the mirror image is left
            assert!(level(&output, 1000.0) < 0.02, "{shift}");
            assert!(level(&output, 1000.0 - shift) < 0.02, "{shift}");
        }
    }

    #[test]
    fn clock_offset_stretches_the_recording() {
        let input = tone(1000.0, 160_000);
        let fast = ClockOffset { ppm: 1000.0 }.process(&input);
        assert_eq!(fast.len(), 160_160);
        // more samples per period: the tone sounds 1 Hz lower to the receiver
        assert!(level(&fast, 999.0) > 0.98, "{}", level(&fast, 999.0));
        assert!(level(&fast, 1000.0) < 0.05, "{}", level(&fast, 1000.0));
        assert_eq!(ClockOffset { ppm: -1000.0 }.process(&input).len(), 159_840);
        let same = ClockOffset { ppm: 0.0 }.process(&input);
        assert!(same.iter().zip(&input).all(|(a, b)| (a - b).abs() < 1e-6));
    }

    #[test]
    fn a_chain_applies_channels_in_order() {
        let mut chain = Chain::new().then(Gain(4.0)).then(Clip(1.0)).then(Gain(0.5));
        assert_eq!(chain.process(&[0.1, 1.0]), [0.2, 0.5]);
    }
}

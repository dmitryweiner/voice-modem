//! Sample rate conversion between the audio device and the core.
//!
//! A windowed-sinc interpolator for any ratio, fed in pieces. The pass band
//! ends at 0.9 of the lower of the two Nyquist frequencies; the modem itself
//! stays below 3.4 kHz.

use std::f64::consts::PI;

/// Zero crossings of the sinc on each side of its centre.
const ZEROS: f64 = 12.0;
/// The cutoff as a share of the lower Nyquist frequency.
const CUTOFF: f64 = 0.9;
/// Points of the kernel table per input sample; values between are interpolated.
const TABLE_RESOLUTION: f64 = 128.0;

pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// Half of the kernel width, input samples.
    half_width: f64,
    /// The kernel at distances 0, 1/128, 2/128… input samples from its centre:
    /// computing a sine per tap made the converter the slowest part of the receiver.
    kernel: Vec<f32>,
    /// Input not yet done with; `buffer[0]` is input sample number `base`,
    /// counted from the silence put in front of the stream.
    buffer: Vec<f32>,
    base: u64,
    /// Number of output samples produced.
    produced: u64,
}

impl Resampler {
    pub fn new(from_hz: u32, to_hz: u32) -> Self {
        let cutoff = CUTOFF * (f64::from(to_hz) / f64::from(from_hz)).min(1.0);
        let half_width = ZEROS / cutoff;
        let kernel = (0..=(half_width * TABLE_RESOLUTION).ceil() as usize + 1)
            .map(|i| {
                let t = i as f64 / TABLE_RESOLUTION;
                if t >= half_width {
                    return 0.0;
                }
                let x = PI * cutoff * t;
                let sinc = if x < 1e-9 { 1.0 } else { x.sin() / x };
                // Blackman window
                let w = PI * t / half_width;
                (sinc * (0.42 + 0.5 * w.cos() + 0.08 * (2.0 * w).cos())) as f32
            })
            .collect();
        // Silence in front, so that the first output sample is the first input sample.
        Self {
            step: f64::from(from_hz) / f64::from(to_hz),
            half_width,
            kernel,
            buffer: vec![0.0; half_width.ceil() as usize],
            base: 0,
            produced: 0,
        }
    }

    /// The delay of the output against the input, in input samples: the
    /// samples the kernel has to see ahead.
    pub fn latency(&self) -> usize {
        self.half_width.ceil() as usize
    }

    /// Takes the next input samples and appends the output that can be computed to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.buffer.extend_from_slice(input);
        let lead = self.half_width.ceil();
        loop {
            // Position of the next output sample in the padded input.
            let centre = self.produced as f64 * self.step + lead;
            let first = (centre - self.half_width).ceil().max(0.0) as u64;
            let last = (centre + self.half_width).floor() as u64;
            if last >= self.base + self.buffer.len() as u64 {
                break;
            }
            let (mut sum, mut weight) = (0.0f32, 0.0f32);
            for k in first..=last {
                let at = (k as f64 - centre).abs() * TABLE_RESOLUTION;
                let index = at as usize;
                let between = (at - index as f64) as f32;
                let h =
                    self.kernel[index] + (self.kernel[index + 1] - self.kernel[index]) * between;
                sum += h * self.buffer[(k - self.base) as usize];
                weight += h;
            }
            out.push(sum / weight);
            self.produced += 1;
        }
        // Forget what no later output sample reaches back to.
        let centre = self.produced as f64 * self.step + lead;
        let keep_from = (centre - self.half_width).ceil().max(0.0) as u64;
        let drop = (keep_from.saturating_sub(self.base) as usize).min(self.buffer.len());
        self.buffer.drain(..drop);
        self.base += drop as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, rate: u32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| (2.0 * PI * hz * n as f64 / f64::from(rate)).sin() as f32)
            .collect()
    }

    fn convert(from: u32, to: u32, input: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        Resampler::new(from, to).process(input, &mut out);
        out
    }

    /// The largest difference between `signal` and a tone of amplitude 1, away from the ends.
    fn error(signal: &[f32], hz: f64, rate: u32) -> f32 {
        let expected = tone(hz, rate, signal.len());
        let middle = 200..signal.len() - 200;
        signal[middle.clone()]
            .iter()
            .zip(&expected[middle])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_tone_keeps_its_frequency_level_and_phase() {
        for (from, to) in [
            (48_000, 16_000),
            (44_100, 16_000),
            (16_000, 48_000),
            (16_000, 44_100),
            (16_000, 16_000),
        ] {
            for hz in [300.0, 1000.0, 3400.0] {
                let out = convert(from, to, &tone(hz, from, from as usize));
                let e = error(&out, hz, to);
                assert!(e < 0.003, "{from} → {to}, {hz} Hz: error {e}");
            }
        }
        // A device at 8 kHz carries the band up to 3.6 kHz only.
        let e = error(
            &convert(8000, 16_000, &tone(2625.0, 8000, 8000)),
            2625.0,
            16_000,
        );
        assert!(e < 0.003, "8000 → 16000: error {e}");
    }

    #[test]
    fn output_length_follows_the_ratio() {
        // all but the look-ahead of the kernel comes out at once
        let out = convert(48_000, 16_000, &vec![0.0; 48_000]);
        assert!((15_980..=16_000).contains(&out.len()), "{}", out.len());
        let out = convert(16_000, 44_100, &vec![0.0; 16_000]);
        assert!((44_050..=44_100).contains(&out.len()), "{}", out.len());
    }

    #[test]
    fn what_does_not_fit_the_new_rate_is_removed() {
        // 10 kHz would fold to 6 kHz at 16 kHz
        let out = convert(48_000, 16_000, &tone(10_000.0, 48_000, 48_000));
        let peak = out[200..out.len() - 200]
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak < 0.003, "{peak}");
    }

    #[test]
    fn pieces_of_any_size_give_the_same_output() {
        let input = tone(1234.0, 44_100, 30_000);
        let whole = convert(44_100, 16_000, &input);
        for size in [1usize, 128, 441, 5000] {
            let mut resampler = Resampler::new(44_100, 16_000);
            let mut out = Vec::new();
            for piece in input.chunks(size) {
                resampler.process(piece, &mut out);
            }
            assert_eq!(out, whole, "pieces of {size}");
        }
    }

    #[test]
    fn does_not_keep_old_input() {
        let mut resampler = Resampler::new(48_000, 16_000);
        let mut out = Vec::new();
        for _ in 0..100 {
            resampler.process(&[0.0; 4800], &mut out);
        }
        assert!(resampler.buffer.len() < 200, "{}", resampler.buffer.len());
    }
}

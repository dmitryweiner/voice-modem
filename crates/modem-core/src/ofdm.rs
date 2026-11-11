//! Coherent OFDM for the fast modes: 64 carriers on the grid of the tones,
//! each carrying two or four bits in its phase and amplitude.
//!
//! Where the tones of the robust modes are compared by energy, here the
//! receiver has to know what the channel did to every carrier. Two training
//! symbols in front of the data tell it; after them every eighth carrier of a
//! data symbol is a pilot with a known value, a different eighth each time.
//! The pilots show how the phase has moved since the training — the clocks
//! of two sound cards never agree, and the preamble gives the frequency
//! offset only to a fraction of a hertz — and keep the picture of the channel
//! fresh.
//!
//! Each symbol starts with a copy of its own end (the cyclic prefix), so an
//! echo shorter than that only changes the picture of the channel. The long
//! echo of a room is noise for these modes: every carrier sounds all the
//! time and there is nowhere to hide from it.

use std::f32::consts::TAU;

use crate::SAMPLE_RATE;
use crate::coding::scramble_bits;
use crate::ldpc::{self, Rate};

/// Carriers of a symbol, data and pilots.
pub const CARRIERS: usize = 64;
/// The lowest carrier, Hz, and the distance between neighbours: the grid of `mfsk`.
const FIRST_HZ: f32 = 750.0;
const SPACING_HZ: f32 = 31.25;
/// The part of a symbol the receiver looks at, samples: one period of the grid.
const USEFUL_LEN: usize = 512;
/// The cyclic prefix, samples (8 ms).
const PREFIX_LEN: usize = 128;
/// A symbol on the air, samples (40 ms).
pub const SYMBOL_LEN: usize = PREFIX_LEN + USEFUL_LEN;
/// The receiver's window starts this many samples before the end of the
/// prefix: the clocks may drift that far either way during a frame.
const BACKOFF: usize = PREFIX_LEN / 2;
/// Symbols with known values on every carrier in front of the data.
pub const TRAINING_SYMBOLS: usize = 2;
/// Every `PILOT_STEP`-th carrier of a data symbol is a pilot.
const PILOT_STEP: usize = 8;
/// From one data symbol to the next the pilots move by this many carriers:
/// in eight symbols every carrier has been one.
const PILOT_HOP: usize = 3;
/// Carriers of a data symbol that carry data.
pub const DATA_CARRIERS: usize = CARRIERS - CARRIERS / PILOT_STEP;
/// The peak of the signal over its root mean square: what is louder is cut
/// off. The sum of 64 carriers exceeds this once in a few hundred samples.
const PEAK_TO_RMS: f32 = 2.8;
/// How much of a new pilot goes into the picture of the channel.
const CHANNEL_UPDATE: f32 = 0.25;
/// Gains of the loops that follow the common phase and the slope of the
/// phase over the carriers (the drift of the timing): position and rate.
const PHASE_GAIN: (f32, f32) = (0.5, 0.1);
const SLOPE_GAIN: (f32, f32) = (0.2, 0.02);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OfdmParams {
    /// Bits in one carrier: 2 (four phases) or 4 (sixteen points).
    pub bits_per_carrier: usize,
    /// The code over the block.
    pub rate: Rate,
}

impl OfdmParams {
    pub const fn bits_per_symbol(&self) -> usize {
        DATA_CARRIERS * self.bits_per_carrier
    }

    /// Symbols that carry a block of `bytes` bytes, training included.
    pub fn block_symbols(&self, bytes: usize) -> usize {
        TRAINING_SYMBOLS + ldpc::block_bits(self.rate, bytes).div_ceil(self.bits_per_symbol())
    }

    /// Bits of the block per second, while its data symbols last.
    pub fn bit_rate(&self) -> f32 {
        let coded = self.bits_per_symbol() as f32 * SAMPLE_RATE as f32 / SYMBOL_LEN as f32;
        coded * self.rate.info_bits() as f32 / ldpc::CODE_BITS as f32
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Complex {
    re: f32,
    im: f32,
}

impl Complex {
    const fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }

    fn from_angle(angle: f32) -> Self {
        let (im, re) = angle.sin_cos();
        Self { re, im }
    }

    fn conj(self) -> Self {
        Self::new(self.re, -self.im)
    }

    fn norm(self) -> f32 {
        self.re * self.re + self.im * self.im
    }

    fn angle(self) -> f32 {
        self.im.atan2(self.re)
    }

    fn scaled(self, by: f32) -> Self {
        Self::new(self.re * by, self.im * by)
    }
}

impl std::ops::Mul for Complex {
    type Output = Complex;
    fn mul(self, other: Complex) -> Complex {
        Complex::new(
            self.re * other.re - self.im * other.im,
            self.re * other.im + self.im * other.re,
        )
    }
}

impl std::ops::Add for Complex {
    type Output = Complex;
    fn add(self, other: Complex) -> Complex {
        Complex::new(self.re + other.re, self.im + other.im)
    }
}

impl std::ops::Sub for Complex {
    type Output = Complex;
    fn sub(self, other: Complex) -> Complex {
        Complex::new(self.re - other.re, self.im - other.im)
    }
}

fn carrier_hz(carrier: usize) -> f32 {
    FIRST_HZ + SPACING_HZ * carrier as f32
}

/// The known value of a carrier in a training symbol and when it is a pilot.
/// The phases grow as the square of the carrier number: the sum of such
/// carriers is a sweep with hardly any peaks.
fn known(carrier: usize) -> Complex {
    let turns = (carrier * carrier) as f32 / (2 * CARRIERS) as f32;
    Complex::from_angle(TAU * turns.fract())
}

/// Whether a carrier of data symbol number `symbol` is a pilot.
const fn is_pilot(symbol: usize, carrier: usize) -> bool {
    (carrier + PILOT_STEP - symbol * PILOT_HOP % PILOT_STEP).is_multiple_of(PILOT_STEP)
}

/// Levels of one axis of the constellation for `bits` bits per axis, and what
/// to multiply them by for a mean power of 1 per carrier. Level number `g`
/// stands at `2g − (levels − 1)` and carries the bits `g XOR (g >> 1)`:
/// neighbours differ in one bit.
fn axis(bits: usize) -> (usize, f32) {
    let levels = 1usize << bits;
    (levels, (3.0 / (2 * (levels * levels - 1)) as f32).sqrt())
}

fn level_of(value: usize) -> usize {
    let mut level = value;
    let mut shifted = value >> 1;
    while shifted != 0 {
        level ^= shifted;
        shifted >>= 1;
    }
    level
}

/// The point of the constellation for the bits of one carrier, first half on the real axis.
fn point(bits: &[u8]) -> Complex {
    let half = bits.len() / 2;
    let (levels, scale) = axis(half);
    let coordinate = |bits: &[u8]| {
        let value = bits.iter().fold(0, |v, &bit| (v << 1) | usize::from(bit));
        (2 * level_of(value)) as f32 - (levels - 1) as f32
    };
    Complex::new(coordinate(&bits[..half]), coordinate(&bits[half..])).scaled(scale)
}

/// Appends the soft bits of one axis: for every bit, how much nearer the
/// received coordinate is to a level with the bit clear than to one with it set.
fn soft_axis(coordinate: f32, bits: usize, weight: f32, out: &mut Vec<f32>) {
    let (levels, scale) = axis(bits);
    for bit in (0..bits).rev() {
        let (mut zero, mut one) = (f32::INFINITY, f32::INFINITY);
        for level in 0..levels {
            let at = ((2 * level) as f32 - (levels - 1) as f32) * scale;
            let distance = (coordinate - at) * (coordinate - at);
            if ((level ^ (level >> 1)) >> bit) & 1 == 0 {
                zero = zero.min(distance);
            } else {
                one = one.min(distance);
            }
        }
        out.push(weight * (one - zero));
    }
}

/// Appends one symbol with the given values of its carriers to `out`.
fn symbol(values: &[Complex; CARRIERS], amplitude: f32, out: &mut Vec<f32>) {
    // 64 carriers of the level `a` have the root mean square a·√32.
    let level = amplitude / PEAK_TO_RMS / (CARRIERS as f32 / 2.0).sqrt();
    let start = out.len();
    out.resize(start + SYMBOL_LEN, 0.0);
    let useful = start + PREFIX_LEN;
    for (carrier, value) in values.iter().enumerate() {
        let step = Complex::from_angle(TAU * carrier_hz(carrier) / SAMPLE_RATE as f32);
        let mut phasor = *value;
        for sample in &mut out[useful..] {
            *sample += level * phasor.re;
            phasor = phasor * step;
        }
    }
    // The prefix is the end of the symbol: a period of every carrier.
    out.copy_within(useful + USEFUL_LEN - PREFIX_LEN.., start);
    for sample in &mut out[start..] {
        *sample = sample.clamp(-amplitude, amplitude);
    }
}

/// Appends the samples of a block of bytes: the training symbols, then the
/// data symbols with the coded bits.
pub fn modulate(params: &OfdmParams, data: &[u8], amplitude: f32, out: &mut Vec<f32>) {
    let training: [Complex; CARRIERS] = std::array::from_fn(known);
    for _ in 0..TRAINING_SYMBOLS {
        symbol(&training, amplitude, out);
    }
    let mut bits = ldpc::encode_block(params.rate, data);
    // Up to a whole symbol with more of the scrambler sequence.
    let padded = bits.len().div_ceil(params.bits_per_symbol()) * params.bits_per_symbol();
    let padding: Vec<u8> = scramble_bits(padded).skip(bits.len()).collect();
    bits.extend(padding);
    for (index, chunk) in bits.chunks_exact(params.bits_per_symbol()).enumerate() {
        let mut data_bits = chunk.chunks_exact(params.bits_per_carrier);
        let values = std::array::from_fn(|carrier| {
            if is_pilot(index, carrier) {
                known(carrier)
            } else {
                point(data_bits.next().expect("56 data carriers in a symbol"))
            }
        });
        symbol(&values, amplitude, out);
    }
}

/// What the receiver made of a block.
#[derive(Clone, Debug, PartialEq)]
pub struct Demodulated {
    /// The bytes of the block, right or not: the caller has a checksum.
    pub bytes: Vec<u8>,
    /// Signal-to-noise ratio on the carriers, dB: the power of a carrier
    /// over what the pilots were off by. Echo longer than the prefix and the
    /// distortion of the speaker count as noise here.
    pub snr_db: f32,
}

/// The carriers of the symbol whose samples start at `position` of the
/// block: `samples` is the whole block.
fn carriers(samples: &[f32], position: usize, freq_offset_hz: f32) -> [Complex; CARRIERS] {
    let start = position + PREFIX_LEN - BACKOFF;
    let window = &samples[start..start + USEFUL_LEN];
    std::array::from_fn(|carrier| {
        let hz = carrier_hz(carrier);
        // The phase of the carrier is counted from the end of the prefix; the
        // offset has turned the signal since the start of the block.
        let turns = -(f64::from(hz) * -(BACKOFF as f64) + f64::from(freq_offset_hz) * start as f64)
            / f64::from(SAMPLE_RATE);
        let mut phasor = Complex::from_angle((turns.fract() * std::f64::consts::TAU) as f32);
        let step = Complex::from_angle(-TAU * (hz + freq_offset_hz) / SAMPLE_RATE as f32);
        let mut sum = Complex::default();
        for &sample in window {
            sum = sum + phasor.scaled(sample);
            phasor = phasor * step;
        }
        sum
    })
}

/// A value that follows a measured error with a position and a rate.
#[derive(Clone, Copy, Default)]
struct Loop {
    value: f32,
    rate: f32,
}

impl Loop {
    fn predict(&mut self) {
        self.value += self.rate;
    }

    fn correct(&mut self, error: f32, (position, rate): (f32, f32)) {
        self.value += position * error;
        self.rate += rate * error;
    }
}

/// Distance of a carrier from the middle of the band, in carriers.
fn from_centre(carrier: usize) -> f32 {
    carrier as f32 - (CARRIERS - 1) as f32 / 2.0
}

/// How the loops say a carrier has turned since the training.
fn turn_of(phase: &Loop, slope: &Loop, carrier: usize) -> Complex {
    Complex::from_angle(phase.value + slope.value * from_centre(carrier))
}

/// Demodulates and decodes a block of `bytes` bytes from its samples:
/// `block_symbols(bytes) · SYMBOL_LEN` of them, the signal shifted up by
/// `freq_offset_hz`.
pub fn demodulate(
    params: &OfdmParams,
    samples: &[f32],
    bytes: usize,
    freq_offset_hz: f32,
) -> Demodulated {
    let symbols = params.block_symbols(bytes);
    assert!(samples.len() >= symbols * SYMBOL_LEN, "not enough samples");
    let at = |index: usize| carriers(samples, index * SYMBOL_LEN, freq_offset_hz);

    // The channel as the first training symbol shows it, and how far the
    // phase went by the second: the rest of the frequency offset.
    let (first, second) = (at(0), at(1));
    let seen = |values: &[Complex; CARRIERS]| -> [Complex; CARRIERS] {
        std::array::from_fn(|carrier| values[carrier] * known(carrier).conj())
    };
    let (first, second) = (seen(&first), seen(&second));
    let turn = first
        .iter()
        .zip(&second)
        .fold(Complex::default(), |sum, (a, b)| sum + *b * a.conj())
        .angle();
    let back = Complex::from_angle(-turn);
    let mut channel: [Complex; CARRIERS] =
        std::array::from_fn(|carrier| (first[carrier] + second[carrier] * back).scaled(0.5));
    let mut phase = Loop {
        value: turn,
        rate: turn,
    };
    let mut slope = Loop::default();

    let half = params.bits_per_carrier / 2;
    let mut soft = Vec::with_capacity((symbols - TRAINING_SYMBOLS) * params.bits_per_symbol());
    let (mut signal, mut noise) = (0.0f32, 0.0f32);
    for index in 0..symbols - TRAINING_SYMBOLS {
        let received = at(TRAINING_SYMBOLS + index);
        phase.predict();
        slope.predict();
        let pilots = || (0..CARRIERS).filter(|&carrier| is_pilot(index, carrier));

        // How far the pilots are from where the loops expect them: all by the
        // same angle, and by one that grows across the band.
        let errors: Vec<(usize, Complex)> = pilots()
            .map(|carrier| {
                let pilot = received[carrier] * known(carrier).conj();
                (
                    carrier,
                    pilot * (channel[carrier] * turn_of(&phase, &slope, carrier)).conj(),
                )
            })
            .collect();
        let common = errors
            .iter()
            .fold(Complex::default(), |sum, (_, error)| sum + *error);
        let back = Complex::from_angle(-common.angle());
        let (mut tilt, mut weight) = (0.0f32, 0.0f32);
        let middle = errors
            .iter()
            .map(|&(carrier, error)| from_centre(carrier) * error.norm().sqrt())
            .sum::<f32>()
            / errors
                .iter()
                .map(|(_, error)| error.norm().sqrt())
                .sum::<f32>()
                .max(f32::MIN_POSITIVE);
        for &(carrier, error) in &errors {
            let strength = error.norm().sqrt();
            let distance = from_centre(carrier) - middle;
            tilt += strength * distance * (error * back).angle();
            weight += strength * distance * distance;
        }
        phase.correct(common.angle(), PHASE_GAIN);
        slope.correct(tilt / weight.max(f32::MIN_POSITIVE), SLOPE_GAIN);

        for carrier in 0..CARRIERS {
            let turn = turn_of(&phase, &slope, carrier);
            let reference = channel[carrier] * turn;
            let power = reference.norm();
            if is_pilot(index, carrier) {
                let pilot = received[carrier] * known(carrier).conj();
                signal += power;
                noise += (pilot - reference).norm();
                // The pilot as the channel alone would give it, into the picture.
                channel[carrier] = channel[carrier].scaled(1.0 - CHANNEL_UPDATE)
                    + (pilot * turn.conj()).scaled(CHANNEL_UPDATE);
            } else {
                let equalised =
                    (received[carrier] * reference.conj()).scaled(1.0 / power.max(1e-30));
                soft_axis(equalised.re, half, power, &mut soft);
                soft_axis(equalised.im, half, power, &mut soft);
            }
        }
    }
    let ratio = signal / noise.max(f32::MIN_POSITIVE);
    Demodulated {
        bytes: ldpc::decode_block(params.rate, &soft, bytes),
        snr_db: (10.0 * ratio.max(1e-4).log10()).min(60.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QPSK: OfdmParams = OfdmParams {
        bits_per_carrier: 2,
        rate: Rate::Half,
    };
    const QAM16: OfdmParams = OfdmParams {
        bits_per_carrier: 4,
        rate: Rate::Half,
    };
    const QAM16_FAST: OfdmParams = OfdmParams {
        bits_per_carrier: 4,
        rate: Rate::ThreeQuarters,
    };
    const ALL: [OfdmParams; 3] = [QPSK, QAM16, QAM16_FAST];

    fn data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 37 + 11) as u8).collect()
    }

    fn modulated(params: &OfdmParams, data: &[u8]) -> Vec<f32> {
        let mut samples = Vec::new();
        modulate(params, data, 0.5, &mut samples);
        samples
    }

    #[test]
    fn speeds() {
        assert_eq!(ALL.map(|p| p.bit_rate()), [1400.0, 2800.0, 4200.0]);
        assert_eq!(SYMBOL_LEN, 640);
        assert_eq!(DATA_CARRIERS, 56);
    }

    #[test]
    fn block_lengths() {
        // 4 bytes: 32 + 324 coded bits in 4 symbols of 112, after the training
        assert_eq!(QPSK.block_symbols(4), 2 + 4);
        assert_eq!(QAM16.block_symbols(4), 2 + 2);
        // the longest block: 16 640 coded bits
        assert_eq!(QPSK.block_symbols(1027), 2 + 149);
        let samples = modulated(&QPSK, &data(4));
        assert_eq!(samples.len(), 6 * SYMBOL_LEN);
    }

    #[test]
    fn every_carrier_is_a_pilot_once_in_eight_symbols() {
        let mut times = [0; CARRIERS];
        for symbol in 0..8 {
            let pilots: Vec<usize> = (0..CARRIERS).filter(|&c| is_pilot(symbol, c)).collect();
            assert_eq!(pilots.len(), 8, "symbol {symbol}");
            assert!(pilots.windows(2).all(|pair| pair[1] - pair[0] == 8));
            pilots.iter().for_each(|&c| times[c] += 1);
        }
        assert_eq!(times, [1; CARRIERS]);
        assert!(is_pilot(0, 0) && is_pilot(1, 3) && is_pilot(8, 0));
    }

    #[test]
    fn constellations_have_unit_power_and_neighbours_differ_in_one_bit() {
        for bits in [2usize, 4] {
            let points: Vec<(usize, Complex)> = (0..1 << bits)
                .map(|value| {
                    let word: Vec<u8> = (0..bits)
                        .rev()
                        .map(|bit| (value >> bit) as u8 & 1)
                        .collect();
                    (value, point(&word))
                })
                .collect();
            let power = points.iter().map(|(_, p)| p.norm()).sum::<f32>() / points.len() as f32;
            assert!((power - 1.0).abs() < 1e-5, "{bits} bits: {power}");
            let nearest = 2.0 * axis(bits / 2).1;
            for (a, first) in &points {
                for (b, second) in &points {
                    if ((*first - *second).norm().sqrt() - nearest).abs() < 1e-4 {
                        assert_eq!((a ^ b).count_ones(), 1, "{a:b} and {b:b}");
                    }
                }
            }
        }
    }

    #[test]
    fn soft_bits_point_at_the_sent_bits() {
        for bits in [1usize, 2] {
            for value in 0..1usize << (2 * bits) {
                let word: Vec<u8> = (0..2 * bits)
                    .rev()
                    .map(|bit| (value >> bit) as u8 & 1)
                    .collect();
                let sent = point(&word);
                let mut soft = Vec::new();
                soft_axis(sent.re, bits, 1.0, &mut soft);
                soft_axis(sent.im, bits, 1.0, &mut soft);
                let hard: Vec<u8> = soft.iter().map(|&llr| u8::from(llr < 0.0)).collect();
                assert_eq!(hard, word);
            }
        }
    }

    #[test]
    fn stays_within_the_amplitude_and_is_seldom_cut() {
        for params in ALL {
            let samples = modulated(&params, &data(300));
            let peak = samples.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak <= 0.5, "{peak}");
            let cut = samples.iter().filter(|x| x.abs() >= 0.5).count();
            assert!(cut * 100 < samples.len(), "{cut} of {}", samples.len());
            let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt();
            assert!((rms * PEAK_TO_RMS / 0.5 - 1.0).abs() < 0.05, "{rms}");
        }
        // the training symbols are a sweep: nowhere near the limit
        let training = &modulated(&QPSK, &[])[..TRAINING_SYMBOLS * SYMBOL_LEN];
        let peak = training.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak < 0.35, "{peak}");
    }

    #[test]
    fn the_prefix_is_the_end_of_the_symbol() {
        let samples = modulated(&QAM16, &data(50));
        for symbol in samples.as_chunks::<SYMBOL_LEN>().0 {
            for index in 0..PREFIX_LEN {
                let (prefix, end) = (symbol[index], symbol[USEFUL_LEN + index]);
                assert!((prefix - end).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn every_block_comes_back() {
        for params in ALL {
            for len in [0usize, 1, 40, 41, 259, 1027] {
                let sent = data(len);
                let samples = modulated(&params, &sent);
                assert_eq!(samples.len(), params.block_symbols(len) * SYMBOL_LEN);
                let received = demodulate(&params, &samples, len, 0.0);
                assert_eq!(received.bytes, sent, "{params:?}, {len} bytes");
                assert!(received.snr_db > 25.0, "{params:?}: {}", received.snr_db);
            }
        }
    }

    #[test]
    fn the_level_and_the_sign_do_not_matter() {
        let sent = data(100);
        for gain in [-1.0, 0.001, 1.9] {
            let samples: Vec<f32> = modulated(&QAM16, &sent).iter().map(|x| x * gain).collect();
            assert_eq!(demodulate(&QAM16, &samples, 100, 0.0).bytes, sent, "{gain}");
        }
    }

    #[test]
    fn an_echo_shorter_than_the_prefix_is_not_noise() {
        let sent = data(200);
        let clean = modulated(&QAM16_FAST, &sent);
        let mut samples = clean.clone();
        // 2 ms and 3.5 ms late, nearly as loud as the direct sound
        for (delay, gain) in [(32, 0.6), (56, -0.5)] {
            for (index, x) in clean.iter().enumerate() {
                if let Some(sample) = samples.get_mut(index + delay) {
                    *sample += gain * x;
                }
            }
        }
        let received = demodulate(&QAM16_FAST, &samples, 200, 0.0);
        assert_eq!(received.bytes, sent);
        assert!(received.snr_db > 25.0, "{}", received.snr_db);
    }

    #[test]
    fn a_window_a_few_samples_off_is_as_good() {
        let sent = data(120);
        let clean = modulated(&QAM16, &sent);
        for late in [-40i32, -7, 9, 40] {
            // the block as a receiver sees it whose idea of the start is `late` samples late
            let samples: Vec<f32> = (0..clean.len() as i32)
                .map(|index| {
                    usize::try_from(index + late)
                        .ok()
                        .and_then(|at| clean.get(at))
                        .copied()
                        .unwrap_or(0.0)
                })
                .collect();
            assert_eq!(demodulate(&QAM16, &samples, 120, 0.0).bytes, sent, "{late}");
        }
    }

    #[test]
    fn reports_the_noise_on_the_carriers() {
        let sent = data(400);
        let clean = modulated(&QPSK, &sent);
        // White noise 10 dB below the signal over the whole band: the signal
        // sits in 2 kHz of the 8, so on a carrier it is 16 dB above the noise.
        let rms = (clean.iter().map(|x| x * x).sum::<f32>() / clean.len() as f32).sqrt();
        let mut seed = 1u32;
        let mut uniform = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let sigma = rms / 10f32.powf(10.0 / 20.0);
        let noisy: Vec<f32> = clean
            .iter()
            .map(|x| {
                let gaussian = (-2.0 * (uniform() + 1e-9).ln()).sqrt() * (TAU * uniform()).cos();
                x + sigma * gaussian
            })
            .collect();
        let received = demodulate(&QPSK, &noisy, 400, 0.0);
        assert_eq!(received.bytes, sent);
        assert!((received.snr_db - 16.0).abs() < 1.5, "{}", received.snr_db);
    }
}

//! Transmitter: a payload → the samples of one frame.

use std::f32::consts::TAU;

use crate::SAMPLE_RATE;
use crate::coding::encode_block;
use crate::crc::crc32;
use crate::frame::{Frame, HEADER_MODE, Header, Kind, MAX_PAYLOAD, Mode};
use crate::mfsk::Modulator;
use crate::sync::preamble;

/// Frequency of the lead-in tone, Hz.
const LEAD_IN_HZ: f32 = 1000.0;
/// The frame fades in and out over this many samples to avoid clicks.
const RAMP: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TxConfig {
    /// Peak level of the signal, 0..=1.
    pub amplitude: f32,
    /// A tone before the preamble that opens the VOX or squelch of a radio
    /// and wakes up the voice detector of a vocoder; 0 for none.
    pub lead_in_ms: u32,
}

impl Default for TxConfig {
    fn default() -> Self {
        Self {
            amplitude: 0.5,
            lead_in_ms: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayloadTooLong;

/// Length of a frame in samples, without the lead-in tone.
pub fn frame_len(kind: Kind, mode: Mode, payload_len: usize) -> usize {
    let payload = if kind.has_payload() {
        crate::rx::payload_len(mode, payload_len)
    } else {
        0
    };
    crate::sync::PREAMBLE_LEN + crate::rx::header_len() + payload
}

/// The samples of one frame: lead-in tone, preamble, header and, unless the
/// frame is an acknowledgement, the payload with its CRC-32.
pub fn encode_frame(
    frame: &Frame,
    mode: Mode,
    config: &TxConfig,
) -> Result<Vec<f32>, PayloadTooLong> {
    let Frame {
        kind,
        station,
        seq,
        ref payload,
    } = *frame;
    let len = u8::try_from(payload.len()).map_err(|_| PayloadTooLong)?;
    debug_assert!(payload.len() <= MAX_PAYLOAD);
    debug_assert!(
        kind.has_payload() || payload.is_empty(),
        "an acknowledgement carries no payload"
    );
    let amplitude = config.amplitude;
    let lead_in = (u64::from(config.lead_in_ms) * u64::from(SAMPLE_RATE) / 1000) as usize;
    let mut samples = Vec::with_capacity(lead_in + frame_len(kind, mode, payload.len()));

    let step = TAU * LEAD_IN_HZ / SAMPLE_RATE as f32;
    samples.extend((0..lead_in).map(|n| amplitude * (step * n as f32 % TAU).sin()));
    fade(&mut samples);

    preamble(amplitude, &mut samples);

    let data_start = samples.len();
    let header = Header {
        kind,
        mode,
        station,
        seq,
        len,
    };
    let mut modulator = Modulator::new(HEADER_MODE.mfsk(), amplitude);
    for value in encode_block(&header.to_bytes(), HEADER_MODE.mfsk().bits_per_symbol()) {
        modulator.symbol(value, &mut samples);
    }
    if kind.has_payload() {
        let mut block = payload.clone();
        block.extend(crc32(payload).to_be_bytes());
        let mut modulator = Modulator::new(mode.mfsk(), amplitude);
        for value in encode_block(&block, mode.mfsk().bits_per_symbol()) {
            modulator.symbol(value, &mut samples);
        }
    }
    fade(&mut samples[data_start..]);
    Ok(samples)
}

/// Raised-cosine fade-in and fade-out.
fn fade(samples: &mut [f32]) {
    let len = samples.len();
    for i in 0..RAMP.min(len / 2) {
        let gain = 0.5 - 0.5 * (TAU / 2.0 * i as f32 / RAMP as f32).cos();
        samples[i] *= gain;
        samples[len - 1 - i] *= gain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(payload: &[u8]) -> Frame {
        Frame {
            kind: Kind::Data,
            station: 7,
            seq: 0,
            payload: payload.to_vec(),
        }
    }

    fn encode(payload: &[u8], config: &TxConfig) -> Vec<f32> {
        encode_frame(&data(payload), Mode::R1, config).unwrap()
    }

    #[test]
    fn frame_length() {
        let frame = encode(b"hello", &TxConfig::default());
        // preamble + 31 header symbols + ((5 + 4) · 8 + 6) · 2 / 4 = 39 payload symbols
        assert_eq!(frame.len(), 4096 + (31 + 39) * 512);
        assert_eq!(frame.len(), frame_len(Kind::Data, Mode::R1, 5));
    }

    #[test]
    fn an_acknowledgement_is_the_preamble_and_the_header() {
        let ack = Frame {
            kind: Kind::Ack { advice: Mode::R3 },
            station: 7,
            seq: 3,
            payload: vec![],
        };
        let frame = encode_frame(&ack, Mode::R1, &TxConfig::default()).unwrap();
        assert_eq!(frame.len(), 4096 + 31 * 512);
        assert_eq!(frame.len(), frame_len(ack.kind, Mode::R1, 0));
        assert!(frame[frame.len() - 1].abs() < 1e-3);
    }

    #[test]
    fn stays_within_the_amplitude() {
        let config = TxConfig {
            amplitude: 0.25,
            lead_in_ms: 100,
        };
        let frame = encode(&[0xFF; 40], &config);
        let peak = frame.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak <= 0.25 && peak > 0.24, "{peak}");
    }

    #[test]
    fn lead_in_adds_a_tone_in_front() {
        let plain = encode(b"x", &TxConfig::default());
        let config = TxConfig {
            lead_in_ms: 300,
            ..TxConfig::default()
        };
        let with_tone = encode(b"x", &config);
        assert_eq!(with_tone.len(), plain.len() + 4800);
        assert_eq!(with_tone[4800..], plain[..]);
    }

    #[test]
    fn starts_and_ends_quietly() {
        let config = TxConfig {
            lead_in_ms: 50,
            ..TxConfig::default()
        };
        let frame = encode(b"abc", &config);
        assert!(frame[0].abs() < 1e-3 && frame[frame.len() - 1].abs() < 1e-3);
    }

    #[test]
    fn accepts_empty_and_full_payloads_only() {
        let config = TxConfig::default();
        assert!(encode_frame(&data(&[]), Mode::R1, &config).is_ok());
        assert!(encode_frame(&data(&[0; MAX_PAYLOAD]), Mode::R1, &config).is_ok());
        assert_eq!(
            encode_frame(&data(&[0; MAX_PAYLOAD + 1]), Mode::R1, &config),
            Err(PayloadTooLong)
        );
    }
}

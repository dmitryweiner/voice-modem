//! Measures where the modem stops working: the share of frames received
//! against the noise level, the frequency offset and the clock offset, and
//! how fast the receiver is. The numbers behind `docs/measurements.md` and
//! the limits asserted in `tests/link.rs`.
//!
//!   cargo run --release -p modem-sim --example sweep

use std::time::Instant;

use modem_core::{Frame, Kind, Mode, Receiver, RxEvent, SAMPLE_RATE, TxConfig, encode_frame};
use modem_sim::{Awgn, Chain, Channel, ClockOffset, FreqShift, Pad, rms};

fn received(samples: &[f32], payload: &[u8]) -> bool {
    let mut receiver = Receiver::new();
    receiver.push(samples);
    matches!(receiver.poll(), Some(RxEvent::Frame { frame, .. }) if frame.payload == payload)
}

fn encode(payload: &[u8]) -> Vec<f32> {
    let frame = Frame {
        kind: Kind::Data,
        station: 1,
        seq: 1,
        payload: payload.to_vec(),
    };
    encode_frame(&frame, Mode::R1, &TxConfig::default()).unwrap()
}

fn main() {
    const TRIALS: u64 = 50;
    let payload: Vec<u8> = (0..40u8).map(|i| i.wrapping_mul(37)).collect();
    let frame = encode(&payload);
    let level = rms(&frame);

    println!("40-byte frames received, white noise (SNR in the 8 kHz band):");
    for snr in (-18..=-10).rev() {
        let good = (0..TRIALS)
            .filter(|&seed| {
                let pad = Pad {
                    before: 3000 + seed as usize * 13,
                    after: 2000,
                };
                received(
                    &Chain::new()
                        .then(pad)
                        .then(Awgn::with_snr(level, snr as f32, seed))
                        .process(&frame),
                    &payload,
                )
            })
            .count();
        println!("  {snr:>4} dB: {good}/{TRIALS}");
    }

    println!("frequency offset at −10 dB (received/{TRIALS}, mean estimate):");
    for shift in [-250.0f32, -200.0, -100.0, -30.0, 30.0, 100.0, 200.0, 250.0] {
        let mut estimates = Vec::new();
        for seed in 0..TRIALS {
            let mut channel = Chain::new()
                .then(Pad {
                    before: 3000,
                    after: 2000,
                })
                .then(FreqShift(shift))
                .then(Awgn::with_snr(level, -10.0, seed));
            let mut receiver = Receiver::new();
            receiver.push(&channel.process(&frame));
            if let Some(RxEvent::Frame { stats, .. }) = receiver.poll() {
                estimates.push(stats.freq_offset_hz);
            }
        }
        let mean = estimates.iter().sum::<f32>() / estimates.len().max(1) as f32;
        println!(
            "  {shift:>6} Hz: {}/{TRIALS}, {mean:.1} Hz",
            estimates.len()
        );
    }

    println!("clock offset, a full 255-byte frame (34 s), no noise:");
    let long: Vec<u8> = (0..255u8).collect();
    let long_frame = encode(&long);
    for ppm in [
        -1000.0f32, -700.0, -500.0, -300.0, -100.0, 100.0, 300.0, 500.0, 700.0, 1000.0,
    ] {
        let mut channel = Chain::new()
            .then(Pad {
                before: 3000,
                after: 2000,
            })
            .then(ClockOffset { ppm });
        println!(
            "  {ppm:>6} ppm: {}",
            if received(&channel.process(&long_frame), &long) {
                "received"
            } else {
                "lost"
            }
        );
    }

    let minutes = 10;
    let noise = Awgn::new(0.1, 99).process(&vec![0.0; SAMPLE_RATE as usize * 60 * minutes]);
    let mut receiver = Receiver::new();
    let started = Instant::now();
    receiver.push(&noise);
    let elapsed = started.elapsed().as_secs_f32();
    println!(
        "{minutes} min of noise: {} false frames, searched in {elapsed:.1} s ({:.0}× real time)",
        std::iter::from_fn(|| receiver.poll()).count(),
        60.0 * minutes as f32 / elapsed
    );
}

//! Compares the modes: for each, the white noise and the room echo it
//! withstands. The numbers behind the mode table in `docs/measurements.md`
//! and the thresholds of the mode choice in `modem-core/src/link.rs`.
//!
//!   cargo run --release -p modem-sim --example modes
//!
//! A column is the share of 40-byte frames received (of 20) at that level
//! and, in brackets, the mean of the SNR the receiver reported. For the fast
//! modes also the clock offsets the longest frame survives and the time the
//! receiver takes for it.

use std::time::Instant;

use modem_core::{Frame, Kind, Mode, Receiver, RxEvent, TxConfig, encode_frame};
use modem_sim::{Awgn, Chain, Channel, ClockOffset, Pad, Reverb, rms};

const TRIALS: u64 = 20;

/// Frames received, and the mean of the SNR the receiver reported for them:
/// on the carriers for a fast mode, on the tones of the header for the others.
fn received(frame: &[f32], payload: &[u8], channel: impl Fn(u64) -> Chain) -> (usize, f32) {
    let mut estimates = Vec::new();
    for seed in 0..TRIALS {
        let mut receiver = Receiver::new();
        receiver.push(&channel(seed).process(frame));
        if let Some(RxEvent::Frame { frame, stats, .. }) = receiver.poll()
            && frame.payload == payload
        {
            estimates.push(stats.carriers_snr_db.unwrap_or(stats.snr_db));
        }
    }
    (
        estimates.len(),
        estimates.iter().sum::<f32>() / estimates.len().max(1) as f32,
    )
}

fn main() {
    let payload: Vec<u8> = (0..40u8).map(|i| i.wrapping_mul(37)).collect();
    let data = Frame {
        kind: Kind::Data,
        station: 1,
        seq: 1,
        payload: payload.clone(),
    };
    let padded = || {
        Chain::new().then(Pad {
            before: 3000,
            after: 2000,
        })
    };

    for mode in Mode::ALL {
        let frame = encode_frame(&data, mode, &TxConfig::default()).unwrap();
        // The level of the header, a single tone: what "SNR" means for every mode.
        let level = rms(&frame[4096..4096 + 8000]);
        println!(
            "{mode:?}: {} bit/s, the frame takes {:.1} s",
            mode.bit_rate(),
            frame.len() as f32 / 16_000.0
        );

        print!("  white noise, SNR dB:        ");
        for snr in [21, 18, 15, 12, 9, 6, 3, 0, -3, -6, -9, -12, -15] {
            let (good, estimate) = received(&frame, &payload, |seed| {
                padded().then(Awgn::with_snr(level, snr as f32, seed))
            });
            print!(" {snr:>3}:{good:<2}({estimate:>3.0})");
        }
        println!();
        for rt60 in [0.3f32, 0.5] {
            print!("  room RT60 {rt60} s, direct dB:  ");
            for direct in [24, 21, 18, 15, 12, 9, 6, 3, 0, -3, -6, -9] {
                let room = |seed| {
                    padded()
                        .then(Reverb::new(rt60, direct as f32, seed + 100))
                        .then(Awgn::with_snr(level, 30.0, seed))
                };
                let (good, estimate) = received(&frame, &payload, room);
                print!(" {direct:>3}:{good:<2}({estimate:>3.0})");
            }
            println!();
        }
        if mode < Mode::F1 {
            continue;
        }

        let long: Vec<u8> = (0..1023).map(|i| (i * 37 % 256) as u8).collect();
        let long_frame = encode_frame(
            &Frame {
                payload: long.clone(),
                ..data.clone()
            },
            mode,
            &TxConfig::default(),
        )
        .unwrap();
        print!(
            "  1023 bytes ({:.1} s), clock offset ppm:",
            long_frame.len() as f32 / 16_000.0
        );
        for ppm in [-1000, -500, -300, -200, -100, 100, 200, 300, 500, 1000] {
            let (good, _) = received(&long_frame, &long, |seed| {
                padded()
                    .then(ClockOffset { ppm: ppm as f32 })
                    .then(Awgn::with_snr(level, 20.0, seed))
            });
            print!(" {ppm}:{good}");
        }
        println!();
        let mut receiver = Receiver::new();
        receiver.push(&long_frame[..long_frame.len() - 1]);
        let started = Instant::now();
        receiver.push(&long_frame[long_frame.len() - 1..]);
        println!(
            "  its carriers are demodulated and decoded in {:.0} ms",
            started.elapsed().as_secs_f32() * 1000.0
        );
    }
}

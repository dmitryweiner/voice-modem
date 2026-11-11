//! Would a longer pause between the header and the payload help in a room?
//! The last tones of the header ring on at full level into the payload,
//! whose tones or carriers are many and each much weaker. The protocol has
//! 64 ms of silence there (none before a payload in R1); this adds more and
//! counts the frames received. The measurement that put the pause into the
//! protocol, see `docs/measurements.md`.
//!
//!   cargo run --release -p modem-sim --example pause [extra samples, default 0 and 1024]
//!
//! A column is the direct sound over the reflections, dB, and the frames
//! received of 40, each with another frame number and another room.

use modem_core::rx::{header_len, payload_pause};
use modem_core::{Frame, Kind, Mode, Receiver, RxEvent, TxConfig, encode_frame};
use modem_sim::{Awgn, Chain, Channel, Pad, Reverb, rms};

const TRIALS: u64 = 40;
const BEFORE: usize = 3000;

fn main() {
    let extras: Vec<usize> = std::env::args()
        .skip(1)
        .map(|arg| arg.parse().expect("a number of samples"))
        .collect();
    let extras = if extras.is_empty() {
        vec![0, 1024]
    } else {
        extras
    };
    let payload: Vec<u8> = (0..40u8).map(|i| i.wrapping_mul(37)).collect();
    for extra in extras {
        println!("{extra} samples more:");
        for mode in [Mode::R2, Mode::R3, Mode::R4, Mode::F1, Mode::F2, Mode::F3] {
            for rt60 in [0.3f32, 0.5] {
                print!("  {mode:?}, RT60 {rt60} s:");
                for direct in [18, 15, 12, 9, 6, 3, 0, -3, -6] {
                    let good = (0..TRIALS)
                        .filter(|&seed| {
                            let data = Frame {
                                kind: Kind::Data,
                                station: 1,
                                seq: (seed % 64) as u8,
                                payload: payload.clone(),
                            };
                            let mut frame =
                                encode_frame(&data, mode, &TxConfig::default()).unwrap();
                            let level = rms(&frame[4096..4096 + 8000]);
                            // more silence where the payload starts…
                            let at = 4096 + header_len() + payload_pause(mode);
                            frame.splice(at..at, std::iter::repeat_n(0.0, extra));
                            let mut heard = Chain::new()
                                .then(Pad {
                                    before: BEFORE,
                                    after: 2000,
                                })
                                .then(Reverb::new(rt60, direct as f32, seed + 100))
                                .then(Awgn::with_snr(level, 30.0, seed))
                                .process(&frame);
                            // …which the receiver knows nothing of: it is cut out of the recording
                            heard.drain(BEFORE + at..BEFORE + at + extra);
                            let mut receiver = Receiver::new();
                            receiver.push(&heard);
                            std::iter::from_fn(|| receiver.poll()).any(|event| {
                                matches!(event, RxEvent::Frame { frame, .. } if frame.payload == payload)
                            })
                        })
                        .count();
                    print!(" {direct}:{good}");
                }
                println!();
            }
        }
    }
}

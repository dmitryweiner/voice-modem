//! Scratch measurement: frame success rate against SNR and other impairments.
use modem_core::{Mode, Receiver, RxEvent, TxConfig, encode_frame};
use modem_sim::{Awgn, Chain, Channel, ClockOffset, FreqShift, Pad, rms};

fn ok(samples: &[f32], payload: &[u8]) -> bool {
    let mut rx = Receiver::new();
    rx.push(samples);
    matches!(rx.poll(), Some(RxEvent::Frame { payload: p, .. }) if p == payload)
}

fn main() {
    let payload: Vec<u8> = (0..40u8).map(|i| i.wrapping_mul(37)).collect();
    let frame = encode_frame(1, Mode::R1, &payload, &TxConfig::default()).unwrap();
    let level = rms(&frame);
    println!("frame rms {level}");
    for snr in [
        -6.0f32, -8.0, -10.0, -11.0, -12.0, -13.0, -14.0, -15.0, -16.0,
    ] {
        let trials = 40;
        let good = (0..trials)
            .filter(|&seed| {
                let mut ch = Chain::new()
                    .then(Pad {
                        before: 3000 + seed as usize * 13,
                        after: 2000,
                    })
                    .then(Awgn::with_snr(level, snr, seed));
                ok(&ch.process(&frame), &payload)
            })
            .count();
        println!("snr {snr:>5} dB: {good}/{trials}");
    }
    for shift in [-250.0f32, -200.0, -100.0, -30.0, 30.0, 100.0, 200.0, 250.0] {
        let mut ch = Chain::new()
            .then(Pad {
                before: 3000,
                after: 2000,
            })
            .then(FreqShift(shift))
            .then(Awgn::with_snr(level, -6.0, 1));
        let samples = ch.process(&frame);
        let mut rx = Receiver::new();
        rx.push(&samples);
        println!(
            "shift {shift}: {:?}",
            rx.poll().map(|e| match e {
                RxEvent::Frame { stats, .. } => format!("ok {stats:?}"),
                RxEvent::Damaged { stats, .. } => format!("damaged {stats:?}"),
            })
        );
    }
    let long: Vec<u8> = (0..255u8).collect();
    let frame = encode_frame(1, Mode::R1, &long, &TxConfig::default()).unwrap();
    for ppm in [
        -1000.0f32, -500.0, -300.0, -200.0, -100.0, 100.0, 200.0, 300.0, 500.0, 1000.0,
    ] {
        let mut ch = Chain::new()
            .then(Pad {
                before: 3000,
                after: 2000,
            })
            .then(ClockOffset { ppm });
        println!("ppm {ppm}: {}", ok(&ch.process(&frame), &long));
    }
    // false alarms in noise
    let noise = Awgn::new(0.1, 99).process(&vec![0.0; 16_000 * 600]);
    let mut rx = Receiver::new();
    let t = std::time::Instant::now();
    rx.push(&noise);
    println!("10 min of noise: {:?} in {:?}", rx.poll(), t.elapsed());
}

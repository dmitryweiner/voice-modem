//! The modem through simulated channels. The limits here are regression
//! guards with a margin of about 3 dB to what `examples/sweep.rs` measures.

use modem_core::{
    Config, Event, Frame, Kind, Mode, Modem, Receiver, RxEvent, RxStats, TxConfig, encode_frame,
};
use modem_sim::{
    Awgn, BandPass, Chain, Channel, Clip, ClockOffset, Echo, FreqShift, Gain, Pad, Reverb, rms,
};

fn payload(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(37).wrapping_add(11))
        .collect()
}

fn frame(payload: &[u8]) -> Vec<f32> {
    let frame = Frame {
        kind: Kind::Data,
        station: 1,
        seq: 1,
        payload: payload.to_vec(),
    };
    encode_frame(&frame, Mode::R1, &TxConfig::default()).unwrap()
}

fn receive(samples: &[f32]) -> Vec<RxEvent> {
    let mut receiver = Receiver::new();
    receiver.push(samples);
    std::iter::from_fn(|| receiver.poll()).collect()
}

/// The stats of the only frame received, which must carry `payload`.
fn stats_of(events: &[RxEvent], payload: &[u8]) -> RxStats {
    match events {
        [RxEvent::Frame { frame, stats, .. }] if frame.payload == payload => *stats,
        other => panic!("expected one good frame, got {other:?}"),
    }
}

fn padded() -> Chain {
    Chain::new().then(Pad {
        before: 5000,
        after: 3000,
    })
}

#[test]
fn white_noise_12_db_above_the_signal() {
    let payload = payload(40);
    let frame = frame(&payload);
    let level = rms(&frame);
    for seed in 0..20 {
        let heard = padded()
            .then(Awgn::with_snr(level, -12.0, seed))
            .process(&frame);
        let stats = stats_of(&receive(&heard), &payload);
        assert!(
            (stats.snr_db + 12.0).abs() < 1.5,
            "seed {seed}: estimated {} dB",
            stats.snr_db
        );
    }
}

#[test]
fn never_invents_a_frame() {
    let payload = payload(40);
    let frame = frame(&payload);
    // Far too much noise: nothing may come out, least of all a wrong payload.
    for seed in 0..10 {
        let heard = padded()
            .then(Awgn::with_snr(rms(&frame), -30.0, seed))
            .process(&frame);
        assert_eq!(receive(&heard), [], "seed {seed}");
    }
    // Two minutes of nothing but noise.
    let noise = Awgn::new(0.2, 5).process(&vec![0.0; 16_000 * 120]);
    assert_eq!(receive(&noise), []);
}

#[test]
fn frequency_offset_of_a_mistuned_radio() {
    let payload = payload(40);
    let frame = frame(&payload);
    for (seed, shift) in [-200.0f32, -75.0, 40.0, 200.0].into_iter().enumerate() {
        let heard = padded()
            .then(FreqShift(shift))
            .then(Awgn::with_snr(rms(&frame), -9.0, seed as u64))
            .process(&frame);
        let stats = stats_of(&receive(&heard), &payload);
        assert!(
            (stats.freq_offset_hz - shift).abs() < 3.0,
            "shift {shift}: estimated {}",
            stats.freq_offset_hz
        );
    }
}

#[test]
fn sound_cards_with_different_clocks() {
    // The longest frame, 34 s: at 300 ppm the last symbol is 160 samples off.
    let payload = payload(255);
    let frame = frame(&payload);
    for ppm in [-300.0, 300.0] {
        let heard = padded().then(ClockOffset { ppm }).process(&frame);
        stats_of(&receive(&heard), &payload);
    }
}

#[test]
fn telephone_band_with_room_echo_and_noise() {
    let payload = payload(40);
    let frame = frame(&payload);
    // Echoes after 12, 30 and 55 ms: longer than a symbol they would be another matter.
    let echo = Echo(vec![(192, 0.6), (480, -0.4), (880, 0.25)]);
    let heard = padded()
        .then(echo)
        .then(BandPass::telephone())
        .then(Awgn::with_snr(rms(&frame), -6.0, 3))
        .process(&frame);
    stats_of(&receive(&heard), &payload);
}

#[test]
fn across_a_reverberant_room() {
    let payload = payload(40);
    let frame = frame(&payload);
    // The reflections carry four times the energy of the direct sound and
    // take half a second to die away: several metres from a small speaker.
    // With one set of tones the modem stops at equal energies (0 dB).
    for seed in 0..12 {
        let room = Reverb::new(0.5, -6.0, 100 + seed);
        let heard = padded()
            .then(room)
            .then(Awgn::with_snr(rms(&frame), 10.0, seed))
            .process(&frame);
        stats_of(&receive(&heard), &payload);
    }
}

#[test]
fn overdriven_and_very_quiet() {
    let payload = payload(40);
    let frame = frame(&payload);
    // Ten times too loud into a clipping input: the tones turn into square waves.
    stats_of(
        &receive(&padded().then(Gain(10.0)).then(Clip(0.5)).process(&frame)),
        &payload,
    );
    // 60 dB down, with noise still 6 dB above the signal.
    let quiet = padded()
        .then(Gain(0.001))
        .then(Awgn::with_snr(rms(&frame) * 0.001, -6.0, 8))
        .process(&frame);
    stats_of(&receive(&quiet), &payload);
}

/// A data frame in a mode, and the level of its header: what "SNR" is counted from.
fn frame_in(mode: Mode, payload: &[u8]) -> (Vec<f32>, f32) {
    let frame = Frame {
        kind: Kind::Data,
        station: 1,
        seq: 1,
        payload: payload.to_vec(),
    };
    let samples = encode_frame(&frame, mode, &TxConfig::default()).unwrap();
    let level = rms(&samples[4096..4096 + 8000]);
    (samples, level)
}

#[test]
fn fast_modes_in_white_noise() {
    let payload = payload(200);
    // 3 dB above where each stops (`examples/modes.rs`)
    for (mode, snr) in [(Mode::F1, 6.0), (Mode::F2, 12.0), (Mode::F3, 15.0)] {
        let (frame, level) = frame_in(mode, &payload);
        for seed in 0..12 {
            let heard = padded()
                .then(Awgn::with_snr(level, snr, seed))
                .process(&frame);
            let stats = stats_of(&receive(&heard), &payload);
            // the carriers have the level of the header tone over the same noise
            let carriers = stats.carriers_snr_db.unwrap();
            assert!((carriers - snr).abs() < 2.5, "{mode:?}: {carriers} dB");
        }
    }
}

#[test]
fn fast_modes_near_the_speaker_in_a_room() {
    let payload = payload(200);
    // The direct sound over all reflections: 9 dB is within a metre of a small speaker.
    for (mode, direct) in [(Mode::F1, 9.0), (Mode::F2, 18.0), (Mode::F3, 21.0)] {
        let (frame, level) = frame_in(mode, &payload);
        for seed in 0..12 {
            let heard = padded()
                .then(Reverb::new(0.5, direct, 100 + seed))
                .then(Awgn::with_snr(level, 30.0, seed))
                .process(&frame);
            stats_of(&receive(&heard), &payload);
        }
    }
}

#[test]
fn fast_modes_with_a_frequency_offset_and_drifting_clocks() {
    // The longest frame of each: 6 s, 3 s and 2 s.
    let payload = payload(1023);
    for mode in [Mode::F1, Mode::F2, Mode::F3] {
        let (frame, level) = frame_in(mode, &payload);
        for (seed, shift) in [-200.0f32, -31.0, 77.7].into_iter().enumerate() {
            let heard = padded()
                .then(FreqShift(shift))
                .then(Awgn::with_snr(level, 20.0, seed as u64))
                .process(&frame);
            stats_of(&receive(&heard), &payload);
        }
        // at 200 ppm the end of a 6 s frame is 20 samples off
        for ppm in [-200.0, 200.0] {
            let heard = padded()
                .then(ClockOffset { ppm })
                .then(Awgn::with_snr(level, 20.0, 5))
                .process(&frame);
            stats_of(&receive(&heard), &payload);
        }
    }
}

#[test]
fn fast_modes_over_a_telephone_line_and_through_a_clipping_input() {
    let payload = payload(300);
    for mode in [Mode::F1, Mode::F2, Mode::F3] {
        let (frame, level) = frame_in(mode, &payload);
        // echoes of 2 and 5 ms: inside the cyclic prefix
        let line = padded()
            .then(Echo(vec![(32, 0.5), (80, -0.3)]))
            .then(BandPass::telephone())
            .then(Awgn::with_snr(level, 20.0, 3))
            .process(&frame);
        stats_of(&receive(&line), &payload);
        // a level meant for the tones cuts the peaks of the carriers: 6 dB above their RMS
        let loud = padded().then(Clip(0.35)).process(&frame);
        stats_of(&receive(&loud), &payload);
    }
}

/// Two modems with sound cards at the same rate, each hearing the other
/// through `noise` after a delay of one block, and itself at half the level.
fn talk(
    alice: &mut Modem,
    bob: &mut Modem,
    noise: &mut [Awgn; 2],
    seconds: f32,
    mut jam: impl FnMut(f32) -> bool,
) {
    const BLOCK: usize = 512;
    let mut heard = [vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]];
    for step in 0..(seconds * 16_000.0) as usize / BLOCK {
        let mut played = [vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]];
        alice.push_rx(&heard[0]);
        alice.pull_tx(&mut played[0]);
        bob.push_rx(&heard[1]);
        bob.pull_tx(&mut played[1]);
        let jammed = jam(step as f32 * BLOCK as f32 / 16_000.0);
        for side in 0..2 {
            let mix: Vec<f32> = played[1 - side]
                .iter()
                .zip(&played[side])
                .map(|(other, own)| other + 0.5 * own)
                .collect();
            heard[side] = noise[side].process(&mix);
            if jammed {
                // 23 dB above the signal: a single tone would not stop the modem
                heard[side] = Awgn::new(5.0, step as u64).process(&heard[side]);
            }
        }
    }
}

fn station(station: u16) -> Modem {
    Modem::new(Config {
        station,
        seed: u64::from(station) * 31,
        ..Config::default()
    })
    .unwrap()
}

fn events(modem: &mut Modem) -> Vec<Event> {
    std::iter::from_fn(|| modem.poll_event()).collect()
}

fn texts(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| {
            if let Event::MessageReceived { text, .. } = e {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn two_modems_chat_with_noise_6_db_above_the_signal() {
    let (mut alice, mut bob) = (station(1), station(2));
    // the signal has the RMS 0.35
    let mut noise = [Awgn::new(0.7, 21), Awgn::new(0.7, 22)];
    let first = alice.send_text("Как слышно? Приём.").unwrap();
    let second = bob.send_text("Слышу хорошо 👍").unwrap();
    talk(&mut alice, &mut bob, &mut noise, 60.0, |_| false);
    let (at_alice, at_bob) = (events(&mut alice), events(&mut bob));
    assert_eq!(texts(&at_bob), ["Как слышно? Приём."]);
    assert_eq!(texts(&at_alice), ["Слышу хорошо 👍"]);
    assert!(
        at_alice.contains(&Event::MessageDelivered { id: first }),
        "{at_alice:?}"
    );
    assert!(
        at_bob.contains(&Event::MessageDelivered { id: second }),
        "{at_bob:?}"
    );
}

#[test]
fn a_frame_drowned_by_interference_is_repeated_and_delivered_once() {
    let (mut alice, mut bob) = (station(1), station(2));
    let mut noise = [Awgn::new(0.05, 1), Awgn::new(0.05, 2)];
    let id = alice.send_text("through the interference").unwrap();
    // loud noise over the first three seconds: most of the first attempt
    talk(&mut alice, &mut bob, &mut noise, 30.0, |time| time < 3.0);
    let (at_alice, at_bob) = (events(&mut alice), events(&mut bob));
    assert_eq!(texts(&at_bob), ["through the interference"]);
    assert_eq!(
        at_alice
            .iter()
            .filter(|e| matches!(e, Event::Retry { .. }))
            .count(),
        1,
        "{at_alice:?}"
    );
    assert!(
        at_alice.contains(&Event::MessageDelivered { id }),
        "{at_alice:?}"
    );
}

#[test]
fn a_dead_channel_ends_in_not_delivered() {
    let (mut alice, mut bob) = (station(1), station(2));
    let mut noise = [Awgn::new(0.05, 1), Awgn::new(0.05, 2)];
    let id = alice.send_text("nobody will hear this").unwrap();
    talk(&mut alice, &mut bob, &mut noise, 60.0, |_| true);
    assert_eq!(
        events(&mut alice).last(),
        Some(&Event::MessageFailed { id })
    );
    assert_eq!(texts(&events(&mut bob)), [] as [&str; 0]);
    assert!(alice.is_idle());
}

#[test]
fn the_speed_settles_where_the_noise_allows() {
    let (mut alice, mut bob) = (station(1), station(2));
    // Noise 2 dB below a single tone: enough for 125 bit/s, not for 250.
    let mut noise = [Awgn::new(0.28, 7), Awgn::new(0.28, 8)];
    let ids: Vec<u32> = (0..5)
        .map(|n| {
            alice
                .send_text(&format!("message number {n}, of some length"))
                .unwrap()
        })
        .collect();
    talk(&mut alice, &mut bob, &mut noise, 60.0, |_| false);
    let (at_alice, at_bob) = (events(&mut alice), events(&mut bob));
    assert_eq!(texts(&at_bob).len(), 5);
    for id in ids {
        assert!(
            at_alice.contains(&Event::MessageDelivered { id }),
            "{at_alice:?}"
        );
    }
    let modes: Vec<Mode> = at_alice
        .iter()
        .filter_map(|e| {
            if let Event::TxMode { mode } = e {
                Some(*mode)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(modes, [Mode::R2]);
    assert!(
        !at_alice.iter().any(|e| matches!(e, Event::Retry { .. })),
        "{at_alice:?}"
    );
}

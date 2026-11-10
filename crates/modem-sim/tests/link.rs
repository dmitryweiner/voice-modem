//! The modem through simulated channels. The limits here are regression
//! guards with a margin of about 3 dB to what `examples/sweep.rs` measures.

use modem_core::{
    Command, Config, Event, Mode, Modem, Receiver, RxEvent, RxStats, TxConfig, encode_frame,
};
use modem_sim::{
    Awgn, BandPass, Chain, Channel, Clip, ClockOffset, Echo, FreqShift, Gain, Pad, rms,
};

fn payload(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(37).wrapping_add(11))
        .collect()
}

fn frame(payload: &[u8]) -> Vec<f32> {
    encode_frame(1, Mode::R1, payload, &TxConfig::default()).unwrap()
}

fn receive(samples: &[f32]) -> Vec<RxEvent> {
    let mut receiver = Receiver::new();
    receiver.push(samples);
    std::iter::from_fn(|| receiver.poll()).collect()
}

/// The stats of the only frame received, which must carry `payload`.
fn stats_of(events: &[RxEvent], payload: &[u8]) -> RxStats {
    match events {
        [
            RxEvent::Frame {
                payload: got,
                stats,
                ..
            },
        ] if got == payload => *stats,
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

#[test]
fn two_modems_talk_through_a_bad_channel() {
    let (mut alice, mut bob) = (
        Modem::new(Config::default()).unwrap(),
        Modem::new(Config::default()).unwrap(),
    );
    let mut channel = Chain::new()
        .then(BandPass::telephone())
        .then(FreqShift(35.0))
        .then(ClockOffset { ppm: 120.0 })
        .then(Awgn::new(0.5, 21));

    let mut say = |from: &mut Modem, to: &mut Modem, text: &str| {
        from.command(Command::SendText(text.into())).unwrap();
        let mut sent = vec![0.0; from.tx_pending() + 8000];
        from.pull_tx(&mut sent);
        // the sound card hands the recording over in small blocks
        for block in channel.process(&sent).chunks(1024) {
            to.push_rx(block);
        }
        match to.poll_event() {
            Some(Event::MessageReceived { text, .. }) => text,
            other => panic!("expected a message, got {other:?}"),
        }
    };

    assert_eq!(
        say(&mut alice, &mut bob, "Как слышно? Приём."),
        "Как слышно? Приём."
    );
    assert_eq!(
        say(&mut bob, &mut alice, "Слышу хорошо 👍"),
        "Слышу хорошо 👍"
    );
}

//! The modem as the application sees it: texts and samples in, events and
//! samples out. Connects the link layer to the transmitter and the receiver.
//!
//! Time is counted in received samples, so the application must keep
//! feeding `push_rx` — silence too — for anything to happen: transmissions
//! start, acknowledgements time out and repeats go out while samples flow.
//! Whatever `pull_tx` gives must be played at once: the modem takes its own
//! transmission to last as long as its samples and does not listen meanwhile.

use std::collections::VecDeque;

use crate::SAMPLE_RATE;
use crate::frame::{Kind, Mode};
use crate::link::{GAP, Link, LinkConfig, LinkEvent, SendError};
use crate::resample::Resampler;
use crate::rx::{Receiver, RxEvent, RxStats};
use crate::tx::{TxConfig, encode_frame, frame_len};

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Sample rate of the audio device, Hz: 8000 to 192000. The samples of
    /// `push_rx` and `pull_tx` are at this rate; the core converts.
    pub sample_rate: u32,
    pub tx: TxConfig,
    /// The address of this station on the channel: pick it at random.
    pub station: u16,
    /// The name or callsign sent in a greeting; may be empty.
    pub name: String,
    /// True: frames are acknowledged and repeated. False: everything is sent
    /// once and nothing is expected back.
    pub reliable: bool,
    /// True: every transmission waits for `ptt_pressed` (a radio with a
    /// push-to-talk button held by a human).
    pub manual_ptt: bool,
    /// Seed of the random waits of the link layer.
    pub seed: u64,
    /// The mode of every data frame, or None to adapt: start with the
    /// slowest and follow the advice of the other station.
    pub mode: Option<Mode>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_rate: SAMPLE_RATE,
            tx: TxConfig::default(),
            station: 1,
            name: String::new(),
            reliable: true,
            manual_ptt: false,
            seed: 1,
            mode: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    UnsupportedSampleRate(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandError {
    EmptyText,
    /// `max` is the limit in UTF-8 bytes.
    TextTooLong {
        max: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    MessageReceived {
        from: u16,
        text: String,
        stats: RxStats,
    },
    /// A station greeted with its name.
    PeerHello {
        from: u16,
        name: String,
        stats: RxStats,
    },
    /// A message of an unreliable modem has been transmitted.
    MessageSent { id: u32 },
    /// The other side acknowledged the whole message.
    MessageDelivered { id: u32 },
    /// The message was not acknowledged after every attempt.
    MessageFailed { id: u32 },
    /// No acknowledgement: part of the message goes out again, for the `attempt`-th time.
    Retry { id: u32, attempt: u8 },
    /// Something was transmitted nearby but could not be read.
    FrameLost { stats: RxStats },
    /// Push-to-talk: press the button and call `ptt_pressed`.
    TxRequest,
    /// Data frames go out in this mode from now on.
    TxMode { mode: Mode },
}

pub struct Modem {
    tx_config: TxConfig,
    /// Device rate → core rate and back; None when they are the same.
    rx_resampler: Option<Resampler>,
    tx_resampler: Option<Resampler>,
    receiver: Receiver,
    link: Link,
    /// Samples received so far, at the core rate: the time of the link layer.
    clock: u64,
    tx: VecDeque<f32>,
    events: VecDeque<Event>,
}

impl Modem {
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        let rate = config.sample_rate;
        if !(8000..=192_000).contains(&rate) {
            return Err(ConfigError::UnsupportedSampleRate(rate));
        }
        let converts = rate != SAMPLE_RATE;
        let Config {
            tx,
            station,
            name,
            reliable,
            manual_ptt,
            seed,
            mode,
            ..
        } = config;
        let ack_len = frame_len(Kind::Ack { advice: Mode::R1 }, Mode::R1, 0) as u64
            + u64::from(tx.lead_in_ms) * u64::from(SAMPLE_RATE) / 1000;
        Ok(Self {
            tx_config: tx,
            rx_resampler: converts.then(|| Resampler::new(rate, SAMPLE_RATE)),
            tx_resampler: converts.then(|| Resampler::new(SAMPLE_RATE, rate)),
            receiver: Receiver::new(),
            link: Link::new(
                LinkConfig {
                    station,
                    name,
                    reliable,
                    manual_ptt,
                    seed,
                    mode,
                },
                ack_len,
            ),
            clock: 0,
            tx: VecDeque::new(),
            events: VecDeque::new(),
        })
    }

    /// Queues a text; its fate is reported in events under the returned id.
    pub fn send_text(&mut self, text: &str) -> Result<u32, CommandError> {
        self.link.send_text(text).map_err(|error| match error {
            SendError::Empty => CommandError::EmptyText,
            SendError::TooLong { max } => CommandError::TextTooLong { max },
        })
    }

    /// Queues a greeting with the name of this station; stations that hear it answer with theirs.
    pub fn hello(&mut self) {
        self.link.hello(self.clock);
    }

    /// Push-to-talk: the button is down, transmit what is waiting.
    pub fn ptt_pressed(&mut self) {
        self.link.ptt_pressed();
    }

    /// Samples from the microphone.
    pub fn push_rx(&mut self, samples: &[f32]) {
        let converted;
        let samples = match &mut self.rx_resampler {
            Some(resampler) => {
                let mut out = Vec::new();
                resampler.process(samples, &mut out);
                converted = out;
                &converted[..]
            }
            None => samples,
        };
        // While transmitting and right after, the microphone hears the modem itself.
        if self.clock >= self.link.deaf_until() {
            self.receiver.push(samples);
        }
        self.clock += samples.len() as u64;

        while let Some(event) = self.receiver.poll() {
            match event {
                RxEvent::Frame { frame, stats, .. } => {
                    self.link.on_frame(&frame, stats.snr_db, self.clock);
                    self.take_link_events(Some(stats));
                }
                RxEvent::Damaged { stats, .. } | RxEvent::Unreadable { stats } => {
                    self.events.push_back(Event::FrameLost { stats })
                }
            }
        }
        self.transmit();
        self.take_link_events(None);
    }

    /// Asks the link layer what to transmit now and puts its samples into the queue.
    fn transmit(&mut self) {
        let frames = self.link.poll(self.clock, self.receiver.busy());
        if frames.is_empty() {
            return;
        }
        let mut burst = Vec::new();
        for (index, frame) in frames.iter().enumerate() {
            // The tone that opens the radio is needed once, at the start.
            let config = if index == 0 {
                self.tx_config
            } else {
                TxConfig {
                    lead_in_ms: 0,
                    ..self.tx_config
                }
            };
            let samples = encode_frame(frame, self.link.tx_mode(), &config)
                .expect("the link layer keeps frames within the limit");
            burst.extend(samples);
            // The pause also pushes the end of the frame out of the resampler.
            burst.resize(burst.len() + GAP as usize, 0.0);
        }
        self.link.on_tx(self.clock, burst.len() as u64);
        // What was being received is cut off by the transmission.
        self.receiver.reset();
        match &mut self.tx_resampler {
            Some(resampler) => {
                let mut converted = Vec::new();
                resampler.process(&burst, &mut converted);
                self.tx.extend(converted);
            }
            None => self.tx.extend(burst),
        }
    }

    /// Moves the events of the link layer out; `stats` are those of the frame that caused them.
    fn take_link_events(&mut self, stats: Option<RxStats>) {
        while let Some(event) = self.link.poll_event() {
            self.events.push_back(match (event, stats) {
                (LinkEvent::Text { from, text }, Some(stats)) => {
                    Event::MessageReceived { from, text, stats }
                }
                (LinkEvent::Hello { from, name, .. }, Some(stats)) => {
                    Event::PeerHello { from, name, stats }
                }
                (LinkEvent::Text { .. } | LinkEvent::Hello { .. }, None) => {
                    unreachable!("only a received frame brings a message")
                }
                (LinkEvent::Sent { id }, _) => Event::MessageSent { id },
                (LinkEvent::Delivered { id }, _) => Event::MessageDelivered { id },
                (LinkEvent::Failed { id }, _) => Event::MessageFailed { id },
                (LinkEvent::Retry { id, attempt }, _) => Event::Retry { id, attempt },
                (LinkEvent::TxRequest, _) => Event::TxRequest,
                (LinkEvent::TxMode { mode }, _) => Event::TxMode { mode },
            });
        }
    }

    /// Fills `out` with samples for the speaker: silence when there is nothing to send.
    pub fn pull_tx(&mut self, out: &mut [f32]) {
        for sample in out {
            *sample = self.tx.pop_front().unwrap_or(0.0);
        }
    }

    /// Samples waiting to be played.
    pub fn tx_pending(&self) -> usize {
        self.tx.len()
    }

    /// Nothing queued, nothing awaited, nothing left to play.
    pub fn is_idle(&self) -> bool {
        self.link.is_idle(self.clock) && self.tx.is_empty()
    }

    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::MAX_TEXT_BYTES;

    fn modem(station: u16) -> Modem {
        Modem::new(Config {
            station,
            seed: u64::from(station),
            ..Config::default()
        })
        .unwrap()
    }

    /// A perfect cable between sound cards: what each modem plays, the others hear a block later.
    fn run(modems: &mut [&mut Modem], seconds: f32) {
        const BLOCK: usize = 512;
        let mut heard = vec![vec![0.0f32; BLOCK]; modems.len()];
        for _ in 0..(seconds * SAMPLE_RATE as f32) as usize / BLOCK {
            let mut played = vec![vec![0.0f32; BLOCK]; modems.len()];
            for (modem, (block, incoming)) in modems.iter_mut().zip(played.iter_mut().zip(&heard)) {
                modem.push_rx(incoming);
                modem.pull_tx(block);
            }
            for (index, mix) in heard.iter_mut().enumerate() {
                mix.fill(0.0);
                for (other, block) in played.iter().enumerate() {
                    // a modem hears itself too, as through a speaker next to its microphone
                    let gain = if other == index { 0.5 } else { 1.0 };
                    mix.iter_mut()
                        .zip(block)
                        .for_each(|(sum, sample)| *sum += gain * sample);
                }
            }
        }
    }

    fn all_events(modem: &mut Modem) -> Vec<Event> {
        std::iter::from_fn(|| modem.poll_event()).collect()
    }

    /// The events without the changes of mode, which most tests do not care about.
    fn events(modem: &mut Modem) -> Vec<Event> {
        all_events(modem)
            .into_iter()
            .filter(|e| !matches!(e, Event::TxMode { .. }))
            .collect()
    }

    fn texts(events: &[Event]) -> Vec<(u16, &str)> {
        events
            .iter()
            .filter_map(|e| {
                if let Event::MessageReceived { from, text, .. } = e {
                    Some((*from, text.as_str()))
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn a_text_is_delivered_and_the_sender_learns_it() {
        let (mut alice, mut bob) = (modem(1), modem(2));
        let id = alice.send_text("Привет, модем! 👋").unwrap();
        run(&mut [&mut alice, &mut bob], 8.0);
        assert_eq!(texts(&events(&mut bob)), [(1, "Привет, модем! 👋")]);
        assert_eq!(events(&mut alice), [Event::MessageDelivered { id }]);
        assert!(alice.is_idle() && bob.is_idle());
    }

    #[test]
    fn a_conversation_in_both_directions() {
        let (mut alice, mut bob) = (modem(1), modem(2));
        alice.send_text("Как слышно?").unwrap();
        run(&mut [&mut alice, &mut bob], 7.0);
        bob.send_text("Слышу хорошо").unwrap();
        // both speak at once now
        alice.send_text("И ещё вопрос").unwrap();
        run(&mut [&mut alice, &mut bob], 40.0);
        let (at_alice, at_bob) = (events(&mut alice), events(&mut bob));
        assert_eq!(texts(&at_bob), [(1, "Как слышно?"), (1, "И ещё вопрос")]);
        assert_eq!(texts(&at_alice), [(2, "Слышу хорошо")]);
        assert_eq!(
            at_alice
                .iter()
                .filter(|e| matches!(e, Event::MessageDelivered { .. }))
                .count(),
            2
        );
        assert_eq!(
            at_bob
                .iter()
                .filter(|e| matches!(e, Event::MessageDelivered { .. }))
                .count(),
            1
        );
        assert!(
            !at_alice
                .iter()
                .chain(&at_bob)
                .any(|e| matches!(e, Event::MessageFailed { .. }))
        );
    }

    #[test]
    fn does_not_receive_its_own_transmission() {
        let mut alone = modem(1);
        let id = alone.send_text("anyone?").unwrap();
        // its speaker is next to its microphone, nobody else is there
        run(&mut [&mut alone], 60.0);
        let all = events(&mut alone);
        assert!(
            all.iter()
                .all(|e| matches!(e, Event::Retry { .. } | Event::MessageFailed { .. })),
            "{all:?}"
        );
        assert_eq!(all.last(), Some(&Event::MessageFailed { id }));
        assert!(alone.is_idle());
    }

    #[test]
    fn a_long_text_arrives_whole() {
        let (mut alice, mut bob) = (modem(1), modem(2));
        let long = "The quick brown fox jumps over the lazy dog. ".repeat(4);
        let id = alice.send_text(&long).unwrap();
        run(&mut [&mut alice, &mut bob], 40.0);
        assert_eq!(texts(&events(&mut bob)), [(1, long.as_str())]);
        assert_eq!(events(&mut alice), [Event::MessageDelivered { id }]);
    }

    #[test]
    fn over_a_clean_channel_the_speed_climbs_to_the_fastest_mode() {
        let (mut alice, mut bob) = (modem(1), modem(2));
        let long = "The quick brown fox jumps over the lazy dog. ".repeat(20);
        let id = alice.send_text(&long).unwrap();
        // 900 bytes: 190 in the first two frames, the rest at 250 and 500 bit/s
        run(&mut [&mut alice, &mut bob], 60.0);
        let at_alice = all_events(&mut alice);
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
        assert_eq!(modes, [Mode::R2, Mode::R3, Mode::R4]);
        assert!(
            at_alice.contains(&Event::MessageDelivered { id }),
            "{at_alice:?}"
        );
        assert_eq!(texts(&events(&mut bob)), [(1, long.as_str())]);
    }

    #[test]
    fn a_fixed_mode_is_used_from_the_first_frame() {
        let mut alice = Modem::new(Config {
            mode: Some(Mode::R4),
            reliable: false,
            ..Config::default()
        })
        .unwrap();
        let mut bob = modem(2);
        alice.send_text(&"fast and one way ".repeat(12)).unwrap();
        run(&mut [&mut alice, &mut bob], 6.0);
        assert_eq!(all_events(&mut alice), [Event::MessageSent { id: 1 }]);
        assert_eq!(texts(&events(&mut bob)).len(), 1);
    }

    #[test]
    fn greetings_carry_the_names() {
        let mut alice = Modem::new(Config {
            station: 1,
            name: "Alice".into(),
            ..Config::default()
        })
        .unwrap();
        let mut bob = Modem::new(Config {
            station: 2,
            name: "UB1XYZ".into(),
            seed: 2,
            ..Config::default()
        })
        .unwrap();
        alice.hello();
        run(&mut [&mut alice, &mut bob], 15.0);
        assert!(
            matches!(&events(&mut bob)[..], [Event::PeerHello { from: 1, name, .. }] if name == "Alice")
        );
        assert!(
            matches!(&events(&mut alice)[..], [Event::PeerHello { from: 2, name, .. }] if name == "UB1XYZ")
        );
    }

    #[test]
    fn unreliable_mode_transmits_once_without_a_listener() {
        let mut sender = Modem::new(Config {
            reliable: false,
            ..Config::default()
        })
        .unwrap();
        let id = sender.send_text("to whom it may concern").unwrap();
        let mut receiver = modem(2);
        run(&mut [&mut sender, &mut receiver], 6.0);
        assert_eq!(events(&mut sender), [Event::MessageSent { id }]);
        assert_eq!(
            texts(&events(&mut receiver)),
            [(1, "to whom it may concern")]
        );
        // nothing came back
        assert!(sender.is_idle() && receiver.is_idle());
    }

    #[test]
    fn push_to_talk_transmits_only_after_the_button() {
        let mut alice = Modem::new(Config {
            manual_ptt: true,
            tx: TxConfig {
                lead_in_ms: 300,
                ..TxConfig::default()
            },
            ..Config::default()
        })
        .unwrap();
        let mut bob = modem(2);
        let id = alice.send_text("over").unwrap();
        run(&mut [&mut alice, &mut bob], 2.0);
        assert_eq!(events(&mut alice), [Event::TxRequest]);
        assert_eq!(alice.tx_pending(), 0);
        alice.ptt_pressed();
        run(&mut [&mut alice, &mut bob], 8.0);
        assert_eq!(texts(&events(&mut bob)), [(1, "over")]);
        assert_eq!(events(&mut alice), [Event::MessageDelivered { id }]);
    }

    #[test]
    fn idle_modem_plays_silence() {
        let mut modem = modem(1);
        let mut block = [1.0; 64];
        modem.pull_tx(&mut block);
        assert_eq!(block, [0.0; 64]);
        assert_eq!(modem.tx_pending(), 0);
        assert!(modem.is_idle());
    }

    #[test]
    fn nothing_happens_without_incoming_samples() {
        let mut modem = modem(1);
        modem.send_text("waiting for the clock").unwrap();
        assert_eq!(modem.tx_pending(), 0);
        assert!(!modem.is_idle());
        modem.push_rx(&[0.0; 512]);
        assert!(modem.tx_pending() > 0);
    }

    #[test]
    fn rejects_texts_it_cannot_send() {
        let mut modem = modem(1);
        assert_eq!(modem.send_text(""), Err(CommandError::EmptyText));
        assert_eq!(modem.send_text(&"x".repeat(MAX_TEXT_BYTES)), Ok(1));
        assert_eq!(
            modem.send_text(&"x".repeat(MAX_TEXT_BYTES + 1)),
            Err(CommandError::TextTooLong { max: 1000 })
        );
        // counted in bytes, not characters
        assert_eq!(
            modem.send_text(&"я".repeat(501)),
            Err(CommandError::TextTooLong { max: 1000 })
        );
    }

    #[test]
    fn works_at_the_sample_rates_of_sound_cards() {
        for (from, to) in [
            (48_000, 44_100),
            (44_100, 48_000),
            (8000, 16_000),
            (16_000, 96_000),
        ] {
            let mut alice = Modem::new(Config {
                sample_rate: from,
                reliable: false,
                ..Config::default()
            })
            .unwrap();
            let mut bob = Modem::new(Config {
                sample_rate: to,
                station: 2,
                ..Config::default()
            })
            .unwrap();
            alice.send_text("any rate").unwrap();
            alice.send_text("twice").unwrap();
            // The air between two sound cards: a converter of its own.
            let mut air = Resampler::new(from, to);
            let block = from as usize / 50;
            let mut seconds = 0.0;
            while !alice.is_idle() {
                alice.push_rx(&vec![0.0; block]);
                let mut played = vec![0.0; block];
                alice.pull_tx(&mut played);
                let mut heard = Vec::new();
                air.process(&played, &mut heard);
                bob.push_rx(&heard);
                seconds += 0.02;
            }
            bob.push_rx(&vec![0.0; to as usize]);
            // two frames of 3.0 and 2.6 s with pauses, whatever the rate
            assert!((seconds - 6.0f32).abs() < 0.2, "{from}: {seconds} s");
            assert_eq!(
                texts(&events(&mut bob)),
                [(1, "any rate"), (1, "twice")],
                "{from} → {to}"
            );
        }
    }

    #[test]
    fn rejects_impossible_sample_rates() {
        for rate in [0, 4000, 400_000] {
            let config = Config {
                sample_rate: rate,
                ..Config::default()
            };
            assert_eq!(
                Modem::new(config).err(),
                Some(ConfigError::UnsupportedSampleRate(rate))
            );
        }
    }

    #[test]
    fn reports_what_was_heard_but_not_read() {
        let (mut alice, mut bob) = (modem(1), modem(2));
        alice.send_text("this one gets damaged on the way").unwrap();
        alice.push_rx(&[0.0; 512]);
        let mut samples = vec![0.0; alice.tx_pending()];
        alice.pull_tx(&mut samples);
        for (n, x) in samples[25_000..50_000].iter_mut().enumerate() {
            *x = (n as f32 * 0.4).sin() * 0.5;
        }
        bob.push_rx(&samples);
        assert!(matches!(&events(&mut bob)[..], [Event::FrameLost { .. }]));
    }
}

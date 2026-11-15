//! The modem as the application sees it: texts, files and samples in, events
//! and samples out. Connects the link layer to the transmitter and the
//! receiver, and the file transfer to the link layer.
//!
//! Time is counted in received samples, so the application must keep
//! feeding `push_rx` — silence too — for anything to happen: transmissions
//! start, acknowledgements time out and repeats go out while samples flow.
//! Whatever `pull_tx` gives must be played at once: the modem takes its own
//! transmission to last as long as its samples and does not listen meanwhile.

use std::collections::VecDeque;

use crate::SAMPLE_RATE;
use crate::files::{
    ANNOUNCE_EVERY, ANNOUNCE_HEADER, CHUNK_HEADER, FileError, FileEvent, Files, SYMBOL_HEADER,
};
use crate::fountain::symbols_for;
use crate::frame::{Kind, Mode};
use crate::link::{GAP, Heard, Link, LinkConfig, LinkEvent, SendError};
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
    /// `max` is the limit in bytes (of the file deflated, when it deflates).
    FileTooLarge {
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
    /// A station offers a file: answer with `accept_file` or `cancel_file`.
    FileOffered {
        id: u32,
        from: u16,
        name: String,
        size: u32,
    },
    /// The other side has `done` bytes of the file `id` (sending), or this
    /// side has (receiving). The time left is unknown before the first chunk.
    FileProgress {
        id: u32,
        done: u32,
        seconds_left: Option<u32>,
    },
    /// The whole file arrived and its checksum is right.
    FileReceived {
        id: u32,
        name: String,
        bytes: Vec<u8>,
    },
    /// The other side has the whole file.
    FileDelivered { id: u32 },
    /// Sending: the other side stopped answering. Receiving: the content
    /// arrived damaged, or the offer gave way to newer ones.
    FileFailed { id: u32 },
    /// The other side declined the file or gave the transfer up.
    FileRefused { id: u32 },
    /// A station broadcasts a file: it is being collected, `FileProgress`
    /// and `FileReceived` follow under this id.
    FileHeard {
        id: u32,
        from: u16,
        name: String,
        size: u32,
    },
    /// So many symbols of the file being broadcast went; `needed` of them
    /// make the file.
    FileCast { id: u32, symbols: u32, needed: u32 },
    /// The broadcast ended by itself: ten times the file went.
    FileSent { id: u32 },
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
    files: Files,
    /// Frames are acknowledged: the other side can answer an offer.
    reliable: bool,
    /// Length of the lead-in tone, samples at the core rate.
    lead_in: u64,
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
        let lead_in = u64::from(tx.lead_in_ms) * u64::from(SAMPLE_RATE) / 1000;
        let ack = Kind::Ack {
            advice: Mode::R1,
            missing: 0,
        };
        let ack_len = frame_len(ack, Mode::R1, 0) as u64 + lead_in;
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
            files: Files::new(),
            reliable,
            lead_in,
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

    /// Takes back a text that is not delivered yet; false when it is no
    /// longer waiting. What is already playing is played to the end.
    pub fn cancel(&mut self, id: u32) -> bool {
        self.link.cancel(id)
    }

    /// Offers a file to the other side; its fate is reported in events under
    /// the returned id (files and texts are numbered apart). Nothing of the
    /// content is sent before the offer is accepted there. A modem that
    /// expects no answers broadcasts the file instead, in symbols that fill
    /// a frame each, until it is cancelled or ten times the file went.
    pub fn send_file(&mut self, name: &str, bytes: Vec<u8>) -> Result<u32, CommandError> {
        let queued = if self.reliable {
            self.files.send(name, bytes)
        } else {
            let symbol = self.link.room() - SYMBOL_HEADER;
            self.files.broadcast(name, bytes, symbol)
        };
        queued.map_err(|FileError::TooLarge { max }| CommandError::FileTooLarge { max })
    }

    /// Consents to an offered file; false when there is no such offer.
    pub fn accept_file(&mut self, id: u32) -> bool {
        self.files.accept(id, self.clock)
    }

    /// Gives up a file that is being sent or received, or declines an
    /// offer; the other side is told. False when there is no such file any
    /// more. Nothing more is reported about it.
    pub fn cancel_file(&mut self, id: u32) -> bool {
        self.files.cancel(id)
    }

    /// Bytes the file `id`, offered and not yet sent, goes on the air as:
    /// fewer than it has when it deflates. `file_seconds` of these is how
    /// long it takes.
    pub fn file_sent_bytes(&self, id: u32) -> Option<usize> {
        self.files.sent_bytes(id)
    }

    /// Seconds a file of `size` bytes on the air takes at the speed of the
    /// moment, once it is accepted and if no frame is lost. Broadcast: until
    /// every piece went once, which a listener on a clean channel needs.
    pub fn file_seconds(&self, size: usize) -> u32 {
        let mode = self.link.tx_mode();
        let room = self.link.room();
        let samples = if self.reliable {
            let chunk = room - CHUNK_HEADER;
            // A frame with so many bytes of the file, its acknowledgement and the pause.
            let exchange = |bytes: usize| {
                let message = 1 + CHUNK_HEADER + bytes;
                self.lead_in + frame_len(Kind::Data, mode, message) as u64 + self.link.answer_len()
            };
            let whole = (size / chunk) as u64 * exchange(chunk);
            let rest = match size % chunk {
                0 => 0,
                bytes => exchange(bytes),
            };
            whole + rest
        } else {
            // A frame with a message and the pauses after it; nothing is awaited.
            let frame = |message: usize| {
                self.lead_in + frame_len(Kind::Broadcast, mode, 1 + message) as u64 + 2 * GAP
            };
            // A symbol is a piece of the file in a frame of its own; an
            // announce with a name of some sixteen bytes goes before every eight.
            let pieces = symbols_for(size, room - SYMBOL_HEADER);
            let symbol = size.div_ceil(pieces).max(1);
            let pieces = pieces as u64;
            pieces * frame(SYMBOL_HEADER + symbol)
                + pieces.div_ceil(u64::from(ANNOUNCE_EVERY)) * frame(ANNOUNCE_HEADER + 16)
        };
        samples.div_ceil(u64::from(SAMPLE_RATE)) as u32
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
                RxEvent::Frame {
                    frame,
                    stats,
                    missing,
                    ..
                } => {
                    let heard = Heard {
                        snr_db: stats.snr_db,
                        carriers_snr_db: stats.carriers_snr_db,
                        missing: u8::try_from(missing).unwrap_or(u8::MAX),
                    };
                    self.link.on_frame(&frame, heard, self.clock);
                    self.take_link_events(Some(stats));
                }
                RxEvent::Damaged { stats, .. } | RxEvent::Unreadable { stats } => {
                    self.events.push_back(Event::FrameLost { stats })
                }
            }
        }
        self.pump_files();
        self.transmit();
        self.take_link_events(None);
        while let Some(event) = self.files.poll_event() {
            self.events.push_back(match event {
                FileEvent::Offered {
                    id,
                    from,
                    name,
                    size,
                } => Event::FileOffered {
                    id,
                    from,
                    name,
                    size,
                },
                FileEvent::Progress {
                    id,
                    done,
                    seconds_left,
                } => Event::FileProgress {
                    id,
                    done,
                    seconds_left,
                },
                FileEvent::Received { id, name, bytes } => Event::FileReceived { id, name, bytes },
                FileEvent::Delivered { id } => Event::FileDelivered { id },
                FileEvent::Failed { id } => Event::FileFailed { id },
                FileEvent::Refused { id } => Event::FileRefused { id },
                FileEvent::Heard {
                    id,
                    from,
                    name,
                    size,
                } => Event::FileHeard {
                    id,
                    from,
                    name,
                    size,
                },
                FileEvent::Cast {
                    id,
                    symbols,
                    needed,
                } => Event::FileCast {
                    id,
                    symbols,
                    needed,
                },
                FileEvent::Sent { id } => Event::FileSent { id },
            });
        }
    }

    /// Hands the messages of the file transfer to the link layer: answers
    /// at once, a chunk only when nothing else waits there.
    fn pump_files(&mut self) {
        while let Some(token) = self.files.poll_take_back() {
            self.link.cancel(token);
        }
        while let Some(bytes) = self
            .files
            .poll(self.link.room(), self.link.nothing_queued())
        {
            let token = self.link.send_file(bytes);
            self.files.sent_as(token);
        }
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
            // What becomes of a message of the file transfer is its business.
            let files = match &event {
                LinkEvent::File { from, bytes } => {
                    self.files.on_message(*from, bytes, self.clock);
                    true
                }
                // Over a one-way channel "sent" is all a broadcast learns.
                LinkEvent::Delivered { id } | LinkEvent::Sent { id } => {
                    self.files.on_delivered(*id, self.clock)
                }
                LinkEvent::Failed { id } => self.files.on_failed(*id),
                LinkEvent::Retry { id, .. } => self.files.owns(*id),
                _ => false,
            };
            if files {
                continue;
            }
            self.events.push_back(match (event, stats) {
                (LinkEvent::Text { from, text }, Some(stats)) => {
                    Event::MessageReceived { from, text, stats }
                }
                (LinkEvent::Hello { from, name, .. }, Some(stats)) => {
                    Event::PeerHello { from, name, stats }
                }
                (LinkEvent::Text { .. } | LinkEvent::Hello { .. }, None)
                | (LinkEvent::File { .. }, _) => {
                    unreachable!("only a received frame brings a message, and files took theirs")
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
        self.link.is_idle(self.clock) && self.files.is_idle() && self.tx.is_empty()
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
    #[derive(Default)]
    struct Cable {
        /// What each modem hears next. Kept between runs: a frame may be on its way.
        heard: Vec<Vec<f32>>,
    }

    impl Cable {
        const BLOCK: usize = 512;

        fn run(&mut self, modems: &mut [&mut Modem], seconds: f32) {
            self.heard.resize(modems.len(), vec![0.0; Self::BLOCK]);
            for _ in 0..(seconds * SAMPLE_RATE as f32) as usize / Self::BLOCK {
                let mut played = vec![vec![0.0f32; Self::BLOCK]; modems.len()];
                for (modem, (block, incoming)) in
                    modems.iter_mut().zip(played.iter_mut().zip(&self.heard))
                {
                    modem.push_rx(incoming);
                    modem.pull_tx(block);
                }
                for (index, mix) in self.heard.iter_mut().enumerate() {
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
    }

    /// A cable for one run: between runs the channel must be silent.
    fn run(modems: &mut [&mut Modem], seconds: f32) {
        Cable::default().run(modems, seconds);
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
    fn a_cancelled_text_is_not_repeated() {
        let mut alone = modem(1);
        let id = alone.send_text("anyone?").unwrap();
        run(&mut [&mut alone], 4.0);
        assert!(alone.cancel(id));
        run(&mut [&mut alone], 20.0);
        assert_eq!(events(&mut alone), []);
        assert!(alone.is_idle());
        assert!(!alone.cancel(id));
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
        let first = alice.send_text(&long).unwrap();
        let second = alice.send_text(&long).unwrap();
        // 900 bytes: 440 in three frames of tones, the rest in one of
        // carriers; then all of it again in one frame, in half a second
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
        assert_eq!(modes, [Mode::R2, Mode::R3, Mode::F1, Mode::F2, Mode::F3]);
        for id in [first, second] {
            assert!(
                at_alice.contains(&Event::MessageDelivered { id }),
                "{at_alice:?}"
            );
        }
        assert_eq!(texts(&events(&mut bob)), [(1, long.as_str()); 2]);
    }

    #[test]
    fn a_fixed_mode_is_used_from_the_first_frame() {
        let mut alice = Modem::new(Config {
            mode: Some(Mode::F3),
            reliable: false,
            ..Config::default()
        })
        .unwrap();
        let mut bob = modem(2);
        alice.send_text(&"fast and one way ".repeat(50)).unwrap();
        run(&mut [&mut alice, &mut bob], 4.0);
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
            // two frames of packed texts with their answers and pauses, whatever the rate
            assert!((seconds - 5.46f32).abs() < 0.2, "{from}: {seconds} s");
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

    /// Bytes that do not deflate: the file goes on the air as it is.
    fn file(len: usize) -> Vec<u8> {
        let mut seed = 7u32;
        (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect()
    }

    /// The modem `from` offers a file and `to` accepts it; their ids of the file.
    fn offer_and_accept(from: &mut Modem, to: &mut Modem, len: usize) -> (u32, u32) {
        let up = from.send_file("notes.txt", file(len)).unwrap();
        run(&mut [from, to], 8.0);
        let offered = events(to);
        let [
            Event::FileOffered {
                id: down,
                from: 1,
                name,
                size,
            },
        ] = &offered[..]
        else {
            panic!("{offered:?}");
        };
        assert_eq!((name.as_str(), *size), ("notes.txt", len as u32));
        assert_eq!(events(from), []);
        assert!(to.accept_file(*down));
        (up, *down)
    }

    #[test]
    fn a_file_is_offered_accepted_and_arrives_whole() {
        let (mut alice, mut bob) = (modem(1), modem(2));
        let (up, down) = offer_and_accept(&mut alice, &mut bob, 5000);
        run(&mut [&mut alice, &mut bob], 90.0);
        let (at_alice, at_bob) = (events(&mut alice), events(&mut bob));
        // nothing of the link layer shows through: only the file is reported
        assert!(
            at_alice[..at_alice.len() - 1]
                .iter()
                .all(|e| matches!(e, Event::FileProgress { id, .. } if *id == up)),
            "{at_alice:?}"
        );
        assert_eq!(at_alice.last(), Some(&Event::FileDelivered { id: up }));
        // The offer went in the slowest mode; the chunks fill the frames of
        // the next ones: a block of R2, one of R3, two of F1, and the rest
        // in a frame of F2, which takes four.
        assert_eq!(at_bob.len(), 4);
        assert_eq!(
            at_bob[..3]
                .iter()
                .map(|e| match e {
                    Event::FileProgress { done, .. } => *done,
                    _ => 0,
                })
                .collect::<Vec<_>>(),
            [121, 369, 2408]
        );
        assert_eq!(
            at_bob.last(),
            Some(&Event::FileReceived {
                id: down,
                name: "notes.txt".into(),
                bytes: file(5000)
            })
        );
        assert!(alice.is_idle() && bob.is_idle());
    }

    #[test]
    fn a_text_goes_ahead_of_the_rest_of_a_file() {
        let (mut alice, mut bob) = (modem(1), modem(2));
        offer_and_accept(&mut alice, &mut bob, 1000);
        run(&mut [&mut alice, &mut bob], 12.0);
        let text = alice.send_text("meanwhile").unwrap();
        run(&mut [&mut alice, &mut bob], 60.0);
        let (at_alice, at_bob) = (events(&mut alice), events(&mut bob));
        let place = |events: &[Event], wanted: fn(&Event) -> bool| {
            events.iter().position(wanted).expect("the event")
        };
        assert!(at_alice.contains(&Event::MessageDelivered { id: text }));
        let got_text = place(&at_bob, |e| matches!(e, Event::MessageReceived { .. }));
        let got_file = place(&at_bob, |e| matches!(e, Event::FileReceived { .. }));
        assert!(got_text < got_file);
    }

    #[test]
    fn a_cancelled_file_stops_on_both_sides() {
        let (mut alice, mut bob) = (modem(1), modem(2));
        let (up, down) = offer_and_accept(&mut alice, &mut bob, 5000);
        run(&mut [&mut alice, &mut bob], 12.0);
        assert!(alice.cancel_file(up));
        assert!(!alice.cancel_file(up));
        run(&mut [&mut alice, &mut bob], 20.0);
        assert_eq!(
            events(&mut bob).last(),
            Some(&Event::FileRefused { id: down })
        );
        assert!(
            events(&mut alice)
                .iter()
                .all(|e| matches!(e, Event::FileProgress { .. }))
        );
        assert!(alice.is_idle() && bob.is_idle());
    }

    #[test]
    fn a_declined_file_is_reported_to_its_sender() {
        let (mut alice, mut bob) = (modem(1), modem(2));
        let up = alice.send_file("notes.txt", file(100)).unwrap();
        run(&mut [&mut alice, &mut bob], 8.0);
        assert!(bob.cancel_file(1));
        assert!(!bob.accept_file(1));
        run(&mut [&mut alice, &mut bob], 8.0);
        assert_eq!(events(&mut alice), [Event::FileRefused { id: up }]);
    }

    #[test]
    fn a_file_into_the_void_fails() {
        let mut alone = modem(1);
        let up = alone.send_file("notes.txt", file(100)).unwrap();
        run(&mut [&mut alone], 60.0);
        assert_eq!(events(&mut alone), [Event::FileFailed { id: up }]);
    }

    #[test]
    fn the_time_a_file_takes_is_known_beforehand() {
        let fixed = |station, mode| {
            Modem::new(Config {
                station,
                seed: u64::from(station),
                mode: Some(mode),
                ..Config::default()
            })
            .unwrap()
        };
        for (mode, len) in [(Mode::R1, 300), (Mode::R3, 1000), (Mode::F3, 20_000)] {
            let (mut alice, mut bob) = (fixed(1, mode), fixed(2, mode));
            let estimate = alice.file_seconds(len) as f32;
            offer_and_accept(&mut alice, &mut bob, len);
            let mut cable = Cable::default();
            // from the consent of the receiver to the last acknowledgement
            let (mut start, mut end, mut time) = (None, None, 0.0);
            while end.is_none() && time < 2.0 * estimate + 20.0 {
                cable.run(&mut [&mut alice, &mut bob], 0.5);
                time += 0.5;
                for event in events(&mut alice) {
                    match event {
                        Event::FileProgress { seconds_left, .. } if seconds_left.is_none() => {
                            start = Some(time)
                        }
                        Event::FileDelivered { .. } => end = Some(time),
                        _ => {}
                    }
                }
            }
            let took = end.expect("delivered") - start.expect("accepted");
            assert!(
                (estimate / took - 1.0).abs() < 0.1,
                "{mode:?}: {estimate} s expected, took {took} s"
            );
        }
    }

    fn one_way(station: u16, mode: Mode) -> Modem {
        Modem::new(Config {
            station,
            seed: u64::from(station),
            reliable: false,
            mode: Some(mode),
            ..Config::default()
        })
        .unwrap()
    }

    #[test]
    fn a_file_is_broadcast_to_whoever_listens() {
        let (mut alice, mut bob) = (one_way(1, Mode::R1), one_way(2, Mode::R1));
        let content = file(300);
        let up = alice.send_file("notes.txt", content.clone()).unwrap();
        run(&mut [&mut alice, &mut bob], 120.0);
        let at_bob = events(&mut bob);
        assert_eq!(
            at_bob[0],
            Event::FileHeard {
                id: 1,
                from: 1,
                name: "notes.txt".into(),
                size: 300
            }
        );
        assert!(
            at_bob.contains(&Event::FileReceived {
                id: 1,
                name: "notes.txt".into(),
                bytes: content
            }),
            "{at_bob:?}"
        );
        // symbols of 59 bytes in R1: six make the file, and more keep coming
        let at_alice = events(&mut alice);
        assert!(at_alice.contains(&Event::FileCast {
            id: up,
            symbols: 6,
            needed: 6
        }));
        assert!(
            at_alice
                .iter()
                .all(|e| matches!(e, Event::FileCast { id, .. } if *id == up)),
            "{at_alice:?}"
        );
        assert!(alice.cancel_file(up));
        run(&mut [&mut alice, &mut bob], 20.0);
        assert!(alice.is_idle());
    }

    #[test]
    fn the_time_a_broadcast_takes_is_known_beforehand() {
        for (mode, len) in [(Mode::R1, 300), (Mode::F3, 30_000)] {
            let (mut alice, mut bob) = (one_way(1, mode), one_way(2, mode));
            let estimate = alice.file_seconds(len) as f32;
            let up = alice.send_file("photo.jpg", file(len)).unwrap();
            assert_eq!(alice.file_sent_bytes(up), Some(len));
            // from the start until a listener on a clean channel has the file
            let mut cable = Cable::default();
            let (mut time, mut took) = (0.0, None);
            while took.is_none() && time < 2.0 * estimate + 20.0 {
                cable.run(&mut [&mut alice, &mut bob], 0.5);
                time += 0.5;
                if events(&mut bob)
                    .iter()
                    .any(|e| matches!(e, Event::FileReceived { .. }))
                {
                    took = Some(time);
                }
            }
            let took = took.expect("received");
            assert!(
                (estimate / took - 1.0).abs() < 0.1,
                "{mode:?}: {estimate} s expected, took {took} s"
            );
        }
    }

    #[test]
    fn a_file_waiting_to_go_keeps_the_modem_busy() {
        let mut alice = one_way(1, Mode::R1);
        let up = alice.send_file("notes.txt", file(10)).unwrap();
        assert!(!alice.is_idle());
        assert!(alice.cancel_file(up));
        assert!(alice.is_idle());
        // and an offer waits for its answer
        let mut bob = modem(2);
        let offered = bob.send_file("notes.txt", file(10)).unwrap();
        assert!(!bob.is_idle());
        assert!(bob.cancel_file(offered));
        run(&mut [&mut bob], 30.0);
        assert!(bob.is_idle());
    }

    #[test]
    fn a_broadcast_file_takes_at_most_256_pieces() {
        let mut alice = one_way(1, Mode::R1);
        assert_eq!(
            alice.send_file("big", file(256 * 59 + 1)),
            Err(CommandError::FileTooLarge { max: 256 * 59 })
        );
    }

    #[test]
    fn a_file_that_deflates_takes_the_time_of_what_goes_on_the_air() {
        let (mut alice, mut bob) = (modem(1), modem(2));
        let log: Vec<u8> = (0..400)
            .flat_map(|n| format!("{n:05} heard a frame, {} dB\n", n % 13).into_bytes())
            .collect();
        let up = alice.send_file("modem.log", log.clone()).unwrap();
        let sent = alice.file_sent_bytes(up).unwrap();
        assert!(sent * 4 < log.len(), "{sent} of {}", log.len());
        assert_eq!(alice.file_sent_bytes(up + 1), None);
        run(&mut [&mut alice, &mut bob], 8.0);
        let Some(Event::FileOffered { id, size, .. }) = events(&mut bob).pop() else {
            panic!("no offer");
        };
        // the receiver is offered the file, not what goes on the air
        assert_eq!(size as usize, log.len());
        assert!(bob.accept_file(id));
        run(&mut [&mut alice, &mut bob], 60.0);
        assert_eq!(
            events(&mut bob).last(),
            Some(&Event::FileReceived {
                id,
                name: "modem.log".into(),
                bytes: log
            })
        );
        assert_eq!(
            events(&mut alice).last(),
            Some(&Event::FileDelivered { id: up })
        );
    }
}

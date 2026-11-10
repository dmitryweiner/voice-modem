//! The modem as the application sees it: commands and samples in, events and
//! samples out.
//!
//! For now one text message is one frame. Sessions, acknowledgements,
//! fragmentation and presets come with the link and transport layers
//! (`plans/PLAN.md`).

use std::collections::VecDeque;

use crate::SAMPLE_RATE;
use crate::frame::{MAX_PAYLOAD, Mode};
use crate::resample::Resampler;
use crate::rx::{Receiver, RxEvent, RxStats};
use crate::tx::{TxConfig, encode_frame};

/// Silence after every transmitted frame, samples (100 ms).
const FRAME_GAP: usize = 1600;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// Sample rate of the audio device, Hz: 8000 to 192000. The samples of
    /// `push_rx` and `pull_tx` are at this rate; the core converts.
    pub sample_rate: u32,
    pub tx: TxConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_rate: SAMPLE_RATE,
            tx: TxConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    UnsupportedSampleRate(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    SendText(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandError {
    /// The text does not fit into one frame; `max` is the limit in UTF-8 bytes.
    TextTooLong { max: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    MessageReceived {
        text: String,
        stats: RxStats,
    },
    /// A frame was heard but its payload could not be read.
    FrameLost {
        stats: RxStats,
    },
}

pub struct Modem {
    config: Config,
    /// Device rate → core rate and back; None when they are the same.
    rx_resampler: Option<Resampler>,
    tx_resampler: Option<Resampler>,
    receiver: Receiver,
    tx: VecDeque<f32>,
    next_seq: u8,
    events: VecDeque<Event>,
}

impl Modem {
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        let rate = config.sample_rate;
        if !(8000..=192_000).contains(&rate) {
            return Err(ConfigError::UnsupportedSampleRate(rate));
        }
        let converts = rate != SAMPLE_RATE;
        Ok(Self {
            config,
            rx_resampler: converts.then(|| Resampler::new(rate, SAMPLE_RATE)),
            tx_resampler: converts.then(|| Resampler::new(SAMPLE_RATE, rate)),
            receiver: Receiver::new(),
            tx: VecDeque::new(),
            next_seq: 0,
            events: VecDeque::new(),
        })
    }

    pub fn command(&mut self, command: Command) -> Result<(), CommandError> {
        match command {
            Command::SendText(text) => {
                let frame = encode_frame(self.next_seq, Mode::R1, text.as_bytes(), &self.config.tx)
                    .map_err(|_| CommandError::TextTooLong { max: MAX_PAYLOAD })?;
                let mut frame = frame;
                self.next_seq = self.next_seq.wrapping_add(1);
                // The pause also pushes the end of the frame out of the resampler.
                frame.resize(frame.len() + FRAME_GAP, 0.0);
                match &mut self.tx_resampler {
                    Some(resampler) => {
                        let mut converted = Vec::new();
                        resampler.process(&frame, &mut converted);
                        self.tx.extend(converted);
                    }
                    None => self.tx.extend(frame),
                }
                Ok(())
            }
        }
    }

    /// Samples from the microphone.
    pub fn push_rx(&mut self, samples: &[f32]) {
        match &mut self.rx_resampler {
            Some(resampler) => {
                let mut converted = Vec::new();
                resampler.process(samples, &mut converted);
                self.receiver.push(&converted);
            }
            None => self.receiver.push(samples),
        }
        while let Some(event) = self.receiver.poll() {
            self.events.push_back(match event {
                RxEvent::Frame { payload, stats, .. } => Event::MessageReceived {
                    text: String::from_utf8_lossy(&payload).into_owned(),
                    stats,
                },
                RxEvent::Damaged { stats, .. } => Event::FrameLost { stats },
            });
        }
    }

    /// Fills `out` with samples for the speaker: silence when there is nothing to send.
    pub fn pull_tx(&mut self, out: &mut [f32]) {
        for sample in out {
            *sample = self.tx.pop_front().unwrap_or(0.0);
        }
    }

    /// Samples waiting to be transmitted.
    pub fn tx_pending(&self) -> usize {
        self.tx.len()
    }

    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modem() -> Modem {
        Modem::new(Config::default()).unwrap()
    }

    /// Everything `from` has to transmit, in device-sized blocks.
    fn transmit(from: &mut Modem) -> Vec<f32> {
        let mut samples = Vec::new();
        while from.tx_pending() > 0 {
            let mut block = [0.0; 128];
            from.pull_tx(&mut block);
            samples.extend(block);
        }
        samples
    }

    fn texts(modem: &mut Modem) -> Vec<String> {
        std::iter::from_fn(|| modem.poll_event())
            .map(|event| match event {
                Event::MessageReceived { text, .. } => text,
                Event::FrameLost { .. } => panic!("frame lost"),
            })
            .collect()
    }

    #[test]
    fn text_travels_from_one_modem_to_another() {
        let (mut alice, mut bob) = (modem(), modem());
        alice
            .command(Command::SendText("Привет, модем! 👋".into()))
            .unwrap();
        alice.command(Command::SendText("second".into())).unwrap();
        bob.push_rx(&transmit(&mut alice));
        assert_eq!(texts(&mut bob), ["Привет, модем! 👋", "second"]);
        assert_eq!(texts(&mut alice), [] as [&str; 0]);
    }

    #[test]
    fn idle_modem_transmits_silence() {
        let mut modem = modem();
        let mut block = [1.0; 64];
        modem.pull_tx(&mut block);
        assert_eq!(block, [0.0; 64]);
        assert_eq!(modem.tx_pending(), 0);
    }

    #[test]
    fn frames_are_separated_by_a_pause() {
        let mut modem = modem();
        modem.command(Command::SendText("a".into())).unwrap();
        let samples = transmit(&mut modem);
        let frame = crate::tx::frame_len(Mode::R1, 1);
        assert!(samples.len() >= frame + FRAME_GAP);
        assert!(samples[frame..].iter().all(|&x| x == 0.0));
    }

    #[test]
    fn rejects_text_longer_than_a_frame() {
        let mut modem = modem();
        assert_eq!(modem.command(Command::SendText("x".repeat(255))), Ok(()));
        assert_eq!(
            modem.command(Command::SendText("x".repeat(256))),
            Err(CommandError::TextTooLong { max: 255 })
        );
        // counted in bytes, not characters
        assert_eq!(
            modem.command(Command::SendText("я".repeat(128))),
            Err(CommandError::TextTooLong { max: 255 })
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
                ..Config::default()
            })
            .unwrap();
            let mut bob = Modem::new(Config {
                sample_rate: to,
                ..Config::default()
            })
            .unwrap();
            alice.command(Command::SendText("any rate".into())).unwrap();
            alice.command(Command::SendText("twice".into())).unwrap();
            let sent = transmit(&mut alice);
            // 5.06 s of frames and pauses, whatever the rate of the sender
            let seconds = sent.len() as f32 / from as f32;
            assert!((seconds - 5.064).abs() < 0.02, "{from}: {seconds} s");
            // The air between two sound cards: a converter of its own.
            let mut heard = Vec::new();
            Resampler::new(from, to).process(&sent, &mut heard);
            for block in heard.chunks(1000) {
                bob.push_rx(block);
            }
            assert_eq!(texts(&mut bob), ["any rate", "twice"], "{from} → {to}");
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
    fn reports_a_lost_frame() {
        let (mut alice, mut bob) = (modem(), modem());
        alice
            .command(Command::SendText("this one gets damaged on the way".into()))
            .unwrap();
        let mut samples = transmit(&mut alice);
        for (n, x) in samples[20_000..45_000].iter_mut().enumerate() {
            *x = (n as f32 * 0.4).sin() * 0.5;
        }
        bob.push_rx(&samples);
        assert!(matches!(bob.poll_event(), Some(Event::FrameLost { .. })));
        assert_eq!(bob.poll_event(), None);
    }
}

//! The modem as the application sees it: commands and samples in, events and
//! samples out.
//!
//! Stage 1: one text message is one frame. Sessions, acknowledgements,
//! fragmentation and presets come with the link and transport layers
//! (`plans/PLAN.md`).

use std::collections::VecDeque;

use crate::SAMPLE_RATE;
use crate::frame::{MAX_PAYLOAD, Mode};
use crate::rx::{Receiver, RxEvent, RxStats};
use crate::tx::{TxConfig, encode_frame};

/// Silence after every transmitted frame, samples (100 ms).
const FRAME_GAP: usize = 1600;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// Sample rate of the audio device, Hz. Only 16000 until the core gets its resampler.
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
    receiver: Receiver,
    tx: VecDeque<f32>,
    next_seq: u8,
    events: VecDeque<Event>,
}

impl Modem {
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        if config.sample_rate != SAMPLE_RATE {
            return Err(ConfigError::UnsupportedSampleRate(config.sample_rate));
        }
        Ok(Self {
            config,
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
                self.next_seq = self.next_seq.wrapping_add(1);
                self.tx.extend(frame);
                self.tx.extend(std::iter::repeat_n(0.0, FRAME_GAP));
                Ok(())
            }
        }
    }

    /// Samples from the microphone.
    pub fn push_rx(&mut self, samples: &[f32]) {
        self.receiver.push(samples);
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
    fn rejects_other_sample_rates_for_now() {
        let config = Config {
            sample_rate: 48_000,
            ..Config::default()
        };
        assert_eq!(
            Modem::new(config).err(),
            Some(ConfigError::UnsupportedSampleRate(48_000))
        );
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

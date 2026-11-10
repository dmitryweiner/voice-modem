//! Browser binding of the core. A thin adapter: logic and its tests live in
//! `modem-core`; this crate only converts types at the JS boundary.

use modem_core::{Command, CommandError, Config, ConfigError, Event};
use wasm_bindgen::prelude::*;

/// Version of the core.
#[wasm_bindgen]
pub fn version() -> String {
    modem_core::VERSION.to_owned()
}

/// The longest text of one message, in UTF-8 bytes.
#[wasm_bindgen(js_name = maxTextBytes)]
pub fn max_text_bytes() -> usize {
    modem_core::MAX_PAYLOAD
}

/// Something the modem has to tell the application.
#[wasm_bindgen(getter_with_clone)]
pub struct ModemEvent {
    /// "message": a text was received; "frameLost": a frame was heard but could not be read.
    pub kind: String,
    /// The text of a message, empty otherwise.
    pub text: String,
    /// Signal-to-noise ratio of the frame, dB, noise over the band up to 8 kHz.
    #[wasm_bindgen(js_name = snrDb)]
    pub snr_db: f32,
    /// Frequency offset of the frame, Hz.
    #[wasm_bindgen(js_name = freqOffsetHz)]
    pub freq_offset_hz: f32,
}

impl From<Event> for ModemEvent {
    fn from(event: Event) -> Self {
        let (kind, text, stats) = match event {
            Event::MessageReceived { text, stats } => ("message", text, stats),
            Event::FrameLost { stats } => ("frameLost", String::new(), stats),
        };
        Self {
            kind: kind.to_owned(),
            text,
            snr_db: stats.snr_db,
            freq_offset_hz: stats.freq_offset_hz,
        }
    }
}

fn new_modem(sample_rate: u32) -> Result<modem_core::Modem, String> {
    modem_core::Modem::new(Config {
        sample_rate,
        ..Config::default()
    })
    .map_err(|ConfigError::UnsupportedSampleRate(rate)| {
        format!("unsupported sample rate: {rate} Hz")
    })
}

fn send_text(modem: &mut modem_core::Modem, text: String) -> Result<(), String> {
    modem
        .command(Command::SendText(text))
        .map_err(|CommandError::TextTooLong { max }| format!("the text is longer than {max} bytes"))
}

#[wasm_bindgen]
pub struct Modem(modem_core::Modem);

#[wasm_bindgen]
impl Modem {
    /// A modem for an audio device running at `sample_rate` Hz.
    #[wasm_bindgen(constructor)]
    pub fn new(sample_rate: u32) -> Result<Modem, JsError> {
        new_modem(sample_rate)
            .map(Modem)
            .map_err(|message| JsError::new(&message))
    }

    /// Queues a text for transmission; fails when it does not fit into one frame.
    #[wasm_bindgen(js_name = sendText)]
    pub fn send_text(&mut self, text: String) -> Result<(), JsError> {
        send_text(&mut self.0, text).map_err(|message| JsError::new(&message))
    }

    /// Samples from the microphone.
    #[wasm_bindgen(js_name = pushRx)]
    pub fn push_rx(&mut self, samples: &[f32]) {
        self.0.push_rx(samples);
    }

    /// The next `len` samples for the speaker: silence when there is nothing to send.
    #[wasm_bindgen(js_name = pullTx)]
    pub fn pull_tx(&mut self, len: usize) -> Vec<f32> {
        let mut samples = vec![0.0; len];
        self.0.pull_tx(&mut samples);
        samples
    }

    /// Samples waiting to be transmitted.
    #[wasm_bindgen(js_name = txPending)]
    pub fn tx_pending(&self) -> usize {
        self.0.tx_pending()
    }

    #[wasm_bindgen(js_name = pollEvent)]
    pub fn poll_event(&mut self) -> Option<ModemEvent> {
        self.0.poll_event().map(ModemEvent::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_the_core_version() {
        assert_eq!(version(), modem_core::VERSION);
        assert_eq!(max_text_bytes(), 255);
    }

    #[test]
    fn errors_are_put_into_words() {
        assert_eq!(
            new_modem(100).err(),
            Some("unsupported sample rate: 100 Hz".to_string())
        );
        let mut modem = new_modem(48_000).unwrap();
        assert_eq!(
            send_text(&mut modem, "x".repeat(256)),
            Err("the text is longer than 255 bytes".to_string())
        );
        assert_eq!(send_text(&mut modem, "fits".into()), Ok(()));
    }

    #[test]
    fn a_message_goes_from_modem_to_modem() {
        let (mut alice, mut bob) = (
            Modem(new_modem(48_000).unwrap()),
            Modem(new_modem(48_000).unwrap()),
        );
        send_text(&mut alice.0, "через wasm".into()).unwrap();
        let pending = alice.tx_pending();
        assert!(pending > 0);
        bob.push_rx(&alice.pull_tx(pending));
        assert_eq!(alice.tx_pending(), 0);
        let event = bob.poll_event().unwrap();
        assert_eq!(
            (event.kind.as_str(), event.text.as_str()),
            ("message", "через wasm")
        );
        assert!(event.snr_db > 20.0 && event.freq_offset_hz.abs() < 1.0);
        assert!(bob.poll_event().is_none());
    }
}

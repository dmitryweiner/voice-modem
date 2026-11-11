//! Browser binding of the core. A thin adapter: logic and its tests live in
//! `modem-core`; this crate only converts types at the JS boundary.

use modem_core::{CommandError, Config, ConfigError, Event, Mode, RxStats};
use wasm_bindgen::prelude::*;

/// Version of the core.
#[wasm_bindgen]
pub fn version() -> String {
    modem_core::VERSION.to_owned()
}

/// The longest text of one message, in UTF-8 bytes.
#[wasm_bindgen(js_name = maxTextBytes)]
pub fn max_text_bytes() -> usize {
    modem_core::MAX_TEXT_BYTES
}

/// The longest station name, in UTF-8 bytes; a longer one is cut.
#[wasm_bindgen(js_name = maxNameBytes)]
pub fn max_name_bytes() -> usize {
    modem_core::MAX_NAME_BYTES
}

/// Something the modem has to tell the application. Fields that do not
/// belong to the kind of the event are empty or zero.
#[wasm_bindgen(getter_with_clone)]
#[derive(Default)]
pub struct ModemEvent {
    /// "message": a text was received.
    /// "hello": a station greeted; `text` is its name.
    /// "sent", "delivered", "failed": the fate of the message `id`.
    /// "retry": no acknowledgement, the message `id` goes out for the `attempt`-th time.
    /// "frameLost": something was transmitted nearby but could not be read.
    /// "txRequest": push-to-talk — press the button and call `pttPressed`.
    /// "txMode": data frames go out in the mode number `mode` from now on.
    pub kind: String,
    pub text: String,
    /// The id `sendText` returned.
    pub id: u32,
    pub attempt: u8,
    /// 0 is the slowest mode, R1.
    pub mode: u8,
    /// Bits of payload per second in that mode.
    #[wasm_bindgen(js_name = bitRate)]
    pub bit_rate: f32,
    /// The address of the station a message or a greeting came from.
    pub station: u16,
    /// Signal-to-noise ratio of the received frame, dB, noise over the band up to 8 kHz.
    #[wasm_bindgen(js_name = snrDb)]
    pub snr_db: f32,
    /// Frequency offset of the received frame, Hz.
    #[wasm_bindgen(js_name = freqOffsetHz)]
    pub freq_offset_hz: f32,
}

impl ModemEvent {
    fn of(kind: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            ..Self::default()
        }
    }

    fn heard(kind: &str, stats: RxStats) -> Self {
        Self {
            snr_db: stats.snr_db,
            freq_offset_hz: stats.freq_offset_hz,
            ..Self::of(kind)
        }
    }
}

impl From<Event> for ModemEvent {
    fn from(event: Event) -> Self {
        match event {
            Event::MessageReceived { from, text, stats } => Self {
                text,
                station: from,
                ..Self::heard("message", stats)
            },
            Event::PeerHello { from, name, stats } => Self {
                text: name,
                station: from,
                ..Self::heard("hello", stats)
            },
            Event::MessageSent { id } => Self {
                id,
                ..Self::of("sent")
            },
            Event::MessageDelivered { id } => Self {
                id,
                ..Self::of("delivered")
            },
            Event::MessageFailed { id } => Self {
                id,
                ..Self::of("failed")
            },
            Event::Retry { id, attempt } => Self {
                id,
                attempt,
                ..Self::of("retry")
            },
            Event::FrameLost { stats } => Self::heard("frameLost", stats),
            Event::TxRequest => Self::of("txRequest"),
            Event::TxMode { mode } => Self {
                mode: mode.id(),
                bit_rate: mode.bit_rate(),
                ..Self::of("txMode")
            },
        }
    }
}

/// `mode`: the number of the mode of every data frame, or anything else (255) to adapt.
fn new_modem(
    sample_rate: u32,
    station: u16,
    name: String,
    manual_ptt: bool,
    seed: u32,
    mode: u8,
) -> Result<modem_core::Modem, String> {
    let config = Config {
        sample_rate,
        station,
        name,
        manual_ptt,
        seed: u64::from(seed),
        mode: Mode::from_id(mode),
        ..Config::default()
    };
    modem_core::Modem::new(config).map_err(|ConfigError::UnsupportedSampleRate(rate)| {
        format!("unsupported sample rate: {rate} Hz")
    })
}

fn send_text(modem: &mut modem_core::Modem, text: &str) -> Result<u32, String> {
    modem.send_text(text).map_err(|error| match error {
        CommandError::EmptyText => "the text is empty".to_owned(),
        CommandError::TextTooLong { max } => format!("the text is longer than {max} bytes"),
    })
}

#[wasm_bindgen]
pub struct Modem(modem_core::Modem);

#[wasm_bindgen]
impl Modem {
    /// A modem for an audio device running at `sample_rate` Hz. `station` is
    /// the address of this station and `seed` feeds its random waits: both
    /// random numbers. With `manual_ptt` every transmission waits for `pttPressed`.
    /// `mode` is the number of a mode to use for every data frame (0 = R1 … 3 =
    /// R4), or 255 to adapt to the channel.
    #[wasm_bindgen(constructor)]
    pub fn new(
        sample_rate: u32,
        station: u16,
        name: String,
        manual_ptt: bool,
        seed: u32,
        mode: u8,
    ) -> Result<Modem, JsError> {
        new_modem(sample_rate, station, name, manual_ptt, seed, mode)
            .map(Modem)
            .map_err(|message| JsError::new(&message))
    }

    /// Queues a text; its fate is reported in events under the returned id.
    #[wasm_bindgen(js_name = sendText)]
    pub fn send_text(&mut self, text: &str) -> Result<u32, JsError> {
        send_text(&mut self.0, text).map_err(|message| JsError::new(&message))
    }

    /// Queues a greeting with the name of this station.
    pub fn hello(&mut self) {
        self.0.hello();
    }

    /// Push-to-talk: the button is down, transmit what is waiting.
    #[wasm_bindgen(js_name = pttPressed)]
    pub fn ptt_pressed(&mut self) {
        self.0.ptt_pressed();
    }

    /// Samples from the microphone. Keep them coming: the modem's clock is their count.
    #[wasm_bindgen(js_name = pushRx)]
    pub fn push_rx(&mut self, samples: &[f32]) {
        self.0.push_rx(samples);
    }

    /// The next `len` samples for the speaker, to be played at once.
    #[wasm_bindgen(js_name = pullTx)]
    pub fn pull_tx(&mut self, len: usize) -> Vec<f32> {
        let mut samples = vec![0.0; len];
        self.0.pull_tx(&mut samples);
        samples
    }

    /// Samples waiting to be played.
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

    fn modem(station: u16) -> Modem {
        Modem(
            new_modem(
                48_000,
                station,
                format!("station {station}"),
                false,
                u32::from(station),
                255,
            )
            .unwrap(),
        )
    }

    fn kinds(modem: &mut Modem) -> Vec<(String, String, u32)> {
        std::iter::from_fn(|| modem.poll_event())
            .map(|e| (e.kind, e.text, e.id))
            .collect()
    }

    /// Each hears what the other plays, a block later.
    fn run(a: &mut Modem, b: &mut Modem, seconds: usize) {
        let (mut to_a, mut to_b) = (vec![0.0; 2048], vec![0.0; 2048]);
        for _ in 0..seconds * 48_000 / 2048 {
            a.push_rx(&to_a);
            b.push_rx(&to_b);
            to_b = a.pull_tx(2048);
            to_a = b.pull_tx(2048);
        }
    }

    #[test]
    fn version_and_limits_are_those_of_the_core() {
        assert_eq!(version(), modem_core::VERSION);
        assert_eq!((max_text_bytes(), max_name_bytes()), (1000, 32));
    }

    #[test]
    fn errors_are_put_into_words() {
        assert_eq!(
            new_modem(100, 1, String::new(), false, 1, 255).err(),
            Some("unsupported sample rate: 100 Hz".to_string())
        );
        let mut modem = modem(1);
        assert_eq!(
            send_text(&mut modem.0, &"x".repeat(1001)),
            Err("the text is longer than 1000 bytes".to_string())
        );
        assert_eq!(
            send_text(&mut modem.0, ""),
            Err("the text is empty".to_string())
        );
        assert_eq!(send_text(&mut modem.0, "fits"), Ok(1));
    }

    #[test]
    fn a_message_is_delivered_and_both_sides_are_told() {
        let (mut alice, mut bob) = (modem(1), modem(2));
        let id = send_text(&mut alice.0, "через wasm").unwrap();
        run(&mut alice, &mut bob, 8);
        let event = bob.poll_event().unwrap();
        assert_eq!(
            (event.kind.as_str(), event.text.as_str(), event.station),
            ("message", "через wasm", 1)
        );
        assert!(event.snr_db > 20.0 && event.freq_offset_hz.abs() < 1.0);
        assert!(bob.poll_event().is_none());
        let mut at_alice = std::iter::from_fn(|| alice.poll_event());
        let delivered = at_alice.next().unwrap();
        assert_eq!((delivered.kind.as_str(), delivered.id), ("delivered", id));
        // over a cable the other side advises the fastest mode: one step up for now
        let faster = at_alice.next().unwrap();
        assert_eq!(
            (faster.kind.as_str(), faster.mode, faster.bit_rate),
            ("txMode", 1, 125.0)
        );
        assert!(at_alice.next().is_none());
    }

    #[test]
    fn a_fixed_mode_is_set_by_its_number() {
        let mut alice = Modem(new_modem(48_000, 1, String::new(), false, 1, 3).unwrap());
        let mut bob = modem(2);
        send_text(&mut alice.0, "at 500 bit/s from the first frame").unwrap();
        run(&mut alice, &mut bob, 5);
        assert_eq!(
            bob.poll_event().unwrap().text,
            "at 500 bit/s from the first frame"
        );
        assert_eq!(
            kinds(&mut alice),
            [("delivered".to_string(), String::new(), 1)]
        );
    }

    #[test]
    fn greetings_and_push_to_talk_reach_the_application() {
        let mut alice = Modem(new_modem(48_000, 1, "Alice".into(), true, 1, 255).unwrap());
        let mut bob = modem(2);
        alice.hello();
        run(&mut alice, &mut bob, 1);
        assert_eq!(
            kinds(&mut alice),
            [("txRequest".to_string(), String::new(), 0)]
        );
        alice.ptt_pressed();
        run(&mut alice, &mut bob, 6);
        assert_eq!(
            kinds(&mut bob),
            [("hello".to_string(), "Alice".to_string(), 0)]
        );
    }
}

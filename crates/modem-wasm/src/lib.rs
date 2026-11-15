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
    /// "message": a text was received; `sealed` when it came sealed.
    /// "unopened": a sealed message came from `station` that the password of
    /// this station does not open.
    /// "fileOffer": a station offers the file `id` of `size` bytes; `text` is its name.
    /// "fileProgress": `done` bytes of the file `id` are through; `secondsLeft` to go.
    /// "fileReceived": the file `id` arrived whole: `bytes`; `text` is its name.
    /// "fileDelivered", "fileFailed", "fileRefused": the fate of the file `id`
    /// (refused: the other side declined it or gave the transfer up).
    /// "fileHeard": the station `station` broadcasts the file `id` of `size`
    /// bytes, named `text`: it is being collected.
    /// "fileCast": `symbols` symbols of the file `id` being broadcast went;
    /// `needed` make the file. "fileSent": that broadcast ended by itself.
    /// "hello": a station greeted; `text` is its name.
    /// "sent", "delivered", "failed": the fate of the message `id`.
    /// "retry": no acknowledgement, the message `id` goes out for the `attempt`-th time.
    /// "frameLost": something was transmitted nearby but could not be read.
    /// "txRequest": push-to-talk — press the button and call `pttPressed`.
    /// "txMode": data frames go out in the mode number `mode` from now on.
    pub kind: String,
    pub text: String,
    /// The id `sendText` returned; for the file events, the id `sendFile`
    /// returned or the one of the offer. Texts and files are numbered apart.
    pub id: u32,
    /// The content of a received file.
    pub bytes: Vec<u8>,
    /// The length of an offered file, bytes.
    pub size: u32,
    /// Bytes of the file that are through.
    pub done: u32,
    /// Seconds the rest of the file takes at the pace of the last chunks; 0 when unknown.
    #[wasm_bindgen(js_name = secondsLeft)]
    pub seconds_left: u32,
    /// Symbols of a broadcast file that went, and how many make the file.
    pub symbols: u32,
    pub needed: u32,
    /// The message came sealed and opened with the password.
    pub sealed: bool,
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
            Event::MessageReceived {
                from,
                text,
                stats,
                sealed,
            } => Self {
                text,
                station: from,
                sealed,
                ..Self::heard("message", stats)
            },
            Event::Unopened { from, stats } => Self {
                station: from,
                ..Self::heard("unopened", stats)
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
            Event::FileOffered {
                id,
                from,
                name,
                size,
            } => Self {
                id,
                station: from,
                text: name,
                size,
                ..Self::of("fileOffer")
            },
            Event::FileProgress {
                id,
                done,
                seconds_left,
            } => Self {
                id,
                done,
                seconds_left: seconds_left.unwrap_or(0),
                ..Self::of("fileProgress")
            },
            Event::FileReceived { id, name, bytes } => Self {
                id,
                text: name,
                bytes,
                ..Self::of("fileReceived")
            },
            Event::FileDelivered { id } => Self {
                id,
                ..Self::of("fileDelivered")
            },
            Event::FileFailed { id } => Self {
                id,
                ..Self::of("fileFailed")
            },
            Event::FileRefused { id } => Self {
                id,
                ..Self::of("fileRefused")
            },
            Event::FileHeard {
                id,
                from,
                name,
                size,
            } => Self {
                id,
                station: from,
                text: name,
                size,
                ..Self::of("fileHeard")
            },
            Event::FileCast {
                id,
                symbols,
                needed,
            } => Self {
                id,
                symbols,
                needed,
                ..Self::of("fileCast")
            },
            Event::FileSent { id } => Self {
                id,
                ..Self::of("fileSent")
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

/// `mode`: the number of the mode of every data frame, or anything else (255)
/// to adapt. `entropy`: up to eight random bytes for the nonces of the seal.
#[allow(clippy::too_many_arguments)] // the constructor of JavaScript takes them as they are
fn new_modem(
    sample_rate: u32,
    station: u16,
    name: String,
    manual_ptt: bool,
    one_way: bool,
    seed: u32,
    mode: u8,
    password: String,
    entropy: &[u8],
) -> Result<modem_core::Modem, String> {
    let entropy = entropy
        .iter()
        .take(8)
        .fold(0u64, |value, &byte| (value << 8) | u64::from(byte));
    let config = Config {
        sample_rate,
        station,
        name,
        manual_ptt,
        reliable: !one_way,
        seed: u64::from(seed),
        mode: Mode::from_id(mode),
        password,
        entropy,
        ..Config::default()
    };
    modem_core::Modem::new(config).map_err(|ConfigError::UnsupportedSampleRate(rate)| {
        format!("unsupported sample rate: {rate} Hz")
    })
}

fn words(error: CommandError) -> String {
    match error {
        CommandError::EmptyText => "the text is empty".to_owned(),
        CommandError::TextTooLong { max } => format!("the text is longer than {max} bytes"),
        CommandError::FileTooLarge { max } => format!("the file is larger than {max} bytes"),
    }
}

fn send_text(modem: &mut modem_core::Modem, text: &str) -> Result<u32, String> {
    modem.send_text(text).map_err(words)
}

fn send_file(modem: &mut modem_core::Modem, name: &str, bytes: Vec<u8>) -> Result<u32, String> {
    modem.send_file(name, bytes).map_err(words)
}

#[wasm_bindgen]
pub struct Modem(modem_core::Modem);

#[wasm_bindgen]
impl Modem {
    /// A modem for an audio device running at `sample_rate` Hz. `station` is
    /// the address of this station and `seed` feeds its random waits: both
    /// random numbers. With `manual_ptt` every transmission waits for `pttPressed`.
    /// With `one_way` nothing is acknowledged or expected back: texts go
    /// once, files are broadcast. `mode` is the number of a mode to use for
    /// every data frame (0 = R1 … 3 = R4, 4 = F1 … 6 = F3), or 255 to adapt
    /// to the channel (one way: R1). With a `password` texts and files go
    /// sealed; `entropy` is eight random bytes for that. The key takes a
    /// fraction of a second to derive.
    #[wasm_bindgen(constructor)]
    #[allow(clippy::too_many_arguments)] // as JavaScript passes them
    pub fn new(
        sample_rate: u32,
        station: u16,
        name: String,
        manual_ptt: bool,
        one_way: bool,
        seed: u32,
        mode: u8,
        password: String,
        entropy: &[u8],
    ) -> Result<Modem, JsError> {
        new_modem(
            sample_rate,
            station,
            name,
            manual_ptt,
            one_way,
            seed,
            mode,
            password,
            entropy,
        )
        .map(Modem)
        .map_err(|message| JsError::new(&message))
    }

    /// Queues a text; its fate is reported in events under the returned id.
    #[wasm_bindgen(js_name = sendText)]
    pub fn send_text(&mut self, text: &str) -> Result<u32, JsError> {
        send_text(&mut self.0, text).map_err(|message| JsError::new(&message))
    }

    /// Takes back the text `id` if it is not delivered yet; false when it
    /// is no longer waiting.
    pub fn cancel(&mut self, id: u32) -> bool {
        self.0.cancel(id)
    }

    /// Offers a file to the other side; its fate is reported in the file
    /// events under the returned id. Nothing of the content is sent before
    /// the offer is accepted there.
    #[wasm_bindgen(js_name = sendFile)]
    pub fn send_file(&mut self, name: &str, bytes: Vec<u8>) -> Result<u32, JsError> {
        send_file(&mut self.0, name, bytes).map_err(|message| JsError::new(&message))
    }

    /// Consents to the offered file `id`; false when there is no such offer.
    #[wasm_bindgen(js_name = acceptFile)]
    pub fn accept_file(&mut self, id: u32) -> bool {
        self.0.accept_file(id)
    }

    /// Gives up the file `id`, sent or received, or declines its offer;
    /// false when there is no such file any more.
    #[wasm_bindgen(js_name = cancelFile)]
    pub fn cancel_file(&mut self, id: u32) -> bool {
        self.0.cancel_file(id)
    }

    /// Bytes the file `id`, offered and not yet sent, goes on the air as:
    /// fewer than it has when it deflates; undefined for any other id.
    #[wasm_bindgen(js_name = fileSentBytes)]
    pub fn file_sent_bytes(&self, id: u32) -> Option<u32> {
        self.0.file_sent_bytes(id).map(|bytes| bytes as u32)
    }

    /// Seconds a file of `size` bytes takes at the speed of the moment, once
    /// it is accepted and if no frame is lost.
    #[wasm_bindgen(js_name = fileSeconds)]
    pub fn file_seconds(&self, size: u32) -> u32 {
        self.0.file_seconds(size as usize)
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
                false,
                u32::from(station),
                255,
                String::new(),
                &[],
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
            new_modem(
                100,
                1,
                String::new(),
                false,
                false,
                1,
                255,
                String::new(),
                &[]
            )
            .err(),
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
        let mut alice = Modem(
            new_modem(
                48_000,
                1,
                String::new(),
                false,
                false,
                1,
                3,
                String::new(),
                &[],
            )
            .unwrap(),
        );
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
    fn the_fast_modes_have_numbers_too() {
        let mut alice = Modem(
            new_modem(
                48_000,
                1,
                String::new(),
                false,
                false,
                1,
                6,
                String::new(),
                &[],
            )
            .unwrap(),
        );
        let mut bob = modem(2);
        let long = "at 4200 bit/s from the first frame. ".repeat(25);
        send_text(&mut alice.0, &long).unwrap();
        run(&mut alice, &mut bob, 6);
        assert_eq!(bob.poll_event().unwrap().text, long);
        assert_eq!(
            kinds(&mut alice),
            [("delivered".to_string(), String::new(), 1)]
        );
    }

    #[test]
    fn greetings_and_push_to_talk_reach_the_application() {
        let mut alice = Modem(
            new_modem(
                48_000,
                1,
                "Alice".into(),
                true,
                false,
                1,
                255,
                String::new(),
                &[],
            )
            .unwrap(),
        );
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

    #[test]
    fn a_file_is_offered_accepted_and_received() {
        let (mut alice, mut bob) = (modem(1), modem(2));
        let content: Vec<u8> = (0..=255).collect();
        // ten full frames of the slowest mode
        assert_eq!(alice.file_seconds(570), 125);
        let up = send_file(&mut alice.0, "байты.bin", content.clone()).unwrap();
        run(&mut alice, &mut bob, 8);
        let offer = bob.poll_event().unwrap();
        assert_eq!(
            (offer.kind.as_str(), offer.text.as_str(), offer.size),
            ("fileOffer", "байты.bin", 256)
        );
        assert_eq!((offer.station, offer.id), (1, 1));
        assert!(bob.accept_file(offer.id));
        assert!(!bob.accept_file(offer.id));
        run(&mut alice, &mut bob, 40);

        let at_bob: Vec<ModemEvent> = std::iter::from_fn(|| bob.poll_event())
            .filter(|e| e.kind != "txMode")
            .collect();
        let progress = &at_bob[0];
        assert_eq!(
            (progress.kind.as_str(), progress.id, progress.done),
            ("fileProgress", 1, 121)
        );
        assert!(progress.seconds_left > 0);
        let received = at_bob.last().unwrap();
        assert_eq!(
            (received.kind.as_str(), received.text.as_str(), received.id),
            ("fileReceived", "байты.bin", 1)
        );
        assert_eq!(received.bytes, content);

        let at_alice: Vec<ModemEvent> = std::iter::from_fn(|| alice.poll_event())
            .filter(|e| e.kind != "txMode")
            .collect();
        // the consent: nothing sent yet, the time left unknown
        assert_eq!(
            (
                at_alice[0].kind.as_str(),
                at_alice[0].done,
                at_alice[0].seconds_left
            ),
            ("fileProgress", 0, 0)
        );
        let delivered = at_alice.last().unwrap();
        assert_eq!(
            (delivered.kind.as_str(), delivered.id),
            ("fileDelivered", up)
        );
    }

    #[test]
    fn a_file_is_declined_or_given_up() {
        let (mut alice, mut bob) = (modem(1), modem(2));
        let up = send_file(&mut alice.0, "a.txt", vec![1, 2, 3]).unwrap();
        run(&mut alice, &mut bob, 8);
        assert!(bob.cancel_file(1));
        run(&mut alice, &mut bob, 8);
        let refused = std::iter::from_fn(|| alice.poll_event()).last().unwrap();
        assert_eq!((refused.kind.as_str(), refused.id), ("fileRefused", up));

        let lonely = send_file(&mut alice.0, "b.txt", vec![1, 2, 3]).unwrap();
        assert_eq!(alice.file_sent_bytes(lonely), Some(3));
        assert!(alice.cancel_file(lonely));
        assert!(!alice.cancel_file(lonely));
    }

    #[test]
    fn a_file_nobody_answers_fails() {
        let mut alone = modem(1);
        let mut nobody = modem(2);
        let up = send_file(&mut alone.0, "a.txt", vec![1, 2, 3]).unwrap();
        for _ in 0..60 * 48_000 / 2048 {
            alone.push_rx(&[0.0; 2048]);
            alone.pull_tx(2048);
        }
        let failed = std::iter::from_fn(|| alone.poll_event()).last().unwrap();
        assert_eq!((failed.kind.as_str(), failed.id), ("fileFailed", up));
        assert!(nobody.poll_event().is_none());
    }

    #[test]
    fn a_one_way_modem_broadcasts_its_files() {
        let one_way = |station| {
            Modem(
                new_modem(
                    48_000,
                    station,
                    String::new(),
                    false,
                    true,
                    1,
                    0,
                    String::new(),
                    &[],
                )
                .unwrap(),
            )
        };
        let (mut alice, mut bob) = (one_way(1), one_way(2));
        let content: Vec<u8> = (0..100u32).map(|i| ((i * 7919) >> 3) as u8).collect();
        let up = send_file(&mut alice.0, "a.bin", content.clone()).unwrap();
        // symbols of 59 bytes in R1: two pieces
        run(&mut alice, &mut bob, 40);
        let at_bob: Vec<ModemEvent> = std::iter::from_fn(|| bob.poll_event()).collect();
        let heard = &at_bob[0];
        assert_eq!(
            (
                heard.kind.as_str(),
                heard.text.as_str(),
                heard.size,
                heard.station
            ),
            ("fileHeard", "a.bin", 100, 1)
        );
        let received = at_bob.last().unwrap();
        assert_eq!(
            (received.kind.as_str(), received.id),
            ("fileReceived", heard.id)
        );
        assert_eq!(received.bytes, content);
        let cast = std::iter::from_fn(|| alice.poll_event())
            .find(|e| e.kind == "fileCast" && e.symbols == 2)
            .unwrap();
        assert_eq!((cast.id, cast.needed), (up, 2));
    }

    #[test]
    fn a_password_seals_what_goes_and_says_what_came_sealed() {
        let sealing = |station, password: &str| {
            Modem(
                new_modem(
                    48_000,
                    station,
                    String::new(),
                    false,
                    false,
                    1,
                    255,
                    password.into(),
                    &[station as u8; 8],
                )
                .unwrap(),
            )
        };
        let (mut alice, mut bob, mut eve) = (sealing(1, "shared"), sealing(2, "shared"), modem(3));
        send_text(&mut alice.0, "meet at the bridge").unwrap();
        // Eve hears what Bob hears
        let (mut to_bob, mut to_alice) = (vec![0.0; 2048], vec![0.0; 2048]);
        for _ in 0..8 * 48_000 / 2048 {
            alice.push_rx(&to_alice);
            bob.push_rx(&to_bob);
            eve.push_rx(&to_bob);
            to_bob = alice.pull_tx(2048);
            to_alice = bob.pull_tx(2048);
            eve.pull_tx(2048);
        }
        let at_bob = bob.poll_event().unwrap();
        assert_eq!(
            (at_bob.kind.as_str(), at_bob.text.as_str(), at_bob.sealed),
            ("message", "meet at the bridge", true)
        );
        let at_eve = eve.poll_event().unwrap();
        assert_eq!(
            (at_eve.kind.as_str(), at_eve.station, at_eve.text.as_str()),
            ("unopened", 1, "")
        );
    }
}

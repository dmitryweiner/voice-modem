//! Receiver: a stream of samples → frames.
//!
//! Searches for the preamble, then demodulates the header and the payload.
//! Fed in pieces of any size, it gives the same frames as when fed at once.
//!
//! Once a header is read the receiver takes the whole announced payload and
//! does not look for preambles inside it: a frame that starts before a
//! cut-off one would have ended is lost.

use std::collections::VecDeque;

use crate::SAMPLE_RATE;
use crate::coding::{block_symbols, decode_block};
use crate::crc::crc32;
use crate::frame::{Frame, HEADER_BYTES, HEADER_MFSK, HEADER_MODE, Header, Mode, Scheme};
use crate::mfsk::{MfskParams, soft_bits, tone_energies};
use crate::ofdm;
use crate::sync::{CHIRP_LEN, HZ_PER_SAMPLE, PREAMBLE_LEN, Reference};

/// The up-chirp is looked for at every `SEARCH_STEP`-th position; its
/// correlation peak is about 16 samples wide.
const SEARCH_STEP: usize = 4;
/// Share of the window energy that must be the chirp to call it found. Noise
/// gives about 0.001 on average and exceeds this once in e¹² positions.
const THRESHOLD: f32 = 0.012;
/// After the threshold is crossed, the true peak is the largest value this far ahead.
const REFINE: usize = 256;
/// A preamble this clear is reported even when the header after it cannot be
/// read: something was transmitted. Well above what the tones of a frame
/// whose beginning was missed can look like (up to about 0.1).
const CLEAR: f32 = 0.25;
/// The largest frequency offset the receiver follows, Hz.
pub const MAX_FREQ_OFFSET_HZ: f32 = 250.0;
/// How far the offset moves each chirp peak, samples.
const MAX_SHIFT: usize = (MAX_FREQ_OFFSET_HZ / HZ_PER_SAMPLE) as usize;
/// CRC-32 after the payload.
const CHECKSUM_BYTES: usize = 4;

/// Length of the header in samples.
pub fn header_len() -> usize {
    block_symbols(HEADER_BYTES, HEADER_MFSK.bits_per_symbol()) * HEADER_MFSK.symbol_len
}

/// Silence between the header and a payload in another mode than the
/// header's, samples (64 ms). In a room the last tones of the header ring on
/// at full level, far above each of the many tones or carriers of such a
/// payload: this lets them die down.
pub const PAYLOAD_PAUSE: usize = 1024;

/// The pause between the header and a payload in this mode, samples.
pub fn payload_pause(mode: Mode) -> usize {
    if mode == HEADER_MODE {
        0
    } else {
        PAYLOAD_PAUSE
    }
}

/// Length in samples of a payload of `bytes` bytes with its checksum.
pub fn payload_len(mode: Mode, bytes: usize) -> usize {
    let bytes = bytes + CHECKSUM_BYTES;
    match mode.scheme() {
        Scheme::Tones(mfsk) => block_symbols(bytes, mfsk.bits_per_symbol()) * mfsk.symbol_len,
        Scheme::Carriers(params) => params.block_symbols(bytes) * ofdm::SYMBOL_LEN,
    }
}

/// What the receiver measured on a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RxStats {
    /// Signal-to-noise ratio in dB, the noise taken over the whole band up to
    /// half the sample rate (8 kHz). An estimate from the tone energies.
    pub snr_db: f32,
    /// How far the signal is shifted in frequency, Hz.
    pub freq_offset_hz: f32,
    /// Position of the first sample of the preamble in the stream.
    pub start: u64,
    /// For a payload in a fast mode: the signal-to-noise ratio on its
    /// carriers, dB, as the pilots show it.
    pub carriers_snr_db: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RxEvent {
    /// A frame with correct checksums.
    Frame {
        frame: Frame,
        mode: Mode,
        stats: RxStats,
    },
    /// The header was read but the payload checksum is wrong.
    Damaged {
        station: u16,
        seq: u8,
        mode: Mode,
        stats: RxStats,
    },
    /// A clear preamble, but the header after it could not be read.
    Unreadable { stats: RxStats },
}

enum State {
    /// Looking for the up-chirp from this position on.
    Search { from: u64 },
    Header {
        start: u64,
        freq_offset_hz: f32,
        resume: u64,
        clear: bool,
    },
    Payload {
        start: u64,
        freq_offset_hz: f32,
        header: Header,
        header_snr: SnrEstimate,
    },
}

/// The signal and the noise seen in the tone filters over the symbols of a block.
#[derive(Clone, Copy, Default)]
struct SnrEstimate {
    /// Sums over the symbols of the power of the signal and of the noise, in
    /// the same units.
    signal: f32,
    noise: f32,
}

impl SnrEstimate {
    /// Takes the tone energies of one symbol of `symbol_len` samples, in
    /// groups of `tones`: the loudest tone of each group is the signal, the
    /// rest is noise.
    ///
    /// A tone of power P gives the energy P·N²/2 in its own filter of N
    /// samples; noise of power σ² gives N·σ² in every filter.
    fn add(&mut self, energies: &[f32], tones: usize, symbol_len: usize) {
        let groups = energies.len() / tones;
        let loudest: f32 = energies
            .chunks_exact(tones)
            .map(|group| group.iter().copied().fold(0.0, f32::max))
            .sum();
        let noise = (energies.iter().sum::<f32>() - loudest) / (energies.len() - groups) as f32;
        let n = symbol_len as f32;
        self.signal += (loudest - groups as f32 * noise) * 2.0 / (n * n);
        self.noise += noise / n;
    }

    fn merged(self, other: SnrEstimate) -> SnrEstimate {
        SnrEstimate {
            signal: self.signal + other.signal,
            noise: self.noise + other.noise,
        }
    }

    fn snr_db(self) -> f32 {
        let ratio = self.signal / self.noise.max(f32::MIN_POSITIVE);
        (10.0 * ratio.max(1e-4).log10()).min(60.0)
    }
}

pub struct Receiver {
    /// Samples not yet done with; `buffer[0]` is sample number `base` of the
    /// stream, counted from `MAX_SHIFT` samples of silence put in front of it:
    /// a frequency offset moves the up-chirp peak of a frame that starts with
    /// the stream to before its first sample.
    buffer: Vec<f32>,
    base: u64,
    state: State,
    up: Reference,
    down: Reference,
    events: VecDeque<RxEvent>,
}

impl Default for Receiver {
    fn default() -> Self {
        Self::new()
    }
}

impl Receiver {
    pub fn new() -> Self {
        Self {
            buffer: vec![0.0; MAX_SHIFT],
            base: 0,
            state: State::Search { from: 0 },
            up: Reference::new(true),
            down: Reference::new(false),
            events: VecDeque::new(),
        }
    }

    /// Takes the next samples of the stream.
    pub fn push(&mut self, samples: &[f32]) {
        self.buffer.extend_from_slice(samples);
        while self.step() {}
        self.forget();
    }

    pub fn poll(&mut self) -> Option<RxEvent> {
        self.events.pop_front()
    }

    /// Forgets the frame being received and every sample so far: the stream
    /// was interrupted. Events not yet polled stay.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.buffer.resize(MAX_SHIFT, 0.0);
        self.base = 0;
        self.state = State::Search { from: 0 };
    }

    /// `len` samples from stream position `start`, once they have all arrived.
    fn samples(&self, start: u64, len: usize) -> Option<&[f32]> {
        let offset = (start - self.base) as usize;
        self.buffer.get(offset..offset + len)
    }

    /// Drops the samples no state will look at again.
    fn forget(&mut self) {
        let keep_from = match self.state {
            State::Search { from } => from,
            State::Header { start, resume, .. } => start.min(resume),
            State::Payload { start, .. } => start,
        };
        let drop = (keep_from.saturating_sub(self.base) as usize).min(self.buffer.len());
        self.buffer.drain(..drop);
        self.base += drop as u64;
    }

    /// True while a frame is being received: its preamble was found and
    /// its end has not come yet.
    pub fn busy(&self) -> bool {
        !matches!(self.state, State::Search { .. })
    }

    /// Advances the state machine; false when it needs more samples.
    fn step(&mut self) -> bool {
        match self.state {
            State::Search { from } => self.search(from),
            State::Header {
                start,
                freq_offset_hz,
                resume,
                clear,
            } => self.header(start, freq_offset_hz, resume, clear),
            State::Payload {
                start,
                freq_offset_hz,
                header,
                header_snr,
            } => self.payload(start, freq_offset_hz, header, header_snr),
        }
    }

    fn search(&mut self, from: u64) -> bool {
        // Everything a detection may look at: the refined up-chirp and the shifted down-chirp.
        let lookahead = REFINE + PREAMBLE_LEN + 2 * MAX_SHIFT;
        let mut position = from;
        loop {
            if self.samples(position, lookahead).is_none() {
                self.state = State::Search { from: position };
                return false;
            }
            if self.metric(&self.up, position) >= THRESHOLD {
                break;
            }
            position += SEARCH_STEP as u64;
        }

        let resume = position + REFINE as u64 + 1;
        let (up_at, up_metric) = self.peak(&self.up, position, position + REFINE as u64);
        // The offset moves the two peaks apart or together, by up to MAX_SHIFT each.
        let expected = up_at + CHIRP_LEN as u64;
        let (down_at, down_metric) = self.peak(
            &self.down,
            expected - 2 * MAX_SHIFT as u64,
            expected + 2 * MAX_SHIFT as u64,
        );
        if down_metric < THRESHOLD {
            self.state = State::Search { from: resume };
            return true;
        }
        let shift = (down_at as i64 - expected as i64) as f32 / 2.0;
        let preamble_at = (up_at as i64 + shift.round() as i64) as u64;
        self.state = State::Header {
            start: preamble_at + PREAMBLE_LEN as u64,
            freq_offset_hz: shift * HZ_PER_SAMPLE,
            resume,
            clear: up_metric >= CLEAR && down_metric >= CLEAR,
        };
        true
    }

    fn metric(&self, reference: &Reference, position: u64) -> f32 {
        self.samples(position, CHIRP_LEN)
            .map_or(0.0, |window| reference.metric(window))
    }

    /// The position in `from..=to` where the chirp matches best, and how well.
    /// Looks at every `SEARCH_STEP`-th position first, then around the best
    /// of them: the tones of a frame whose beginning was missed cross the
    /// threshold all the time, and each crossing costs this search twice.
    fn peak(&self, reference: &Reference, from: u64, to: u64) -> (u64, f32) {
        let from = from.max(self.base);
        let best_of = |positions: &mut dyn Iterator<Item = u64>, mut best: (u64, f32)| {
            for position in positions {
                let metric = self.metric(reference, position);
                if metric > best.1 {
                    best = (position, metric);
                }
            }
            best
        };
        let coarse = best_of(&mut (from..=to).step_by(SEARCH_STEP), (from, 0.0));
        let around = coarse.0.saturating_sub(SEARCH_STEP as u64 - 1).max(from)
            ..=(coarse.0 + SEARCH_STEP as u64 - 1).min(to);
        best_of(&mut around.into_iter(), coarse)
    }

    /// Soft bits of the symbols from `start` on, or None until all of them have arrived.
    fn demodulate(
        &self,
        mfsk: &MfskParams,
        start: u64,
        symbols: usize,
        freq_offset_hz: f32,
    ) -> Option<(Vec<f32>, SnrEstimate)> {
        let samples = self.samples(start, symbols * mfsk.symbol_len)?;
        let mut soft = Vec::with_capacity(symbols * mfsk.bits_per_symbol());
        let mut snr = SnrEstimate::default();
        for (index, symbol) in samples.chunks_exact(mfsk.symbol_len).enumerate() {
            let energies = tone_energies(mfsk, symbol, index, freq_offset_hz);
            soft_bits(mfsk, &energies, &mut soft);
            snr.add(&energies, mfsk.tones(), mfsk.symbol_len);
        }
        Some((soft, snr))
    }

    fn stats(&self, header_start: u64, freq_offset_hz: f32, snr: SnrEstimate) -> RxStats {
        RxStats {
            snr_db: snr.snr_db(),
            freq_offset_hz,
            start: header_start.saturating_sub((PREAMBLE_LEN + MAX_SHIFT) as u64),
            carriers_snr_db: None,
        }
    }

    fn header(&mut self, start: u64, freq_offset_hz: f32, resume: u64, clear: bool) -> bool {
        let mfsk = HEADER_MFSK;
        let symbols = block_symbols(HEADER_BYTES, mfsk.bits_per_symbol());
        let Some((soft, header_snr)) = self.demodulate(&mfsk, start, symbols, freq_offset_hz)
        else {
            return false;
        };
        let stats = self.stats(start, freq_offset_hz, header_snr);
        self.state = match Header::from_bytes(&decode_block(&soft, HEADER_BYTES)) {
            Some(header) if header.kind.has_payload() => State::Payload {
                start,
                freq_offset_hz,
                header,
                header_snr,
            },
            Some(Header {
                kind,
                mode,
                station,
                seq,
                ..
            }) => {
                let frame = Frame {
                    kind,
                    station,
                    seq,
                    payload: Vec::new(),
                };
                self.events.push_back(RxEvent::Frame { frame, mode, stats });
                State::Search {
                    from: start + header_len() as u64,
                }
            }
            None => {
                if clear {
                    self.events.push_back(RxEvent::Unreadable { stats });
                }
                // Not a frame, or not a readable one: go on searching right after the preamble.
                State::Search { from: resume }
            }
        };
        true
    }

    fn payload(
        &mut self,
        header_start: u64,
        freq_offset_hz: f32,
        header: Header,
        header_snr: SnrEstimate,
    ) -> bool {
        let bytes = usize::from(header.len) + CHECKSUM_BYTES;
        let start = header_start + (header_len() + payload_pause(header.mode)) as u64;
        let len = payload_len(header.mode, usize::from(header.len));
        let mut stats = self.stats(header_start, freq_offset_hz, header_snr);
        let mut payload = match header.mode.scheme() {
            Scheme::Tones(mfsk) => {
                let symbols = len / mfsk.symbol_len;
                let Some((soft, payload_snr)) =
                    self.demodulate(&mfsk, start, symbols, freq_offset_hz)
                else {
                    return false;
                };
                // The level is that of a single tone, as in the header: a payload of
                // several tones at once has less power and would lower the estimate.
                if mfsk.groups == 1 {
                    stats.snr_db = header_snr.merged(payload_snr).snr_db();
                }
                decode_block(&soft, bytes)
            }
            Scheme::Carriers(params) => {
                let Some(samples) = self.samples(start, len) else {
                    return false;
                };
                let block = ofdm::demodulate(&params, samples, bytes, freq_offset_hz);
                stats.carriers_snr_db = Some(block.snr_db);
                block.bytes
            }
        };
        let checksum = payload.split_off(usize::from(header.len));
        let Header {
            kind,
            mode,
            station,
            seq,
            ..
        } = header;
        self.events
            .push_back(if checksum == crc32(&payload).to_be_bytes() {
                RxEvent::Frame {
                    frame: Frame {
                        kind,
                        station,
                        seq,
                        payload,
                    },
                    mode,
                    stats,
                }
            } else {
                RxEvent::Damaged {
                    station,
                    seq,
                    mode,
                    stats,
                }
            });
        self.state = State::Search {
            from: start + len as u64,
        };
        true
    }
}

/// Duration of `samples` samples in seconds.
pub fn seconds(samples: usize) -> f32 {
    samples as f32 / SAMPLE_RATE as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Kind;
    use crate::tx::{TxConfig, encode_frame};

    fn data(seq: u8, payload: &[u8]) -> Frame {
        Frame {
            kind: Kind::Data,
            station: 0x1234,
            seq,
            payload: payload.to_vec(),
        }
    }

    fn frame(seq: u8, payload: &[u8]) -> Vec<f32> {
        encode_frame(&data(seq, payload), Mode::R1, &TxConfig::default()).unwrap()
    }

    fn receive(samples: &[f32]) -> Vec<RxEvent> {
        let mut receiver = Receiver::new();
        receiver.push(samples);
        std::iter::from_fn(|| receiver.poll()).collect()
    }

    fn payloads(events: &[RxEvent]) -> Vec<(u8, Vec<u8>)> {
        events
            .iter()
            .map(|event| match event {
                RxEvent::Frame { frame, .. } => (frame.seq, frame.payload.clone()),
                other => panic!("not a good frame: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn receives_a_clean_frame() {
        let events = receive(&frame(7, b"hello, modem"));
        assert_eq!(payloads(&events), [(7, b"hello, modem".to_vec())]);
        let RxEvent::Frame { mode, stats, .. } = &events[0] else {
            unreachable!()
        };
        assert_eq!(*mode, Mode::R1);
        assert_eq!(stats.start, 0);
        assert!(stats.freq_offset_hz.abs() < 1.0, "{}", stats.freq_offset_hz);
        // The fades at the ends of the frame leak into other tones: the estimate tops out here.
        assert!(stats.snr_db > 25.0, "{}", stats.snr_db);
    }

    #[test]
    fn receives_empty_and_full_frames() {
        let full: Vec<u8> = (0..=254).collect();
        assert_eq!(payloads(&receive(&frame(0, &[]))), [(0, vec![])]);
        assert_eq!(payloads(&receive(&frame(63, &full))), [(63, full)]);
    }

    #[test]
    fn receives_frames_in_every_mode() {
        for mode in Mode::ALL {
            for len in [0usize, 40, 255] {
                let payload: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
                let mut samples = vec![0.0; 1234];
                samples
                    .extend(encode_frame(&data(9, &payload), mode, &TxConfig::default()).unwrap());
                samples.extend(frame(10, b"next"));
                let events = receive(&samples);
                assert_eq!(
                    payloads(&events),
                    [(9, payload), (10, b"next".to_vec())],
                    "{mode:?}, {len} bytes"
                );
                let RxEvent::Frame {
                    mode: seen, stats, ..
                } = &events[0]
                else {
                    unreachable!()
                };
                assert_eq!(*seen, mode);
                // only the fast modes have carriers to measure
                assert_eq!(
                    stats.carriers_snr_db.is_some(),
                    mode >= Mode::F1,
                    "{mode:?}"
                );
                assert!(
                    stats.carriers_snr_db.is_none_or(|snr| snr > 25.0),
                    "{stats:?}"
                );
            }
        }
    }

    #[test]
    fn a_fast_frame_carries_a_whole_long_text() {
        let payload: Vec<u8> = (0..1023).map(|i| (i * 13 + 5) as u8).collect();
        for mode in [Mode::F1, Mode::F3] {
            let samples = encode_frame(&data(1, &payload), mode, &TxConfig::default()).unwrap();
            assert_eq!(
                payloads(&receive(&samples)),
                [(1, payload.clone())],
                "{mode:?}"
            );
        }
    }

    #[test]
    fn finds_the_frame_anywhere_in_the_stream() {
        for lead in [1usize, 3, 517, 4000, 20_001] {
            let mut samples = vec![0.0; lead];
            samples.extend(frame(1, b"offset"));
            samples.extend(vec![0.0; 300]);
            let events = receive(&samples);
            assert_eq!(payloads(&events), [(1, b"offset".to_vec())], "lead {lead}");
            let RxEvent::Frame { stats, .. } = &events[0] else {
                unreachable!()
            };
            assert_eq!(stats.start, lead as u64, "lead {lead}");
        }
    }

    #[test]
    fn receives_frames_one_after_another() {
        let mut samples = Vec::new();
        for (seq, text) in [b"first".as_slice(), b"second", b"third"]
            .iter()
            .enumerate()
        {
            samples.extend(frame(seq as u8, text));
            samples.extend(vec![0.0; 100 * seq]);
        }
        assert_eq!(
            payloads(&receive(&samples)),
            [
                (0, b"first".to_vec()),
                (1, b"second".to_vec()),
                (2, b"third".to_vec())
            ]
        );
    }

    #[test]
    fn pieces_of_any_size_give_the_same_result() {
        let mut samples = vec![0.0; 777];
        samples.extend(frame(1, b"in pieces"));
        samples.extend(frame(2, b"again"));
        let whole = receive(&samples);
        assert_eq!(whole.len(), 2);
        for size in [1usize, 128, 1000, 4801] {
            let mut receiver = Receiver::new();
            let mut events = Vec::new();
            for piece in samples.chunks(size) {
                receiver.push(piece);
                events.extend(std::iter::from_fn(|| receiver.poll()));
            }
            assert_eq!(events, whole, "pieces of {size}");
        }
    }

    #[test]
    fn does_not_keep_old_samples() {
        let mut receiver = Receiver::new();
        for _ in 0..100 {
            receiver.push(&[0.0; 1600]);
        }
        receiver.push(&frame(0, b"x"));
        assert!(
            receiver.buffer.len() < 2 * PREAMBLE_LEN,
            "{}",
            receiver.buffer.len()
        );
        assert!(receiver.poll().is_some());
    }

    #[test]
    fn the_level_does_not_matter() {
        for gain in [0.001, 0.1, 1.9] {
            let samples: Vec<f32> = frame(4, b"level").iter().map(|x| x * gain).collect();
            assert_eq!(
                payloads(&receive(&samples)),
                [(4, b"level".to_vec())],
                "gain {gain}"
            );
        }
    }

    #[test]
    fn a_lead_in_tone_is_skipped() {
        let config = TxConfig {
            lead_in_ms: 400,
            ..TxConfig::default()
        };
        let samples = encode_frame(&data(9, b"after the tone"), Mode::R1, &config).unwrap();
        let events = receive(&samples);
        assert_eq!(payloads(&events), [(9, b"after the tone".to_vec())]);
        let RxEvent::Frame { stats, .. } = &events[0] else {
            unreachable!()
        };
        assert_eq!(stats.start, 6400);
    }

    #[test]
    fn reports_a_damaged_payload() {
        let mut samples = frame(5, &[0x55; 60]);
        let payload_at = PREAMBLE_LEN + header_len();
        // Silence is only erasures, one steady tone meets only every fourth
        // symbol, and the code fills in even 3 s of noise: the whole payload
        // has to go.
        let mut seed = 1u32;
        for x in &mut samples[payload_at..] {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *x = (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
        }
        let events = receive(&samples);
        assert!(
            matches!(
                events[..],
                [RxEvent::Damaged {
                    seq: 5,
                    mode: Mode::R1,
                    ..
                }]
            ),
            "{events:?}"
        );
    }

    #[test]
    fn a_truncated_frame_gives_nothing_and_the_next_one_is_received() {
        let mut samples = frame(1, b"cut off at the start of the header");
        samples.truncate(PREAMBLE_LEN + 4 * 512);
        samples.extend(vec![0.0; 3000]);
        samples.extend(frame(2, b"whole"));
        let events = receive(&samples);
        let whole: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, RxEvent::Frame { .. }))
            .cloned()
            .collect();
        assert_eq!(payloads(&whole), [(2, b"whole".to_vec())], "{events:?}");
        // the cut-off one had a clear preamble: it is reported as heard
        assert!(
            matches!(events[0], RxEvent::Unreadable { stats } if stats.start == 0),
            "{events:?}"
        );
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn receives_an_acknowledgement_which_is_a_header_alone() {
        let ack = Frame {
            kind: Kind::Ack { advice: Mode::R3 },
            station: 0xABCD,
            seq: 37,
            payload: vec![],
        };
        let mut samples = vec![0.0; 900];
        samples.extend(encode_frame(&ack, Mode::R1, &TxConfig::default()).unwrap());
        // right after it, with no pause, a data frame
        samples.extend(frame(1, b"next"));
        let events = receive(&samples);
        assert!(
            matches!(&events[0], RxEvent::Frame { frame, .. } if *frame == ack),
            "{events:?}"
        );
        assert_eq!(payloads(&events[1..]), [(1, b"next".to_vec())]);
    }

    #[test]
    fn keeps_the_kind_and_the_station_of_a_frame() {
        let sent = Frame {
            kind: Kind::Broadcast,
            station: 0xFFFF,
            seq: 0,
            payload: b"to all".to_vec(),
        };
        let events = receive(&encode_frame(&sent, Mode::R1, &TxConfig::default()).unwrap());
        assert!(
            matches!(&events[..], [RxEvent::Frame { frame, .. }] if *frame == sent),
            "{events:?}"
        );
    }

    #[test]
    fn is_busy_from_the_preamble_to_the_end_of_the_frame() {
        let samples = frame(1, b"busy while it lasts");
        let mut receiver = Receiver::new();
        assert!(!receiver.busy());
        receiver.push(&samples[..PREAMBLE_LEN]);
        assert!(
            !receiver.busy(),
            "a preamble is found only when all of it has arrived"
        );
        receiver.push(&samples[PREAMBLE_LEN..PREAMBLE_LEN + 2000]);
        assert!(receiver.busy());
        receiver.push(&samples[PREAMBLE_LEN + 2000..samples.len() - 1]);
        assert!(receiver.busy());
        receiver.push(&samples[samples.len() - 1..]);
        assert!(!receiver.busy());
        assert!(receiver.poll().is_some());
    }

    #[test]
    fn silence_and_tones_give_nothing() {
        assert_eq!(receive(&vec![0.0; 50_000]), []);
        let tone: Vec<f32> = (0..50_000).map(|n| (n as f32 * 0.5).sin() * 0.5).collect();
        assert_eq!(receive(&tone), []);
    }

    #[test]
    fn a_shifted_frame_at_the_very_start_of_the_stream() {
        // Every tone and the chirps 40 Hz up: the up-chirp peak is 41 samples before the frame.
        let mut samples = Vec::new();
        crate::sync::preamble_shifted(0.5, 40.0, &mut samples);
        let shifted = MfskParams {
            first_tone_hz: 790.0,
            ..HEADER_MFSK
        };
        let header = Header {
            kind: Kind::Data,
            mode: Mode::R1,
            station: 9,
            seq: 3,
            len: 2,
        };
        let mut block = b"hi".to_vec();
        block.extend(crc32(b"hi").to_be_bytes());
        // the header and the payload are blocks of their own: each starts with the first set of tones
        for bytes in [&header.to_bytes()[..], &block] {
            let mut modulator = crate::mfsk::Modulator::new(shifted, 0.5);
            for value in crate::coding::encode_block(bytes, 4) {
                modulator.symbol(value, &mut samples);
            }
        }
        let events = receive(&samples);
        assert_eq!(payloads(&events), [(3, b"hi".to_vec())]);
        let RxEvent::Frame { stats, .. } = &events[0] else {
            unreachable!()
        };
        assert_eq!(stats.start, 0);
        assert!(
            (stats.freq_offset_hz - 40.0).abs() < 1.0,
            "{}",
            stats.freq_offset_hz
        );
    }

    #[test]
    fn lengths() {
        assert_eq!(header_len(), 31 * 512);
        assert_eq!(payload_len(Mode::R1, 0), 19 * 512);
        // two training symbols, then 32 + 324 coded bits in symbols of 112
        assert_eq!(payload_len(Mode::F1, 0), (2 + 4) * 640);
        assert_eq!(seconds(16_000), 1.0);
    }
}

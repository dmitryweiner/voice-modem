//! The link layer: who transmits when, acknowledgements and repeats,
//! messages cut into frames. Pure logic on frames and a clock — no sound
//! here; `modem.rs` connects it to the transmitter and the receiver.
//!
//! Stations take turns on one channel (half duplex). A data frame is
//! acknowledged by the station that received it and repeated until it is; a
//! message longer than one frame goes segment by segment, each acknowledged
//! before the next (stop and wait). See `docs/protocol.md`.
//!
//! Made for two stations. More can share a channel and all receive, but a
//! data frame has no destination: every station that hears it acknowledges,
//! and acknowledgements sent at the same moment collide. Addresses come
//! with sessions.
//!
//! Time is the number of samples at the core rate; the caller supplies it.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::SAMPLE_RATE;
use crate::frame::{Frame, Kind, MAX_PAYLOAD, Mode, SEQ_MODULUS};

const fn ms(milliseconds: u64) -> u64 {
    milliseconds * SAMPLE_RATE as u64 / 1000
}

/// The largest payload of a frame in a mode, bytes: short frames are lost
/// less often and cost less to repeat, but a frame has more than a second of
/// preamble and header, and a fast mode should not spend its time on those.
/// In the slowest mode a full frame is five seconds on the air; a fast mode
/// takes a whole text in one frame.
pub const fn frame_bytes(mode: Mode) -> usize {
    match mode {
        Mode::R1 => 64,
        Mode::R2 => 128,
        Mode::R3 | Mode::R4 => 255,
        Mode::F1 | Mode::F2 | Mode::F3 => MAX_PAYLOAD,
    }
}

/// The modes a station chooses from by itself, from the most robust up.
/// R4 is not among them: F1 is nearly three times as fast and withstands
/// more noise and no less echo. R4 is for channels that do not keep the phase,
/// and is set by hand.
pub const LADDER: [Mode; 6] = [Mode::R1, Mode::R2, Mode::R3, Mode::F1, Mode::F2, Mode::F3];

/// The place of a mode on the ladder, or of the nearest slower one.
fn rung(mode: Mode) -> usize {
    LADDER.iter().rposition(|&m| m <= mode).unwrap_or(0)
}

/// How well a frame was heard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heard {
    /// As `RxStats::snr_db`: of a single tone of the header, echo counted as noise.
    pub snr_db: f32,
    /// As `RxStats::carriers_snr_db`: on the carriers of a fast frame.
    pub carriers_snr_db: Option<f32>,
}

/// The fastest mode worth trying when a frame from the other station was
/// heard like this. Each threshold is 3 dB above where the mode stops
/// working in white noise and in a simulated room
/// (`modem-sim/examples/modes.rs`). The tones of a header say whether
/// carriers are worth a try; how many bits a carrier can take, only
/// carriers tell.
pub fn advise(heard: Heard) -> Mode {
    match heard.carriers_snr_db {
        Some(c) if c >= 16.0 => return Mode::F3,
        Some(c) if c >= 11.0 => return Mode::F2,
        Some(c) if c >= 6.0 => return Mode::F1,
        _ => {}
    }
    match heard.snr_db {
        s if s >= 6.0 && heard.carriers_snr_db.is_none() => Mode::F1,
        s if s >= 5.0 => Mode::R3,
        s if s >= -1.0 => Mode::R2,
        _ => Mode::R1,
    }
}

/// After this long without an acknowledgement from anyone the next message
/// starts in the slowest mode again: the channel may have changed.
const MODE_MEMORY: u64 = ms(120_000);
/// A mode that got no acknowledgement is not tried again for this long,
/// whatever the advice: the signal-to-noise ratio does not tell everything
/// (a codec, a speaker that distorts).
const MODE_BAR: u64 = ms(60_000);
/// The longest text of one message, bytes of UTF-8.
pub const MAX_TEXT_BYTES: usize = 1000;
/// The longest station name, bytes of UTF-8.
pub const MAX_NAME_BYTES: usize = 32;

/// After its own transmission a station does not listen for this long: the
/// output buffer of the sound device and the echo of the room.
pub const GUARD_AFTER_TX: u64 = ms(300);
/// An acknowledgement is sent this long after the frame it answers, when
/// the sender listens again.
const ACK_DELAY: u64 = ms(500);
/// How much later than the earliest possible moment an acknowledgement may
/// still arrive: delays of sound devices and of the channel.
const ACK_MARGIN: u64 = ms(1000);
/// With push-to-talk a human has to press a button before each answer.
const PTT_ALLOWANCE: u64 = ms(10_000);
/// A frame is transmitted at most this many times.
pub const MAX_ATTEMPTS: u8 = 5;
/// A repeat waits a random time in this range first, so that two stations
/// that collided do not collide again.
const BACKOFF: (u64, u64) = (ms(200), ms(1200));
/// A station that has just transmitted hears nothing for `GUARD_AFTER_TX`:
/// whoever answers it must start later than that.
const TURNAROUND: u64 = GUARD_AFTER_TX + ms(100);
const _: () = assert!(
    ACK_DELAY >= TURNAROUND,
    "an acknowledgement must not start while its receiver is deaf"
);
/// After the channel falls silent a station waits a random time in this
/// range before it transmits.
const LISTEN: (u64, u64) = (TURNAROUND, TURNAROUND + ms(500));
/// The pause between two frames of one station.
pub const GAP: u64 = ms(100);
/// A greeting is answered with one's own unless one was sent this recently.
const HELLO_FRESH: u64 = ms(60_000);
/// Stations remembered for duplicate detection and reassembly.
const MAX_PEERS: usize = 64;

/// The first byte of every payload: the type in the low half, flags above.
const TYPE_MASK: u8 = 0x0F;
const TYPE_TEXT: u8 = 0;
const TYPE_HELLO: u8 = 1;
/// The first segment of a message.
const FLAG_FIRST: u8 = 0x40;
/// More segments of the message follow.
const FLAG_MORE: u8 = 0x80;

/// What this version of the modem can do, sent in the greeting. Nothing
/// optional exists yet; encryption and fast modes will claim bits here.
pub const CAPABILITIES: u16 = 0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkConfig {
    /// The address of this station; random, chosen by the application.
    pub station: u16,
    /// The name or callsign sent in the greeting; may be empty.
    pub name: String,
    /// True: frames are acknowledged and repeated. False: every frame is
    /// sent once and nothing is expected back (one-way channels).
    pub reliable: bool,
    /// True: every transmission waits for `Link::ptt_pressed` — a human
    /// holds the push-to-talk button of a radio.
    pub manual_ptt: bool,
    /// Seed of the random waits.
    pub seed: u64,
    /// The mode of every data frame, or None to choose it by what the other
    /// station advises in its acknowledgements, starting from the slowest.
    pub mode: Option<Mode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkEvent {
    Text {
        from: u16,
        text: String,
    },
    /// A station greeted: its name and what it can do.
    Hello {
        from: u16,
        name: String,
        capabilities: u16,
    },
    /// An unacknowledged message has been transmitted.
    Sent {
        id: u32,
    },
    /// Every frame of the message was acknowledged.
    Delivered {
        id: u32,
    },
    /// A frame of the message was never acknowledged; the rest of it is dropped.
    Failed {
        id: u32,
    },
    /// No acknowledgement came; the frame goes out again, for the `attempt`-th time.
    Retry {
        id: u32,
        attempt: u8,
    },
    /// Push-to-talk: there is something to transmit, waiting for `ptt_pressed`.
    TxRequest,
    /// Data frames go out in this mode from now on.
    TxMode {
        mode: Mode,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    Empty,
    TooLong { max: usize },
}

/// A message waiting to be sent, or partly sent.
struct Outgoing {
    /// The id its fate is reported under; None for a greeting, which nobody tracks.
    message: Option<u32>,
    /// The type of its segments, the first byte of their payloads without the flags.
    kind: u8,
    bytes: Vec<u8>,
    /// Bytes the other side has (or, with no acknowledgements, that were sent).
    done: usize,
}

impl Outgoing {
    /// Where a segment that starts at `done` ends in a frame of this mode.
    fn cut(&self, mode: Mode) -> usize {
        let room = frame_bytes(mode) - self.header_bytes();
        (self.done + room).min(self.bytes.len())
    }

    /// Bytes of a segment before those of the message: the type and, in
    /// all but the first, the offset.
    fn header_bytes(&self) -> usize {
        if self.done == 0 { 1 } else { 3 }
    }

    /// The payload of the segment from `done` to `end`.
    fn segment(&self, end: usize) -> Vec<u8> {
        let more = if end < self.bytes.len() { FLAG_MORE } else { 0 };
        let mut payload = Vec::with_capacity(self.header_bytes() + end - self.done);
        if self.done == 0 {
            payload.push(self.kind | FLAG_FIRST | more);
        } else {
            payload.push(self.kind | more);
            payload.extend((self.done as u16).to_be_bytes());
        }
        payload.extend_from_slice(&self.bytes[self.done..end]);
        payload
    }
}

enum Phase {
    /// To be transmitted at this time or later.
    Ready {
        at: u64,
    },
    /// Handed to the transmitter in the current burst.
    OnAir,
    WaitAck {
        deadline: u64,
    },
}

/// The segment of the first message of the queue that is on its way.
struct InFlight {
    /// The segment ends at this byte of the message.
    end: usize,
    seq: u8,
    attempts: u8,
    phase: Phase,
}

#[derive(Default)]
struct Peer {
    last_seq: Option<u8>,
    /// The segments of a message received so far.
    partial: Option<Vec<u8>>,
}

pub struct Link {
    config: LinkConfig,
    /// Length of an acknowledgement frame, samples.
    ack_len: u64,
    random: u64,
    queue: VecDeque<Outgoing>,
    /// The mode of the next data frame.
    tx_mode: Mode,
    /// When the last acknowledgement for this station was received.
    last_ack: u64,
    /// This rung of the ladder and those above it are not used before this time.
    barred: Option<(usize, u64)>,
    in_flight: Option<InFlight>,
    /// Acknowledgements owed: station, frame number, the mode to advise, when to send.
    acks: VecDeque<(u16, u8, Mode, u64)>,
    next_seq: u8,
    next_message: u32,
    /// The own transmission lasts until this time.
    tx_until: u64,
    /// The station does not listen before this time.
    deaf_until: u64,
    /// No data frame before this time: the channel was busy a moment ago.
    quiet_until: u64,
    peers: HashMap<u16, Peer>,
    greeted: HashSet<u16>,
    last_hello: Option<u64>,
    ptt_requested: bool,
    ptt_armed: bool,
    events: VecDeque<LinkEvent>,
}

impl Link {
    /// `ack_len` is the length of an acknowledgement frame on the air, samples.
    pub fn new(config: LinkConfig, ack_len: u64) -> Self {
        Self {
            random: config.seed | 1,
            tx_mode: config.mode.unwrap_or(Mode::R1),
            last_ack: 0,
            barred: None,
            config,
            ack_len,
            queue: VecDeque::new(),
            in_flight: None,
            acks: VecDeque::new(),
            next_seq: 0,
            next_message: 1,
            tx_until: 0,
            deaf_until: 0,
            quiet_until: 0,
            peers: HashMap::new(),
            greeted: HashSet::new(),
            last_hello: None,
            ptt_requested: false,
            ptt_armed: false,
            events: VecDeque::new(),
        }
    }

    /// A random time in the range (xorshift64).
    fn random_in(&mut self, (from, to): (u64, u64)) -> u64 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        from + self.random % (to - from)
    }

    /// Queues a text; returns the id its fate is reported under.
    pub fn send_text(&mut self, text: &str) -> Result<u32, SendError> {
        if text.is_empty() {
            return Err(SendError::Empty);
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(SendError::TooLong {
                max: MAX_TEXT_BYTES,
            });
        }
        let id = self.next_message;
        self.next_message += 1;
        self.queue.push_back(Outgoing {
            message: Some(id),
            kind: TYPE_TEXT,
            bytes: text.as_bytes().to_vec(),
            done: 0,
        });
        Ok(id)
    }

    /// Takes back a text that is not delivered yet: what is left of it is
    /// not sent and nothing more is reported about it. False when there is
    /// no such text any more. A frame already on the air still arrives, so
    /// the other side may have the text all the same.
    pub fn cancel(&mut self, id: u32) -> bool {
        let is_it = |outgoing: &Outgoing| outgoing.message == Some(id);
        // The segment on its way is one of the first message of the queue.
        let flight = if self.queue.front().is_some_and(is_it) {
            self.in_flight.take()
        } else {
            None
        };
        // Its acknowledgement may still come: nothing goes out into it.
        if let Some(InFlight {
            phase: Phase::WaitAck { deadline },
            ..
        }) = flight
        {
            self.quiet_until = self.quiet_until.max(deadline);
        }
        let queued = self.queue.len();
        self.queue.retain(|outgoing| !is_it(outgoing));
        // The button was asked for this text alone: the request is withdrawn.
        if self.queue.is_empty() && self.acks.is_empty() {
            self.ptt_requested = false;
            self.ptt_armed = false;
        }
        self.queue.len() < queued
    }

    /// Queues a greeting: the name of this station and what it can do.
    pub fn hello(&mut self, now: u64) {
        let mut bytes = CAPABILITIES.to_be_bytes().to_vec();
        let mut end = self.config.name.len().min(MAX_NAME_BYTES);
        while !self.config.name.is_char_boundary(end) {
            end -= 1;
        }
        bytes.extend_from_slice(&self.config.name.as_bytes()[..end]);
        self.queue.push_back(Outgoing {
            message: None,
            kind: TYPE_HELLO,
            bytes,
            done: 0,
        });
        self.last_hello = Some(now);
    }

    /// The mode of the data frame `poll` has just returned, and of the next ones.
    pub fn tx_mode(&self) -> Mode {
        self.tx_mode
    }

    fn set_mode(&mut self, mode: Mode) {
        if self.config.mode.is_none() && mode != self.tx_mode {
            self.tx_mode = mode;
            self.events.push_back(LinkEvent::TxMode { mode });
        }
    }

    /// The part of the first message that is on its way was received (or,
    /// with no acknowledgements, sent): on to the next, or the message is done.
    fn advance(&mut self, end: usize) {
        let Some(outgoing) = self.queue.front_mut() else {
            return;
        };
        outgoing.done = end;
        if end < outgoing.bytes.len() {
            return;
        }
        let id = outgoing.message;
        self.queue.pop_front();
        if let Some(id) = id {
            self.events.push_back(if self.config.reliable {
                LinkEvent::Delivered { id }
            } else {
                LinkEvent::Sent { id }
            });
        }
    }

    /// Push-to-talk: the button is down, transmit what is waiting.
    pub fn ptt_pressed(&mut self) {
        self.ptt_armed = self.ptt_requested;
    }

    pub fn poll_event(&mut self) -> Option<LinkEvent> {
        self.events.pop_front()
    }

    /// The station does not listen before this time: it transmits, then the guard.
    pub fn deaf_until(&self) -> u64 {
        self.deaf_until
    }

    /// Nothing to transmit and nothing awaited.
    pub fn is_idle(&self, now: u64) -> bool {
        self.queue.is_empty()
            && self.in_flight.is_none()
            && self.acks.is_empty()
            && now >= self.tx_until
    }

    /// A frame was received at `now`.
    pub fn on_frame(&mut self, frame: &Frame, heard: Heard, now: u64) {
        match frame.kind {
            Kind::Ack { advice } => self.on_ack(frame, advice, now),
            Kind::Data | Kind::Broadcast => self.on_data(frame, heard, now),
        }
    }

    fn on_ack(&mut self, frame: &Frame, advice: Mode, now: u64) {
        let ours = frame.station == self.config.station;
        // A late acknowledgement, after the repeat was already scheduled, counts as well.
        let Some(flight) = self
            .in_flight
            .take_if(|f| ours && f.seq == frame.seq && !matches!(f.phase, Phase::OnAir))
        else {
            return;
        };
        self.advance(flight.end);
        // Down at once to what the other side advises, up one step at a
        // time, and not to where a frame was lost a moment ago.
        let mut target = rung(advice).min(rung(self.tx_mode) + 1);
        if let Some((barred, until)) = self.barred
            && now < until
        {
            target = target.min(barred.saturating_sub(1));
        }
        self.set_mode(LADDER[target]);
        self.last_ack = now;
        // The station that acknowledged does not listen yet.
        self.quiet_until = self.quiet_until.max(now + TURNAROUND);
    }

    fn on_data(&mut self, frame: &Frame, heard: Heard, now: u64) {
        let from = frame.station;
        // Its own echo, or another station that happened to pick the same address.
        if from == self.config.station {
            return;
        }
        if frame.kind == Kind::Data
            && !self
                .acks
                .iter()
                .any(|&(station, seq, ..)| (station, seq) == (from, frame.seq))
        {
            self.acks
                .push_back((from, frame.seq, advise(heard), now + ACK_DELAY));
        }
        if self.peers.len() >= MAX_PEERS && !self.peers.contains_key(&from) {
            self.peers.clear();
        }
        let peer = self.peers.entry(from).or_default();
        // A repeat of what was already received: its acknowledgement was lost.
        if peer.last_seq == Some(frame.seq) {
            return;
        }
        peer.last_seq = Some(frame.seq);

        let Some((&first, mut body)) = frame.payload.split_first() else {
            return;
        };
        // Every segment but the first says where in the message it starts.
        let mut offset = 0;
        if first & FLAG_FIRST == 0 {
            let Some((&at, rest)) = body.split_first_chunk::<2>() else {
                return;
            };
            (offset, body) = (usize::from(u16::from_be_bytes(at)), rest);
        }
        match first & TYPE_MASK {
            TYPE_TEXT => {
                if first & FLAG_FIRST != 0 {
                    peer.partial = Some(Vec::new());
                }
                // Without the first segment the rest is of no use.
                let Some(partial) = &mut peer.partial else {
                    return;
                };
                // A segment that was cut again for a slower mode may bring
                // bytes that are here already.
                let Some(known) = partial.len().checked_sub(offset) else {
                    peer.partial = None;
                    return;
                };
                partial.extend_from_slice(body.get(known..).unwrap_or_default());
                if first & FLAG_MORE == 0 {
                    let text = String::from_utf8_lossy(partial).into_owned();
                    peer.partial = None;
                    self.events.push_back(LinkEvent::Text { from, text });
                }
            }
            TYPE_HELLO if offset == 0 => {
                let Some((&[high, low], name)) = body.split_first_chunk::<2>() else {
                    return;
                };
                let name = String::from_utf8_lossy(name).into_owned();
                self.events.push_back(LinkEvent::Hello {
                    from,
                    name,
                    capabilities: u16::from_be_bytes([high, low]),
                });
                let fresh = self.last_hello.is_some_and(|at| now < at + HELLO_FRESH);
                if self.greeted.insert(from) && !fresh {
                    self.hello(now);
                }
            }
            // A type from a later version: acknowledged, not understood.
            _ => {}
        }
    }

    /// What to transmit now, if anything: called regularly with the time and
    /// whether a frame is being received. The frames go out one after
    /// another at once, those with a payload in the mode `tx_mode()`; report
    /// their total length with `on_tx`.
    pub fn poll(&mut self, now: u64, busy: bool) -> Vec<Frame> {
        if busy {
            self.quiet_until = now + self.random_in(LISTEN);
        }
        if now < self.tx_until {
            return Vec::new();
        }
        self.check_timeout(now);
        if self.in_flight.is_none() && !self.queue.is_empty() {
            if now > self.last_ack + MODE_MEMORY {
                self.set_mode(Mode::R1);
            }
            if let Some(outgoing) = self.queue.front() {
                let seq = self.next_seq;
                self.next_seq = (seq + 1) % SEQ_MODULUS;
                self.in_flight = Some(InFlight {
                    end: outgoing.cut(self.tx_mode),
                    seq,
                    attempts: 0,
                    phase: Phase::Ready { at: now },
                });
            }
        }

        let manual = self.config.manual_ptt;
        let ack_due = self.acks.front().is_some_and(|&(.., due)| due <= now);
        // With push-to-talk the human decides when the channel is free.
        // Otherwise: silence for a while, and no acknowledgement owed — the
        // other side waits for that first.
        let clear = manual || (!busy && now >= self.quiet_until && self.acks.is_empty());
        let data_ready = matches!(&self.in_flight, Some(InFlight { phase: Phase::Ready { at }, .. }) if *at <= now);
        if !ack_due && !(data_ready && clear) {
            return Vec::new();
        }
        if manual && !self.ptt_armed {
            if !self.ptt_requested {
                self.ptt_requested = true;
                self.events.push_back(LinkEvent::TxRequest);
            }
            return Vec::new();
        }
        self.ptt_requested = false;
        self.ptt_armed = false;

        let mut frames = Vec::new();
        // One press of the button carries everything that is waiting.
        while let Some(&(station, seq, advice, due)) = self.acks.front()
            && (manual || due <= now)
        {
            self.acks.pop_front();
            frames.push(Frame {
                kind: Kind::Ack { advice },
                station,
                seq,
                payload: Vec::new(),
            });
        }
        if data_ready
            && clear
            && let (Some(flight), Some(outgoing)) = (&mut self.in_flight, self.queue.front())
        {
            flight.attempts += 1;
            flight.phase = Phase::OnAir;
            // A repeat in a slower mode is cut again to the size of that mode.
            flight.end = flight.end.min(outgoing.cut(self.tx_mode));
            let kind = if self.config.reliable {
                Kind::Data
            } else {
                Kind::Broadcast
            };
            frames.push(Frame {
                kind,
                station: self.config.station,
                seq: flight.seq,
                payload: outgoing.segment(flight.end),
            });
        }
        frames
    }

    /// The frames returned by `poll` at `now` take `len` samples on the air.
    pub fn on_tx(&mut self, now: u64, len: u64) {
        self.tx_until = now + len;
        self.deaf_until = self.tx_until + GUARD_AFTER_TX;
        let after = self.tx_until + GAP;
        match &mut self.in_flight {
            Some(flight) if matches!(flight.phase, Phase::OnAir) => {
                if self.config.reliable {
                    let patience = if self.config.manual_ptt {
                        PTT_ALLOWANCE
                    } else {
                        0
                    };
                    flight.phase = Phase::WaitAck {
                        deadline: self.tx_until + ACK_DELAY + self.ack_len + ACK_MARGIN + patience,
                    };
                } else {
                    let end = flight.end;
                    self.in_flight = None;
                    self.advance(end);
                    self.quiet_until = self.quiet_until.max(after);
                }
            }
            // An acknowledgement went out first: the data frame follows it.
            Some(InFlight {
                phase: Phase::Ready { at },
                ..
            }) => *at = (*at).max(after),
            _ => self.quiet_until = self.quiet_until.max(after),
        }
    }

    fn check_timeout(&mut self, now: u64) {
        let Some(flight) = &mut self.in_flight else {
            return;
        };
        let Phase::WaitAck { deadline } = flight.phase else {
            return;
        };
        if now < deadline {
            return;
        }
        let id = self.queue.front().and_then(|outgoing| outgoing.message);
        if flight.attempts < MAX_ATTEMPTS {
            let attempt = flight.attempts + 1;
            let at = now + self.random_in(BACKOFF);
            // Whatever the reason, a slower mode has the better chance.
            if self.config.mode.is_none() {
                let failed = rung(self.tx_mode);
                self.barred = Some((failed, now + MODE_BAR));
                self.set_mode(LADDER[failed.saturating_sub(1)]);
            }
            if let Some(flight) = &mut self.in_flight {
                flight.phase = Phase::Ready { at };
            }
            if let Some(id) = id {
                self.events.push_back(LinkEvent::Retry { id, attempt });
            }
            return;
        }
        // The rest of the message is of no use without this part.
        self.in_flight = None;
        self.queue.pop_front();
        if let Some(id) = id {
            self.events.push_back(LinkEvent::Failed { id });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tx::frame_len;

    /// Length of an acknowledgement and of an R1 data frame with `bytes` of payload, samples.
    const ACK_LEN: u64 = 4096 + 31 * 512;
    fn data_len(bytes: usize) -> u64 {
        frame_len(Kind::Data, Mode::R1, bytes) as u64
    }
    const TICK: u64 = ms(10);
    const ACK: Kind = Kind::Ack { advice: Mode::R1 };
    /// A frame heard well enough for the slowest mode only.
    const QUIET: Heard = Heard {
        snr_db: -5.0,
        carriers_snr_db: None,
    };

    fn config(station: u16) -> LinkConfig {
        LinkConfig {
            station,
            name: format!("station {station}"),
            reliable: true,
            manual_ptt: false,
            seed: u64::from(station) * 7919,
            mode: None,
        }
    }

    /// Decides whether a frame from a sender (first index) reaches a receiver.
    type Hears = Box<dyn FnMut(usize, usize, &Frame) -> bool>;

    /// Two or more links on one channel where frames take their real time.
    struct Air {
        links: Vec<Link>,
        now: u64,
        /// Frames on the air: sender, the frame, its mode, start, end.
        flying: Vec<(usize, Frame, Mode, u64, u64)>,
        /// Every frame that was transmitted: sender, frame, start.
        log: Vec<(usize, Frame, u64)>,
        hears: Hears,
        /// How well every frame is heard, dB, on the tones of its header and
        /// on the carriers of a fast one: decides the advice in acknowledgements.
        snr: f32,
        events: Vec<Vec<LinkEvent>>,
    }

    impl Air {
        fn new(configs: Vec<LinkConfig>) -> Self {
            let events = configs.iter().map(|_| Vec::new()).collect();
            let links = configs.into_iter().map(|c| Link::new(c, ACK_LEN)).collect();
            Self {
                links,
                now: 0,
                flying: Vec::new(),
                log: Vec::new(),
                hears: Box::new(|_, _, _| true),
                snr: -5.0,
                events,
            }
        }

        fn pair() -> Self {
            Self::new(vec![config(1), config(2)])
        }

        fn run(&mut self, duration: u64) {
            let end = self.now + duration;
            while self.now < end {
                self.now += TICK;
                let now = self.now;
                // Frames that ended are received — unless the receiver was transmitting meanwhile.
                let (landed, flying): (Vec<_>, Vec<_>) = std::mem::take(&mut self.flying)
                    .into_iter()
                    .partition(|f| f.4 <= now);
                self.flying = flying;
                for (sender, frame, mode, start, _) in landed {
                    let heard = Heard {
                        snr_db: self.snr,
                        carriers_snr_db: (mode >= Mode::F1 && frame.kind.has_payload())
                            .then_some(self.snr),
                    };
                    for receiver in 0..self.links.len() {
                        let collided =
                            self.log.iter().any(|(other, _, at)| {
                                *other != sender && *at >= start && *at < now
                            }) || self.links[receiver].deaf_until() > start;
                        if receiver != sender && !collided && (self.hears)(sender, receiver, &frame)
                        {
                            self.links[receiver].on_frame(&frame, heard, now);
                        }
                    }
                }
                for index in 0..self.links.len() {
                    // A receiver notices a frame 0.3 s after it started.
                    let busy = self
                        .flying
                        .iter()
                        .any(|(sender, _, _, start, _)| *sender != index && now >= start + ms(300));
                    let frames = self.links[index].poll(now, busy);
                    let mut at = now;
                    for frame in frames {
                        let mode = self.links[index].tx_mode();
                        let len = frame_len(frame.kind, mode, frame.payload.len()) as u64;
                        self.log.push((index, frame.clone(), at));
                        self.flying.push((index, frame, mode, at, at + len));
                        at += len + GAP;
                    }
                    if at > now {
                        self.links[index].on_tx(now, at - now);
                    }
                    while let Some(event) = self.links[index].poll_event() {
                        self.events[index].push(event);
                    }
                }
            }
        }

        /// Frames of this kind sent by a station; any acknowledgement counts as `ACK`.
        fn sent(&self, index: usize, kind: Kind) -> usize {
            self.log
                .iter()
                .filter(|(sender, frame, _)| {
                    *sender == index
                        && std::mem::discriminant(&frame.kind) == std::mem::discriminant(&kind)
                })
                .count()
        }
    }

    fn text(from: u16, text: &str) -> LinkEvent {
        LinkEvent::Text {
            from,
            text: text.into(),
        }
    }

    #[test]
    fn a_text_is_delivered_and_acknowledged() {
        let mut air = Air::pair();
        let id = air.links[0].send_text("hello").unwrap();
        air.run(ms(6000));
        assert_eq!(air.events[1], [text(1, "hello")]);
        assert_eq!(air.events[0], [LinkEvent::Delivered { id }]);
        assert_eq!((air.sent(0, Kind::Data), air.sent(1, ACK)), (1, 1));
        assert!(air.links.iter().all(|link| link.is_idle(air.now)));
    }

    #[test]
    fn the_acknowledgement_waits_until_the_sender_listens_again() {
        let mut air = Air::pair();
        air.links[0].send_text("hello").unwrap();
        air.run(ms(6000));
        let (_, _, data_at) = air.log[0];
        let (_, ack, ack_at) = &air.log[1];
        assert_eq!((ack.kind, ack.station, ack.seq), (ACK, 1, 0));
        let pause = ack_at - (data_at + data_len(6));
        assert!(
            (ACK_DELAY..ACK_DELAY + 2 * TICK).contains(&pause),
            "{pause}"
        );
    }

    #[test]
    fn a_lost_frame_is_repeated_until_it_gets_through() {
        let mut air = Air::pair();
        let mut losses = 2;
        air.hears = Box::new(move |_, _, frame| {
            let lost = frame.kind == Kind::Data && losses > 0;
            losses -= usize::from(lost);
            !lost
        });
        let id = air.links[0].send_text("again").unwrap();
        air.run(ms(20_000));
        assert_eq!(air.events[1], [text(1, "again")]);
        assert_eq!(
            air.events[0],
            [
                LinkEvent::Retry { id, attempt: 2 },
                LinkEvent::Retry { id, attempt: 3 },
                LinkEvent::Delivered { id }
            ]
        );
        assert_eq!(air.sent(0, Kind::Data), 3);
    }

    #[test]
    fn a_lost_acknowledgement_does_not_deliver_the_text_twice() {
        let mut air = Air::pair();
        let mut losses = 1;
        air.hears = Box::new(move |_, _, frame| {
            let lost = matches!(frame.kind, Kind::Ack { .. }) && losses > 0;
            losses -= usize::from(lost);
            !lost
        });
        let id = air.links[0].send_text("once").unwrap();
        air.run(ms(20_000));
        assert_eq!(air.events[1], [text(1, "once")]);
        assert_eq!(
            air.events[0],
            [
                LinkEvent::Retry { id, attempt: 2 },
                LinkEvent::Delivered { id }
            ]
        );
        // the repeat was acknowledged again
        assert_eq!(air.sent(1, ACK), 2);
    }

    #[test]
    fn gives_up_after_five_attempts() {
        let mut air = Air::pair();
        air.hears = Box::new(|_, _, _| false);
        let id = air.links[0].send_text("into the void").unwrap();
        let second = air.links[0].send_text("next").unwrap();
        air.run(ms(120_000));
        assert_eq!(air.sent(0, Kind::Data), 2 * usize::from(MAX_ATTEMPTS));
        let failed: Vec<_> = air.events[0]
            .iter()
            .filter(|e| matches!(e, LinkEvent::Failed { .. }))
            .collect();
        assert_eq!(
            failed,
            [&LinkEvent::Failed { id }, &LinkEvent::Failed { id: second }]
        );
        let attempts: Vec<u8> = air.events[0]
            .iter()
            .filter_map(|e| {
                if let LinkEvent::Retry { id: i, attempt } = e {
                    (*i == id).then_some(*attempt)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(attempts, [2, 3, 4, 5]);
        assert!(air.links[0].is_idle(air.now));
    }

    #[test]
    fn a_long_text_goes_in_segments_and_arrives_whole() {
        let mut air = Air::pair();
        let long: String = "Съешь же ещё этих мягких французских булок, да выпей чаю. ".repeat(4);
        assert!(long.len() > 3 * 64);
        let id = air.links[0].send_text(&long).unwrap();
        air.run(ms(90_000));
        assert_eq!(air.events[1], [text(1, &long)]);
        assert_eq!(air.events[0], [LinkEvent::Delivered { id }]);
        // 63 bytes in the first frame, 61 after their offset in each of the others
        let segments = 1 + (long.len() - 63).div_ceil(61);
        assert_eq!(
            (air.sent(0, Kind::Data), air.sent(1, ACK)),
            (segments, segments)
        );
        assert!(
            air.log
                .iter()
                .all(|(_, frame, _)| frame.payload.len() <= frame_bytes(Mode::R1))
        );
    }

    #[test]
    fn a_message_that_failed_midway_does_not_leak_into_the_next() {
        let mut air = Air::pair();
        // The second segment of the first message never arrives.
        air.hears = Box::new(|_, _, frame| !(frame.kind == Kind::Data && frame.seq == 1));
        let first = air.links[0].send_text(&"a".repeat(180)).unwrap();
        let second = air.links[0].send_text("clean").unwrap();
        air.run(ms(120_000));
        assert_eq!(air.events[1], [text(1, "clean")]);
        assert!(air.events[0].contains(&LinkEvent::Failed { id: first }));
        assert!(air.events[0].contains(&LinkEvent::Delivered { id: second }));
        // the third segment of the failed message was not even tried
        assert_eq!(air.sent(0, Kind::Data), 1 + usize::from(MAX_ATTEMPTS) + 1);
    }

    #[test]
    fn a_cancelled_text_that_waits_in_the_queue_is_never_sent() {
        let mut air = Air::pair();
        let first = air.links[0].send_text("goes").unwrap();
        let second = air.links[0].send_text("never mind").unwrap();
        let third = air.links[0].send_text("goes too").unwrap();
        assert!(air.links[0].cancel(second));
        air.run(ms(15_000));
        assert_eq!(air.events[1], [text(1, "goes"), text(1, "goes too")]);
        assert_eq!(
            air.events[0],
            [
                LinkEvent::Delivered { id: first },
                LinkEvent::Delivered { id: third }
            ]
        );
        // there is nothing left to cancel
        assert!(!air.links[0].cancel(second));
        assert!(!air.links[0].cancel(first));
        assert!(!air.links[0].cancel(99));
    }

    #[test]
    fn a_cancelled_text_is_not_repeated_and_the_next_one_goes() {
        let mut air = Air::pair();
        air.hears = Box::new(|_, _, frame| frame.payload.last() != Some(&b'?'));
        let lost = air.links[0].send_text("anyone?").unwrap();
        let next = air.links[0].send_text("next").unwrap();
        // the first transmission and the wait for its acknowledgement are over, the repeat is not
        air.run(ms(5800));
        assert_eq!(air.sent(0, Kind::Data), 1);
        assert!(air.links[0].cancel(lost));
        air.run(ms(15_000));
        assert_eq!(air.events[1], [text(1, "next")]);
        // no failure is reported: the application knows what it cancelled
        assert_eq!(
            air.events[0],
            [
                LinkEvent::Retry {
                    id: lost,
                    attempt: 2
                },
                LinkEvent::Delivered { id: next }
            ]
        );
        assert_eq!(air.sent(0, Kind::Data), 2);
        assert!(air.links[0].is_idle(air.now));
    }

    #[test]
    fn the_next_text_waits_for_the_answer_to_the_cancelled_one() {
        let mut air = Air::pair();
        air.links[0].send_text("on the air").unwrap();
        air.links[0].send_text("next").unwrap();
        air.run(ms(1000));
        assert!(air.links[0].cancel(1));
        air.run(ms(15_000));
        // the frame was on the air already: it arrives, and its acknowledgement collides with nothing
        assert_eq!(air.events[1], [text(1, "on the air"), text(1, "next")]);
        assert_eq!(air.events[0], [LinkEvent::Delivered { id: 2 }]);
        assert_eq!((air.sent(0, Kind::Data), air.sent(1, ACK)), (2, 2));
    }

    #[test]
    fn cancelling_a_long_text_drops_the_rest_of_it() {
        let mut air = Air::pair();
        let long = air.links[0].send_text(&"a".repeat(180)).unwrap();
        let next = air.links[0].send_text("clean").unwrap();
        // the first segment is acknowledged, the second is on its way
        air.run(ms(13_000));
        assert_eq!(air.sent(0, Kind::Data), 2);
        assert!(air.links[0].cancel(long));
        air.run(ms(25_000));
        assert_eq!(air.events[1], [text(1, "clean")]);
        assert_eq!(air.events[0], [LinkEvent::Delivered { id: next }]);
        assert_eq!(air.sent(0, Kind::Data), 3);
    }

    #[test]
    fn both_stations_talk_at_once_and_both_texts_arrive() {
        let mut air = Air::pair();
        let a = air.links[0].send_text("from one").unwrap();
        let b = air.links[1].send_text("from two").unwrap();
        air.run(ms(60_000));
        assert_eq!(
            air.events[1]
                .iter()
                .filter(|e| matches!(e, LinkEvent::Text { .. }))
                .collect::<Vec<_>>(),
            [&text(1, "from one")]
        );
        assert_eq!(
            air.events[0]
                .iter()
                .filter(|e| matches!(e, LinkEvent::Text { .. }))
                .collect::<Vec<_>>(),
            [&text(2, "from two")]
        );
        assert!(air.events[0].contains(&LinkEvent::Delivered { id: a }));
        assert!(air.events[1].contains(&LinkEvent::Delivered { id: b }));
    }

    #[test]
    fn does_not_start_while_another_station_transmits() {
        let mut air = Air::pair();
        air.links[0].send_text(&"x".repeat(60)).unwrap();
        air.run(ms(1000));
        air.links[1].send_text("wait for it").unwrap();
        air.run(ms(30_000));
        let (_, _, first_at) = air.log[0];
        let first_end = first_at + data_len(61);
        let (_, _, second_at) = *air
            .log
            .iter()
            .find(|(sender, frame, _)| *sender == 1 && frame.kind == Kind::Data)
            .unwrap();
        assert!(
            second_at >= first_end + LISTEN.0,
            "started {} samples after the end",
            second_at as i64 - first_end as i64
        );
        assert_eq!((air.sent(0, Kind::Data), air.sent(1, Kind::Data)), (1, 1));
    }

    #[test]
    fn ignores_its_own_address() {
        let mut link = Link::new(config(5), ACK_LEN);
        link.on_frame(
            &Frame {
                kind: Kind::Data,
                station: 5,
                seq: 0,
                payload: vec![TYPE_TEXT | FLAG_FIRST, b'x'],
            },
            QUIET,
            100,
        );
        assert_eq!(link.poll_event(), None);
        assert!(link.is_idle(100));
    }

    #[test]
    fn an_acknowledgement_for_another_frame_changes_nothing() {
        let mut air = Air::pair();
        air.hears = Box::new(|_, _, _| false);
        air.links[0].send_text("x").unwrap();
        air.run(ms(3000));
        let now = air.now;
        air.links[0].on_frame(
            &Frame {
                kind: ACK,
                station: 1,
                seq: 9,
                payload: vec![],
            },
            QUIET,
            now,
        );
        air.links[0].on_frame(
            &Frame {
                kind: ACK,
                station: 2,
                seq: 0,
                payload: vec![],
            },
            QUIET,
            now,
        );
        assert!(!air.links[0].is_idle(now));
        air.links[0].on_frame(
            &Frame {
                kind: ACK,
                station: 1,
                seq: 0,
                payload: vec![],
            },
            QUIET,
            now,
        );
        assert!(air.links[0].is_idle(now + ms(10_000)));
    }

    #[test]
    fn unreliable_mode_sends_once_and_asks_for_nothing() {
        let mut air = Air::new(vec![
            LinkConfig {
                reliable: false,
                ..config(1)
            },
            config(2),
        ]);
        let id = air.links[0].send_text(&"b".repeat(100)).unwrap();
        air.run(ms(30_000));
        assert_eq!(air.events[1], [text(1, &"b".repeat(100))]);
        assert_eq!(air.events[0], [LinkEvent::Sent { id }]);
        assert_eq!((air.sent(0, Kind::Broadcast), air.sent(1, ACK)), (2, 0));
        // the two segments are two pauses apart: one closes the burst, one opens the next
        let gap = air.log[1].2 - (air.log[0].2 + data_len(64));
        assert!((2 * GAP..2 * GAP + 2 * TICK).contains(&gap), "{gap}");
    }

    #[test]
    fn greetings_are_exchanged_once() {
        let mut air = Air::pair();
        let now = air.now;
        air.links[0].hello(now);
        air.run(ms(30_000));
        assert_eq!(
            air.events[1],
            [LinkEvent::Hello {
                from: 1,
                name: "station 1".into(),
                capabilities: CAPABILITIES
            }]
        );
        assert_eq!(
            air.events[0],
            [LinkEvent::Hello {
                from: 2,
                name: "station 2".into(),
                capabilities: CAPABILITIES
            }]
        );
        assert_eq!((air.sent(0, Kind::Data), air.sent(1, Kind::Data)), (1, 1));
        // greeting the same station again is not answered a second time
        let now = air.now;
        air.links[0].hello(now);
        air.run(ms(30_000));
        assert_eq!((air.sent(0, Kind::Data), air.sent(1, Kind::Data)), (2, 1));
    }

    #[test]
    fn a_long_name_is_cut_at_a_character() {
        let mut link = Link::new(
            LinkConfig {
                name: "я".repeat(40),
                ..config(1)
            },
            ACK_LEN,
        );
        link.hello(0);
        let frames = link.poll(ms(1000), false);
        assert_eq!(frames[0].payload.len(), 3 + MAX_NAME_BYTES);
        assert_eq!(
            String::from_utf8(frames[0].payload[3..].to_vec()).unwrap(),
            "я".repeat(16)
        );
    }

    #[test]
    fn rejects_empty_and_overlong_texts() {
        let mut link = Link::new(config(1), ACK_LEN);
        assert_eq!(link.send_text(""), Err(SendError::Empty));
        assert_eq!(
            link.send_text(&"x".repeat(MAX_TEXT_BYTES + 1)),
            Err(SendError::TooLong { max: 1000 })
        );
        assert_eq!(link.send_text(&"x".repeat(MAX_TEXT_BYTES)), Ok(1));
        assert_eq!(link.send_text("y"), Ok(2));
    }

    #[test]
    fn unknown_payload_types_are_acknowledged_and_ignored() {
        let mut link = Link::new(config(1), ACK_LEN);
        link.on_frame(
            &Frame {
                kind: Kind::Data,
                station: 2,
                seq: 0,
                payload: vec![0x0F | FLAG_FIRST, 1, 2],
            },
            QUIET,
            0,
        );
        link.on_frame(
            &Frame {
                kind: Kind::Data,
                station: 2,
                seq: 1,
                payload: vec![],
            },
            QUIET,
            0,
        );
        assert_eq!(link.poll_event(), None);
        let acks = link.poll(ACK_DELAY, false);
        assert_eq!(
            acks.iter()
                .map(|f| (f.kind, f.station, f.seq))
                .collect::<Vec<_>>(),
            [(ACK, 2, 0), (ACK, 2, 1)]
        );
    }

    #[test]
    fn push_to_talk_waits_for_the_button() {
        let mut air = Air::new(vec![
            LinkConfig {
                manual_ptt: true,
                ..config(1)
            },
            LinkConfig {
                manual_ptt: true,
                ..config(2)
            },
        ]);
        let id = air.links[0].send_text("over").unwrap();
        air.run(ms(3000));
        assert_eq!(air.events[0], [LinkEvent::TxRequest]);
        assert!(air.log.is_empty());

        air.links[0].ptt_pressed();
        air.run(ms(5000));
        assert_eq!(air.sent(0, Kind::Data), 1);
        // the other operator is asked to press for the acknowledgement
        assert_eq!(air.events[1], [text(1, "over"), LinkEvent::TxRequest]);
        assert_eq!(air.sent(1, ACK), 0);

        // they take their time, and answer with a text in the same press
        air.run(ms(6000));
        let reply = air.links[1].send_text("roger").unwrap();
        air.links[1].ptt_pressed();
        air.run(ms(8000));
        assert_eq!(
            air.events[0],
            [
                LinkEvent::TxRequest,
                LinkEvent::Delivered { id },
                text(2, "roger"),
                LinkEvent::TxRequest
            ]
        );
        let burst: Vec<Kind> = air
            .log
            .iter()
            .filter(|(sender, _, _)| *sender == 1)
            .map(|(_, frame, _)| frame.kind)
            .collect();
        assert_eq!(burst, [ACK, Kind::Data]);

        air.links[0].ptt_pressed();
        air.run(ms(5000));
        assert!(air.events[1].contains(&LinkEvent::Delivered { id: reply }));
        // no frame was ever repeated: the timeouts allow for the human
        assert_eq!((air.sent(0, Kind::Data), air.sent(1, Kind::Data)), (1, 1));
    }

    #[test]
    fn a_press_with_nothing_to_send_is_forgotten() {
        let mut air = Air::new(vec![
            LinkConfig {
                manual_ptt: true,
                ..config(1)
            },
            config(2),
        ]);
        air.links[0].ptt_pressed();
        air.links[0].send_text("later").unwrap();
        air.run(ms(3000));
        assert!(air.log.is_empty());
        assert_eq!(air.events[0], [LinkEvent::TxRequest]);
    }

    #[test]
    fn a_press_for_a_cancelled_text_does_not_carry_over_to_the_next() {
        let mut air = Air::new(vec![
            LinkConfig {
                manual_ptt: true,
                ..config(1)
            },
            config(2),
        ]);
        let id = air.links[0].send_text("never mind").unwrap();
        air.run(ms(1000));
        assert_eq!(air.events[0], [LinkEvent::TxRequest]);
        assert!(air.links[0].cancel(id));
        air.links[0].ptt_pressed();
        air.links[0].send_text("later").unwrap();
        air.run(ms(3000));
        assert!(air.log.is_empty());
        assert_eq!(air.events[0], [LinkEvent::TxRequest, LinkEvent::TxRequest]);
    }

    /// The modes of the data frames a station sent, in order.
    fn modes(air: &Air, index: usize) -> Vec<Mode> {
        air.events[index]
            .iter()
            .filter_map(|e| {
                if let LinkEvent::TxMode { mode } = e {
                    Some(*mode)
                } else {
                    None
                }
            })
            .collect()
    }

    /// How a frame of tones, and how a frame of carriers was heard.
    fn tones(snr_db: f32) -> Heard {
        Heard {
            snr_db,
            carriers_snr_db: None,
        }
    }
    fn carriers(snr_db: f32, carriers_snr_db: f32) -> Heard {
        Heard {
            snr_db,
            carriers_snr_db: Some(carriers_snr_db),
        }
    }

    #[test]
    fn advice_follows_the_signal_to_noise_ratio() {
        assert_eq!(
            [-20.0, -1.1, -1.0, 4.9, 5.0, 5.9, 6.0, 40.0].map(|snr| advise(tones(snr))),
            [
                Mode::R1,
                Mode::R1,
                Mode::R2,
                Mode::R2,
                Mode::R3,
                Mode::R3,
                // tones cannot tell more than that carriers are worth a try
                Mode::F1,
                Mode::F1
            ]
        );
    }

    #[test]
    fn carriers_tell_how_many_bits_they_can_take() {
        assert_eq!(
            [6.0, 10.9, 11.0, 15.9, 16.0, 30.0].map(|snr| advise(carriers(10.0, snr))),
            [Mode::F1, Mode::F1, Mode::F2, Mode::F2, Mode::F3, Mode::F3]
        );
        // whatever the tones of the header looked like
        assert_eq!(advise(carriers(-3.0, 20.0)), Mode::F3);
        // a fast frame that barely got through: back to tones, as fast as they go
        assert_eq!(advise(carriers(20.0, 5.9)), Mode::R3);
        assert_eq!(advise(carriers(2.0, 5.9)), Mode::R2);
    }

    #[test]
    fn the_ladder_skips_the_mode_that_another_beats() {
        assert!(LADDER.is_sorted() && !LADDER.contains(&Mode::R4));
        assert_eq!(rung(Mode::R1), 0);
        assert_eq!(LADDER[rung(Mode::F3)], Mode::F3);
        // advice from a station that has R4 on its ladder
        assert_eq!(LADDER[rung(Mode::R4)], Mode::R3);
    }

    #[test]
    fn the_mode_climbs_one_step_per_acknowledged_frame_up_to_the_advice() {
        let mut air = Air::pair();
        air.snr = 8.0;
        for text in ["one", "two", "three", "four", "five"] {
            air.links[0].send_text(text).unwrap();
        }
        air.run(ms(40_000));
        assert_eq!(
            air.events[1]
                .iter()
                .filter(|e| matches!(e, LinkEvent::Text { .. }))
                .count(),
            5
        );
        // advised F1 at 8 dB: R1 → R2 → R3 → F1 and no further
        assert_eq!(modes(&air, 0), [Mode::R2, Mode::R3, Mode::F1]);
        assert_eq!(air.links[0].tx_mode(), Mode::F1);
        // the listener sent nothing but acknowledgements: its own mode is untouched
        assert_eq!(air.links[1].tx_mode(), Mode::R1);
        let advice: Vec<Kind> = air
            .log
            .iter()
            .filter(|(sender, ..)| *sender == 1)
            .map(|(_, frame, _)| frame.kind)
            .collect();
        assert_eq!(advice, [Kind::Ack { advice: Mode::F1 }; 5]);
    }

    /// A pair whose first station has sent enough over a good channel to be in the fastest mode.
    fn warmed_up() -> Air {
        let mut air = Air::pair();
        air.snr = 20.0;
        for _ in 0..5 {
            air.links[0].send_text("warming up").unwrap();
        }
        air.run(ms(40_000));
        assert_eq!(
            modes(&air, 0),
            [Mode::R2, Mode::R3, Mode::F1, Mode::F2, Mode::F3]
        );
        air
    }

    #[test]
    fn the_mode_drops_at_once_when_the_advice_falls() {
        let mut air = warmed_up();
        air.snr = -8.0;
        air.links[0].send_text("the station moved away").unwrap();
        air.run(ms(20_000));
        assert_eq!(air.links[0].tx_mode(), Mode::R1);
        assert_eq!(modes(&air, 0).last(), Some(&Mode::R1));
    }

    #[test]
    fn a_frame_without_an_answer_is_repeated_one_mode_slower() {
        let mut air = warmed_up();
        // from now on the fastest two modes do not get through
        let mut attempts = 0;
        air.hears = Box::new(move |sender, _, frame| {
            if sender == 0 && frame.kind == Kind::Data {
                attempts += 1;
                return attempts >= 3;
            }
            true
        });
        let id = air.links[0].send_text("fading").unwrap();
        air.run(ms(30_000));
        assert!(air.events[0].contains(&LinkEvent::Delivered { id }));
        // F3 failed, F2 failed, F1 got through. Its acknowledgement advises
        // F3 again, but what has just failed is not tried again so soon.
        assert_eq!(modes(&air, 0)[5..], [Mode::F2, Mode::F1]);
        air.links[0].send_text("still F1").unwrap();
        air.run(ms(10_000));
        assert_eq!(air.links[0].tx_mode(), Mode::F1);
        // a minute later it is
        air.run(ms(60_000));
        air.links[0].send_text("up again").unwrap();
        air.run(ms(10_000));
        assert_eq!(modes(&air, 0)[5..], [Mode::F2, Mode::F1, Mode::F2]);
    }

    /// The data frames a station sent: frame number and payload length.
    fn data_frames(air: &Air, index: usize) -> Vec<(u8, usize)> {
        air.log
            .iter()
            .filter(|(sender, frame, _)| *sender == index && frame.kind == Kind::Data)
            .map(|(_, frame, _)| (frame.seq, frame.payload.len()))
            .collect()
    }

    #[test]
    fn faster_modes_carry_more_of_a_message_per_frame() {
        assert_eq!(
            Mode::ALL.map(frame_bytes),
            [64, 128, 255, 255, 1023, 1023, 1023]
        );
        let mut air = Air::pair();
        air.snr = 20.0;
        let long = "x".repeat(900);
        let id = air.links[0].send_text(&long).unwrap();
        air.run(ms(60_000));
        assert_eq!(air.events[1], [text(1, &long)]);
        assert!(air.events[0].contains(&LinkEvent::Delivered { id }));
        // a full frame in R1, in R2 and in R3, the rest in one frame of F1:
        // 63 bytes of the text, then 125, 252 and 460 after their offsets
        assert_eq!(
            data_frames(&air, 0),
            [(0, 64), (1, 128), (2, 255), (3, 463)]
        );
    }

    #[test]
    fn a_repeat_in_a_slower_mode_is_cut_to_the_size_of_that_mode() {
        let mut air = warmed_up();
        let before = data_frames(&air, 0).len();
        // the next three data frames are lost
        let mut losses = 3;
        air.hears = Box::new(move |sender, _, frame| {
            let lost = sender == 0 && frame.kind == Kind::Data && losses > 0;
            losses -= usize::from(lost);
            !lost
        });
        // only the tones get through from now on
        air.snr = 5.5;
        let long: String = (0..600)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let id = air.links[0].send_text(&long).unwrap();
        air.run(ms(60_000));
        assert_eq!(air.events[1].last(), Some(&text(1, &long)));
        assert!(air.events[0].contains(&LinkEvent::Delivered { id }));
        // One frame of F3 with the whole text; repeated as it is in F2 and
        // F1, which take as much; then cut for R3, the first piece under the
        // same number: 254 bytes, 252 after an offset, and the rest.
        assert_eq!(
            data_frames(&air, 0)[before..],
            [(5, 601), (5, 601), (5, 601), (5, 255), (6, 255), (7, 97)]
        );
    }

    #[test]
    fn a_text_cut_again_after_a_lost_acknowledgement_arrives_once() {
        let mut air = warmed_up();
        let before = air.events[1].len();
        // the next three acknowledgements are lost: the frame goes in F3, F2
        // and F1, then in pieces of R3
        let mut losses = 3;
        air.hears = Box::new(move |_, _, frame| {
            let lost = matches!(frame.kind, Kind::Ack { .. }) && losses > 0;
            losses -= usize::from(lost);
            !lost
        });
        let long: String = (0..600)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let id = air.links[0].send_text(&long).unwrap();
        let next = air.links[0].send_text("and one more").unwrap();
        air.run(ms(90_000));
        // the whole text was there with the first frame; the pieces that came after changed nothing
        assert_eq!(
            air.events[1][before..],
            [text(1, &long), text(1, "and one more")]
        );
        assert!(air.events[0].contains(&LinkEvent::Delivered { id }));
        assert!(air.events[0].contains(&LinkEvent::Delivered { id: next }));
        assert_eq!(
            data_frames(&air, 0)[5..9],
            [(5, 601), (5, 601), (5, 601), (5, 255)]
        );
    }

    #[test]
    fn pieces_that_overlap_what_was_received_fill_in_the_rest() {
        let mut air = Air::pair();
        air.snr = 5.5;
        // The acknowledgement of the third frame is lost: 252 bytes in R3
        // that the receiver has, sent again as 125 in R2 and so on.
        let mut acks = 0;
        air.hears = Box::new(move |_, _, frame| {
            acks += usize::from(matches!(frame.kind, Kind::Ack { .. }));
            !(matches!(frame.kind, Kind::Ack { .. }) && acks == 3)
        });
        let long: String = (0..700)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let id = air.links[0].send_text(&long).unwrap();
        air.run(ms(120_000));
        assert_eq!(air.events[1], [text(1, &long)]);
        assert!(air.events[0].contains(&LinkEvent::Delivered { id }));
        let frames = data_frames(&air, 0);
        assert_eq!(frames[..4], [(0, 64), (1, 128), (2, 255), (2, 128)]);
    }

    #[test]
    fn a_piece_from_beyond_what_was_received_drops_the_message() {
        let mut link = Link::new(config(1), ACK_LEN);
        let piece = |seq, first: u8, rest: &[u8]| Frame {
            kind: Kind::Data,
            station: 2,
            seq,
            payload: [&[first][..], rest].concat(),
        };
        link.on_frame(
            &piece(0, TYPE_TEXT | FLAG_FIRST | FLAG_MORE, b"abc"),
            QUIET,
            0,
        );
        // starts at byte 5 where only 3 are known: a frame is missing
        link.on_frame(&piece(1, TYPE_TEXT | FLAG_MORE, b"\0\x05fg"), QUIET, 0);
        link.on_frame(&piece(2, TYPE_TEXT, b"\0\x07h"), QUIET, 0);
        // too short to hold an offset
        link.on_frame(&piece(3, TYPE_TEXT, b"\0"), QUIET, 0);
        assert_eq!(link.poll_event(), None);
        link.on_frame(
            &piece(4, TYPE_TEXT | FLAG_FIRST | FLAG_MORE, b"abc"),
            QUIET,
            0,
        );
        link.on_frame(&piece(5, TYPE_TEXT, b"\0\x02cde"), QUIET, 0);
        assert_eq!(link.poll_event(), Some(text(2, "abcde")));
    }

    #[test]
    fn frame_numbers_start_again_after_63() {
        let mut air = Air::pair();
        air.snr = 8.0;
        let sent: Vec<String> = (0..70).map(|n| format!("text {n}")).collect();
        for text in &sent {
            air.links[0].send_text(text).unwrap();
        }
        air.run(ms(400_000));
        let received: Vec<LinkEvent> = sent.iter().map(|sent| text(1, sent)).collect();
        assert_eq!(air.events[1], received);
        let numbers: Vec<u8> = data_frames(&air, 0).iter().map(|&(seq, _)| seq).collect();
        assert_eq!(numbers[62..66], [62, 63, 0, 1]);
    }

    #[test]
    fn a_fixed_mode_is_kept_whatever_happens() {
        let mut air = Air::new(vec![
            LinkConfig {
                mode: Some(Mode::R3),
                ..config(1)
            },
            config(2),
        ]);
        air.snr = 30.0;
        let mut losses = 2;
        air.hears = Box::new(move |_, _, frame| {
            let lost = frame.kind == Kind::Data && losses > 0;
            losses -= usize::from(lost);
            !lost
        });
        air.links[0].send_text("always R3").unwrap();
        air.links[0].send_text("again").unwrap();
        air.run(ms(40_000));
        assert_eq!(air.events[1].len(), 2);
        assert_eq!(modes(&air, 0), []);
        assert_eq!(air.links[0].tx_mode(), Mode::R3);
    }

    #[test]
    fn after_a_long_silence_the_mode_starts_from_the_slowest_again() {
        let mut air = warmed_up();
        air.run(ms(130_000));
        air.links[0].send_text("still there?").unwrap();
        air.run(ms(10_000));
        let all = modes(&air, 0);
        assert_eq!(all[all.len() - 2..], [Mode::R1, Mode::R2]);
    }
}

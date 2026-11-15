//! File transfer above the link layer: an offer, the consent of the
//! receiver, the content in chunks, a checksum at the end. Pure logic on
//! messages and a clock — no frames here; `modem.rs` hands the messages to
//! the link layer and brings back what became of them.
//!
//! A chunk is a message of its own, cut to one frame of the mode of the
//! moment, and is handed over only when the link has nothing else to send:
//! a text typed meanwhile goes first. A file that deflates goes deflated.
//!
//! Over a one-way channel nobody can consent or acknowledge: the file is
//! broadcast instead, as symbols of a fountain code (`fountain.rs`) until
//! the sender stops, and whoever listens collects them. See `docs/protocol.md`.

use std::collections::{HashSet, VecDeque};

use crate::SAMPLE_RATE;
use crate::crc::crc32;
use crate::fountain::{Decoder, Encoder, symbols_for};

/// The longest file name, bytes of UTF-8; a longer one is cut.
pub const MAX_FILE_NAME_BYTES: usize = 255;
/// The largest file, bytes: sizes and offsets are 32 bits on the wire.
pub const MAX_FILE_BYTES: usize = u32::MAX as usize;
/// Bytes of a message with a chunk before the content: the kind, the
/// transfer, the offset.
pub const CHUNK_HEADER: usize = 6;
/// Bytes of a message with a symbol before the symbol: the kind, the
/// transfer, the number of the symbol.
pub const SYMBOL_HEADER: usize = 4;
/// Bytes of an announce before the name.
pub const ANNOUNCE_HEADER: usize = 17;
/// The most pieces a broadcast file has: the work of solving for them grows
/// as their square.
pub const MAX_PIECES: usize = 256;
/// An announce goes before every this many symbols, for those who tune in late.
pub const ANNOUNCE_EVERY: u32 = 8;
/// A broadcast stops by itself after this many times the pieces of its file.
const CAST_TIMES: u32 = 10;

/// The first byte of every message; the number of the transfer follows.
const OP_OFFER: u8 = 0;
const OP_ACCEPT: u8 = 1;
const OP_DATA: u8 = 2;
/// The sender gives the transfer up.
const OP_ABORT: u8 = 3;
/// The receiver declines the offer or gives the transfer up.
const OP_REFUSE: u8 = 4;
/// A broadcast file: what it is and how its symbols are cut.
const OP_ANNOUNCE: u8 = 5;
/// A symbol of a broadcast file.
const OP_SYMBOL: u8 = 6;

/// How the content goes on the air: the bytes of the file, or deflated
/// (RFC 1951) when that is shorter.
const AS_IT_IS: u8 = 0;
const DEFLATE: u8 = 1;
/// The level of deflate: the smallest output. A file of a megabyte takes a
/// fraction of a second, its transfer hours.
const DEFLATE_LEVEL: u8 = 9;

/// Files offered or partly received that are kept at once.
const MAX_DOWNLOADS: usize = 8;
/// The time left is judged by this many last chunks.
const PACE_CHUNKS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileEvent {
    /// A station offers a file: answer with `accept` or `cancel`.
    Offered {
        id: u32,
        from: u16,
        name: String,
        size: u32,
    },
    /// The other side has `done` bytes of the file (sending), or this side
    /// has (receiving). The time left is unknown before the first chunk.
    Progress {
        id: u32,
        done: u32,
        seconds_left: Option<u32>,
    },
    /// The whole file is here and its checksum is right.
    Received {
        id: u32,
        name: String,
        bytes: Vec<u8>,
    },
    /// The other side has the whole file.
    Delivered { id: u32 },
    /// Sending: the other side stopped answering. Receiving: the checksum
    /// of the content is wrong, or the offer gave way to newer ones.
    Failed { id: u32 },
    /// The other side declined the file or gave the transfer up.
    Refused { id: u32 },
    /// A station broadcasts a file: it is being collected.
    Heard {
        id: u32,
        from: u16,
        name: String,
        size: u32,
    },
    /// So many symbols of the file being broadcast went; `needed` of them
    /// make the file. Who has it, the sender never learns.
    Cast { id: u32, symbols: u32, needed: u32 },
    /// The broadcast ended by itself: ten times the file went.
    Sent { id: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileError {
    TooLarge { max: usize },
}

/// How fast a transfer goes: the time and the bytes done after the last few chunks.
struct Pace(VecDeque<(u64, usize)>);

impl Pace {
    fn start(now: u64, done: usize) -> Self {
        Self(VecDeque::from([(now, done)]))
    }

    /// Seconds until `size` bytes are done at the pace of the last chunks.
    fn seconds_left(&mut self, now: u64, done: usize, size: usize) -> Option<u32> {
        self.0.push_back((now, done));
        if self.0.len() > PACE_CHUNKS + 1 {
            self.0.pop_front();
        }
        let &(since, had) = self.0.front()?;
        let bytes = done.checked_sub(had).filter(|&bytes| bytes > 0)? as u64;
        let samples = size.saturating_sub(done) as u64 * now.saturating_sub(since) / bytes;
        Some(samples.div_ceil(u64::from(SAMPLE_RATE)) as u32)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Waits for the files before it.
    Queued,
    /// The offer is on its way.
    Offering,
    /// The offer arrived; the human on the other side decides.
    Offered,
    Sending,
}

fn deflate(bytes: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(bytes, DEFLATE_LEVEL)
}

/// What an offer tells of a file besides its name.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Shape {
    /// Length of the file and the CRC-32 of its content.
    size: usize,
    crc: u32,
    /// `AS_IT_IS` or `DEFLATE`.
    encoding: u8,
    /// Bytes of it on the air.
    sent: usize,
}

impl Shape {
    /// `done` bytes on the air as bytes of the file, for the progress.
    fn in_file(self, done: usize) -> u32 {
        if self.sent == 0 {
            return 0;
        }
        (done as u64 * self.size as u64 / self.sent as u64) as u32
    }

    /// The file from the bytes on the air, if they are what the offer said.
    fn decode(self, bytes: Vec<u8>) -> Option<Vec<u8>> {
        let content = match self.encoding {
            AS_IT_IS => bytes,
            DEFLATE => {
                miniz_oxide::inflate::decompress_to_vec_with_limit(&bytes, self.size).ok()?
            }
            _ => return None,
        };
        (content.len() == self.size && crc32(&content) == self.crc).then_some(content)
    }
}

/// A file this station sends.
struct Upload {
    id: u32,
    /// The number of the transfer on the wire.
    wire: u8,
    name: String,
    shape: Shape,
    /// What goes on the air.
    bytes: Vec<u8>,
    stage: Stage,
    /// Bytes on the air the other side has.
    done: usize,
    /// The message of this file the link has now: its token and, for a
    /// chunk, the byte of the file it ends at.
    flight: Option<(u32, usize)>,
    pace: Pace,
}

/// A file offered to this station or being received.
struct Download {
    id: u32,
    from: u16,
    wire: u8,
    name: String,
    shape: Shape,
    /// The bytes on the air received so far.
    bytes: Vec<u8>,
    accepted: bool,
    pace: Pace,
}

/// A file this station broadcasts.
struct Cast {
    id: u32,
    wire: u8,
    name: String,
    shape: Shape,
    encoder: Encoder,
    /// The number of the next symbol: so many went.
    next: u32,
    /// The symbols it stops after.
    last: u32,
    /// The announce before the current eight symbols went.
    announced: bool,
    /// The message of it the link has now, and whether it is a symbol.
    flight: Option<(u32, bool)>,
}

impl Cast {
    fn announce(&self) -> Vec<u8> {
        let mut bytes = message(OP_ANNOUNCE, self.wire, self.shape.size);
        bytes.extend(self.shape.crc.to_be_bytes());
        bytes.push(self.shape.encoding);
        bytes.extend((self.shape.sent as u32).to_be_bytes());
        bytes.extend((self.encoder.symbol_bytes() as u16).to_be_bytes());
        bytes.extend_from_slice(self.name.as_bytes());
        bytes
    }

    fn symbol(&self) -> Vec<u8> {
        let mut bytes = vec![OP_SYMBOL, self.wire];
        bytes.extend((self.next as u16).to_be_bytes());
        bytes.extend(self.encoder.symbol(self.next));
        bytes
    }
}

/// What an announce tells of a broadcast file.
struct Announced {
    name: String,
    shape: Shape,
    decoder: Decoder,
    pace: Pace,
}

/// A broadcast file being collected.
struct Catch {
    id: u32,
    from: u16,
    wire: u8,
    /// None until an announce is heard.
    announced: Option<Announced>,
    /// Symbols heard before the announce.
    early: Vec<(u32, Vec<u8>)>,
    /// The file is here, or the human gave it up: what comes is ignored.
    over: bool,
}

/// What the link has from here that `sent_as` is to name.
enum Polled {
    /// A chunk that ends at this byte on the air.
    Chunk(usize),
    /// A message of the broadcast; true for a symbol.
    Cast(bool),
}

fn message(op: u8, wire: u8, number: usize) -> Vec<u8> {
    let mut bytes = vec![op, wire];
    bytes.extend((number as u32).to_be_bytes());
    bytes
}

fn number(bytes: &[u8]) -> Option<(usize, &[u8])> {
    let (number, rest) = bytes.split_first_chunk::<4>()?;
    Some((u32::from_be_bytes(*number) as usize, rest))
}

#[derive(Default)]
pub struct Files {
    next_id: u32,
    next_wire: u8,
    /// The first is the one being sent; the others wait.
    uploads: VecDeque<Upload>,
    downloads: Vec<Download>,
    /// Broadcasts: the first goes now, the others wait.
    casts: VecDeque<Cast>,
    catches: Vec<Catch>,
    /// Answers and refusals waiting for the link.
    outbox: VecDeque<Vec<u8>>,
    /// What `poll` has just returned, until `sent_as` names its token.
    polled: Option<Polled>,
    /// Tokens of the answers and refusals the link has.
    control: HashSet<u32>,
    /// Messages to take back from the link.
    take_back: VecDeque<u32>,
    events: VecDeque<FileEvent>,
}

impl Files {
    pub fn new() -> Self {
        Self::default()
    }

    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    /// Queues a file; returns the id its fate is reported under. Nothing of
    /// its content is sent before the other side accepts the offer.
    pub fn send(&mut self, name: &str, bytes: Vec<u8>) -> Result<u32, FileError> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(FileError::TooLarge {
                max: MAX_FILE_BYTES,
            });
        }
        let mut end = name.len().min(MAX_FILE_NAME_BYTES);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        let id = self.id();
        let wire = self.next_wire;
        self.next_wire = wire.wrapping_add(1);
        let (size, crc) = (bytes.len(), crc32(&bytes));
        let deflated = deflate(&bytes);
        let (encoding, bytes) = if deflated.len() < size {
            (DEFLATE, deflated)
        } else {
            (AS_IT_IS, bytes)
        };
        self.uploads.push_back(Upload {
            id,
            wire,
            name: name[..end].to_owned(),
            shape: Shape {
                size,
                crc,
                encoding,
                sent: bytes.len(),
            },
            bytes,
            stage: Stage::Queued,
            done: 0,
            flight: None,
            pace: Pace::start(0, 0),
        });
        Ok(id)
    }

    /// Queues a file to broadcast in symbols of at most `symbol` bytes;
    /// returns the id its fate is reported under. Nobody is asked: it goes
    /// until it is cancelled or has gone ten times.
    pub fn broadcast(
        &mut self,
        name: &str,
        bytes: Vec<u8>,
        symbol: usize,
    ) -> Result<u32, FileError> {
        let symbol = symbol.clamp(1, usize::from(u16::MAX));
        let (size, crc) = (bytes.len(), crc32(&bytes));
        let deflated = deflate(&bytes);
        let (encoding, bytes) = if deflated.len() < size {
            (DEFLATE, deflated)
        } else {
            (AS_IT_IS, bytes)
        };
        let pieces = symbols_for(bytes.len(), symbol);
        if pieces > MAX_PIECES {
            return Err(FileError::TooLarge {
                max: MAX_PIECES * symbol,
            });
        }
        // The announce goes in one frame: as much of the name as fits next to a symbol.
        let mut end = name.len().min(symbol + SYMBOL_HEADER - ANNOUNCE_HEADER);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        let id = self.id();
        let wire = self.next_wire;
        self.next_wire = wire.wrapping_add(1);
        // As many pieces as frames take, as even as can be: no symbol is
        // longer, nor its frame, than the file needs.
        let encoder = Encoder::new(&bytes, bytes.len().div_ceil(pieces).max(1));
        let pieces = encoder.pieces() as u32;
        self.casts.push_back(Cast {
            id,
            wire,
            name: name[..end].to_owned(),
            shape: Shape {
                size,
                crc,
                encoding,
                sent: bytes.len(),
            },
            encoder,
            next: 0,
            last: (CAST_TIMES * pieces).min(1 << 16),
            announced: false,
            flight: None,
        });
        Ok(id)
    }

    /// Bytes the file `id`, queued to be sent, goes on the air as: fewer
    /// than it has when it deflates.
    pub fn sent_bytes(&self, id: u32) -> Option<usize> {
        let upload = self.uploads.iter().find(|upload| upload.id == id);
        let cast = self.casts.iter().find(|cast| cast.id == id);
        upload
            .map(|upload| upload.shape.sent)
            .or(cast.map(|cast| cast.shape.sent))
    }

    /// Consents to an offered file; false when there is no such offer.
    pub fn accept(&mut self, id: u32, now: u64) -> bool {
        let Some(index) = self
            .downloads
            .iter()
            .position(|download| download.id == id && !download.accepted)
        else {
            return false;
        };
        let download = &mut self.downloads[index];
        download.accepted = true;
        download.pace = Pace::start(now, download.bytes.len());
        self.outbox
            .push_back(message(OP_ACCEPT, download.wire, download.bytes.len()));
        self.finish_if_whole(index);
        true
    }

    /// Gives up a file, sent or received, or declines an offer, and tells
    /// the other side. Nothing more is reported about it. False when there
    /// is no such file any more.
    pub fn cancel(&mut self, id: u32) -> bool {
        // A broadcast tells nobody: nobody would hear an answer.
        if let Some(index) = self.casts.iter().position(|cast| cast.id == id) {
            let flight = self.casts.remove(index).and_then(|cast| cast.flight);
            self.take_back.extend(flight.map(|(token, _)| token));
            return true;
        }
        if let Some(catch) = self
            .catches
            .iter_mut()
            .find(|catch| catch.id == id && !catch.over)
        {
            catch.over = true;
            catch.announced = None;
            catch.early.clear();
            return true;
        }
        if let Some(index) = self.uploads.iter().position(|upload| upload.id == id) {
            let Some(upload) = self.uploads.remove(index) else {
                return false;
            };
            if upload.stage != Stage::Queued {
                self.take_back.extend(upload.flight.map(|(token, _)| token));
                self.outbox.push_back(vec![OP_ABORT, upload.wire]);
            }
            return true;
        }
        let Some(index) = self.downloads.iter().position(|d| d.id == id) else {
            return false;
        };
        let download = self.downloads.remove(index);
        self.outbox.push_back(vec![OP_REFUSE, download.wire]);
        true
    }

    /// The next message for the link, if any. `room` is the longest message
    /// one frame takes now; `free` — the link has nothing else to send, so a
    /// chunk may go. Name the token of the message with `sent_as` at once.
    pub fn poll(&mut self, room: usize, free: bool) -> Option<Vec<u8>> {
        self.polled = None;
        if let Some(bytes) = self.outbox.pop_front() {
            return Some(bytes);
        }
        let Some(upload) = self.uploads.front_mut() else {
            let cast = self
                .casts
                .front()
                .filter(|cast| free && cast.flight.is_none())?;
            let symbol = cast.announced;
            self.polled = Some(Polled::Cast(symbol));
            return Some(if symbol {
                cast.symbol()
            } else {
                cast.announce()
            });
        };
        match upload.stage {
            Stage::Queued => {
                upload.stage = Stage::Offering;
                self.polled = Some(Polled::Chunk(0));
                let shape = upload.shape;
                let mut bytes = message(OP_OFFER, upload.wire, shape.size);
                bytes.extend(shape.crc.to_be_bytes());
                bytes.push(shape.encoding);
                bytes.extend((shape.sent as u32).to_be_bytes());
                bytes.extend_from_slice(upload.name.as_bytes());
                Some(bytes)
            }
            Stage::Sending if free && upload.flight.is_none() => {
                let end = (upload.done + room.saturating_sub(CHUNK_HEADER).max(1))
                    .min(upload.bytes.len());
                self.polled = Some(Polled::Chunk(end));
                let mut bytes = message(OP_DATA, upload.wire, upload.done);
                bytes.extend_from_slice(&upload.bytes[upload.done..end]);
                Some(bytes)
            }
            _ => None,
        }
    }

    /// The link knows the message `poll` has just returned under this token.
    pub fn sent_as(&mut self, token: u32) {
        match (
            self.polled.take(),
            self.uploads.front_mut(),
            self.casts.front_mut(),
        ) {
            (Some(Polled::Chunk(end)), Some(upload), _) => upload.flight = Some((token, end)),
            (Some(Polled::Cast(symbol)), _, Some(cast)) => cast.flight = Some((token, symbol)),
            _ => {
                self.control.insert(token);
            }
        }
    }

    /// Nothing to send: no answer, no file offered, sent or broadcast.
    pub fn is_idle(&self) -> bool {
        self.outbox.is_empty() && self.uploads.is_empty() && self.casts.is_empty()
    }

    /// A message the link should take back: it is of no use any more.
    pub fn poll_take_back(&mut self) -> Option<u32> {
        self.take_back.pop_front()
    }

    pub fn poll_event(&mut self) -> Option<FileEvent> {
        self.events.pop_front()
    }

    /// Whether the link has this message from here.
    pub fn owns(&self, token: u32) -> bool {
        self.control.contains(&token)
            || self
                .uploads
                .front()
                .is_some_and(|upload| upload.flight.is_some_and(|(flying, _)| flying == token))
            || self
                .casts
                .front()
                .is_some_and(|cast| cast.flight.is_some_and(|(flying, _)| flying == token))
    }

    /// A message of the broadcast went; false when it is not one from here.
    fn on_cast(&mut self, token: u32) -> bool {
        let Some(cast) = self.casts.front_mut() else {
            return false;
        };
        let Some((_, symbol)) = cast.flight.take_if(|(flying, _)| *flying == token) else {
            return false;
        };
        if !symbol {
            cast.announced = true;
            return true;
        }
        cast.next += 1;
        cast.announced = !cast.next.is_multiple_of(ANNOUNCE_EVERY);
        let (id, symbols) = (cast.id, cast.next);
        self.events.push_back(FileEvent::Cast {
            id,
            symbols,
            needed: cast.encoder.pieces() as u32,
        });
        if symbols >= cast.last {
            self.casts.pop_front();
            self.events.push_back(FileEvent::Sent { id });
        }
        true
    }

    /// The other side has the message; false when it is not one from here.
    pub fn on_delivered(&mut self, token: u32, now: u64) -> bool {
        if self.control.remove(&token) || self.on_cast(token) {
            return true;
        }
        let Some(upload) = self.uploads.front_mut() else {
            return false;
        };
        let Some((_, end)) = upload.flight.take_if(|(flying, _)| *flying == token) else {
            return false;
        };
        match upload.stage {
            Stage::Offering => upload.stage = Stage::Offered,
            Stage::Sending => {
                upload.done = end;
                let (id, sent) = (upload.id, upload.bytes.len());
                if end < sent {
                    let seconds_left = upload.pace.seconds_left(now, end, sent);
                    self.events.push_back(FileEvent::Progress {
                        id,
                        done: upload.shape.in_file(end),
                        seconds_left,
                    });
                } else {
                    self.uploads.pop_front();
                    self.events.push_back(FileEvent::Delivered { id });
                }
            }
            Stage::Queued | Stage::Offered => {}
        }
        true
    }

    /// The link gave the message up; false when it is not one from here.
    pub fn on_failed(&mut self, token: u32) -> bool {
        // An answer that did not get through: the human sees the transfer
        // stand still and cancels it.
        if self.control.remove(&token) {
            return true;
        }
        if !self.owns(token) {
            return false;
        }
        if let Some(upload) = self.uploads.pop_front() {
            self.events.push_back(FileEvent::Failed { id: upload.id });
        }
        true
    }

    /// A message of the file transfer came from the station `from`.
    pub fn on_message(&mut self, from: u16, bytes: &[u8], now: u64) {
        let [op, wire, rest @ ..] = bytes else {
            return;
        };
        let wire = *wire;
        match *op {
            OP_OFFER => {
                if let Some((size, rest)) = number(rest)
                    && let Some((crc, rest)) = rest.split_first_chunk::<4>()
                    && let Some((&encoding, rest)) = rest.split_first()
                    && let Some((sent, name)) = number(rest)
                {
                    let shape = Shape {
                        size,
                        crc: u32::from_be_bytes(*crc),
                        encoding,
                        sent,
                    };
                    if matches!(encoding, AS_IT_IS | DEFLATE) {
                        let name = String::from_utf8_lossy(name).into_owned();
                        self.on_offer(from, wire, name, shape, now);
                    } else {
                        // From a later version: this one could not read it.
                        self.outbox.push_back(vec![OP_REFUSE, wire]);
                    }
                }
            }
            OP_ACCEPT => {
                if let Some((offset, _)) = number(rest) {
                    self.on_accept(wire, offset, now);
                }
            }
            OP_DATA => {
                if let Some((offset, content)) = number(rest) {
                    self.on_data(from, wire, offset, content, now);
                }
            }
            OP_ABORT => {
                if let Some(index) = self.download(from, wire) {
                    let id = self.downloads.remove(index).id;
                    self.events.push_back(FileEvent::Refused { id });
                }
            }
            OP_ANNOUNCE => {
                if let Some((size, rest)) = number(rest)
                    && let Some((crc, rest)) = rest.split_first_chunk::<4>()
                    && let Some((&encoding, rest)) = rest.split_first()
                    && let Some((sent, rest)) = number(rest)
                    && let Some((symbol, name)) = rest.split_first_chunk::<2>()
                {
                    let shape = Shape {
                        size,
                        crc: u32::from_be_bytes(*crc),
                        encoding,
                        sent,
                    };
                    let name = String::from_utf8_lossy(name).into_owned();
                    let symbol = usize::from(u16::from_be_bytes(*symbol));
                    self.on_announce(from, wire, name, shape, symbol, now);
                }
            }
            OP_SYMBOL => {
                if let Some((number, bytes)) = rest.split_first_chunk::<2>() {
                    self.on_symbol(
                        from,
                        wire,
                        u32::from(u16::from_be_bytes(*number)),
                        bytes,
                        now,
                    );
                }
            }
            OP_REFUSE => {
                if let Some(upload) = self
                    .uploads
                    .pop_front_if(|upload| upload.wire == wire && upload.stage != Stage::Queued)
                {
                    self.take_back.extend(upload.flight.map(|(token, _)| token));
                    self.events.push_back(FileEvent::Refused { id: upload.id });
                }
            }
            // A message from a later version.
            _ => {}
        }
    }

    /// The broadcast `wire` of the station `from`, made when first heard of.
    fn catch(&mut self, from: u16, wire: u8) -> &mut Catch {
        if let Some(index) = self
            .catches
            .iter()
            .position(|catch| (catch.from, catch.wire) == (from, wire))
        {
            return &mut self.catches[index];
        }
        if self.catches.len() >= MAX_DOWNLOADS {
            let oldest = self.catches.remove(0);
            if !oldest.over && oldest.announced.is_some() {
                self.events.push_back(FileEvent::Failed { id: oldest.id });
            }
        }
        let id = self.id();
        self.catches.push(Catch {
            id,
            from,
            wire,
            announced: None,
            early: Vec::new(),
            over: false,
        });
        self.catches.last_mut().expect("just pushed")
    }

    fn on_announce(
        &mut self,
        from: u16,
        wire: u8,
        name: String,
        shape: Shape,
        symbol: usize,
        now: u64,
    ) {
        let pieces = symbols_for(shape.sent, symbol.max(1));
        if symbol == 0 || pieces > MAX_PIECES || !matches!(shape.encoding, AS_IT_IS | DEFLATE) {
            return;
        }
        let catch = self.catch(from, wire);
        if catch.over || catch.announced.is_some() {
            return;
        }
        let mut decoder = Decoder::new(pieces, symbol);
        for (number, bytes) in catch.early.drain(..) {
            decoder.add(number, &bytes);
        }
        let (id, rank) = (catch.id, decoder.rank());
        catch.announced = Some(Announced {
            name: name.clone(),
            shape,
            decoder,
            pace: Pace::start(now, rank),
        });
        self.events.push_back(FileEvent::Heard {
            id,
            from,
            name,
            size: shape.size as u32,
        });
        if rank > 0 {
            self.events.push_back(FileEvent::Progress {
                id,
                done: shape.in_file((rank * symbol).min(shape.sent)),
                seconds_left: None,
            });
        }
        self.finish_catch(from, wire);
    }

    fn on_symbol(&mut self, from: u16, wire: u8, number: u32, bytes: &[u8], now: u64) {
        let catch = self.catch(from, wire);
        if catch.over {
            return;
        }
        let id = catch.id;
        let Some(announced) = &mut catch.announced else {
            if catch.early.len() < MAX_PIECES {
                catch.early.push((number, bytes.to_vec()));
            }
            return;
        };
        if !announced.decoder.add(number, bytes) {
            return;
        }
        let (rank, shape) = (announced.decoder.rank(), announced.shape);
        let pieces = symbols_for(shape.sent, bytes.len());
        if rank < pieces {
            let seconds_left = announced.pace.seconds_left(now, rank, pieces);
            self.events.push_back(FileEvent::Progress {
                id,
                done: shape.in_file((rank * bytes.len()).min(shape.sent)),
                seconds_left,
            });
        }
        self.finish_catch(from, wire);
    }

    /// The broadcast file is there once every piece is known.
    fn finish_catch(&mut self, from: u16, wire: u8) {
        let Some(catch) = self
            .catches
            .iter_mut()
            .find(|catch| (catch.from, catch.wire) == (from, wire))
        else {
            return;
        };
        let Some(announced) = &catch.announced else {
            return;
        };
        let Some(mut bytes) = announced.decoder.decoded() else {
            return;
        };
        let Announced { name, shape, .. } = catch.announced.take().expect("checked above");
        catch.over = true;
        bytes.truncate(shape.sent);
        let id = catch.id;
        self.events.push_back(match shape.decode(bytes) {
            Some(bytes) => FileEvent::Received { id, name, bytes },
            None => FileEvent::Failed { id },
        });
    }

    fn download(&self, from: u16, wire: u8) -> Option<usize> {
        self.downloads
            .iter()
            .position(|download| (download.from, download.wire) == (from, wire))
    }

    fn on_offer(&mut self, from: u16, wire: u8, name: String, shape: Shape, now: u64) {
        // The same file again: its sender gave up and starts anew. What is
        // here already is not asked for twice.
        if let Some(download) = self
            .downloads
            .iter_mut()
            .find(|d| d.name == name && d.shape == shape)
        {
            (download.from, download.wire) = (from, wire);
            if download.accepted {
                download.pace = Pace::start(now, download.bytes.len());
                self.outbox
                    .push_back(message(OP_ACCEPT, wire, download.bytes.len()));
            }
            return;
        }
        if self.downloads.len() >= MAX_DOWNLOADS {
            let id = self.downloads.remove(0).id;
            self.events.push_back(FileEvent::Failed { id });
        }
        let id = self.id();
        self.downloads.push(Download {
            id,
            from,
            wire,
            name: name.clone(),
            shape,
            bytes: Vec::new(),
            accepted: false,
            pace: Pace::start(now, 0),
        });
        self.events.push_back(FileEvent::Offered {
            id,
            from,
            name,
            size: shape.size as u32,
        });
    }

    /// The receiver wants the file from this byte on: at the start of a
    /// transfer, or when a chunk came from beyond what it has.
    fn on_accept(&mut self, wire: u8, offset: usize, now: u64) {
        let Some(upload) = self.uploads.front_mut().filter(|upload| {
            upload.wire == wire && upload.stage != Stage::Queued && offset <= upload.bytes.len()
        }) else {
            return;
        };
        // The offer whose acknowledgement was lost, or a chunk the receiver has no use for.
        self.take_back
            .extend(upload.flight.take().map(|(token, _)| token));
        upload.stage = Stage::Sending;
        upload.done = offset;
        upload.pace = Pace::start(now, offset);
        let id = upload.id;
        if offset < upload.bytes.len() {
            self.events.push_back(FileEvent::Progress {
                id,
                done: upload.shape.in_file(offset),
                seconds_left: None,
            });
        } else {
            self.uploads.pop_front();
            self.events.push_back(FileEvent::Delivered { id });
        }
    }

    fn on_data(&mut self, from: u16, wire: u8, offset: usize, content: &[u8], now: u64) {
        let Some(index) = self.download(from, wire) else {
            // Nobody waits for this file here: its sender should stop.
            self.outbox.push_back(vec![OP_REFUSE, wire]);
            return;
        };
        let download = &mut self.downloads[index];
        if !download.accepted {
            return;
        }
        let have = download.bytes.len();
        let Some(known) = have.checked_sub(offset) else {
            self.outbox.push_back(message(OP_ACCEPT, wire, have));
            return;
        };
        let new = content.get(known..).unwrap_or_default();
        let new = &new[..new.len().min(download.shape.sent - have)];
        if new.is_empty() {
            return;
        }
        download.bytes.extend_from_slice(new);
        let (id, done, shape) = (download.id, download.bytes.len(), download.shape);
        if done < shape.sent {
            let seconds_left = download.pace.seconds_left(now, done, shape.sent);
            self.events.push_back(FileEvent::Progress {
                id,
                done: shape.in_file(done),
                seconds_left,
            });
        }
        self.finish_if_whole(index);
    }

    fn finish_if_whole(&mut self, index: usize) {
        let download = &self.downloads[index];
        if !download.accepted || download.bytes.len() < download.shape.sent {
            return;
        }
        let Download {
            id,
            name,
            shape,
            bytes,
            ..
        } = self.downloads.remove(index);
        self.events.push_back(match shape.decode(bytes) {
            Some(bytes) => FileEvent::Received { id, name, bytes },
            None => FileEvent::Failed { id },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: u64 = SAMPLE_RATE as u64;

    /// Bytes that do not deflate: they go on the air as they are.
    fn content(len: usize) -> Vec<u8> {
        let mut seed = 1u32;
        (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect()
    }

    /// Decides whether a message of a side (0 or 1) is lost for good.
    type Lost = Box<dyn FnMut(usize, &[u8]) -> bool>;

    /// Two stations whose link delivers every message in a second, in order.
    struct Pair {
        files: [Files; 2],
        now: u64,
        next_token: u32,
        room: usize,
        lost: Lost,
        /// One-way: the sender is told every message went, lost or not.
        one_way: bool,
        /// Every message handed to the link: the side and the bytes.
        log: Vec<(usize, Vec<u8>)>,
        taken_back: Vec<(usize, u32)>,
        events: [Vec<FileEvent>; 2],
    }

    impl Pair {
        fn new() -> Self {
            Self {
                files: [Files::new(), Files::new()],
                now: 0,
                next_token: 100,
                room: 106,
                lost: Box::new(|_, _| false),
                one_way: false,
                log: Vec::new(),
                taken_back: Vec::new(),
                events: [Vec::new(), Vec::new()],
            }
        }

        /// One message of a side through the link; false when it has none.
        fn step(&mut self, side: usize) -> bool {
            while let Some(token) = self.files[side].poll_take_back() {
                self.taken_back.push((side, token));
            }
            let message = self.files[side].poll(self.room, true);
            if let Some(bytes) = &message {
                let token = self.next_token;
                self.next_token += 1;
                self.files[side].sent_as(token);
                assert!(self.files[side].owns(token));
                self.now += SECOND;
                self.log.push((side, bytes.clone()));
                let lost = (self.lost)(side, bytes);
                if !lost {
                    self.files[1 - side].on_message(side as u16 + 1, bytes, self.now);
                }
                if lost && !self.one_way {
                    assert!(self.files[side].on_failed(token));
                } else {
                    assert!(self.files[side].on_delivered(token, self.now));
                }
                assert!(!self.files[side].owns(token));
            }
            for side in 0..2 {
                while let Some(event) = self.files[side].poll_event() {
                    self.events[side].push(event);
                }
            }
            message.is_some()
        }

        /// Until both sides have nothing more to say.
        fn run(&mut self) {
            while self.step(0) | self.step(1) {}
        }

        /// The messages of this kind a side handed to the link.
        fn sent(&self, side: usize, op: u8) -> Vec<&[u8]> {
            self.log
                .iter()
                .filter(|(from, bytes)| *from == side && bytes[0] == op)
                .map(|(_, bytes)| &bytes[..])
                .collect()
        }

        /// A file of `len` bytes offered by side 0 and accepted by side 1;
        /// returns its ids on the two sides.
        fn accepted(&mut self, len: usize) -> (u32, u32) {
            let up = self.files[0].send("notes.txt", content(len)).unwrap();
            self.run();
            let Some(&FileEvent::Offered { id: down, .. }) = self.events[1].last() else {
                panic!("no offer: {:?}", self.events[1]);
            };
            assert!(self.files[1].accept(down, self.now));
            (up, down)
        }
    }

    fn progress(id: u32, done: u32, seconds_left: u32) -> FileEvent {
        FileEvent::Progress {
            id,
            done,
            seconds_left: Some(seconds_left),
        }
    }

    #[test]
    fn nothing_but_the_offer_is_sent_before_the_receiver_consents() {
        let mut pair = Pair::new();
        let id = pair.files[0].send("notes.txt", content(250)).unwrap();
        pair.run();
        assert_eq!(pair.log.len(), 1);
        let offer = &pair.log[0].1;
        assert_eq!(offer[..2], [OP_OFFER, 0]);
        assert_eq!(offer[2..6], 250u32.to_be_bytes());
        assert_eq!(offer[6..10], crc32(&content(250)).to_be_bytes());
        // it does not deflate: it goes as it is, as long as it is
        assert_eq!(offer[10], AS_IT_IS);
        assert_eq!(offer[11..15], 250u32.to_be_bytes());
        assert_eq!(&offer[15..], b"notes.txt");
        assert_eq!(pair.events[0], []);
        assert_eq!(
            pair.events[1],
            [FileEvent::Offered {
                id: 1,
                from: 1,
                name: "notes.txt".into(),
                size: 250
            }]
        );
        assert_eq!(id, 1);
    }

    #[test]
    fn an_accepted_file_arrives_in_chunks_and_both_sides_follow_it() {
        let mut pair = Pair::new();
        let (up, down) = pair.accepted(250);
        pair.run();
        assert_eq!(pair.sent(1, OP_ACCEPT), [[OP_ACCEPT, 0, 0, 0, 0, 0]]);
        let chunks = pair.sent(0, OP_DATA);
        assert_eq!(
            chunks.iter().map(|c| c.len()).collect::<Vec<_>>(),
            [106, 106, 56]
        );
        assert_eq!(chunks[1][..6], [OP_DATA, 0, 0, 0, 0, 100]);
        // a chunk a second: the sender counts from the consent, the receiver from its own
        assert_eq!(
            pair.events[0],
            [
                FileEvent::Progress {
                    id: up,
                    done: 0,
                    seconds_left: None
                },
                progress(up, 100, 2),
                progress(up, 200, 1),
                FileEvent::Delivered { id: up },
            ]
        );
        assert_eq!(
            pair.events[1][1..],
            [
                progress(down, 100, 3),
                progress(down, 200, 1),
                FileEvent::Received {
                    id: down,
                    name: "notes.txt".into(),
                    bytes: content(250)
                },
            ]
        );
    }

    #[test]
    fn the_time_left_follows_the_last_chunks() {
        let mut pair = Pair::new();
        let (up, _) = pair.accepted(3000);
        for _ in 0..12 {
            pair.step(1);
            pair.step(0);
        }
        // the link got ten times slower: ten seconds a chunk from now on
        for _ in 0..PACE_CHUNKS {
            pair.now += 9 * SECOND;
            pair.step(0);
        }
        let Some(&FileEvent::Progress {
            id,
            done,
            seconds_left,
        }) = pair.events[0].last()
        else {
            panic!("{:?}", pair.events[0]);
        };
        assert_eq!((id, done), (up, 2000));
        assert_eq!(seconds_left, Some(100));
    }

    #[test]
    fn chunks_fill_the_frame_of_the_mode_and_wait_for_other_messages() {
        let mut pair = Pair::new();
        pair.accepted(3000);
        pair.step(1);
        let files = &mut pair.files[0];
        // a text is waiting in the link
        assert_eq!(files.poll(63, false), None);
        assert_eq!(files.poll(63, true).map(|chunk| chunk.len()), Some(63));
        files.sent_as(7);
        // one chunk at a time
        assert_eq!(files.poll(63, true), None);
        assert!(files.on_delivered(7, 0));
        // the mode got faster meanwhile
        let chunk = files.poll(1022, true).unwrap();
        assert_eq!(chunk.len(), 1022);
        assert_eq!(chunk[2..6], 57u32.to_be_bytes());
        assert_eq!(chunk[6..], content(3000)[57..57 + 1016]);
    }

    #[test]
    fn a_declined_offer_is_refused_at_the_sender() {
        let mut pair = Pair::new();
        let up = pair.files[0].send("notes.txt", content(250)).unwrap();
        pair.run();
        assert!(pair.files[1].cancel(1));
        assert!(!pair.files[1].cancel(1));
        pair.run();
        assert_eq!(pair.sent(1, OP_REFUSE), [[OP_REFUSE, 0]]);
        assert_eq!(pair.events[0], [FileEvent::Refused { id: up }]);
        assert_eq!(pair.sent(0, OP_DATA).len(), 0);
        // too late to accept
        assert!(!pair.files[1].accept(1, 0));
    }

    #[test]
    fn the_receiver_gives_up_a_transfer() {
        let mut pair = Pair::new();
        let (up, down) = pair.accepted(1000);
        pair.step(1);
        pair.step(0);
        pair.step(0);
        assert!(pair.files[1].cancel(down));
        pair.run();
        // one more chunk was on its way when the refusal came
        assert_eq!(pair.sent(0, OP_DATA).len(), 3);
        assert_eq!(pair.events[0].last(), Some(&FileEvent::Refused { id: up }));
        assert!(!pair.files[0].cancel(up));
    }

    #[test]
    fn the_sender_gives_up_a_transfer_and_takes_its_chunk_back() {
        let mut pair = Pair::new();
        let (up, down) = pair.accepted(1000);
        pair.step(1);
        pair.step(0);
        // the next chunk is with the link when the human cancels
        pair.files[0].poll(106, true).unwrap();
        pair.files[0].sent_as(55);
        assert!(pair.files[0].cancel(up));
        pair.run();
        assert_eq!(pair.taken_back, [(0, 55)]);
        assert_eq!(pair.sent(0, OP_ABORT), [[OP_ABORT, 0]]);
        assert_eq!(
            pair.events[1].last(),
            Some(&FileEvent::Refused { id: down })
        );
        assert_eq!(pair.events[0].len(), 2, "{:?}", pair.events[0]);
        // what was received is gone: the same file is a new offer
        pair.files[0].send("notes.txt", content(1000)).unwrap();
        pair.run();
        assert!(matches!(
            pair.events[1].last(),
            Some(FileEvent::Offered { .. })
        ));
    }

    #[test]
    fn a_file_waiting_for_its_turn_is_cancelled_silently() {
        let mut pair = Pair::new();
        pair.files[0].send("first", content(10)).unwrap();
        let second = pair.files[0].send("second", content(10)).unwrap();
        assert!(pair.files[0].cancel(second));
        pair.run();
        assert_eq!(pair.log.len(), 1);
    }

    #[test]
    fn files_go_one_after_another() {
        let mut pair = Pair::new();
        let first = pair.files[0].send("first", content(150)).unwrap();
        let second = pair.files[0].send("second", content(50)).unwrap();
        pair.run();
        // the second is not offered while the first waits for its answer
        assert_eq!(pair.sent(0, OP_OFFER).len(), 1);
        assert!(pair.files[1].accept(1, 0));
        pair.run();
        assert!(pair.events[0].contains(&FileEvent::Delivered { id: first }));
        assert_eq!(pair.sent(0, OP_OFFER).len(), 2);
        let Some(&FileEvent::Offered { id, .. }) = pair.events[1].last() else {
            panic!("{:?}", pair.events[1]);
        };
        assert!(pair.files[1].accept(id, 0));
        pair.run();
        assert_eq!(
            pair.events[0].last(),
            Some(&FileEvent::Delivered { id: second })
        );
        assert_eq!(
            pair.events[1].last(),
            Some(&FileEvent::Received {
                id,
                name: "second".into(),
                bytes: content(50)
            })
        );
        // the second transfer has its own number on the wire
        assert_eq!(pair.sent(0, OP_DATA).last().unwrap()[1], 1);
    }

    #[test]
    fn a_broken_transfer_goes_on_from_where_it_stopped() {
        let mut pair = Pair::new();
        let (up, down) = pair.accepted(1000);
        let mut chunks = 0;
        pair.lost = Box::new(move |_, bytes| {
            chunks += usize::from(bytes[0] == OP_DATA);
            bytes[0] == OP_DATA && chunks == 4
        });
        pair.run();
        assert_eq!(pair.events[0].last(), Some(&FileEvent::Failed { id: up }));
        let offers = pair.events[1].len();

        // the human sends the same file again
        let again = pair.files[0].send("notes.txt", content(1000)).unwrap();
        pair.run();
        // no question to the receiver: it has agreed already
        assert_eq!(
            pair.sent(1, OP_ACCEPT).last().unwrap()[..],
            [OP_ACCEPT, 1, 0, 0, 1, 44]
        );
        assert_eq!(pair.sent(0, OP_DATA).len(), 4 + 7);
        assert_eq!(
            pair.events[0][pair.events[0].len() - 8..][..2],
            [
                FileEvent::Progress {
                    id: again,
                    done: 300,
                    seconds_left: None
                },
                progress(again, 400, 6),
            ]
        );
        assert_eq!(
            pair.events[0].last(),
            Some(&FileEvent::Delivered { id: again })
        );
        assert!(
            pair.events[1][offers..]
                .iter()
                .all(|e| !matches!(e, FileEvent::Offered { .. }))
        );
        assert_eq!(
            pair.events[1].last(),
            Some(&FileEvent::Received {
                id: down,
                name: "notes.txt".into(),
                bytes: content(1000)
            })
        );
    }

    #[test]
    fn an_offer_repeated_before_the_answer_is_not_shown_twice() {
        let mut pair = Pair::new();
        let first = pair.files[0].send("notes.txt", content(100)).unwrap();
        pair.run();
        pair.files[0].cancel(first);
        // the note that it was given up is lost
        pair.lost = Box::new(|_, bytes| bytes[0] == OP_ABORT);
        pair.run();
        pair.lost = Box::new(|_, _| false);
        let second = pair.files[0].send("notes.txt", content(100)).unwrap();
        pair.run();
        assert_eq!(pair.events[1].len(), 1);
        assert!(pair.files[1].accept(1, 0));
        pair.run();
        assert_eq!(
            pair.events[0].last(),
            Some(&FileEvent::Delivered { id: second })
        );
    }

    #[test]
    fn consent_that_overtakes_the_acknowledgement_of_the_offer_starts_the_transfer() {
        let mut pair = Pair::new();
        let files = &mut pair.files[0];
        let up = files.send("notes.txt", content(100)).unwrap();
        files.poll(106, true).unwrap();
        files.sent_as(9);
        files.on_message(2, &[OP_ACCEPT, 0, 0, 0, 0, 0], 0);
        assert_eq!(files.poll_take_back(), Some(9));
        assert!(!files.owns(9));
        pair.run();
        assert_eq!(
            pair.events[0].last(),
            Some(&FileEvent::Delivered { id: up })
        );
    }

    #[test]
    fn a_chunk_from_beyond_what_is_here_asks_for_the_missing_part() {
        let mut pair = Pair::new();
        let (_, down) = pair.accepted(1000);
        pair.step(1);
        pair.step(0);
        let mut far = message(OP_DATA, 0, 500);
        far.extend_from_slice(&content(1000)[500..600]);
        pair.files[1].on_message(1, &far, 0);
        pair.run();
        assert_eq!(
            pair.sent(1, OP_ACCEPT).last().unwrap()[..],
            [OP_ACCEPT, 0, 0, 0, 0, 100]
        );
        assert_eq!(
            pair.events[1].last(),
            Some(&FileEvent::Received {
                id: down,
                name: "notes.txt".into(),
                bytes: content(1000)
            })
        );
    }

    #[test]
    fn a_chunk_that_overlaps_what_is_here_adds_the_rest() {
        let mut pair = Pair::new();
        let (_, down) = pair.accepted(150);
        pair.step(1);
        pair.step(0);
        let mut overlapping = message(OP_DATA, 0, 60);
        overlapping.extend_from_slice(&content(150)[60..]);
        // more than the file has: the rest is not taken
        overlapping.extend_from_slice(b"junk");
        pair.files[1].on_message(1, &overlapping, 0);
        assert_eq!(
            pair.files[1].poll_event(),
            Some(FileEvent::Received {
                id: down,
                name: "notes.txt".into(),
                bytes: content(150)
            })
        );
    }

    #[test]
    fn a_chunk_nobody_waits_for_is_refused() {
        let mut pair = Pair::new();
        let (up, _) = pair.accepted(1000);
        pair.step(1);
        pair.step(0);
        // the receiver was restarted
        pair.files[1] = Files::new();
        pair.run();
        assert_eq!(pair.sent(0, OP_DATA).len(), 2);
        assert_eq!(pair.events[0].last(), Some(&FileEvent::Refused { id: up }));
    }

    /// An offer as another station would send it.
    fn offer(wire: u8, size: usize, crc: u32, encoding: u8, sent: usize) -> Vec<u8> {
        let mut offer = message(OP_OFFER, wire, size);
        offer.extend(crc.to_be_bytes());
        offer.push(encoding);
        offer.extend((sent as u32).to_be_bytes());
        offer.extend_from_slice(b"name");
        offer
    }

    /// What a receiver makes of an accepted offer and one chunk with all of the content.
    fn received(offer: &[u8], content: &[u8]) -> Option<FileEvent> {
        let mut files = Files::new();
        files.on_message(2, offer, 0);
        assert!(files.accept(1, 0));
        let mut chunk = message(OP_DATA, offer[1], 0);
        chunk.extend_from_slice(content);
        files.on_message(2, &chunk, 0);
        std::iter::from_fn(|| files.poll_event()).last()
    }

    #[test]
    fn content_with_a_wrong_checksum_is_not_delivered() {
        let offer = offer(3, 4, crc32(b"good"), AS_IT_IS, 4);
        assert_eq!(received(&offer, b"evil"), Some(FileEvent::Failed { id: 1 }));
    }

    /// Text that deflates well: lines of a log.
    fn log(lines: usize) -> Vec<u8> {
        (0..lines)
            .flat_map(|n| format!("{n:05} the modem heard a frame, {} dB\n", n % 17).into_bytes())
            .collect()
    }

    #[test]
    fn a_file_that_deflates_goes_deflated() {
        let mut pair = Pair::new();
        let content = log(300);
        let up = pair.files[0].send("modem.log", content.clone()).unwrap();
        pair.run();
        let offer = pair.sent(0, OP_OFFER)[0].to_vec();
        assert_eq!(offer[10], DEFLATE);
        let sent = u32::from_be_bytes(offer[11..15].try_into().unwrap()) as usize;
        assert_eq!(sent, deflate(&content).len());
        assert!(sent * 4 < content.len(), "{sent} of {}", content.len());
        assert!(pair.files[1].accept(1, pair.now));
        pair.run();
        // the chunks carry the deflated bytes
        let chunks: usize = pair
            .sent(0, OP_DATA)
            .iter()
            .map(|c| c.len() - CHUNK_HEADER)
            .sum();
        assert_eq!(chunks, sent);
        assert_eq!(
            pair.events[0].last(),
            Some(&FileEvent::Delivered { id: up })
        );
        assert_eq!(
            pair.events[1].last(),
            Some(&FileEvent::Received {
                id: 1,
                name: "modem.log".into(),
                bytes: content.clone()
            })
        );
        // progress counts bytes of the file, not of the deflated stream
        let done: Vec<u32> = pair.events[1]
            .iter()
            .filter_map(|e| match e {
                FileEvent::Progress { done, .. } => Some(*done),
                _ => None,
            })
            .collect();
        let first = 100 * content.len() / sent;
        assert!(
            done[0] as usize + 1 >= first && done[0] as usize <= first,
            "{done:?}"
        );
        assert!(done.windows(2).all(|pair| pair[0] < pair[1]));
        assert!((*done.last().unwrap() as usize) < content.len());
    }

    #[test]
    fn a_file_that_does_not_deflate_goes_as_it_is() {
        let mut pair = Pair::new();
        pair.files[0].send("photo.jpg", content(3000)).unwrap();
        pair.files[0].send("empty", Vec::new()).unwrap();
        pair.run();
        let offer = pair.sent(0, OP_OFFER)[0];
        assert_eq!(
            (offer[10], &offer[11..15]),
            (AS_IT_IS, &3000u32.to_be_bytes()[..])
        );
    }

    #[test]
    fn a_deflated_file_goes_on_after_a_break_too() {
        let mut pair = Pair::new();
        let content = log(1000);
        pair.files[0].send("modem.log", content.clone()).unwrap();
        pair.run();
        assert!(pair.files[1].accept(1, 0));
        let mut chunks = 0;
        pair.lost = Box::new(move |_, bytes| {
            chunks += usize::from(bytes[0] == OP_DATA);
            bytes[0] == OP_DATA && chunks == 3
        });
        pair.run();
        let sent_before = pair.sent(0, OP_DATA).len();
        pair.lost = Box::new(|_, _| false);
        pair.files[0].send("modem.log", content.clone()).unwrap();
        pair.run();
        // asked once, and the first two chunks were not sent again
        assert_eq!(pair.sent(0, OP_OFFER).len(), 2);
        let resumed_at = pair.sent(1, OP_ACCEPT).last().unwrap()[2..6].to_vec();
        assert_eq!(resumed_at, (2 * 100u32).to_be_bytes());
        assert!(pair.sent(0, OP_DATA).len() > sent_before);
        assert_eq!(
            pair.events[1].last(),
            Some(&FileEvent::Received {
                id: 1,
                name: "modem.log".into(),
                bytes: content
            })
        );
    }

    #[test]
    fn deflated_content_that_does_not_inflate_to_the_file_is_not_delivered() {
        let content = log(50);
        let crc = crc32(&content);
        let deflated = deflate(&content);
        // not a deflate stream
        let junk = vec![0xFF; deflated.len()];
        let bad = offer(0, content.len(), crc, DEFLATE, junk.len());
        assert_eq!(received(&bad, &junk), Some(FileEvent::Failed { id: 1 }));
        // more than the file: the receiver stops at its length
        let short = offer(0, content.len() - 1, crc, DEFLATE, deflated.len());
        assert_eq!(
            received(&short, &deflated),
            Some(FileEvent::Failed { id: 1 })
        );
        // and the right one
        let good = offer(0, content.len(), crc, DEFLATE, deflated.len());
        assert!(matches!(
            received(&good, &deflated),
            Some(FileEvent::Received { bytes, .. }) if bytes == content
        ));
    }

    #[test]
    fn an_encoding_from_a_later_version_is_declined() {
        let mut files = Files::new();
        files.on_message(2, &offer(4, 10, 0, 7, 10), 0);
        assert_eq!(files.poll_event(), None);
        assert_eq!(files.poll(106, true), Some(vec![OP_REFUSE, 4]));
    }

    #[test]
    fn an_empty_file_needs_no_chunks() {
        let mut pair = Pair::new();
        let (up, down) = pair.accepted(0);
        pair.run();
        assert_eq!(pair.sent(0, OP_DATA).len(), 0);
        assert_eq!(pair.events[0], [FileEvent::Delivered { id: up }]);
        assert_eq!(
            pair.events[1].last(),
            Some(&FileEvent::Received {
                id: down,
                name: "notes.txt".into(),
                bytes: vec![]
            })
        );
    }

    #[test]
    fn an_offer_without_an_answer_fails() {
        let mut pair = Pair::new();
        pair.lost = Box::new(|_, _| true);
        let up = pair.files[0].send("notes.txt", content(10)).unwrap();
        pair.run();
        assert_eq!(pair.events[0], [FileEvent::Failed { id: up }]);
        assert!(!pair.files[0].on_failed(100));
        assert!(!pair.files[0].on_delivered(100, 0));
    }

    #[test]
    fn a_long_name_is_cut_at_a_character() {
        let mut pair = Pair::new();
        pair.files[0].send(&"я".repeat(200), vec![1]).unwrap();
        pair.run();
        let Some(FileEvent::Offered { name, .. }) = pair.events[1].last() else {
            panic!("{:?}", pair.events[1]);
        };
        assert_eq!(*name, "я".repeat(127));
    }

    #[test]
    fn the_oldest_offer_gives_way_when_there_are_too_many() {
        let mut pair = Pair::new();
        for n in 0..=MAX_DOWNLOADS {
            let up = pair.files[0].send(&format!("file {n}"), vec![1]).unwrap();
            pair.run();
            // the sender does not wait for the answer
            pair.lost = Box::new(|_, _| true);
            pair.files[0].cancel(up);
            pair.run();
            pair.lost = Box::new(|_, _| false);
        }
        assert_eq!(pair.events[1].len(), MAX_DOWNLOADS + 2);
        assert_eq!(pair.events[1][MAX_DOWNLOADS], FileEvent::Failed { id: 1 });
        assert!(!pair.files[1].accept(1, 0));
        assert!(pair.files[1].accept(2, 0));
    }

    #[test]
    fn messages_that_make_no_sense_are_ignored() {
        let mut files = Files::new();
        for bytes in [
            &[][..],
            &[OP_OFFER],
            &[OP_OFFER, 0, 0, 0, 0, 1, 2, 3],
            &[OP_OFFER, 0, 0, 0, 0, 1, 2, 3, 4, 5, 0, 0, 0],
            &[OP_ACCEPT, 0, 0],
            &[OP_ACCEPT, 0, 0, 0, 0, 9],
            &[OP_DATA, 0, 1],
            &[OP_ABORT, 0],
            &[OP_REFUSE, 0],
            &[9, 0, 1, 2, 3, 4],
        ] {
            files.on_message(2, bytes, 0);
        }
        assert_eq!(files.poll_event(), None);
        assert_eq!(files.poll(106, true), None);
    }

    /// A pair on a one-way channel: side 0 broadcasts, side 1 listens.
    fn one_way() -> Pair {
        Pair {
            one_way: true,
            ..Pair::new()
        }
    }

    /// Bytes of a symbol in a message as long as one frame of the pair takes.
    const SYMBOL: usize = 106 - SYMBOL_HEADER;

    /// Side 0 broadcasts until the listener has an event like this, or `limit` messages went.
    fn broadcast_until(pair: &mut Pair, limit: usize, done: fn(&FileEvent) -> bool) {
        for _ in 0..limit {
            if pair.events[1].iter().any(done) {
                return;
            }
            pair.step(0);
        }
    }

    fn has_file(event: &FileEvent) -> bool {
        matches!(event, FileEvent::Received { .. })
    }

    #[test]
    fn a_broadcast_file_goes_without_an_offer_and_is_collected() {
        let mut pair = one_way();
        let up = pair.files[0]
            .broadcast("notes.txt", content(1000), SYMBOL)
            .unwrap();
        pair.step(0);
        let announce = pair.log[0].1.clone();
        assert_eq!(announce[..2], [OP_ANNOUNCE, 0]);
        assert_eq!(announce[2..6], 1000u32.to_be_bytes());
        assert_eq!(announce[6..10], crc32(&content(1000)).to_be_bytes());
        assert_eq!(announce[10], AS_IT_IS);
        assert_eq!(announce[11..15], 1000u32.to_be_bytes());
        // 1000 bytes take 10 symbols of at most 102: 10 pieces of 100
        assert_eq!(announce[15..17], 100u16.to_be_bytes());
        assert_eq!(&announce[17..], b"notes.txt");
        assert_eq!(
            pair.events[1],
            [FileEvent::Heard {
                id: 1,
                from: 1,
                name: "notes.txt".into(),
                size: 1000
            }]
        );
        broadcast_until(&mut pair, 100, has_file);
        // the announce again after eight
        let ops: Vec<u8> = pair.log.iter().map(|(_, bytes)| bytes[0]).collect();
        let mut expected = vec![OP_ANNOUNCE];
        expected.extend([OP_SYMBOL; 8]);
        expected.extend([OP_ANNOUNCE, OP_SYMBOL, OP_SYMBOL]);
        assert_eq!(ops, expected);
        // the first symbols are the pieces of the file
        let first = &pair.log[1].1;
        assert_eq!(first[..4], [OP_SYMBOL, 0, 0, 0]);
        assert_eq!(first[4..], content(1000)[..100]);
        assert_eq!(pair.log[2].1[..4], [OP_SYMBOL, 0, 0, 1]);
        // the listener follows the pieces it has
        let done: Vec<u32> = pair.events[1]
            .iter()
            .filter_map(|e| match e {
                FileEvent::Progress { done, .. } => Some(*done),
                _ => None,
            })
            .collect();
        assert_eq!(done, (1..10).map(|pieces| pieces * 100).collect::<Vec<_>>());
        assert_eq!(
            pair.events[1].last(),
            Some(&FileEvent::Received {
                id: 1,
                name: "notes.txt".into(),
                bytes: content(1000)
            })
        );
        // the sender says how much went; it never learns who has the file
        assert_eq!(
            pair.events[0].last(),
            Some(&FileEvent::Cast {
                id: up,
                symbols: 10,
                needed: 10
            })
        );
        let heard = pair.events[1].len();
        for _ in 0..20 {
            pair.step(0);
        }
        assert_eq!(pair.events[1].len(), heard);
        assert!(
            pair.events[0]
                .iter()
                .all(|e| matches!(e, FileEvent::Cast { .. }))
        );
    }

    #[test]
    fn symbols_lost_on_the_way_are_made_up_by_later_ones() {
        let mut pair = one_way();
        let mut count = 0;
        // every third message is lost, the announces among them
        pair.lost = Box::new(move |_, _| {
            count += 1;
            count % 3 == 0
        });
        pair.files[0]
            .broadcast("notes.txt", content(3000), SYMBOL)
            .unwrap();
        broadcast_until(&mut pair, 200, has_file);
        assert_eq!(
            pair.events[1].last(),
            Some(&FileEvent::Received {
                id: 1,
                name: "notes.txt".into(),
                bytes: content(3000)
            })
        );
        // 30 pieces; a third lost: some 45 symbols, a few announces, a symbol or two more
        let symbols = pair.sent(0, OP_SYMBOL).len();
        assert!((45..=50).contains(&symbols), "{symbols}");
    }

    #[test]
    fn a_listener_that_tunes_in_late_keeps_what_it_heard_before_the_announce() {
        let mut pair = one_way();
        let mut count = 0;
        // the announce and three symbols go by before it listens
        pair.lost = Box::new(move |_, _| {
            count += 1;
            count <= 4
        });
        pair.files[0]
            .broadcast("notes.txt", content(2000), SYMBOL)
            .unwrap();
        for _ in 0..9 {
            pair.step(0);
        }
        // five symbols heard, nothing known of the file yet
        assert_eq!(pair.events[1], []);
        pair.step(0);
        assert_eq!(
            pair.events[1][..2],
            [
                FileEvent::Heard {
                    id: 1,
                    from: 1,
                    name: "notes.txt".into(),
                    size: 2000
                },
                FileEvent::Progress {
                    id: 1,
                    done: 5 * 100,
                    seconds_left: None
                }
            ]
        );
        broadcast_until(&mut pair, 100, has_file);
        assert!(matches!(
            pair.events[1].last(),
            Some(FileEvent::Received { bytes, .. }) if *bytes == content(2000)
        ));
    }

    #[test]
    fn a_broadcast_is_stopped_by_its_sender_and_given_up_by_a_listener() {
        let mut pair = one_way();
        let up = pair.files[0]
            .broadcast("notes.txt", content(3000), SYMBOL)
            .unwrap();
        for _ in 0..4 {
            pair.step(0);
        }
        assert!(pair.files[1].cancel(1));
        // the listener hears more, and says nothing: there is nobody to tell
        for _ in 0..4 {
            pair.step(0);
        }
        assert_eq!(pair.events[1].len(), 1 + 3);
        assert_eq!(pair.files[1].poll(106, true), None);
        // the next symbol is with the link when the sender stops
        pair.files[0].poll(106, true).unwrap();
        pair.files[0].sent_as(77);
        assert!(pair.files[0].cancel(up));
        assert!(!pair.files[0].cancel(up));
        assert_eq!(pair.files[0].poll_take_back(), Some(77));
        let sent = pair.log.len();
        pair.run();
        assert_eq!(pair.log.len(), sent);
    }

    #[test]
    fn a_small_file_goes_in_symbols_no_longer_than_it() {
        let mut files = Files::new();
        files.broadcast("tiny", content(30), SYMBOL).unwrap();
        let announce = files.poll(106, true).unwrap();
        assert_eq!(announce[15..17], 30u16.to_be_bytes());
        files.sent_as(1);
        assert!(files.on_delivered(1, 0));
        assert_eq!(files.poll(106, true).unwrap().len(), SYMBOL_HEADER + 30);
        // pieces as even as can be: 205 bytes in three of 69, the last with a byte of padding
        let mut files = Files::new();
        files.broadcast("even", content(205), SYMBOL).unwrap();
        assert_eq!(files.poll(106, true).unwrap()[15..17], 69u16.to_be_bytes());
    }

    #[test]
    fn a_broadcast_ends_after_ten_times_the_file() {
        let mut pair = one_way();
        let up = pair.files[0]
            .broadcast("tiny", content(50), SYMBOL)
            .unwrap();
        pair.run();
        // one piece: ten symbols and an announce before every eight
        assert_eq!(pair.sent(0, OP_SYMBOL).len(), 10);
        assert_eq!(pair.sent(0, OP_ANNOUNCE).len(), 2);
        assert_eq!(pair.events[0].last(), Some(&FileEvent::Sent { id: up }));
        assert!(!pair.files[0].cancel(up));
    }

    #[test]
    fn files_broadcast_one_after_another() {
        let mut pair = one_way();
        pair.files[0]
            .broadcast("first", content(50), SYMBOL)
            .unwrap();
        pair.files[0].broadcast("second", log(20), SYMBOL).unwrap();
        pair.run();
        let names: Vec<&str> = pair.events[1]
            .iter()
            .filter_map(|e| match e {
                FileEvent::Received { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["first", "second"]);
        // the second has its own number and goes deflated
        let announces = pair.sent(0, OP_ANNOUNCE);
        let second = announces.last().unwrap();
        assert_eq!((second[1], second[10]), (1, DEFLATE));
    }

    #[test]
    fn a_broadcast_takes_at_most_256_pieces() {
        let mut files = Files::new();
        assert_eq!(
            files.broadcast("big", content(256 * SYMBOL + 1), SYMBOL),
            Err(FileError::TooLarge { max: 256 * SYMBOL })
        );
        assert!(
            files
                .broadcast("fits", content(256 * SYMBOL), SYMBOL)
                .is_ok()
        );
        // a long name is cut to what one frame takes
        let mut files = Files::new();
        files.broadcast(&"я".repeat(100), vec![1], SYMBOL).unwrap();
        let announce = files.poll(106, true).unwrap();
        assert_eq!(announce.len(), 17 + 88);
    }

    #[test]
    fn broadcast_messages_that_make_no_sense_are_ignored() {
        let mut files = Files::new();
        let announce = |pieces_of: usize, symbol: u16| {
            let mut bytes = message(OP_ANNOUNCE, 0, 10);
            bytes.extend(0u32.to_be_bytes());
            bytes.push(AS_IT_IS);
            bytes.extend((pieces_of as u32).to_be_bytes());
            bytes.extend(symbol.to_be_bytes());
            bytes
        };
        for bytes in [
            vec![OP_ANNOUNCE, 0, 1],
            vec![OP_SYMBOL, 0, 0],
            // symbols of no length, and too many pieces
            announce(10, 0),
            announce(257 * 4, 4),
        ] {
            files.on_message(2, &bytes, 0);
        }
        assert_eq!(files.poll_event(), None);
    }
}

//! File transfer above the link layer: an offer, the consent of the
//! receiver, the content in chunks, a checksum at the end. Pure logic on
//! messages and a clock — no frames here; `modem.rs` hands the messages to
//! the link layer and brings back what became of them.
//!
//! A chunk is a message of its own, cut to one frame of the mode of the
//! moment, and is handed over only when the link has nothing else to send:
//! a text typed meanwhile goes first. See `docs/protocol.md`.

use std::collections::{HashSet, VecDeque};

use crate::SAMPLE_RATE;
use crate::crc::crc32;

/// The longest file name, bytes of UTF-8; a longer one is cut.
pub const MAX_FILE_NAME_BYTES: usize = 255;
/// The largest file, bytes: sizes and offsets are 32 bits on the wire.
pub const MAX_FILE_BYTES: usize = u32::MAX as usize;
/// Bytes of a message with a chunk before the content: the kind, the
/// transfer, the offset.
pub const CHUNK_HEADER: usize = 6;

/// The first byte of every message; the number of the transfer follows.
const OP_OFFER: u8 = 0;
const OP_ACCEPT: u8 = 1;
const OP_DATA: u8 = 2;
/// The sender gives the transfer up.
const OP_ABORT: u8 = 3;
/// The receiver declines the offer or gives the transfer up.
const OP_REFUSE: u8 = 4;

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

/// A file this station sends.
struct Upload {
    id: u32,
    /// The number of the transfer on the wire.
    wire: u8,
    name: String,
    bytes: Vec<u8>,
    stage: Stage,
    /// Bytes the other side has.
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
    size: usize,
    crc: u32,
    bytes: Vec<u8>,
    accepted: bool,
    pace: Pace,
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
    /// Answers and refusals waiting for the link.
    outbox: VecDeque<Vec<u8>>,
    /// The end of the chunk `poll` has just returned, until `sent_as` names its token.
    polled: Option<usize>,
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
        self.uploads.push_back(Upload {
            id,
            wire,
            name: name[..end].to_owned(),
            bytes,
            stage: Stage::Queued,
            done: 0,
            flight: None,
            pace: Pace::start(0, 0),
        });
        Ok(id)
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
        let upload = self.uploads.front_mut()?;
        match upload.stage {
            Stage::Queued => {
                upload.stage = Stage::Offering;
                self.polled = Some(0);
                let mut bytes = message(OP_OFFER, upload.wire, upload.bytes.len());
                bytes.extend(crc32(&upload.bytes).to_be_bytes());
                bytes.extend_from_slice(upload.name.as_bytes());
                Some(bytes)
            }
            Stage::Sending if free && upload.flight.is_none() => {
                let end = (upload.done + room.saturating_sub(CHUNK_HEADER).max(1))
                    .min(upload.bytes.len());
                self.polled = Some(end);
                let mut bytes = message(OP_DATA, upload.wire, upload.done);
                bytes.extend_from_slice(&upload.bytes[upload.done..end]);
                Some(bytes)
            }
            _ => None,
        }
    }

    /// The link knows the message `poll` has just returned under this token.
    pub fn sent_as(&mut self, token: u32) {
        match (self.polled.take(), self.uploads.front_mut()) {
            (Some(end), Some(upload)) => upload.flight = Some((token, end)),
            _ => {
                self.control.insert(token);
            }
        }
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
    }

    /// The other side has the message; false when it is not one from here.
    pub fn on_delivered(&mut self, token: u32, now: u64) -> bool {
        if self.control.remove(&token) {
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
                let (id, size) = (upload.id, upload.bytes.len());
                if end < size {
                    let seconds_left = upload.pace.seconds_left(now, end, size);
                    self.events.push_back(FileEvent::Progress {
                        id,
                        done: end as u32,
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
                    && let Some((crc, name)) = rest.split_first_chunk::<4>()
                {
                    let name = String::from_utf8_lossy(name).into_owned();
                    self.on_offer(from, wire, name, size, u32::from_be_bytes(*crc), now);
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

    fn download(&self, from: u16, wire: u8) -> Option<usize> {
        self.downloads
            .iter()
            .position(|download| (download.from, download.wire) == (from, wire))
    }

    fn on_offer(&mut self, from: u16, wire: u8, name: String, size: usize, crc: u32, now: u64) {
        // The same file again: its sender gave up and starts anew. What is
        // here already is not asked for twice.
        if let Some(download) = self
            .downloads
            .iter_mut()
            .find(|d| d.name == name && d.size == size && d.crc == crc)
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
            size,
            crc,
            bytes: Vec::new(),
            accepted: false,
            pace: Pace::start(now, 0),
        });
        self.events.push_back(FileEvent::Offered {
            id,
            from,
            name,
            size: size as u32,
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
                done: offset as u32,
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
        let new = &new[..new.len().min(download.size - have)];
        if new.is_empty() {
            return;
        }
        download.bytes.extend_from_slice(new);
        let (id, done, size) = (download.id, download.bytes.len(), download.size);
        if done < size {
            let seconds_left = download.pace.seconds_left(now, done, size);
            self.events.push_back(FileEvent::Progress {
                id,
                done: done as u32,
                seconds_left,
            });
        }
        self.finish_if_whole(index);
    }

    fn finish_if_whole(&mut self, index: usize) {
        let download = &self.downloads[index];
        if !download.accepted || download.bytes.len() < download.size {
            return;
        }
        let Download {
            id,
            name,
            crc,
            bytes,
            ..
        } = self.downloads.remove(index);
        self.events.push_back(if crc32(&bytes) == crc {
            FileEvent::Received { id, name, bytes }
        } else {
            FileEvent::Failed { id }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: u64 = SAMPLE_RATE as u64;

    fn content(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 + i / 256) as u8).collect()
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
                if (self.lost)(side, bytes) {
                    assert!(self.files[side].on_failed(token));
                } else {
                    self.files[1 - side].on_message(side as u16 + 1, bytes, self.now);
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
        assert_eq!(&offer[10..], b"notes.txt");
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

    #[test]
    fn content_with_a_wrong_checksum_is_not_delivered() {
        let mut files = Files::new();
        let mut offer = message(OP_OFFER, 3, 4);
        offer.extend(crc32(b"good").to_be_bytes());
        offer.extend_from_slice(b"name");
        files.on_message(2, &offer, 0);
        assert!(files.accept(1, 0));
        let mut chunk = message(OP_DATA, 3, 0);
        chunk.extend_from_slice(b"evil");
        files.on_message(2, &chunk, 0);
        let events: Vec<_> = std::iter::from_fn(|| files.poll_event()).collect();
        assert_eq!(events.last(), Some(&FileEvent::Failed { id: 1 }));
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
}

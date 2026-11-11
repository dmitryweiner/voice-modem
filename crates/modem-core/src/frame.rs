//! Frames, modes and the header. See `docs/protocol.md`.

use crate::crc::crc16;
use crate::mfsk::MfskParams;

/// Version of the wire format, the two highest bits of the first header byte.
pub const PROTOCOL_VERSION: u8 = 0;

/// The largest payload of one frame, bytes (the length is one header byte).
pub const MAX_PAYLOAD: usize = 255;

/// Bytes of the header on the wire, CRC included.
pub const HEADER_BYTES: usize = 7;

/// How the payload of a frame is modulated and coded. All modes are tones
/// compared by energy, 32 ms symbols on a grid of 31.25 Hz from 750 Hz, with
/// the rate 1/2 convolutional code; they differ in how many tones sound at
/// once and how many sets of tones take turns. Faster modes give each tone
/// less power and the echo of a room less room.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    /// One tone of 16, four sets in turn; 62.5 bit/s. The most robust.
    R1,
    /// Four tones at once, each one of 4, four sets in turn; 125 bit/s.
    R2,
    /// Eight tones at once, each one of 4, two sets in turn; 250 bit/s.
    R3,
    /// Sixteen tones at once, each one of 4, one set; 500 bit/s.
    R4,
}

/// The mode of the header of every frame: the most robust one.
pub const HEADER_MODE: Mode = Mode::R1;

impl Mode {
    /// From the most robust to the fastest.
    pub const ALL: [Mode; 4] = [Mode::R1, Mode::R2, Mode::R3, Mode::R4];

    pub const fn id(self) -> u8 {
        self as u8
    }

    pub const fn from_id(id: u8) -> Option<Mode> {
        match id {
            0 => Some(Mode::R1),
            1 => Some(Mode::R2),
            2 => Some(Mode::R3),
            3 => Some(Mode::R4),
            _ => None,
        }
    }

    pub const fn mfsk(self) -> MfskParams {
        let (bits_per_tone, groups, sets) = match self {
            Mode::R1 => (4, 1, 4),
            Mode::R2 => (2, 4, 4),
            Mode::R3 => (2, 8, 2),
            Mode::R4 => (2, 16, 1),
        };
        MfskParams {
            bits_per_tone,
            groups,
            sets,
            symbol_len: 512,
            first_tone_hz: 750.0,
            spacing_hz: 31.25,
        }
    }

    /// Bits of payload per second, before the overhead of the frame.
    pub fn bit_rate(self) -> f32 {
        let mfsk = self.mfsk();
        mfsk.bits_per_symbol() as f32 * crate::SAMPLE_RATE as f32 / mfsk.symbol_len as f32 / 2.0
    }
}

/// What a frame is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Carries a payload and asks for an acknowledgement.
    Data,
    /// Acknowledges a data frame; the header alone, no payload. `advice` is
    /// the fastest mode its sender expects to receive from the station it
    /// answers, judged by how well the acknowledged frame was heard.
    Ack { advice: Mode },
    /// Carries a payload and expects no answer.
    Broadcast,
}

impl Kind {
    const fn id(self) -> u8 {
        match self {
            Kind::Data => 0,
            Kind::Ack { .. } => 1,
            Kind::Broadcast => 2,
        }
    }

    const fn from_id(id: u8) -> Option<Kind> {
        match id {
            0 => Some(Kind::Data),
            1 => Some(Kind::Ack { advice: Mode::R1 }),
            2 => Some(Kind::Broadcast),
            _ => None,
        }
    }

    /// Whether the header is followed by a payload block.
    pub const fn has_payload(self) -> bool {
        !matches!(self, Kind::Ack { .. })
    }
}

/// One frame as the link layer sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub kind: Kind,
    /// The sender of a data or broadcast frame; in an acknowledgement, the
    /// station whose frame is acknowledged.
    pub station: u16,
    /// Number of the frame, counted by its sender; in an acknowledgement,
    /// the number of the acknowledged frame.
    pub seq: u8,
    /// Empty in an acknowledgement.
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    pub mode: Mode,
    pub station: u16,
    pub seq: u8,
    /// Payload length in bytes; 0 in an acknowledgement.
    pub len: u8,
}

impl Header {
    pub fn to_bytes(self) -> [u8; HEADER_BYTES] {
        let [station_high, station_low] = self.station.to_be_bytes();
        let first = (PROTOCOL_VERSION << 6) | (self.kind.id() << 4) | self.mode.id();
        // An acknowledgement has no payload to measure: the byte carries its advice.
        let last = if let Kind::Ack { advice } = self.kind {
            advice.id()
        } else {
            self.len
        };
        let body = [first, station_high, station_low, self.seq, last];
        let [crc_high, crc_low] = crc16(&body).to_be_bytes();
        [
            body[0], body[1], body[2], body[3], body[4], crc_high, crc_low,
        ]
    }

    /// None when the checksum, the protocol version, the kind or the mode is not ours.
    pub fn from_bytes(bytes: &[u8]) -> Option<Header> {
        let &[
            first,
            station_high,
            station_low,
            seq,
            len,
            crc_high,
            crc_low,
        ] = bytes
        else {
            return None;
        };
        if crc16(&bytes[..5]) != u16::from_be_bytes([crc_high, crc_low])
            || first >> 6 != PROTOCOL_VERSION
        {
            return None;
        }
        let kind = match Kind::from_id((first >> 4) & 0x03)? {
            Kind::Ack { .. } => Kind::Ack {
                advice: Mode::from_id(len)?,
            },
            kind => kind,
        };
        Some(Header {
            kind,
            mode: Mode::from_id(first & 0x0F)?,
            station: u16::from_be_bytes([station_high, station_low]),
            seq,
            len: if kind.has_payload() { len } else { 0 },
        })
    }
}

#[cfg(test)]
// The first header byte is written as its fields: version, kind, mode.
#[allow(clippy::unusual_byte_groupings)]
mod tests {
    use super::*;

    const HEADER: Header = Header {
        kind: Kind::Broadcast,
        mode: Mode::R1,
        station: 0xBEEF,
        seq: 200,
        len: 17,
    };

    /// A header with the given first bytes and a correct checksum.
    fn with_body(body: [u8; 5]) -> Option<Header> {
        let [high, low] = crc16(&body).to_be_bytes();
        Header::from_bytes(&[body[0], body[1], body[2], body[3], body[4], high, low])
    }

    #[test]
    fn round_trip() {
        for kind in [Kind::Data, Kind::Broadcast] {
            let header = Header { kind, ..HEADER };
            assert_eq!(Header::from_bytes(&header.to_bytes()), Some(header));
        }
        let ack = Header {
            kind: Kind::Ack { advice: Mode::R3 },
            len: 0,
            ..HEADER
        };
        assert_eq!(Header::from_bytes(&ack.to_bytes()), Some(ack));
    }

    #[test]
    fn layout() {
        let bytes = HEADER.to_bytes();
        // version 0, kind 2 (broadcast), mode 0
        assert_eq!(bytes[..5], [0b00_10_0000, 0xBE, 0xEF, 200, 17]);
        assert_eq!(u16::from_be_bytes([bytes[5], bytes[6]]), crc16(&bytes[..5]));
    }

    #[test]
    fn rejects_any_flipped_bit() {
        let bytes = HEADER.to_bytes();
        for bit in 0..HEADER_BYTES * 8 {
            let mut damaged = bytes;
            damaged[bit / 8] ^= 1 << (bit % 8);
            assert_eq!(Header::from_bytes(&damaged), None, "bit {bit}");
        }
    }

    #[test]
    fn rejects_unknown_versions_kinds_and_modes() {
        assert!(with_body([0b00_00_0000, 0, 1, 2, 3]).is_some());
        assert_eq!(with_body([0b01_00_0000, 0, 1, 2, 3]), None);
        assert_eq!(with_body([0b00_11_0000, 0, 1, 2, 3]), None);
        assert_eq!(with_body([0b00_00_1111, 0, 1, 2, 3]), None);
    }

    #[test]
    fn an_acknowledgement_carries_advice_instead_of_a_length() {
        assert_eq!(
            with_body([0b00_01_0000, 0, 1, 2, 0]).map(|h| (h.kind, h.len)),
            Some((Kind::Ack { advice: Mode::R1 }, 0))
        );
        assert_eq!(
            with_body([0b00_01_0000, 0, 1, 2, 3]).map(|h| (h.kind, h.len)),
            Some((Kind::Ack { advice: Mode::R4 }, 0))
        );
        // not a mode
        assert_eq!(with_body([0b00_01_0000, 0, 1, 2, 4]), None);
        let ack = Kind::Ack { advice: Mode::R1 };
        assert!(!ack.has_payload() && Kind::Data.has_payload() && Kind::Broadcast.has_payload());
    }

    #[test]
    fn rejects_wrong_lengths() {
        assert_eq!(Header::from_bytes(&HEADER.to_bytes()[..6]), None);
        assert_eq!(Header::from_bytes(&[0; 8]), None);
    }

    #[test]
    fn mode_ids_round_trip() {
        for (id, mode) in Mode::ALL.into_iter().enumerate() {
            assert_eq!(mode.id(), id as u8);
            assert_eq!(Mode::from_id(mode.id()), Some(mode));
        }
        assert_eq!(Mode::from_id(4), None);
    }

    #[test]
    fn every_mode_fits_the_band_and_each_is_twice_as_fast_as_the_one_before() {
        assert_eq!(Mode::ALL.map(Mode::bit_rate), [62.5, 125.0, 250.0, 500.0]);
        for mode in Mode::ALL {
            let mfsk = mode.mfsk();
            assert_eq!(
                (mfsk.first_tone_hz, mfsk.last_tone_hz()),
                (750.0, 2718.75),
                "{mode:?}"
            );
        }
    }
}

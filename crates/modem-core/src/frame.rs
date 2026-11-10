//! Frame layout: modes and the header. See `docs/protocol.md`.

use crate::crc::crc16;
use crate::mfsk::MfskParams;

/// Version of the wire format, the high half of the first header byte.
pub const PROTOCOL_VERSION: u8 = 0;

/// The largest payload of one frame, bytes (the length is one header byte).
pub const MAX_PAYLOAD: usize = 255;

/// Bytes of the header on the wire, CRC included.
pub const HEADER_BYTES: usize = 5;

/// How the payload of a frame is modulated and coded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Robust: 16 tones 750–2625 Hz every 125 Hz, 32 ms symbols, rate 1/2
    /// convolutional code; 62.5 bit/s.
    R1,
}

/// The mode of the header of every frame: the most robust one.
pub const HEADER_MODE: Mode = Mode::R1;

impl Mode {
    pub const fn id(self) -> u8 {
        match self {
            Mode::R1 => 0,
        }
    }

    pub const fn from_id(id: u8) -> Option<Mode> {
        match id {
            0 => Some(Mode::R1),
            _ => None,
        }
    }

    pub const fn mfsk(self) -> MfskParams {
        match self {
            Mode::R1 => MfskParams {
                bits_per_symbol: 4,
                symbol_len: 512,
                first_tone_hz: 750.0,
                spacing_hz: 125.0,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub mode: Mode,
    /// Frame number, wraps around.
    pub seq: u8,
    /// Payload length in bytes.
    pub len: u8,
}

impl Header {
    pub fn to_bytes(self) -> [u8; HEADER_BYTES] {
        let body = [(PROTOCOL_VERSION << 4) | self.mode.id(), self.seq, self.len];
        let [high, low] = crc16(&body).to_be_bytes();
        [body[0], body[1], body[2], high, low]
    }

    /// None when the checksum, the protocol version or the mode is not ours.
    pub fn from_bytes(bytes: &[u8]) -> Option<Header> {
        let &[first, seq, len, high, low] = bytes else {
            return None;
        };
        if crc16(&bytes[..3]) != u16::from_be_bytes([high, low]) || first >> 4 != PROTOCOL_VERSION {
            return None;
        }
        Some(Header {
            mode: Mode::from_id(first & 0x0F)?,
            seq,
            len,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: Header = Header {
        mode: Mode::R1,
        seq: 200,
        len: 17,
    };

    #[test]
    fn round_trip() {
        assert_eq!(Header::from_bytes(&HEADER.to_bytes()), Some(HEADER));
    }

    #[test]
    fn layout() {
        let bytes = HEADER.to_bytes();
        assert_eq!(bytes[..3], [0x00, 200, 17]);
        assert_eq!(u16::from_be_bytes([bytes[3], bytes[4]]), crc16(&bytes[..3]));
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
    fn rejects_unknown_versions_and_modes() {
        let with_first = |first: u8| {
            let body = [first, 1, 2];
            let [high, low] = crc16(&body).to_be_bytes();
            Header::from_bytes(&[body[0], body[1], body[2], high, low])
        };
        assert!(with_first(0x00).is_some());
        assert_eq!(with_first(0x10), None);
        assert_eq!(with_first(0x0F), None);
    }

    #[test]
    fn rejects_wrong_lengths() {
        assert_eq!(Header::from_bytes(&HEADER.to_bytes()[..4]), None);
        assert_eq!(Header::from_bytes(&[0; 6]), None);
    }

    #[test]
    fn mode_ids_round_trip() {
        assert_eq!(Mode::from_id(Mode::R1.id()), Some(Mode::R1));
    }
}

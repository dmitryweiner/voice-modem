//! Reading and writing WAV files: just enough for mono recordings at the
//! sample rate of the core.

use modem_core::SAMPLE_RATE;

const PCM: u16 = 1;
const FLOAT: u16 = 3;
const EXTENSIBLE: u16 = 0xFFFE;

/// A 16-bit mono WAV file at the sample rate of the core.
pub fn encode(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    bytes.extend(b"RIFF");
    bytes.extend((36 + data_len).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(PCM.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(SAMPLE_RATE.to_le_bytes());
    bytes.extend((SAMPLE_RATE * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data_len.to_le_bytes());
    for &sample in samples {
        bytes.extend(((sample.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    bytes
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The samples of a WAV file: 16-bit or float, any number of channels (mixed
/// down to one), at the sample rate of the core only.
pub fn decode(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a WAV file".into());
    }
    let mut format = None;
    let mut at = 12;
    while let (Some(id), Some(len)) = (bytes.get(at..at + 4), u32_at(bytes, at + 4)) {
        let body = at + 8;
        // A streamed file may announce more data than it has: take what is there.
        let end = body.saturating_add(len as usize).min(bytes.len());
        match id {
            b"fmt " => {
                let truncated = || "truncated WAV header".to_string();
                let mut kind = u16_at(bytes, body).ok_or_else(truncated)?;
                let channels = usize::from(u16_at(bytes, body + 2).ok_or_else(truncated)?);
                let rate = u32_at(bytes, body + 4).ok_or_else(truncated)?;
                let bits = u16_at(bytes, body + 14).ok_or_else(truncated)?;
                if kind == EXTENSIBLE {
                    kind = u16_at(bytes, body + 24).ok_or_else(truncated)?;
                }
                if rate != SAMPLE_RATE {
                    return Err(format!(
                        "the sample rate is {rate} Hz, need {SAMPLE_RATE} (ffmpeg -i in.wav -ar {SAMPLE_RATE} out.wav)"
                    ));
                }
                if channels == 0 || !matches!((kind, bits), (PCM, 16) | (FLOAT, 32)) {
                    return Err(format!(
                        "unsupported WAV format: type {kind}, {bits} bits, {channels} channels"
                    ));
                }
                format = Some((kind, channels));
            }
            b"data" => {
                let (kind, channels) = format.ok_or("WAV data before the format")?;
                let values: Vec<f32> = if kind == PCM {
                    bytes[body..end]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
                        .collect()
                } else {
                    bytes[body..end]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| f32::from_le_bytes(*b))
                        .collect()
                };
                return Ok(values
                    .chunks_exact(channels)
                    .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                    .collect());
            }
            _ => {}
        }
        // chunks are padded to an even length
        at = end + (len as usize & 1);
    }
    Err("no audio data in the WAV file".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_within_16_bits() {
        let samples: Vec<f32> = (0..1000).map(|n| (n as f32 * 0.05).sin() * 0.9).collect();
        let decoded = decode(&encode(&samples)).unwrap();
        assert_eq!(decoded.len(), samples.len());
        assert!(
            decoded
                .iter()
                .zip(&samples)
                .all(|(a, b)| (a - b).abs() < 1.0 / 16384.0)
        );
    }

    #[test]
    fn header_layout() {
        let bytes = encode(&[0.0, 1.0, -1.0, 2.0]);
        assert_eq!(bytes.len(), 44 + 8);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(u32_at(&bytes, 4), Some(44));
        assert_eq!(u32_at(&bytes, 24), Some(16_000));
        assert_eq!(u32_at(&bytes, 40), Some(8));
        // full scale, clamped
        assert_eq!(bytes[44..], [0, 0, 0xFF, 0x7F, 0x01, 0x80, 0xFF, 0x7F]);
    }

    /// A WAV file with the given format and an extra chunk before the data.
    fn file(kind: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVELIST\x03\0\0\0abc\0fmt \x10\0\0\0".to_vec();
        bytes.extend(kind.to_le_bytes());
        bytes.extend(channels.to_le_bytes());
        bytes.extend(rate.to_le_bytes());
        bytes.extend([0; 6]);
        bytes.extend(bits.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend((data.len() as u32).to_le_bytes());
        bytes.extend(data);
        bytes
    }

    #[test]
    fn mixes_stereo_down_and_skips_other_chunks() {
        let data: Vec<u8> = [16384i16, 0, -16384, -16384]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(
            decode(&file(PCM, 2, 16_000, 16, &data)).unwrap(),
            [0.25, -0.5]
        );
    }

    #[test]
    fn reads_float_samples() {
        let data: Vec<u8> = [0.5f32, -0.25]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(
            decode(&file(FLOAT, 1, 16_000, 32, &data)).unwrap(),
            [0.5, -0.25]
        );
    }

    #[test]
    fn takes_what_is_there_when_the_length_lies() {
        let mut bytes = encode(&[0.5; 10]);
        bytes[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&bytes).unwrap().len(), 10);
    }

    #[test]
    fn explains_what_it_cannot_read() {
        assert_eq!(decode(b"OggS not a wav"), Err("not a WAV file".into()));
        assert!(
            decode(&file(PCM, 1, 8000, 16, &[]))
                .unwrap_err()
                .contains("8000 Hz, need 16000")
        );
        assert!(
            decode(&file(PCM, 1, 16_000, 24, &[]))
                .unwrap_err()
                .contains("unsupported")
        );
        assert!(
            decode(&encode(&[])[..36])
                .unwrap_err()
                .contains("no audio data")
        );
    }
}

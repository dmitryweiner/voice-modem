//! Voice Modem core: DSP, modulations, FEC, frames, link and transport layers.
//!
//! The core knows nothing about the platform: samples and commands in, samples
//! and events out. No threads, timers or system time — time is the number of
//! processed samples, so every run is deterministic. Pure Rust dependencies
//! only, so it builds for wasm32 and Android alike.
//!
//! The wire format is specified in `docs/protocol.md`.

pub mod bits;
pub mod coding;
pub mod conv;
pub mod crc;
pub mod frame;
pub mod ldpc;
pub mod link;
pub mod mfsk;
pub mod modem;
pub mod ofdm;
pub mod resample;
pub mod rx;
pub mod sync;
pub mod tx;

pub use frame::{Frame, Kind};
pub use frame::{MAX_PAYLOAD, Mode};
pub use link::{MAX_NAME_BYTES, MAX_TEXT_BYTES};
pub use modem::{CommandError, Config, ConfigError, Event, Modem};
pub use rx::{Receiver, RxEvent, RxStats};
pub use tx::{TxConfig, encode_frame};

/// Version of the core; shown in the UI and sent in the session hello.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The sample rate all signal processing runs at, Hz.
pub const SAMPLE_RATE: u32 = 16_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_major_minor_patch() {
        let parts: Vec<&str> = VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "{VERSION}");
        assert!(parts.iter().all(|p| p.parse::<u32>().is_ok()), "{VERSION}");
    }
}

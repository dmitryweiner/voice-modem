//! Voice Modem core: DSP, modulations, FEC, frames, link and transport layers.
//!
//! The core knows nothing about the platform: samples and commands in, samples
//! and events out. No threads, timers or system time — time is the number of
//! processed samples, so every run is deterministic. Pure Rust dependencies
//! only, so it builds for wasm32 and Android alike.

/// Version of the core; shown in the UI and sent in the session hello.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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

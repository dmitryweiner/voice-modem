//! What a file costs in each mode: the time a transfer takes once it is
//! accepted and no frame is lost, and the speed that makes — against the
//! speed of the mode itself. The difference is what stop-and-wait pays for
//! the preamble, the header and the acknowledgement of every frame.
//!
//!     cargo run --release -p modem-sim --example files
//!
//! Source of the table of file speeds in `docs/measurements.md`.

use modem_core::{Config, Mode, Modem};

fn main() {
    const SIZE: usize = 100_000;
    println!("mode  bit/s  10 KB, s  100 KB, min  net bit/s  share");
    for mode in Mode::ALL {
        let modem = Modem::new(Config {
            mode: Some(mode),
            ..Config::default()
        })
        .expect("the default configuration is valid");
        let seconds = modem.file_seconds(SIZE);
        let net = SIZE as f32 * 8.0 / seconds as f32;
        println!(
            "{mode:?}  {:>6}  {:>8}  {:>11.0}  {net:>9.0}  {:>4.0}%",
            mode.bit_rate(),
            modem.file_seconds(10_000),
            seconds as f32 / 60.0,
            100.0 * net / mode.bit_rate(),
        );
    }
}

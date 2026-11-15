//! What a file costs in each mode: the time a transfer takes once it is
//! accepted and no frame is lost, and the speed that makes — against the
//! speed of the mode itself. The difference is what stop-and-wait pays for
//! the preamble, the header and the acknowledgement of every frame.
//!
//! One way, broadcast: the time until every piece went once, which a
//! listener on a clean channel needs, and the largest file a broadcast takes.
//!
//!     cargo run --release -p modem-sim --example files
//!
//! Source of the tables of file speeds in `docs/measurements.md`.

use modem_core::files::{MAX_PIECES, SYMBOL_HEADER};
use modem_core::link::frame_bytes;
use modem_core::{Config, Mode, Modem};

fn main() {
    const SIZE: usize = 100_000;
    println!(
        "mode  bit/s  10 KB, s  100 KB, min  net bit/s  share | one way: 10 KB, s  net bit/s  largest, KB"
    );
    for mode in Mode::ALL {
        let modem = |reliable| {
            Modem::new(Config {
                mode: Some(mode),
                reliable,
                ..Config::default()
            })
            .expect("the default configuration is valid")
        };
        let (two_way, one_way) = (modem(true), modem(false));
        let seconds = two_way.file_seconds(SIZE);
        let net = SIZE as f32 * 8.0 / seconds as f32;
        let broadcast = one_way.file_seconds(10_000);
        // a symbol fills a frame but for the type of the message and its own header
        let largest = MAX_PIECES * (frame_bytes(mode) - 1 - SYMBOL_HEADER);
        println!(
            "{mode:?}  {:>6}  {:>8}  {:>11.0}  {net:>9.0}  {:>4.0}% | {broadcast:>16}  {:>9.0}  {:>11.0}",
            mode.bit_rate(),
            two_way.file_seconds(10_000),
            seconds as f32 / 60.0,
            100.0 * net / mode.bit_rate(),
            10_000.0 * 8.0 / broadcast as f32,
            largest as f32 / 1000.0,
        );
    }
}

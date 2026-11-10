//! `modem-cli`: see `modem_cli::USAGE`.

use std::process::ExitCode;

fn main() -> ExitCode {
    match modem_cli::parse(std::env::args().skip(1)).and_then(modem_cli::run) {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

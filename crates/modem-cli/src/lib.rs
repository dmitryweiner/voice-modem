//! The native tool of the modem core: text ⇄ WAV and channel simulation.

pub mod wav;

use std::fmt::Write as _;
use std::fs;

use modem_core::rx::seconds;
use modem_core::{Command, Config, Event, Modem, TxConfig, VERSION};
use modem_sim::{Awgn, BandPass, Chain, Channel, Clip, ClockOffset, FreqShift, rms};

pub const USAGE: &str = "\
modem-cli — the Voice Modem core from the command line

  modem-cli encode <text>... -o out.wav [--lead-in MS] [--amplitude 0..1]
      One frame per text (up to 255 bytes of UTF-8 each) into a 16 kHz WAV file.

  modem-cli decode in.wav
      Prints every message heard in a 16 kHz WAV file with the time, the
      signal-to-noise ratio (8 kHz band) and the frequency offset.
      Fails when there is none.

  modem-cli channel in.wav -o out.wav [--telephone] [--shift HZ] [--ppm N]
                                      [--clip LEVEL] [--snr DB] [--seed N]
      What a channel does to a recording, applied in this order: the
      300–3400 Hz band of a telephone, a frequency shift, a sound-card clock
      offset, clipping, white noise (SNR against the RMS of the whole file).

  modem-cli --version
";

#[derive(Clone, Debug, PartialEq)]
pub enum Cli {
    Encode {
        texts: Vec<String>,
        output: String,
        lead_in_ms: u32,
        amplitude: f32,
    },
    Decode {
        input: String,
    },
    Channel {
        input: String,
        output: String,
        telephone: bool,
        shift_hz: f32,
        ppm: f32,
        clip: Option<f32>,
        snr_db: Option<f32>,
        seed: u64,
    },
    Help,
    Version,
}

/// The value of an option, parsed.
fn value<T: std::str::FromStr>(
    option: &str,
    args: &mut impl Iterator<Item = String>,
) -> Result<T, String> {
    let text = args.next().ok_or(format!("{option} needs a value"))?;
    text.parse()
        .map_err(|_| format!("{option}: cannot read {text:?}"))
}

fn one(what: &str, mut items: Vec<String>) -> Result<String, String> {
    if items.len() == 1 {
        Ok(items.remove(0))
    } else {
        Err(format!("need exactly one {what}"))
    }
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Cli, String> {
    let mut args = args.into_iter();
    let command = args.next().unwrap_or_default();
    match command.as_str() {
        "" | "help" | "--help" | "-h" => return Ok(Cli::Help),
        "--version" | "-V" => return Ok(Cli::Version),
        "encode" | "decode" | "channel" => {}
        other => return Err(format!("unknown command {other:?}")),
    }

    let mut positional = Vec::new();
    let mut output = None;
    let (mut lead_in_ms, mut amplitude) = (0, TxConfig::default().amplitude);
    let (mut telephone, mut shift_hz, mut ppm, mut clip, mut snr_db, mut seed) =
        (false, 0.0, 0.0, None, None, 1);
    while let Some(arg) = args.next() {
        let commands: &[&str] = match arg.as_str() {
            "-o" | "--output" => &["encode", "channel"],
            "--lead-in" | "--amplitude" => &["encode"],
            "--telephone" | "--shift" | "--ppm" | "--clip" | "--snr" | "--seed" => &["channel"],
            // A text may not start with "--"; a negative number is the value of an option.
            option if option.starts_with("--") => return Err(format!("unknown option {option}")),
            _ => {
                positional.push(arg);
                continue;
            }
        };
        if !commands.contains(&command.as_str()) {
            return Err(format!("{command} has no option {arg}"));
        }
        match arg.as_str() {
            "-o" | "--output" => output = Some(value::<String>(&arg, &mut args)?),
            "--lead-in" => lead_in_ms = value(&arg, &mut args)?,
            "--amplitude" => amplitude = value(&arg, &mut args)?,
            "--telephone" => telephone = true,
            "--shift" => shift_hz = value(&arg, &mut args)?,
            "--ppm" => ppm = value(&arg, &mut args)?,
            "--clip" => clip = Some(value(&arg, &mut args)?),
            "--snr" => snr_db = Some(value(&arg, &mut args)?),
            _ => seed = value(&arg, &mut args)?,
        }
    }

    let output = output.ok_or("need -o <file.wav>".to_string());
    Ok(match command.as_str() {
        "encode" => {
            if positional.is_empty() {
                return Err("need at least one text".into());
            }
            if !(amplitude > 0.0 && amplitude <= 1.0) {
                return Err("--amplitude must be above 0 and at most 1".into());
            }
            Cli::Encode {
                texts: positional,
                output: output?,
                lead_in_ms,
                amplitude,
            }
        }
        "decode" => Cli::Decode {
            input: one("input file", positional)?,
        },
        _ => Cli::Channel {
            input: one("input file", positional)?,
            output: output?,
            telephone,
            shift_hz,
            ppm,
            clip,
            snr_db,
            seed,
        },
    })
}

/// The samples of the given texts, one frame each.
pub fn encode(texts: &[String], tx: TxConfig) -> Result<Vec<f32>, String> {
    let mut modem = Modem::new(Config {
        tx,
        ..Config::default()
    })
    .map_err(|e| format!("{e:?}"))?;
    for text in texts {
        modem
            .command(Command::SendText(text.clone()))
            .map_err(|_| {
                format!(
                    "the text is {} bytes long, a frame takes {}: {text:?}",
                    text.len(),
                    modem_core::MAX_PAYLOAD
                )
            })?;
    }
    let mut samples = vec![0.0; modem.tx_pending()];
    modem.pull_tx(&mut samples);
    Ok(samples)
}

/// A report of what is heard in the samples, a line per frame, and the number of messages.
pub fn decode(samples: &[f32]) -> (String, usize) {
    let mut modem = Modem::new(Config::default()).expect("the default configuration is valid");
    modem.push_rx(samples);
    let (mut report, mut messages) = (String::new(), 0);
    while let Some(event) = modem.poll_event() {
        let (stats, text) = match event {
            Event::MessageReceived { text, stats } => {
                messages += 1;
                (stats, text)
            }
            Event::FrameLost { stats } => (stats, "(damaged frame)".to_string()),
        };
        let time = seconds(stats.start as usize);
        writeln!(
            report,
            "{time:8.2} s  {:+5.1} dB  {:+6.1} Hz  {text}",
            stats.snr_db, stats.freq_offset_hz
        )
        .expect("writing to a string");
    }
    (report, messages)
}

fn read(path: &str) -> Result<Vec<f32>, String> {
    wav::decode(&fs::read(path).map_err(|e| format!("{path}: {e}"))?)
        .map_err(|e| format!("{path}: {e}"))
}

fn write(path: &str, samples: &[f32]) -> Result<(), String> {
    fs::write(path, wav::encode(samples)).map_err(|e| format!("{path}: {e}"))
}

/// Runs a command; the text is what to print.
pub fn run(cli: Cli) -> Result<String, String> {
    match cli {
        Cli::Help => Ok(USAGE.to_string()),
        Cli::Version => Ok(format!("modem-core {VERSION}\n")),
        Cli::Encode {
            texts,
            output,
            lead_in_ms,
            amplitude,
        } => {
            let samples = encode(
                &texts,
                TxConfig {
                    amplitude,
                    lead_in_ms,
                },
            )?;
            write(&output, &samples)?;
            Ok(format!(
                "{output}: {} frame(s), {:.1} s\n",
                texts.len(),
                seconds(samples.len())
            ))
        }
        Cli::Decode { input } => match decode(&read(&input)?) {
            (report, 0) => Err(format!("{report}{input}: no messages")),
            (report, _) => Ok(report),
        },
        Cli::Channel {
            input,
            output,
            telephone,
            shift_hz,
            ppm,
            clip,
            snr_db,
            seed,
        } => {
            let samples = read(&input)?;
            let mut channel = Chain::new();
            if telephone {
                channel = channel.then(BandPass::telephone());
            }
            if shift_hz != 0.0 {
                channel = channel.then(FreqShift(shift_hz));
            }
            if ppm != 0.0 {
                channel = channel.then(ClockOffset { ppm });
            }
            if let Some(level) = clip {
                channel = channel.then(Clip(level));
            }
            if let Some(snr_db) = snr_db {
                channel = channel.then(Awgn::with_snr(rms(&samples), snr_db, seed));
            }
            let heard = channel.process(&samples);
            write(&output, &heard)?;
            Ok(format!("{output}: {:.1} s\n", seconds(heard.len())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    fn temp(name: &str) -> String {
        std::env::temp_dir()
            .join(format!("modem-cli-test-{}-{name}", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn parses_encode() {
        let cli = parse(
            [
                "encode",
                "hello there",
                "second",
                "-o",
                "a.wav",
                "--lead-in",
                "300",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(
            cli,
            Cli::Encode {
                texts: vec!["hello there".into(), "second".into()],
                output: "a.wav".into(),
                lead_in_ms: 300,
                amplitude: 0.5
            }
        );
    }

    #[test]
    fn parses_decode_and_channel() {
        assert_eq!(
            parse(args("decode in.wav")),
            Ok(Cli::Decode {
                input: "in.wav".into()
            })
        );
        assert_eq!(
            parse(args(
                "channel in.wav -o out.wav --snr -6 --shift -40.5 --ppm 100 --telephone --clip 0.3 --seed 9"
            )),
            Ok(Cli::Channel {
                input: "in.wav".into(),
                output: "out.wav".into(),
                telephone: true,
                shift_hz: -40.5,
                ppm: 100.0,
                clip: Some(0.3),
                snr_db: Some(-6.0),
                seed: 9
            })
        );
        assert_eq!(
            parse(args("channel in.wav -o out.wav")),
            Ok(Cli::Channel {
                input: "in.wav".into(),
                output: "out.wav".into(),
                telephone: false,
                shift_hz: 0.0,
                ppm: 0.0,
                clip: None,
                snr_db: None,
                seed: 1
            })
        );
    }

    #[test]
    fn help_and_version() {
        assert_eq!(parse(args("")), Ok(Cli::Help));
        assert_eq!(parse(args("--help")), Ok(Cli::Help));
        assert_eq!(parse(args("--version")), Ok(Cli::Version));
        assert!(run(Cli::Help).unwrap().contains("modem-cli encode"));
        assert_eq!(
            run(Cli::Version).unwrap(),
            format!("modem-core {VERSION}\n")
        );
    }

    #[test]
    fn explains_wrong_arguments() {
        for (line, message) in [
            ("transmit x", "unknown command \"transmit\""),
            ("encode -o a.wav", "need at least one text"),
            ("encode hi", "need -o <file.wav>"),
            ("encode hi -o", "-o needs a value"),
            (
                "encode hi -o a.wav --lead-in soon",
                "--lead-in: cannot read \"soon\"",
            ),
            (
                "encode hi -o a.wav --amplitude 1.5",
                "--amplitude must be above 0 and at most 1",
            ),
            ("encode hi -o a.wav --snr 3", "encode has no option --snr"),
            ("decode", "need exactly one input file"),
            ("decode a.wav b.wav", "need exactly one input file"),
            ("decode a.wav --loud", "unknown option --loud"),
            ("channel a.wav", "need -o <file.wav>"),
        ] {
            assert_eq!(parse(args(line)), Err(message.to_string()), "{line}");
        }
    }

    #[test]
    fn text_goes_through_a_file_and_a_channel() {
        let (clean, noisy) = (temp("clean.wav"), temp("noisy.wav"));
        let texts = vec!["Привет!".to_string(), "over a noisy line".to_string()];
        let printed = run(Cli::Encode {
            texts,
            output: clean.clone(),
            lead_in_ms: 200,
            amplitude: 0.5,
        })
        .unwrap();
        assert_eq!(printed, format!("{clean}: 2 frame(s), 7.6 s\n"));

        run(Cli::Channel {
            input: clean.clone(),
            output: noisy.clone(),
            telephone: true,
            shift_hz: 50.0,
            ppm: -80.0,
            clip: Some(0.4),
            snr_db: Some(-8.0),
            seed: 4,
        })
        .unwrap();
        let report = run(Cli::Decode {
            input: noisy.clone(),
        })
        .unwrap();
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(lines.len(), 2, "{report}");
        assert!(
            lines[0].ends_with("Hz  Привет!") && lines[1].ends_with("Hz  over a noisy line"),
            "{report}"
        );
        assert!(lines[0].trim_start().starts_with("0.20 s"), "{report}");
        assert!(lines[0].contains("+50."), "{report}");

        for path in [clean, noisy] {
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn decode_fails_when_nothing_is_heard() {
        let silent = temp("silent.wav");
        fs::write(&silent, wav::encode(&[0.0; 32_000])).unwrap();
        assert_eq!(
            run(Cli::Decode {
                input: silent.clone()
            }),
            Err(format!("{silent}: no messages"))
        );
        fs::remove_file(silent).unwrap();
        assert!(
            run(Cli::Decode {
                input: "/nonexistent/x.wav".into()
            })
            .unwrap_err()
            .starts_with("/nonexistent/x.wav: ")
        );
    }

    #[test]
    fn rejects_a_text_longer_than_a_frame() {
        let error = encode(&["x".repeat(300)], TxConfig::default()).unwrap_err();
        assert!(
            error.starts_with("the text is 300 bytes long, a frame takes 255"),
            "{error}"
        );
    }
}

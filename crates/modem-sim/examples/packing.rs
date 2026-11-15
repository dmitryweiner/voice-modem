//! What packing gives: chat texts in UTF-8 and packed (`text.rs`), with the
//! frames of the slowest mode they take and their time on the air; files of
//! a few kinds as they are and deflated (`files.rs`).
//!
//!     cargo run --release -p modem-sim --example packing
//!
//! Source of the tables of packing in `docs/measurements.md`.

use modem_core::files::Files;
use modem_core::link::frame_bytes;
use modem_core::text::pack;
use modem_core::tx::frame_len;
use modem_core::{Kind, Mode};

/// Frames of R1 a message of `len` bytes takes, and their seconds on the air.
fn in_r1(len: usize) -> (usize, f32) {
    let (mut frames, mut samples, mut done) = (0, 0, 0);
    while done < len || frames == 0 {
        // the type, and the offset in every segment but the first
        let header = if done == 0 { 1 } else { 3 };
        let take = (frame_bytes(Mode::R1) - header).min(len - done);
        samples += frame_len(Kind::Data, Mode::R1, header + take);
        done += take;
        frames += 1;
    }
    (frames, samples as f32 / 16_000.0)
}

fn main() {
    let texts = [
        "ok",
        "да",
        "How do you hear me? Over.",
        "Как слышно? Приём.",
        "Call me back when you get this message, thanks!",
        "Перезвони, когда получишь это сообщение, спасибо!",
        "Meeting tomorrow at 10:30 by the metro entrance, don't be late.",
        "Встречаемся завтра в 10:30 у входа в метро, не опаздывай.",
        "Mixed: модем over the air, 62.5 бит/с — ok? 👍",
    ];
    println!("chars  UTF-8  packed  share  R1 frames  R1 s (as is → packed)  text");
    for text in texts {
        let packed = pack(text).len();
        let (utf8, sent) = (text.len(), packed.min(text.len()));
        let (before, after) = (in_r1(utf8), in_r1(sent));
        println!(
            "{:>5}  {utf8:>5}  {packed:>6}  {:>4.0}%  {} → {}       {:>5.1} → {:>5.1}         {text}",
            text.chars().count(),
            100.0 * packed as f32 / utf8 as f32,
            before.0,
            after.0,
            before.1,
            after.1,
        );
    }

    let mut seed = 1u32;
    let mut noise = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 24) as u8
    };
    let log: Vec<u8> = (0..2000)
        .flat_map(|n| format!("{n:05} frame R3 heard, SNR {} dB\n", n % 23).into_bytes())
        .collect();
    let json: Vec<u8> = (0..500)
        .flat_map(|n| {
            format!("{{\"id\":{n},\"name\":\"station {n}\",\"mode\":\"F2\"}},").into_bytes()
        })
        .collect();
    let prose = "Съешь же ещё этих мягких французских булок, да выпей чаю. The quick brown fox jumps over the lazy dog. "
        .repeat(100)
        .into_bytes();
    let photo: Vec<u8> = (0..50_000).map(|_| noise()).collect();
    println!("\nfile                     bytes   on air  share");
    for (name, bytes) in [
        ("a log", log),
        ("JSON", json),
        ("a text repeated", prose),
        ("a photo (random bytes)", photo),
    ] {
        let len = bytes.len();
        let mut files = Files::new();
        let id = files.send(name, bytes).expect("a small file");
        let sent = files.sent_bytes(id).expect("just queued");
        println!(
            "{name:<22} {len:>7}  {sent:>8}  {:>4.0}%{}",
            100.0 * sent as f32 / len as f32,
            if sent < len { "" } else { "  (goes as it is)" }
        );
    }
}

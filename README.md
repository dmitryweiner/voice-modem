# Voice Modem

An acoustic modem in the browser. Two people open the app, connect over any
voice channel — a phone call, a radio, a messenger call, or just a speaker and
a microphone in one room — and exchange text and small files. Data is encoded
as sound in the voice band; speed and modulation adapt to the channel.

No server: a static, offline-capable web app (PWA). The signal-processing core
is written in Rust and runs in the browser as WebAssembly inside a Web Worker.

**Status: stage 5 of 7 done.** Two devices exchange texts and files through
the air: open
[the app](https://dmitryweiner.github.io/voice-modem/) on both, next to each
other, turn on the microphone and write. Every text is acknowledged by the
other side and repeated until it is, up to five times; the sender sees
"delivered" or "not delivered". Stations take turns, can greet each other by
name, and a radio with a push-to-talk button is supported: the app asks when
to press it. A file is offered first and sent once the other side accepts it,
with the progress and the time left on both screens; a transfer that broke
off goes on from where it stopped when the file is sent again. Texts go
packed (a Russian one in about 40% of its bytes), files deflated when that
helps. A one-way station (a radio that only transmits, a recording) sends
texts once and broadcasts files in a fountain code: whoever listens collects
any of the symbols, as many as the file has pieces. With a password shared
by the stations everything they send goes sealed (Argon2id and
ChaCha20-Poly1305): others hear only that the channel is busy.

Seven modes from 62.5 to 4200 bit/s, chosen by the modem itself from how well
the other side hears it: tones for noise, distance and phone codecs, and
carriers (OFDM under an LDPC code) for a cable or a speaker close by. The
slowest is received down to 15 dB below white noise and through speech
codecs down to 4.75 kbit/s; over the air between a laptop and a phone it is
reliable at 8 m. The fast ones are measured in simulation only so far
([measurements](docs/measurements.md), [protocol](docs/protocol.md),
[sound in the browser](docs/web-audio.md)). The rest is being built stage by
stage — see [plans/PLAN.md](plans/PLAN.md).

The same modem from the command line:

```bash
cargo run --release -p modem-cli -- encode "Hello over the air" -o hello.wav
cargo run --release -p modem-cli -- channel hello.wav -o noisy.wav --telephone --snr -10
cargo run --release -p modem-cli -- decode noisy.wav

# a file broadcast into a recording, and collected from it
cargo run --release -p modem-cli -- encode --file photo.jpg --times 2 --mode f1 -o photo.wav
cargo run --release -p modem-cli -- decode photo.wav --save received/

# sealed: only the password opens it
cargo run --release -p modem-cli -- encode "Meet at the bridge" --password s3cret -o sealed.wav
cargo run --release -p modem-cli -- decode sealed.wav --password s3cret
```

## Development

Needs Node (the version in `.node-version`), a Rust toolchain with the
`wasm32-unknown-unknown` target, and [wasm-pack](https://wasm-bindgen.github.io/wasm-pack/).

```bash
fnm use            # Node from .node-version
npm ci
npm run dev        # builds the WASM core, then the dev server
npm run dev:lan    # the same on the local network over HTTPS, to test on a phone
npm run check      # cargo fmt/clippy/test + WASM build + typecheck + lint + unit tests
npm run smoke      # browser smoke (-- --preview for the build and offline)
node scripts/codecs.mjs   # the modem through speech codecs (needs ffmpeg)
npm run build      # → dist/
```

See [AGENTS.md](AGENTS.md) for the layout and conventions, [docs/](docs/)
for the documentation.

## License

[GPL-3.0-or-later](LICENSE).

# Voice Modem

An acoustic modem in the browser. Two people open the app, connect over any
voice channel — a phone call, a radio, a messenger call, or just a speaker and
a microphone in one room — and exchange text and small files. Data is encoded
as sound in the voice band; speed and modulation adapt to the channel.

No server: a static, offline-capable web app (PWA). The signal-processing core
is written in Rust and runs in the browser as WebAssembly inside a Web Worker.

**Status: stage 4 of 7 done.** Two devices exchange texts through the air: open
[the app](https://dmitryweiner.github.io/voice-modem/) on both, next to each
other, turn on the microphone and write. Every text is acknowledged by the
other side and repeated until it is, up to five times; the sender sees
"delivered" or "not delivered". Stations take turns, can greet each other by
name, and a radio with a push-to-talk button is supported: the app asks when
to press it.

Seven modes from 62.5 to 4200 bit/s, chosen by the modem itself from how well
the other side hears it: tones for noise, distance and phone codecs, and
carriers (OFDM under an LDPC code) for a cable or a speaker close by. The slowest is received down to 15 dB below white
noise and through speech codecs down to 4.75 kbit/s; over the air between a
laptop and a phone it is reliable at 8 m
([measurements](docs/measurements.md), [protocol](docs/protocol.md),
[sound in the browser](docs/web-audio.md)). The rest is being built stage by
stage — see [plans/PLAN.md](plans/PLAN.md).

The same modem from the command line:

```bash
cargo run --release -p modem-cli -- encode "Hello over the air" -o hello.wav
cargo run --release -p modem-cli -- channel hello.wav -o noisy.wav --telephone --snr -10
cargo run --release -p modem-cli -- decode noisy.wav
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

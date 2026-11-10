# Voice Modem

An acoustic modem in the browser. Two people open the app, connect over any
voice channel — a phone call, a radio, a messenger call, or just a speaker and
a microphone in one room — and exchange text and small files. Data is encoded
as sound in the voice band; speed and modulation adapt to the channel.

No server: a static, offline-capable web app (PWA). The signal-processing core
is written in Rust and runs in the browser as WebAssembly inside a Web Worker.

**Status: stage 1 of 7.** The core has its first, robust mode: 62.5 bit/s
over 16 tones, received down to 15 dB below white noise and through speech
codecs down to 6 kbit/s ([measurements](docs/measurements.md),
[protocol](docs/protocol.md)). It works from the command line; the web page
still only loads the core. The rest is being built stage by stage — see
[plans/PLAN.md](plans/PLAN.md).

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
npm run check      # cargo fmt/clippy/test + WASM build + typecheck + lint + unit tests
npm run smoke      # browser smoke (-- --preview for the build and offline)
node scripts/codecs.mjs   # the modem through speech codecs (needs ffmpeg)
npm run build      # → dist/
```

See [AGENTS.md](AGENTS.md) for the layout and conventions, [docs/](docs/)
for the documentation.

## License

[GPL-3.0-or-later](LICENSE).

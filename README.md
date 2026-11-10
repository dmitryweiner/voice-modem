# Voice Modem

An acoustic modem in the browser. Two people open the app, connect over any
voice channel — a phone call, a radio, a messenger call, or just a speaker and
a microphone in one room — and exchange text and small files. Data is encoded
as sound in the voice band; speed and modulation adapt to the channel.

No server: a static, offline-capable web app (PWA). The signal-processing core
is written in Rust and runs in the browser as WebAssembly inside a Web Worker.

**Status: skeleton (stage 0).** The page loads the core and shows its version;
the modem itself is being built stage by stage — see
[plans/PLAN.md](plans/PLAN.md).

## Development

Needs Node (the version in `.node-version`), a Rust toolchain with the
`wasm32-unknown-unknown` target, and [wasm-pack](https://wasm-bindgen.github.io/wasm-pack/).

```bash
fnm use            # Node from .node-version
npm ci
npm run dev        # builds the WASM core, then the dev server
npm run check      # cargo fmt/clippy/test + WASM build + typecheck + lint + unit tests
npm run smoke      # browser smoke (-- --preview for the build and offline)
npm run build      # → dist/
```

See [AGENTS.md](AGENTS.md) for the layout and conventions, [docs/](docs/)
for the documentation.

## License

[GPL-3.0-or-later](LICENSE).

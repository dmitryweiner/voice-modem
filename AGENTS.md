# Agents guide

Voice Modem — an acoustic modem in the browser: text and small files over any
voice channel (phone call, radio, messenger, speaker and microphone), encoded
as sound. A static, offline-capable web app: TypeScript + Vite + React; the
signal-processing core is Rust, compiled to WebAssembly and run in a Web
Worker. Tests on `cargo test`, Vitest (node and jsdom) and a Playwright smoke.
UI in English and Russian. Deployed to GitHub Pages.

The shared conventions behind this setup are in `~/projects/CLAUDE.md` and
`~/projects/_conventions/`; this file is what holds for this repository.

## Plans

- [plans/PLAN.md](plans/PLAN.md) — the development plan: architecture,
  protocol layers, stages 0–7 and the decisions taken. Stages 0
  (skeleton), 1 (PHY minimum: mode R1, `modem-cli`, the simulator), 2 (sound
  in the browser) and 3 (link layer: acknowledgements and repeats, taking
  turns, greetings, push-to-talk) and 4 (fast modes and adaptation: tone
  modes R2–R4, coherent OFDM modes F1–F3 under an LDPC code, the choice of
  mode by the advice in acknowledgements) and 5 (file transfer over the
  two-way link with offer, consent, cancelling and going on after a break;
  several blocks per frame under one acknowledgement; packed texts and
  deflated files; broadcast in a fountain code for one-way stations;
  sealing by a shared password) are done. Stage 6 (presets) is next.

## Commands

Node.js — the version in `.node-version` (`fnm use`; check `node -v`: the
login shell may put an older one first). Rust — stable with the
`wasm32-unknown-unknown` target, plus `wasm-pack`; they live in
`~/.cargo/bin`, which a non-login shell may lack in `PATH`.

- `npm ci` — install dependencies.
- `npm run wasm` — build `crates/modem-wasm` into `src/wasm/` (gitignored).
  `dev`, `build` and `check` run it themselves.
- `npm run dev` — dev server.
- `npm run dev:lan` — the dev server on the local network over HTTPS with a
  self-signed certificate (`--mode lan`): sound needs a secure context, and a
  LAN address over http is not one. The phone shows a certificate warning
  once; accept it.
- `npm run check` — `check:rust` (cargo fmt, clippy, test) + WASM build +
  typecheck + oxlint + prettier + unit tests. Must pass before any work is
  called done.
- `npm run smoke` — browser smoke on the dev server (builds `modem-cli` for
  the fake microphone, so it needs cargo; on a Mac it wakes the display,
  without which Chromium processes no sound); `-- --preview` builds and
  also checks offline mode; `-- --shots` saves screenshots to `shots/`.
- `npm run build` — WASM, typecheck and build into `dist/`.
- `npm run format` — prettier and `cargo fmt`.
- `npm run icons` — render the PWA PNG icons from `public/icon.svg`.
- `cargo run --release -p modem-cli -- help` — the native tool: `encode`
  texts and a file broadcast into a WAV file (sealed with `--password`),
  `decode` a WAV file (and save the files in it), pass one through a
  simulated `channel`.

## Scripts

- `cargo run --release -p modem-sim --example sweep` — where the modem stops
  working: frames received against noise, frequency and clock offset; the
  speed of the receiver. Source of `docs/measurements.md`.
- `cargo run --release -p modem-sim --example modes` — the modes compared:
  the white noise and the room echo each withstands, what the receiver
  reports there; for the fast modes also clock offsets and decoding time.
  Source of the mode table and of the thresholds of `advise` in `link.rs`.
- `cargo run --release -p modem-sim --example pause` — what more silence
  between the header and the payload would give in a room.
- `cargo run --release -p modem-sim --example files` — what a file costs in
  each mode: the time for 10 and 100 KB and the net speed against the speed
  of the mode, two-way and broadcast. Source of the file tables in
  `docs/measurements.md`.
- `cargo run --release -p modem-sim --example packing` — chat texts in UTF-8
  and packed, with their frames in R1; files as they are and deflated.
  Source of the packing tables in `docs/measurements.md`.
- `node scripts/codecs.mjs` — the modem through real speech codecs with
  ffmpeg (`--mode r3` or `f1` for another mode, `--keep` keeps the WAV files).
- `npm run smoke` — the browser smoke, see above.

CI (`.github/workflows/ci.yml`) runs `check` and the preview smoke on every
push and PR; a push to `main` publishes `dist/` to GitHub Pages.

## What costs what

| Operation                       | Time (M-series Mac)                                |
| ------------------------------- | -------------------------------------------------- |
| `npm run wasm`, nothing changed | 0.3 s                                              |
| `npm run wasm`, clean           | ~40 s (first run also installs `wasm-bindgen-cli`) |
| `npm run check`, warm           | ~11 s                                              |
| `npm run smoke -- --preview`    | ~36 s                                              |
| `cargo test --workspace`, warm  | ~6 s (the link and file tests take most of it)     |
| the sweep example               | ~50 s                                              |
| the modes example               | ~2 min                                             |
| the pause example               | ~1.5 min                                           |
| `node scripts/codecs.mjs`       | ~10 s per mode                                     |
| the key of a password           | ~0.1 s, native and as WASM                         |
| receiver, searching in noise    | 131× real time native; 95× as WASM at 48 kHz       |
| receiver, amid modem tones      | 36× real time native (a frame with a missed start) |

## Finding out why something fails on a real channel

What worked for the range over the air: reproduce the numbers the user saw
(the SNR shown under a message, where it stops) in `modem-sim` first — add
the missing channel model if need be — then compare variants there
(`examples/modes.rs`), and only then change the modem. A change is judged by
the user's next test on real devices, which goes into `docs/measurements.md`.

## Test-driven development is mandatory

Every behaviour change goes red → green → refactor:

1. Write a failing test that describes the new behaviour or reproduces the bug.
   Run it and see it fail for the expected reason.
2. Write the minimal code that makes it pass.
3. Refactor with the tests green.

Signal processing and protocol → `cargo test` in `crates/` (through
`modem-sim` once channels matter); pure TypeScript logic → `*.test.ts` in
node; components → `*.test.tsx` in jsdom (`// @vitest-environment jsdom`,
`@testing-library/react`); what needs a real browser (service worker, layout,
the worker with WASM, audio) → a step in `scripts/smoke.mjs`.

## Layout

- `Cargo.toml` — the Rust workspace (the dev profile is optimised: signal
  processing in tests is too slow otherwise).
- `crates/modem-core` — the modem. `modem.rs` — what the application sees
  (texts, files, events, samples; the password, one way or two); `link.rs` — the link layer, pure logic on frames
  and a clock (acknowledgements, repeats, turns, segments and cutting them
  again for a slower mode, greetings, push-to-talk, cancelling, the ladder
  of modes and the advice, the blocks a frame takes and which of them
  arrived); `files.rs` — file transfer, pure logic on
  messages and a clock (offers, consent, chunks, deflate, the checksum,
  going on after a break, the time left; broadcast files over a one-way
  channel); `fountain.rs` — the fountain code of broadcast files;
  `seal.rs` — the key of a password and the seal of a message; `text.rs` —
  texts packed in a fixed Huffman code; `tx.rs` / `rx.rs` — a frame to
  samples and a stream of samples to frames; `sync.rs` — the chirp preamble;
  `mfsk.rs` — tones: several at once, sets of tones in turn against room
  echo; `ofdm.rs` — carriers: training, pilots, the phase loops, soft bits;
  `ldpc.rs` — the code of the fast modes; `frame.rs` — the modes R1–R4 and
  F1–F3, the blocks of a payload, frame kinds and the header;
  `resample.rs` — device rate ⇄ 16 kHz; `coding.rs` — interleaver, scrambler;
  `conv.rs` — convolutional code and Viterbi; `crc.rs`, `bits.rs`.
- `crates/modem-sim` — channel models (noise, band-pass, echo, room
  reverberation, frequency and clock offset, clipping); `tests/link.rs` — the
  modem through them; `examples/sweep.rs`, `examples/modes.rs`,
  `examples/pause.rs`, `examples/files.rs`, `examples/packing.rs` — the
  measurements (see Scripts).
- `crates/modem-cli` — the native tool: `lib.rs` (arguments, commands),
  `wav.rs`.
- `crates/modem-wasm` — the wasm-bindgen wrapper, a thin adapter: `Modem`,
  `ModemEvent`, `version`, `maxTextBytes`, `maxNameBytes`.
- `src/main.tsx` — entry: renders the app with browser storage, languages and
  the worker client; registers the service worker in production.
- `src/App.tsx` — the shell: language, core status, the password (in
  memory only). `src/ModemPanel.tsx` — the modem on the page: station
  settings (name, push-to-talk, one way, password, speed), microphone
  switch, state and speed, input meter, the scrolling box of messages and
  files with their statuses (sealed or not, broadcast as the share sent),
  cancel, resend and clear, the text and the file to send, the offer of a
  file to accept or decline.
- `src/audio/` — `engine.ts` (AudioContext, microphone, wiring; browser
  glue), `modem-io.worklet.js` (the AudioWorklet: plain JS, no imports),
  `meter.ts` (peak and spectrum bars). Design: `docs/web-audio.md`.
- `src/worker/` — `protocol.ts` (typed messages with a `jobId`, events),
  `client.ts` (page side, creates the worker lazily), `handler.ts` (the
  session: one modem on one audio channel; tested in node, also with the
  real WASM), `modem.worker.ts` (the thin worker file, the only importer of
  WASM).
- `src/wasm/` — wasm-pack output, gitignored.
- `src/i18n/` — `en.ts` defines the messages, `ru.ts` follows its type;
  `index.ts` — locales, detection, text direction; `plural.ts` —
  `Intl.PluralRules` helper, signed numbers.
- `src/storage.ts` — versioned, validated settings in localStorage: language,
  name, push-to-talk, one way, speed. Never the password.
- `src/sw-plugin.ts` — Vite plugin emitting `sw.js` that precaches the build;
  `src/pwa.ts` — its registration, an update check on every return to the
  page and a reload when a new version takes control (`_conventions/pwa.md`).
- `src/style.css` — mobile first, logical properties, light and dark. The app
  fills the screen and the messages scroll in their box; a phone hides what
  a desktop shows (heading, introduction, settings while the modem runs).
- `public/` — manifest, icons (`icon.svg` is the source; still the template's).
- `scripts/lib/harness.mjs` — Playwright plumbing; `scripts/smoke.mjs` — the
  smoke; `scripts/icons.mjs` — icon rendering.
- `docs/` — user and design docs; `plans/` — plans.

## Architectural invariants

- The core (`modem-core`) knows nothing about the platform: samples and
  commands in, samples and events out; no threads, timers, system time or
  system randomness — time is the number of processed samples, and the
  application gives the seeds and the entropy of the seal (`Config`). Pure
  Rust dependencies only (it must build for wasm32 and Android), without
  default features that reach for the system (`getrandom`). `unsafe` is
  forbidden in our crates.
- The main thread never imports WASM: only `src/worker/modem.worker.ts` does.
  CI greps the main chunk for `WebAssembly`.
- No SharedArrayBuffer (GitHub Pages sends no COOP/COEP headers): the page,
  the worker and the AudioWorklet exchange `postMessage` with transferables.
- Sound never passes through the main thread: the worklet and the worker
  talk over their own MessageChannel. The worklet only moves samples.
- Browser APIs reach components as parameters (`startAudio`, `core`,
  `storage`, `fileUrl`), so component tests run without a browser.
- The wire format is specified in `docs/protocol.md`; a change to it changes
  that file in the same PR. It is version 0 and changes freely: builds of
  different PRs do not talk to each other, so both devices are updated
  together. Limits claimed in `docs/measurements.md` are
  guarded by `crates/modem-sim/tests/link.rs` with a margin.
- The receiver never delivers a block of a payload whose CRC-32 failed, nor
  one that came after it.
- The core's clock is the count of received samples: the host must keep
  calling `push_rx` (silence too) and play what `pull_tx` gives at once.
  `link.rs` knows no sound and `rx.rs`/`tx.rs` know no link rules; only
  `modem.rs` joins them. `files.rs` knows neither frames nor the link:
  messages and their fates pass through `modem.rs` as well.
- Over the two-way link nothing of a file's content is transmitted before
  its receiver consents; a one-way station broadcasts without asking, as
  nobody can answer. No file is delivered whose length or CRC-32 is wrong.
- A station with a password seals every text and file message; only the
  greeting, frame headers and acknowledgements go in the clear. The
  password is never stored (a test in `App.test.tsx` checks localStorage).
- `modem-wasm` and the worker file stay thin adapters; logic lives where it is
  unit-tested (`modem-core`, `handler.ts`).

## Conventions

- Code comments, docs and README in English.
- Pure logic lives in modules without the DOM; time, randomness, storage and
  browser APIs are parameters so tests can replace them.
- The package name in `package.json` is the app id: Vite injects it as
  `__APP_ID__`, the storage key and the service worker's cache prefix. All
  apps on `<user>.github.io` share one origin, so it must be unique.
- Every interface text lives in `src/i18n/`; components take them from
  `messages[locale]`. `index.html` holds only the English title and
  description (a test keeps them equal to `en.ts`). A new message goes to
  `en.ts` first; TypeScript then demands it in the others.
- Language: the saved choice, else the first supported browser language, else
  English. Switching redraws without losing state. There is no RTL
  language yet, but `<html dir>` follows `direction(locale)` and CSS uses
  logical properties only.
- A new stored field gets a default and a test that data saved before it
  still loads (as `name`, `manualPtt`, `mode`, `oneWay` did). Changing the meaning or
  the type of an existing field needs a bump of `VERSION` in
  `src/storage.ts` and a test for the reset or migration.
- No `as` casts in app code; a disabled lint rule carries its reason.
- A helper script that may be needed again goes to `scripts/` (Rust
  experiments: `crates/modem-sim/examples/`) with a header comment and a line
  in this file. Experiments are kept, not deleted after one use. A new
  browser check is a step or a flag in `scripts/smoke.mjs`, not a new file.
- Each feature gets its own branch and a pull request.

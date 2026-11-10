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
  protocol layers, stages 0–7 and the decisions taken. Stage 0 (skeleton) is
  done; stage 1 (PHY minimum) is next.

## Commands

Node.js — the version in `.node-version` (`fnm use`; check `node -v`: the
login shell may put an older one first). Rust — stable with the
`wasm32-unknown-unknown` target, plus `wasm-pack`; they live in
`~/.cargo/bin`, which a non-login shell may lack in `PATH`.

- `npm ci` — install dependencies.
- `npm run wasm` — build `crates/modem-wasm` into `src/wasm/` (gitignored).
  `dev`, `build` and `check` run it themselves.
- `npm run dev` — dev server.
- `npm run check` — `check:rust` (cargo fmt, clippy, test) + WASM build +
  typecheck + oxlint + prettier + unit tests. Must pass before any work is
  called done.
- `npm run smoke` — browser smoke on the dev server; `-- --preview` builds and
  also checks offline mode; `-- --shots` saves screenshots to `shots/`.
- `npm run build` — WASM, typecheck and build into `dist/`.
- `npm run format` — prettier and `cargo fmt`.
- `npm run icons` — render the PWA PNG icons from `public/icon.svg`.
- `cargo run -p modem-cli` — the native tool (prints the core version for now).

CI (`.github/workflows/ci.yml`) runs `check` and the preview smoke on every
push and PR; a push to `main` publishes `dist/` to GitHub Pages.

## What costs what

| Operation                       | Time (M-series Mac)                                |
| ------------------------------- | -------------------------------------------------- |
| `npm run wasm`, nothing changed | 0.3 s                                              |
| `npm run wasm`, clean           | ~40 s (first run also installs `wasm-bindgen-cli`) |
| `npm run check`, warm           | ~3 s                                               |
| `npm run smoke -- --preview`    | ~4 s                                               |

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

- `Cargo.toml` — the Rust workspace; `crates/modem-core` — DSP, modulations,
  FEC, frames, link and transport; `crates/modem-sim` — channel models for
  tests; `crates/modem-cli` — native tool (text/file ⇄ WAV);
  `crates/modem-wasm` — the wasm-bindgen wrapper, a thin adapter.
- `src/main.tsx` — entry: renders the app with browser storage, languages and
  the worker client; registers the service worker in production.
- `src/App.tsx` — the React UI.
- `src/worker/` — `protocol.ts` (typed messages with a `jobId`), `client.ts`
  (page side, creates the worker lazily), `handler.ts` (worker logic, tested
  in node), `modem.worker.ts` (the thin worker file, the only importer of
  WASM).
- `src/wasm/` — wasm-pack output, gitignored.
- `src/i18n/` — `en.ts` defines the messages, `ru.ts` follows its type;
  `index.ts` — locales, detection, text direction; `plural.ts` —
  `Intl.PluralRules` helper.
- `src/storage.ts` — versioned, validated settings in localStorage.
- `src/sw-plugin.ts` — Vite plugin emitting `sw.js` that precaches the build;
  `src/pwa.ts` — its registration, an update check on every return to the
  page and a reload when a new version takes control (`_conventions/pwa.md`).
- `src/style.css` — mobile first, logical properties, light and dark.
- `public/` — manifest, icons (`icon.svg` is the source; still the template's).
- `scripts/lib/harness.mjs` — Playwright plumbing; `scripts/smoke.mjs` — the
  smoke; `scripts/icons.mjs` — icon rendering.
- `docs/` — user and design docs; `plans/` — plans.

## Architectural invariants

- The core (`modem-core`) knows nothing about the platform: samples and
  commands in, samples and events out; no threads, timers or system time —
  time is the number of processed samples. Pure Rust dependencies only (it
  must build for wasm32 and Android). `unsafe` is forbidden workspace-wide.
- The main thread never imports WASM: only `src/worker/modem.worker.ts` does.
  CI greps the main chunk for `WebAssembly`.
- No SharedArrayBuffer (GitHub Pages sends no COOP/COEP headers): the page,
  the worker and the AudioWorklet exchange `postMessage` with transferables.
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
- Do not change the stored format without bumping `VERSION` in
  `src/storage.ts` and a test for the reset or migration.
- No `as` casts in app code; a disabled lint rule carries its reason.
- A helper script that may be needed again goes to `scripts/` with a header
  comment and a line in this file. A new browser check is a step or a flag in
  `scripts/smoke.mjs`, not a new file.
- Each feature gets its own branch and a pull request.

# Voice Modem — development plan

Status: **approved on 2026-11-10**. The open questions of the draft were settled
with the author; the answers are recorded in [section 12](#12-decisions) and
folded into the text. Work proceeds stage by stage ([section 11](#11-stages)).

## 1. What we are building

A web app that is an acoustic modem. Two people open the app, connect over any
voice channel (a phone call, a radio, a messenger call, or just a speaker and a
microphone in one room) and exchange text and small files. Data is encoded as
sound in the voice band; speed and modulation adapt to the quality of the
channel.

There is no server: the app is static and works offline after the first load
(PWA). The whole signal-processing core is written in Rust and runs in the
browser as WebAssembly; later the same core is linked into an Android app.

### Non-goals (for the first version)

- Compatibility with existing protocols (V.xx modems, FreeDV, fldigi, ggwave).
  The protocol is our own.
- Voice, video or streaming data.
- Large files. The target is up to ~100 KB (see the speed table below).
- Automatic dialling or control of the call or the radio (CAT, PTT over a cable).
- Full duplex.

## 2. What it looks like physically

A browser cannot mix sound into a cellular call, so the scenarios are:

| Scenario     | How it connects                                                                | Specifics                                                                        |
| ------------ | ------------------------------------------------------------------------------ | -------------------------------------------------------------------------------- |
| Over the air | speaker of one device → microphone of the other                                | room echo, noise, weak speakers                                                  |
| Phone        | a phone on speakerphone next to the device running the app, or a headset cable | vocoder (AMR/EVS), noise suppression, VAD cuts "non-speech" signal               |
| Radio        | a radio next to the device, or an audio cable; a human presses PTT (or VOX)    | half duplex, squelch tail, FM pre-emphasis, frequency offset on SSB              |
| Internet     | a messenger call on the same or a neighbouring device                          | Opus codec, packet loss, time stretching by the jitter buffer, noise suppression |

Consequence: the hardest case is a cellular phone through an acoustic coupling
(vocoder + air). It needs a very robust slow mode.

The phone scenario is acoustic coupling through the speakerphone; a cable is a
supported option, not a requirement.

### Expected speeds (guidance, not promises)

| Channel                   | Net speed      | 10 KB file  |
| ------------------------- | -------------- | ----------- |
| Over the air, ~1 m, quiet | 0.5–2 kbit/s   | 1–3 min     |
| Wired phone / G.711       | 1–3 kbit/s     | 0.5–1.5 min |
| Cellular (AMR/EVS)        | 50–300 bit/s   | 5–30 min    |
| FM radio                  | 0.3–1.5 kbit/s | 1–5 min     |
| SSB (HF)                  | 0.1–1 kbit/s   | 1.5–15 min  |
| VoIP (Opus)               | 0.3–2 kbit/s   | 1–5 min     |

The numbers are refined at stages 1 and 4 from simulation and from measurements
on real channels.

## 3. Architecture

```
┌────────────────────────── browser ──────────────────────────┐
│  UI (TypeScript, React)                                      │
│   chat · files · presets · channel indicators                │
│        │ events/commands (postMessage)                       │
│  Web Worker ── WASM: modem-core                              │
│        │ sample blocks (Float32Array, transferable)          │
│  AudioWorklet (thin JS: microphone capture, playback queue)  │
└──────────────────────────────────────────────────────────────┘
```

- **AudioWorklet** only moves samples — no logic. The microphone is opened with
  `echoCancellation / noiseSuppression / autoGainControl = false`.
- **Worker + WASM**: the core blocks neither the UI nor the audio thread. They
  exchange `postMessage` with transferable buffers (no SharedArrayBuffer: it
  needs COOP/COEP headers, which GitHub Pages does not send). The main thread
  never imports WASM.
- **The core** knows nothing about the browser: a stream of samples and commands
  in, a stream of samples and events out. No threads, timers or system time
  inside — time is the number of processed samples. That makes the core
  deterministic, testable and portable.

### Repository layout

The web app lives in the repository root, as in every project under
`~/projects` (it starts from `web-app-template`); the Rust workspace sits next
to it.

```
Cargo.toml        # Rust workspace
crates/
  modem-core/     # DSP, modulations, FEC, frames, link and transport layers
  modem-sim/      # channel models for tests (noise, band, frequency offset, echo, codecs)
  modem-cli/      # native tool: text/file ⇄ WAV, runs through the simulator
  modem-wasm/     # wasm-bindgen wrapper around modem-core
package.json
vite.config.ts
src/              # UI, Worker, AudioWorklet
scripts/
docs/             # protocol specification, measurement notes
plans/
```

The core depends on pure Rust only (`rustfft` and the like), no C libraries:
that is the condition for a simple build for wasm32 and Android.

### Core API (sketch)

```rust
let mut modem = Modem::new(Config { sample_rate, preset });
modem.push_rx(&samples);              // samples from the microphone
modem.pull_tx(&mut out);              // samples for the speaker (silence if idle)
modem.command(Command::SendText(..)); // SendFile, Connect, SetPreset, Abort…
while let Some(ev) = modem.poll_event() { .. }
// events: MessageReceived, FileProgress, LinkState, ChannelStats, TxRequest (for PTT)…
```

## 4. Physical layer

- Internal sample rate 16 kHz; a resampler in the core from/to the device rate
  (44.1/48 kHz).
- Default working band 300–3400 Hz; a preset can narrow it (for example
  500–2700 Hz for a radio) or widen it (air/internet — next version).

### Mode ladder

One protocol, several modes from "gets through anything" to "fast over a clean
channel". The parameters are tentative and are fixed from simulation results.

| Group      | Modulation                                      | Net speed      | For                                    |
| ---------- | ----------------------------------------------- | -------------- | -------------------------------------- |
| R (robust) | non-coherent MFSK, long symbols                 | ~30–150 bit/s  | vocoders, heavy noise, time stretching |
| M (medium) | OFDM, differential QPSK                         | ~300–900 bit/s | radio, VoIP, air with echo             |
| F (fast)   | coherent OFDM with pilots, QPSK → 16QAM → 64QAM | ~1–5 kbit/s    | clean air, wired line                  |

OFDM reference parameters: FFT 512 at 16 kHz (31.25 Hz spacing, 32 ms symbol) +
8 ms cyclic prefix, ~99 carriers in the 300–3400 Hz band. Presets change the
prefix length and the carrier set.

### Frame

```
[lead-in tone*] [chirp preamble] [header] [data ...]
```

- **Lead-in tone** (optional, set by the preset) — opens the VOX / squelch of a
  radio, "wakes up" the VAD of a vocoder.
- **Preamble** — a linear chirp: frame detection, symbol timing, estimation of
  the frequency offset and of the channel frequency response.
- **Header** — always in the most robust mode: data mode number, length, frame
  number, CRC. The receiver does not need to know the mode in advance.
- **Data** — in the mode named by the header, with CRC-32.

### Forward error correction

- LDPC with soft decisions (quasi-cyclic matrices from IEEE 802.11n, rates
  1/2, 2/3, 3/4, 5/6) + interleaving + scrambler.
- R modes additionally use repetition/diversity in time and frequency.
- Fallback if LDPC does not fit the schedule: a K=7 convolutional code with a
  Viterbi decoder.

### Synchronisation and compensation

Sound-card clock drift (up to hundreds of ppm), carrier offset (SSB — up to
±200 Hz), AGC and level changes, channel estimation and equalisation by pilots.

## 5. Link layer

- **Half duplex only** in the first version: stations transmit in turn. Reasons:
  PTT radios; own echo (the speaker is heard by the own microphone); vocoders
  handle double talk badly.
- **ARQ**: selective acknowledgement of a burst of frames, only lost frames are
  resent; timeouts account for channel delays (VoIP, cellular — hundreds of ms).
- **Medium access**: listen before talk (busy detector), random back-off on
  collision.
- **PTT mode**: the core emits a "transmission needed" event, the UI shows a
  large "Press PTT" hint, transmission starts on a button/space; for VOX —
  automatically with the lead-in tone.
- **Broadcast mode** (no return channel): transmission without
  acknowledgements, for one-way channels and for record/playback. Redundancy
  comes from **fountain codes** (LT/RaptorQ family, pure Rust; the exact code is
  chosen at stage 5): the sender emits an endless stream of repair blocks and
  the receiver decodes once it has collected slightly more than the file size,
  whichever blocks those were.

## 6. Channel adaptation

1. **Probing at connection setup.** An exchange of probe frames in mode R:
   chirp + multitone. Each side measures SNR per frequency, frequency response,
   delay spread, frequency offset and distortion, and reports to the peer. The
   results select the starting mode, band and carrier mask.
2. **Continuous tuning.** In every acknowledgement the receiver reports fresh
   measurements (SNR, decoder margin, share of lost frames) and a recommended
   mode. The transmitter steps down fast (after 1–2 failures) and up carefully
   (after a run of successes and with SNR margin).
3. **Independent per direction**: channels are often asymmetric.
4. **Emergency fallback**: on loss of link both sides return to mode R.
5. Later: per-carrier bit loading (different modulation on different
   frequencies).

Adaptation can be switched off and the mode fixed by hand (needed for broadcast
mode and debugging).

## 7. Transport and application layer

- **Session**: hello (protocol version, callsign/name, capabilities), probing,
  exchange, close. Working without a session is possible — a single message in
  the robust mode. The capabilities field exists from stage 3, so encryption
  and later features are added without a protocol break.
- **Text**: UTF-8, compression of short messages, statuses "sending → delivered
  / not delivered".
- **Files**: offer (name, size, hash) → consent of the receiver → blocks → hash
  check. Progress, remaining-time estimate, cancel, resume after a break.
  Compression (deflate) when it pays off.
- **File size limit** is soft: no hard cap, a warning when the estimated
  transfer time at the current speed exceeds 10 minutes.
- Text messages have priority and cut into a file transfer.
- **Encryption** (stage 5): optional, by a shared password, off by default.
  Password → key with a memory-hard KDF, AEAD over the payload (planned:
  Argon2id + ChaCha20-Poly1305, pure Rust crates; confirmed at stage 5).
  Headers stay in the clear so that a station without the password still sees
  that the channel is busy. The radio presets keep it off and the UI warns when
  it is switched on there: encryption is forbidden on amateur radio bands.

## 8. Presets

A preset is a set of starting parameters and limits; adaptation works inside it.

| Preset       | Band        | Starting mode | Allowed modes                               | Specifics                                                      |
| ------------ | ----------- | ------------- | ------------------------------------------- | -------------------------------------------------------------- |
| Over the air | 300–3400 Hz | M             | R, M, F                                     | long cyclic prefix (echo)                                      |
| Phone        | 300–3400 Hz | R             | R, M (F only if probing shows a clean line) | lead-in tone, continuous "speech-like" spectrum against VAD    |
| FM radio     | 500–2700 Hz | R             | R, M, F                                     | PTT/VOX, 300–500 ms lead-in tone, pre-emphasis accounted for   |
| SSB radio    | 500–2500 Hz | R             | R, M                                        | frequency-offset search ±200 Hz, robustness to fading          |
| Internet     | 300–3400 Hz | M             | R, M, F                                     | robustness to packet loss and time stretching, longer timeouts |
| Custom       | —           | —             | —                                           | every parameter by hand                                        |

## 9. Web app

- Vite + TypeScript + React, following the conventions in
  `~/projects/CLAUDE.md` (TDD, oxlint + prettier, versioned storage key,
  mobile first). PWA (offline, install to the home screen). Deployed to GitHub
  Pages.
- **Chat screen**: a feed of messages and files with statuses, input field,
  file attachment, transfer progress.
- **Link panel**: preset, state (idle / probing / receiving / transmitting),
  current mode and speed, SNR, input/output level meters with a clipping
  warning.
- **Diagnostics** (collapsible): spectrum/waterfall, constellation, channel
  frequency response, frame log.
- **Settings**: input/output devices, volume, name/callsign, manual mode
  choice, PTT/VOX, encryption password.
- **Level wizard**: test signal → "quieter/louder" hint.
- Interface languages: English (default and fallback) and Russian.
- Browsers: current Chrome/Edge/Firefox on desktop, Chrome on Android. Safari
  on iOS is supported as far as possible and does not block a release (iOS does
  not always allow switching off microphone processing — checked at stage 2).

## 10. Testing

- **Unit tests** of DSP blocks, FEC, frames.
- **Channel simulator** (`modem-sim`): white noise, band-pass filter, frequency
  and clock offset, multipath, fading, clipping, block loss/stretching. Curves
  of error rate against SNR for every mode — as regression tests in CI.
- **Real codecs**: WAV through Opus / AMR / G.711 (ffmpeg) — a separate optional
  test set.
- **End-to-end tests**: two instances of the core through the simulator — chat,
  file, link break, adaptation up and down.
- **Field measurements**: over the air first, then a real cellular call, then a
  messenger and radios. Results go to `docs/`.
- **Web**: vitest for logic and DOM, a Playwright smoke at a phone viewport.
- CI (GitHub Actions): `cargo fmt/clippy/test`, the wasm build, `npm run check`,
  the web build. `.wasm` is built in CI, not committed.

## 11. Stages

**Progress**: stages 0 and 1 are done (2026-11-10). What stage 1 settled is in
[docs/protocol.md](../docs/protocol.md) and
[docs/measurements.md](../docs/measurements.md). It differs from the text
above in three places: the code of mode R1 is the convolutional one (LDPC
comes with the fast modes at stage 4); the core takes 16 kHz only — the
resampler moves to stage 2, where a browser first needs it; the simulator has
no fading, packet loss or time stretching yet (stage 6, with the presets).
Open from stage 1: iLBC loses whole frames, and AMR/EVS could not be tested
without a real call.

Every stage ends with a working, checkable result. One branch and PR per
feature; `main` deploys.

| №   | Stage                        | Result                                                                                                                             |
| --- | ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| 0   | Skeleton                     | workspace, CI, wasm build, an empty web app on GitHub Pages                                                                        |
| 1   | PHY minimum                  | one R mode (MFSK) + preamble + frame + FEC; `modem-cli` encodes text to WAV and decodes it back; tests through the simulator       |
| 2   | Web minimum                  | sound in the browser; one-way text over the air between two devices; level meters and spectrum                                     |
| 3   | Link layer                   | half duplex, ARQ, sessions (with the capabilities field); two-way chat with delivery statuses; PTT mode                            |
| 4   | Fast modes and adaptation    | OFDM (M, F), LDPC, probing, automatic mode choice and change on the fly                                                            |
| 5   | Files, broadcast, encryption | file transfer, progress, cancel, resume, compression; broadcast mode with fountain codes; optional encryption by a shared password |
| 6   | Presets                      | tuning for every channel from codec simulation and field measurements                                                              |
| 7   | Polish                       | PWA, level wizard, localisation, protocol documentation                                                                            |
| —   | Later                        | Android (core through UniFFI/JNI + Oboe), full duplex with frequency division, per-carrier bit loading, wideband modes             |

Main risks: (1) cellular vocoders and noise suppression in messengers may leave
only tens of bit/s — checked early, at stage 1, by running through codecs;
(2) audio processing in browsers/OS that cannot be switched off (iOS above all)
— checked at stage 2; (3) the size of stage 4 — it is the largest and is split
into "M modes" and "F modes" if needed; (4) stage 5 grew with fountain codes
and encryption — it is split into three PRs (files, broadcast, encryption).

## 12. Decisions

Settled with the author on 2026-11-10.

| №   | Question                                                    | Decision                                                                                                                  |
| --- | ----------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| 1   | Priority channel for debugging and the first field tests    | Over the air, then a cellular phone                                                                                       |
| 2   | Phone scenario through the speakerphone (acoustic coupling) | Yes; a cable is an option                                                                                                 |
| 3   | UI framework                                                | React + TypeScript                                                                                                        |
| 4   | Interface languages                                         | English (default, fallback) + Russian                                                                                     |
| 5   | Hosting                                                     | GitHub Pages, no server                                                                                                   |
| 6   | Duplex in the first version                                 | Half duplex only                                                                                                          |
| 7   | Broadcast mode (no return channel) in the first version     | Yes, **with fountain codes** (stage 5) — changed from the draft's "simple repetition"                                     |
| 8   | Encryption by a shared password                             | **In the first version**, optional, off by default (stage 5) — changed from the draft's "later"; warning on radio presets |
| 9   | File size limit                                             | Soft: a warning when the estimate exceeds 10 min                                                                          |
| 10  | iOS Safari                                                  | Supported as far as possible, does not block a release                                                                    |
| 11  | Own protocol with no compatibility                          | Yes                                                                                                                       |
| 12  | License                                                     | GPL-3.0-or-later; dependencies are chosen to be compatible                                                                |
| 13  | Repository layout                                           | Web app in the root (as in `web-app-template`), Rust workspace in `crates/`                                               |
| 14  | Where the plan lives and in what language                   | `plans/PLAN.md`, English (this file; translated from the Russian draft `PLAN.md`)                                         |
| 15  | Stage for encryption                                        | Stage 5, with files; the capabilities field is laid down at stage 3                                                       |

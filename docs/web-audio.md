# Sound in the browser

How samples get from the microphone to the modem core and back to the
speaker, and why it is built this way.

```
 page (main thread)                       AudioWorklet thread
┌───────────────────────────┐           ┌──────────────────────────────┐
│ ModemPanel                │  { port } │ modem-io.worklet.js          │
│ engine.ts: AudioContext,  │──────────▶│  microphone → blocks of 2048 │
│  getUserMedia, Analyser   │           │  blocks → speaker, 'idle'    │
│ client.ts                 │           └──────────────▲───────────────┘
└─────────────┬─────────────┘                          │ MessageChannel:
   requests,  │  events                                │ Float32Array blocks,
   { port }   ▼                                        ▼ both ways
┌──────────────────────────────────────────────────────────────────────┐
│ modem worker: handler.ts → WASM core (modem-wasm → modem-core)       │
└──────────────────────────────────────────────────────────────────────┘
```

- **Sound never passes through the main thread.** `engine.ts` creates a
  `MessageChannel` and gives one end to the worklet and the other, through
  `client.start`, to the worker. A busy page (rendering, garbage collection)
  cannot make the modem lose samples.
- **The worklet only moves samples** (`src/audio/modem-io.worklet.js`). It is
  plain JavaScript without imports, shipped as a file of its own: the same
  file in the dev server, the build and the unit test, which loads it with a
  stand-in for `AudioWorkletProcessor`.
- **The core converts the sample rate.** The device runs at its own rate
  (48 or 44.1 kHz as a rule); `Modem::new` takes it and resamples to and from
  16 kHz inside (`crates/modem-core/src/resample.rs`).
- **The microphone is opened raw**: `echoCancellation`, `noiseSuppression` and
  `autoGainControl` off. Each of them is built to keep speech and remove
  everything else. Whether a browser honours the request is its own matter:
  iOS Safari is the known doubt, to be checked on a device.
- **A transmission is handed over whole.** `sendText` makes the core produce
  every sample of the frame, and the worker posts them to the worklet as one
  block; the worklet plays it and answers `'idle'` when the last sample has
  gone to the device. No timing depends on message delivery.
- **The modem does not listen to itself.** While it transmits, and for half a
  second after `'idle'` (the output buffer of the device and the echo of the
  room), the worker drops the microphone blocks. This is the whole of the
  half-duplex discipline for now; the link layer replaces it.
- **The meter is the browser's own `AnalyserNode`**, read by the page 20 times
  a second: the peak of the waveform and the spectrum up to 4 kHz in 48 bars
  (`src/audio/meter.ts`). It is a display only; the modem measures the signal
  itself.

## Testing

The pure parts have unit tests: the session logic with a stand-in core and
with the real WASM core (two sessions, sound passed from one to the other in
Node), the worklet, the meter, the panel with a stand-in audio device.

The whole chain runs in `npm run smoke`: Chromium gets a WAV file made by
`modem-cli` as its microphone (`--use-file-for-fake-audio-capture`), and the
text must appear on the page. Then a text is sent, and the page must come
back to listening when the sound has been played.

What only real devices can tell — speakers, microphones, rooms, what phones
do to the audio path — goes to [measurements.md](measurements.md).

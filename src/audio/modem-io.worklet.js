// The AudioWorklet of the modem: moves samples and nothing else.
//
// Microphone samples are collected into blocks and sent to the modem worker;
// blocks received from the worker are played. Both go through a MessagePort
// that the page hands over in a `{ port }` message, so sound never passes
// through the main thread. When everything given has been played, the
// worker is told 'idle' (AUDIO_IDLE in src/worker/protocol.ts).
//
// Plain JavaScript without imports on purpose: Vite ships this file as it is
// (`new URL(..., import.meta.url)` in engine.ts), the same in dev and build.

/** Samples per block sent to the worker: 43 ms at 48 kHz. */
const BLOCK = 2048;

class ModemIo extends AudioWorkletProcessor {
  constructor() {
    super();
    /** @type {MessagePort | null} */
    this.link = null;
    this.block = new Float32Array(BLOCK);
    this.filled = 0;
    /** @type {Float32Array[]} */
    this.queue = [];
    /** Samples of queue[0] already played. */
    this.played = 0;
    this.playing = false;
    this.port.onmessage = (event) => {
      const port = event.data?.port;
      if (!port) return;
      this.link = port;
      port.onmessage = (message) => {
        if (message.data instanceof Float32Array && message.data.length > 0) {
          this.queue.push(message.data);
          this.playing = true;
        }
      };
    };
  }

  /**
   * @param {Float32Array[][]} inputs
   * @param {Float32Array[][]} outputs
   */
  process(inputs, outputs) {
    const input = inputs[0]?.[0];
    if (input && this.link) this.capture(input);
    const output = outputs[0]?.[0];
    if (output) this.play(output);
    return true;
  }

  /** @param {Float32Array} input */
  capture(input) {
    let at = 0;
    while (at < input.length) {
      const count = Math.min(input.length - at, BLOCK - this.filled);
      this.block.set(input.subarray(at, at + count), this.filled);
      this.filled += count;
      at += count;
      if (this.filled === BLOCK) {
        const full = this.block;
        this.block = new Float32Array(BLOCK);
        this.filled = 0;
        this.link?.postMessage(full, [full.buffer]);
      }
    }
  }

  /** @param {Float32Array} output zero-filled by the browser */
  play(output) {
    let at = 0;
    while (at < output.length && this.queue.length > 0) {
      const head = this.queue[0];
      const count = Math.min(output.length - at, head.length - this.played);
      output.set(head.subarray(this.played, this.played + count), at);
      at += count;
      this.played += count;
      if (this.played === head.length) {
        this.queue.shift();
        this.played = 0;
      }
    }
    if (this.playing && this.queue.length === 0) {
      this.playing = false;
      this.link?.postMessage('idle');
    }
  }
}

registerProcessor('modem-io', ModemIo);

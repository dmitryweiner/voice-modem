import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { beforeAll, describe, expect, it } from 'vitest';
import init, { maxTextBytes, Modem, version } from '../wasm/modem_wasm.js';
import { coreClient, type WorkerPort } from './client';
import { createSession, type Core, type CoreModem, type CoreModemEvent } from './handler';
import { AUDIO_IDLE, isWorkerIn, isWorkerOut, type CoreEvent, type WorkerIn, type WorkerOut } from './protocol';

/** The worker's end of the audio channel: records what is sent to the worklet, lets the test speak for it. */
class FakeAudioPort {
  played: Float32Array[] = [];
  closed = false;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  postMessage(samples: Float32Array) {
    this.played.push(samples);
  }
  close() {
    this.closed = true;
  }
  /** A message from the worklet. */
  hear(data: unknown) {
    this.onmessage?.({ data });
  }
  get port() {
    return this as unknown as MessagePort;
  }
}

class FakeModem implements CoreModem {
  heard: Float32Array[] = [];
  events: CoreModemEvent[] = [];
  freed = false;
  private pending = 0;
  constructor(readonly sampleRate: number) {}
  sendText(text: string) {
    if (text.length > 5) throw new Error('the text is longer than 5 bytes');
    this.pending += this.sampleRate * text.length;
  }
  pushRx(samples: Float32Array) {
    this.heard.push(samples);
  }
  pullTx(length: number) {
    this.pending -= length;
    return new Float32Array(length).fill(0.5);
  }
  txPending() {
    return this.pending;
  }
  pollEvent() {
    return this.events.shift();
  }
  free() {
    this.freed = true;
  }
}

function fakeCore() {
  const modems: FakeModem[] = [];
  const core: Core = {
    version: () => '1.2.3',
    maxTextBytes: () => 5,
    createModem(sampleRate) {
      if (sampleRate < 8000) throw new Error(`unsupported sample rate: ${sampleRate} Hz`);
      const modem = new FakeModem(sampleRate);
      modems.push(modem);
      return modem;
    },
  };
  return { core, modems };
}

/** A session on a fake core, started at 48 kHz. */
async function startedSession() {
  const { core, modems } = fakeCore();
  const out: WorkerOut[] = [];
  const handle = createSession(Promise.resolve(core), (reply) => out.push(reply));
  const audio = new FakeAudioPort();
  await handle({ type: 'start', jobId: 1, sampleRate: 48_000, port: audio.port });
  const modem = modems[0];
  if (!modem) throw new Error('no modem was created');
  return { handle, out, audio, modem, modems };
}

const events = (out: WorkerOut[]): CoreEvent[] => out.flatMap((reply) => (reply.type === 'event' ? [reply.event] : []));

describe('protocol', () => {
  it('recognises requests', () => {
    const { port1 } = new MessageChannel();
    expect(isWorkerIn({ type: 'version', jobId: 1 })).toBe(true);
    expect(isWorkerIn({ type: 'stop', jobId: 1 })).toBe(true);
    expect(isWorkerIn({ type: 'start', jobId: 1, sampleRate: 48_000, port: port1 })).toBe(true);
    expect(isWorkerIn({ type: 'sendText', jobId: 1, text: 'hi' })).toBe(true);
    expect(isWorkerIn({ type: 'version' })).toBe(false);
    expect(isWorkerIn({ type: 'start', jobId: 1, sampleRate: 48_000, port: {} })).toBe(false);
    expect(isWorkerIn({ type: 'start', jobId: 1, sampleRate: '48000', port: port1 })).toBe(false);
    expect(isWorkerIn({ type: 'sendText', jobId: 1 })).toBe(false);
    expect(isWorkerIn({ type: 'other', jobId: 1 })).toBe(false);
    expect(isWorkerIn(null)).toBe(false);
    port1.close();
  });

  it('recognises replies and events', () => {
    const good: WorkerOut[] = [
      { type: 'version', jobId: 1, version: '0.1.0' },
      { type: 'started', jobId: 1, maxTextBytes: 255 },
      { type: 'sent', jobId: 1, seconds: 2.5 },
      { type: 'stopped', jobId: 1 },
      { type: 'error', jobId: 1, message: 'x' },
      { type: 'event', event: { kind: 'message', text: 'hi', snrDb: 10, freqOffsetHz: 0 } },
      { type: 'event', event: { kind: 'frameLost', snrDb: -3, freqOffsetHz: 12 } },
      { type: 'event', event: { kind: 'txEnd' } },
    ];
    for (const reply of good) expect({ reply, ok: isWorkerOut(reply) }).toEqual({ reply, ok: true });
    const bad = [
      { type: 'version', jobId: 1 },
      { type: 'error', message: 'x' },
      { type: 'sent', jobId: 1, seconds: Number.NaN },
      { type: 'event', event: { kind: 'message', snrDb: 10, freqOffsetHz: 0 } },
      { type: 'event', event: { kind: 'unknown' } },
      { type: 'event' },
      'version',
    ];
    for (const reply of bad) expect({ reply, ok: isWorkerOut(reply) }).toEqual({ reply, ok: false });
  });
});

describe('session', () => {
  it('answers with the version of the core', async () => {
    const out: WorkerOut[] = [];
    await createSession(Promise.resolve(fakeCore().core), (reply) => out.push(reply))({ type: 'version', jobId: 7 });
    expect(out).toEqual([{ type: 'version', jobId: 7, version: '1.2.3' }]);
  });

  it('answers with an error when the core did not load', async () => {
    const out: WorkerOut[] = [];
    await createSession(Promise.reject(new Error('no wasm')), (reply) => out.push(reply))({ type: 'version', jobId: 7 });
    expect(out).toEqual([{ type: 'error', jobId: 7, message: 'no wasm' }]);
  });

  it('starts a modem at the sample rate of the device', async () => {
    const { out, modem } = await startedSession();
    expect(out).toEqual([{ type: 'started', jobId: 1, maxTextBytes: 5 }]);
    expect(modem.sampleRate).toBe(48_000);
  });

  it('reports a sample rate the core refuses', async () => {
    const out: WorkerOut[] = [];
    const handle = createSession(Promise.resolve(fakeCore().core), (reply) => out.push(reply));
    await handle({ type: 'start', jobId: 3, sampleRate: 100, port: new FakeAudioPort().port });
    expect(out).toEqual([{ type: 'error', jobId: 3, message: 'unsupported sample rate: 100 Hz' }]);
  });

  it('feeds microphone blocks to the modem and passes its events on', async () => {
    const { out, audio, modem } = await startedSession();
    out.length = 0;
    const block = new Float32Array(2048);
    audio.hear(block);
    expect(modem.heard).toEqual([block]);
    expect(out).toEqual([]);

    modem.events.push(
      { kind: 'message', text: 'hi', snrDb: 12, freqOffsetHz: -1 },
      { kind: 'frameLost', text: '', snrDb: -5, freqOffsetHz: 3 },
    );
    audio.hear(block);
    expect(events(out)).toEqual([
      { kind: 'message', text: 'hi', snrDb: 12, freqOffsetHz: -1 },
      { kind: 'frameLost', snrDb: -5, freqOffsetHz: 3 },
    ]);
  });

  it('frees the event objects of the core and ignores kinds it does not know', async () => {
    const { out, audio, modem } = await startedSession();
    out.length = 0;
    let freed = 0;
    const free = () => {
      freed++;
    };
    modem.events.push(
      { kind: 'future', text: '', snrDb: 0, freqOffsetHz: 0, free },
      { kind: 'frameLost', text: '', snrDb: 0, freqOffsetHz: 0, free },
    );
    audio.hear(new Float32Array(16));
    expect(freed).toBe(2);
    expect(events(out)).toEqual([{ kind: 'frameLost', snrDb: 0, freqOffsetHz: 0 }]);
  });

  it('sends the whole transmission to the worklet at once', async () => {
    const { handle, out, audio } = await startedSession();
    out.length = 0;
    await handle({ type: 'sendText', jobId: 2, text: 'ab' });
    expect(out).toEqual([{ type: 'sent', jobId: 2, seconds: 2 }]);
    expect(audio.played.map((samples) => samples.length)).toEqual([96_000]);
  });

  it('does not listen to itself: deaf while transmitting and half a second after', async () => {
    const { handle, out, audio, modem } = await startedSession();
    await handle({ type: 'sendText', jobId: 2, text: 'a' });
    out.length = 0;
    const block = new Float32Array(8000);

    audio.hear(block);
    expect(modem.heard).toEqual([]);

    audio.hear(AUDIO_IDLE);
    expect(events(out)).toEqual([{ kind: 'txEnd' }]);
    // 0.5 s at 48 kHz is three blocks of 8000
    for (let i = 0; i < 3; i++) audio.hear(block);
    expect(modem.heard).toEqual([]);
    audio.hear(block);
    expect(modem.heard).toEqual([block]);

    // 'idle' without a transmission means nothing
    out.length = 0;
    audio.hear(AUDIO_IDLE);
    audio.hear(block);
    expect(out).toEqual([]);
    expect(modem.heard).toHaveLength(2);
  });

  it('refuses a text the core refuses, and anything before the start', async () => {
    const { handle, out, audio } = await startedSession();
    out.length = 0;
    await handle({ type: 'sendText', jobId: 2, text: 'too long' });
    expect(out).toEqual([{ type: 'error', jobId: 2, message: 'the text is longer than 5 bytes' }]);
    expect(audio.played).toEqual([]);

    const idle: WorkerOut[] = [];
    await createSession(Promise.resolve(fakeCore().core), (reply) => idle.push(reply))({ type: 'sendText', jobId: 9, text: 'a' });
    expect(idle).toEqual([{ type: 'error', jobId: 9, message: 'the modem is not started' }]);
  });

  it('stop frees the modem and closes the audio channel', async () => {
    const { handle, out, audio, modem } = await startedSession();
    out.length = 0;
    await handle({ type: 'stop', jobId: 4 });
    expect(out).toEqual([{ type: 'stopped', jobId: 4 }]);
    expect(modem.freed).toBe(true);
    expect(audio.closed).toBe(true);
    expect(audio.onmessage).toBeNull();
  });

  it('a second start replaces the first modem', async () => {
    const { handle, audio, modem, modems } = await startedSession();
    const second = new FakeAudioPort();
    await handle({ type: 'start', jobId: 5, sampleRate: 44_100, port: second.port });
    expect(modem.freed).toBe(true);
    expect(audio.closed).toBe(true);
    expect(modems.map((m) => m.sampleRate)).toEqual([48_000, 44_100]);
    second.hear(new Float32Array(8));
    expect(modems[1]?.heard).toHaveLength(1);
  });
});

class FakeWorker implements WorkerPort {
  sent: WorkerIn[] = [];
  transfers: Transferable[][] = [];
  private onMessage: (event: { data: unknown }) => void = () => {};
  private onError: () => void = () => {};
  postMessage(message: WorkerIn, transfer: Transferable[]) {
    this.sent.push(message);
    this.transfers.push(transfer);
  }
  addEventListener(type: 'message' | 'error', listener: ((event: { data: unknown }) => void) | (() => void)) {
    if (type === 'message') this.onMessage = listener;
    else this.onError = listener as () => void;
  }
  reply(data: unknown) {
    this.onMessage({ data });
  }
  fail() {
    this.onError();
  }
}

function setup() {
  const workers: FakeWorker[] = [];
  const client = coreClient(() => {
    const worker = new FakeWorker();
    workers.push(worker);
    return worker;
  });
  return { client, workers };
}

describe('coreClient', () => {
  it('creates the worker on the first request only', () => {
    const { client, workers } = setup();
    expect(workers).toHaveLength(0);
    void client.version();
    void client.version();
    expect(workers).toHaveLength(1);
    expect(workers[0]?.sent).toEqual([
      { type: 'version', jobId: 1 },
      { type: 'version', jobId: 2 },
    ]);
  });

  it('resolves each request with its own reply', async () => {
    const { client, workers } = setup();
    const first = client.version();
    const second = client.version();
    workers[0]?.reply({ type: 'version', jobId: 2, version: 'second' });
    workers[0]?.reply({ type: 'version', jobId: 1, version: 'first' });
    expect(await Promise.all([first, second])).toEqual(['first', 'second']);
  });

  it('ignores messages it does not understand', async () => {
    const { client, workers } = setup();
    const request = client.version();
    workers[0]?.reply({ jobId: 1 });
    workers[0]?.reply({ type: 'version', jobId: 99, version: 'other' });
    workers[0]?.reply({ type: 'version', jobId: 1, version: '0.1.0' });
    expect(await request).toBe('0.1.0');
  });

  it('rejects on an error reply and on a reply of the wrong type', async () => {
    const { client, workers } = setup();
    const failed = client.version();
    workers[0]?.reply({ type: 'error', jobId: 1, message: 'no wasm' });
    await expect(failed).rejects.toThrow('no wasm');
    const confused = client.version();
    workers[0]?.reply({ type: 'stopped', jobId: 2 });
    await expect(confused).rejects.toThrow('unexpected reply of the modem worker: stopped');
  });

  it('rejects everything pending when the worker itself fails', async () => {
    const { client, workers } = setup();
    const requests = [client.version(), client.version()];
    workers[0]?.fail();
    expect((await Promise.allSettled(requests)).map((r) => r.status)).toEqual(['rejected', 'rejected']);
    await expect(requests[0]).rejects.toThrow('the modem worker failed');
  });

  it('hands the audio port over to the worker on start', async () => {
    const { client, workers } = setup();
    const { port1, port2 } = new MessageChannel();
    const started = client.start(44_100, port1);
    expect(workers[0]?.sent).toEqual([{ type: 'start', jobId: 1, sampleRate: 44_100, port: port1 }]);
    expect(workers[0]?.transfers).toEqual([[port1]]);
    workers[0]?.reply({ type: 'started', jobId: 1, maxTextBytes: 255 });
    expect(await started).toEqual({ maxTextBytes: 255 });
    port1.close();
    port2.close();
  });

  it('sends a text and stops', async () => {
    const { client, workers } = setup();
    const sent = client.sendText('hello');
    workers[0]?.reply({ type: 'sent', jobId: 1, seconds: 3.5 });
    expect(await sent).toEqual({ seconds: 3.5 });
    const stopped = client.stop();
    workers[0]?.reply({ type: 'stopped', jobId: 2 });
    await expect(stopped).resolves.toBeUndefined();
    expect(workers[0]?.sent.slice(0, 2)).toEqual([
      { type: 'sendText', jobId: 1, text: 'hello' },
      { type: 'stop', jobId: 2 },
    ]);
  });

  it('passes events to the listeners until they unsubscribe', async () => {
    const { client, workers } = setup();
    const heard: CoreEvent[] = [];
    const unsubscribe = client.onEvent((event) => heard.push(event));
    const request = client.version();
    workers[0]?.reply({ type: 'event', event: { kind: 'txEnd' } });
    unsubscribe();
    workers[0]?.reply({ type: 'event', event: { kind: 'txEnd' } });
    workers[0]?.reply({ type: 'version', jobId: 1, version: 'x' });
    await request;
    expect(heard).toEqual([{ kind: 'txEnd' }]);
  });
});

const realCore: Core = { version, maxTextBytes, createModem: (sampleRate) => new Modem(sampleRate) };

/** A session on the real core, started at 48 kHz. */
async function realSession(jobId: number) {
  const out: WorkerOut[] = [];
  const audio = new FakeAudioPort();
  const handle = createSession(Promise.resolve(realCore), (reply) => out.push(reply));
  await handle({ type: 'start', jobId, sampleRate: 48_000, port: audio.port });
  return { handle, out, audio };
}

describe('the WASM core', () => {
  beforeAll(async () => {
    // Node has no fetch for file URLs: give the bytes (run `npm run wasm` first).
    await init({ module_or_path: readFileSync(resolve(__dirname, '../wasm/modem_wasm_bg.wasm')) });
  });

  it('reports the version of the Rust workspace', () => {
    const cargo = readFileSync(resolve(__dirname, '../../Cargo.toml'), 'utf8');
    const expected = /^version = "(.+)"$/m.exec(cargo)?.[1];
    expect(expected).toBeDefined();
    expect(version()).toBe(expected);
    expect(maxTextBytes()).toBe(255);
  });

  it('carries a text from one session to another, as sound at 48 kHz', async () => {
    const [alice, bob] = [await realSession(1), await realSession(2)];

    await alice.handle({ type: 'sendText', jobId: 3, text: 'Привет из браузера' });
    const sound = alice.audio.played[0];
    if (!sound) throw new Error('nothing was transmitted');
    // the worklet delivers the microphone in blocks of 2048
    for (let at = 0; at < sound.length; at += 2048) bob.audio.hear(sound.slice(at, at + 2048));

    expect(events(bob.out)).toEqual([
      { kind: 'message', text: 'Привет из браузера', snrDb: expect.any(Number), freqOffsetHz: expect.any(Number) },
    ]);
    const sent = alice.out.find((reply) => reply.type === 'sent');
    expect(sent).toEqual({ type: 'sent', jobId: 3, seconds: expect.closeTo(sound.length / 48_000, 5) });
    await alice.handle({ type: 'stop', jobId: 4 });
    await bob.handle({ type: 'stop', jobId: 5 });
  });

  it('refuses a text longer than a frame', async () => {
    const out: WorkerOut[] = [];
    const handle = createSession(Promise.resolve(realCore), (reply) => out.push(reply));
    await handle({ type: 'start', jobId: 1, sampleRate: 44_100, port: new FakeAudioPort().port });
    await handle({ type: 'sendText', jobId: 2, text: 'я'.repeat(128) });
    expect(out[1]).toEqual({ type: 'error', jobId: 2, message: 'the text is longer than 255 bytes' });
  });
});

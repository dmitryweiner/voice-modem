import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { beforeAll, describe, expect, it } from 'vitest';
import init, { maxTextBytes, Modem, version } from '../wasm/modem_wasm.js';
import { coreClient, type WorkerPort } from './client';
import { createSession, type Core, type CoreModem, type CoreModemEvent, type ModemOptions } from './handler';
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
  texts: string[] = [];
  calls: (string | [string, number])[] = [];
  freed = false;
  /** Samples the modem wants played; the test sets it. */
  pending = 0;
  constructor(readonly options: ModemOptions) {}
  sendText(text: string) {
    if (text.length > 5) throw new Error('the text is longer than 5 bytes');
    this.texts.push(text);
    return this.texts.length;
  }
  hello() {
    this.calls.push('hello');
  }
  pttPressed() {
    this.calls.push('ptt');
  }
  cancel(id: number) {
    this.calls.push(['cancel', id]);
    return true;
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

/** An event of the core with every field, as the WASM object has them. */
const coreEvent = (fields: Partial<CoreModemEvent> & { kind: string }): CoreModemEvent => ({
  text: '',
  id: 0,
  attempt: 0,
  mode: 0,
  bitRate: 0,
  station: 0,
  snrDb: 0,
  freqOffsetHz: 0,
  ...fields,
});

function fakeCore() {
  const modems: FakeModem[] = [];
  const core: Core = {
    version: () => '1.2.3',
    maxTextBytes: () => 5,
    createModem(options) {
      if (options.sampleRate < 8000) throw new Error(`unsupported sample rate: ${options.sampleRate} Hz`);
      const modem = new FakeModem(options);
      modems.push(modem);
      return modem;
    },
  };
  return { core, modems };
}

/** Random numbers that are not: 0x12345678, 0x12345679… */
function counter() {
  let next = 0x12345678;
  return () => next++;
}

const START = { type: 'start', sampleRate: 48_000, name: 'Alice', manualPtt: false, mode: null } as const;

/** A session on a fake core, started at 48 kHz. */
async function startedSession() {
  const { core, modems } = fakeCore();
  const out: WorkerOut[] = [];
  const handle = createSession(Promise.resolve(core), (reply) => out.push(reply), counter());
  const audio = new FakeAudioPort();
  await handle({ ...START, jobId: 1, port: audio.port });
  const modem = modems[0];
  if (!modem) throw new Error('no modem was created');
  return { handle, out, audio, modem, modems };
}

/** A session that was never started. */
function idleSession(core: Promise<Core> = Promise.resolve(fakeCore().core)) {
  const out: WorkerOut[] = [];
  return { out, handle: createSession(core, (reply) => out.push(reply), counter()) };
}

const events = (out: WorkerOut[]): CoreEvent[] => out.flatMap((reply) => (reply.type === 'event' ? [reply.event] : []));

describe('protocol', () => {
  it('recognises requests', () => {
    const { port1 } = new MessageChannel();
    const start = { ...START, jobId: 1, port: port1 };
    const good = [
      { type: 'version', jobId: 1 },
      { type: 'stop', jobId: 1 },
      { type: 'hello', jobId: 1 },
      { type: 'ptt', jobId: 1 },
      start,
      { type: 'sendText', jobId: 1, text: 'hi' },
      { type: 'cancel', jobId: 1, id: 3 },
    ];
    for (const request of good) expect({ request, ok: isWorkerIn(request) }).toEqual({ request, ok: true });
    const bad = [
      { type: 'version' },
      { ...start, port: {} },
      { ...start, sampleRate: '48000' },
      { ...start, name: undefined },
      { ...start, manualPtt: 'no' },
      { ...start, mode: 'fast' },
      { ...start, mode: undefined },
      { type: 'sendText', jobId: 1 },
      { type: 'other', jobId: 1 },
      null,
    ];
    for (const request of bad) expect({ request, ok: isWorkerIn(request) }).toEqual({ request, ok: false });
    port1.close();
  });

  it('recognises replies and events', () => {
    const good: WorkerOut[] = [
      { type: 'version', jobId: 1, version: '0.1.0' },
      { type: 'started', jobId: 1, maxTextBytes: 1000 },
      { type: 'queued', jobId: 1, id: 3 },
      { type: 'done', jobId: 1 },
      { type: 'error', jobId: 1, message: 'x' },
      { type: 'event', event: { kind: 'message', text: 'hi', station: 7, snrDb: 10, freqOffsetHz: 0 } },
      { type: 'event', event: { kind: 'hello', name: '', station: 7 } },
      { type: 'event', event: { kind: 'delivered', id: 1 } },
      { type: 'event', event: { kind: 'failed', id: 1 } },
      { type: 'event', event: { kind: 'sent', id: 1 } },
      { type: 'event', event: { kind: 'retry', id: 1, attempt: 2 } },
      { type: 'event', event: { kind: 'frameLost', snrDb: -3, freqOffsetHz: 12 } },
      { type: 'event', event: { kind: 'txRequest' } },
      { type: 'event', event: { kind: 'txMode', mode: 1, bitRate: 125 } },
      { type: 'event', event: { kind: 'txStart', seconds: 2.5 } },
      { type: 'event', event: { kind: 'txEnd' } },
    ];
    for (const reply of good) expect({ reply, ok: isWorkerOut(reply) }).toEqual({ reply, ok: true });
    const bad = [
      { type: 'version', jobId: 1 },
      { type: 'error', message: 'x' },
      { type: 'queued', jobId: 1, id: Number.NaN },
      { type: 'event', event: { kind: 'message', text: 'hi', snrDb: 10, freqOffsetHz: 0 } },
      { type: 'event', event: { kind: 'retry', id: 1 } },
      { type: 'event', event: { kind: 'txStart' } },
      { type: 'event', event: { kind: 'unknown' } },
      { type: 'event' },
      'version',
    ];
    for (const reply of bad) expect({ reply, ok: isWorkerOut(reply) }).toEqual({ reply, ok: false });
  });
});

describe('session', () => {
  it('answers with the version of the core', async () => {
    const { handle, out } = idleSession();
    await handle({ type: 'version', jobId: 7 });
    expect(out).toEqual([{ type: 'version', jobId: 7, version: '1.2.3' }]);
  });

  it('answers with an error when the core did not load', async () => {
    const { handle, out } = idleSession(Promise.reject(new Error('no wasm')));
    await handle({ type: 'version', jobId: 7 });
    expect(out).toEqual([{ type: 'error', jobId: 7, message: 'no wasm' }]);
  });

  it('starts a modem for the device, with a random address and seed', async () => {
    const { out, modem } = await startedSession();
    expect(out).toEqual([{ type: 'started', jobId: 1, maxTextBytes: 5 }]);
    expect(modem.options).toEqual({ sampleRate: 48_000, name: 'Alice', manualPtt: false, mode: null, station: 0x5678, seed: 0x12345679 });
  });

  it('reports a sample rate the core refuses', async () => {
    const { handle, out } = idleSession();
    await handle({ ...START, jobId: 3, sampleRate: 100, port: new FakeAudioPort().port });
    expect(out).toEqual([{ type: 'error', jobId: 3, message: 'unsupported sample rate: 100 Hz' }]);
  });

  it('feeds every microphone block to the modem and passes its events on', async () => {
    const { out, audio, modem } = await startedSession();
    out.length = 0;
    const block = new Float32Array(2048);
    audio.hear(block);
    expect(modem.heard).toEqual([block]);
    expect(out).toEqual([]);

    modem.events.push(
      coreEvent({ kind: 'message', text: 'hi', station: 9, snrDb: 12, freqOffsetHz: -1 }),
      coreEvent({ kind: 'hello', text: 'Bob', station: 9 }),
      coreEvent({ kind: 'delivered', id: 4 }),
      coreEvent({ kind: 'failed', id: 5 }),
      coreEvent({ kind: 'sent', id: 6 }),
      coreEvent({ kind: 'retry', id: 7, attempt: 3 }),
      coreEvent({ kind: 'frameLost', snrDb: -5, freqOffsetHz: 3 }),
      coreEvent({ kind: 'txMode', mode: 2, bitRate: 250 }),
      coreEvent({ kind: 'txRequest' }),
    );
    audio.hear(block);
    expect(events(out)).toEqual([
      { kind: 'message', text: 'hi', station: 9, snrDb: 12, freqOffsetHz: -1 },
      { kind: 'hello', name: 'Bob', station: 9 },
      { kind: 'delivered', id: 4 },
      { kind: 'failed', id: 5 },
      { kind: 'sent', id: 6 },
      { kind: 'retry', id: 7, attempt: 3 },
      { kind: 'frameLost', snrDb: -5, freqOffsetHz: 3 },
      { kind: 'txMode', mode: 2, bitRate: 250 },
      { kind: 'txRequest' },
    ]);
  });

  it('frees the event objects of the core and ignores kinds it does not know', async () => {
    const { out, audio, modem } = await startedSession();
    out.length = 0;
    let freed = 0;
    const free = () => {
      freed++;
    };
    modem.events.push(coreEvent({ kind: 'future', free }), coreEvent({ kind: 'txRequest', free }));
    audio.hear(new Float32Array(16));
    expect(freed).toBe(2);
    expect(events(out)).toEqual([{ kind: 'txRequest' }]);
  });

  it('plays what the modem wants to transmit, all at once, and tells the page', async () => {
    const { out, audio, modem } = await startedSession();
    out.length = 0;
    audio.hear(new Float32Array(2048));
    expect(audio.played).toEqual([]);

    modem.pending = 96_000;
    audio.hear(new Float32Array(2048));
    expect(audio.played.map((samples) => samples.length)).toEqual([96_000]);
    expect(events(out)).toEqual([{ kind: 'txStart', seconds: 2 }]);

    // the modem keeps hearing the microphone meanwhile: what to ignore is its own business
    audio.hear(new Float32Array(2048));
    expect(modem.heard).toHaveLength(3);
    audio.hear(AUDIO_IDLE);
    expect(events(out)).toEqual([{ kind: 'txStart', seconds: 2 }, { kind: 'txEnd' }]);
  });

  it('queues a text and returns its id', async () => {
    const { handle, out, modem } = await startedSession();
    out.length = 0;
    await handle({ type: 'sendText', jobId: 2, text: 'ab' });
    await handle({ type: 'sendText', jobId: 3, text: 'cd' });
    expect(out).toEqual([
      { type: 'queued', jobId: 2, id: 1 },
      { type: 'queued', jobId: 3, id: 2 },
    ]);
    expect(modem.texts).toEqual(['ab', 'cd']);
  });

  it('passes the greeting and the push-to-talk button to the modem', async () => {
    const { handle, out, modem } = await startedSession();
    out.length = 0;
    await handle({ type: 'hello', jobId: 2 });
    await handle({ type: 'ptt', jobId: 3 });
    expect(modem.calls).toEqual(['hello', 'ptt']);
    expect(out).toEqual([
      { type: 'done', jobId: 2 },
      { type: 'done', jobId: 3 },
    ]);
  });

  it('takes a text back by its id', async () => {
    const { handle, out, modem } = await startedSession();
    out.length = 0;
    await handle({ type: 'cancel', jobId: 2, id: 7 });
    expect(modem.calls).toEqual([['cancel', 7]]);
    expect(out).toEqual([{ type: 'done', jobId: 2 }]);
  });

  it('refuses a text the core refuses, and anything before the start', async () => {
    const { handle, out } = await startedSession();
    out.length = 0;
    await handle({ type: 'sendText', jobId: 2, text: 'too long' });
    expect(out).toEqual([{ type: 'error', jobId: 2, message: 'the text is longer than 5 bytes' }]);

    const idle = idleSession();
    await idle.handle({ type: 'sendText', jobId: 9, text: 'a' });
    await idle.handle({ type: 'hello', jobId: 10 });
    await idle.handle({ type: 'ptt', jobId: 11 });
    expect(idle.out.map((reply) => reply.type === 'error' && reply.message)).toEqual(
      Array.from({ length: 3 }, () => 'the modem is not started'),
    );
  });

  it('stop frees the modem and closes the audio channel', async () => {
    const { handle, out, audio, modem } = await startedSession();
    out.length = 0;
    await handle({ type: 'stop', jobId: 4 });
    expect(out).toEqual([{ type: 'done', jobId: 4 }]);
    expect(modem.freed).toBe(true);
    expect(audio.closed).toBe(true);
    expect(audio.onmessage).toBeNull();
  });

  it('a second start replaces the first modem', async () => {
    const { handle, audio, modem, modems } = await startedSession();
    const second = new FakeAudioPort();
    await handle({ ...START, jobId: 5, sampleRate: 44_100, manualPtt: true, mode: 3, port: second.port });
    expect(modem.freed).toBe(true);
    expect(audio.closed).toBe(true);
    expect(modems.map((m) => [m.options.sampleRate, m.options.manualPtt])).toEqual([
      [48_000, false],
      [44_100, true],
    ]);
    // each start draws a new address
    expect(modems[1]?.options.station).not.toBe(modems[0]?.options.station);
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
    workers[0]?.reply({ type: 'done', jobId: 2 });
    await expect(confused).rejects.toThrow('unexpected reply of the modem worker: done');
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
    const started = client.start(44_100, port1, { name: 'Bob', manualPtt: true, mode: 2 });
    expect(workers[0]?.sent).toEqual([{ type: 'start', jobId: 1, sampleRate: 44_100, port: port1, name: 'Bob', manualPtt: true, mode: 2 }]);
    expect(workers[0]?.transfers).toEqual([[port1]]);
    workers[0]?.reply({ type: 'started', jobId: 1, maxTextBytes: 1000 });
    expect(await started).toEqual({ maxTextBytes: 1000 });
    port1.close();
    port2.close();
  });

  it('sends a text, greets, presses the button and stops', async () => {
    const { client, workers } = setup();
    const sent = client.sendText('hello');
    workers[0]?.reply({ type: 'queued', jobId: 1, id: 8 });
    expect(await sent).toEqual({ id: 8 });
    const rest = [client.hello(), client.ptt(), client.cancel(8), client.stop()];
    for (const jobId of [2, 3, 4, 5]) workers[0]?.reply({ type: 'done', jobId });
    expect(await Promise.all(rest)).toEqual([undefined, undefined, undefined, undefined]);
    expect(workers[0]?.sent).toEqual([
      { type: 'sendText', jobId: 1, text: 'hello' },
      { type: 'hello', jobId: 2 },
      { type: 'ptt', jobId: 3 },
      { type: 'cancel', jobId: 4, id: 8 },
      { type: 'stop', jobId: 5 },
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

const realCore: Core = {
  version,
  maxTextBytes,
  createModem: ({ sampleRate, station, name, manualPtt, seed, mode }) => new Modem(sampleRate, station, name, manualPtt, seed, mode ?? 255),
};

/** A session on the real core, started at 48 kHz. */
async function realSession(name: string, first: number) {
  const out: WorkerOut[] = [];
  const audio = new FakeAudioPort();
  let next = first;
  const handle = createSession(
    Promise.resolve(realCore),
    (reply) => out.push(reply),
    () => next++,
  );
  await handle({ ...START, jobId: 0, name, port: audio.port });
  return { handle, out, audio, incoming: [] as Float32Array[] };
}
type RealSession = Awaited<ReturnType<typeof realSession>>;

/**
 * Lets two sessions hear each other for a while, as two worklets would:
 * each gets microphone blocks of 2048 samples all the time, silence included.
 */
function converse(a: RealSession, b: RealSession, seconds: number) {
  const next = (side: RealSession) => {
    const block = new Float32Array(2048);
    let filled = 0;
    while (filled < 2048 && side.incoming[0]) {
      const head = side.incoming[0];
      const count = Math.min(2048 - filled, head.length);
      block.set(head.subarray(0, count), filled);
      filled += count;
      if (count === head.length) side.incoming.shift();
      else side.incoming[0] = head.subarray(count);
    }
    return block;
  };
  for (let block = 0; block < (seconds * 48_000) / 2048; block++) {
    for (const [side, other] of [
      [a, b],
      [b, a],
    ] as const) {
      const before = side.audio.played.length;
      side.audio.hear(next(side));
      other.incoming.push(...side.audio.played.slice(before));
    }
  }
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
    expect(maxTextBytes()).toBe(1000);
  });

  it('delivers a text from one session to another as sound at 48 kHz, and the acknowledgement back', async () => {
    const [alice, bob] = [await realSession('Alice', 100), await realSession('Bob', 200)];
    await alice.handle({ type: 'sendText', jobId: 3, text: 'Привет из браузера' });
    expect(alice.out.at(-1)).toEqual({ type: 'queued', jobId: 3, id: 1 });
    converse(alice, bob, 9);

    expect(events(bob.out)).toEqual([
      { kind: 'message', text: 'Привет из браузера', station: 100, snrDb: expect.any(Number), freqOffsetHz: expect.any(Number) },
      { kind: 'txStart', seconds: expect.closeTo(1.35, 1) },
    ]);
    expect(events(alice.out)).toEqual([
      { kind: 'txStart', seconds: expect.closeTo(6.44, 1) },
      { kind: 'delivered', id: 1 },
      // heard as through a cable: the other side advises the fastest mode, one step up for now
      { kind: 'txMode', mode: 1, bitRate: 125 },
    ]);
    await alice.handle({ type: 'stop', jobId: 4 });
    await bob.handle({ type: 'stop', jobId: 5 });
  });

  it('exchanges greetings with the names', async () => {
    const [alice, bob] = [await realSession('Alice', 100), await realSession('Боб', 200)];
    await alice.handle({ type: 'hello', jobId: 3 });
    converse(alice, bob, 14);
    expect(events(bob.out).filter((event) => event.kind === 'hello')).toEqual([{ kind: 'hello', name: 'Alice', station: 100 }]);
    expect(events(alice.out).filter((event) => event.kind === 'hello')).toEqual([{ kind: 'hello', name: 'Боб', station: 200 }]);
  });

  it('refuses texts the core cannot send', async () => {
    const { handle, out } = await realSession('', 1);
    await handle({ type: 'sendText', jobId: 2, text: 'я'.repeat(501) });
    await handle({ type: 'sendText', jobId: 3, text: '' });
    expect(out.slice(1)).toEqual([
      { type: 'error', jobId: 2, message: 'the text is longer than 1000 bytes' },
      { type: 'error', jobId: 3, message: 'the text is empty' },
    ]);
  });
});

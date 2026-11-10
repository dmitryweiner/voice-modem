import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import init, { version } from '../wasm/modem_wasm.js';
import { coreClient, type WorkerPort } from './client';
import { handle } from './handler';
import { isWorkerIn, isWorkerOut, type WorkerIn } from './protocol';

class FakeWorker implements WorkerPort {
  sent: WorkerIn[] = [];
  private onMessage: (event: { data: unknown }) => void = () => {};
  private onError: () => void = () => {};
  postMessage(message: WorkerIn) {
    this.sent.push(message);
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

describe('protocol', () => {
  it('recognises requests', () => {
    expect(isWorkerIn({ type: 'version', jobId: 1 })).toBe(true);
    expect(isWorkerIn({ type: 'version' })).toBe(false);
    expect(isWorkerIn({ type: 'other', jobId: 1 })).toBe(false);
    expect(isWorkerIn(null)).toBe(false);
  });

  it('recognises replies', () => {
    expect(isWorkerOut({ type: 'version', jobId: 1, version: '0.1.0' })).toBe(true);
    expect(isWorkerOut({ type: 'error', jobId: 1, message: 'x' })).toBe(true);
    expect(isWorkerOut({ type: 'version', jobId: 1 })).toBe(false);
    expect(isWorkerOut({ type: 'error', message: 'x' })).toBe(false);
    expect(isWorkerOut('version')).toBe(false);
  });
});

describe('handle', () => {
  it('answers with the version of the core', async () => {
    const core = Promise.resolve({ version: () => '1.2.3' });
    expect(await handle({ type: 'version', jobId: 7 }, core)).toEqual({ type: 'version', jobId: 7, version: '1.2.3' });
  });

  it('answers with an error when the core did not load', async () => {
    const core = Promise.reject(new Error('no wasm'));
    expect(await handle({ type: 'version', jobId: 7 }, core)).toEqual({ type: 'error', jobId: 7, message: 'no wasm' });
  });
});

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

  it('rejects on an error reply', async () => {
    const { client, workers } = setup();
    const request = client.version();
    workers[0]?.reply({ type: 'error', jobId: 1, message: 'no wasm' });
    await expect(request).rejects.toThrow('no wasm');
  });

  it('rejects everything pending when the worker itself fails', async () => {
    const { client, workers } = setup();
    const requests = [client.version(), client.version()];
    workers[0]?.fail();
    await expect(Promise.all(requests)).rejects.toThrow('the modem worker failed');
    expect((await Promise.allSettled(requests)).map((r) => r.status)).toEqual(['rejected', 'rejected']);
  });
});

describe('the WASM binding', () => {
  it('reports the version of the Rust workspace', async () => {
    const cargo = readFileSync(resolve(__dirname, '../../Cargo.toml'), 'utf8');
    const expected = /^version = "(.+)"$/m.exec(cargo)?.[1];
    // Node has no fetch for file URLs: give the bytes (run `npm run wasm` first).
    await init({ module_or_path: readFileSync(resolve(__dirname, '../wasm/modem_wasm_bg.wasm')) });
    expect(expected).toBeDefined();
    expect(version()).toBe(expected);
  });
});

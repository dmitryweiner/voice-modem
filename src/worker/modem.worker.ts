// The modem worker: a thin adapter between postMessage and the WASM core.
// The only module that imports WASM; the main thread never does.
import init, { maxTextBytes, Modem, version } from '../wasm/modem_wasm.js';
import { createSession, type Core } from './handler';
import { isWorkerIn } from './protocol';

const core: Promise<Core> = init().then(() => ({ version, maxTextBytes, createModem: (sampleRate) => new Modem(sampleRate) }));
// A failed load is reported in the reply to each request.
core.catch(() => {});

const handle = createSession(core, (reply) => postMessage(reply));

addEventListener('message', (event: MessageEvent<unknown>) => {
  if (isWorkerIn(event.data)) void handle(event.data);
});

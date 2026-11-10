// The modem worker: a thin adapter between postMessage and the WASM core.
// The only module that imports WASM; the main thread never does.
import init, { version } from '../wasm/modem_wasm.js';
import { handle, type Core } from './handler';
import { isWorkerIn } from './protocol';

const core: Promise<Core> = init().then(() => ({ version }));
// A failed load is reported in the reply to each request.
core.catch(() => {});

addEventListener('message', (event: MessageEvent<unknown>) => {
  if (!isWorkerIn(event.data)) return;
  void handle(event.data, core).then((reply) => postMessage(reply));
});

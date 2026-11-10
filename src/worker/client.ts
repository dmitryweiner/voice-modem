import { isWorkerOut, type WorkerIn } from './protocol';

/** What the UI can ask the modem core for. */
export interface CoreClient {
  version(): Promise<string>;
}

/** The part of `Worker` the client uses; tests replace it. */
export interface WorkerPort {
  postMessage(message: WorkerIn): void;
  addEventListener(type: 'message', listener: (event: { data: unknown }) => void): void;
  addEventListener(type: 'error', listener: () => void): void;
}

interface Pending {
  resolve: (version: string) => void;
  reject: (error: Error) => void;
}

/** A client of the modem worker. The worker is created on the first request. */
export function coreClient(createWorker: () => WorkerPort): CoreClient {
  let worker: WorkerPort | null = null;
  let nextJobId = 1;
  const pending = new Map<number, Pending>();

  const start = () => {
    const created = createWorker();
    created.addEventListener('message', ({ data }) => {
      if (!isWorkerOut(data)) return;
      const job = pending.get(data.jobId);
      if (!job) return;
      pending.delete(data.jobId);
      if (data.type === 'error') job.reject(new Error(data.message));
      else job.resolve(data.version);
    });
    // The worker script itself failed (not loaded, threw at start): nothing will be answered.
    created.addEventListener('error', () => {
      for (const job of pending.values()) job.reject(new Error('the modem worker failed'));
      pending.clear();
    });
    return created;
  };

  return {
    version: () =>
      new Promise((resolve, reject) => {
        worker ??= start();
        const jobId = nextJobId++;
        pending.set(jobId, { resolve, reject });
        // oxlint-disable-next-line unicorn/require-post-message-target-origin -- a Worker, not a window: there is no target origin
        worker.postMessage({ type: 'version', jobId });
      }),
  };
}

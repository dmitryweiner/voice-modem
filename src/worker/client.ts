import { isWorkerOut, type CoreEvent, type WorkerIn, type WorkerOut } from './protocol';

/** What the UI can ask the modem core for. */
export interface CoreClient {
  version(): Promise<string>;
  /**
   * Connects the modem to an audio device: its sample rate and the page's end
   * of the channel to the AudioWorklet (the port is handed over to the worker).
   */
  start(sampleRate: number, port: MessagePort): Promise<{ maxTextBytes: number }>;
  /** Transmits a text; resolves when the sound is queued, with its length. */
  sendText(text: string): Promise<{ seconds: number }>;
  stop(): Promise<void>;
  /** Calls the listener for every event of the modem; returns the unsubscribe function. */
  onEvent(listener: (event: CoreEvent) => void): () => void;
}

/** The part of `Worker` the client uses; tests replace it. */
export interface WorkerPort {
  postMessage(message: WorkerIn, transfer: Transferable[]): void;
  addEventListener(type: 'message', listener: (event: { data: unknown }) => void): void;
  addEventListener(type: 'error', listener: () => void): void;
}

type Request = WorkerIn extends infer In ? (In extends { jobId: number } ? Omit<In, 'jobId'> : never) : never;

interface Pending {
  resolve: (reply: WorkerOut) => void;
  reject: (error: Error) => void;
}

const unexpected = (reply: WorkerOut) => new Error(`unexpected reply of the modem worker: ${reply.type}`);

/** A client of the modem worker. The worker is created on the first request. */
export function coreClient(createWorker: () => WorkerPort): CoreClient {
  let worker: WorkerPort | null = null;
  let nextJobId = 1;
  const pending = new Map<number, Pending>();
  const listeners = new Set<(event: CoreEvent) => void>();

  const start = () => {
    const created = createWorker();
    created.addEventListener('message', ({ data }) => {
      if (!isWorkerOut(data)) return;
      if (data.type === 'event') {
        for (const listener of listeners) listener(data.event);
        return;
      }
      const job = pending.get(data.jobId);
      if (!job) return;
      pending.delete(data.jobId);
      if (data.type === 'error') job.reject(new Error(data.message));
      else job.resolve(data);
    });
    // The worker script itself failed (not loaded, threw at start): nothing will be answered.
    created.addEventListener('error', () => {
      for (const job of pending.values()) job.reject(new Error('the modem worker failed'));
      pending.clear();
    });
    return created;
  };

  const request = (message: Request, transfer: Transferable[] = []) =>
    new Promise<WorkerOut>((resolve, reject) => {
      worker ??= start();
      const jobId = nextJobId++;
      pending.set(jobId, { resolve, reject });
      // oxlint-disable-next-line unicorn/require-post-message-target-origin -- a Worker, not a window: there is no target origin
      worker.postMessage({ ...message, jobId }, transfer);
    });

  return {
    async version() {
      const reply = await request({ type: 'version' });
      if (reply.type !== 'version') throw unexpected(reply);
      return reply.version;
    },
    async start(sampleRate, port) {
      const reply = await request({ type: 'start', sampleRate, port }, [port]);
      if (reply.type !== 'started') throw unexpected(reply);
      return { maxTextBytes: reply.maxTextBytes };
    },
    async sendText(text) {
      const reply = await request({ type: 'sendText', text });
      if (reply.type !== 'sent') throw unexpected(reply);
      return { seconds: reply.seconds };
    },
    async stop() {
      const reply = await request({ type: 'stop' });
      if (reply.type !== 'stopped') throw unexpected(reply);
    },
    onEvent(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

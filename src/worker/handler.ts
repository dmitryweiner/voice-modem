import type { WorkerIn, WorkerOut } from './protocol';

/** The part of the WASM core the worker uses; tests replace it. */
export interface Core {
  version(): string;
}

/** Answers one request. A failure of the core becomes an `error` reply, never an exception. */
export async function handle(message: WorkerIn, core: Promise<Core>): Promise<WorkerOut> {
  const { jobId } = message;
  try {
    return { type: 'version', jobId, version: (await core).version() };
  } catch (error) {
    return { type: 'error', jobId, message: error instanceof Error ? error.message : String(error) };
  }
}

// Messages between the page and the modem worker. Every request carries a
// jobId and gets exactly one reply with the same jobId.

export type WorkerIn = { type: 'version'; jobId: number };

export type WorkerOut = { type: 'version'; jobId: number; version: string } | { type: 'error'; jobId: number; message: string };

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null;

export function isWorkerIn(value: unknown): value is WorkerIn {
  return isRecord(value) && typeof value.jobId === 'number' && value.type === 'version';
}

export function isWorkerOut(value: unknown): value is WorkerOut {
  if (!isRecord(value) || typeof value.jobId !== 'number') return false;
  if (value.type === 'version') return typeof value.version === 'string';
  return value.type === 'error' && typeof value.message === 'string';
}

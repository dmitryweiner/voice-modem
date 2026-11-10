// Messages between the page and the modem worker. Every request carries a
// jobId and gets exactly one reply with the same jobId; events come unasked.
//
// Sound does not pass through the page: `start` hands the worker one end of
// a MessageChannel whose other end is in the AudioWorklet (see `AudioLink`).

/** Something the modem tells the application. */
export type CoreEvent =
  | { kind: 'message'; text: string; snrDb: number; freqOffsetHz: number }
  /** A frame was heard but could not be read. */
  | { kind: 'frameLost'; snrDb: number; freqOffsetHz: number }
  /** The last transmitted sample has left for the speaker. */
  | { kind: 'txEnd' };

export type WorkerIn =
  | { type: 'version'; jobId: number }
  | { type: 'start'; jobId: number; sampleRate: number; port: MessagePort }
  | { type: 'sendText'; jobId: number; text: string }
  | { type: 'stop'; jobId: number };

export type WorkerOut =
  | { type: 'version'; jobId: number; version: string }
  | { type: 'started'; jobId: number; maxTextBytes: number }
  /** The text is on its way to the speaker and takes so many seconds. */
  | { type: 'sent'; jobId: number; seconds: number }
  | { type: 'stopped'; jobId: number }
  | { type: 'error'; jobId: number; message: string }
  | { type: 'event'; event: CoreEvent };

/**
 * The channel between the worker and the AudioWorklet. The worklet sends
 * blocks of microphone samples and the string 'idle' when it has played
 * everything it was given; the worker sends blocks of samples to play.
 */
export const AUDIO_IDLE = 'idle';

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null;
const isNumber = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value);

export function isWorkerIn(value: unknown): value is WorkerIn {
  if (!isRecord(value) || !isNumber(value.jobId)) return false;
  switch (value.type) {
    case 'version':
    case 'stop':
      return true;
    case 'start':
      return isNumber(value.sampleRate) && value.port instanceof MessagePort;
    case 'sendText':
      return typeof value.text === 'string';
    default:
      return false;
  }
}

function isCoreEvent(value: unknown): value is CoreEvent {
  if (!isRecord(value)) return false;
  if (value.kind === 'txEnd') return true;
  if (!isNumber(value.snrDb) || !isNumber(value.freqOffsetHz)) return false;
  return value.kind === 'frameLost' || (value.kind === 'message' && typeof value.text === 'string');
}

export function isWorkerOut(value: unknown): value is WorkerOut {
  if (!isRecord(value)) return false;
  if (value.type === 'event') return isCoreEvent(value.event);
  if (!isNumber(value.jobId)) return false;
  switch (value.type) {
    case 'version':
      return typeof value.version === 'string';
    case 'started':
      return isNumber(value.maxTextBytes);
    case 'sent':
      return isNumber(value.seconds);
    case 'stopped':
      return true;
    case 'error':
      return typeof value.message === 'string';
    default:
      return false;
  }
}

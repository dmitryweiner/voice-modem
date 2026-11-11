// Messages between the page and the modem worker. Every request carries a
// jobId and gets exactly one reply with the same jobId; events come unasked.
//
// Sound does not pass through the page: `start` hands the worker one end of
// a MessageChannel whose other end is in the AudioWorklet (see `AUDIO_IDLE`).

/** Something the modem tells the application. */
export type CoreEvent =
  | { kind: 'message'; text: string; station: number; snrDb: number; freqOffsetHz: number }
  /** A station greeted with its name (which may be empty). */
  | { kind: 'hello'; name: string; station: number }
  /** The message with this id was acknowledged / given up on / sent without asking for an answer. */
  | { kind: 'delivered' | 'failed' | 'sent'; id: number }
  /** No acknowledgement: the message goes out again, for the `attempt`-th time. */
  | { kind: 'retry'; id: number; attempt: number }
  /** Something was transmitted nearby but could not be read. */
  | { kind: 'frameLost'; snrDb: number; freqOffsetHz: number }
  /** Data frames go out in this mode from now on (0 is the slowest), at so many bits per second. */
  | { kind: 'txMode'; mode: number; bitRate: number }
  /** Push-to-talk: the modem waits for the `ptt` request. */
  | { kind: 'txRequest' }
  /** Sound of this length has gone to the speaker. */
  | { kind: 'txStart'; seconds: number }
  /** The last sample has been played. */
  | { kind: 'txEnd' };

export interface StationOptions {
  /** The name or callsign sent in a greeting. */
  name: string;
  /** Every transmission waits for the `ptt` request. */
  manualPtt: boolean;
  /** The number of the mode of every data frame (0 is the slowest), or null to adapt to the channel. */
  mode: number | null;
}

/** The modes a station can be fixed to, slowest first, with their speed in bit/s. */
export const MODE_BIT_RATES = [62.5, 125, 250, 500] as const;

export type WorkerIn =
  | { type: 'version'; jobId: number }
  | ({ type: 'start'; jobId: number; sampleRate: number; port: MessagePort } & StationOptions)
  | { type: 'sendText'; jobId: number; text: string }
  | { type: 'hello'; jobId: number }
  | { type: 'ptt'; jobId: number }
  | { type: 'stop'; jobId: number };

export type WorkerOut =
  | { type: 'version'; jobId: number; version: string }
  | { type: 'started'; jobId: number; maxTextBytes: number }
  /** The text waits for its turn; events will tell its fate under this id. */
  | { type: 'queued'; jobId: number; id: number }
  /** A request that has nothing to return was carried out. */
  | { type: 'done'; jobId: number }
  | { type: 'error'; jobId: number; message: string }
  | { type: 'event'; event: CoreEvent };

/**
 * The channel between the worker and the AudioWorklet carries blocks of
 * samples both ways (microphone to the worker, speaker to the worklet) and
 * this string from the worklet when it has played everything it was given.
 */
export const AUDIO_IDLE = 'idle';

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null;
const isNumber = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value);

export function isWorkerIn(value: unknown): value is WorkerIn {
  if (!isRecord(value) || !isNumber(value.jobId)) return false;
  switch (value.type) {
    case 'version':
    case 'hello':
    case 'ptt':
    case 'stop':
      return true;
    case 'start':
      return (
        isNumber(value.sampleRate) &&
        value.port instanceof MessagePort &&
        typeof value.name === 'string' &&
        typeof value.manualPtt === 'boolean' &&
        (value.mode === null || isNumber(value.mode))
      );
    case 'sendText':
      return typeof value.text === 'string';
    default:
      return false;
  }
}

function isCoreEvent(value: unknown): value is CoreEvent {
  if (!isRecord(value)) return false;
  const heard = isNumber(value.snrDb) && isNumber(value.freqOffsetHz);
  switch (value.kind) {
    case 'message':
      return heard && typeof value.text === 'string' && isNumber(value.station);
    case 'hello':
      return typeof value.name === 'string' && isNumber(value.station);
    case 'delivered':
    case 'failed':
    case 'sent':
      return isNumber(value.id);
    case 'retry':
      return isNumber(value.id) && isNumber(value.attempt);
    case 'frameLost':
      return heard;
    case 'txMode':
      return isNumber(value.mode) && isNumber(value.bitRate);
    case 'txStart':
      return isNumber(value.seconds);
    case 'txRequest':
    case 'txEnd':
      return true;
    default:
      return false;
  }
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
    case 'queued':
      return isNumber(value.id);
    case 'done':
      return true;
    case 'error':
      return typeof value.message === 'string';
    default:
      return false;
  }
}

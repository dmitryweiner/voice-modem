import { AUDIO_IDLE, type CoreEvent, type StationOptions, type WorkerIn, type WorkerOut } from './protocol';

/** A modem of the WASM core, as the worker uses it; tests replace it. */
export interface CoreModem {
  /** Queues a text and returns its id; throws when the text is empty or too long. */
  sendText(text: string): number;
  hello(): void;
  pttPressed(): void;
  /** Takes back a text that is not delivered yet; false when it is no longer waiting. */
  cancel(id: number): boolean;
  pushRx(samples: Float32Array): void;
  pullTx(length: number): Float32Array;
  txPending(): number;
  pollEvent(): CoreModemEvent | undefined;
  free(): void;
}

export interface CoreModemEvent {
  kind: string;
  text: string;
  id: number;
  attempt: number;
  mode: number;
  bitRate: number;
  station: number;
  snrDb: number;
  freqOffsetHz: number;
  free?: () => void;
}

export interface ModemOptions extends StationOptions {
  sampleRate: number;
  /** The address of the station on the channel, 16 bits. */
  station: number;
  /** Seed of the random waits of the link layer, 32 bits. */
  seed: number;
}

/** The part of the WASM core the worker uses; tests replace it. */
export interface Core {
  version(): string;
  maxTextBytes(): number;
  /** Throws when the sample rate is not supported. */
  createModem(options: ModemOptions): CoreModem;
}

function toEvent(event: CoreModemEvent): CoreEvent | null {
  const { kind, text, id, attempt, mode, bitRate, station, snrDb, freqOffsetHz } = event;
  event.free?.();
  switch (kind) {
    case 'message':
      return { kind, text, station, snrDb, freqOffsetHz };
    case 'hello':
      return { kind, name: text, station };
    case 'delivered':
    case 'failed':
    case 'sent':
      return { kind, id };
    case 'retry':
      return { kind, id, attempt };
    case 'frameLost':
      return { kind, snrDb, freqOffsetHz };
    case 'txMode':
      return { kind, mode, bitRate };
    case 'txRequest':
      return { kind };
    default:
      return null;
  }
}

/**
 * The state of the worker: one modem connected to one audio channel.
 * `random` gives a 32-bit random number (the station address, the seed).
 * Returns the function that answers a request; a failure of the core becomes
 * an `error` reply, never an exception.
 */
export function createSession(
  core: Promise<Core>,
  post: (out: WorkerOut) => void,
  random: () => number,
): (message: WorkerIn) => Promise<void> {
  let modem: CoreModem | null = null;
  /** The worker's end of the channel to the AudioWorklet. */
  let audio: MessagePort | null = null;
  let sampleRate = 0;

  const stop = () => {
    if (audio) {
      audio.onmessage = null;
      audio.close();
    }
    modem?.free();
    modem = null;
    audio = null;
  };

  const running = () => {
    if (!modem || !audio) throw new Error('the modem is not started');
    return { modem, audio };
  };

  const onAudio = (data: unknown) => {
    if (!modem || !audio) return;
    if (data === AUDIO_IDLE) {
      post({ type: 'event', event: { kind: 'txEnd' } });
      return;
    }
    if (!(data instanceof Float32Array)) return;
    // The microphone is the modem's clock: every block goes in, silence too.
    modem.pushRx(data);
    for (let event = modem.pollEvent(); event; event = modem.pollEvent()) {
      const out = toEvent(event);
      if (out) post({ type: 'event', event: out });
    }
    // The modem takes its transmission to start now: all of it goes to the speaker at once.
    const pending = modem.txPending();
    if (pending > 0) {
      const samples = modem.pullTx(pending);
      audio.postMessage(samples, [samples.buffer]);
      post({ type: 'event', event: { kind: 'txStart', seconds: samples.length / sampleRate } });
    }
  };

  const answer = async (message: WorkerIn): Promise<WorkerOut> => {
    const { jobId } = message;
    const loaded = await core;
    switch (message.type) {
      case 'version':
        return { type: 'version', jobId, version: loaded.version() };
      case 'start': {
        stop();
        const { name, manualPtt, mode } = message;
        // An address of zero is as good as any other; the seed must not be zero for some generators.
        modem = loaded.createModem({
          sampleRate: message.sampleRate,
          name,
          manualPtt,
          mode,
          station: random() & 0xffff,
          seed: random() | 1,
        });
        sampleRate = message.sampleRate;
        audio = message.port;
        audio.onmessage = (event: MessageEvent<unknown>) => onAudio(event.data);
        return { type: 'started', jobId, maxTextBytes: loaded.maxTextBytes() };
      }
      case 'sendText':
        return { type: 'queued', jobId, id: running().modem.sendText(message.text) };
      case 'hello':
        running().modem.hello();
        break;
      case 'ptt':
        running().modem.pttPressed();
        break;
      case 'cancel':
        // Whether it was still waiting does not matter: if not, an event has told its fate.
        running().modem.cancel(message.id);
        break;
      case 'stop':
        stop();
        break;
    }
    return { type: 'done', jobId };
  };

  return async (message) => {
    try {
      post(await answer(message));
    } catch (error) {
      post({ type: 'error', jobId: message.jobId, message: error instanceof Error ? error.message : String(error) });
    }
  };
}

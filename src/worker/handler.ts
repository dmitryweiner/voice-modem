import { AUDIO_IDLE, type CoreEvent, type WorkerIn, type WorkerOut } from './protocol';

/** A modem of the WASM core, as the worker uses it; tests replace it. */
export interface CoreModem {
  /** Throws when the text does not fit into one frame. */
  sendText(text: string): void;
  pushRx(samples: Float32Array): void;
  pullTx(length: number): Float32Array;
  txPending(): number;
  pollEvent(): CoreModemEvent | undefined;
  free(): void;
}

export interface CoreModemEvent {
  kind: string;
  text: string;
  snrDb: number;
  freqOffsetHz: number;
  free?: () => void;
}

/** The part of the WASM core the worker uses; tests replace it. */
export interface Core {
  version(): string;
  maxTextBytes(): number;
  /** Throws when the sample rate is not supported. */
  createModem(sampleRate: number): CoreModem;
}

/**
 * After a transmission the microphone still hears it: the output buffer of
 * the device, then the echo of the room. Received sound is dropped for this
 * long after the worklet reports it has nothing more to play.
 */
const DEAF_AFTER_TX_SECONDS = 0.5;

function toEvent(event: CoreModemEvent): CoreEvent | null {
  const { kind, text, snrDb, freqOffsetHz } = event;
  event.free?.();
  if (kind === 'message') return { kind, text, snrDb, freqOffsetHz };
  return kind === 'frameLost' ? { kind, snrDb, freqOffsetHz } : null;
}

/**
 * The state of the worker: one modem connected to one audio channel.
 * Returns the function that answers a request; a failure of the core becomes
 * an `error` reply, never an exception.
 */
export function createSession(core: Promise<Core>, post: (out: WorkerOut) => void): (message: WorkerIn) => Promise<void> {
  let modem: CoreModem | null = null;
  /** The worker's end of the channel to the AudioWorklet. */
  let audio: MessagePort | null = null;
  let sampleRate = 0;
  let transmitting = false;
  /** Received samples still to drop after a transmission. */
  let deaf = 0;

  const stop = () => {
    if (audio) {
      audio.onmessage = null;
      audio.close();
    }
    modem?.free();
    modem = null;
    audio = null;
    transmitting = false;
    deaf = 0;
  };

  const onAudio = (data: unknown) => {
    if (!modem) return;
    if (data === AUDIO_IDLE) {
      if (!transmitting) return;
      transmitting = false;
      deaf = Math.round(DEAF_AFTER_TX_SECONDS * sampleRate);
      post({ type: 'event', event: { kind: 'txEnd' } });
      return;
    }
    if (!(data instanceof Float32Array) || transmitting) return;
    if (deaf > 0) {
      deaf -= data.length;
      return;
    }
    modem.pushRx(data);
    for (let event = modem.pollEvent(); event; event = modem.pollEvent()) {
      const out = toEvent(event);
      if (out) post({ type: 'event', event: out });
    }
  };

  const answer = async (message: WorkerIn): Promise<WorkerOut> => {
    const { jobId } = message;
    const loaded = await core;
    if (message.type === 'version') return { type: 'version', jobId, version: loaded.version() };
    if (message.type === 'start') {
      stop();
      modem = loaded.createModem(message.sampleRate);
      sampleRate = message.sampleRate;
      audio = message.port;
      audio.onmessage = (event: MessageEvent<unknown>) => onAudio(event.data);
      return { type: 'started', jobId, maxTextBytes: loaded.maxTextBytes() };
    }
    if (message.type === 'sendText') {
      if (!modem || !audio) throw new Error('the modem is not started');
      modem.sendText(message.text);
      const samples = modem.pullTx(modem.txPending());
      transmitting = true;
      audio.postMessage(samples, [samples.buffer]);
      return { type: 'sent', jobId, seconds: samples.length / sampleRate };
    }
    stop();
    return { type: 'stopped', jobId };
  };

  return async (message) => {
    try {
      post(await answer(message));
    } catch (error) {
      post({ type: 'error', jobId: message.jobId, message: error instanceof Error ? error.message : String(error) });
    }
  };
}

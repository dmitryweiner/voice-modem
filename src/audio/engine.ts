// The audio device of the browser: microphone in, speaker out, both wired to
// the modem worker through the AudioWorklet. Browser glue: its pure parts
// are in meter.ts and the worklet, the whole is checked by the smoke.
import { peak, spectrumBars, type Meter } from './meter';

export type AudioErrorKind = 'denied' | 'noDevice' | 'unsupported' | 'failed';

/** A running audio device. */
export interface AudioSession {
  /** Sample rate of the device, Hz. */
  sampleRate: number;
  /** The page's end of the channel to the AudioWorklet; goes to the modem worker. */
  port: MessagePort;
  /** The input level and spectrum right now. */
  meter(): Meter;
  stop(): void;
}

export type StartAudio = () => Promise<AudioSession>;

/** Why the audio device did not start, from what the browser threw. */
export function audioErrorKind(error: unknown): AudioErrorKind {
  if (error instanceof Error && error.message === 'unsupported') return 'unsupported';
  const name = error instanceof Error ? error.name : '';
  if (name === 'NotAllowedError' || name === 'SecurityError') return 'denied';
  if (name === 'NotFoundError' || name === 'OverconstrainedError') return 'noDevice';
  return 'failed';
}

/** Opens the microphone and the speaker. Call from a click: browsers start audio only on a user's action. */
export const startAudio: StartAudio = async () => {
  if (!('mediaDevices' in navigator) || !('AudioWorkletNode' in globalThis)) throw new Error('unsupported');
  // Before the first await: Safari starts a context only inside the click itself.
  const context = new AudioContext({ latencyHint: 'interactive' });
  let stream: MediaStream | null = null;
  const release = () => {
    for (const track of stream?.getTracks() ?? []) track.stop();
    void context.close();
  };
  try {
    // Everything that "improves" speech destroys a modem signal.
    stream = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: false, noiseSuppression: false, autoGainControl: false, channelCount: 1 },
    });
    await context.audioWorklet.addModule(new URL('./modem-io.worklet.js', import.meta.url));
    await context.resume();

    const source = context.createMediaStreamSource(stream);
    const node = new AudioWorkletNode(context, 'modem-io', {
      numberOfInputs: 1,
      numberOfOutputs: 1,
      outputChannelCount: [1],
      channelCount: 1,
      channelCountMode: 'explicit',
    });
    const analyser = context.createAnalyser();
    analyser.fftSize = 2048;
    analyser.smoothingTimeConstant = 0.5;
    source.connect(node);
    source.connect(analyser);
    node.connect(context.destination);

    const channel = new MessageChannel();
    // oxlint-disable-next-line unicorn/require-post-message-target-origin -- a MessagePort, not a window
    node.port.postMessage({ port: channel.port1 }, [channel.port1]);

    const wave = new Float32Array(analyser.fftSize);
    const bins = new Uint8Array(analyser.frequencyBinCount);
    const binHz = context.sampleRate / analyser.fftSize;
    return {
      sampleRate: context.sampleRate,
      port: channel.port2,
      meter() {
        analyser.getFloatTimeDomainData(wave);
        analyser.getByteFrequencyData(bins);
        return { peak: peak(wave), bars: spectrumBars(bins, binHz) };
      },
      stop: release,
    };
  } catch (error) {
    release();
    throw error;
  }
};

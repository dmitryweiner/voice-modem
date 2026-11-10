import { beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { audioErrorKind } from './engine';
import { BAR_COUNT, peak, SILENT, spectrumBars } from './meter';

describe('meter', () => {
  it('takes the largest sample as the peak, at most full scale', () => {
    expect(peak(new Float32Array([0.1, -0.7, 0.3]))).toBeCloseTo(0.7);
    expect(peak(new Float32Array([0.1, -1.5]))).toBe(1);
    expect(peak(new Float32Array(0))).toBe(0);
  });

  it('groups frequency bins into bars up to 4 kHz, the loudest bin of each', () => {
    // bins 23.4375 Hz apart, as with a 2048-point analyser at 48 kHz
    const bins = new Uint8Array(1024);
    bins[0] = 51; // 0 Hz → bar 0
    bins[3] = 255; // 70 Hz → bar 0
    bins[43] = 102; // 1008 Hz → bar 12 (bars are 83.3 Hz wide)
    bins[170] = 204; // 3984 Hz → the last bar
    bins[171] = 255; // 4008 Hz: beyond the display
    const bars = spectrumBars(bins, 23.4375);
    expect(bars).toHaveLength(BAR_COUNT);
    expect(bars[0]).toBe(1);
    expect(bars[12]).toBeCloseTo(0.4);
    expect(bars[47]).toBeCloseTo(0.8);
    expect(bars.filter((level) => level > 0)).toHaveLength(3);
  });

  it('has a silent meter of the same shape', () => {
    expect(SILENT).toEqual({ peak: 0, bars: spectrumBars(new Uint8Array(1024), 23.4375) });
  });
});

const named = (name: string) => Object.assign(new Error('x'), { name });

describe('audioErrorKind', () => {
  it('tells why the audio device did not start', () => {
    expect(audioErrorKind(named('NotAllowedError'))).toBe('denied');
    expect(audioErrorKind(named('SecurityError'))).toBe('denied');
    expect(audioErrorKind(named('NotFoundError'))).toBe('noDevice');
    expect(audioErrorKind(named('OverconstrainedError'))).toBe('noDevice');
    expect(audioErrorKind(new Error('unsupported'))).toBe('unsupported');
    expect(audioErrorKind(named('AbortError'))).toBe('failed');
    expect(audioErrorKind('boom')).toBe('failed');
  });
});

describe('the AudioWorklet processor', () => {
  interface Processor {
    port: { onmessage: ((event: { data: unknown }) => void) | null };
    process(inputs: Float32Array[][], outputs: Float32Array[][]): boolean;
  }
  /** The worklet's end of the channel to the worker. */
  class Link {
    sent: unknown[] = [];
    onmessage: ((event: { data: unknown }) => void) | null = null;
    postMessage(data: unknown) {
      this.sent.push(data);
    }
  }

  let ModemIo: new () => Processor;
  let registeredAs = '';
  let processor: Processor;
  let link: Link;

  /** One render quantum: 128 samples in, 128 out. */
  function render(input: Float32Array | null = new Float32Array(128)) {
    const output = new Float32Array(128);
    expect(processor.process(input ? [[input]] : [[]], [[output]])).toBe(true);
    return output;
  }

  beforeAll(async () => {
    // What an AudioWorkletGlobalScope provides.
    Object.assign(globalThis, {
      AudioWorkletProcessor: class {
        port = { onmessage: null };
      },
      registerProcessor(name: string, processorClass: new () => Processor) {
        registeredAs = name;
        ModemIo = processorClass;
      },
    });
    await import('./modem-io.worklet.js');
  });

  beforeEach(() => {
    processor = new ModemIo();
    link = new Link();
    processor.port.onmessage?.({ data: { port: link } });
  });

  it('registers under the name the engine asks for', () => {
    expect(registeredAs).toBe('modem-io');
  });

  it('sends the microphone to the worker in blocks of 2048 samples', () => {
    for (let i = 0; i < 15; i++) render(new Float32Array(128).fill(i));
    expect(link.sent).toEqual([]);
    render(new Float32Array(128).fill(15));
    expect(link.sent).toHaveLength(1);
    const block = link.sent[0] as Float32Array;
    expect(block).toHaveLength(2048);
    expect([block[0], block[127], block[128], block[2047]]).toEqual([0, 0, 1, 15]);
    // the next block starts clean
    for (let i = 0; i < 16; i++) render(new Float32Array(128).fill(-1));
    expect((link.sent[1] as Float32Array).every((sample) => sample === -1)).toBe(true);
  });

  it('captures nothing before it has the channel or while there is no input', () => {
    const lonely = new ModemIo();
    expect(lonely.process([[new Float32Array(128)]], [[new Float32Array(128)]])).toBe(true);
    for (let i = 0; i < 32; i++) render(null);
    expect(link.sent).toEqual([]);
  });

  it('plays silence when there is nothing to play', () => {
    expect(render().every((sample) => sample === 0)).toBe(true);
    expect(link.sent).toEqual([]);
  });

  it('plays what the worker sends, across quanta and blocks, then says idle once', () => {
    link.onmessage?.({ data: new Float32Array(200).fill(0.5) });
    link.onmessage?.({ data: new Float32Array(100).fill(-0.5) });

    expect(render().every((sample) => sample === 0.5)).toBe(true);
    const second = render();
    expect([second[0], second[71], second[72], second[127]]).toEqual([0.5, 0.5, -0.5, -0.5]);
    expect(link.sent).toEqual([]);

    const third = render();
    expect([third[0], third[43], third[44], third[127]]).toEqual([-0.5, -0.5, 0, 0]);
    expect(link.sent).toEqual(['idle']);

    render();
    expect(link.sent).toEqual(['idle']);
  });

  it('ignores what is not samples', () => {
    link.onmessage?.({ data: 'hello' });
    link.onmessage?.({ data: new Float32Array(0) });
    render();
    expect(link.sent).toEqual([]);
  });
});

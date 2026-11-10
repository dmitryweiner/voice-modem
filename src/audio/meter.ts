// What the input meter shows, computed from the data of an AnalyserNode.

export interface Meter {
  /** The largest sample of the last moment, 0..1 (1 is full scale: clipping). */
  peak: number;
  /** Levels of the spectrum bars, 0..1 each, low frequencies first. */
  bars: number[];
}

/** Bars of the spectrum display. */
export const BAR_COUNT = 48;
/** The display ends here: the modem lives below 3.4 kHz. */
export const SPECTRUM_MAX_HZ = 4000;
/** At and above this peak the input is taken to be clipping. */
export const CLIP_LEVEL = 0.98;

export const SILENT: Meter = { peak: 0, bars: Array.from({ length: BAR_COUNT }, () => 0) };

export function peak(samples: Float32Array): number {
  let max = 0;
  for (const sample of samples) max = Math.max(max, Math.abs(sample));
  return Math.min(max, 1);
}

/**
 * Groups the bins of `getByteFrequencyData` (0..255 each, `binHz` apart) into
 * `count` bars from 0 to `maxHz`; a bar shows its loudest bin.
 */
export function spectrumBars(bins: Uint8Array, binHz: number, count = BAR_COUNT, maxHz = SPECTRUM_MAX_HZ): number[] {
  const bars = Array.from({ length: count }, () => 0);
  const barHz = maxHz / count;
  for (const [index, level] of bins.entries()) {
    const bar = Math.floor((index * binHz) / barHz);
    if (bar >= count) break;
    bars[bar] = Math.max(bars[bar] ?? 0, level / 255);
  }
  return bars;
}

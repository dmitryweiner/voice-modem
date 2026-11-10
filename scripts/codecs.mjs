// Runs the modem through real speech codecs with ffmpeg: encodes a few
// messages with modem-cli, passes the WAV through each codec and back, and
// decodes it. Prints how many messages survived and the signal-to-noise
// ratio the receiver measured. Needs ffmpeg and a Rust toolchain; codecs
// this ffmpeg build lacks are reported as "not available".
//
//   node scripts/codecs.mjs          # the table
//   node scripts/codecs.mjs --keep   # also keeps the WAV files and prints where
//
// The results are recorded in docs/measurements.md.

import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { parseArgs } from './lib/harness.mjs';

const TEXTS = [
  'Hi',
  'The quick brown fox jumps over the lazy dog.',
  'Привет! Как слышно? Приём.',
  '0123456789 0123456789 0123456789 0123456789 0123456789 0123456789 0123456789 0123456789',
  'x'.repeat(255),
];

/** A codec: its name, the file it writes, the ffmpeg options to write it and to read it back. */
const codec = (name, file, encode, decode = []) => ({ name, file, encode, decode });
const CODECS = [
  codec('none (16-bit WAV)', 'x.wav', []),
  codec('G.711 µ-law, 64 kbit/s', 'x.wav', ['-ar', '8000', '-c:a', 'pcm_mulaw']),
  codec('G.726, 32 kbit/s', 'x.wav', ['-ar', '8000', '-c:a', 'g726', '-b:a', '32k']),
  codec('G.726, 16 kbit/s', 'x.wav', ['-ar', '8000', '-c:a', 'g726', '-b:a', '16k']),
  codec('iLBC, 15.2 kbit/s', 'x.lbc', ['-ar', '8000', '-c:a', 'ilbc_at', '-b:a', '15.2k']),
  codec('G.723.1, 6.3 kbit/s', 'x.tco', ['-ar', '8000', '-c:a', 'g723_1', '-b:a', '6.3k'], ['-f', 'g723_1']),
  codec('AMR-NB, 12.2 kbit/s', 'x.amr', ['-ar', '8000', '-c:a', 'libopencore_amrnb', '-b:a', '12.2k']),
  codec('AMR-NB, 4.75 kbit/s', 'x.amr', ['-ar', '8000', '-c:a', 'libopencore_amrnb', '-b:a', '4.75k']),
  ...[24, 16, 12, 8, 6].map((k) => codec(`Opus voice, ${k} kbit/s`, 'x.ogg', ['-c:a', 'libopus', '-application', 'voip', '-b:a', `${k}k`])),
];

const flags = parseArgs(process.argv.slice(2));
const dir = mkdtempSync(join(tmpdir(), 'voice-modem-codecs-'));
const quiet = { stdio: ['ignore', 'pipe', 'pipe'], encoding: 'utf8' };
const ffmpeg = (...args) => execFileSync('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', ...args], quiet);

execFileSync('cargo', ['build', '--release', '--quiet', '-p', 'modem-cli'], { stdio: 'inherit' });
const cli = (...args) => execFileSync('target/release/modem-cli', args, quiet);

const source = join(dir, 'source.wav');
console.log(
  cli('encode', ...TEXTS, '-o', source)
    .trim()
    .replace(dir, '…'),
);
console.log(`\n| ${'Codec'.padEnd(24)} | Messages | SNR, dB (min … max) |\n| ${'-'.repeat(24)} | -------- | ------------------- |`);

for (const [index, { name, file, encode, decode }] of CODECS.entries()) {
  const coded = join(dir, `${index}-${file}`);
  const heard = join(dir, `${index}.wav`);
  let result;
  try {
    ffmpeg('-i', source, '-ac', '1', ...encode, coded);
    ffmpeg(...decode, '-i', coded, '-ac', '1', '-ar', '16000', '-c:a', 'pcm_s16le', heard);
  } catch {
    result = `not available in this ffmpeg build |`;
  }
  if (!result) {
    let report = '';
    try {
      report = cli('decode', heard);
    } catch (error) {
      // exit code 1: no messages; the report of damaged frames is in stdout
      report = String(error.stdout);
    }
    const lines = report.split('\n').filter((line) => line.includes(' dB '));
    const received = TEXTS.filter((text) => lines.some((line) => line.endsWith(`Hz  ${text}`))).length;
    const snr = lines.map((line) => Number(/([+-][\d.]+) dB/.exec(line)?.[1]));
    const range = snr.length ? `${Math.min(...snr).toFixed(1)} … ${Math.max(...snr).toFixed(1)}` : '—';
    result = `${`${received}/${TEXTS.length}`.padEnd(8)} | ${range.padEnd(19)} |`;
  }
  console.log(`| ${name.padEnd(24)} | ${result}`);
}

if (flags.has('keep')) console.log(`\nfiles: ${dir}`);
else rmSync(dir, { recursive: true });

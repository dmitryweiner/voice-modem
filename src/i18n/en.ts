import { plural, signed } from './plural';

// English is the default locale and defines the shape of every message set.
export const en = {
  title: 'Voice Modem',
  description: 'An acoustic modem in the browser: text and small files over any voice channel.',
  intro:
    'Text as sound. Open this page on two devices next to each other, turn on the microphone on both and send a text from one of them.',
  languageLabel: 'Language',
  source: 'Source code',
  coreLoading: 'Loading the modem core…',
  coreFailed: 'The modem core did not load.',

  micOn: 'Turn the microphone on',
  micOff: 'Turn off',
  micStarting: 'Starting…',
  micDenied: 'The browser did not allow the microphone. Allow it for this page and try again.',
  micNoDevice: 'No microphone found.',
  micUnsupported: 'This browser cannot handle sound the way the modem needs.',
  micFailed: 'The audio device did not start.',
  listening: 'Listening',
  transmitting: 'Transmitting…',
  inputLevel: 'Input level',
  clipping: 'Too loud: turn the volume down or move the devices apart.',
  spectrum: 'Spectrum of the input, 0 to 4 kHz',
  messages: 'Messages',
  noMessages: 'Nothing yet. Texts sent and received appear here.',
  messageLabel: 'Text to send',
  send: 'Send',
  received: 'Received',
  sending: 'Sending…',
  sent: 'Sent',
  frameLost: 'A message was heard but could not be read.',
  sendFailed: 'The text was not sent.',

  coreReady: (version: string) => `Modem core ${version} is ready.`,
  tooLong: (bytes: number) => `Too long by ${bytes} ${plural('en', bytes, { one: 'byte', other: 'bytes' })}.`,
  snr: (db: number) => `signal ${signed(db)} dB to noise`,
  seconds: (seconds: number) => `${seconds.toFixed(1)} s`,
};

export type Messages = typeof en;

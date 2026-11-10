// @vitest-environment jsdom
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App } from './App';
import type { AudioSession, StartAudio } from './audio/engine';
import { SILENT, type Meter } from './audio/meter';
import { LOCALE_NAMES, LOCALES } from './i18n';
import { loadSettings, type KeyValueStorage } from './storage';
import type { CoreClient } from './worker/client';
import type { CoreEvent } from './worker/protocol';

function $<T extends Element>(selector: string, type: abstract new () => T): T {
  const el = document.querySelector(selector);
  if (!(el instanceof type)) throw new Error(`missing ${selector}`);
  return el;
}
const text = (selector: string) => $(selector, HTMLElement).textContent;
const exists = (selector: string) => document.querySelector(selector) !== null;

class MemoryStorage implements KeyValueStorage {
  data = new Map<string, string>();
  getItem(key: string) {
    return this.data.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.data.set(key, value);
  }
}

/** A modem core that records what it is asked and says what the test tells it to. */
function fakeCore(overrides: Partial<CoreClient> = {}) {
  const listeners = new Set<(event: CoreEvent) => void>();
  const calls = { started: [] as [number, MessagePort][], sent: [] as string[], stopped: 0 };
  const client: CoreClient = {
    version: () => Promise.resolve('1.2.3'),
    start(sampleRate, port) {
      calls.started.push([sampleRate, port]);
      return Promise.resolve({ maxTextBytes: 10 });
    },
    sendText(message) {
      calls.sent.push(message);
      return Promise.resolve({ seconds: 3.96 });
    },
    stop() {
      calls.stopped++;
      return Promise.resolve();
    },
    onEvent(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    ...overrides,
  };
  const emit = (event: CoreEvent) => act(() => listeners.forEach((listener) => listener(event)));
  return { client, calls, emit, listeners };
}

/** An audio device that never touches the browser. */
function fakeAudio() {
  const state: { meter: Meter; stopped: number } = { meter: SILENT, stopped: 0 };
  const port = {} as MessagePort;
  const session: AudioSession = {
    sampleRate: 48_000,
    port,
    meter: () => state.meter,
    stop() {
      state.stopped++;
    },
  };
  const startAudio = vi.fn<StartAudio>(() => Promise.resolve(session));
  return { startAudio, state, port };
}

interface Setup {
  storage?: KeyValueStorage | null;
  languages?: readonly string[];
  core?: ReturnType<typeof fakeCore>;
  audio?: ReturnType<typeof fakeAudio>;
}

/** Renders the app and lets the core answer. */
async function start({ storage = new MemoryStorage(), languages = ['en-US'], core = fakeCore(), audio = fakeAudio() }: Setup = {}) {
  // oxlint-disable-next-line typescript/require-await -- async `act` flushes the promise of the core
  await act(async () => {
    render(<App storage={storage} languages={languages} core={core.client} startAudio={audio.startAudio} />);
  });
  return { storage, core, audio };
}

/** A click or a submit whose handler awaits something. */
async function press(action: () => void) {
  // oxlint-disable-next-line typescript/require-await -- async `act` flushes the promises of the handler
  await act(async () => action());
}

/** The app with the microphone turned on. */
async function listening(setup: Setup = {}) {
  const started = await start(setup);
  await press(() => $('#mic', HTMLButtonElement).click());
  return started;
}

async function send(message: string) {
  fireEvent.change($('#draft', HTMLInputElement), { target: { value: message } });
  await press(() => fireEvent.submit($('.composer', HTMLFormElement)));
}

const selectLocale = (locale: string) => fireEvent.change($('#locale', HTMLSelectElement), { target: { value: locale } });

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe('language', () => {
  it('lists every locale in the switcher by its own name', async () => {
    await start();
    const options = [...$('#locale', HTMLSelectElement).options].map((o) => [o.value, o.textContent]);
    expect(options).toEqual(LOCALES.map((l) => [l, LOCALE_NAMES[l]]));
  });

  it('follows the browser when nothing is saved', async () => {
    await start({ languages: ['ru-RU', 'en'] });
    expect(document.documentElement.lang).toBe('ru');
    expect(document.title).toBe('Голосовой модем');
    expect(text('h1')).toBe('Голосовой модем');
    expect($('#locale', HTMLSelectElement).getAttribute('aria-label')).toBe('Язык');
  });

  it('falls back to English', async () => {
    await start({ languages: ['de-DE'] });
    expect(document.documentElement.lang).toBe('en');
    expect(document.documentElement.dir).toBe('ltr');
    expect(text('h1')).toBe('Voice Modem');
  });

  it('saves the switcher choice and prefers it over the browser', async () => {
    const { storage } = await start({ languages: ['en-US'] });
    selectLocale('ru');
    expect(document.documentElement.lang).toBe('ru');
    expect(loadSettings(storage).locale).toBe('ru');
    cleanup();
    await start({ storage, languages: ['en-US'] });
    expect(document.documentElement.lang).toBe('ru');
    expect($('#locale', HTMLSelectElement).value).toBe('ru');
  });

  it('works without storage', async () => {
    await start({ storage: null, languages: [] });
    selectLocale('ru');
    expect(text('h1')).toBe('Голосовой модем');
  });
});

const never = () => new Promise<string>(() => {});

describe('modem core', () => {
  it('says it is loading until the core answers, and keeps the microphone off', async () => {
    await start({ core: fakeCore({ version: never }) });
    expect(text('#core')).toBe('Loading the modem core…');
    expect($('#core', HTMLElement).dataset.status).toBe('loading');
    expect($('#mic', HTMLButtonElement).disabled).toBe(true);
  });

  it('shows the version of the loaded core', async () => {
    await start();
    expect(text('#core')).toBe('Modem core 1.2.3 is ready.');
    expect($('#core', HTMLElement).dataset.status).toBe('ready');
    expect($('#mic', HTMLButtonElement).disabled).toBe(false);
  });

  it('says so when the core did not load', async () => {
    await start({ core: fakeCore({ version: () => Promise.reject(new Error('no wasm')) }) });
    expect(text('#core')).toBe('The modem core did not load.');
    expect($('#core', HTMLElement).dataset.status).toBe('failed');
    expect($('#mic', HTMLButtonElement).disabled).toBe(true);
  });

  it('keeps the core state when the language changes', async () => {
    await start();
    selectLocale('ru');
    expect(text('#core')).toBe('Ядро модема 1.2.3 готово.');
  });
});

describe('microphone', () => {
  it('is off at first: no meter, nothing to type into', async () => {
    const { audio } = await start();
    expect(text('#mic')).toBe('Turn the microphone on');
    expect(audio.startAudio).not.toHaveBeenCalled();
    expect(exists('#level') || exists('#draft') || exists('#state')).toBe(false);
  });

  it('turns on: the audio device starts and its port goes to the modem', async () => {
    const { core, audio } = await listening();
    expect(audio.startAudio).toHaveBeenCalledTimes(1);
    expect(core.calls.started).toEqual([[48_000, audio.port]]);
    expect(text('#state')).toBe('Listening');
    expect(text('#mic')).toBe('Turn off');
    expect(exists('#level') && exists('#spectrum') && exists('#draft')).toBe(true);
  });

  it('shows that it is starting while the browser asks for permission', async () => {
    const audio = fakeAudio();
    audio.startAudio.mockReturnValue(new Promise(() => {}));
    await listening({ audio });
    expect(text('#mic')).toBe('Starting…');
    expect($('#mic', HTMLButtonElement).disabled).toBe(true);
  });

  it.each([
    ['NotAllowedError', 'The browser did not allow the microphone. Allow it for this page and try again.'],
    ['NotFoundError', 'No microphone found.'],
    ['AbortError', 'The audio device did not start.'],
  ])('explains a %s and lets the user try again', async (name, message) => {
    const audio = fakeAudio();
    audio.startAudio.mockRejectedValueOnce(Object.assign(new Error('x'), { name }));
    const { core } = await listening({ audio });
    expect(text('#mic-error')).toBe(message);
    expect(core.calls.started).toEqual([]);

    await press(() => $('#mic', HTMLButtonElement).click());
    expect(exists('#mic-error')).toBe(false);
    expect(text('#state')).toBe('Listening');
  });

  it('releases the audio device when the modem does not start', async () => {
    const core = fakeCore({ start: () => Promise.reject(new Error('unsupported sample rate: 48000 Hz')) });
    const { audio } = await listening({ core });
    expect(text('#mic-error')).toBe('The audio device did not start.');
    expect(audio.state.stopped).toBe(1);
  });

  it('turns off: the device and the modem stop', async () => {
    const { core, audio } = await listening();
    await press(() => $('#mic', HTMLButtonElement).click());
    expect(audio.state.stopped).toBe(1);
    expect(core.calls.stopped).toBe(1);
    expect(text('#mic')).toBe('Turn the microphone on');
    expect(exists('#draft')).toBe(false);
  });

  it('stops the device when the page goes away', async () => {
    const { core, audio } = await listening();
    cleanup();
    expect(audio.state.stopped).toBe(1);
    expect(core.calls.stopped).toBe(1);
    expect(core.listeners.size).toBe(0);
  });
});

describe('input meter', () => {
  it('follows the level and the spectrum of the microphone', async () => {
    vi.useFakeTimers();
    const { audio } = await listening();
    expect($('#level', HTMLElement).getAttribute('aria-valuenow')).toBe('0');

    audio.state.meter = { peak: 0.42, bars: SILENT.bars.map((_, index) => (index === 3 ? 0.5 : 0)) };
    act(() => {
      vi.advanceTimersByTime(60);
    });
    expect($('#level', HTMLElement).getAttribute('aria-valuenow')).toBe('42');
    const bars = [...$('#spectrum', HTMLElement).children].map((bar) => (bar as HTMLElement).style.blockSize);
    expect(bars).toHaveLength(48);
    expect([bars[2], bars[3]]).toEqual(['0%', '50%']);
    expect(exists('#clipping')).toBe(false);
  });

  it('warns when the input clips', async () => {
    vi.useFakeTimers();
    const { audio } = await listening();
    audio.state.meter = { ...SILENT, peak: 1 };
    act(() => {
      vi.advanceTimersByTime(60);
    });
    expect(text('#clipping')).toBe('Too loud: turn the volume down or move the devices apart.');
    expect($('#level', HTMLElement).className).toBe('level clipping');
  });
});

describe('messages', () => {
  it('shows a received text with its quality', async () => {
    const { core } = await listening();
    expect(text('.empty')).toBe('Nothing yet. Texts sent and received appear here.');
    core.emit({ kind: 'message', text: 'Привет!', snrDb: 11.6, freqOffsetHz: 2 });
    expect(text('#feed .received .text')).toBe('Привет!');
    expect(text('#feed .received small')).toBe('Received · signal +12 dB to noise');
    expect(exists('.empty')).toBe(false);
  });

  it('says when a message was heard but not read', async () => {
    const { core } = await listening();
    core.emit({ kind: 'frameLost', snrDb: -20, freqOffsetHz: 0 });
    expect(text('#feed .lost')).toBe('A message was heard but could not be read.');
  });

  it('sends a text: transmitting until the last sample is played', async () => {
    const { core } = await listening();
    expect($('#send', HTMLButtonElement).disabled).toBe(true);
    await send('  hello ');
    expect(core.calls.sent).toEqual(['hello']);
    expect(text('#feed .sent .text')).toBe('hello');
    expect(text('#feed .sent small')).toBe('Sending… · 4.0 s');
    expect(text('#state')).toBe('Transmitting…');
    expect($('#draft', HTMLInputElement).value).toBe('');

    // nothing else goes out meanwhile
    await send('again');
    expect(core.calls.sent).toEqual(['hello']);
    expect($('#send', HTMLButtonElement).disabled).toBe(true);

    core.emit({ kind: 'txEnd' });
    expect(text('#state')).toBe('Listening');
    expect(text('#feed .sent small')).toBe('Sent · 4.0 s');
    expect($('#send', HTMLButtonElement).disabled).toBe(false);
  });

  it('does not send an empty text or one longer than a frame, counted in bytes', async () => {
    const { core } = await listening();
    await send('   ');
    await send('12345678901');
    expect(text('#too-long')).toBe('Too long by 1 byte.');
    await send('приветы');
    expect(text('#too-long')).toBe('Too long by 4 bytes.');
    expect(core.calls.sent).toEqual([]);
    await send('приве');
    expect(exists('#too-long')).toBe(false);
    expect(core.calls.sent).toEqual(['приве']);
  });

  it('says when the text was not sent and keeps it', async () => {
    const core = fakeCore({ sendText: () => Promise.reject(new Error('the modem is not started')) });
    await listening({ core });
    await send('hello');
    expect(text('#send-failed')).toBe('The text was not sent.');
    expect($('#draft', HTMLInputElement).value).toBe('hello');
    expect(text('#state')).toBe('Listening');
    expect(exists('#feed')).toBe(false);
  });

  it('keeps the messages when the language changes and when the microphone is turned off', async () => {
    const { core } = await listening();
    core.emit({ kind: 'message', text: 'stays', snrDb: 3, freqOffsetHz: 0 });
    selectLocale('ru');
    expect(text('#feed .received small')).toBe('Принято · сигнал/шум +3 дБ');
    expect(text('#state')).toBe('Приём');
    await press(() => $('#mic', HTMLButtonElement).click());
    expect(text('#feed .received .text')).toBe('stays');
  });
});

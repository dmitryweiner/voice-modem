// @vitest-environment jsdom
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App } from './App';
import type { AudioSession, StartAudio } from './audio/engine';
import { SILENT, type Meter } from './audio/meter';
import { LOCALE_NAMES, LOCALES } from './i18n';
import { loadSettings, type KeyValueStorage } from './storage';
import type { CoreClient } from './worker/client';
import type { CoreEvent, StationOptions } from './worker/protocol';

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
  const calls = {
    started: [] as [number, MessagePort, StationOptions][],
    sent: [] as string[],
    cancelled: [] as number[],
    files: [] as [string, Uint8Array][],
    accepted: [] as number[],
    givenUp: [] as number[],
    stopped: 0,
    hello: 0,
    ptt: 0,
  };
  const client: CoreClient = {
    version: () => Promise.resolve('1.2.3'),
    start(sampleRate, port, options) {
      calls.started.push([sampleRate, port, options]);
      return Promise.resolve({ maxTextBytes: 10 });
    },
    sendText(message) {
      calls.sent.push(message);
      return Promise.resolve({ id: calls.sent.length });
    },
    sendFile(name, bytes) {
      calls.files.push([name, bytes]);
      return Promise.resolve({ id: calls.files.length, seconds: 40 });
    },
    acceptFile(id) {
      calls.accepted.push(id);
      return Promise.resolve();
    },
    cancelFile(id) {
      calls.givenUp.push(id);
      return Promise.resolve();
    },
    hello() {
      calls.hello++;
      return Promise.resolve();
    },
    ptt() {
      calls.ptt++;
      return Promise.resolve();
    },
    cancel(id) {
      calls.cancelled.push(id);
      return Promise.resolve();
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
    render(
      <App
        storage={storage}
        languages={languages}
        core={core.client}
        startAudio={audio.startAudio}
        fileUrl={(bytes) => `blob:${bytes.length}:${bytes[0]}`}
      />,
    );
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
    expect(core.calls.started).toEqual([[48_000, audio.port, { name: '', manualPtt: false, oneWay: false, password: '', mode: null }]]);
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

const received = (message: string, snrDb = 3, sealed = false): CoreEvent => ({
  kind: 'message',
  text: message,
  station: 7,
  snrDb,
  freqOffsetHz: 0,
  sealed,
});

describe('messages', () => {
  it('shows a received text with its quality', async () => {
    const { core } = await listening();
    expect(text('.empty')).toBe('Nothing yet. Texts and files sent and received appear here.');
    core.emit(received('Привет!', 11.6));
    expect(text('#feed .received .text')).toBe('Привет!');
    expect(text('#feed .received small')).toBe('Received · signal +12 dB to noise');
    expect(exists('.empty')).toBe(false);
  });

  it('says when a message was heard but not read', async () => {
    const { core } = await listening();
    core.emit({ kind: 'frameLost', snrDb: -20, freqOffsetHz: 0 });
    expect(text('#feed .note')).toBe('A message was heard but could not be read.');
  });

  it('sends a text and follows it to delivery', async () => {
    const { core } = await listening();
    expect($('#send', HTMLButtonElement).disabled).toBe(true);
    await send('  hello ');
    expect(core.calls.sent).toEqual(['hello']);
    expect(text('#feed .sent .text')).toBe('hello');
    expect(text('#feed .sent small')).toBe('Sending…');
    expect($('#draft', HTMLInputElement).value).toBe('');

    core.emit({ kind: 'txStart', seconds: 4 });
    expect(text('#state')).toBe('Transmitting…');
    core.emit({ kind: 'txEnd' });
    expect(text('#state')).toBe('Listening');
    expect(text('#feed .sent small')).toBe('Sending…');

    core.emit({ kind: 'delivered', id: 1 });
    expect(text('#feed .sent small')).toBe('Delivered');
    expect($('#feed .sent', HTMLElement).dataset.status).toBe('delivered');
  });

  it('shows the repeats and the failure of a text nobody answers', async () => {
    const { core } = await listening();
    await send('hello');
    core.emit({ kind: 'retry', id: 1, attempt: 2 });
    expect(text('#feed .sent small')).toBe('Sending… · attempt 2');
    core.emit({ kind: 'retry', id: 1, attempt: 3 });
    expect(text('#feed .sent small')).toBe('Sending… · attempt 3');
    core.emit({ kind: 'failed', id: 1 });
    expect(text('#feed .sent small')).toBe('Not delivered: no answer');
    expect($('#feed .sent', HTMLElement).dataset.status).toBe('failed');
  });

  it('sends a failed text again at a press, in place of the old one', async () => {
    const { core } = await listening();
    await send('lost');
    await send('fine');
    expect(exists('.resend')).toBe(false);
    core.emit({ kind: 'failed', id: 1 });
    core.emit({ kind: 'delivered', id: 2 });
    const again = $('#feed .resend', HTMLButtonElement);
    expect(again.getAttribute('aria-label')).toBe('Send again');

    await press(() => again.click());
    expect(core.calls.sent).toEqual(['lost', 'fine', 'lost']);
    const feed = [...document.querySelectorAll('#feed .sent')].map((item) => [
      item.querySelector('.text')?.textContent,
      item.querySelector('small')?.textContent,
    ]);
    expect(feed).toEqual([
      ['fine', 'Delivered'],
      ['lost', 'Sending…'],
    ]);
    expect(exists('.resend')).toBe(false);
    core.emit({ kind: 'delivered', id: 3 });
    expect(text('#feed .sent:last-child small')).toBe('Delivered');
  });

  it('keeps a failed text when sending it again is refused', async () => {
    let refuse = false;
    const core = fakeCore({ sendText: () => (refuse ? Promise.reject(new Error('no')) : Promise.resolve({ id: 1 })) });
    await listening({ core });
    await send('lost');
    core.emit({ kind: 'failed', id: 1 });
    refuse = true;
    await press(() => $('#feed .resend', HTMLButtonElement).click());
    expect(text('#send-failed')).toBe('The text was not sent.');
    expect(text('#feed .sent small')).toBe('Not delivered: no answer');
  });

  it('takes back a text that is on its way, and can send it again', async () => {
    const { core } = await listening();
    await send('wrong');
    core.emit({ kind: 'retry', id: 1, attempt: 2 });
    const cancel = $('#feed .cancel', HTMLButtonElement);
    expect(cancel.getAttribute('aria-label')).toBe('Cancel');

    await press(() => cancel.click());
    expect(core.calls.cancelled).toEqual([1]);
    expect(text('#feed .sent small')).toBe('Cancelled');
    expect($('#feed .sent', HTMLElement).dataset.status).toBe('cancelled');
    expect(exists('.cancel')).toBe(false);

    await press(() => $('#feed .resend', HTMLButtonElement).click());
    expect(core.calls.sent).toEqual(['wrong', 'wrong']);
    expect(text('#feed .sent small')).toBe('Sending…');
  });

  it('offers to cancel only what is still on its way', async () => {
    const { core } = await listening();
    await send('one');
    await send('two');
    await send('three');
    core.emit({ kind: 'delivered', id: 1 });
    core.emit({ kind: 'failed', id: 2 });
    expect(document.querySelectorAll('#feed .cancel')).toHaveLength(1);
    expect(exists('#feed .sent:last-child .cancel')).toBe(true);
  });

  it('keeps a text on its way when the modem did not take it back', async () => {
    const core = fakeCore({ cancel: () => Promise.reject(new Error('the modem is not started')) });
    await listening({ core });
    await send('stays');
    await press(() => $('#feed .cancel', HTMLButtonElement).click());
    expect(text('#feed .sent small')).toBe('Sending…');
    // delivered while the request was on its way: that is what is shown
    core.emit({ kind: 'delivered', id: 1 });
    expect(text('#feed .sent small')).toBe('Delivered');
  });

  it('clears the messages at a press', async () => {
    const { core } = await listening();
    expect(exists('#clear')).toBe(false);
    core.emit(received('one'));
    await send('two');
    await press(() => $('#clear', HTMLButtonElement).click());
    expect(exists('#feed')).toBe(false);
    expect(text('.feed-box .empty')).toBe('Nothing yet. Texts and files sent and received appear here.');
    // what was on its way still reports, to nobody
    core.emit({ kind: 'delivered', id: 1 });
    core.emit(received('three'));
    expect(text('#feed .received .text')).toBe('three');
  });

  it('scrolls to the newest message', async () => {
    const { core } = await listening();
    const box = $('.feed-box', HTMLDivElement);
    Object.defineProperty(box, 'scrollHeight', { value: 900, configurable: true });
    core.emit(received('new'));
    expect(box.scrollTop).toBe(900);
  });

  it('queues more texts while one is on its way, each with its own fate', async () => {
    const { core } = await listening();
    await send('one');
    core.emit({ kind: 'txStart', seconds: 4 });
    await send('two');
    expect(core.calls.sent).toEqual(['one', 'two']);
    core.emit({ kind: 'failed', id: 1 });
    core.emit({ kind: 'delivered', id: 2 });
    const notes = [...document.querySelectorAll('#feed .sent small')].map((note) => note.textContent);
    expect(notes).toEqual(['Not delivered: no answer', 'Delivered']);
  });

  it('does not send an empty text or one that is too long, counted in bytes', async () => {
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
    expect(exists('#feed')).toBe(false);
  });

  it('keeps the messages when the language changes and when the microphone is turned off', async () => {
    const { core } = await listening();
    core.emit(received('stays'));
    await send('pending');
    selectLocale('ru');
    expect(text('#feed .received small')).toBe('Принято · сигнал/шум +3 дБ');
    expect(text('#state')).toBe('Приём');
    await press(() => $('#mic', HTMLButtonElement).click());
    expect(text('#feed .received .text')).toBe('stays');
    // what was on its way when the modem was turned off will not be acknowledged
    expect(text('#feed .sent small')).toBe('Не доставлено: нет ответа');
  });
});

const typeName = (name: string) => fireEvent.change($('#name', HTMLInputElement), { target: { value: name } });

/** Picks a file in the file input, as the dialog of the browser would. */
async function choose(name: string, bytes: Uint8Array<ArrayBuffer>) {
  await press(() => fireEvent.change($('#file', HTMLInputElement), { target: { files: [new File([bytes], name)] } }));
  // reading the file takes one more turn
  await press(() => {});
}

const offer: CoreEvent = { kind: 'fileOffer', id: 7, name: 'photo.jpg', size: 1536, station: 9 };

describe('files', () => {
  it('offers a chosen file and follows it to delivery', async () => {
    const { core } = await listening();
    const bytes = new Uint8Array(2048).fill(7);
    await choose('notes.txt', bytes);
    expect(core.calls.files).toEqual([['notes.txt', bytes]]);
    expect(text('#feed .file.sent .text')).toBe('notes.txt');
    expect(text('#feed .file small')).toBe('2.0 KB · Waiting for the other side to accept…');
    expect($('#feed .file', HTMLElement).dataset.status).toBe('offered');
    expect(exists('#feed progress')).toBe(false);

    core.emit({ kind: 'fileProgress', id: 1, done: 0, secondsLeft: null });
    expect(text('#feed .file small')).toBe('2.0 KB · Sending… 0%');
    core.emit({ kind: 'fileProgress', id: 1, done: 512, secondsLeft: 75 });
    expect(text('#feed .file small')).toBe('2.0 KB · Sending… 25% · about 75 s left');
    const bar = $('#feed progress', HTMLProgressElement);
    expect([bar.value, bar.max]).toEqual([512, 2048]);
    core.emit({ kind: 'fileProgress', id: 1, done: 1024, secondsLeft: 150 });
    expect(text('#feed .file small')).toBe('2.0 KB · Sending… 50% · about 3 min left');

    core.emit({ kind: 'fileDelivered', id: 1 });
    expect(text('#feed .file small')).toBe('2.0 KB · Delivered');
    expect(exists('#feed progress')).toBe(false);
    expect(exists('#feed .cancel')).toBe(false);
  });

  it('says how long a file takes when that is more than ten minutes', async () => {
    const core = fakeCore({ sendFile: () => Promise.resolve({ id: 1, seconds: 1500 }) });
    await listening({ core });
    await choose('big.bin', new Uint8Array(3_000_000));
    expect(text('#feed .file small')).toBe('2.9 MB · Waiting for the other side to accept… · about 25 min at the current speed');
  });

  it('shows an offered file, accepts it and gives what arrives to save', async () => {
    const { core } = await listening();
    core.emit(offer);
    expect(text('#feed .file.received .text')).toBe('photo.jpg');
    expect(text('#feed .file small')).toBe('1.5 KB · Wants to send you this file');
    expect($('#feed .accept', HTMLButtonElement).getAttribute('aria-label')).toBe('Accept');
    expect($('#feed .cancel', HTMLButtonElement).getAttribute('aria-label')).toBe('Decline');

    await press(() => $('#feed .accept', HTMLButtonElement).click());
    expect(core.calls.accepted).toEqual([7]);
    expect(text('#feed .file small')).toBe('1.5 KB · Receiving… 0%');
    expect(exists('#feed .accept')).toBe(false);
    expect($('#feed .cancel', HTMLButtonElement).getAttribute('aria-label')).toBe('Cancel');
    core.emit({ kind: 'fileProgress', id: 7, done: 1000, secondsLeft: 4 });
    expect(text('#feed .file small')).toBe('1.5 KB · Receiving… 65% · about 4 s left');

    core.emit({ kind: 'fileReceived', id: 7, name: 'photo.jpg', bytes: new Uint8Array(1536).fill(5) });
    expect(text('#feed .file small')).toBe('1.5 KB · Received');
    const save = $('#feed .save', HTMLAnchorElement);
    expect([save.textContent, save.getAttribute('href'), save.download]).toEqual(['Save', 'blob:1536:5', 'photo.jpg']);
    expect(exists('#feed .cancel')).toBe(false);
  });

  it('declines an offered file', async () => {
    const { core } = await listening();
    core.emit(offer);
    await press(() => $('#feed .cancel', HTMLButtonElement).click());
    expect(core.calls.givenUp).toEqual([7]);
    expect(text('#feed .file small')).toBe('1.5 KB · Cancelled');
    expect(exists('#feed button')).toBe(false);
  });

  it('gives up a file on its way, and offers it again at a press', async () => {
    const { core } = await listening();
    const bytes = new Uint8Array([1, 2, 3]);
    await choose('notes.txt', bytes);
    core.emit({ kind: 'fileProgress', id: 1, done: 1, secondsLeft: 9 });
    await press(() => $('#feed .cancel', HTMLButtonElement).click());
    expect(core.calls.givenUp).toEqual([1]);
    expect(text('#feed .file small')).toBe('3 bytes · Cancelled');
    // a chunk acknowledged meanwhile changes nothing
    core.emit({ kind: 'fileProgress', id: 1, done: 2, secondsLeft: 5 });
    expect(text('#feed .file small')).toBe('3 bytes · Cancelled');

    await press(() => $('#feed .resend', HTMLButtonElement).click());
    expect(core.calls.files).toEqual([
      ['notes.txt', bytes],
      ['notes.txt', bytes],
    ]);
    expect(document.querySelectorAll('#feed .file')).toHaveLength(1);
    expect(text('#feed .file small')).toBe('3 bytes · Waiting for the other side to accept…');
  });

  it('says what went wrong with a file sent', async () => {
    const { core } = await listening();
    await choose('one', new Uint8Array(1));
    await choose('two', new Uint8Array(1));
    core.emit({ kind: 'fileRefused', id: 1 });
    core.emit({ kind: 'fileFailed', id: 2 });
    const notes = [...document.querySelectorAll('#feed .file small')].map((note) => note.textContent);
    expect(notes).toEqual(['1 byte · Refused by the other side', '1 byte · Not delivered: no answer']);
    expect(document.querySelectorAll('#feed .resend')).toHaveLength(2);
  });

  it('says what went wrong with a file being received', async () => {
    const { core } = await listening();
    core.emit(offer);
    core.emit({ ...offer, id: 8 });
    await press(() => $('#feed .accept', HTMLButtonElement).click());
    core.emit({ kind: 'fileFailed', id: 7 });
    core.emit({ kind: 'fileRefused', id: 8 });
    const notes = [...document.querySelectorAll('#feed .file small')].map((note) => note.textContent);
    expect(notes).toEqual(['1.5 KB · Not received', '1.5 KB · Cancelled by the sender']);
    expect(exists('#feed button')).toBe(false);
  });

  it('says when the file was not offered', async () => {
    const core = fakeCore({ sendFile: () => Promise.reject(new Error('no')) });
    await listening({ core });
    await choose('notes.txt', new Uint8Array(1));
    expect(text('#send-failed')).toBe('The file was not sent.');
    expect(exists('#feed')).toBe(false);
  });

  it('keeps a file that arrives after the messages were cleared', async () => {
    const { core } = await listening();
    core.emit(offer);
    await press(() => $('#feed .accept', HTMLButtonElement).click());
    await press(() => $('#clear', HTMLButtonElement).click());
    core.emit({ kind: 'fileReceived', id: 7, name: 'photo.jpg', bytes: new Uint8Array([4]) });
    expect(text('#feed .file small')).toBe('1 byte · Received');
    expect($('#feed .save', HTMLAnchorElement).getAttribute('href')).toBe('blob:1:4');
  });

  it('does not take the files of a restarted modem for the old ones with the same numbers', async () => {
    const { core } = await listening();
    await choose('old.txt', new Uint8Array(1));
    await send('old');
    await press(() => $('#mic', HTMLButtonElement).click());
    expect(text('#feed .file small')).toBe('1 byte · Not delivered: no answer');
    await press(() => $('#mic', HTMLButtonElement).click());
    // the new modem counts from one again
    core.emit({ kind: 'fileDelivered', id: 1 });
    core.emit({ kind: 'delivered', id: 1 });
    expect(text('#feed .file small')).toBe('1 byte · Not delivered: no answer');
    expect(text('#feed .sent:not(.file) small')).toBe('Not delivered: no answer');
  });

  it('has nothing to attach while the microphone is off', async () => {
    await start();
    expect(exists('#file')).toBe(false);
  });
});

/** The app with the microphone turned on as a one-way station. */
async function oneWay() {
  const started = await start();
  fireEvent.click($('#one-way', HTMLInputElement));
  await press(() => $('#mic', HTMLButtonElement).click());
  return started;
}

describe('password', () => {
  it('goes to the modem and is never stored', async () => {
    const storage = new MemoryStorage();
    const { core } = await start({ storage });
    const field = $('#password', HTMLInputElement);
    expect([field.type, field.getAttribute('aria-label')]).toEqual(['password', 'Password']);
    expect(exists('#password-warning')).toBe(false);
    fireEvent.change(field, { target: { value: 'correct horse' } });
    expect(text('#password-warning')).toBe('Encryption is forbidden on amateur radio bands.');
    await press(() => $('#mic', HTMLButtonElement).click());
    expect(core.calls.started[0]?.[2]).toEqual({ name: '', manualPtt: false, oneWay: false, password: 'correct horse', mode: null });
    expect(text('#sealing')).toBe('Sealed with the password');
    // the name is kept, the password is not
    fireEvent.click($('#mic', HTMLButtonElement));
    fireEvent.change($('#name', HTMLInputElement), { target: { value: 'Alice' } });
    expect([...storage.data.values()].join()).not.toContain('correct horse');
    cleanup();
    await start({ storage });
    expect($('#password', HTMLInputElement).value).toBe('');
  });

  it('marks what came sealed, and says when something sealed did not open', async () => {
    const { core } = await listening();
    core.emit(received('meet at the bridge', 12, true));
    expect(text('#feed .received small')).toBe('Received · sealed · signal +12 dB to noise');
    core.emit({ kind: 'unopened', station: 7, snrDb: 5, freqOffsetHz: 0 });
    expect(text('#feed .note')).toBe('A sealed message did not open: is the password the same?');
    expect(exists('#sealing')).toBe(false);
  });
});

describe('one way', () => {
  it('is remembered and given to the modem', async () => {
    const { storage, core } = await oneWay();
    expect($('#one-way', HTMLInputElement).closest('label')?.textContent).toBe('One-way');
    expect(loadSettings(storage).oneWay).toBe(true);
    expect(core.calls.started[0]?.[2]).toEqual({ name: '', manualPtt: false, oneWay: true, password: '', mode: null });
    cleanup();
    await start({ storage });
    expect($('#one-way', HTMLInputElement).checked).toBe(true);
  });

  it('broadcasts a chosen file until it is stopped', async () => {
    const { core } = await oneWay();
    await choose('photo.jpg', new Uint8Array(2048));
    expect(core.calls.files).toEqual([['photo.jpg', new Uint8Array(2048)]]);
    // nobody is asked: it goes at once
    expect(text('#feed .file small')).toBe('2.0 KB · Broadcasting… 0% sent');
    expect($('#feed .file', HTMLElement).dataset.status).toBe('moving');
    core.emit({ kind: 'fileCast', id: 1, symbols: 3, needed: 6 });
    expect(text('#feed .file small')).toBe('2.0 KB · Broadcasting… 50% sent');
    const bar = $('#feed progress', HTMLProgressElement);
    expect([bar.value, bar.max]).toEqual([3, 6]);
    // on and on: whoever listens has a better chance with every symbol
    core.emit({ kind: 'fileCast', id: 1, symbols: 9, needed: 6 });
    expect(text('#feed .file small')).toBe('2.0 KB · Broadcasting… 150% sent');
    expect([bar.value, bar.max]).toEqual([6, 6]);
    await press(() => $('#feed .cancel', HTMLButtonElement).click());
    expect(core.calls.givenUp).toEqual([1]);
    expect(text('#feed .file small')).toBe('2.0 KB · Cancelled');
    // and again
    await press(() => $('#feed .resend', HTMLButtonElement).click());
    expect(text('#feed .file small')).toBe('2.0 KB · Broadcasting… 0% sent');
  });

  it('says when a broadcast ended by itself', async () => {
    const { core } = await oneWay();
    await choose('tiny', new Uint8Array(1));
    core.emit({ kind: 'fileSent', id: 1 });
    expect(text('#feed .file small')).toBe('1 byte · Sent');
    expect(exists('#feed button')).toBe(false);
  });

  it('collects a file that a station broadcasts, without being asked', async () => {
    const { core } = await listening();
    core.emit({ kind: 'fileHeard', id: 4, name: 'photo.jpg', size: 1536, station: 9 });
    expect(text('#feed .file.received small')).toBe('1.5 KB · Receiving… 0%');
    expect(exists('#feed .accept')).toBe(false);
    expect($('#feed .cancel', HTMLButtonElement).getAttribute('aria-label')).toBe('Cancel');
    core.emit({ kind: 'fileProgress', id: 4, done: 768, secondsLeft: 20 });
    expect(text('#feed .file small')).toBe('1.5 KB · Receiving… 50% · about 20 s left');
    core.emit({ kind: 'fileReceived', id: 4, name: 'photo.jpg', bytes: new Uint8Array(1536).fill(2) });
    expect($('#feed .save', HTMLAnchorElement).getAttribute('href')).toBe('blob:1536:2');
  });

  it('stops collecting at a press', async () => {
    const { core } = await listening();
    core.emit({ kind: 'fileHeard', id: 4, name: 'photo.jpg', size: 1536, station: 9 });
    await press(() => $('#feed .cancel', HTMLButtonElement).click());
    expect(core.calls.givenUp).toEqual([4]);
    expect(text('#feed .file small')).toBe('1.5 KB · Cancelled');
  });
});

describe('station', () => {
  it('remembers the name and the kind of radio, and gives them to the modem', async () => {
    const { storage, core } = await start();
    typeName('UB1XYZ');
    expect($('#ptt-mode', HTMLInputElement).closest('label')?.textContent).toBe('Push-to-talk');
    fireEvent.click($('#ptt-mode', HTMLInputElement));
    fireEvent.change($('#mode', HTMLSelectElement), { target: { value: '2' } });
    expect(loadSettings(storage)).toEqual({ locale: null, name: 'UB1XYZ', manualPtt: true, oneWay: false, mode: 2 });
    await press(() => $('#mic', HTMLButtonElement).click());
    expect(core.calls.started[0]?.[2]).toEqual({ name: 'UB1XYZ', manualPtt: true, oneWay: false, password: '', mode: 2 });
    expect(text('#speed')).toBe('250 bit/s');

    cleanup();
    await start({ storage });
    expect($('#name', HTMLInputElement).value).toBe('UB1XYZ');
    expect($('#ptt-mode', HTMLInputElement).checked).toBe(true);
    expect($('#mode', HTMLSelectElement).value).toBe('2');
    fireEvent.change($('#mode', HTMLSelectElement), { target: { value: 'auto' } });
    expect(loadSettings(storage).mode).toBeNull();
  });

  it('shows the speed the modem has settled on', async () => {
    const { core } = await listening();
    expect([...$('#mode', HTMLSelectElement).options].map((o) => o.textContent)).toEqual([
      'Speed: by the channel',
      '62.5 bit/s',
      '125 bit/s',
      '250 bit/s',
      '500 bit/s',
      '1400 bit/s',
      '2800 bit/s',
      '4200 bit/s',
    ]);
    expect(text('#speed')).toBe('62.5 bit/s');
    core.emit({ kind: 'txMode', mode: 2, bitRate: 250 });
    expect(text('#speed')).toBe('250 bit/s');
    selectLocale('ru');
    core.emit({ kind: 'txMode', mode: 0, bitRate: 62.5 });
    expect(text('#speed')).toBe('62,5 бит/с');
  });

  it('cannot be changed while the modem runs', async () => {
    await listening();
    expect($('.station', HTMLFieldSetElement).disabled).toBe(true);
    await press(() => $('#mic', HTMLButtonElement).click());
    expect($('.station', HTMLFieldSetElement).disabled).toBe(false);
  });

  it('says hello and shows who is on the channel', async () => {
    const { core } = await listening();
    await press(() => $('#hello', HTMLButtonElement).click());
    expect(core.calls.hello).toBe(1);
    core.emit({ kind: 'hello', name: 'Боб', station: 9 });
    core.emit({ kind: 'hello', name: '', station: 10 });
    const notes = [...document.querySelectorAll('#feed .peer')].map((note) => note.textContent);
    expect(notes).toEqual(['Боб is on the channel.', 'A station is on the channel.']);
  });
});

/** The app as a push-to-talk radio, microphone on. */
async function radio() {
  const started = await start();
  fireEvent.click($('#ptt-mode', HTMLInputElement));
  await press(() => $('#mic', HTMLButtonElement).click());
  return started;
}

describe('push-to-talk', () => {
  it('asks for the button when the modem has something to transmit', async () => {
    const { core } = await radio();
    expect(exists('#ptt')).toBe(false);
    core.emit({ kind: 'txRequest' });
    expect(text('#ptt p')).toBe('Press and hold the button of the radio, then press here or the space bar.');

    await press(() => $('#ptt-button', HTMLButtonElement).click());
    expect(core.calls.ptt).toBe(1);
    expect(exists('#ptt')).toBe(false);
    core.emit({ kind: 'txStart', seconds: 5 });
    expect(text('#state')).toBe('Transmitting… keep the button pressed');
    core.emit({ kind: 'txEnd' });
    expect(text('#state')).toBe('Listening');
  });

  it('takes the space bar for the button, but not while a text is typed', async () => {
    const { core } = await radio();
    core.emit({ kind: 'txRequest' });
    fireEvent.keyDown($('#draft', HTMLInputElement), { code: 'Space' });
    expect(core.calls.ptt).toBe(0);
    fireEvent.keyDown(document.body, { code: 'Enter' });
    expect(core.calls.ptt).toBe(0);
    await press(() => fireEvent.keyDown(document.body, { code: 'Space' }));
    expect(core.calls.ptt).toBe(1);
    expect(exists('#ptt')).toBe(false);
    // no request, no key
    fireEvent.keyDown(document.body, { code: 'Space' });
    expect(core.calls.ptt).toBe(1);
  });

  it('forgets the request when the microphone is turned off', async () => {
    const { core } = await radio();
    core.emit({ kind: 'txRequest' });
    await press(() => $('#mic', HTMLButtonElement).click());
    await press(() => $('#mic', HTMLButtonElement).click());
    expect(exists('#ptt')).toBe(false);
  });
});

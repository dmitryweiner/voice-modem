// @vitest-environment jsdom
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { App } from './App';
import { LOCALE_NAMES, LOCALES } from './i18n';
import { loadSettings, type KeyValueStorage } from './storage';
import type { CoreClient } from './worker/client';

function $<T extends Element>(selector: string, type: abstract new () => T): T {
  const el = document.querySelector(selector);
  if (!(el instanceof type)) throw new Error(`missing ${selector}`);
  return el;
}
const text = (selector: string) => $(selector, HTMLElement).textContent;

class MemoryStorage implements KeyValueStorage {
  data = new Map<string, string>();
  getItem(key: string) {
    return this.data.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.data.set(key, value);
  }
}

const ready: CoreClient = { version: () => Promise.resolve('1.2.3') };
const failing: CoreClient = { version: () => Promise.reject(new Error('no wasm')) };
const never: CoreClient = { version: () => new Promise(() => {}) };

/** Renders the app and lets the core answer. */
async function start({ storage = new MemoryStorage(), languages = ['en-US'], core = ready } = {}) {
  // oxlint-disable-next-line typescript/require-await -- async `act` flushes the promise of the core
  await act(async () => {
    render(<App storage={storage} languages={languages} core={core} />);
  });
  return storage;
}

afterEach(cleanup);

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
    const storage = await start({ languages: ['en-US'] });
    fireEvent.change($('#locale', HTMLSelectElement), { target: { value: 'ru' } });
    expect(document.documentElement.lang).toBe('ru');
    expect(loadSettings(storage).locale).toBe('ru');
    cleanup();
    await start({ storage, languages: ['en-US'] });
    expect(document.documentElement.lang).toBe('ru');
    expect($('#locale', HTMLSelectElement).value).toBe('ru');
  });

  it('works without storage', async () => {
    render(<App storage={null} languages={[]} core={never} />);
    fireEvent.change($('#locale', HTMLSelectElement), { target: { value: 'ru' } });
    expect(text('h1')).toBe('Голосовой модем');
  });
});

describe('modem core', () => {
  it('says it is loading until the core answers', () => {
    render(<App storage={null} languages={[]} core={never} />);
    expect(text('#core')).toBe('Loading the modem core…');
    expect($('#core', HTMLElement).dataset.status).toBe('loading');
  });

  it('shows the version of the loaded core', async () => {
    await start();
    expect(text('#core')).toBe('Modem core 1.2.3 is ready.');
    expect($('#core', HTMLElement).dataset.status).toBe('ready');
  });

  it('says so when the core did not load', async () => {
    await start({ core: failing });
    expect(text('#core')).toBe('The modem core did not load.');
    expect($('#core', HTMLElement).dataset.status).toBe('failed');
  });

  it('keeps the core state when the language changes', async () => {
    await start();
    fireEvent.change($('#locale', HTMLSelectElement), { target: { value: 'ru' } });
    expect(text('#core')).toBe('Ядро модема 1.2.3 готово.');
  });
});

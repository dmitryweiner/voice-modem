// @vitest-environment jsdom
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { detectLocale, direction, isLocale, LOCALES, messages } from './index';
import { plural } from './plural';

const html = readFileSync(resolve(__dirname, '../../index.html'), 'utf8');

describe('detectLocale', () => {
  it('prefers the saved locale', () => {
    expect(detectLocale('en', ['ru-RU'])).toBe('en');
  });

  it('takes the first supported browser language', () => {
    expect(detectLocale(null, ['ru-RU', 'en'])).toBe('ru');
    expect(detectLocale(null, ['de-DE', 'ru', 'en'])).toBe('ru');
    expect(detectLocale(null, ['en-GB', 'ru'])).toBe('en');
    expect(detectLocale(null, ['RU'])).toBe('ru');
  });

  it('falls back to English', () => {
    expect(detectLocale(null, [])).toBe('en');
    expect(detectLocale(null, ['de-DE', 'fr'])).toBe('en');
    expect(detectLocale('xx', ['de'])).toBe('en');
  });
});

describe('isLocale', () => {
  it('accepts only supported locales', () => {
    expect(LOCALES.every(isLocale)).toBe(true);
    expect(isLocale('de')).toBe(false);
    expect(isLocale(null)).toBe(false);
  });
});

describe('direction', () => {
  it('is left-to-right for every current locale', () => {
    expect(LOCALES.map(direction)).toEqual(LOCALES.map(() => 'ltr'));
  });
});

describe('messages', () => {
  it('have the same keys in every locale', () => {
    const keys = Object.keys(messages.en).toSorted();
    for (const locale of LOCALES) expect(Object.keys(messages[locale]).toSorted()).toEqual(keys);
  });

  it.each(LOCALES)('are not empty (%s)', (locale) => {
    for (const [key, value] of Object.entries(messages[locale])) {
      const text: unknown = typeof value === 'function' ? value('1.2.3') : value;
      expect({ key, text }).toEqual({ key, text: expect.stringMatching(/\S/) });
    }
  });

  it('match the English defaults in index.html', () => {
    const doc = new DOMParser().parseFromString(html, 'text/html');
    expect(doc.title).toBe(messages.en.title);
    expect(doc.querySelector('meta[name="description"]')?.getAttribute('content')).toBe(messages.en.description);
  });

  it('put the core version into the status line', () => {
    expect(messages.en.coreReady('0.1.0')).toBe('Modem core 0.1.0 is ready.');
    expect(messages.ru.coreReady('0.1.0')).toBe('Ядро модема 0.1.0 готово.');
  });
});

describe('plural', () => {
  it('picks the form by the rules of the locale', () => {
    const forms = { one: 'файл', few: 'файла', many: 'файлов', other: 'файла' };
    expect([1, 3, 5, 21].map((n) => plural('ru', n, forms))).toEqual(['файл', 'файла', 'файлов', 'файл']);
  });

  it('falls back to "other" when a form is missing', () => {
    expect(plural('ru', 3, { other: 'x' })).toBe('x');
  });
});

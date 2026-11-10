import { en, type Messages } from './en';
import { ru } from './ru';

export type { Messages };
export type Locale = 'en' | 'ru';

export const LOCALES: readonly Locale[] = ['en', 'ru'];
export const DEFAULT_LOCALE: Locale = 'en';
/** Language names for the switcher, each in its own language. */
export const LOCALE_NAMES: Record<Locale, string> = { en: 'English', ru: 'Русский' };
/** No right-to-left language yet; adding one here is all the layout needs (CSS uses logical properties). */
const RTL_LOCALES: ReadonlySet<Locale> = new Set();

export const messages: Record<Locale, Messages> = { en, ru };

export function isLocale(value: unknown): value is Locale {
  return LOCALES.some((locale) => locale === value);
}

/** The saved locale, else the first supported browser language, else English. */
export function detectLocale(saved: unknown, languages: readonly string[]): Locale {
  if (isLocale(saved)) return saved;
  for (const language of languages) {
    const base = language.toLowerCase().split(/[-_]/)[0];
    if (isLocale(base)) return base;
  }
  return DEFAULT_LOCALE;
}

/** Text direction of the locale, for `<html dir>`. */
export function direction(locale: Locale): 'ltr' | 'rtl' {
  return RTL_LOCALES.has(locale) ? 'rtl' : 'ltr';
}

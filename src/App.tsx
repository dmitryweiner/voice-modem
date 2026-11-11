import { useEffect, useState } from 'react';
import type { StartAudio } from './audio/engine';
import { detectLocale, direction, isLocale, LOCALE_NAMES, LOCALES, messages, type Messages } from './i18n';
import { ModemPanel } from './ModemPanel';
import { loadSettings, saveSettings, type KeyValueStorage, type Settings } from './storage';
import type { CoreClient } from './worker/client';

export interface AppProps {
  storage: KeyValueStorage | null;
  /** Browser languages in preference order (`navigator.languages`). */
  languages: readonly string[];
  core: CoreClient;
  /** Opens the microphone and the speaker. */
  startAudio: StartAudio;
}

type CoreState = { status: 'loading' } | { status: 'ready'; version: string } | { status: 'failed' };

function coreText(t: Messages, core: CoreState): string {
  if (core.status === 'ready') return t.coreReady(core.version);
  return core.status === 'failed' ? t.coreFailed : t.coreLoading;
}

export function App({ storage, languages, core, startAudio }: AppProps) {
  const [settings, setSettings] = useState<Settings>(() => loadSettings(storage));
  const [coreState, setCoreState] = useState<CoreState>({ status: 'loading' });
  const locale = detectLocale(settings.locale, languages);
  const t = messages[locale];

  useEffect(() => {
    document.documentElement.lang = locale;
    document.documentElement.dir = direction(locale);
    document.title = t.title;
    document.querySelector('meta[name="description"]')?.setAttribute('content', t.description);
  }, [locale, t]);

  useEffect(() => {
    let current = true;
    core.version().then(
      (version) => current && setCoreState({ status: 'ready', version }),
      () => current && setCoreState({ status: 'failed' }),
    );
    return () => {
      current = false;
    };
  }, [core]);

  const update = (next: Settings) => {
    setSettings(next);
    saveSettings(storage, next);
  };

  return (
    <>
      <header>
        <span className="brand">{t.title}</span>
        <select
          id="locale"
          aria-label={t.languageLabel}
          value={locale}
          onChange={(event) => {
            const { value } = event.target;
            if (isLocale(value)) update({ ...settings, locale: value });
          }}
        >
          {LOCALES.map((l) => (
            <option key={l} value={l} lang={l}>
              {LOCALE_NAMES[l]}
            </option>
          ))}
        </select>
      </header>
      <main>
        <h1>{t.title}</h1>
        <p className="intro">{t.intro}</p>
        <ModemPanel
          t={t}
          core={core}
          startAudio={startAudio}
          coreReady={coreState.status === 'ready'}
          station={{ name: settings.name, manualPtt: settings.manualPtt }}
          onStationChange={(station) => update({ ...settings, ...station })}
        />
      </main>
      <footer>
        <output id="core" data-status={coreState.status} aria-live="polite">
          {coreText(t, coreState)}
        </output>
        <a href="https://github.com/dmitryweiner/voice-modem">{t.source}</a> · GPL-3.0-or-later
      </footer>
    </>
  );
}

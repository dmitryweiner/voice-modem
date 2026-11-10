// English is the default locale and defines the shape of every message set.
export const en = {
  title: 'Voice Modem',
  description: 'An acoustic modem in the browser: text and small files over any voice channel.',
  intro:
    'Text and small files over a phone call, a radio or just a speaker and a microphone. The modem is being built; for now this page only loads its core.',
  languageLabel: 'Language',
  source: 'Source code',
  coreLoading: 'Loading the modem core…',
  coreFailed: 'The modem core did not load.',

  coreReady: (version: string) => `Modem core ${version} is ready.`,
};

export type Messages = typeof en;

import { describe, expect, it } from 'vitest';
import { DEFAULT_SETTINGS, loadSettings, saveSettings, STORAGE_KEY, type KeyValueStorage } from './storage';

class MemoryStorage implements KeyValueStorage {
  data = new Map<string, string>();
  getItem(key: string) {
    return this.data.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.data.set(key, value);
  }
}

const broken: KeyValueStorage = {
  getItem: () => {
    throw new Error('denied');
  },
  setItem: () => {
    throw new Error('denied');
  },
};

describe('settings storage', () => {
  it('uses the package name as the key', () => {
    expect(STORAGE_KEY).toBe('voice-modem');
  });

  it('returns defaults when nothing is saved or storage is unavailable', () => {
    expect(loadSettings(new MemoryStorage())).toEqual(DEFAULT_SETTINGS);
    expect(loadSettings(null)).toEqual(DEFAULT_SETTINGS);
    expect(loadSettings(broken)).toEqual(DEFAULT_SETTINGS);
  });

  it('saves and loads settings', () => {
    const storage = new MemoryStorage();
    const settings = { locale: 'ru', name: 'UB1XYZ', manualPtt: true, mode: 2 } as const;
    saveSettings(storage, settings);
    expect(loadSettings(storage)).toEqual(settings);
  });

  it('ignores write errors', () => {
    expect(() => saveSettings(broken, DEFAULT_SETTINGS)).not.toThrow();
    expect(() => saveSettings(null, DEFAULT_SETTINGS)).not.toThrow();
  });

  it.each(['not json', '[]', '{"version":2,"settings":{"locale":"ru"}}', '{"version":1}'])('resets unreadable data: %s', (raw) => {
    const storage = new MemoryStorage();
    storage.setItem(STORAGE_KEY, raw);
    expect(loadSettings(storage)).toEqual(DEFAULT_SETTINGS);
  });

  it('replaces bad fields with defaults one by one', () => {
    const storage = new MemoryStorage();
    const save = (settings: unknown) => storage.setItem(STORAGE_KEY, JSON.stringify({ version: 1, settings }));
    save({ locale: 'xx', name: 'Alice', manualPtt: true });
    expect(loadSettings(storage)).toEqual({ locale: null, name: 'Alice', manualPtt: true, mode: null });
    save({ locale: 'ru', name: 7, manualPtt: 'yes', unknown: 1 });
    expect(loadSettings(storage)).toEqual({ locale: 'ru', name: '', manualPtt: false, mode: null });
    save({ locale: 'ru', name: 'x'.repeat(33) });
    expect(loadSettings(storage).name).toBe('');
    for (const mode of [7, -1, 1.5, '2']) {
      save({ mode });
      expect(loadSettings(storage).mode).toBeNull();
    }
    save({ mode: 6 });
    expect(loadSettings(storage).mode).toBe(6);
  });

  it('reads what an earlier version saved: the new fields get their defaults', () => {
    const storage = new MemoryStorage();
    storage.setItem(STORAGE_KEY, '{"version":1,"settings":{"locale":"ru"}}');
    expect(loadSettings(storage)).toEqual({ locale: 'ru', name: '', manualPtt: false, mode: null });
  });
});

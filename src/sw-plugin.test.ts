import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { precacheFiles, serviceWorkerPlugin, serviceWorkerSource } from './sw-plugin';

type Listener = (event: {
  request?: FakeRequest;
  waitUntil: (p: Promise<unknown>) => void;
  respondWith: (p: Promise<unknown>) => void;
}) => void;
interface FakeRequest {
  url: string;
  method: string;
  mode: string;
}

/** Runs the generated worker against in-memory caches and returns its event dispatcher. */
function runWorker(source: string, network: (url: string) => Promise<string>) {
  const stores = new Map<string, Map<string, string>>();
  const listeners = new Map<string, Listener>();
  const origin = 'https://example.org';
  const scope = `${origin}/app/`;
  const key = (r: string | FakeRequest) => new URL(typeof r === 'string' ? r : r.url, scope).href.split('?')[0]!;
  const caches = {
    open: async (name: string) => {
      const store = stores.get(name) ?? new Map<string, string>();
      stores.set(name, store);
      return {
        addAll: async (urls: string[]) => {
          const bodies = await Promise.all(urls.map((u) => network(key(u))));
          urls.forEach((u, i) => store.set(key(u), bodies[i] ?? ''));
        },
        put: async (r: FakeRequest, body: string) => void store.set(key(r), body),
      };
    },
    keys: async () => [...stores.keys()],
    delete: async (name: string) => stores.delete(name),
    // Responses carry `Vary: Origin`, so they only match when Vary is ignored.
    match: async (r: string | FakeRequest, options?: { ignoreVary?: boolean }) => {
      if (!options?.ignoreVary) return undefined;
      for (const s of stores.values()) if (s.has(key(r))) return s.get(key(r));
      return undefined;
    },
  };
  const self = {
    addEventListener: (type: string, fn: Listener) => listeners.set(type, fn),
    skipWaiting: vi.fn<() => Promise<void>>(async () => {}),
    clients: { claim: vi.fn<() => Promise<void>>(async () => {}) },
    location: { origin, href: `${scope}sw.js` },
  };
  const fakeFetch = async (r: FakeRequest) => {
    const body = await network(key(r));
    return { ok: true, clone: () => body, body };
  };
  // Runs the generated worker source with fake browser globals.
  // oxlint-disable-next-line typescript/no-implied-eval
  new Function('self', 'caches', 'fetch', 'location', source)(self, caches, fakeFetch, self.location);
  const dispatch = async (type: string, request?: FakeRequest) => {
    let pending: Promise<unknown> = Promise.resolve();
    let response: Promise<unknown> | undefined;
    listeners.get(type)!({ request, waitUntil: (p) => (pending = p), respondWith: (p) => (response = p) });
    await pending;
    return response ? await response : undefined;
  };
  return { stores, dispatch, self, scope };
}

describe('precacheFiles', () => {
  it('lists the start page and all build files except the worker and source maps, sorted', () => {
    expect(precacheFiles(['index.html', 'assets/b.js', 'assets/b.js.map', 'sw.js'], ['manifest.webmanifest', 'icon-192.png'])).toEqual([
      './',
      'assets/b.js',
      'icon-192.png',
      'index.html',
      'manifest.webmanifest',
    ]);
  });
});

const online = async (url: string) => `net:${url}`;

describe('generated service worker', () => {
  const files = ['./', 'index.html', 'assets/app.js'];

  it('precaches every file on install', async () => {
    const w = runWorker(serviceWorkerSource('app', 'v1', files), online);
    await w.dispatch('install');
    expect([...w.stores.get('app-v1')!.keys()]).toEqual([w.scope, `${w.scope}index.html`, `${w.scope}assets/app.js`]);
  });

  it('takes over right after install: mobile pages are suspended, not closed, so waiting never ends', async () => {
    const w = runWorker(serviceWorkerSource('app', 'v1', files), online);
    await w.dispatch('install');
    expect(w.self.skipWaiting).toHaveBeenCalled();
  });

  it('removes caches of older versions on activate', async () => {
    const old = runWorker(serviceWorkerSource('app', 'v1', files), online);
    await old.dispatch('install');
    const w = runWorker(serviceWorkerSource('app', 'v2', files), online);
    for (const [name, store] of old.stores) w.stores.set(name, store);
    await w.dispatch('install');
    await w.dispatch('activate');
    expect([...w.stores.keys()]).toEqual(['app-v2']);
    expect(w.self.clients.claim).toHaveBeenCalled();
  });

  it('keeps caches of other apps: every project on <user>.github.io shares one origin', async () => {
    const other = runWorker(serviceWorkerSource('other-app', 'v1', files), online);
    await other.dispatch('install');
    const w = runWorker(serviceWorkerSource('app', 'v1', files), online);
    for (const [name, store] of other.stores) w.stores.set(name, store);
    await w.dispatch('install');
    await w.dispatch('activate');
    expect([...w.stores.keys()].toSorted()).toEqual(['app-v1', 'other-app-v1']);
  });

  it('serves cached files and the start page when offline', async () => {
    let offline = false;
    const w = runWorker(serviceWorkerSource('app', 'v1', files), (url) => (offline ? Promise.reject(new Error('offline')) : online(url)));
    await w.dispatch('install');
    offline = true;
    expect(await w.dispatch('fetch', { url: `${w.scope}assets/app.js`, method: 'GET', mode: 'cors' })).toBe(`net:${w.scope}assets/app.js`);
    expect(await w.dispatch('fetch', { url: `${w.scope}?utm=1`, method: 'GET', mode: 'navigate' })).toBe(`net:${w.scope}`);
  });

  it('ignores other origins and non-GET requests', async () => {
    const w = runWorker(serviceWorkerSource('app', 'v1', files), online);
    expect(await w.dispatch('fetch', { url: 'https://cdn.example.com/x.js', method: 'GET', mode: 'cors' })).toBeUndefined();
    expect(await w.dispatch('fetch', { url: `${w.scope}index.html`, method: 'POST', mode: 'cors' })).toBeUndefined();
  });
});

/** Runs the plugin on a bundle with a single index.html and returns the emitted worker. */
function emit(code: string) {
  const emitFile = vi.fn<(file: { fileName: string; source: string }) => void>();
  const plugin = serviceWorkerPlugin({ appId: 'app', publicDir: resolve(__dirname, '../public') });
  const hook = plugin.generateBundle as (this: unknown, o: unknown, b: unknown) => void;
  hook.call({ emitFile }, {}, { 'index.html': { type: 'asset', fileName: 'index.html', source: code } });
  return emitFile.mock.calls[0]![0];
}

describe('serviceWorkerPlugin', () => {
  it('emits sw.js with a version that changes with the build output', () => {
    const a = emit('<p>a</p>');
    expect(a.fileName).toBe('sw.js');
    expect(a.source).toContain('"manifest.webmanifest"');
    expect(a.source).toContain('"index.html"');
    expect(emit('<p>b</p>').source).not.toBe(a.source);
    expect(emit('<p>a</p>').source).toBe(a.source);
  });
});

describe('web app manifest', () => {
  const html = readFileSync(resolve(__dirname, '../index.html'), 'utf8');
  const manifest = JSON.parse(readFileSync(resolve(__dirname, '../public/manifest.webmanifest'), 'utf8'));

  it('is linked from the page', () => {
    expect(html).toContain('rel="manifest"');
    expect(html).toContain('name="theme-color"');
  });

  it('describes an installable standalone app with existing icons', () => {
    expect(manifest).toMatchObject({ name: 'Voice Modem', start_url: './', scope: './', display: 'standalone', lang: 'en' });
    const sizes = manifest.icons.map((i: { sizes: string }) => i.sizes);
    expect(sizes).toEqual(expect.arrayContaining(['192x192', '512x512']));
    for (const icon of manifest.icons)
      expect({ src: icon.src, exists: existsSync(resolve(__dirname, '../public', icon.src)) }).toEqual({ src: icon.src, exists: true });
  });
});

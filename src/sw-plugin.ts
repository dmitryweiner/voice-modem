import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { join, relative, sep } from 'node:path';
import type { Plugin } from 'vite';

const WORKER_FILE = 'sw.js';

/** Files the worker downloads on install, relative to the worker; './' is the start page. */
export function precacheFiles(bundleFiles: string[], publicFiles: string[]): string[] {
  const files = [...bundleFiles, ...publicFiles].filter((f) => f !== WORKER_FILE && !f.endsWith('.map'));
  return ['./', ...[...new Set(files)].toSorted()];
}

/**
 * Offline-first worker: everything is precached on install and served from the cache.
 * A new build gets a new cache name and takes over at once (skipWaiting): on phones pages are
 * suspended rather than closed, so a waiting worker would never activate. Old caches are removed,
 * and the page reloads itself on `controllerchange` (see pwa.ts) before it needs an old asset.
 */
export function serviceWorkerSource(appId: string, version: string, files: string[]): string {
  return `const PREFIX = ${JSON.stringify(`${appId}-`)};
const CACHE = PREFIX + ${JSON.stringify(version)};
const FILES = ${JSON.stringify(files)};

self.addEventListener('install', (event) => {
  event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(FILES)).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k.startsWith(PREFIX) && k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (event) => {
  const request = event.request;
  if (request.method !== 'GET' || new URL(request.url).origin !== self.location.origin) return;
  // ignoreVary: assets are requested with an Origin header (crossorigin) that the precached copies lack.
  const cached = request.mode === 'navigate'
    ? caches.match(request, { ignoreSearch: true, ignoreVary: true }).then((r) => r || caches.match('./', { ignoreVary: true }))
    : caches.match(request, { ignoreVary: true });
  event.respondWith(cached.then((r) => r || fetch(request)));
});
`;
}

function listFiles(dir: string): string[] {
  try {
    return readdirSync(dir, { recursive: true, withFileTypes: true })
      .filter((e) => e.isFile())
      .map((e) => relative(dir, join(e.parentPath, e.name)).split(sep).join('/'));
  } catch {
    return [];
  }
}

/** Emits sw.js listing the build output and the public folder. */
export function serviceWorkerPlugin({ appId, publicDir }: { appId: string; publicDir: string }): Plugin {
  return {
    name: 'app-service-worker',
    apply: 'build',
    enforce: 'post',
    generateBundle(_options, bundle) {
      const publicFiles = listFiles(publicDir);
      const hash = createHash('sha256');
      for (const name of Object.keys(bundle).toSorted()) {
        const item = bundle[name]!;
        hash.update(name).update(item.type === 'chunk' ? item.code : item.source);
      }
      for (const name of publicFiles) hash.update(name).update(readFileSync(join(publicDir, name)));
      const version = hash.digest('hex').slice(0, 12);
      this.emitFile({
        type: 'asset',
        fileName: WORKER_FILE,
        source: serviceWorkerSource(appId, version, precacheFiles(Object.keys(bundle), publicFiles)),
      });
    },
  };
}

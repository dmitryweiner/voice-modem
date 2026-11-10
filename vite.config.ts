import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';
import { serviceWorkerPlugin } from './src/sw-plugin.ts';

/** The package name is the app's id: the storage key and the service worker's cache prefix. */
function appId(): string {
  const pkg: unknown = JSON.parse(readFileSync(resolve(import.meta.dirname, 'package.json'), 'utf8'));
  if (typeof pkg === 'object' && pkg !== null && 'name' in pkg && typeof pkg.name === 'string') return pkg.name;
  throw new Error('package.json has no name');
}

const APP_ID = appId();

export default defineConfig({
  // Relative base lets the build work both on GitHub Pages (/<repo>/) and from any static folder.
  base: './',
  define: { __APP_ID__: JSON.stringify(APP_ID) },
  plugins: [react(), serviceWorkerPlugin({ appId: APP_ID, publicDir: resolve(import.meta.dirname, 'public') })],
  // The WASM glue finds its .wasm via import.meta.url, which an iife worker lacks.
  worker: { format: 'es' },
  test: {
    environment: 'node',
    include: ['src/**/*.test.{ts,tsx}'],
  },
});

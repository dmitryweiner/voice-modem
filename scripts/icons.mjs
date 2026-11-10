// Renders the PNG icons of the PWA from public/icon.svg with Playwright's
// chromium, so the SVG stays the only source and no image tool is needed.
//
//   npm run icons
//
// Writes public/icon-192.png, icon-512.png, apple-touch-icon.png (180, on an
// opaque background: iOS fills transparency with black) and
// icon-maskable-512.png (the icon shrunk into the 80 % safe zone on the
// theme colour, so launchers can crop it to any shape).

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { chromium } from 'playwright';

const root = resolve(import.meta.dirname, '..');
const svg = readFileSync(resolve(root, 'public/icon.svg'), 'utf8');
const manifest = JSON.parse(readFileSync(resolve(root, 'public/manifest.webmanifest'), 'utf8'));
const background = manifest.theme_color;

const icons = [
  { file: 'icon-192.png', size: 192, scale: 1, fill: 'transparent' },
  { file: 'icon-512.png', size: 512, scale: 1, fill: 'transparent' },
  { file: 'apple-touch-icon.png', size: 180, scale: 1, fill: background },
  { file: 'icon-maskable-512.png', size: 512, scale: 0.8, fill: background },
];

const browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_BROWSER || undefined });
try {
  for (const { file, size, scale, fill } of icons) {
    const page = await browser.newPage({ viewport: { width: size, height: size } });
    const inner = Math.round(size * scale);
    await page.setContent(
      `<body style="margin:0;display:grid;place-items:center;width:${size}px;height:${size}px;background:${fill}">` +
        `<div style="width:${inner}px;height:${inner}px">${svg.replace('<svg ', '<svg width="100%" height="100%" ')}</div></body>`,
    );
    await page.screenshot({ path: resolve(root, 'public', file), omitBackground: fill === 'transparent' });
    await page.close();
    console.log(`public/${file} ${size}×${size}`);
  }
} finally {
  await browser.close();
}

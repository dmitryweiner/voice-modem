// Browser smoke of the real app: language detection and memory, the modem
// core loaded through the worker, no horizontal scroll on a phone, no console
// errors. With --preview it runs against a fresh production build and also
// checks that the service worker makes the app (WASM core included) work
// offline and that a new deploy reaches a page that is already open (phones
// suspend pages, never close them).
//
//   npm run smoke               # dev server
//   npm run smoke -- --preview  # production build + offline check
//   npm run smoke -- --shots    # also saves shots/<viewport>-<locale>.png
//
// Prints one line per step; exits 1 on the first failed step or on any
// console error. New checks are new steps here, not new scripts.

import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import { check, launch, parseArgs, VIEWPORTS } from './lib/harness.mjs';

const flags = parseArgs(process.argv.slice(2));
const preview = flags.has('preview');
const shots = flags.has('shots');
const app = await launch({ preview });

async function step(name, fn) {
  app.label(name);
  await fn();
  console.log(`ok  ${name}`);
}

/** Waits for the status line that only the WASM core, asked through the worker, can produce. */
async function coreReady(page) {
  const status = page.locator('#core[data-status="ready"]');
  await status.waitFor({ timeout: 10_000 }).catch(() => check(false, 'the modem core did not load'));
  check(
    /^Modem core \d+\.\d+\.\d+ is ready\.$|^Ядро модема \d+\.\d+\.\d+ готово\.$/.test(await status.textContent()),
    'core status text is wrong',
  );
}

try {
  await step('English by default, the modem core loads in its worker', async () => {
    const { page, context } = await app.open({ locale: 'de-DE' });
    check((await page.locator('h1').textContent()) === 'Voice Modem', 'heading is not English');
    await coreReady(page);
    await context.close();
  });

  await step('browser language is detected, the choice survives a reload', async () => {
    const { page, context } = await app.open({ locale: 'ru-RU' });
    check((await page.locator('html').getAttribute('lang')) === 'ru', 'ru-RU browser did not get Russian');
    await page.selectOption('#locale', 'en');
    await page.reload();
    check((await page.locator('html').getAttribute('lang')) === 'en', 'the switcher choice was lost on reload');
    await context.close();
  });

  for (const [name, viewport] of Object.entries(VIEWPORTS)) {
    for (const locale of ['en-US', 'ru-RU']) {
      await step(`no horizontal scroll on ${name} (${viewport.width}px, ${locale})`, async () => {
        const { page, context } = await app.open({ viewport, locale });
        await coreReady(page);
        const width = await page.evaluate(() => document.documentElement.scrollWidth);
        check(width <= viewport.width, `page is ${width}px wide`);
        if (shots) await page.screenshot({ path: `shots/${name}-${locale}.png`, fullPage: true });
        await context.close();
      });
    }
  }

  if (preview) {
    await step('works offline after the first visit', async () => {
      const { page, context } = await app.open();
      await page.evaluate(() => navigator.serviceWorker.ready);
      await context.setOffline(true);
      await page.reload();
      check((await page.locator('h1').textContent()) === 'Voice Modem', 'the app did not load offline');
      await coreReady(page);
      await context.close();
    });

    await step('a new deploy reaches an already open page', async () => {
      const { page, context } = await app.open();
      await page.evaluate(() => navigator.serviceWorker.ready);
      await page.evaluate(() => (window.oldPage = true));
      // A changed sw.js is what a deploy looks like to the browser.
      const worker = 'dist/sw.js';
      const original = readFileSync(worker, 'utf8');
      appendFileSync(worker, `\n// deploy ${Date.now()}\n`);
      try {
        const reloaded = page.waitForEvent('load', { timeout: 10_000 }).catch(() => null);
        // What a phone does when the user returns to a suspended page.
        await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
        check((await reloaded) !== null, 'the open page never switched to the new version');
        check((await page.evaluate(() => window.oldPage)) === undefined, 'the page was not reloaded');
      } finally {
        writeFileSync(worker, original);
      }
      await context.close();
    });
  }

  check(app.errors.length === 0, `console errors:\n${app.errors.join('\n')}`);
  console.log(`smoke passed (${preview ? 'preview' : 'dev'})`);
} catch (error) {
  console.error(`FAIL ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
} finally {
  await app.close();
}

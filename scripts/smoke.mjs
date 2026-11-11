// Browser smoke of the real app: language detection and memory, the modem
// core loaded through the worker, a message received from the microphone and
// one transmitted, no horizontal scroll on a phone, no console errors. With
// --preview it runs against a fresh production build and also checks that
// the service worker makes the app (WASM core included) work offline and
// that a new deploy reaches a page that is already open (phones suspend
// pages, never close them).
//
// The microphone is a WAV file made by modem-cli, which Chromium plays in a
// loop as its fake capture device: the whole chain from the AudioWorklet to
// the screen runs as with a real sound card. Needs a Rust toolchain.
//
//   npm run smoke               # dev server
//   npm run smoke -- --preview  # production build + offline check
//   npm run smoke -- --shots    # also saves shots/<viewport>-<locale>.png
//
// Prints one line per step; exits 1 on the first failed step or on any
// console error. New checks are new steps here, not new scripts.

import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { appendFileSync, readFileSync, writeFileSync } from 'node:fs';
import { check, launch, parseArgs, VIEWPORTS } from './lib/harness.mjs';

const flags = parseArgs(process.argv.slice(2));
const preview = flags.has('preview');
const shots = flags.has('shots');

// The recording ends with silence long enough for the page to get a word in: it
// transmits only when the channel is quiet.
const HEARD = 'Hello from the fake microphone';
const temp = mkdtempSync(join(tmpdir(), 'voice-modem-smoke-'));
const microphone = join(temp, 'microphone.wav');
execFileSync('cargo', ['run', '--release', '--quiet', '-p', 'modem-cli', '--', 'encode', HEARD, '-o', microphone, '--pause', '6'], {
  stdio: ['ignore', 'ignore', 'inherit'],
});

const app = await launch({
  preview,
  audio: true,
  browserArgs: ['--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream', `--use-file-for-fake-audio-capture=${microphone}`],
});

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

  await step('a message played into the microphone appears on the page', async () => {
    const { page, context } = await app.open({ viewport: VIEWPORTS.phone });
    await coreReady(page);
    await page.click('#mic');
    await page
      .locator('#state')
      .waitFor({ timeout: 10_000 })
      .catch(() => check(false, 'the microphone did not turn on'));
    // the recording loops: the first pass may have started before the worklet was ready
    const received = page.locator('#feed .received .text').first();
    await received.waitFor({ timeout: 30_000 }).catch(() => check(false, 'nothing was received'));
    check((await received.textContent()) === HEARD, `received ${JSON.stringify(await received.textContent())}`);
    const bars = await page
      .locator('#spectrum > i')
      .evaluateAll((list) => list.filter((bar) => bar.getBoundingClientRect().height > 5).length);
    check(bars > 0, 'the spectrum shows nothing');
    // the app fits the screen: the messages scroll inside their box, the page does not
    const height = await page.evaluate(() => document.documentElement.scrollHeight);
    check(height <= VIEWPORTS.phone.height, `the page is ${height}px tall on a phone`);
    const box = await page.locator('.feed-box').boundingBox();
    check(box !== null && box.height >= 150, `the message box is ${box?.height}px tall`);
    if (shots) await page.screenshot({ path: 'shots/phone-receiving.png', fullPage: true });
    await context.close();
  });

  await step('a text is transmitted, repeated when nobody acknowledges it, and cancelled', async () => {
    const { page, context } = await app.open({ viewport: VIEWPORTS.phone });
    await coreReady(page);
    await page.click('#mic');
    await page.fill('#draft', 'ok');
    await page.click('#send');
    check((await page.locator('#feed .sent .text').textContent()) === 'ok', 'the sent text is not in the feed');
    check((await page.locator('#feed .sent small').textContent()) === 'Sending…', 'the text is not marked as being sent');
    // it waits for the recording to fall silent, then plays 2.8 s of sound
    await page
      .locator('#state.transmitting')
      .waitFor({ timeout: 20_000 })
      .catch(() => check(false, 'never started transmitting'));
    await page
      .locator('#state:not(.transmitting)')
      .waitFor({ timeout: 10_000 })
      .catch(() => check(false, 'the transmission never ended'));
    // the fake microphone does not answer: the link layer tries again
    const repeated = page.locator('#feed .sent small', { hasText: 'attempt 2' });
    await repeated.waitFor({ timeout: 20_000 }).catch(() => check(false, 'the text was not repeated'));
    // taken back, it stops there
    await page.click('#feed .cancel');
    const cancelled = page.locator('#feed .sent small', { hasText: 'Cancelled' });
    await cancelled.waitFor({ timeout: 5000 }).catch(() => check(false, 'the text was not cancelled'));
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
  rmSync(temp, { recursive: true });
}

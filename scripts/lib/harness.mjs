// Shared harness of the browser scripts: flag parsing, starting a vite
// server, opening pages that collect console errors. Repository rule: extend
// these instead of writing throw-away scripts (AGENTS.md).

import { spawn } from 'node:child_process';
import { chromium } from 'playwright';

/**
 * Ports of this project's own, with --strictPort: another project's server
 * on Vite's default 5173 is never tested by mistake. Pick new ones per project.
 */
const DEV_PORT = 5394;
const PREVIEW_PORT = 4394;

/** Phone and desktop viewports every layout check runs at. */
export const VIEWPORTS = {
  phone: { width: 390, height: 844 },
  desktop: { width: 1280, height: 860 },
};

/** Parses `--flag value` / `--flag`; valueFlags lists the flags that take a value. */
export function parseArgs(argv, valueFlags = []) {
  const values = new Set(valueFlags);
  const flags = new Map();
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (!arg.startsWith('--')) continue;
    const name = arg.slice(2);
    flags.set(name, values.has(name) ? argv[++i] : 'true');
  }
  return flags;
}

async function serverUp(base) {
  try {
    return (await fetch(base)).ok;
  } catch {
    return false;
  }
}

function run(cmd, args) {
  return new Promise((resolve, reject) => {
    const proc = spawn(cmd, args, { stdio: 'inherit' });
    proc.on('exit', (code) => (code === 0 ? resolve() : reject(new Error(`${cmd} ${args.join(' ')} → ${code}`))));
  });
}

/** Starts a dev / preview server unless one already listens on the port. */
export async function ensureServer(preview) {
  const port = preview ? PREVIEW_PORT : DEV_PORT;
  const base = `http://localhost:${port}/`;
  if (await serverUp(base)) return { base, proc: null };
  // vite preview serves the last build; without a rebuild the smoke would
  // test old code and fail somewhere puzzling. `npm run build` also rebuilds
  // the WASM core, which the dev server expects to be there already.
  if (preview) await run('npm', ['run', 'build']);
  else await run('npm', ['run', 'wasm']);
  const proc = spawn('npx', ['vite', ...(preview ? ['preview'] : []), '--port', String(port), '--strictPort'], { stdio: 'ignore' });
  for (let i = 0; i < 40 && !(await serverUp(base)); i++) await new Promise((r) => setTimeout(r, 500));
  if (!(await serverUp(base))) {
    proc.kill();
    throw new Error(`server did not start on :${port}`);
  }
  return { base, proc };
}

/**
 * On a Mac whose display sleeps, Chromium's audio stops: an AudioContext says
 * "running" while its clock stands still, and a fake microphone is silent.
 * `caffeinate -u` declares the user active (and wakes the display) for as long
 * as this process lives.
 */
function keepAwake() {
  if (process.platform !== 'darwin') return null;
  return spawn('caffeinate', ['-u', '-d', '-w', String(process.pid)], { stdio: 'ignore' }).on('error', () => {});
}

/**
 * Starts the server and a browser (with extra command-line `browserArgs`;
 * `audio: true` when the page will use sound).
 * `open({ locale, viewport })` makes a fresh
 * context (empty storage, the given browser language) and loads the app.
 * Console errors and page errors of every page land in `errors`, tagged with
 * the step set by `label(text)`.
 */
export async function launch({ preview = false, browserArgs = [], audio = false } = {}) {
  const awake = audio ? keepAwake() : null;
  const { base, proc } = await ensureServer(preview);
  // In a cloud container Playwright may miss its own chromium while another
  // is installed: PLAYWRIGHT_BROWSER gives its path.
  const browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_BROWSER || undefined, args: browserArgs });
  const errors = [];
  let step = 'start';

  async function open({ locale = 'en-US', viewport = VIEWPORTS.desktop } = {}) {
    const context = await browser.newContext({ locale, viewport });
    const page = await context.newPage();
    page.on('pageerror', (e) => errors.push(`[${step}] ${String(e)}`));
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(`[${step}] ${m.text()}`);
    });
    await page.goto(base, { waitUntil: 'load' });
    return { page, context };
  }

  return {
    base,
    errors,
    open,
    label: (text) => {
      step = text;
    },
    async close() {
      await browser.close();
      proc?.kill();
      awake?.kill();
    },
  };
}

/** Throws with a readable message unless the condition holds. */
export function check(condition, message) {
  if (!condition) throw new Error(message);
}

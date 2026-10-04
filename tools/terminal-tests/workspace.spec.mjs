// Phase 12 — browser-terminal acceptance fixtures for the local fm binary.
//
// server.mjs owns the PTY child; this spec owns the fixture lifecycle: temp
// workspace + isolated HOME/XDG dirs, loopback fixtureUrl from the server's
// READY line, xterm buffer assertions, and bounded cleanup.

import { test, expect } from '@playwright/test';
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import url from 'node:url';

const HERE = path.dirname(url.fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, '..', '..');
const FM_BIN = process.env.FM_BIN ?? path.join(REPO, 'target/release/fm');
if (!fs.existsSync(FM_BIN)) {
  throw new Error(`fm binary missing at ${FM_BIN} — run cargo build --release`);
}

let fixtureRoot = '';
let fixtureUrl = '';
let server = null;

/** Serialized xterm screen: every buffer line, trailing blanks trimmed. */
async function terminalText(page) {
  return page.evaluate(() => {
    const b = window.term.buffer.active;
    const lines = [];
    for (let i = 0; i < b.length; i += 1) {
      lines.push(b.getLine(i)?.translateToString(true) ?? '');
    }
    return lines.join('\n');
  });
}

/** Read a file strictly inside the generated fixture root. */
async function readFixtureFile(name) {
  const resolved = path.resolve(fixtureRoot, name);
  if (resolved !== fixtureRoot && !resolved.startsWith(fixtureRoot + path.sep)) {
    throw new Error(`fixture read escapes root: ${name}`);
  }
  return fs.promises.readFile(resolved, 'utf8');
}

function fmProcesses() {
  try {
    const out = execFileSync('pgrep', ['-f', FM_BIN], { encoding: 'utf8' }).trim();
    return out ? out.split('\n') : [];
  } catch {
    return [];
  }
}

test.beforeAll(async () => {
  fixtureRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'fm-fixture-'));
  fs.writeFileSync(path.join(fixtureRoot, 'hello.txt'), 'hello fm\n');
  fs.writeFileSync(path.join(fixtureRoot, 'notes.md'), '# notes\n');
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'fm-home-'));
  const xdgConfig = path.join(home, '.config');
  const xdgData = path.join(home, '.local', 'share');
  fs.mkdirSync(xdgConfig, { recursive: true });
  fs.mkdirSync(xdgData, { recursive: true });

  server = spawn('node', [path.join(HERE, 'server.mjs')], {
    env: {
      ...process.env,
      FM_BIN,
      FIXTURE_ROOT: fixtureRoot,
      HOME: home,
      XDG_CONFIG_HOME: xdgConfig,
      XDG_DATA_HOME: xdgData,
    },
    stdio: ['ignore', 'pipe', 'inherit'],
  });

  fixtureUrl = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('server ready timeout')), 10_000);
    let buf = '';
    server.stdout.on('data', (d) => {
      buf += d.toString();
      const m = buf.match(/READY (http:\/\/127\.0\.0\.1:\d+)/);
      if (m) {
        clearTimeout(timer);
        resolve(m[1]);
      }
    });
    server.on('exit', () => reject(new Error('server exited before ready')));
  });
});

test.afterAll(async () => {
  if (!server || server.exitCode !== null) return;
  const exited = new Promise((resolve) => server.once('exit', resolve));
  server.kill('SIGTERM');
  await Promise.race([
    exited,
    new Promise((_, reject) =>
      setTimeout(() => reject(new Error('server shutdown exceeded bound')), 5_000)
    ),
  ]);
});

test('web profile has a usable command route', async ({ page }) => {
  await page.goto(fixtureUrl);
  await page.getByRole('button', { name: 'Focus terminal' }).click();
  await expect
    .poll(async () => await terminalText(page), { timeout: 15_000 })
    .toContain('Commands');
  // The fixture tree is the fm root, not the user's home.
  await expect
    .poll(async () => await terminalText(page))
    .toContain('hello.txt');
});

test('clean quit exits the child; server teardown reaps it', async ({ page }) => {
  await page.goto(fixtureUrl);
  await page.getByRole('button', { name: 'Focus terminal' }).click();
  await expect
    .poll(async () => await terminalText(page), { timeout: 15_000 })
    .toContain('Commands');
  expect(fmProcesses().length).toBeGreaterThan(0);

  // 'q' (Quit) exits fm cleanly → the server reports child exit to the page.
  await page.keyboard.type('q');
  await expect
    .poll(async () => await terminalText(page), { timeout: 10_000 })
    .toContain('child exited');
  await expect.poll(() => fmProcesses().length).toBe(0);

  const exited = new Promise((resolve) => server.once('exit', () => resolve(true)));
  server.kill('SIGTERM');
  await expect(
    Promise.race([
      exited,
      new Promise((_, reject) =>
        setTimeout(() => reject(new Error('server shutdown exceeded bound')), 5_000)
      ),
    ])
  ).resolves.toBeTruthy();
  expect(fmProcesses().length).toBe(0);
});

export { terminalText, readFixtureFile };

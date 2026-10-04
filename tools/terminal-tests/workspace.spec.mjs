// Phase 12 — browser-terminal acceptance matrix for the local fm binary.
//
// server.mjs owns the PTY child; each test owns an isolated fixture: temp
// workspace + isolated HOME/XDG dirs + its own server process. fixtureUrl
// comes from the server's READY line; xterm buffer assertions via
// terminalText; fixture reads confined to the generated root via
// readFixtureFile.

import { test, expect } from '@playwright/test';
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import url from 'node:url';

const HERE = path.dirname(url.fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, '..', '..');
const FM_BIN = process.env.FM_BIN ?? path.join(REPO, 'target', 'release', 'fm');
if (!fs.existsSync(FM_BIN)) {
  throw new Error(`fm binary missing at ${FM_BIN} — run cargo build --release`);
}

const servers = [];

test.afterEach(async () => {
  while (servers.length) {
    const server = servers.pop();
    if (server.exitCode !== null) continue;
    const exited = new Promise((resolve) => server.once('exit', () => resolve(true)));
    server.kill('SIGTERM');
    await Promise.race([
      exited,
      new Promise((_, reject) =>
        setTimeout(() => reject(new Error('server shutdown exceeded bound')), 5_000)
      ),
    ]);
  }
});

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

/**
 * Spawn a fixture: temp workspace (seeded with `files`), isolated HOME/XDG,
 * and a server bound to a loopback ephemeral port.
 * @returns {{fixtureUrl: string, fixtureRoot: string, server: object}}
 */
async function launchFixture({ files = {}, args = '', cols = 80, rows = 24 } = {}) {
  const fixtureRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'fm-fixture-'));
  for (const [name, content] of Object.entries(files)) {
    fs.writeFileSync(path.join(fixtureRoot, name), content);
  }
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'fm-home-'));
  const xdgConfig = path.join(home, '.config');
  const xdgData = path.join(home, '.local', 'share');
  fs.mkdirSync(xdgConfig, { recursive: true });
  fs.mkdirSync(xdgData, { recursive: true });

  const server = spawn('node', [path.join(HERE, 'server.mjs')], {
    env: {
      ...process.env,
      FM_BIN,
      FIXTURE_ROOT: fixtureRoot,
      HOME: home,
      XDG_CONFIG_HOME: xdgConfig,
      XDG_DATA_HOME: xdgData,
      FM_ARGS: args,
      FM_COLS: String(cols),
      FM_ROWS: String(rows),
    },
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  servers.push(server);

  const fixtureUrl = await new Promise((resolve, reject) => {
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
  return { fixtureUrl, fixtureRoot, server };
}

/** Read a file strictly inside the generated fixture root. */
async function readFixtureFile(fixtureRoot, name) {
  const resolved = path.resolve(fixtureRoot, name);
  if (resolved !== fixtureRoot && !resolved.startsWith(fixtureRoot + path.sep)) {
    throw new Error(`fixture read escapes root: ${name}`);
  }
  return fs.promises.readFile(resolved, 'utf8');
}

function fmProcesses() {
  try {
    const out = execFileSync('pgrep', ['-f', FM_BIN], { encoding: 'utf8' }).trim();
    const pids = out ? out.split('\n') : [];
    // `pgrep -f` matches ANY process whose cmdline mentions the path —
    // strace wrappers, `bash -c` scripts embedding the path, an editor's
    // open command. Filter to processes whose executable actually IS the
    // fm binary; zombies have no /proc/<pid>/exe link and drop out too.
    const target = fs.realpathSync(FM_BIN);
    return pids.filter((pid) => {
      try {
        return fs.realpathSync(`/proc/${pid}/exe`) === target;
      } catch {
        return false; // dead, reaped, or not our binary
      }
    });
  } catch {
    return [];
  }
}

async function waitForChrome(page, extra = 'Commands', timeout = 15_000) {
  await expect.poll(async () => await terminalText(page), { timeout }).toContain(extra);
}

/** Type a key chord through xterm (exact bytes the browser sends). */
async function send(page, text) {
  await page.evaluate((d) => window.term.input(d, true), text);
}

test('web profile has a usable command route', async ({ page }) => {
  const { fixtureUrl } = await launchFixture({ files: { 'hello.txt': 'hi\n' } });
  await page.goto(fixtureUrl);
  await page.getByRole('button', { name: 'Focus terminal' }).click();
  await waitForChrome(page);
  await expect
    .poll(async () => await terminalText(page))
    .toContain('hello.txt');
});

test('clean quit exits the child; teardown leaves no processes', async ({ page }) => {
  const { fixtureUrl, server } = await launchFixture({ files: { 'a.txt': 'a\n' } });
  await page.goto(fixtureUrl);
  await page.getByRole('button', { name: 'Focus terminal' }).click();
  await waitForChrome(page);
  expect(fmProcesses().length).toBeGreaterThan(0);

  // 'q' (Quit) exits fm cleanly → the server reports child exit to the page.
  await send(page, 'q');
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

test('browser-reserved Ctrl+P stays excluded; Alt+G o opens Quick Open', async ({ page }) => {
  const { fixtureUrl } = await launchFixture({ files: { 'a.txt': 'a\n' } });
  await page.goto(fixtureUrl);
  await page.getByRole('button', { name: 'Focus terminal' }).click();
  await waitForChrome(page);

  // Ctrl+P is browser-reserved in the web profile: it must not reach fm.
  await send(page, '\x10');
  const short = await terminalText(page);
  expect(short).not.toContain('Quick Open');

  // The explicit non-reserved route works: Alt+G o → Quick Open overlay.
  await send(page, '\x1bgo');
  await expect
    .poll(async () => await terminalText(page), { timeout: 10_000 })
    .toContain('Quick Open');
  await send(page, '\x1b'); // Esc closes the overlay
  await waitForChrome(page);
});

test('full workflow: two files, edit, terminal, paste, save exact bytes', async ({ page }) => {
  const { fixtureUrl, fixtureRoot } = await launchFixture({
    files: { 'a.py': 'print("a")\n', 'config.yaml': '' },
  });
  await page.goto(fixtureUrl);
  await page.getByRole('button', { name: 'Focus terminal' }).click();
  await waitForChrome(page);

  // Open a.py via Quick Open (Alt+G o), edit it — dirty state retained.
  await send(page, '\x1bgo');
  await waitForChrome(page, 'Quick Open');
  await send(page, 'a.py\r');
  await waitForChrome(page, 'a.py [EDIT]');
  await send(page, '#edited\r');
  await expect
    .poll(async () => await terminalText(page))
    .toContain('●'); // dirty marker (icons enabled)

  // Open the second document; the first stays dirty in the background.
  await send(page, '\x1bgo');
  await waitForChrome(page, 'Quick Open');
  await send(page, 'config.yaml\r');
  await waitForChrome(page, 'config.yaml [EDIT]');

  // Run a shell command in the embedded terminal, then return to the editor.
  await send(page, '\x1bgt'); // Alt+G t toggles the terminal pane
  await waitForChrome(page, 'Terminal');
  await send(page, 'echo shell-ok\r');
  await expect
    .poll(async () => await terminalText(page), { timeout: 10_000 })
    .toContain('shell-ok');
  await send(page, '\x1bg2'); // Alt+G 2 refocuses the editor

  // Paste YAML through the real browser paste path (bracketed paste into fm).
  await page.evaluate(() => window.term.paste('training:\n  lr: 0.001\n'));
  await send(page, '\x13'); // Ctrl+S saves
  await expect
    .poll(async () => await terminalText(page))
    .toContain('config.yaml [EDIT] ');
  expect(await readFixtureFile(fixtureRoot, 'config.yaml')).toBe('training:\n  lr: 0.001\n');

  // Switching back to a.py shows the retained dirty buffer, not disk bytes.
  await send(page, '\x1bgo');
  await waitForChrome(page, 'Quick Open');
  await send(page, 'a.py\r');
  await expect
    .poll(async () => await terminalText(page))
    .toContain('#edited');
});

test('pty resize through the page reflows fm without losing chrome', async ({ page }) => {
  const { fixtureUrl } = await launchFixture({ files: { 'a.txt': 'a\n' }, cols: 80, rows: 24 });
  await page.goto(fixtureUrl);
  await page.getByRole('button', { name: 'Focus terminal' }).click();
  await waitForChrome(page);
  const initialCols = await page.evaluate(() => window.term.cols);
  expect(initialCols).toBeGreaterThan(0);

  // Enlarging the viewport resizes xterm → ws resize → PTY resize → fm redraw.
  await page.setViewportSize({ width: 1600, height: 900 });
  await expect
    .poll(async () => await page.evaluate(() => window.term.cols))
    .toBeGreaterThan(initialCols);
  const wideCols = await page.evaluate(() => window.term.cols);
  await waitForChrome(page);

  // Shrinking reflows too — chrome survives both directions.
  await page.setViewportSize({ width: 640, height: 480 });
  await expect.poll(async () => await page.evaluate(() => window.term.cols)).toBeLessThan(wideCols);
  await waitForChrome(page);
  await send(page, 'q');
  await expect
    .poll(async () => await terminalText(page), { timeout: 10_000 })
    .toContain('child exited');
});

test('browser copy fallback works without fm mouse protocol', async ({ page }) => {
  const { fixtureUrl } = await launchFixture({
    files: { 'a.txt': 'a\n' },
    args: '--no-mouse',
  });
  await page.goto(fixtureUrl);
  await page.getByRole('button', { name: 'Focus terminal' }).click();
  await waitForChrome(page);

  // With --no-mouse, selection is pure xterm (the browser-native fallback):
  // select-all then read the selection as the copy payload.
  const selection = await page.evaluate(() => {
    window.term.selectAll();
    return window.term.getSelection();
  });
  expect(selection).toContain('Commands');
});

test('feature-off flags still render a usable workspace', async ({ page }) => {
  const { fixtureUrl } = await launchFixture({
    files: { 'a.txt': 'a\n' },
    args: '--no-mouse --no-icons --no-watcher --no-terminal --no-git',
  });
  await page.goto(fixtureUrl);
  await page.getByRole('button', { name: 'Focus terminal' }).click();
  await waitForChrome(page);
  await send(page, 'q');
  await expect
    .poll(async () => await terminalText(page), { timeout: 10_000 })
    .toContain('child exited');
});

export { terminalText, readFixtureFile };

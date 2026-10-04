// Phase 12 Task 1 — browser-terminal fixture server.
//
// Spawns the local `fm` binary on a real PTY (node-pty) and bridges it to a
// browser xterm.js page over a loopback-only WebSocket. No auth, no external
// service, ephemeral port. One page ↔ one child process; closing the page or
// exiting the server kills and reaps the child.
//
// Env:
//   FM_BIN           absolute path to the fm binary (required)
//   FIXTURE_ROOT     workspace root shown in the file manager (required)
//   XDG_CONFIG_HOME  isolated config dir (required — never the user's real one)
//   XDG_DATA_HOME    isolated state/recovery dir (required)
//   HOME             isolated home (required)
//   FM_ARGS          extra CLI flags, space separated (optional)
//   FM_COLS/FM_ROWS  initial PTY size (default 80x24)
//
// Prints `READY http://127.0.0.1:<port>` once listening; that line is the
// fixtureUrl contract the Playwright spec waits for.

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import url from 'node:url';
import { spawn } from 'node-pty';
import { WebSocketServer } from 'ws';

const HERE = path.dirname(url.fileURLToPath(import.meta.url));

const required = ['FM_BIN', 'FIXTURE_ROOT', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'HOME'];
for (const key of required) {
  if (!process.env[key]) {
    console.error(`server: missing required env ${key}`);
    process.exit(64);
  }
}

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.map': 'application/json',
};

// Static map: page + vendored xterm assets only. Nothing outside this dir and
// node_modules/@xterm is ever served.
const STATIC = new Map([
  ['/', path.join(HERE, 'index.html')],
  ['/xterm.js', path.join(HERE, 'node_modules/@xterm/xterm/lib/xterm.js')],
  ['/xterm.css', path.join(HERE, 'node_modules/@xterm/xterm/css/xterm.css')],
  ['/addon-fit.js', path.join(HERE, 'node_modules/@xterm/addon-fit/lib/addon-fit.js')],
]);

const server = http.createServer((req, res) => {
  const target = STATIC.get(req.url);
  if (!target) {
    res.writeHead(404).end('not found');
    return;
  }
  fs.readFile(target, (err, data) => {
    if (err) {
      res.writeHead(500).end();
      return;
    }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(target)] ?? 'application/octet-stream' });
    res.end(data);
  });
});

const wss = new WebSocketServer({ server, path: '/pty' });

let child = null;
let childExited = false;
const pending = [];

// Late-joining pages replay the bounded tail of raw PTY output — enough for
// xterm to reconstruct the current frame on reconnect.
const REPLAY_MAX = 256 * 1024;
let replay = '';

function ensureChild() {
  if (child && !childExited) return child;
  replay = '';
  const cols = Number(process.env.FM_COLS ?? 80);
  const rows = Number(process.env.FM_ROWS ?? 24);
  const args = [process.env.FIXTURE_ROOT, ...(process.env.FM_ARGS ?? '').split(' ').filter(Boolean)];
  child = spawn(process.env.FM_BIN, args, {
    name: 'xterm-256color',
    cols,
    rows,
    cwd: process.env.FIXTURE_ROOT,
    env: {
      ...process.env,
      TERM: 'xterm-256color',
      // Every state surface is redirected into the fixture dirs.
      HOME: process.env.HOME,
      XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME,
      XDG_DATA_HOME: process.env.XDG_DATA_HOME,
      XDG_CACHE_HOME: process.env.XDG_CACHE_HOME ?? process.env.HOME + '/.cache',
      XDG_STATE_HOME: process.env.XDG_STATE_HOME ?? process.env.HOME + '/.local/state',
    },
  });
  childExited = false;
  child.onData((data) => {
    replay += data;
    if (replay.length > REPLAY_MAX) replay = replay.slice(-REPLAY_MAX);
    for (const ws of pending) {
      if (ws.readyState === ws.OPEN) ws.send(JSON.stringify({ type: 'output', data }));
    }
  });
  child.onExit(({ exitCode }) => {
    childExited = true;
    for (const ws of pending) {
      if (ws.readyState === ws.OPEN) ws.send(JSON.stringify({ type: 'exit', code: exitCode }));
    }
  });
  return child;
}

wss.on('connection', (ws) => {
  pending.push(ws);
  const ptyProc = ensureChild();
  if (replay) ws.send(JSON.stringify({ type: 'output', data: replay }));
  ws.on('message', (raw) => {
    let msg;
    try {
      msg = JSON.parse(raw.toString());
    } catch {
      return;
    }
    if (msg.type === 'input' && typeof msg.data === 'string' && !childExited) {
      ptyProc.write(msg.data);
    } else if (msg.type === 'resize' && !childExited) {
      const cols = Math.max(1, Math.min(500, msg.cols | 0));
      const rows = Math.max(1, Math.min(500, msg.rows | 0));
      ptyProc.resize(cols, rows);
    }
  });
  ws.on('close', () => {
    const i = pending.indexOf(ws);
    if (i >= 0) pending.splice(i, 1);
  });
});

function shutdown() {
  try {
    if (child && !childExited) child.kill();
  } catch {
    /* already gone */
  }
  for (const ws of pending) {
    try { ws.terminate(); } catch { /* ignore */ }
  }
  server.close(() => process.exit(0));
  // Bounded teardown: never linger on a stuck child.
  setTimeout(() => process.exit(0), 1500).unref();
}

process.on('SIGINT', shutdown);
process.on('SIGTERM', shutdown);
process.on('SIGHUP', shutdown);

server.listen(0, '127.0.0.1', () => {
  const { port } = server.address();
  console.log(`READY http://127.0.0.1:${port}`);
});

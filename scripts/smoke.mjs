#!/usr/bin/env node
// Browser smoke test for the built site. It accepts either a ready WebGPU canvas
// or the intentional WebGPU unsupported error, and fails on asset/load/runtime
// errors. This keeps CI useful on machines whose headless Chrome lacks WebGPU.
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { setTimeout as sleep } from 'node:timers/promises';
import { hasFlag, take, takeNumber } from './cli.mjs';
import { startStaticServer } from './serve-static.mjs';

class Cdp {
  constructor(wsUrl) {
    this.ws = new WebSocket(wsUrl);
    this.nextId = 1;
    this.pending = new Map();
    this.console = [];
    this.exceptions = [];
  }

  async open() {
    await new Promise((resolveOpen, rejectOpen) => {
      this.ws.addEventListener('open', resolveOpen, { once: true });
      this.ws.addEventListener('error', rejectOpen, { once: true });
    });
    this.ws.addEventListener('message', (event) => {
      const msg = JSON.parse(event.data);
      if (msg.id && this.pending.has(msg.id)) {
        const { resolvePending, rejectPending } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        msg.error ? rejectPending(new Error(JSON.stringify(msg.error))) : resolvePending(msg.result);
        return;
      }
      if (msg.method === 'Runtime.consoleAPICalled') {
        const text = msg.params.args.map((arg) => arg.value ?? arg.description ?? '').join(' ');
        this.console.push({ type: msg.params.type, text });
      } else if (msg.method === 'Runtime.exceptionThrown') {
        this.exceptions.push(msg.params.exceptionDetails?.text ?? JSON.stringify(msg.params.exceptionDetails));
      } else if (msg.method === 'Log.entryAdded') {
        this.console.push({ type: msg.params.entry.level, text: msg.params.entry.text });
      }
    });
  }

  send(method, params = {}) {
    const id = this.nextId++;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolvePending, rejectPending) => {
      this.pending.set(id, { resolvePending, rejectPending });
    });
  }

  close() {
    this.ws.close();
  }
}

const dir = take('--dir', 'dist');
const targetUrl = take('--url');
const timeoutMs = takeNumber('--timeout', 30_000);
const chromePath = take('--chrome') || process.env.CHROME || findChrome();
const requireBrowser = hasFlag('--require-browser');

if (!chromePath) {
  const message = 'smoke: Chrome/Chromium not found; set CHROME=/path/to/browser';
  if (requireBrowser) throw new Error(message);
  console.log(`${message}; skipping browser smoke`);
  process.exit(0);
}

if (targetUrl) new URL(targetUrl);

let server = null;
let url = targetUrl;
if (!url) {
  ({ server, url } = await startStaticServer({ dir, port: 0, cors: true }));
}

const profileDir = mkdtempSync(join(tmpdir(), 'galacto-smoke-profile-'));
const debugPort = 9400 + Math.floor(Math.random() * 400);
const chrome = spawn(chromePath, [
  `--remote-debugging-port=${debugPort}`,
  `--user-data-dir=${profileDir}`,
  '--headless=new',
  '--no-first-run',
  '--no-default-browser-check',
  '--autoplay-policy=no-user-gesture-required',
  '--enable-unsafe-webgpu',
  '--ignore-gpu-blocklist',
  '--disable-background-timer-throttling',
  '--disable-renderer-backgrounding',
  '--window-size=1280,720',
  url,
], { stdio: ['ignore', 'pipe', 'pipe'] });

try {
  chrome.stderr.on('data', (data) => {
    const text = String(data);
    if (!/DevTools listening|dbus|zygote|sandbox/i.test(text)) process.stderr.write(`[chrome] ${text}`);
  });

  const tabs = await waitForJson(`http://127.0.0.1:${debugPort}/json/list`, timeoutMs);
  const tab = tabs.find((item) => item.url.startsWith(url)) ?? tabs[0];
  if (!tab?.webSocketDebuggerUrl) throw new Error('smoke: could not open Chrome tab');

  const page = new Cdp(tab.webSocketDebuggerUrl);
  await page.open();
  await page.send('Page.enable');
  await page.send('Runtime.enable');
  await page.send('Log.enable');

  const result = await page.send('Runtime.evaluate', {
    expression: `
      new Promise((resolve) => {
        const started = performance.now();
        const poll = () => {
          const loading = document.getElementById("loading");
          const error = document.getElementById("error");
          const errorText = document.getElementById("error-details")?.innerText || "";
          const canvas = document.getElementById("gpu-canvas");
          const ready = Boolean(window.galacto?.isReady?.());
          const loadingDone = !loading || getComputedStyle(loading).display === "none";
          const errorVisible = error && getComputedStyle(error).display !== "none";
          if (ready && loadingDone && canvas?.width > 0 && canvas?.height > 0) {
            resolve({ status: "ready", width: canvas.width, height: canvas.height, title: document.title });
            return;
          }
          if (errorVisible) {
            resolve({ status: "error", errorText, title: document.title });
            return;
          }
          if (performance.now() - started > ${timeoutMs}) {
            resolve({ status: "timeout", loading: loading?.innerText || "", title: document.title });
            return;
          }
          requestAnimationFrame(poll);
        };
        poll();
      })
    `,
    awaitPromise: true,
    returnByValue: true,
  });

  const value = result.result.value;
  const hardErrors = page.exceptions.concat(
    page.console
      .filter((entry) => entry.type === 'error' && !/Failed to initialize application/i.test(entry.text))
      .map((entry) => entry.text),
  );

  if (hardErrors.length) {
    throw new Error(`browser runtime errors:\n${hardErrors.map((item) => `- ${item}`).join('\n')}`);
  }

  if (value.status === 'ready') {
    console.log(`smoke: app ready (${value.width}x${value.height})`);
  } else if (value.status === 'error' && /WebGPU|adapter/i.test(value.errorText)) {
    console.log(`smoke: graceful unsupported-WebGPU state (${value.errorText.trim()})`);
  } else {
    throw new Error(`smoke failed: ${JSON.stringify(value)}`);
  }

  page.close();
} finally {
  chrome.kill('SIGTERM');
  await waitForExit(chrome, 2_000);
  if (server) await new Promise((resolveClose) => server.close(resolveClose));
  rmSync(profileDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}

async function waitForJson(url, timeoutMs) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    try {
      const res = await fetch(url);
      if (res.ok) return await res.json();
    } catch {
      // Chrome may not have opened the debugging endpoint yet.
    }
    await sleep(200);
  }
  throw new Error(`smoke: timed out waiting for ${url}`);
}

function findChrome() {
  const candidates = [
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
    'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe',
    'C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe',
  ];
  return candidates.find((path) => existsSync(path));
}

async function waitForExit(child, timeoutMs) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  await Promise.race([
    new Promise((resolveExit) => child.once('exit', resolveExit)),
    sleep(timeoutMs),
  ]);
}

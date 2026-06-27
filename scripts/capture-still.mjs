#!/usr/bin/env node
// Capture one or more high-resolution stills from the deterministic cinematic
// arrangement, at chosen moments. The same seed + duration always yields the
// same arrangement (see produce.mjs), so this re-renders any frame of a finished
// piece at a far higher resolution than the 4K video master — for a print-quality
// cover, a crisp thumbnail, or a poster.
//
//   npm run capture:still -- --seed 1 --duration 600 --at 120 --width 7680 --height 4320
//   npm run capture:still -- --seed 1 --at 119,120,121          # a small sweep
//
// The render resolution is the viewport size (canvas = 100vw×100vh × dpr, see
// canvas_physical_size in lib.rs), so --width/--height set it directly. WebGPU's
// max 2D texture is 8192, so keep each dimension ≤ 8192 (7680×4320 = 8K UHD).
//
// Timing is wall-clock: the sim advances by elapsed×speed on a fixed-timestep
// accumulator (frame-rate independent up to MAX_FRAME_DT = 0.25s / 4 fps), so a
// still at wall-clock T matches frame T of the video as long as the capture holds
// above ~4 fps — which 8K does comfortably.
//
// KNOWN LIMITATION: an 8K screenshot works for a FRESH/SHORT run, but
// Page.captureScreenshot HANGS when grabbing a large (≥~6-8K) surface late in a long
// session (e.g. the t=120s cover frame) — headless Chrome's GPU-surface readback
// stalls. Confirmed: a fresh t=3s 8K grab succeeds; the t=120s 8K grab times out (also
// via a 4K→8K resize, and with Chrome's background services disabled). 4K is reliable
// at any time. To grab a high-res frame deep into a long arrangement, read it back
// in-page via the canvas captureStream/MediaRecorder path (as capture-canvas-video.mjs
// does for the video) instead of Page.captureScreenshot. Until then, use this for 4K,
// or for short/fresh hi-res grabs.
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";
import { take, takeNumber, hasFlag, run } from "./cli.mjs";
import { ensureChrome, findChrome, getFreePort } from "./preflight.mjs";
import { startStaticServer } from "./serve-static.mjs";

function usage() {
  console.error(`Usage:
  npm run capture:still -- --seed 1 --duration 600 --at 120 --width 7680 --height 4320

Options:
  --seed <N>        deterministic arrangement seed (default 1)
  --duration <sec>  composed-piece duration the arrangement is built for (default 600)
  --at <list>       comma-separated capture times in seconds (default 120)
  --particles <N>   body count (default 32768 — must match the piece to reproduce it)
  --width <px>      render width  (default 7680; ≤ 8192)
  --height <px>     render height (default 4320; ≤ 8192)
  --label <name>    output label (default galacto-still-<seed>)
  --out-dir <dir>   output directory (default renders/stills)
  --port <N>        local static-server port (default 8000)
  --chrome <path>   Chrome/Chromium executable
  --no-build        use an already-built dist/
  --no-headless     show Chrome during capture
`);
}

// ---- CDP over WebSocket (same minimal client the video capture uses) ----
class Cdp {
  constructor(wsUrl) {
    this.ws = new WebSocket(wsUrl);
    this.nextId = 1;
    this.pending = new Map();
  }
  async open() {
    await new Promise((resolveOpen, rejectOpen) => {
      this.ws.addEventListener("open", resolveOpen, { once: true });
      this.ws.addEventListener("error", rejectOpen, { once: true });
    });
    this.ws.addEventListener("message", (event) => {
      const msg = JSON.parse(event.data);
      if (msg.id && this.pending.has(msg.id)) {
        const { resolvePending, rejectPending } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        msg.error ? rejectPending(new Error(JSON.stringify(msg.error))) : resolvePending(msg.result);
      } else if (msg.method === "Runtime.consoleAPICalled") {
        const text = msg.params.args.map((arg) => arg.value ?? arg.description ?? "").join(" ");
        console.log(`[browser:${msg.params.type}] ${text}`);
      } else if (msg.method === "Runtime.exceptionThrown") {
        console.error("[browser:exception]", msg.params.exceptionDetails?.text ?? msg.params);
      } else if (msg.method === "Log.entryAdded") {
        console.log(`[browser-log:${msg.params.entry.level}] ${msg.params.entry.text}`);
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

async function waitForJson(url, timeoutMs = 20_000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(url);
      if (res.ok) return await res.json();
    } catch {
      // Chrome may not have opened the debugging endpoint yet.
    }
    await sleep(200);
  }
  throw new Error(`Timed out waiting for ${url}`);
}

// Newer Chrome swaps the page's execution context between createTarget's
// navigation and the first evaluate; retry the transient "Cannot find default
// execution context" until the post-navigation context resolves.
async function evaluate(page, params, tries = 3, timeoutMs = 30_000) {
  let lastErr;
  for (let i = 0; i < tries; i++) {
    let timeout;
    try {
      const timedOut = new Promise((_, reject) => {
        timeout = setTimeout(() => reject(new Error(`Runtime.evaluate timed out after ${timeoutMs}ms`)), timeoutMs);
        timeout.unref?.();
      });
      return await Promise.race([page.send("Runtime.evaluate", params), timedOut]);
    } catch (e) {
      lastErr = e;
      if (!/execution context|timed out/i.test(String(e?.message))) throw e;
      await sleep(400);
    } finally {
      if (timeout) clearTimeout(timeout);
    }
  }
  throw new Error(`Runtime.evaluate: ${lastErr?.message ?? "no execution context after retries"}`);
}

// Resize the emulated viewport (the canvas tracks 100vw×100vh×dpr, so this drives the
// WebGPU surface size) and nudge the app's resize handler to reconfigure immediately.
async function setViewport(page, w, h) {
  await page.send("Emulation.setDeviceMetricsOverride", {
    width: w, height: h, deviceScaleFactor: 1, mobile: false, screenWidth: w, screenHeight: h,
  });
  await evaluate(page, { expression: "window.dispatchEvent(new Event('resize')); true", returnByValue: true });
}

// Page.captureScreenshot has no built-in timeout; guard it so a stalled GPU readback
// surfaces as an error instead of hanging the run forever.
async function captureScreenshot(page, timeoutMs = 90_000) {
  let timer;
  const timedOut = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`Page.captureScreenshot timed out after ${timeoutMs}ms`)), timeoutMs);
    timer.unref?.();
  });
  try {
    return await Promise.race([
      page.send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false, fromSurface: true }),
      timedOut,
    ]);
  } finally {
    clearTimeout(timer);
  }
}

async function main() {
  if (hasFlag("--help")) return usage();
  const seed = takeNumber("--seed", 1);
  const duration = takeNumber("--duration", 600);
  const particles = takeNumber("--particles", 32768);
  const width = takeNumber("--width", 7680);
  const height = takeNumber("--height", 4320);
  // Run the heavy 120s playback at this (stable) resolution and resize up to
  // --width/--height only to grab each frame: headless WebGPU stalls if it renders
  // 8K continuously for minutes, but a stable 4K run + a brief 8K final frame is fine.
  const runWidth = takeNumber("--run-width", width);
  const runHeight = takeNumber("--run-height", height);
  const atList = take("--at", "120")
    .split(",")
    .map((s) => Number(s.trim()))
    .filter((n) => Number.isFinite(n) && n >= 0)
    .sort((a, b) => a - b);
  const label = take("--label", `galacto-still-${seed}`);
  const outDir = resolve(take("--out-dir", "renders/stills"));
  const port = Number(take("--port", "8000"));
  const chromePath = take("--chrome") || process.env.CHROME || findChrome();
  const headless = !hasFlag("--no-headless");
  const build = !hasFlag("--no-build");

  if (!chromePath) {
    throw new Error("capture-still: Chrome not found; set CHROME=/path/to/browser or pass --chrome");
  }
  ensureChrome(chromePath);
  if (Math.max(width, height) > 8192) {
    console.warn(`⚠ ${width}x${height} exceeds WebGPU's 8192 max texture; the surface may fail to configure.`);
  }
  if (build) {
    console.log("● Building…");
    run("npm", ["run", "build"], { inherit: true });
  }
  mkdirSync(outDir, { recursive: true });

  const { server, url } = await startStaticServer({ dir: "dist", port, cors: true });
  const debugPort = await getFreePort();
  const profileDir = join(outDir, `${label}-chrome-profile`);
  const chromeArgs = [
    `--remote-debugging-port=${debugPort}`,
    "--remote-debugging-address=127.0.0.1",
    `--user-data-dir=${profileDir}`,
    "--no-first-run",
    "--no-default-browser-check",
    "--enable-unsafe-webgpu",
    "--ignore-gpu-blocklist",
    "--disable-background-timer-throttling",
    "--disable-renderer-backgrounding",
    "--disable-backgrounding-occluded-windows",
    // Quiet Chrome's background machinery (updater, GCM, sync, telemetry): it spins
    // up a few minutes in and can stall the GPU surface readback of a long capture.
    "--disable-background-networking",
    "--disable-component-update",
    "--disable-sync",
    "--no-pings",
    "--disable-features=Translate,OptimizationHints,MediaRouter,DialMediaRouteProvider",
    "--hide-scrollbars",
    `--window-size=${runWidth},${runHeight}`,
  ];
  if (headless) chromeArgs.push("--headless=new");
  chromeArgs.push("about:blank");

  const chrome = spawn(chromePath, chromeArgs, { stdio: ["ignore", "pipe", "pipe"] });
  chrome.stderr.on("data", (data) => process.stderr.write(`[chrome] ${data}`));

  try {
    const version = await waitForJson(`http://127.0.0.1:${debugPort}/json/version`);
    const browser = new Cdp(version.webSocketDebuggerUrl);
    await browser.open();
    const target = await browser.send("Target.createTarget", { url: "about:blank" });
    await browser.send("Target.activateTarget", { targetId: target.targetId });
    const tabs = await waitForJson(`http://127.0.0.1:${debugPort}/json/list`);
    const tab = tabs.find((item) => item.id === target.targetId);
    if (!tab) throw new Error("Could not find the capture tab");

    const page = new Cdp(tab.webSocketDebuggerUrl);
    await page.open();
    await page.send("Page.enable");
    await page.send("Runtime.enable");
    await page.send("Log.enable");
    await setViewport(page, runWidth, runHeight);
    await page.send("Page.bringToFront");
    await page.send("Page.navigate", { url });

    const ready = await evaluate(page, {
      expression: `
        new Promise((resolve) => {
          const started = performance.now();
          const check = () => {
            const loading = document.getElementById("loading");
            const error = document.getElementById("error");
            const canvas = document.getElementById("gpu-canvas");
            if (error && getComputedStyle(error).display !== "none") {
              resolve({ ok: false, reason: document.getElementById("error-details")?.innerText || "error visible" });
              return;
            }
            if (canvas && canvas.width > 0 && canvas.height > 0 && (!loading || getComputedStyle(loading).display === "none")) {
              resolve({ ok: true, width: canvas.width, height: canvas.height });
              return;
            }
            if (performance.now() - started > 30000) {
              resolve({ ok: false, reason: "timeout" });
              return;
            }
            setTimeout(check, 50);
          };
          check();
        })
      `,
      awaitPromise: true,
      returnByValue: true,
    });
    console.log("ready:", JSON.stringify(ready.result.value));
    if (!ready.result.value?.ok) {
      throw new Error(`Page did not become ready: ${JSON.stringify(ready.result.value)}`);
    }

    // Hide the UI and force a pure-black page background, like the video capture.
    await evaluate(page, {
      expression: `
        (() => {
          const style = document.createElement("style");
          style.textContent = "#controls,#rotcurve,#update-toast,#feedback-btn,#runtime-notice{display:none!important} body{cursor:none!important;background:#000!important;overflow:hidden!important}";
          document.head.appendChild(style);
          document.documentElement.style.background = "#000";
          document.body.style.background = "#000";
          window.scrollTo(0, 0);
          return true;
        })()
      `,
      returnByValue: true,
    });

    // Start the deterministic arrangement at t=0 and stamp the in-page clock the
    // sim integrates against, so capture times are measured the same way.
    await evaluate(page, {
      expression: `
        (async () => {
          const t0 = performance.now();
          while (!window.galacto && performance.now() - t0 < 10000) await new Promise((r) => setTimeout(r, 50));
          while (window.galacto?.isReady && !window.galacto.isReady() && performance.now() - t0 < 30000) await new Promise((r) => setTimeout(r, 50));
          if (!window.galacto?.isReady?.()) throw new Error("galacto was not ready before capture");
          window.galacto.setParticleCount(${particles});
          window.galacto.startArrangement(${duration}, ${seed});
          window.__galactoStart = performance.now();
          return true;
        })()
      `,
      awaitPromise: true,
      returnByValue: true,
    }, 3, 45_000);

    const hiRes = width !== runWidth || height !== runHeight;
    const saved = [];
    for (const at of atList) {
      // Wait (at the stable run resolution) until the arrangement clock reaches `at`.
      await evaluate(page, {
        expression: `
          new Promise((resolve) => {
            const tick = () => {
              if (performance.now() - (window.__galactoStart || 0) >= ${at * 1000}) resolve(true);
              else setTimeout(tick, 16);
            };
            tick();
          })
        `,
        awaitPromise: true,
        returnByValue: true,
      }, 1, Math.ceil(at * 1000) + 60_000);

      // Resize up to the capture resolution for just this frame, let the surface
      // reconfigure and render a few frames, grab it, then drop back to run res.
      if (hiRes) {
        await setViewport(page, width, height);
        await sleep(3000);
      }
      const shot = await captureScreenshot(page);
      if (hiRes) await setViewport(page, runWidth, runHeight);

      const buf = Buffer.from(shot.data, "base64");
      const outPath = join(outDir, `${label}-at${at}-${width}x${height}.png`);
      writeFileSync(outPath, buf);
      console.log(`captured t=${at}s → ${outPath}  (${(buf.length / 1048576).toFixed(1)} MB)`);
      saved.push(outPath);
    }

    page.close();
    browser.close();
    console.log(`\n✅ ${saved.length} still(s) → ${outDir}`);
  } finally {
    server.close();
    chrome.kill("SIGTERM");
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});

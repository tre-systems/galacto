import { createServer } from "node:http";
import {
  createWriteStream,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";

const args = process.argv.slice(2);

function usage() {
  console.error(`Usage:
  npm run video:capture -- --url http://localhost:8000/ --duration 360 --width 3840 --height 2160 --fps 60 --label galacto-six-minute

Options:
  --compose <seed>   play the deterministic cinematic arrangement (matches
                     generate_piece with the same seed + duration)
  --out-dir renders/proofs
  --chrome "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
  --bitrate 80000000
  --no-headless
  --no-remux
`);
}

function take(name, fallback = null) {
  const index = args.indexOf(name);
  if (index === -1) return fallback;
  const value = args[index + 1];
  if (!value || value.startsWith("--")) {
    throw new Error(`${name} requires a value`);
  }
  return value;
}

function takeNumber(name, fallback) {
  const raw = take(name);
  if (raw == null) return fallback;
  const value = Number(raw);
  if (!Number.isFinite(value)) throw new Error(`${name} must be a number`);
  return value;
}

function hasFlag(name) {
  return args.includes(name);
}

function run(command, commandArgs) {
  const result = spawnSync(command, commandArgs, {
    stdio: "pipe",
    encoding: "utf8",
  });
  if (result.status !== 0) {
    process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    throw new Error(`${command} failed with status ${result.status}`);
  }
  return result.stdout;
}

function startChunkServer(chunkDir) {
  let doneResolve;
  const done = new Promise((resolveDone) => {
    doneResolve = resolveDone;
  });
  let count = 0;
  const server = createServer((req, res) => {
    res.setHeader("Access-Control-Allow-Origin", "*");
    res.setHeader("Access-Control-Allow-Methods", "POST, OPTIONS");
    res.setHeader("Access-Control-Allow-Headers", "Content-Type");
    if (req.method === "OPTIONS") {
      res.writeHead(204);
      res.end();
      return;
    }

    const parsed = new URL(req.url, "http://127.0.0.1");
    if (parsed.searchParams.get("done") === "1") {
      res.writeHead(200);
      res.end("ok");
      doneResolve(count);
      return;
    }

    const index = parsed.searchParams.get("i") ?? String(count);
    const file = join(chunkDir, `${String(index).padStart(6, "0")}.webm.part`);
    const out = createWriteStream(file);
    req.pipe(out);
    out.on("finish", () => {
      count += 1;
      res.writeHead(200);
      res.end("ok");
    });
    out.on("error", (err) => {
      res.writeHead(500);
      res.end(String(err));
    });
  });

  return new Promise((resolveServer) => {
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      resolveServer({ server, port, done });
    });
  });
}

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

async function main() {
  const durationSec = takeNumber("--duration", 10);
  // --compose <seed> plays the deterministic cinematic arrangement for the capture,
  // so the video matches the audio rendered by generate_piece with the same seed +
  // duration. It self-drives via the ?compose=&dur= URL params.
  const composeSeed = take("--compose");
  const baseUrl = take("--url", "http://localhost:8000/");
  const url =
    composeSeed != null
      ? `${baseUrl}${baseUrl.includes("?") ? "&" : "?"}compose=${encodeURIComponent(composeSeed)}&dur=${durationSec}`
      : baseUrl;
  const width = takeNumber("--width", 1920);
  const height = takeNumber("--height", 1080);
  const fps = takeNumber("--fps", 60);
  const label = take("--label", `galacto-${durationSec}s`);
  const outDir = resolve(take("--out-dir", "renders/proofs"));
  const chromePath = take("--chrome", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
  const headless = !hasFlag("--no-headless");
  const remux = !hasFlag("--no-remux");
  const bitrate = takeNumber("--bitrate", width * height >= 3840 * 2160 ? 80_000_000 : 28_000_000);

  if (hasFlag("--help")) {
    usage();
    return;
  }

  const chunkDir = join(outDir, `${label}-chunks`);
  const webmPath = join(outDir, `${label}.webm`);
  const remuxPath = join(outDir, `${label}-video.webm`);
  const screenshotPath = join(outDir, `${label}-preview.png`);
  const profileDir = join(outDir, `${label}-chrome-profile`);
  mkdirSync(outDir, { recursive: true });
  rmSync(chunkDir, { recursive: true, force: true });
  rmSync(profileDir, { recursive: true, force: true });
  mkdirSync(chunkDir, { recursive: true });

  const { server, port: chunkPort, done } = await startChunkServer(chunkDir);
  const debugPort = 9300 + Math.floor(Math.random() * 500);
  const chromeArgs = [
    `--remote-debugging-port=${debugPort}`,
    `--user-data-dir=${profileDir}`,
    "--no-first-run",
    "--no-default-browser-check",
    "--autoplay-policy=no-user-gesture-required",
    "--enable-unsafe-webgpu",
    "--ignore-gpu-blocklist",
    "--disable-background-timer-throttling",
    "--disable-renderer-backgrounding",
    "--disable-backgrounding-occluded-windows",
    "--hide-scrollbars",
    `--window-size=${width},${height}`,
  ];
  if (headless) chromeArgs.push("--headless=new");
  chromeArgs.push("about:blank");

  const chrome = spawn(chromePath, chromeArgs, { stdio: ["ignore", "pipe", "pipe"] });
  chrome.stderr.on("data", (data) => process.stderr.write(`[chrome] ${data}`));
  chrome.stdout.on("data", (data) => process.stdout.write(`[chrome] ${data}`));

  try {
    const version = await waitForJson(`http://127.0.0.1:${debugPort}/json/version`);
    const browser = new Cdp(version.webSocketDebuggerUrl);
    await browser.open();
    const target = await browser.send("Target.createTarget", { url });
    const tabs = await waitForJson(`http://127.0.0.1:${debugPort}/json/list`);
    const tab = tabs.find((item) => item.id === target.targetId) ?? tabs.find((item) => item.url.includes("localhost"));
    if (!tab) throw new Error("Could not find the capture tab");

    const page = new Cdp(tab.webSocketDebuggerUrl);
    await page.open();
    await page.send("Page.enable");
    await page.send("Runtime.enable");
    await page.send("Log.enable");
    await page.send("Emulation.setDeviceMetricsOverride", {
      width,
      height,
      deviceScaleFactor: 1,
      mobile: false,
      screenWidth: width,
      screenHeight: height,
    });

    const ready = await page.send("Runtime.evaluate", {
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
              resolve({ ok: false, reason: "timeout", loading: loading?.innerText, canvas: !!canvas });
              return;
            }
            requestAnimationFrame(check);
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

    await page.send("Runtime.evaluate", {
      expression: `
        (() => {
          const style = document.createElement("style");
          style.textContent = "#controls,#rotcurve,#update-toast,#feedback-btn{display:none!important} body{cursor:none!important;background:#05060d!important;overflow:hidden!important}";
          document.head.appendChild(style);
          document.documentElement.style.background = "#05060d";
          document.body.style.background = "#05060d";
          window.scrollTo(0, 0);
          return true;
        })()
      `,
      returnByValue: true,
    });

    const screenshot = await page.send("Page.captureScreenshot", {
      format: "png",
      captureBeyondViewport: false,
      fromSurface: true,
    });
    writeFileSync(screenshotPath, Buffer.from(screenshot.data, "base64"));
    console.log(`preview: ${screenshotPath}`);

    const postUrl = `http://127.0.0.1:${chunkPort}/chunk`;
    const recorded = await page.send("Runtime.evaluate", {
      expression: `
        (async () => {
          const canvas = document.getElementById("gpu-canvas");
          const mime = [
            "video/webm;codecs=vp9",
            "video/webm;codecs=vp8",
            "video/webm"
          ].find((item) => MediaRecorder.isTypeSupported(item));
          if (!canvas) throw new Error("No canvas");
          if (!mime) throw new Error("No supported MediaRecorder WebM mime type");
          const stream = canvas.captureStream(${fps});
          const recorder = new MediaRecorder(stream, {
            mimeType: mime,
            videoBitsPerSecond: ${bitrate}
          });
          let index = 0;
          const uploads = [];
          recorder.ondataavailable = (event) => {
            if (event.data && event.data.size > 0) {
              const i = index++;
              uploads.push(fetch("${postUrl}?i=" + i, { method: "POST", body: event.data }));
            }
          };
          const stopped = new Promise((resolve, reject) => {
            recorder.onerror = () => reject(recorder.error || new Error("MediaRecorder error"));
            recorder.onstop = async () => {
              await Promise.all(uploads);
              await fetch("${postUrl}?done=1", { method: "POST", body: new Blob([]) });
              resolve({ chunks: index, mime, width: canvas.width, height: canvas.height });
            };
          });
          recorder.start(1000);
          setTimeout(() => recorder.stop(), ${Math.round(durationSec * 1000)});
          return await stopped;
        })()
      `,
      awaitPromise: true,
      returnByValue: true,
    });
    console.log("recorded:", JSON.stringify(recorded.result.value));

    const chunkCount = await done;
    console.log(`chunks received: ${chunkCount}`);
    const files = readdirSync(chunkDir).filter((file) => file.endsWith(".part")).sort();
    const out = createWriteStream(webmPath);
    for (const file of files) out.write(readFileSync(join(chunkDir, file)));
    await new Promise((resolveWrite) => out.end(resolveWrite));
    console.log(`raw video: ${webmPath}`);

    if (remux) {
      run("ffmpeg", ["-hide_banner", "-y", "-i", webmPath, "-c", "copy", remuxPath]);
      console.log(`remuxed video: ${remuxPath}`);
    }

    page.close();
    browser.close();
  } finally {
    server.close();
    chrome.kill("SIGTERM");
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});

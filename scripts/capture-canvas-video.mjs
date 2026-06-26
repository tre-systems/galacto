import { createServer } from "node:http";
import {
  createWriteStream,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";
import { take, takeNumber, hasFlag, passArg, run } from "./cli.mjs";
import { ensureChrome, ensureExecutable, findChrome, getFreePort } from "./preflight.mjs";

function usage() {
  console.error(`Usage:
  npm run video:capture -- --url http://localhost:8000/ --duration 360 --width 3840 --height 2160 --fps 60 --label galacto-six-minute

Options:
  --compose <seed>   play the deterministic cinematic arrangement (matches
                     generate_piece with the same seed + duration)
  --particles <N>    body count for the arrangement (denser galaxy)
  --produce          render matching audio + mux + start/end captions → final MP4
  --reuse-chunks     skip the capture; assemble from an existing chunk dir (resume
                     a run whose capture finished but a later step failed)
  --out-dir renders/proofs
  --chrome "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
  --bitrate 80000000
  --video-codec auto|h264|hevc|libx264|hevc_videotoolbox
  --no-headless
  --no-remux
`);
}

function startChunkServer(chunkDir, audioPath) {
  let doneResolve;
  const done = new Promise((resolveDone) => {
    doneResolve = resolveDone;
  });
  let audioResolve;
  const audioDone = new Promise((resolveAudio) => {
    audioResolve = resolveAudio;
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

    // The composed-piece audio (one POST) goes to its own file.
    if (parsed.searchParams.get("audio") === "1" && audioPath) {
      const out = createWriteStream(audioPath);
      req.pipe(out);
      out.on("finish", () => {
        res.writeHead(200);
        res.end("ok");
        audioResolve(true);
      });
      out.on("error", (err) => {
        res.writeHead(500);
        res.end(String(err));
      });
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
      resolveServer({ server, port, done, audioDone });
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

// Capture the canvas to a chunked webm via MediaRecorder, POSTing each timeslice to
// the chunk server. Restarts the arrangement from t=0 first so the picture aligns
// with the audio, and samples the render frame rate to confirm the capture was smooth.
async function captureChunks(page, opts) {
  const { screenshotPath, composeSeed, particles, durationSec, fps, bitrate, captureDuration, chunkPort } =
    opts;

  const screenshot = await page.send("Page.captureScreenshot", {
    format: "png",
    captureBeyondViewport: false,
    fromSurface: true,
  });
  writeFileSync(screenshotPath, Buffer.from(screenshot.data, "base64"));
  console.log(`preview: ${screenshotPath}`);

  // Restart the arrangement at t=0 right before recording, so the captured picture
  // begins exactly where generate_piece's audio does (precise alignment).
  if (composeSeed != null) {
    await page.send("Runtime.evaluate", {
      expression: `
        (async () => {
          const started = performance.now();
          while (!window.galacto && performance.now() - started < 10000) {
            await new Promise((r) => requestAnimationFrame(r));
          }
          // Wait for the async engine init before driving it (setters no-op until
          // ready), then apply the body count so the restart re-seeds at it.
          if (window.galacto?.whenReady) await window.galacto.whenReady();
          ${particles != null ? `window.galacto?.setParticleCount(${Number(particles)});` : ""}
          window.galacto?.startArrangement(${durationSec}, ${Number(composeSeed)});
          return true;
        })()
      `,
      awaitPromise: true,
      returnByValue: true,
    });
  }

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
        // Sample the render loop's frame rate during capture so the run can confirm
        // it stayed smooth (and warn if a heavy body count dropped frames).
        const fpsSamples = [];
        const fpsTimer = setInterval(() => {
          const f = window.galacto && window.galacto.fps ? window.galacto.fps() : 0;
          if (f > 0) fpsSamples.push(+f.toFixed(1));
        }, 500);
        const stopped = new Promise((resolve, reject) => {
          recorder.onerror = () => reject(recorder.error || new Error("MediaRecorder error"));
          recorder.onstop = async () => {
            clearInterval(fpsTimer);
            await Promise.all(uploads);
            await fetch("${postUrl}?done=1", { method: "POST", body: new Blob([]) });
            // Drop the first couple of samples while the meter settles.
            const s = fpsSamples.slice(2);
            const fpsMin = s.length ? Math.min(...s) : 0;
            const fpsAvg = s.length ? +(s.reduce((a, b) => a + b, 0) / s.length).toFixed(1) : 0;
            resolve({ chunks: index, mime, width: canvas.width, height: canvas.height, fpsMin, fpsAvg });
          };
        });
        recorder.start(1000);
        setTimeout(() => recorder.stop(), ${Math.round(captureDuration * 1000)});
        return await stopped;
      })()
    `,
    awaitPromise: true,
    returnByValue: true,
  });
  const result = recorded.result.value;
  console.log("recorded:", JSON.stringify(result));

  const { fpsMin, fpsAvg } = result;
  if (fpsAvg) {
    // The meter reads rAF cadence, which varies a little over a long run, so allow
    // some slack before flagging — real GPU stalls show up as a much larger drop.
    const smooth = fpsMin >= fps * 0.85;
    console.log(`${smooth ? "✓" : "⚠"} frame rate: ${fpsAvg} avg / ${fpsMin} min (target ${fps})`);
    if (!smooth) {
      console.log("  ↳ dropped below target — lower --particles or --fps for a smoother capture.");
    }
  }
  return result;
}

async function main() {
  const durationSec = takeNumber("--duration", 10);
  // --compose <seed> plays the deterministic cinematic arrangement for the capture,
  // so the video matches the audio rendered by generate_piece with the same seed +
  // duration. It self-drives via the ?compose=&dur= URL params.
  const composeSeed = take("--compose");
  // --particles <N> renders the arrangement at a higher body count for a denser
  // galaxy; the sim self-throttles its step rate to keep the frame rate smooth.
  const particles = take("--particles");
  const baseUrl = take("--url", "http://localhost:8000/");
  const url =
    composeSeed != null
      ? `${baseUrl}${baseUrl.includes("?") ? "&" : "?"}compose=${encodeURIComponent(composeSeed)}&dur=${durationSec}${
          particles != null ? `&particles=${encodeURIComponent(particles)}` : ""
        }`
      : baseUrl;
  const width = takeNumber("--width", 1920);
  const height = takeNumber("--height", 1080);
  const fps = takeNumber("--fps", 60);
  const label = take("--label", `galacto-${durationSec}s`);
  const outDir = resolve(take("--out-dir", "renders/proofs"));
  const chromePath = take("--chrome") || process.env.CHROME || findChrome();
  const headless = !hasFlag("--no-headless");
  const remux = !hasFlag("--no-remux");
  const bitrate = takeNumber("--bitrate", width * height >= 3840 * 2160 ? 80_000_000 : 28_000_000);
  // --produce renders the matching audio (generate_piece) and muxes + captions the
  // capture into a finished YouTube-ready MP4. Requires --compose <seed>.
  const produce = hasFlag("--produce");
  // --reuse-chunks skips the capture and assembles from an existing chunk dir (resume
  // a run whose capture succeeded but whose audio/mux/caption step failed).
  const reuseChunks = hasFlag("--reuse-chunks");
  const lufs = takeNumber("--lufs", -16);
  // The arrangement's audio rings out a reverb tail past the arc; capture that long
  // so the fade-out is in the picture too. Matches EXPORT_TAIL_SEC in audio.rs.
  const EXPORT_TAIL_SEC = 6;
  const captureDuration = produce ? durationSec + EXPORT_TAIL_SEC : durationSec;

  if (hasFlag("--help")) {
    usage();
    return;
  }
  if (produce && composeSeed == null) {
    throw new Error("--produce requires --compose <seed>");
  }
  if (!chromePath) {
    throw new Error("capture: Chrome/Chromium not found; set CHROME=/path/to/browser or pass --chrome /path/to/browser");
  }
  ensureChrome(chromePath);
  if (remux || produce) {
    ensureExecutable("ffmpeg", {
      args: ["-version"],
      label: "ffmpeg",
      installHint: "install ffmpeg (macOS: brew install ffmpeg)",
    });
  }
  if (produce) {
    ensureExecutable("ffprobe", {
      args: ["-version"],
      label: "ffprobe",
      installHint: "install ffmpeg (macOS: brew install ffmpeg)",
    });
    ensureExecutable("rsvg-convert", {
      args: ["--version"],
      label: "rsvg-convert",
      installHint: "install librsvg (macOS: brew install librsvg)",
    });
  }

  const chunkDir = join(outDir, `${label}-chunks`);
  const webmPath = join(outDir, `${label}.webm`);
  const remuxPath = join(outDir, `${label}-video.webm`);
  const audioPath = join(outDir, `${label}.wav`);
  const muxPath = join(outDir, `${label}-muxed.mkv`);
  const finalPath = join(outDir, `${label}.mp4`);
  const screenshotPath = join(outDir, `${label}-preview.png`);
  const profileDir = join(outDir, `${label}-chrome-profile`);
  mkdirSync(outDir, { recursive: true });
  // --reuse-chunks resumes a run from an existing chunk dir (e.g. after the capture
  // succeeded but a later step failed) — skip the recording and keep the chunks.
  if (!reuseChunks) {
    rmSync(chunkDir, { recursive: true, force: true });
    mkdirSync(chunkDir, { recursive: true });
  }
  rmSync(profileDir, { recursive: true, force: true });

  const { server, port: chunkPort, done, audioDone } = await startChunkServer(
    chunkDir,
    produce ? audioPath : null,
  );
  const debugPort = await getFreePort();
  const chromeArgs = [
    `--remote-debugging-port=${debugPort}`,
    "--remote-debugging-address=127.0.0.1",
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
          style.textContent = "#controls,#rotcurve,#update-toast,#feedback-btn{display:none!important} body{cursor:none!important;background:#000000!important;overflow:hidden!important}";
          document.head.appendChild(style);
          document.documentElement.style.background = "#000000";
          document.body.style.background = "#000000";
          window.scrollTo(0, 0);
          return true;
        })()
      `,
      returnByValue: true,
    });

    let chunkCount;
    if (reuseChunks) {
      chunkCount = readdirSync(chunkDir).filter((f) => f.endsWith(".part")).length;
      console.log(`reusing ${chunkCount} existing chunks from ${chunkDir}`);
    } else {
      const recorded = await captureChunks(page, {
        screenshotPath,
        composeSeed,
        particles,
        durationSec,
        fps,
        bitrate,
        captureDuration,
        chunkPort,
      });
      if (!recorded.chunks) {
        throw new Error("capture: MediaRecorder completed without producing any chunks");
      }
      chunkCount = await done;
      console.log(`chunks received: ${chunkCount}`);
    }
    if (chunkCount <= 0) {
      throw new Error(`capture: no recorded chunks found in ${chunkDir}`);
    }

    // Concatenate the chunks into one webm. MediaRecorder timeslices are valid
    // back-to-back, so a byte concat is a correct webm. Honour stream backpressure —
    // a multi-GB 4K capture has hundreds of chunks, and firing every write() at once
    // (ignoring the `false` return) overruns the buffer and flushes nothing.
    const files = readdirSync(chunkDir).filter((file) => file.endsWith(".part")).sort();
    if (files.length === 0) {
      throw new Error(`capture: no chunk files found in ${chunkDir}`);
    }
    const out = createWriteStream(webmPath);
    for (const file of files) {
      if (!out.write(readFileSync(join(chunkDir, file)))) {
        await new Promise((r) => out.once("drain", r));
      }
    }
    await new Promise((resolve, reject) => {
      out.on("error", reject);
      out.end(resolve);
    });
    console.log(`raw video: ${webmPath} (${(statSync(webmPath).size / 1e9).toFixed(2)} GB)`);

    if (remux) {
      run("ffmpeg", ["-hide_banner", "-y", "-i", webmPath, "-c", "copy", remuxPath]);
      console.log(`remuxed video: ${remuxPath}`);
    }

    if (produce) {
      // Render the matching mastered audio (offline, in the same page) and have the
      // page POST the WAV bytes to our server, which writes them to audioPath.
      const audioUrl = `http://127.0.0.1:${chunkPort}/?audio=1`;
      const rendered = await page.send("Runtime.evaluate", {
        expression: `window.galacto.renderPieceTo(${JSON.stringify(audioUrl)}, ${durationSec}, ${Number(composeSeed)}, ${lufs})`,
        awaitPromise: true,
        returnByValue: true,
      });
      await audioDone;
      console.log(`audio: ${audioPath}`);
      console.log(`master: ${String(rendered.result.value).replace(/\n/g, " | ")}`);
    }

    page.close();
    browser.close();

    if (produce) {
      // Mux the VP9 video with the WAV (copy video, encode AAC) into an intermediate,
      // then add the start/end captions (which re-encodes to HEVC + faststart) → MP4.
      const videoSource = remux ? remuxPath : webmPath;
      run("ffmpeg", [
        "-hide_banner", "-y",
        "-i", videoSource,
        "-i", audioPath,
        "-map", "0:v:0", "-map", "1:a:0",
        "-c:v", "copy", "-c:a", "aac", "-b:a", "320k",
        "-shortest", muxPath,
      ]);
      console.log(`muxed: ${muxPath}`);
      run("node", [
        "scripts/add-video-captions.mjs",
        "--input", muxPath,
        "--output", finalPath,
        ...passArg("--start-title"),
        ...passArg("--start-subtitle"),
        ...passArg("--end-title"),
        ...passArg("--end-subtitle"),
        ...passArg("--video-codec"),
      ]);
      console.log(`\n✅ Finished piece: ${finalPath}`);
    }
  } finally {
    server.close();
    chrome.kill("SIGTERM");
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});

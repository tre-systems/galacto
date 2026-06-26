#!/usr/bin/env node
// One-command production: build → serve → capture video + render mastered audio +
// start/end captions → a finished, YouTube-ready MP4. No UI interaction.
//
//   npm run produce -- --seed 7 --duration 600
//
// Defaults to a 10-minute piece at 4K/60. Same seed + duration always yields the
// same arrangement.
import { take, hasFlag, passArg, run } from "./cli.mjs";
import { ensureChrome, ensureExecutable, findChrome } from "./preflight.mjs";
import { startStaticServer } from "./serve-static.mjs";
import { spawn } from "node:child_process";

function usage() {
  console.error(`Usage:
  npm run produce -- --seed 7 --duration 600 [options]

Options:
  --seed <N>         deterministic arrangement seed (default 1)
  --duration <sec>  composed-piece duration before reverb tail (default 600)
  --width <px>      capture width (default 3840)
  --height <px>     capture height (default 2160)
  --fps <N>         capture frame rate (default 60)
  --particles <N>   body count for the arrangement (default 32768)
  --label <name>    output label (default galacto-piece-<seed>)
  --port <N>        local static-server port (default 8000)
  --chrome <path>   Chrome/Chromium executable
  --no-build        use an already-built dist/
  --no-headless     show Chrome during capture
  --video-codec auto|h264|hevc|libx264|hevc_videotoolbox
`);
}

if (hasFlag("--help")) {
  usage();
  process.exit(0);
}

const seed = take("--seed", "1");
const duration = take("--duration", "600"); // 10 min default composed piece
const width = take("--width", "3840");
const height = take("--height", "2160");
const fps = take("--fps", "60");
// 2× the default body count: a denser, finer galaxy that still holds a rock-steady
// 60+ fps at 4K (per-particle size scales ∝ 1/√count, so the glow fill-rate stays
// flat). Higher values (e.g. 49152) look richer but can dip below 60 at 4K.
const particles = take("--particles", "32768");
const label = take("--label", `galacto-piece-${seed}`);
const port = Number(take("--port", "8000"));
const chromePath = take("--chrome") || process.env.CHROME || findChrome();

if (!chromePath) {
  throw new Error("produce: Chrome/Chromium not found; set CHROME=/path/to/browser or pass --chrome /path/to/browser");
}
ensureChrome(chromePath);
ensureExecutable("ffmpeg", {
  args: ["-version"],
  label: "ffmpeg",
  installHint: "install ffmpeg (macOS: brew install ffmpeg)",
});
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

if (!hasFlag("--no-build")) {
  console.log("● Building…");
  run("npm", ["run", "build"], { inherit: true });
}

function runStreaming(command, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: "inherit" });
    child.on("error", (error) => {
      reject(new Error(`${command} ${args.join(" ")} failed to start: ${error.message}`));
    });
    child.on("close", (status) => {
      if (status === 0) {
        resolve();
      } else {
        reject(new Error(`${command} ${args.join(" ")} failed (status ${status})`));
      }
    });
  });
}

console.log("● Serving dist/…");
const { server, url } = await startStaticServer({ dir: "dist", port, cors: true });
try {
  console.log(`● Producing a ${duration}s piece (seed ${seed}) at ${width}x${height}…`);
  await runStreaming(
    "node",
    [
      "scripts/capture-canvas-video.mjs",
      "--produce",
      "--compose", seed,
      "--duration", duration,
      "--url", url,
      "--width", width,
      "--height", height,
      "--fps", fps,
      "--particles", particles,
      "--label", label,
      "--chrome", chromePath,
      // Forward the optional pass-through flags the user supplied.
      ..."--start-title --start-subtitle --end-title --end-subtitle --lufs --out-dir --bitrate --video-codec"
        .split(" ")
        .flatMap(passArg),
      ...(hasFlag("--no-headless") ? ["--no-headless"] : []),
    ],
    { inherit: true },
  );
} finally {
  server.close();
}

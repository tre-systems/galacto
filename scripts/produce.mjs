#!/usr/bin/env node
// One-command production: build → serve → capture video + render mastered audio +
// start/end captions → a finished, YouTube-ready MP4. No UI interaction.
//
//   npm run produce -- --seed 7 --duration 600
//
// Defaults to a 10-minute piece at 4K/60 (the researched sweet spot for a composed
// ambient track). Same seed + duration always yields the same piece.
import { spawn, spawnSync } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";

const args = process.argv.slice(2);

function take(name, fallback = null) {
  const i = args.indexOf(name);
  if (i === -1) return fallback;
  const v = args[i + 1];
  if (v == null || v.startsWith("--")) throw new Error(`${name} requires a value`);
  return v;
}
function hasFlag(name) {
  return args.includes(name);
}
function passthrough(names) {
  const out = [];
  for (const n of names) {
    const i = args.indexOf(n);
    if (i === -1) continue;
    out.push(n);
    const v = args[i + 1];
    if (v != null && !v.startsWith("--")) out.push(v);
  }
  return out;
}
function run(cmd, a) {
  const r = spawnSync(cmd, a, { stdio: "inherit" });
  if (r.status !== 0) throw new Error(`${cmd} ${a.join(" ")} failed (status ${r.status})`);
}
async function waitForPort(url, timeoutMs = 30000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const res = await fetch(url);
      if (res.ok) return;
    } catch {
      // server not up yet
    }
    await sleep(300);
  }
  throw new Error(`Timed out waiting for ${url}`);
}

const seed = take("--seed", "1");
const duration = take("--duration", "600"); // 10 min — researched length for a piece
const width = take("--width", "3840");
const height = take("--height", "2160");
const fps = take("--fps", "60");
// 2× the default body count: a denser, finer galaxy that still holds a rock-steady
// 60+ fps at 4K (per-particle size scales ∝ 1/√count, so the glow fill-rate stays
// flat). Higher values (e.g. 49152) look richer but can dip below 60 at 4K.
const particles = take("--particles", "32768");
const label = take("--label", `galacto-piece-${seed}`);
const port = Number(take("--port", "8000"));

if (!hasFlag("--no-build")) {
  console.log("● Building…");
  run("npm", ["run", "build"]);
}

console.log("● Serving pkg/…");
const serve = spawn("npx", ["-y", "serve", "pkg", "-l", String(port), "--cors"], {
  stdio: "ignore",
});
try {
  await waitForPort(`http://localhost:${port}/`);
  console.log(`● Producing a ${duration}s piece (seed ${seed}) at ${width}x${height}…`);
  run("node", [
    "scripts/capture-canvas-video.mjs",
    "--produce",
    "--compose", seed,
    "--duration", duration,
    "--url", `http://localhost:${port}/`,
    "--width", width,
    "--height", height,
    "--fps", fps,
    "--particles", particles,
    "--label", label,
    ...passthrough([
      "--start-title",
      "--start-subtitle",
      "--end-title",
      "--end-subtitle",
      "--lufs",
      "--out-dir",
      "--bitrate",
    ]),
    ...(hasFlag("--no-headless") ? ["--no-headless"] : []),
  ]);
} finally {
  serve.kill("SIGTERM");
}

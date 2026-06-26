#!/usr/bin/env node
// One-command production: build → serve → capture video + render mastered audio +
// start/end captions → a finished, YouTube-ready MP4. No UI interaction.
//
//   npm run produce -- --seed 7 --duration 600
//
// Defaults to a 10-minute piece at 4K/60 (the researched sweet spot for a composed
// ambient track). Same seed + duration always yields the same piece.
import { take, hasFlag, passArg, run } from "./cli.mjs";
import { startStaticServer } from "./serve-static.mjs";

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
  run("npm", ["run", "build"], { inherit: true });
}

console.log("● Serving dist/…");
const { server, url } = await startStaticServer({ dir: "dist", port, cors: true });
try {
  console.log(`● Producing a ${duration}s piece (seed ${seed}) at ${width}x${height}…`);
  run(
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
      // Forward the optional pass-through flags the user supplied.
      ..."--start-title --start-subtitle --end-title --end-subtitle --lufs --out-dir --bitrate"
        .split(" ")
        .flatMap(passArg),
      ...(hasFlag("--no-headless") ? ["--no-headless"] : []),
    ],
    { inherit: true },
  );
} finally {
  server.close();
}

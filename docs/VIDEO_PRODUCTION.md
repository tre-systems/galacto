# Galacto Video Production

Current operational runbook for producing a composed Galacto video. Longer-term
native frame export and DAW/stem workflows live in the [backlog](../BACKLOG.md);
this file stays focused on what works today.

## Output Shape

`npm run produce` creates a finished MP4:

- deterministic cinematic arrangement from one seed and duration;
- browser-captured WebGPU canvas, with controls hidden;
- matching offline-rendered, mastered audio from the same seed and duration;
- burned-in start/end captions;
- HEVC video + AAC audio, with `+faststart`.

Defaults:

| Setting | Default |
| --- | --- |
| Duration | `600` seconds |
| Resolution | `3840x2160` |
| Frame rate | `60` fps |
| Bodies | `32768` |
| Audio target | `-16` LUFS |
| Output directory | `renders/proofs/` |

The 32,768-body default is 2x the interactive body count. Particle size scales as
`1/sqrt(count)`, so glow fill-rate stays roughly stable; gravity is still `O(N^2)`,
so higher counts can drop frames.

## One-Command Render

```bash
npm run produce -- --seed 5 --duration 600
```

Typical output:

```text
renders/proofs/galacto-piece-5.mp4
renders/proofs/galacto-piece-5.wav
renders/proofs/galacto-piece-5-preview.png
renders/proofs/galacto-piece-5-chunks/
```

Useful flags:

```bash
npm run produce -- \
  --seed 7 \
  --duration 600 \
  --particles 32768 \
  --width 3840 \
  --height 2160 \
  --fps 60 \
  --lufs -16 \
  --label galacto-piece-7 \
  --out-dir renders/proofs
```

Caption flags are passed through:

```bash
--start-title "Galacto"
--start-subtitle "Self-gravitating N-body galaxy simulation"
--end-title "Galacto"
--end-subtitle "galacto.org\nSimulation and sound: Multivibrator"
```

Use `--no-build` only when `dist/` is already a freshly verified build. Use
`--no-headless` when debugging the capture browser.

## Required Tools

- Node.js 22+ and lockfile npm dependencies (`npm run setup` runs `npm ci`).
- Rust + the wasm target, `wasm-pack` 0.15.0, and `cargo-audit` 0.22.2 (`npm run setup` installs these).
- Chrome/Chromium for canvas capture.
- `ffmpeg` and `ffprobe` on `PATH`.
- `rsvg-convert` from librsvg for caption plates (`brew install librsvg`).

The script builds and serves the local `dist/` artifact itself unless `--no-build`
is supplied.

## What The Pipeline Does

1. `scripts/produce.mjs` builds `dist/` and serves it locally.
2. `scripts/capture-canvas-video.mjs --produce --compose <seed>` opens Chrome,
   loads the page with `?compose=<seed>&dur=<seconds>`, records the WebGPU canvas
   via `canvas.captureStream()`, and posts MediaRecorder chunks to a local chunk
   server.
3. The same browser session calls `window.galacto.renderPieceTo(...)`, which renders
   the matching audio offline through the shared Web Audio graph and pure-Rust
   mastering chain.
4. `ffmpeg` muxes the captured picture and mastered WAV.
5. `scripts/add-video-captions.mjs` burns in start/end captions and writes the MP4.

The capture step prints average/min FPS. Treat a low-FPS warning as a failed render
for production purposes; lower `--particles`, resolution, or frame rate and rerun.

## Resuming A Failed Run

If capture finished but muxing, audio, or captions failed, reuse the existing chunks:

```bash
npm run video:capture -- \
  --produce \
  --reuse-chunks \
  --compose 5 \
  --duration 600 \
  --label galacto-piece-5 \
  --out-dir renders/proofs
```

Keep the same seed, duration, label, and output directory so the chunk directory
matches.

## Captions Only

Add captions to an existing MP4:

```bash
npm run video:captions -- \
  --input renders/proofs/input.mp4 \
  --output renders/proofs/output-captioned.mp4 \
  --start-title "Galacto" \
  --start-subtitle "Self-gravitating N-body galaxy simulation" \
  --end-title "Galacto" \
  --end-subtitle "galacto.org\nSimulation and sound: Multivibrator"
```

These are visual title/credit overlays, not accessibility subtitles. For actual
subtitles, upload an `.srt` separately in YouTube Studio.

## Upload Notes

YouTube re-encodes uploads, so provide the cleanest source that is practical. The
current MP4 is a convenient upload/reference file; keep the WAV next to it as the
audio master.

YouTube's published upload guidance currently recommends MP4, fast-start metadata,
AAC-LC or Opus audio at 48 kHz, BT.709 for SDR, and 53-68 Mbps for 2160p/60 SDR
H.264 uploads. The Galacto helper emits HEVC rather than H.264; YouTube accepts and
re-encodes common upload formats, but use a short private upload before a final
release when changing codec, bitrate, duration, or colour settings.

References:

- <https://support.google.com/youtube/answer/1722171>
- <https://support.google.com/youtube/answer/4603579>

## Known Limits

- Capture is real-time. A 10-minute piece takes about 10 minutes plus build/audio/
  mux/caption time.
- The browser is still driving WebGPU presentation, so a capture can inherit browser
  timing behaviour. The offline audio is sample-exact for the requested duration.
- WebGPU floating-point work is not guaranteed bit-identical across hardware. Use
  one render machine/GPU for final masters.
- The caption helper currently targets macOS VideoToolbox HEVC. Portability and a
  codec flag are tracked in the [backlog](../BACKLOG.md#production-export).

## Lower-Level Commands

Visual capture only:

```bash
npm run build
npm run serve
```

Then, in another terminal:

```bash
npm run video:capture -- \
  --url http://localhost:8000/ \
  --compose 5 \
  --duration 600 \
  --width 3840 \
  --height 2160 \
  --fps 60 \
  --particles 32768 \
  --label galacto-piece-5 \
  --out-dir renders/proofs
```

Use the one-command pipeline for normal production; the lower-level commands are
for debugging or resuming parts of a render.

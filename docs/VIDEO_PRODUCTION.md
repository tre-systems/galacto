# Galacto Video Production

This document sketches a production workflow for a high-quality YouTube video with
sound, plus the engineering path for direct video and audio export.

## Goals

- Produce a clean 16:9 video, ideally 3840x2160 at 60 fps.
- Keep the image free of browser chrome, UI, dropped frames, notifications, and
screen-capture compression.
- Get audio into Logic as high-quality, editable material rather than a browser
capture.
- Keep the final piece focused on the galaxy visuals, with only minimal branding.

## Recommended Creative Shape

Open on the simulation, not on a title card. The first frame should already be
Galacto doing what makes it interesting.

Use a short fade in:

- Visual fade in: 1-2 seconds.
- Audio fade in: 3-5 seconds, slower than the picture.
- Optional title overlay: small, 2-3 seconds, over the visuals.

End with a longer, calmer fade:

- Hold a strong final composition for a few seconds.
- Audio fade out: 8-12 seconds.
- Visual fade or fade-to-black: 3-5 seconds.
- Optional end credits: 5-8 seconds.

Credits fit better at the end than the start. A simple end card is enough:

```text
Galacto
Self-gravitating N-body galaxy simulation
galacto.org
```

If the video uses post-produced audio, add a short music/sound credit there too.
The public creator credit for Galacto videos is **Multivibrator**.

## Fast Workflow: Browser Capture

This is the lowest-effort route and is good enough for test cuts:

1. Run the local production build fullscreen.
2. Hide controls and overlays.
3. Record at 3840x2160, 60 fps using OBS, Screen Studio, or a similar local
   recorder.
4. Capture system audio separately if possible.
5. Import the audio into Logic, process it, and lay the final mix back against
   the picture in Final Cut, Resolve, or Premiere.

This route is practical, but it is not ideal. It records the browser's real-time
presentation, so dropped frames, browser scheduling, display refresh, and capture
compression can all get baked into the master. It also gives Logic a mixed stereo
track rather than clean musical stems.

## Improved Proof Workflow: Direct Canvas Capture

For the six-minute proof render, the practical near-term workflow was better than
screen recording but lighter than the full native exporter: capture the WebGPU
canvas directly with `canvas.captureStream()` in Chrome, then mux it with offline
WAV audio. This avoids browser chrome, notifications, mouse cursors, and screen
recorder compression.

The repo now has a repeatable helper for this proof workflow:

```bash
npm run build
npm run serve
```

In another terminal:

```bash
npm run video:capture -- \
  --url http://localhost:8000/ \
  --duration 360 \
  --width 3840 \
  --height 2160 \
  --fps 60 \
  --label galacto-six-minute \
  --out-dir renders/proofs
```

Outputs:

```text
renders/proofs/
├── galacto-six-minute-preview.png
├── galacto-six-minute.webm
├── galacto-six-minute-video.webm
├── galacto-six-minute-chunks/
└── galacto-six-minute-chrome-profile/
```

Use the `*-video.webm` file for muxing or editing; it is remuxed by ffmpeg after
capture. `renders/` is git-ignored because these files are large.

### Composed piece — locked visuals + audio (recommended)

For a *finished* piece rather than a free-running capture, use the **cinematic
arrangement** (`src/arrangement.rs`): a deterministic A→B→C arc (sparse intro →
gathering build → serene awe peak ~two-thirds in → dispersing resolution) keyed by
a `seed` + `duration`. Because the arc is deterministic, the captured picture and
the offline-mastered audio — produced from the *same* `seed`/`duration` — line up.

1. **Capture the visuals**, playing the arrangement (the `--compose <seed>`
   self-drives the camera + galaxy via `?compose=`):

   ```bash
   npm run video:capture -- --compose 5 --duration 240 \
     --width 3840 --height 2160 --fps 60 --label galacto-piece-5
   ```

2. **Render the matching audio** (mastered): on the local site, open the **Studio
   export** panel → *Compose* → set the same length (4 min) and seed (5) → **Generate
   WAV** → `galacto-piece-5.wav`.

3. **Mux** the two into the final video:

   ```bash
   ffmpeg -i renders/proofs/galacto-piece-5-video.webm -i galacto-piece-5.wav \
     -c:v copy -c:a aac -b:a 320k -shortest galacto-piece-5.mp4
   ```

Same seed + duration ⇒ the journey (build, peak, resolution, stereo pan) is shared,
so audio and picture stay together. Different seeds give different pieces.

This is still not the final production architecture. The simulation is still
running live in Chrome, so a long capture can land a fraction short of the exact
requested duration and may still inherit browser timing behaviour. For the
six-minute proof, the video stream landed at about `5:59.72`; the offline audio
stems were exactly `6:00.00`.

Recommended proof-render post steps:

```bash
# Remove DC offset/subsonic energy from each stem before Logic or muxing.
ffmpeg -y -i master.wav \
  -af highpass=f=20 \
  -ar 48000 -c:a pcm_f32le master-clean.wav

# Make a YouTube/reference master from the cleaned mix.
ffmpeg -y -i master-clean.wav \
  -af loudnorm=I=-14:TP=-1.5:LRA=11 \
  -ar 48000 -c:a pcm_f32le master-clean-youtube.wav

# Mux without re-encoding the captured VP9 picture.
ffmpeg -y \
  -i renders/proofs/galacto-six-minute-video.webm \
  -i master-clean-youtube.wav \
  -map 0:v:0 -map 1:a:0 \
  -c:v copy -c:a libopus -b:a 320k -shortest \
  renders/proofs/galacto-six-minute-youtube.webm
```

For a smaller MP4 delivery/reference file, encode the picture to HEVC and keep
the audio at 48 kHz AAC:

```bash
ffmpeg -y \
  -i renders/proofs/galacto-six-minute-video.webm \
  -i master-clean-youtube.wav \
  -map 0:v:0 -map 1:a:0 \
  -vf "fade=t=in:st=0:d=1.5,fade=t=out:st=354.7:d=5" \
  -c:v hevc_videotoolbox -b:v 55M -maxrate 70M -bufsize 110M -tag:v hvc1 \
  -pix_fmt yuv420p -color_primaries bt709 -color_trc bt709 -colorspace bt709 \
  -c:a aac -b:a 384k -movflags +faststart -shortest \
  renders/proofs/galacto-six-minute-youtube-faded.mp4
```

For a final YouTube upload, prefer the least-compressed source that is convenient
to upload. In this workflow that is usually the VP9/Opus WebM; the HEVC MP4 is a
good smaller reference and still acceptable for upload.

## Title And End Captions

For Galacto, keep captions minimal and burn them into the picture rather than
starting with a separate title card. The visuals should be visible from the first
frame.

Recommended opening caption:

```text
Galacto
Self-gravitating N-body galaxy simulation
```

Recommended end caption:

```text
Galacto
galacto.org
Simulation and sound: Multivibrator
```

Add them to an existing MP4 with:

```bash
npm run video:captions -- \
  --input renders/proofs/galacto-six-minute-youtube-faded.mp4 \
  --output renders/proofs/galacto-six-minute-youtube-captioned.mp4 \
  --start-title "Galacto" \
  --start-subtitle "Self-gravitating N-body galaxy simulation" \
  --end-title "Galacto" \
  --end-subtitle "galacto.org\nSimulation and sound: Multivibrator"
```

The helper overlays the opening caption near the lower third and the end caption
near the centre during the final seconds. It re-encodes the video because burned
text changes the picture, but copies the audio track unchanged. It renders
transparent SVG caption plates with `rsvg-convert`, so install librsvg first if
needed:

```bash
brew install librsvg
```

If "captions" means actual accessibility subtitles rather than visual title
text, create an `.srt` file instead and upload it in YouTube Studio. For this
piece, an `.srt` would likely only contain a short opening and end credit:

```text
1
00:00:01,700 --> 00:00:05,700
Galacto
Self-gravitating N-body galaxy simulation

2
00:05:52,700 --> 00:05:59,500
Galacto
galacto.org
Simulation and sound: Multivibrator
```

Do not use YouTube subtitles for the primary title/credit treatment if the text
is part of the visual composition; subtitles can be turned off, styled
differently by the viewer, or hidden by platform UI.

## Logic Audio Workflow

Use Logic as the final sound-design and mastering stage. Start from the cleaned
48 kHz WAV stems rather than the AAC/Opus audio inside a video file.

Session setup:

- Create a 48 kHz project.
- Import all stems at bar 1 / timecode 00:00:00:00.
- Keep the stems as 32-bit float or 24-bit PCM. Do not normalize them on import.
- Disable Flex/time-stretching unless deliberately editing timing.
- Keep the picture locked and replace only the final audio when exporting.

Suggested stem treatment:

- **Drone** — high-pass around 20-30 Hz, gentle low-shelf cleanup if the master
  gets cloudy, slow modulation or chorus only if it stays subtle.
- **Notes** — small plate or shimmer send, light transient control, avoid making
  the notes much louder than the bed.
- **Texture** — high-pass higher, often 80-150 Hz, then tuck it under the drone;
  automate this stem instead of leaving it static.
- **Reverb returns** — use sends rather than inserting a huge reverb on every
  track. Try ChromaVerb or Space Designer with 6-12 s decay, 20-60 ms predelay,
  high-pass the return, and low-pass the top end if it gets glassy.
- **Bit/crushed reverb** — if using Bitcrusher or a degraded reverb colour, use
  it as a parallel aux at a low level. Put the bit effect before the reverb, then
  high-pass and low-pass the return so it adds texture without turning the whole
  mix gritty.

Master bus:

- Correct DC/subsonic energy first if the imported files have not already been
  cleaned.
- Use broad EQ moves only; the video wants space, not a loud pop master.
- Use gentle compression or Multipressor with modest gain reduction.
- Add saturation/exciter carefully, mainly to help small speakers.
- Put the limiter last. Aim for around `-14 LUFS` integrated and true peak no
  higher than `-1.5 dBTP` for the YouTube upload reference.

Export a final 48 kHz WAV from Logic, then replace the video's audio without
re-encoding the picture:

```bash
ffmpeg -y \
  -i renders/proofs/galacto-six-minute-youtube-captioned.mp4 \
  -i logic-master.wav \
  -map 0:v:0 -map 1:a:0 \
  -c:v copy -c:a aac -b:a 384k -shortest -movflags +faststart \
  renders/proofs/galacto-six-minute-youtube-final.mp4
```

Keep the Logic project and the exported WAV alongside the video. The WAV is the
audio master; the AAC/Opus in the upload file is just a delivery encoding.

## Best Workflow: Direct Export

The best result is a deterministic offline exporter:

1. Define a production timeline: scenario, seed, duration, speed, camera path,
   slider automation, fade timings, and resolution.
2. Step the simulation from that timeline without relying on `requestAnimationFrame`.
3. Render each video frame to an offscreen texture.
4. Save a lossless or near-lossless image sequence.
5. Render audio directly to WAV stems from the same timeline.
6. Mix/master the stems in Logic.
7. Combine the mastered audio with the rendered picture.

This should produce better results than browser capture because the output is
frame-exact, uncompressed before the final encode, repeatable on the same renderer,
and free from UI/capture artifacts.

There is one caveat: WebGPU floating-point reductions are not guaranteed to be
bit-identical across all GPU hardware. A direct renderer gives deterministic
timeline control and exact frame output, but long-run particle trajectories may
still diverge slightly between different GPUs. For video production, render the
final master on one chosen machine/GPU and treat that render as the source of
truth.

## Direct Video Export Design

The current architecture is close to supporting this because the simulation and
renderer are mostly `wgpu` code, with browser-specific setup isolated near the
WASM entry and canvas surface.

A production exporter should be a native Rust binary, for example:

```text
cargo run --release --bin render_video -- \
  --timeline timelines/youtube-hero.toml \
  --out renders/youtube-hero/frames \
  --width 3840 \
  --height 2160 \
  --fps 60
```

Implementation outline:

- Add a `Timeline` format for scenario, camera keyframes, speed, controls, and
  duration.
- Add a headless `wgpu` setup path that creates a device/queue without a browser
  canvas.
- Render into an offscreen texture rather than a swapchain surface.
- Copy the final tonemapped frame texture into a CPU-readable buffer.
- Save numbered frames, for example `frame_000001.png`.
- Use `ffmpeg` to make a high-quality intermediate or YouTube upload file.

Recommended visual intermediates:

- Image sequence: PNG or TIFF at 3840x2160.
- Editing intermediate: ProRes 422 HQ or DNxHR HQX.
- YouTube upload: 4K 60 fps H.265 or H.264 at a high bitrate, or upload the
  ProRes/DNxHR master if file size is acceptable.

YouTube re-encodes uploads, so the source should be as clean as possible. Their
published guidance recommends high-quality source uploads and lists 4K 60 fps SDR
upload bitrates in the 66-85 Mbps range:

- <https://support.google.com/youtube/answer/1722171>
- <https://support.google.com/youtube/answer/4603579>

## Direct Audio Export Design

A self-contained audio export already ships as a **local-only Studio export panel**
(revealed only when the page is served from localhost, so it stays off the public
site): **Record** captures the live `GalaxyState` timeline, and **Export** rebuilds
the same node graph on an
`OfflineAudioContext`, replays the timeline through it (faster than real time,
glitch-free), and runs the result through the pure-Rust master in `src/mastering.rs`
(subsonic high-pass, mono bass, BS.1770 loudness to a target LUFS, a −1 dBTP
true-peak limiter, fades) to a downloadable 24-bit / 48 kHz WAV with a quality
report. This is the right path for a quick, release-ready single file with no DAW —
it renders the actual browser synthesis rather than recording it.

The higher-ceiling route, for a deliberately produced release, is **stems + MIDI for
a DAW**. It builds on the same split:

- `src/music.rs` is pure Rust and already produces `DroneTarget`, `TextureTarget`,
  and `NoteEvent` values; `src/mastering.rs` is the pure render-side DSP.
- `src/audio.rs` holds the Web Audio rendering (live and the offline export).

A native audio renderer would consume the same `GalaxyState` timeline as the video
export and write separate WAV stems plus a MIDI/automation sidecar:

```text
cargo run --release --bin render_audio -- \
  --timeline timelines/youtube-hero.toml \
  --out renders/youtube-hero/audio \
  --sample-rate 48000 \
  --stems
```

Recommended output:

- `drone.wav`
- `notes.wav`
- `noise.wav`
- `delay_return.wav`
- `reverb_return.wav`
- `master_reference.wav`
- `events.mid` or `events.json`, optional, for Logic editing
- `automation.json`, optional, for cutoff, reverb, delay, core activity, and other
  useful control curves

Use 48 kHz WAV. For Logic, 24-bit PCM is fine; 32-bit float is better for stems
because it preserves headroom and avoids accidental clipping during later
processing.

This will lead to better results than browser capture because:

- Logic receives editable stems rather than one mixed browser track.
- There is no browser resampling, device output processing, or screen-recorder
  audio compression.
- The audio can be rendered exactly to the video duration and sample rate.
- A sync marker or shared timeline can make picture/audio alignment exact.

The first implementation does not need a perfect clone of every Web Audio detail.
A useful MVP is:

1. Export note events as MIDI/JSON.
2. Export drone/effect automation curves.
3. Render a rough reference WAV.
4. Finish the actual sound design in Logic.

The full implementation can later add a Rust DSP renderer for oscillators,
envelopes, panning, delay, procedural reverb, noise bed, filters, and compression
to match the in-browser sound more closely.

## Sync

The video and audio exporters should use the same timeline file and duration.
Avoid free-running wall-clock time.

Useful production details:

- Start timecode at `00:00:00:00`.
- Add an optional one-frame visual flash and short click/beep before the creative
  start, then trim it out after sync.
- Store render metadata with the output: git commit, timeline file hash,
  resolution, fps, sample rate, scenario, seed, and render machine/GPU.
- Render a short 10-second preview before committing to a full-length 4K render.

## Suggested Implementation Phases

### Phase 1: Recording Mode

Add a browser-facing production mode:

- hide controls and feedback UI;
- disable idle UI;
- lock camera path/autopilot;
- expose deterministic timeline presets;
- add a clean start/end fade.

This still uses browser capture, but makes the captured result much cleaner.

### Phase 2: Audio Export

The in-app **Record → Export** path already renders a mastered 24-bit / 48 kHz WAV
of the actual synthesis (offline render + `mastering.rs`), which covers a quick
release-ready single file. What remains is the DAW route for a produced release:

- render note events and drone/texture automation from a timeline;
- write 48 kHz WAV stems and MIDI/JSON events;
- import stems into Logic for sound design and mastering.

This gives the biggest audio quality and workflow improvement for the least
engineering risk.

### Phase 3: Headless Video Export

Add a native `wgpu` renderer:

- offscreen texture target;
- fixed timeline stepping;
- frame readback;
- PNG/TIFF sequence output;
- `ffmpeg` assembly into ProRes/DNxHR/H.265.

This gives the best visual master, but is more engineering work than audio export.

### Phase 4: One-Command Production Render

Wrap the pipeline:

```text
npm run render:youtube -- timelines/youtube-hero.toml
```

Expected outputs:

```text
renders/youtube-hero/
├── frames/
├── audio/
├── galacto-youtube-hero-master.mov
├── galacto-youtube-hero-upload.mp4
└── render-metadata.json
```

## Current Recommendation

For the next video, do both in this order:

1. Use `npm run video:capture` for a short proof cut to settle duration, camera
   path, and title/credit treatment.
2. Add opening/end text with `npm run video:captions` and check the result on a
   few representative frames.
3. Bring cleaned 48 kHz stems into Logic and master there; replace the video's
   delivery audio only after the Logic export is final.
4. Build the native audio exporter next, so future renders produce stems and
   events from the same timeline without a temporary proof-render script.
5. Build the headless video exporter if the proof cut looks good enough to justify
   a polished YouTube release.

Direct audio and direct video export should produce better final results than a
screen capture, but the best near-term return is audio export plus a clean
recording mode. Headless video export is the higher-quality long-term path.

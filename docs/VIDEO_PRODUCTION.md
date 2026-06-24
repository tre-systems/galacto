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

Direct audio export is also worth doing, but it should not try to record the Web
Audio output from the browser. The better approach is to render the music engine
offline.

The current split helps:

- `src/music.rs` is pure Rust and already produces `DroneTarget` and `NoteEvent`
  values.
- `src/audio.rs` is browser-specific Web Audio rendering.

For production, add a native audio renderer that consumes the same `GalaxyState`
timeline as the video export and writes WAV files directly:

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

Add a native audio/export tool from `music.rs`:

- render note events and drone automation from a timeline;
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

1. Create a short browser-captured proof cut to settle duration, camera path, and
   title/credit treatment.
2. Build the audio export first, so Logic gets stems and events.
3. Build the headless video exporter if the proof cut looks good enough to justify
   a polished YouTube release.

Direct audio and direct video export should produce better final results than a
screen capture, but the best near-term return is audio export plus a clean
recording mode. Headless video export is the higher-quality long-term path.

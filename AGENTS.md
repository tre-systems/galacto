# Agent Notes

Operational guidance for Claude Code and other repo agents.

## Project

galacto is a browser-based **self-gravitating N-body** galaxy sandbox: 16,384 bodies by default (adjustable up to 10× via the body-count slider) attract each other through an all-pairs gravity sum that runs entirely on the GPU (WebGPU **compute** shaders, workgroup-tiled), drawn with one instanced **billboard** draw. Rust → WebAssembly (single-threaded), `wgpu`/WebGPU, deployed to Cloudflare Pages at [galacto.org](https://galacto.org/). See the [README](README.md) for features and the scenarios.

Read these before substantial work:

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — how the code is organized and how one frame is produced.
- [docs/diagrams/README.md](docs/diagrams/README.md) — the system-overview, frame-loop, and GPU-buffer diagrams.
- [BACKLOG.md](BACKLOG.md) — ordered next work and known constraints.

## Workflow

- Work directly on `main`.
- Check `git status` before editing; preserve unrelated local changes.
- For user-visible code changes the standing flow is: commit, push, watch CI, then smoke-test the live site. Docs-only changes just need commit + push.

## Verification

Standard gate — mirrors CI (`.github/workflows/ci-cd.yml`) and the `.husky/pre-commit` hook exactly:

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo check --target wasm32-unknown-unknown
```

- Web build: `npm run build` (wasm-pack `--target web`, then copies `static/` into `pkg/` and runs `scripts/cache-bust.mjs` to stamp the JS/CSS `?v=<git-sha>` and the service worker's per-deploy cache name).
- Brand rasters: `npm run icons` rasterises `assets/icons/*.svg` → `static/icons/*.png` and `assets/og-card.svg` → `static/og-card.png` (committed; needs librsvg's `rsvg-convert` on PATH — `brew install librsvg`). Re-run after editing those SVGs.
- Local run: `npm run dev` (builds, then serves `pkg/` on port 8000). Needs a WebGPU-capable browser.
- Diagrams: `npm run diagrams` to render, `npm run check:diagrams` to verify (needs Graphviz on PATH — `brew install graphviz`).
- Never bypass the hook with `--no-verify` unless explicitly asked.

## Architecture Rules

- Keep the three concerns separate: **simulation** (GPU compute), **rendering** (GPU draw), and **input/camera** (CPU).
- All physics lives in `src/shaders/update.wgsl` (the `compute_accel` and `integrate` kernels). The CPU never touches per-body state — the particle and accel buffers are GPU-resident and never read back, with one narrow, sanctioned exception: a throttled, async reduction (`reduce_core`) reads back a tiny aggregate (windowed central mass + radial flux, a few floats) purely to drive the audio. It never feeds the simulation, stays off the per-frame hot path (one readback in flight at a time), and must not grow into per-body readback.
- Gravity is all-pairs and self-gravitating (every body attracts every other), evaluated with workgroup-shared tiles. The body count is adjustable (default `NUM_PARTICLES`, up to `MAX_PARTICLES` = 10×) from the body-count slider: the GPU buffers are allocated once at the maximum and only the active `count` is dispatched, drawn, and read back. Every count must stay a multiple of `WORKGROUP_SIZE` (the tile size) so the tile loop never reads out of bounds — `clamp_particle_count` enforces this. Per-body mass scales as `NUM_PARTICLES / count` in `scenarios.rs` so changing the count refines the same galaxy rather than changing its total mass.
- Each fixed step is a drift–kick–drift leapfrog of three compute passes — `drift_half` (half-step drift), `compute_accel` (all-pairs gravity at the midpoint → accel buffer, plus the static halo force and a Chandrasekhar **dynamical-friction** drag against the halo, ∝ each body's mass so only the heavy cores feel it), then `kick_drift_half` (kick + second half-drift, in place); the render pass then reads the particle buffer. One draw per frame.
- Tunable constants split by concern: the core solver's (`G`, the halo's `HALO_V0` / `HALO_RC` / `NFW_RS` / `NFW_G_MAX`, `FIXED_DT`, surfaced to the shader via the `SimulationParams` uniform) in `src/simulation.rs`; the scenario / initial-condition ones (`BULGE_SCALE`, `STAR_MASS`, `DISK_RD`, the per-scenario softenings, the disk stability — `DEFAULT_TEMP` is the default Toomre Q, `toomre_sigma` sets the spiral disk's dispersion from it (with the `TOOMRE_SOFT` softening/thickness correction), `DISP_FRAC` scales the merger disks) in `src/scenarios.rs`. Keep them there rather than scattering magic numbers across the shaders or generators.
- Keep modules small and single-purpose. Match the existing split rather than growing `lib.rs`.
- The soundscape is driven by a `GalaxyState` assembled each frame from the camera (zoom, rotation speed, and orbit angle), the live sim knobs, and the galaxy's own core dynamics from the `reduce_core` readback — central mass + radial flux + churn, plus a *coherence* derived from them (organized collapse vs. random churn) — so it reacts to what's on screen, not just the controls. Those signals are slew-limited (`ease_slew` in `lib.rs`) so the sound always glides and never lurches. It's tuned for calm and serene awe per the relaxation research — a slow/sparse note grid (`step_seconds`, ~50–85 BPM), soft note onsets, consonant scales, and a ~0.1 Hz breathing-pacer swell on the bed (`breathing()` in `audio.rs`), with vastness (sub-bass + cavernous reverb + shimmer) carrying the awe; prefer slow expansion over abrupt dynamics. Keep the generative logic (`music.rs`, which emits a `DroneTarget` pad, a `TextureTarget` for the surrounding space, and `NoteEvent`s) free of web/audio dependencies so it stays native-testable; keep all Web Audio (`web-sys`) in `audio.rs`. All sound is synthesized — no sample/wave files or external audio sources. The WAV export follows the same discipline: `AppState` keeps a rolling capture of the `GalaxyState` timeline, `audio::render_offline` replays it through the shared `Graph` on an `OfflineAudioContext`, and the mastering/analysis/WAV-encode DSP lives in the pure, native-tested `mastering.rs` (no web/audio deps).

## Code Map

- WASM entry + render loop: `src/lib.rs` (`AppState` owns everything; `#[wasm_bindgen(start)]`; `requestAnimationFrame` drives `update` then `update_audio` then `render`; the fixed-step accumulator scales via `set_speed`; `set_gravity` / `set_halo` / `set_particle_size` / `set_halo_visible` tweak the running sim live (rewrite a uniform / toggle the halo overlay, no re-seed); `set_scenario` / `set_halo_profile` / `restart` / `set_particle_count` re-seed; `set_disk_temperature` stages the next seed; `set_sound_enabled` / `set_volume` / `set_muted` drive the soundscape (auto-started on first interaction)).
- WebGPU setup: `src/graphics.rs` (instance → adapter → device/queue → surface config; `resize`, `reconfigure`). No depth buffer — the renderer is additive and order-independent.
- Simulation: `src/simulation.rs` (particle/accel/params/camera buffers, the accel + drift + kick compute pipelines and the render pipeline, bind groups, `reseed`, `compute_pass` / `render_pass`, `update_camera`; the optional dark-matter halo overlay — `halo_pipeline` + `halo_viz_buffer` + `update_halo_view` / `render_halo`, one additive billboard at the origin; plus the audio core-statistics reduction — `reduce_pipeline`, the reductions + mappable staging buffers, `record_core_reduction` / `map_core_readback` (async, wasm-only) / `core_stats`).
- Scenarios / initial conditions: `src/scenarios.rs` (the `Scenario` enum — spiral disk, the multi-galaxy collisions, and the M51 grand-design flyby — built from `seed_spiral_disk` (a halo-supported exponential disk) and `seed_galaxy` (a compact self-bound galaxy) via the shared `push_disk_star`; `circular_velocity` balances disks against the active halo; consumed by `Simulation::reseed`).
- Camera: `src/camera.rs` (orbit camera — scale + rotation; `build_view_projection_matrix`).
- Input: `src/input.rs` (mouse, wheel, touch/pinch, keyboard → camera; pause/reset).
- Units (display only): `src/units.rs` (physical-scale calibration — kpc / km·s⁻¹ / Myr per sim unit, anchored at 0.1 kpc and a 220 km/s halo asymptote; `G = 1` in the sim, so the solver never sees these). Feeds the rotation-curve overlay (`rotation_curve` export → `scenarios::rotation_components`, the disk+bulge+halo decomposition under the live gravity/halo) and the elapsed-time clock (`elapsed_myr`, from `AppState::sim_time`).
- Helpers: `src/utils.rs` (`set_panic_hook`, `console_log!`).
- Core error type: `src/error.rs` (`AppError`); only `lib.rs` converts it to `JsValue` at the wasm-bindgen boundary.
- Post-processing: `src/postprocess.rs` (HDR `rgba16float` scene target + bloom — bright-pass, separable blur, tonemapped composite; rebuilt on resize). Owned by `AppState`, run after the particle pass each frame.
- Music (pure): `src/music.rs` (`MusicEngine` + the `GalaxyState` snapshot → a `DroneTarget` pad (with sub-bass), a `TextureTarget` (starfield, shimmer, reverb/echo/noise levels, pad resonance, orbit-driven stereo bias), and a stream of `NoteEvent`s; maps the visuals to a cosmic ambient soundscape; no web/audio deps, so it unit-tests natively like `scenarios.rs`).
- Audio (Web Audio): `src/audio.rs` (the synthesized layered node graph as a context-agnostic `Graph` — a hard-panned detuned drone pad, an octave-below sub-bass sine, a high twinkling starfield (per-voice LFOs), per-note oscillators, a camera-orbit stereo panner over the pad + starfield, three sends — a procedurally-generated convolver reverb with early reflections, a feedback delay, and a 4×-oversampled octave-up shimmer (a `2x²−1` waveshaper into the reverb) — and a compressor. The `AudioEngine` drives it live (look-ahead scheduler on the AudioContext clock; owned by `AppState` as an `Option`, built lazily on first enable so the context starts inside the user gesture); `render_offline` drives the *same* `Graph` on an `OfflineAudioContext` from a recorded timeline for the WAV export. No sample files).
- Mastering (pure): `src/mastering.rs` (offline mastering + analysis DSP on planar stereo `f32` — subsonic high-pass, mono-bass sum, ITU-R BS.1770 integrated-loudness measurement + normalisation to a target LUFS, a look-ahead true-peak limiter to a -1 dBTP ceiling, raised-cosine fades, 24-bit WAV encoding, and a `MasterReport`. No web/audio deps, so it unit-tests natively).
- Shaders: `src/shaders/update.wgsl` (compute: tiled all-pairs self-gravity + halo + Chandrasekhar dynamical friction + leapfrog integration, in three kernels — `drift_half`, `compute_accel`, `kick_drift_half` (which also dissipates the gas: `vel.w > 0.5` flags gas in `has_gas` scenarios, damped toward circular orbits by `GAS_DAMP`) — plus `reduce_core`, the workgroup tree-reduction of windowed central mass + radial flux for the audio; helpers `halo_vc_sq` / `erf_approx` support the friction term), `src/shaders/render.wgsl` (vertex: project + colour — spiral by live radius (gas, `vel.w > 0.5`, drawn blue), merger by `vel.w` galaxy tint; fragment: brightness/glow), `src/shaders/halo.wgsl` (the optional dark-matter halo overlay: one camera-facing billboard at the origin with a soft radial violet falloff, additive), `src/shaders/post.wgsl` (fullscreen bright-pass / separable blur / tonemap composite).
- Frontend: `static/index.html` (WebGPU support check, loading/error UI, WASM bootstrap, control wiring: scenario dropdown + a restart icon, body-count / speed (reads Myr/s) / disk-temp (Toomre Q) / gas-fraction / bulge-fraction / gravity / star-size / volume sliders, mute button, a grouped **Dark matter halo** section (model dropdown + strength slider (reads km/s) + a Size slider for the scale radius (`set_halo_concentration`, reads kpc) + a Show toggle that drives `set_halo_visible`, and a Curve toggle for the rotation-curve overlay — a `<canvas>` chart fed by the `rotation_curve` / `elapsed_myr` exports, redrawn on gravity/halo/size/bulge changes), a `setupInfoButtons` pass that adds a "?" by each control label (descriptions in one map, a shared popover), a Sentry feedback button (hidden until `sentry.js` attaches the form — `static/sentry.js` loads the feedback-capable SDK bundle and calls `attachTo` only when a DSN is configured), and a draggable cog toggle pinned top-left (the chrome is on the body, so the cog never shifts when the panel opens/closes; position is not persisted) that also closes on an outside click or a scenario switch; a separate **Studio export** panel (`#export-panel`, wired by `setupExport`) that the init script reveals only when served locally (`isLocalHost()`) — a Record toggle (`set_recording`), a loudness select, and an Export button that calls the async `export_audio` and turns the returned WAV bytes into a download with a quality report — so the authoring tool never ships to the public site; the soundscape auto-starts on the visitor's first interaction; registers the service worker), `static/styles.css`.
- PWA: `static/site.webmanifest` (+ an identical `manifest.json`), `static/sw.js` (the service worker: precaches the app shell, network-first navigation, stale-while-revalidate assets; `__CACHE_BUST__` → per-deploy cache name), and `static/icons/*.png`. The app installs to the home screen and launches offline. A new worker does **not** `skipWaiting` on its own — it waits, the page shows an "update available" toast (`#update-toast` in `index.html`), and only on the user's click does the page post `SKIP_WAITING` and reload. Social/SEO: `static/og-card.png` + Open Graph / Twitter / canonical meta in `index.html` (canonical domain `galacto.org`) and `static/sitemap.xml`. Brand PNGs come from `assets/*.svg` via `scripts/gen-icons.mjs`.

## Tests

- The crate is `cdylib` + `rlib`, so `cargo test` runs native unit tests with no GPU. Coverage: the `Particle` / `SimulationParams` buffer-layout contract and tile-count invariant (`src/simulation.rs`), camera math (zoom/rotation clamps, reset, finite matrix — `src/camera.rs`), scenario seeding (body count, positive mass, finiteness, determinism, temperature scaling — `src/scenarios.rs`), the generative music engine (note well-formedness, density-tracks-activity, paused-is-silent, per-scenario drone, determinism — `src/music.rs`), and the offline mastering DSP (WAV header/round-trip, BS.1770 loudness monotonicity, gain-hits-target, true-peak ceiling held, mono-bass coherence — `src/mastering.rs`).
- Pure CPU logic only; the GPU pipeline and DOM wiring are not unit-tested. A headless step mode for the sim is a [BACKLOG](BACKLOG.md) item.

## Commits

- Keep messages short and outcome-focused; reference `file.rs:line` where it helps a reader.
- Stage explicit paths.
- On a pre-commit hook failure, fix the issue and make a NEW commit — do not blind-`--amend`; the failed commit did not happen.

## Code Style

- Files preferably under ~200 lines; functions short and focused.
- `rustfmt` + `clippy` clean: fix all warnings. Write idiomatic, concise Rust.
- Organize code by concern (graphics / simulation / camera / input), not by technical layer.

## Docs

- Docs describe the current state in the present tense. Keep history in git, not in docs.
- Diagrams: Graphviz `.dot` rendered to a committed PNG (`npm run diagrams`) for standalone architecture and flow diagrams; Mermaid inline in Markdown for small ones. See [docs/diagrams/README.md](docs/diagrams/README.md).
- Add a BACKLOG item for useful intent that should not be built immediately.

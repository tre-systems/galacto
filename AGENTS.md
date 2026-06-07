# Agent Notes

Operational guidance for Claude Code and other repo agents.

## Project

galacto is a browser-based **self-gravitating N-body** galaxy sandbox: ~16,000 bodies attract each other through an all-pairs gravity sum that runs entirely on the GPU (WebGPU **compute** shaders, workgroup-tiled), drawn with one instanced **billboard** draw. Rust → WebAssembly (single-threaded), `wgpu`/WebGPU, deployed to Cloudflare Pages at [galacto.tre.systems](https://galacto.tre.systems/). See the [README](README.md) for features and the scenarios.

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

- Web build: `npm run build` (wasm-pack `--target web`, then copies `static/` into `pkg/` and runs `scripts/cache-bust.mjs` to stamp the JS import with `?v=<git-sha>`).
- Local run: `npm run dev` (builds, then serves `pkg/` on port 8000). Needs a WebGPU-capable browser.
- Diagrams: `npm run diagrams` to render, `npm run check:diagrams` to verify (needs Graphviz on PATH — `brew install graphviz`).
- Never bypass the hook with `--no-verify` unless explicitly asked.

## Architecture Rules

- Keep the three concerns separate: **simulation** (GPU compute), **rendering** (GPU draw), and **input/camera** (CPU).
- All physics lives in `src/shaders/update.wgsl` (the `compute_accel` and `integrate` kernels). The CPU never touches per-body state — the particle and accel buffers are GPU-resident and never read back, with one narrow, sanctioned exception: a throttled, async reduction (`reduce_core`) reads back a tiny aggregate (windowed central mass + radial flux, a few floats) purely to drive the audio. It never feeds the simulation, stays off the per-frame hot path (one readback in flight at a time), and must not grow into per-body readback.
- Gravity is all-pairs and self-gravitating (every body attracts every other), evaluated with workgroup-shared tiles. `NUM_PARTICLES` must stay a multiple of `WORKGROUP_SIZE` (the tile size) so the tile loop never reads out of bounds.
- Each fixed step is a drift–kick–drift leapfrog of three compute passes — `drift_half` (half-step drift), `compute_accel` (all-pairs gravity at the midpoint → accel buffer), then `kick_drift_half` (kick + second half-drift, in place); the render pass then reads the particle buffer. One draw per frame.
- Tunable constants split by concern: the core solver's (`G`, the halo's `HALO_V0` / `HALO_RC` / `NFW_RS` / `NFW_G_MAX`, `FIXED_DT`, surfaced to the shader via the `SimulationParams` uniform) in `src/simulation.rs`; the scenario / initial-condition ones (`BULGE_MASS`, `STAR_MASS`, `DISK_RD`, the per-scenario softenings, the disk-temperature `DISP_FRAC` / `DEFAULT_TEMP`) in `src/scenarios.rs`. Keep them there rather than scattering magic numbers across the shaders or generators.
- Keep modules small and single-purpose. Match the existing split rather than growing `lib.rs`.
- The soundscape is driven by a `GalaxyState` assembled each frame from the camera, the live sim knobs, and the galaxy's own core dynamics (central mass + radial flux) from the `reduce_core` readback — so it reacts to what's on screen, not just the controls. Those signals are slew-limited (`ease_slew` in `lib.rs`) so the sound always glides and never lurches. Keep the generative logic (`music.rs`) free of web/audio dependencies so it stays native-testable; keep all Web Audio (`web-sys`) in `audio.rs`. All sound is synthesized — no sample/wave files or external audio sources.

## Code Map

- WASM entry + render loop: `src/lib.rs` (`AppState` owns everything; `#[wasm_bindgen(start)]`; `requestAnimationFrame` drives `update` then `update_audio` then `render`; the fixed-step accumulator scales via `set_speed`; `set_gravity` / `set_halo` / `set_particle_size` tweak the running sim live (rewrite the uniform, no re-seed); `set_scenario` / `set_halo_profile` / `restart` re-seed; `set_disk_temperature` stages the next seed; `set_sound_enabled` switches the soundscape on (the page calls it on first interaction)).
- WebGPU setup: `src/graphics.rs` (instance → adapter → device/queue → surface config; `resize`, `reconfigure`). No depth buffer — the renderer is additive and order-independent.
- Simulation: `src/simulation.rs` (particle/accel/params/camera buffers, the accel + drift + kick compute pipelines and the render pipeline, bind groups, `reseed`, `compute_pass` / `render_pass`, `update_camera`; plus the audio core-statistics reduction — `reduce_pipeline`, the reductions + mappable staging buffers, `record_core_reduction` / `map_core_readback` (async, wasm-only) / `core_stats`).
- Scenarios / initial conditions: `src/scenarios.rs` (the `Scenario` enum — spiral disk, the multi-galaxy collisions, and the M51 grand-design flyby — built from `seed_spiral_disk` (a halo-supported exponential disk) and `seed_galaxy` (a compact self-bound galaxy) via the shared `push_disk_star`; `circular_velocity` balances disks against the active halo; consumed by `Simulation::reseed`).
- Camera: `src/camera.rs` (orbit camera — scale + rotation; `build_view_projection_matrix`).
- Input: `src/input.rs` (mouse, wheel, touch/pinch, keyboard → camera; pause/reset).
- Helpers: `src/utils.rs` (`set_panic_hook`, `console_log!`).
- Core error type: `src/error.rs` (`AppError`); only `lib.rs` converts it to `JsValue` at the wasm-bindgen boundary.
- Post-processing: `src/postprocess.rs` (HDR `rgba16float` scene target + bloom — bright-pass, separable blur, tonemapped composite; rebuilt on resize). Owned by `AppState`, run after the particle pass each frame.
- Music (pure): `src/music.rs` (`MusicEngine` + the `GalaxyState` snapshot → a `DroneTarget` pad and a stream of `NoteEvent`s; maps the visuals to a cosmic ambient soundscape; no web/audio deps, so it unit-tests natively like `scenarios.rs`).
- Audio (Web Audio): `src/audio.rs` (`AudioEngine` — the synthesized node graph: a detuned drone pad + per-note oscillators, a procedurally-generated reverb impulse, a feedback delay, and a compressor, plus a look-ahead scheduler on the AudioContext clock. No sample files. Owned by `AppState` as an `Option`, built lazily on first enable so the context starts inside the user gesture).
- Shaders: `src/shaders/update.wgsl` (compute: tiled all-pairs self-gravity + halo + leapfrog integration, in three kernels — `drift_half`, `compute_accel`, `kick_drift_half` — plus `reduce_core`, the workgroup tree-reduction of windowed central mass + radial flux for the audio), `src/shaders/render.wgsl` (vertex: project + colour — spiral by live radius, merger by `vel.w` galaxy tint; fragment: brightness/glow), `src/shaders/post.wgsl` (fullscreen bright-pass / separable blur / tonemap composite).
- Frontend: `static/index.html` (WebGPU support check, loading/error UI, WASM bootstrap, control wiring: scenario and halo-model dropdowns, speed / disk-temp / gravity / halo / star-size sliders, restart + panel toggle; the soundscape auto-starts on the visitor's first interaction), `static/styles.css`.

## Tests

- The crate is `cdylib` + `rlib`, so `cargo test` runs native unit tests with no GPU. Coverage: the `Particle` / `SimulationParams` buffer-layout contract and tile-count invariant (`src/simulation.rs`), camera math (zoom/rotation clamps, reset, finite matrix — `src/camera.rs`), scenario seeding (body count, positive mass, finiteness, determinism, temperature scaling — `src/scenarios.rs`), and the generative music engine (note well-formedness, density-tracks-activity, paused-is-silent, per-scenario drone, determinism — `src/music.rs`).
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

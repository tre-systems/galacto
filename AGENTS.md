# Agent Notes

Operational guidance for Claude Code and other repo agents.

## Project

galacto is a browser-based **self-gravitating N-body** galaxy sandbox: ~16,000 bodies attract each other through an all-pairs gravity sum that runs entirely on the GPU (WebGPU **compute** shaders, workgroup-tiled), drawn with one instanced **billboard** draw. Rust → WebAssembly (single-threaded), `wgpu`/WebGPU, deployed to Cloudflare Pages at [galacto.tre.systems](https://galacto.tre.systems/). See the [README](README.md) for features and the two scenarios.

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
- All physics lives in `src/shaders/update.wgsl` (the `compute_accel` and `integrate` kernels). The CPU never touches per-body state — the particle and accel buffers are GPU-resident and never read back.
- Gravity is all-pairs and self-gravitating (every body attracts every other), evaluated with workgroup-shared tiles. `NUM_PARTICLES` must stay a multiple of `WORKGROUP_SIZE` (the tile size) so the tile loop never reads out of bounds.
- Each fixed step is a drift–kick–drift leapfrog of three compute passes — `drift_half` (half-step drift), `compute_accel` (all-pairs gravity at the midpoint → accel buffer), then `kick_drift_half` (kick + second half-drift, in place); the render pass then reads the particle buffer. One draw per frame.
- Tunable constants split by concern: the core solver's (`G`, the halo's `HALO_V0` / `HALO_RC` / `NFW_RS` / `NFW_G_MAX`, `FIXED_DT`, surfaced to the shader via the `SimulationParams` uniform) in `src/simulation.rs`; the scenario / initial-condition ones (`BULGE_MASS`, `STAR_MASS`, `DISK_RD`, the per-scenario softenings, the disk-temperature `DISP_FRAC` / `DEFAULT_TEMP`) in `src/scenarios.rs`. Keep them there rather than scattering magic numbers across the shaders or generators.
- Keep modules small and single-purpose. Match the existing split rather than growing `lib.rs`.

## Code Map

- WASM entry + render loop: `src/lib.rs` (`AppState` owns everything; `#[wasm_bindgen(start)]`; `requestAnimationFrame` drives `update` then `render`; the fixed-step accumulator scales via `set_speed`; `set_gravity` / `set_halo` / `set_particle_size` tweak the running sim live (rewrite the uniform, no re-seed); `set_scenario` / `set_halo_profile` / `restart` re-seed; `set_disk_temperature` stages the next seed).
- WebGPU setup: `src/graphics.rs` (instance → adapter → device/queue → surface config; `resize`, `reconfigure`). No depth buffer — the renderer is additive and order-independent.
- Simulation: `src/simulation.rs` (particle/accel/params/camera buffers, the accel + drift + kick compute pipelines and the render pipeline, bind groups, `reseed`, `compute_pass` / `render_pass`, `update_camera`).
- Scenarios / initial conditions: `src/scenarios.rs` (`Scenario` (Spiral / Merger) with `generate_disk` / `generate_merger`, the shared `push_disk_star` disk seeder, and `circular_velocity`; consumed by `Simulation::reseed`).
- Camera: `src/camera.rs` (orbit camera — scale + rotation; `build_view_projection_matrix`).
- Input: `src/input.rs` (mouse, wheel, touch/pinch, keyboard → camera; pause/reset).
- Helpers: `src/utils.rs` (`set_panic_hook`, `console_log!`).
- Core error type: `src/error.rs` (`AppError`); only `lib.rs` converts it to `JsValue` at the wasm-bindgen boundary.
- Post-processing: `src/postprocess.rs` (HDR `rgba16float` scene target + bloom — bright-pass, separable blur, tonemapped composite; rebuilt on resize). Owned by `AppState`, run after the particle pass each frame.
- Shaders: `src/shaders/update.wgsl` (compute: tiled all-pairs self-gravity + halo + leapfrog integration, in three kernels — `drift_half`, `compute_accel`, `kick_drift_half`), `src/shaders/render.wgsl` (vertex: project + colour — spiral by live radius, merger by `vel.w` galaxy tint; fragment: brightness/glow), `src/shaders/post.wgsl` (fullscreen bright-pass / separable blur / tonemap composite).
- Frontend: `static/index.html` (WebGPU support check, loading/error UI, WASM bootstrap, control wiring: scenario and halo-model dropdowns, speed / disk-temp / gravity / halo / star-size sliders, restart + panel toggle), `static/styles.css`.

## Tests

- The crate is `cdylib` + `rlib`, so `cargo test` runs native unit tests with no GPU. Coverage: the `Particle` / `SimulationParams` buffer-layout contract and tile-count invariant (`src/simulation.rs`), camera math (zoom/rotation clamps, reset, finite matrix — `src/camera.rs`), and scenario seeding (body count, positive mass, finiteness, determinism, temperature scaling — `src/scenarios.rs`).
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

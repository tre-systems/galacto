# Agent Notes

Operational guidance for Claude Code and other repo agents.

## Project

galacto is a browser-based **restricted N-body** galaxy-interaction simulation. Two massive cores move under their mutual gravity while ~131,000 massless test stars fall through their combined, Plummer-softened field — producing tidal tails and spiral arms. The cores and stars are advanced entirely on the GPU through WebGPU **compute** shaders, and the stars are drawn with a single instanced **billboard** draw. The core is Rust compiled to WebAssembly (single-threaded), rendered with `wgpu`/WebGPU, and deployed as a static site to Cloudflare Pages at [galacto.tre.systems](https://galacto.tre.systems/).

Read these before substantial work:

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — how the code is organized and how one frame is produced.
- [docs/diagrams/README.md](docs/diagrams/README.md) — the system-overview and frame-loop diagrams.
- [BACKLOG.md](BACKLOG.md) — ordered next work and known constraints.

## Workflow

- Work directly on `main`.
- Check `git status` before editing; preserve unrelated local changes.
- Stage explicit file paths, not `git add -A` / `git add .`.
- For user-visible code changes the standing flow is: commit, push, watch CI, then smoke-test the live site. Docs-only changes just need commit + push.

## Verification

Standard gate — mirrors CI (`.github/workflows/ci-cd.yml`) and the `.husky/pre-commit` hook exactly:

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo check --target wasm32-unknown-unknown
```

- Web build: `npm run build` (wasm-pack `--target web`, then copies `static/` into `pkg/`).
- Local run: `npm run dev` (builds, then serves `pkg/` on port 8000). Needs a WebGPU-capable browser.
- Diagrams: `npm run diagrams` to render, `npm run check:diagrams` to verify (needs Graphviz on PATH — `brew install graphviz`).
- Never bypass the hook with `--no-verify` unless explicitly asked.

## Architecture Rules

- Keep the three concerns separate: **simulation** (GPU compute), **rendering** (GPU draw), and **input/camera** (CPU).
- All physics lives in `src/shaders/update.wgsl` (the `update_cores` and `update_particles` kernels). The CPU never touches per-body state — the particle and core buffers are GPU-resident and never read back.
- Each fixed step runs two compute passes — cores first, then test particles — writing the buffers in place; the render pass then reads the particle buffer. One draw per frame.
- Tunable simulation constants live in `src/simulation.rs` (module consts like `G`, `CORE_MASS`, `SOFTENING`, surfaced to the shader via the `SimulationParams` uniform); keep them there rather than scattering magic numbers across the shader.
- Keep modules small and single-purpose. Match the existing split rather than growing `lib.rs`.

## Code Map

- WASM entry + render loop: `src/lib.rs` (`AppState` owns everything; `#[wasm_bindgen(start)]`; `requestAnimationFrame` drives `update` then `render`).
- WebGPU setup: `src/graphics.rs` (instance → adapter → device/queue → surface config → depth texture; `resize`).
- Simulation: `src/simulation.rs` (particle/core/params/camera buffers, cores + particle compute pipelines and the render pipeline, bind groups, two-galaxy generation, `compute_pass` / `render_pass`).
- Camera: `src/camera.rs` (orbit camera — position, scale, rotation; `build_view_projection_matrix`).
- Input: `src/input.rs` (mouse, wheel, touch/pinch, keyboard → camera; pause/reset).
- Helpers: `src/utils.rs` (`set_panic_hook`, `console_log!`).
- Core error type: `src/error.rs` (`AppError`); only `lib.rs` converts it to `JsValue` at the wasm-bindgen boundary.
- Shaders: `src/shaders/update.wgsl` (compute: softened core + test-particle gravity, symplectic integration), `src/shaders/render.wgsl` (vertex: project + per-galaxy color; fragment: brightness/glow).
- Frontend: `static/index.html` (WebGPU support check, loading/error UI, WASM bootstrap), `static/styles.css`.

## Tests

- There are no tests yet. The crate is `cdylib`-only, which makes native unit tests awkward; `cargo test` runs clean with 0 tests. Adding an `rlib` crate-type to enable testing camera math and particle-init invariants is a [BACKLOG](BACKLOG.md) item.

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

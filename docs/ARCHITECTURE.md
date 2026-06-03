# galacto — Architecture

> Scope: how the code is organized and how one rendered frame is produced. The simulation is small — ~6 Rust modules and 2 WGSL shaders — but it is GPU-first: all per-particle physics runs in a compute shader and never touches the CPU.

![System overview](diagrams/system-overview.png)

## Stack

| Layer            | Choice                          | Notes                                                          |
| ---------------- | ------------------------------- | -------------------------------------------------------------- |
| Language         | Rust (edition 2021)             | ~700 lines across `src/`                                       |
| GPU access       | `wgpu` 24 (WebGPU)              | Compute + render pipelines; `BROWSER_WEBGPU` backend           |
| Shaders          | WGSL                            | `update.wgsl` (compute), `render.wgsl` (vertex + fragment)     |
| Math             | `cgmath`                        | Perspective + look-at for the orbit camera                     |
| WASM bindings    | `wasm-bindgen` + `web-sys`      | Canvas, events, `requestAnimationFrame`, console               |
| Build            | `wasm-pack` (`--target web`)    | Emits `pkg/galacto.js` + `galacto_bg.wasm`                     |
| Host             | Cloudflare Pages                | Serves the static `pkg/` directory                             |
| Scale            | 131,072 particles               | One compute dispatch + one instanced draw per frame            |

The toolchain is plain `stable` (`rust-toolchain.toml`) — no nightly, no `build-std`, no threads.

## Repo Layout

```
src/
├── lib.rs               # WASM entry: AppState owns graphics/sim/camera/input; rAF loop
├── graphics.rs          # WebGPU init: instance → adapter → device/queue → surface → depth texture
├── simulation.rs        # Buffers, pipelines, bind groups, particle init, compute_pass / render_pass
├── camera.rs            # Orbit camera: position, scale, rotation → view-projection matrix
├── input.rs             # Mouse / wheel / touch (pinch) / keyboard → camera; pause + reset
├── utils.rs             # set_panic_hook, console_log! macro
├── error.rs             # AppError — the core's domain error (no JsValue)
└── shaders/
    ├── update.wgsl      # Compute: gravity + Euler integration + boundary bounce
    └── render.wgsl      # Vertex (project + velocity color) + fragment (brightness/glow)
static/                  # Frontend: index.html (WebGPU check + bootstrap), styles.css, favicon.svg
pkg/                     # wasm-pack output + copied static assets — the deploy root (git-ignored)
scripts/                 # render-diagrams.mjs, check-diagrams.mjs
```

## Patterns

galacto is small, but nearly every file is an instance of one of a handful of recurring patterns. Naming them once makes the rest of the code predictable; the detailed sections below are each an elaboration of one of these.

**GPU-resident state, no readback.** After the initial upload, particle positions and velocities live only in a GPU storage buffer. The compute pass is the sole writer and the render pass the sole reader; the CPU never reads particle data back. The CPU's only per-frame writes are two small uniforms (params, camera).

**Compute-then-render over one buffer — no ping-pong.** The same particle buffer is bound `storage, read_write` to the compute pass and `storage, read` to the render pass within a single command encoder. WebGPU inserts a barrier between the passes, so the render reads exactly what the compute just wrote. Double-buffering (ping-pong) is deliberately absent: nothing reads the buffer *while* it is being written, so a second copy would buy nothing. It would only be needed if a frame both read an old generation and wrote a new one concurrently.

**Owning composition root (`AppState`).** One struct (`src/lib.rs`) owns the four subsystems — `Graphics`, `Simulation`, `Camera`, `InputHandler` — and is the only orchestrator. Each frame it calls `update()` then `render()`. Subsystems never reach for each other; they are wired together only through `AppState`.

**Single `#[wasm_bindgen(start)]` entry + self-scheduling rAF loop.** `start()` is the only WASM export. It installs the panic hook, spawns async initialization, and arms a `requestAnimationFrame` callback that re-arms itself every frame — the render loop is a tail chain of rAF calls, not a timer. The loop and the resize handler reach the app through a `thread_local!` `RefCell<Option<Rc<RefCell<AppState>>>>` — the safe single-threaded-WASM global, no `static mut`.

**POD structs mirrored Rust ↔ WGSL.** `Particle` and `SimulationParams` are `#[repr(C)]` + `bytemuck::Pod`, byte-for-byte identical to their WGSL `struct` counterparts, so they `cast_slice` straight into buffers with no serialization. Two trailing `u32` pads keep `SimulationParams` 16-byte aligned (32 bytes) for a uniform. **The Rust definition and the WGSL definition are one contract and must change together.**

**Upload-once vs upload-per-frame.** Data that is static after init — the particle buffer and the `SimulationParams` uniform (entirely constants now that `dt` is fixed) — is uploaded once at creation. Only the camera matrix changes per frame, pushed with `queue.write_buffer` into its `UNIFORM | COPY_DST` buffer.

**Labeled resources.** Every buffer, pipeline, bind group, pass, and texture carries a `label: Some(...)` so it is identifiable in browser GPU debuggers and validation messages.

**Derived visuals in-shader (single source of truth).** Color, brightness, and glow are pure functions of a particle's velocity, computed in the shaders and never stored. Position + velocity is the only state; appearance is recomputed from it each frame, so it can never drift out of sync with the simulation.

**Deferred input: accumulate, then drain.** DOM event handlers write into one shared `InputState` behind an `Rc<RefCell>` (`src/input.rs`). The frame loop reads that state once per frame: it acts on *level* state (is-rotating, is-dragging) and **drains** *edge* state — the pause/reset flags and the accumulated zoom delta are reset as they are consumed. This decouples asynchronous, bursty event delivery from the synchronous once-per-frame update.

**Retained closures keep listeners alive.** Each `add_event_listener` closure is pushed into the handler's `_closures` vector so it is not dropped at the end of setup — dropping it would silently unregister the listener.

**Display-synced canvas sizing.** A `resize` listener keeps the drawing buffer at the displayed size × `devicePixelRatio` and reconfigures the surface, depth texture, and camera aspect together (`AppState::resize`), so the view fills the window without stretching.

**Compile-time-embedded shaders.** WGSL is brought in with `include_str!`, so shaders are compiled into the WASM; there is no runtime fetch or separate asset to deploy.

**Deterministic seeded initialization.** All initial particle state is generated from a fixed RNG seed (`StdRng::seed_from_u64(42)`), so every page load produces an identical starting configuration.

**Single attractor, not N-body.** Particles are attracted only to a fixed mass at the origin — `O(N)` per step — never to each other (`O(N²)`). There is no particle–particle interaction; the disk is an emergent property of many bodies sharing one central field.

**Fixed-timestep accumulator.** The render loop runs at the display's refresh rate, but physics advances in whole `FIXED_DT` (1/60 s) steps: each frame adds the real elapsed time to an accumulator and runs as many fixed compute dispatches as have accumulated — clamped to `MAX_SUBSTEPS`, with a `MAX_FRAME_DT` clamp on the frame delta so a long stall can't spiral. Step size is independent of frame rate, so the same seed evolves identically on a 60 Hz and a 144 Hz display.

**Integrator guard-rails.** The explicit Euler step is kept stable by a softening epsilon (`r² + 1e-6`) at the singularity, a velocity clamp (`max_velocity`), and the fixed timestep itself (a bounded `dt`, never the raw frame delta). Each one stops a specific way open-form integration can blow up.

**FFI-free core with a `JsValue` boundary.** The engine modules — `simulation`, `camera`, and `graphics` — carry no `wasm_bindgen::JsValue`. `Graphics::new` (the only fallible one) returns a domain `AppError` (`src/error.rs`); `Simulation`, `Camera`, and `InputHandler` construction is infallible. `JsValue` is confined to the boundary: `lib.rs` converts `AppError` → `JsValue` in `AppState::new`, and the DOM-event wiring (`input.rs`) plus the `#[wasm_bindgen]` `start` / `run` / `render` surface return `Result<_, JsValue>`. The per-frame hot path is infallible.

## How a Frame Is Produced

![Frame loop: update then render](diagrams/frame-loop.png)

A single `requestAnimationFrame` callback (`animation_frame` in `src/lib.rs`) does two things on the shared `AppState`:

1. **`update(time)`** — let the `InputHandler` apply pending rotate/pan/zoom/reset to the `Camera`, toggle pause if Space was pressed, then add the real frame delta to the fixed-timestep accumulator and compute how many `FIXED_DT` steps to run this frame (0 when paused).
2. **`render()`** — open a command encoder, then:
   - run the **compute pass** once per scheduled step (each its own pass, so step N+1 reads step N's writes): dispatch `update_particles` over `ceil(131072 / 64) = 2048` workgroups, advancing every particle in place;
   - run the **render pass**: write the camera's view-projection matrix into the camera uniform, then issue one `draw(0..131072)` of point primitives with depth testing against a `Depth32Float` buffer;
   - submit and `present()`.

Then it schedules the next frame. The simulation state lives only in GPU memory between frames — there is no CPU-side particle array after the initial upload.

## GPU Data Model

`Simulation::new` (`src/simulation.rs`) creates three buffers and two pipelines:

| Resource          | Contents                                          | Usage                                  |
| ----------------- | ------------------------------------------------- | -------------------------------------- |
| Particle buffer   | `131072 × Particle` (`position[3]`, `velocity[3]` — 24 B each, ~3.1 MB) | `STORAGE \| VERTEX \| COPY_DST` |
| Params buffer     | `SimulationParams { dt, gm, max_velocity, boundary, restitution, particle_count, _pad×2 }` | `UNIFORM \| COPY_DST` |
| Camera buffer     | 4×4 view-projection matrix (64 B)                 | `UNIFORM \| COPY_DST`                  |

- **Compute bind group** (`@compute` visibility): binding 0 = particle buffer as `storage, read_write`; binding 1 = params as `uniform`.
- **Render bind group** (`@vertex` visibility): binding 0 = camera matrix as `uniform`; binding 1 = the *same* particle buffer as `storage, read`.

The particle buffer is bound as both a compute storage target and a vertex-stage storage input, so the data the compute shader just wrote is exactly what the vertex shader reads — no copies, no staging, no ping-pong. The vertex shader indexes `particles[vertex_index]` directly rather than using a vertex buffer layout.

## Simulation & Physics

All physics is in `src/shaders/update.wgsl`, driven by the `SimulationParams` uniform (`dt`, `gm`, `max_velocity`, `boundary`, `restitution`). Per particle, per step:

- **Gravity to a fixed center.** `r² = dot(pos, pos) + 1e-6` (epsilon avoids divide-by-zero at the singularity), then acceleration `a = -gm · pos / r³` toward the origin. `gm` (the gravitational parameter `G·M`) is `40000.0`.
- **Euler integration.** `velocity += a · dt`; speed is clamped to `max_velocity` (140) to keep fast particles from escaping the integrator; then `position += velocity · dt`.
- **Inelastic boundary.** At `|x|`, `|y|`, or `|z|` past `boundary` (600), the position is clamped to the wall and that velocity component is reflected and scaled by `restitution` (0.1, ≈90 % energy loss). This is a *bounce*, not an elastic collision — it bleeds energy so particles settle rather than ricochet forever.

Initial conditions (`Simulation::generate_initial_particles`, seeded `StdRng(42)` → reproducible):

- **~500 close-orbit stars** scattered in a vertically flattened sphere (radius 20–80) around the hole, each given a roughly tangential velocity `sqrt(gm / r) · 0.8` (slightly sub-orbital) so they trace short arcs near the center.
- **The remaining ~130,572 particles** are seeded as a single injected **stream** — all starting near `(10, y, 100)` with `y ∈ [−150, 150]` and a uniform `velocity = (150, 0, 0)` — which the central gravity then sweeps into the disk. This stream, not Keplerian orbits, is what produces the bulk of the motion.

## Rendering

`src/shaders/render.wgsl` draws each particle as a single GPU point:

- **Vertex** — transform `position` by the camera matrix to clip space; compute `normalized_speed = min(|velocity| / 200, 1)` and a color ramp blue (slow) → red (fast): `color = (speed·2, 0.1, 1 − speed)`.
- **Fragment** — scale color by a speed-dependent `brightness = 3 + speed·8` and add a reddish `glow`, output at alpha `0.9`.

The pipeline uses `PointList` topology, `ALPHA_BLENDING`, and depth testing (`Depth32Float`, `Less`, depth-write on) so nearer particles occlude farther ones. The pass clears to a near-black blue `(0.01, 0.01, 0.05)`.

## Camera & Input

`Camera` (`src/camera.rs`) is an orbit camera: it keeps a `scale` (zoom), `rotation_x` / `rotation_y`, and an aspect ratio, and places the eye at `distance = 800 / scale` rotated around the origin, always looking at `(0,0,0)` through a 45° perspective (near 0.1, far 5000). It starts rotated 90° horizontally and zoomed in (`scale = 3.0`); `rotation_x` is clamped to ±1.5 rad and `scale` to 0.3–5.0.

A `resize` listener on the window (`src/lib.rs`) keeps the canvas drawing buffer matched to its displayed size × `devicePixelRatio`, calling `AppState::resize` to reconfigure the surface, recreate the depth texture, and update the camera aspect — so the view fills the window at native resolution without stretching.

`InputHandler` (`src/input.rs`) registers DOM listeners and translates them into camera intent, polled once per frame:

| Input                         | Action            |
| ----------------------------- | ----------------- |
| Left-drag / one-finger drag   | Rotate (orbit)    |
| Right-drag                    | Pan               |
| Wheel / two-finger pinch      | Zoom              |
| Space                         | Pause / resume    |
| R                             | Reset camera      |

## Build & Deploy

- `npm run build` → `wasm-pack build --target web --release --out-name galacto --no-opt`, then copies `static/` into `pkg/` and runs `scripts/cache-bust.mjs`, which stamps the `galacto.js` import in `index.html` with `?v=<git-sha>` so a new deploy always loads fresh glue. Output is `pkg/` (git-ignored, regenerated).
- `npm run dev` → build, then `serve pkg -l 8000`. Open in a WebGPU-capable browser.
- `npm run deploy` → build, then `wrangler pages deploy pkg --project-name=galacto`.
- CI (`.github/workflows/ci-cd.yml`) runs the verification gate on every push/PR and deploys `pkg/` to Cloudflare Pages on push to `main`. The Pages project name lives only in the deploy command; there is no `wrangler.toml`.

## What This Architecture Deliberately Does Not Include

- **No server or persistence.** Everything runs client-side; there is no backend or save state.
- **No CPU physics.** All per-particle work is on the GPU; the CPU only sets `dt`, the camera, and pause state. The particle buffer is never read back.
- **No threads.** The WASM is single-threaded — no `rayon`, no `SharedArrayBuffer`. It therefore needs **no** cross-origin-isolation (COOP/COEP) headers; the `_headers` file only sets `Cache-Control: no-cache` so the wasm and HTML revalidate (the JS glue is cache-busted via `?v=` instead).
- **No N-body gravity.** Particles are attracted only to one fixed central mass (O(N) per step), not to each other (which would be O(N²)). There is no particle–particle interaction.
- **No WebGL fallback.** The renderer targets WebGPU; `index.html` checks for it up front and shows a "WebGPU not supported" message rather than degrading.

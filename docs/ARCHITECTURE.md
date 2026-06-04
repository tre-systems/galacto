# galacto — Architecture

> Scope: how the code is organized and how one rendered frame is produced. The simulation is small — 8 Rust modules and 3 WGSL shaders — but it is GPU-first: all physics runs in compute shaders and never touches the CPU.

![System overview](diagrams/system-overview.png)

## Stack

| Layer            | Choice                          | Notes                                                          |
| ---------------- | ------------------------------- | -------------------------------------------------------------- |
| Language         | Rust (edition 2021)             | ~1,600 lines across `src/`                                     |
| GPU access       | `wgpu` 24 (WebGPU)              | Compute + render pipelines; `BROWSER_WEBGPU` backend           |
| Shaders          | WGSL                            | `update.wgsl` (compute), `render.wgsl` (billboards), `post.wgsl` (bloom) |
| Math             | `cgmath`                        | Perspective + look-at for the orbit camera                     |
| WASM bindings    | `wasm-bindgen` + `web-sys`      | Canvas, events, `requestAnimationFrame`, console               |
| Build            | `wasm-pack` (`--target web`)    | Emits `pkg/galacto.js` + `galacto_bg.wasm`                     |
| Host             | Cloudflare Pages                | Serves the static `pkg/` directory                             |
| Scale            | 131,072 stars + 2 cores         | Two compute passes (cores, then stars) + one instanced draw per step |

The toolchain is plain `stable` (`rust-toolchain.toml`) — no nightly, no `build-std`, no threads.

## Repo Layout

```
src/
├── lib.rs               # WASM entry: AppState owns graphics/sim/camera/input; rAF loop
├── graphics.rs          # WebGPU init: instance → adapter → device/queue → surface
├── simulation.rs        # Buffers, pipelines, bind groups, two-galaxy init, compute_pass / render_pass
├── camera.rs            # Orbit camera: position, scale, rotation → view-projection matrix
├── input.rs             # Mouse / wheel / touch (pinch) / keyboard → camera; pause + reset
├── utils.rs             # set_panic_hook, console_log! macro
├── error.rs             # AppError — the core's domain error (no JsValue)
├── postprocess.rs       # HDR scene target + bloom (bright-pass, blur, tonemap composite)
└── shaders/
    ├── update.wgsl      # Compute: core + test-particle gravity, symplectic integration
    ├── render.wgsl      # Billboard vertex + radial-glow fragment (additive)
    └── post.wgsl        # Fullscreen bright-pass, separable blur, tonemap composite
static/                  # Frontend: index.html (WebGPU check + bootstrap), styles.css, favicon.svg
pkg/                     # wasm-pack output + copied static assets — the deploy root (git-ignored)
scripts/                 # render-diagrams.mjs, check-diagrams.mjs
```

## Patterns

galacto is small, but nearly every file is an instance of one of a handful of recurring patterns. Naming them once makes the rest of the code predictable; the detailed sections below are each an elaboration of one of these.

**GPU-resident state, no readback.** After the initial upload, the cores and the stars' positions/velocities live only in GPU storage buffers. The compute passes are the sole writers and the render pass the sole reader; the CPU never reads body data back. The CPU's only per-frame write is the small camera uniform.

**Compute-then-render over one buffer — no ping-pong.** The same particle buffer is bound `storage, read_write` to the compute pass and `storage, read` to the render pass within a single command encoder. WebGPU inserts a barrier between the passes, so the render reads exactly what the compute just wrote. Double-buffering (ping-pong) is deliberately absent: nothing reads the buffer *while* it is being written, so a second copy would buy nothing. It would only be needed if a frame both read an old generation and wrote a new one concurrently.

**Owning composition root (`AppState`).** One struct (`src/lib.rs`) owns the four subsystems — `Graphics`, `Simulation`, `Camera`, `InputHandler` — and is the only orchestrator. Each frame it calls `update()` then `render()`. Subsystems never reach for each other; they are wired together only through `AppState`.

**Single `#[wasm_bindgen(start)]` entry + self-scheduling rAF loop.** `start()` is the only WASM export. It installs the panic hook, spawns async initialization, and arms a `requestAnimationFrame` callback that re-arms itself every frame — the render loop is a tail chain of rAF calls, not a timer. The loop and the resize handler reach the app through a `thread_local!` `RefCell<Option<Rc<RefCell<AppState>>>>` — the safe single-threaded-WASM global, no `static mut`.

**POD structs mirrored Rust ↔ WGSL.** `Particle`, `Core`, and `SimulationParams` are `#[repr(C)]` + `bytemuck::Pod`, byte-for-byte identical to their WGSL `struct` counterparts, so they `cast_slice` straight into buffers with no serialization. This means matching WGSL's layout rules exactly: `Particle` carries explicit pads (`position`, `_pad`, `velocity`, `_pad` = 32 bytes) because WGSL aligns a storage `vec3<f32>` to 16 bytes — a tightly packed 24-byte Rust struct would scatter velocity bytes into position. `Core` packs position+mass and velocity into two `vec4`s for the same reason, and trailing `u32` pads keep `SimulationParams` 16-byte aligned for a uniform. **The Rust definition and the WGSL definition are one contract and must change together.**

**Upload-once vs upload-per-frame.** Data that is static after init — the star buffer, the core buffer (seeded once, then evolved in place on the GPU), and the `SimulationParams` uniform (entirely constants now that `dt` is fixed) — is uploaded once at creation. Only the camera matrix changes per frame, pushed with `queue.write_buffer` into its `UNIFORM | COPY_DST` buffer.

**Labeled resources.** Every buffer, pipeline, bind group, pass, and texture carries a `label: Some(...)` so it is identifiable in browser GPU debuggers and validation messages.

**Derived visuals in-shader (single source of truth).** Color, brightness, and glow are pure functions of a star's galaxy of origin (its instance index relative to the galaxy split) and its speed, computed in the shaders and never stored. Position + velocity is the only per-star state; appearance is recomputed each frame, so it can never drift out of sync with the simulation.

**Deferred input: accumulate, then drain.** DOM event handlers write into one shared `InputState` behind an `Rc<RefCell>` (`src/input.rs`). The frame loop reads that state once per frame: it acts on *level* state (is-rotating, is-dragging) and **drains** *edge* state — the pause/reset flags and the accumulated zoom delta are reset as they are consumed. This decouples asynchronous, bursty event delivery from the synchronous once-per-frame update.

**Retained closures keep listeners alive.** Each `add_event_listener` closure is pushed into the handler's `_closures` vector so it is not dropped at the end of setup — dropping it would silently unregister the listener.

**Display-synced canvas sizing.** A `resize` listener keeps the drawing buffer at the displayed size × `devicePixelRatio` and reconfigures the surface and camera aspect together (`AppState::resize`), so the view fills the window without stretching.

**Compile-time-embedded shaders.** WGSL is brought in with `include_str!`, so shaders are compiled into the WASM; there is no runtime fetch or separate asset to deploy.

**Deterministic seeded initialization.** All initial particle state is generated from a fixed RNG seed (`StdRng::seed_from_u64(42)`), so every page load produces an identical starting configuration.

**Restricted N-body.** A few massive cores (`NUM_CORES`) move under their mutual gravity — `O(NUM_CORES²)`, trivial — while the many stars are massless test particles in the cores' combined field (`O(N · NUM_CORES)` per step). The stars feel the cores but not each other; there is no self-gravity within a disk, so tidal tails and spiral arms emerge from a shared, *moving* field rather than from star–star interaction.

**Fixed-timestep accumulator.** The render loop runs at the display's refresh rate, but physics advances in whole `FIXED_DT` (1/60 s) steps: each frame adds the real elapsed time to an accumulator and runs as many fixed compute dispatches as have accumulated — clamped to `MAX_SUBSTEPS`, with a `MAX_FRAME_DT` clamp on the frame delta so a long stall can't spiral. Step size is independent of frame rate, so the same seed evolves identically on a 60 Hz and a 144 Hz display.

**Symplectic integration with softening.** Both kernels use symplectic (semi-implicit) Euler — `velocity += a · dt`, then `position += velocity · dt` — which conserves orbital energy far better than explicit Euler, so disks stay coherent over many orbits. A Plummer softening length (`a = G·m·d / (|d|² + ε²)^{3/2}`) keeps close passages finite, and the fixed timestep (a bounded `dt`, never the raw frame delta) bounds the step. There is no velocity clamp and no boundary: stars are free to stream into tidal tails and escape, which is the physically correct behaviour.

**FFI-free core with a `JsValue` boundary.** The engine modules — `simulation`, `camera`, and `graphics` — carry no `wasm_bindgen::JsValue`. `Graphics::new` (the only fallible one) returns a domain `AppError` (`src/error.rs`); `Simulation`, `Camera`, and `InputHandler` construction is infallible. `JsValue` is confined to the boundary: `lib.rs` converts `AppError` → `JsValue` in `AppState::new`, and the DOM-event wiring (`input.rs`) plus the `#[wasm_bindgen]` `start` / `run` / `render` surface return `Result<_, JsValue>`. The per-frame hot path is infallible.

## How a Frame Is Produced

![Frame loop: update then render](diagrams/frame-loop.png)

A single `requestAnimationFrame` callback (`animation_frame` in `src/lib.rs`) does two things on the shared `AppState`:

1. **`update(time)`** — let the `InputHandler` apply pending rotate/pan/zoom/reset to the `Camera`, toggle pause if Space was pressed, then add the real frame delta to the fixed-timestep accumulator and compute how many `FIXED_DT` steps to run this frame (0 when paused).
2. **`render()`** — open a command encoder, then:
   - run the **compute passes** once per scheduled step (each its own pass, so later reads see earlier writes): first `update_cores` (one workgroup) advances the galaxy cores under their mutual gravity, then `update_particles` over `ceil(131072 / 64) = 2048` workgroups advances every star in the cores' field — both in place;
   - run the **particle pass**: write the camera matrix (+ billboard size/aspect) into the camera uniform, then issue one instanced draw — a billboard quad per particle, `draw(0..4, 0..131072)` — additively blended with no depth buffer, into the **HDR scene** target;
   - run the **bloom passes** (`postprocess`): bright-pass + downsample, separable blur (H, V), then a tonemapped composite of scene + bloom into the swapchain;
   - submit and `present()`.

Then it schedules the next frame. The simulation state lives only in GPU memory between frames — there is no CPU-side particle array after the initial upload.

## GPU Data Model

`Simulation::new` (`src/simulation.rs`) creates four buffers and three pipelines (two compute, one render):

| Resource          | Contents                                          | Usage                                  |
| ----------------- | ------------------------------------------------- | -------------------------------------- |
| Particle buffer   | `131072 × Particle` (`position`, `velocity`, padded to 32 B each, ~4.2 MB) | `STORAGE` |
| Core buffer       | `2 × Core` (`pos_mass: vec4`, `vel: vec4` — 32 B each) | `STORAGE` |
| Params buffer     | `SimulationParams { dt, g, softening, particle_count, num_cores, _pad×3 }` | `UNIFORM \| COPY_DST` |
| Camera buffer     | view-projection matrix + billboard size / aspect / galaxy split (80 B) | `UNIFORM \| COPY_DST`              |

- **Compute bind group** (`@compute` visibility), shared by both compute pipelines: binding 0 = particle buffer as `storage, read_write`; binding 1 = params as `uniform`; binding 2 = core buffer as `storage, read_write`. `update_cores` writes the cores (and ignores the particles); `update_particles` reads the cores and writes the particles.
- **Render bind group** (`@vertex` visibility): binding 0 = camera as `uniform`; binding 1 = the *same* particle buffer as `storage, read`.

The particle buffer is written by the compute stage and read by the vertex stage, so the data the compute shader just wrote is exactly what the vertex shader reads — no copies, no staging, no ping-pong. The vertex shader indexes `particles[instance_index]` directly rather than using a vertex buffer layout.

## Simulation & Physics

All physics is in `src/shaders/update.wgsl`, driven by the `SimulationParams` uniform (`dt`, `g`, `softening`, `num_cores`). Two kernels run per step, each in its own pass so the second sees the first's writes:

- **`update_cores`.** A single invocation integrates all cores — `NUM_CORES` is tiny, so one thread reading every start-of-step position avoids read/write races. Each core is accelerated by the others' Plummer-softened gravity, then stepped with symplectic Euler.
- **`update_particles`.** Each star sums the softened acceleration from every core, `a += G · mᵢ · dᵢ / (|dᵢ|² + ε²)^{3/2}` (where `dᵢ` is core_i − star), then takes the same symplectic-Euler step (`velocity += a · dt`; `position += velocity · dt`). There is no velocity clamp and no boundary.

Constants live in `src/simulation.rs`, in the sim's arbitrary unit system: `G = 1`, `CORE_MASS = 450000`, `SOFTENING = 12`.

Initial conditions (`Simulation::generate_initial_galaxies`, seeded `StdRng(42)` → reproducible):

- **Two cores** sit on a bound, eccentric orbit about their shared centre of mass (the origin): on the x-axis at `±213`, with opposed tangential velocities along y (`±15.4`). That gives a semi-major axis ~275 and a ~30 s period, so they swing through a deep pericenter passage and back rather than flying off.
- **Two star disks**, half the stars each, fill a thin, centrally concentrated disk (radius ~4–120, in the x-y plane) around each core. Every star gets its core's bulk velocity plus a prograde tangential velocity from the *softened* circular speed `v_circ² = G · M · r² / (r² + ε²)^{3/2}`, so inner stars don't orbit unphysically fast inside the soft core. A star keeps its galaxy's identity by index (first half = galaxy A, second = B), which the renderer uses to tint it.

## Rendering

`src/shaders/render.wgsl` draws each particle as a camera-facing **billboard quad** (instanced: 4 verts × N particles):

- **Vertex** — transform `position` by the camera matrix, then offset the four quad corners in clip space to a screen-constant size (divided by aspect so they stay square). Color is set by galaxy of origin — cool blue for one, warm amber for the other (instance index vs the galaxy-split value in the camera uniform) — with a slight speed-driven brightness boost.
- **Fragment** — a soft radial falloff from the quad center (`(1 − d)²`) makes each particle a round glow, scaled down so its per-particle contribution stays modest.

The pipeline uses `TriangleStrip` topology with **additive** blending and **no depth buffer** — additive glow is order-independent and there is no opaque geometry — so overlapping particles accumulate brightness. It renders into an offscreen **HDR** (`rgba16float`) target (cleared to a near-black blue `(0.01, 0.01, 0.05)`) so dense regions can exceed 1.0 before tonemapping.

## Post-processing

`src/postprocess.rs` (`src/shaders/post.wgsl`) turns the HDR scene into the final image with a bloom chain, each step a fullscreen pass: a **bright-pass** that box-downsamples to quarter resolution and keeps pixels above a threshold; a **separable Gaussian blur** (horizontal then vertical) on the reduced-resolution bloom buffers; and a **composite** that adds the blurred bloom back to the scene, applies an exposure tonemap (`1 − e^(−c·hdr)`), and writes the swapchain. The scene and bloom targets are rebuilt on resize.

## Camera & Input

`Camera` (`src/camera.rs`) is an orbit camera: it keeps a `scale` (zoom), `rotation_x` / `rotation_y`, and an aspect ratio, and places the eye at `distance = 800 / scale` rotated around the origin, always looking at `(0,0,0)` through a 45° perspective (near 0.1, far 50000 — generous because there is no depth buffer, so far zoom-out never clips). It starts face-on (no rotation, looking down the disk normal) and zoomed out (`scale = 0.7`) so both galaxies and their tidal tails sit in frame; `rotation_x` is clamped to ±1.5 rad and `scale` to 0.1–5.0 (distance `800 / scale`, so 160–8000).

A `resize` listener on the window (`src/lib.rs`) keeps the canvas drawing buffer matched to its displayed size × `devicePixelRatio`, calling `AppState::resize` to reconfigure the surface and update the camera aspect — so the view fills the window at native resolution without stretching.

`InputHandler` (`src/input.rs`) registers DOM listeners and translates them into camera intent, polled once per frame:

| Input                         | Action            |
| ----------------------------- | ----------------- |
| Left-drag / one-finger drag   | Rotate (orbit)    |
| Right-drag                    | Pan               |
| Wheel / two-finger pinch      | Zoom              |
| Space                         | Pause / resume    |
| R                             | Reset camera      |

## Build & Deploy

- `npm run build` → `wasm-pack build --target web --release --out-dir pkg --out-name galacto` (wasm-opt `-O2` is configured in `Cargo.toml`), then copies `static/` into `pkg/` and runs `scripts/cache-bust.mjs`, which stamps the `galacto.js` import in `index.html` with `?v=<git-sha>` so a new deploy always loads fresh glue. Output is `pkg/` (git-ignored, regenerated).
- `npm run dev` → build, then `serve pkg -l 8000`. Open in a WebGPU-capable browser.
- `npm run deploy` → build, then `wrangler pages deploy pkg --project-name=galacto`.
- CI (`.github/workflows/ci-cd.yml`) runs the verification gate on every push/PR and deploys `pkg/` to Cloudflare Pages on push to `main`. The Pages project name lives only in the deploy command; there is no `wrangler.toml`.

## What This Architecture Deliberately Does Not Include

- **No server or persistence.** Everything runs client-side; there is no backend or save state.
- **No CPU physics.** All per-particle work is on the GPU; the CPU only sets `dt`, the camera, and pause state. The particle buffer is never read back.
- **No threads.** The WASM is single-threaded — no `rayon`, no `SharedArrayBuffer`. It therefore needs **no** cross-origin-isolation (COOP/COEP) headers; the `_headers` file only sets `Cache-Control: no-cache` so the wasm and HTML revalidate (the JS glue is cache-busted via `?v=` instead).
- **No self-gravity among the stars.** The stars are massless test particles: they feel the cores but not one another (full self-gravity would be O(N²), or need a tree / particle-mesh solver). Only the handful of cores attract, so the disks have no self-gravity and slowly disperse over many passages rather than re-forming a bound remnant. See the [backlog](../BACKLOG.md) for the self-gravity upgrade path.
- **No WebGL fallback.** The renderer targets WebGPU; `index.html` checks for it up front and shows a "WebGPU not supported" message rather than degrading.

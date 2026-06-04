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
| Scale            | 16,384 self-gravitating bodies  | Two compute passes (all-pairs gravity, then integrate) + one instanced draw per step |

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
    ├── update.wgsl      # Compute: tiled all-pairs self-gravity + symplectic integration
    ├── render.wgsl      # Billboard vertex + radial-glow fragment (additive)
    └── post.wgsl        # Fullscreen bright-pass, separable blur, tonemap composite
static/                  # Frontend: index.html (WebGPU check + bootstrap + speed slider), styles.css, favicon.svg
pkg/                     # wasm-pack output + copied static assets — the deploy root (git-ignored)
scripts/                 # render-diagrams.mjs, check-diagrams.mjs
```

## Patterns

galacto is small, but nearly every file is an instance of one of a handful of recurring patterns. Naming them once makes the rest of the code predictable; the detailed sections below are each an elaboration of one of these.

**GPU-resident state, no readback.** After the initial upload, every body's position/velocity/mass lives only in a GPU storage buffer (plus a scratch acceleration buffer). The compute passes are the sole writers and the render pass the sole reader; the CPU never reads body data back. The CPU's only per-frame write is the small camera uniform.

**Two-pass gravity instead of ping-pong.** The all-pairs sum means every body reads every other body's position, so positions must not change mid-sum. Rather than double-buffer the whole particle array (ping-pong), the step is split into two passes over one buffer: `compute_accel` reads positions and writes a separate `accel` buffer; `integrate` then reads `accel` and advances each body in place. Each body only ever writes its own slot, and the pass boundary makes the write visible to the next read — so a single particle buffer suffices, with the small `accel` buffer as the only extra copy.

**Owning composition root (`AppState`).** One struct (`src/lib.rs`) owns the four subsystems — `Graphics`, `Simulation`, `Camera`, `InputHandler` — and is the only orchestrator. Each frame it calls `update()` then `render()`. Subsystems never reach for each other; they are wired together only through `AppState`.

**Single `#[wasm_bindgen(start)]` entry + self-scheduling rAF loop.** `start()` is the only WASM export. It installs the panic hook, spawns async initialization, and arms a `requestAnimationFrame` callback that re-arms itself every frame — the render loop is a tail chain of rAF calls, not a timer. The loop and the resize handler reach the app through a `thread_local!` `RefCell<Option<Rc<RefCell<AppState>>>>` — the safe single-threaded-WASM global, no `static mut`.

**POD structs mirrored Rust ↔ WGSL.** `Particle` and `SimulationParams` are `#[repr(C)]` + `bytemuck::Pod`, byte-for-byte identical to their WGSL `struct` counterparts, so they `cast_slice` straight into buffers with no serialization. `Particle` packs position+mass and velocity into two `vec4`s (`pos_mass`, `vel` = 32 bytes); using `vec4` rather than `vec3` sidesteps WGSL's 16-byte `vec3` alignment, so the Rust and WGSL layouts are unambiguous. Trailing `u32` pads keep `SimulationParams` 16-byte aligned for a uniform. **The Rust definition and the WGSL definition are one contract and must change together.**

**Upload-once vs upload-per-frame.** Data that is static after init — the particle buffer (seeded once, then evolved in place on the GPU) and the `SimulationParams` uniform (entirely constants now that `dt` is fixed) — is uploaded once at creation; the `accel` buffer is GPU-only scratch that is never uploaded. Only the camera matrix changes per frame, pushed with `queue.write_buffer` into its `UNIFORM | COPY_DST` buffer.

**Labeled resources.** Every buffer, pipeline, bind group, pass, and texture carries a `label: Some(...)` so it is identifiable in browser GPU debuggers and validation messages.

**Derived visuals in-shader (single source of truth).** Color, brightness, and glow are pure functions of a star's galaxy of origin (its instance index relative to the galaxy split) and its speed, computed in the shaders and never stored. Position + velocity is the only per-body state; appearance is recomputed each frame, so it can never drift out of sync with the simulation.

**Deferred input: accumulate, then drain.** DOM event handlers write into one shared `InputState` behind an `Rc<RefCell>` (`src/input.rs`). The frame loop reads that state once per frame: it acts on *level* state (is-rotating, is-dragging) and **drains** *edge* state — the pause/reset flags and the accumulated zoom delta are reset as they are consumed. This decouples asynchronous, bursty event delivery from the synchronous once-per-frame update.

**Retained closures keep listeners alive.** Each `add_event_listener` closure is pushed into the handler's `_closures` vector so it is not dropped at the end of setup — dropping it would silently unregister the listener.

**Display-synced canvas sizing.** A `resize` listener keeps the drawing buffer at the displayed size × `devicePixelRatio` and reconfigures the surface and camera aspect together (`AppState::resize`), so the view fills the window without stretching.

**Compile-time-embedded shaders.** WGSL is brought in with `include_str!`, so shaders are compiled into the WASM; there is no runtime fetch or separate asset to deploy.

**Deterministic seeded initialization.** All initial particle state is generated from a fixed RNG seed (`StdRng::seed_from_u64(42)`), so every page load produces an identical starting configuration.

**All-pairs self-gravity, tiled.** Every body has mass and attracts every other: each body's acceleration is the softened sum over all `N` bodies — `O(N²)` per step. The kernel amortises global-memory reads by staging the bodies in workgroup-shared "tiles": each workgroup loads a tile of positions/masses into shared memory behind a `workgroupBarrier`, then every thread accumulates that tile's pull on its own body. This `O(N²)` cost is why `N` is ~16k, not the hundreds of thousands a test-particle sim allows — but it is also what lets a galaxy genuinely form: the two galaxies sink together via dynamical friction and relax into a bound remnant rather than dispersing.

**Fixed-timestep accumulator.** The render loop runs at the display's refresh rate, but physics advances in whole `FIXED_DT` (1/60 s) steps: each frame adds the real elapsed time — scaled by the speed-slider multiplier — to an accumulator and runs as many fixed steps as have accumulated, clamped to `MAX_SUBSTEPS` (which therefore also caps the top speed: ~128× at 60 fps) with a `MAX_FRAME_DT` clamp so a long stall can't spiral. Each substep is a full `O(N²)` gravity pass, so high speeds are GPU-bound and the frame rate drops — but step size never changes, so integration accuracy is unaffected and the same seed evolves identically regardless of frame rate.

**Symplectic integration with softening.** The integrate kernel uses symplectic (semi-implicit) Euler — `velocity += a · dt`, then `position += velocity · dt` — which conserves energy far better than explicit Euler, so the remnant is stable over many orbits. A Plummer softening length (`a = Σ G·mⱼ·dⱼ / (|dⱼ|² + ε²)^{3/2}`) keeps close encounters finite and, being fairly large, lets the two heavy nuclei coalesce into one soft core instead of locking into a hard binary. There is no velocity clamp and no boundary.

**FFI-free core with a `JsValue` boundary.** The engine modules — `simulation`, `camera`, and `graphics` — carry no `wasm_bindgen::JsValue`. `Graphics::new` (the only fallible one) returns a domain `AppError` (`src/error.rs`); `Simulation`, `Camera`, and `InputHandler` construction is infallible. `JsValue` is confined to the boundary: `lib.rs` converts `AppError` → `JsValue` in `AppState::new`, and the DOM-event wiring (`input.rs`) plus the `#[wasm_bindgen]` `start` / `run` / `render` surface return `Result<_, JsValue>`. The one other export, `set_speed` (called by the page's speed slider), takes a plain `f32` and is infallible, as is the per-frame hot path.

## How a Frame Is Produced

![Frame loop: update then render](diagrams/frame-loop.png)

A single `requestAnimationFrame` callback (`animation_frame` in `src/lib.rs`) does two things on the shared `AppState`:

1. **`update(time)`** — let the `InputHandler` apply pending rotate/pan/zoom/reset to the `Camera`, toggle pause if Space was pressed, then add the real frame delta (scaled by the speed multiplier) to the fixed-timestep accumulator and compute how many `FIXED_DT` steps to run this frame (0 when paused).
2. **`render()`** — open a command encoder, then:
   - run the **compute passes** once per scheduled step (each its own pass, so later reads see earlier writes): first `compute_accel` sums the all-pairs gravity into the accel buffer, then `integrate` advances every body in place — each over `16384 / 256 = 64` workgroups;
   - run the **particle pass**: write the camera matrix (+ billboard size/aspect) into the camera uniform, then issue one instanced draw — a billboard quad per body, `draw(0..4, 0..16384)` — additively blended with no depth buffer, into the **HDR scene** target;
   - run the **bloom passes** (`postprocess`): bright-pass + downsample, separable blur (H, V), then a tonemapped composite of scene + bloom into the swapchain;
   - submit and `present()`.

Then it schedules the next frame. The simulation state lives only in GPU memory between frames — there is no CPU-side particle array after the initial upload.

## GPU Data Model

`Simulation::new` (`src/simulation.rs`) creates four buffers and three pipelines (two compute, one render):

| Resource          | Contents                                          | Usage                                  |
| ----------------- | ------------------------------------------------- | -------------------------------------- |
| Particle buffer   | `16384 × Particle` (`pos_mass: vec4`, `vel: vec4` — 32 B each, ~0.5 MB) | `STORAGE` |
| Accel buffer      | `16384 × vec4<f32>` scratch accelerations         | `STORAGE` |
| Params buffer     | `SimulationParams { dt, g, softening, particle_count, _pad×4 }` | `UNIFORM \| COPY_DST` |
| Camera buffer     | view-projection matrix + billboard size / aspect / galaxy split (80 B) | `UNIFORM \| COPY_DST`              |

- **Compute bind group** (`@compute` visibility), shared by both compute pipelines: binding 0 = particle buffer as `storage, read_write`; binding 1 = params as `uniform`; binding 2 = accel buffer as `storage, read_write`. `compute_accel` reads particles and writes accel; `integrate` reads accel and writes particles.
- **Render bind group** (`@vertex` visibility): binding 0 = camera as `uniform`; binding 1 = the *same* particle buffer as `storage, read`.

The particle buffer is written by the compute stage and read by the vertex stage, so the data the compute shader just wrote is exactly what the vertex shader reads — no copies, no staging. The vertex shader indexes `particles[instance_index]` directly rather than using a vertex buffer layout.

## Simulation & Physics

All physics is in `src/shaders/update.wgsl`, driven by the `SimulationParams` uniform (`dt`, `g`, `softening`, `particle_count`). Two kernels run per step, each in its own pass so the second sees the first's writes:

- **`compute_accel`.** Each body sums the softened gravitational pull of *every* body, `a = Σ G · mⱼ · dⱼ / (|dⱼ|² + ε²)^{3/2}` (where `dⱼ` is body_j − body_i), and writes it to the accel buffer. The sum is tiled through workgroup-shared memory: each workgroup loads `WORKGROUP_SIZE` bodies into a shared array behind a `workgroupBarrier`, every thread accumulates that tile, then the next tile loads. The self term (`dⱼ = 0`) contributes nothing, so it needs no special case. Finally a static **dark-matter halo** term is added: `a -= v₀² · pos / (|pos|² + r_c²)`, a logarithmic potential centred at the origin whose inward pull keeps the system bound (debris orbits back rather than escaping) and gives a flat outer rotation curve.
- **`integrate`.** Reads the acceleration and takes a symplectic-Euler step (`velocity += a · dt`; `position += velocity · dt`), writing the body back in place.

`NUM_PARTICLES` must be a multiple of `WORKGROUP_SIZE` (the tile size, 256) so the tile loop never reads past the buffer. Constants live in `src/simulation.rs`, in the sim's arbitrary unit system: `G = 1`, `CENTER_MASS = 300000`, `STAR_MASS = 20`, `SOFTENING = 25`, and the halo `HALO_V0 = 75` / `HALO_RC = 150`.

Initial conditions (`Simulation::generate_initial_galaxies`, seeded `StdRng(42)` → reproducible) build two galaxies, half the bodies each:

- **A heavy central body** (`CENTER_MASS`) anchors each galaxy. The two start on the x-axis at `±120` with opposed tangential velocities (`±20` along y) — deeply bound, so they fall together and merge in a couple of passages rather than orbiting forever.
- **A disk of lighter stars** (`STAR_MASS`) fills a thin, centrally concentrated disk (radius ~4–120) around each centre, each star on a prograde circular orbit in the centre's softened potential plus the galaxy's bulk velocity. Both disk spins and the orbit share the `+z` axis, so their angular momenta add and the merged remnant rotates. A body keeps its galaxy's identity by index (first half = galaxy A, second = B), which the renderer uses to tint it.

Because the bodies self-gravitate, the centres sink together (dynamical friction) into a single nucleus and the stars relax into one bound, rotating remnant — a collisionless major merger, which realistically yields a puffy elliptical-like remnant rather than a thin spiral disk.

## Rendering

`src/shaders/render.wgsl` draws each particle as a camera-facing **billboard quad** (instanced: 4 verts × N particles):

- **Vertex** — transform the body's position (`pos_mass.xyz`) by the camera matrix, then offset the four quad corners in clip space to a screen-constant size (divided by aspect so they stay square). Color is set by galaxy of origin — cool blue for one, warm amber for the other (instance index vs the galaxy-split value in the camera uniform) — with a slight speed-driven brightness boost.
- **Fragment** — a soft radial falloff from the quad center (`(1 − d)²`) makes each particle a round glow, scaled down so its per-particle contribution stays modest.

The pipeline uses `TriangleStrip` topology with **additive** blending and **no depth buffer** — additive glow is order-independent and there is no opaque geometry — so overlapping particles accumulate brightness. It renders into an offscreen **HDR** (`rgba16float`) target (cleared to a near-black blue `(0.01, 0.01, 0.05)`) so dense regions can exceed 1.0 before tonemapping.

## Post-processing

`src/postprocess.rs` (`src/shaders/post.wgsl`) turns the HDR scene into the final image with a bloom chain, each step a fullscreen pass: a **bright-pass** that box-downsamples to quarter resolution and keeps pixels above a threshold; a **separable Gaussian blur** (horizontal then vertical) on the reduced-resolution bloom buffers; and a **composite** that adds the blurred bloom back to the scene, applies an exposure tonemap (`1 − e^(−c·hdr)`), and writes the swapchain. The scene and bloom targets are rebuilt on resize.

## Camera & Input

`Camera` (`src/camera.rs`) is an orbit camera: it keeps a `scale` (zoom), `rotation_x` / `rotation_y`, and an aspect ratio, and places the eye at `distance = 800 / scale` rotated around the origin, always looking at `(0,0,0)` through a 45° perspective (near 0.1, far 10,000,000 — generous because there is no depth buffer, so far zoom-out never clips). It starts face-on (no rotation, looking down the disk normal) and zoomed out (`scale = 0.7`) so both galaxies and their tidal tails sit in frame; `rotation_x` is clamped to ±1.5 rad and `scale` to 0.001–5.0 (distance `800 / scale`, so 160–800000, wide enough to frame the whole dispersing debris cloud). Wheel/pinch zoom maps the device delta through a bounded exponential step, so a notch is a consistent zoom regardless of input device.

A `resize` listener on the window (`src/lib.rs`) keeps the canvas drawing buffer matched to its displayed size × `devicePixelRatio`, calling `AppState::resize` to reconfigure the surface and update the camera aspect — so the view fills the window at native resolution without stretching.

`InputHandler` (`src/input.rs`) registers DOM listeners and translates them into camera intent, polled once per frame:

| Input                         | Action            |
| ----------------------------- | ----------------- |
| Left-drag / one-finger drag   | Rotate (orbit)    |
| Right-drag                    | Pan               |
| Wheel / two-finger pinch      | Zoom              |
| Space                         | Pause / resume    |
| R                             | Reset camera      |
| Speed slider (on-screen)      | Scale sim speed (0.25×–100×) |

## Build & Deploy

- `npm run build` → `wasm-pack build --target web --release --out-dir pkg --out-name galacto` (wasm-opt `-O2` is configured in `Cargo.toml`), then copies `static/` into `pkg/` and runs `scripts/cache-bust.mjs`, which stamps the `galacto.js` import in `index.html` with `?v=<git-sha>` so a new deploy always loads fresh glue. Output is `pkg/` (git-ignored, regenerated).
- `npm run dev` → build, then `serve pkg -l 8000`. Open in a WebGPU-capable browser.
- `npm run deploy` → build, then `wrangler pages deploy pkg --project-name=galacto`.
- CI (`.github/workflows/ci-cd.yml`) runs the verification gate on every push/PR and deploys `pkg/` to Cloudflare Pages on push to `main`. The Pages project name lives only in the deploy command; there is no `wrangler.toml`.

## What This Architecture Deliberately Does Not Include

- **No server or persistence.** Everything runs client-side; there is no backend or save state.
- **No CPU physics.** All per-body work is on the GPU; the CPU only sets `dt`, the camera, and pause/speed state. The particle buffer is never read back.
- **No threads.** The WASM is single-threaded — no `rayon`, no `SharedArrayBuffer`. It therefore needs **no** cross-origin-isolation (COOP/COEP) headers; the `_headers` file only sets `Cache-Control: no-cache` so the wasm and HTML revalidate (the JS glue is cache-busted via `?v=` instead).
- **No force approximation (yet).** Gravity is the exact all-pairs sum, `O(N²)`. That is what caps the body count near ~16k for interactive speed; a Barnes-Hut tree or particle-mesh/FFT solver would scale to far more bodies but is a much larger change (see the [backlog](../BACKLOG.md)).
- **No dissipation.** The bodies are collisionless (no gas), so a merger relaxes into a puffy, elliptical-like rotating remnant rather than re-forming a thin spiral disk. A dissipative component would be needed for a settling disk.
- **No WebGL fallback.** The renderer targets WebGPU; `index.html` checks for it up front and shows a "WebGPU not supported" message rather than degrading.

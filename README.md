# 🌌 Interacting Galaxies

A GPU-accelerated **N-body** simulation of two galaxies merging: ~16,000 self-gravitating bodies where every star pulls on every other, so the galaxies fall together, raise tidal spiral arms, and relax into a single bound, spinning remnant. Written in **Rust**, compiled to **WebAssembly**, and rendered with **WebGPU** — it runs entirely in the browser.

**Live:** [galacto.tre.systems](https://galacto.tre.systems/) — needs a WebGPU-capable browser (Chrome / Edge 113+, or Firefox with `dom.webgpu.enabled`).

![Two interacting galaxies with tidal tails and spiral arms](screenshot.png)

## Features

- **GPU compute physics** — the all-pairs gravity for every body runs in a WebGPU compute shader (workgroup-tiled); the CPU never touches per-body state.
- **Self-gravity N-body** — every body has mass and attracts every other, so two galaxies merge under their own gravity (and dynamical friction) and relax into a single spinning remnant.
- **Dark-matter halo** — a static logarithmic halo gently pulls everything toward the centre, so the remnant and its debris stay bound near the middle (orbiting back rather than dispersing off-screen) with a flat outer rotation curve.
- **Rust → WebAssembly** — the core compiles to WASM for near-native speed.
- **Interactive 3D camera** — orbit, pan, zoom, pause, and reset, with mouse, keyboard, and touch.
- **Adjustable speed** — an on-screen slider scales the simulation from slow-motion up to ~100× to fast-forward the merger. The top end is GPU-bound (all-pairs gravity is heavy, so the frame rate drops), but the fixed timestep keeps the physics correct.
- **Galaxy coloring** — stars are tinted by which galaxy they began in (cool blue vs warm amber), so mixing and tails stay legible.
- **Edge-deployed** — ships as a static site on Cloudflare Pages.

## Controls

### Desktop

| Input              | Action                              |
| ------------------ | ----------------------------------- |
| **Left-drag**      | Rotate (orbit) the camera           |
| **Right-drag**     | Pan the camera                      |
| **Mouse wheel**    | Zoom in and out                     |
| **Spacebar**       | Pause / resume the simulation       |
| **R**              | Reset the camera                    |
| **Speed slider**   | Scale simulation speed (0.25×–100×) |

### Touch

| Input              | Action               |
| ------------------ | -------------------- |
| **One finger**     | Rotate the camera    |
| **Pinch**          | Zoom in and out      |

## Quick Start

### Prerequisites

- **Rust** — install from [rustup.rs](https://rustup.rs/)
- **Node.js** 16+ — for the build scripts
- **A WebGPU browser** — Chrome / Edge 113+, or Firefox with `dom.webgpu.enabled`

### Installation

```bash
git clone https://github.com/tre-systems/galacto.git
cd galacto
npm run setup   # installs deps and adds the wasm32 target
npm run dev     # builds, then serves on http://localhost:8000
```

## Development

### Project Structure

```
galacto/
├── src/                  # Rust source
│   ├── lib.rs            # WASM entry: AppState + requestAnimationFrame loop
│   ├── graphics.rs       # WebGPU initialization
│   ├── simulation.rs     # Buffers, pipelines, galaxy init, compute/render dispatch
│   ├── camera.rs         # Orbit camera → view-projection matrix
│   ├── input.rs          # Mouse / touch / keyboard → camera
│   ├── utils.rs          # Panic hook, console_log!
│   └── shaders/
│       ├── update.wgsl   # Compute: tiled all-pairs self-gravity + symplectic integration
│       └── render.wgsl   # Vertex + fragment: project + per-galaxy glow
├── static/               # Frontend assets (index.html, styles.css, favicon)
├── docs/                 # Architecture and diagrams
├── scripts/              # Diagram render/check scripts
└── pkg/                  # wasm-pack output (generated, git-ignored)
```

### Key Commands

| Command                 | Description                                      |
| ----------------------- | ------------------------------------------------ |
| `npm run setup`         | Install dependencies and add the WASM target     |
| `npm run build`         | Build the WASM module and copy assets into `pkg/`|
| `npm run dev`           | Build and serve on port 8000                     |
| `npm run deploy`        | Build and deploy to Cloudflare Pages             |
| `npm run test`          | Run Rust tests                                   |
| `npm run lint`          | Run Clippy                                       |
| `npm run format`        | Format with rustfmt                              |
| `npm run diagrams`      | Render the architecture diagrams (needs Graphviz)|

The pre-commit hook runs `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test`; CI runs the same plus a WASM `cargo check` and deploys on push to `main`.

## Architecture

![System overview](docs/diagrams/system-overview.png)

One `requestAnimationFrame` callback updates the camera, then per fixed step runs two GPU **compute** passes — an all-pairs gravity pass that sums each body's acceleration, then an integrate pass that advances it — and issues one instanced **billboard** draw that reads the same buffer. Body state lives only in GPU memory — there is no CPU readback. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full picture.

## Physics

The model is a full **N-body** system: every body has mass and attracts every other (all-pairs gravity, O(N²)). This self-gravity is what lets a galaxy actually form — the two galaxies fall together, sink via dynamical friction, and relax into one bound, rotating remnant instead of dispersing.

- **All-pairs gravity** — each body's acceleration is the softened sum over every other, `a = Σ G·mⱼ·dⱼ / (|dⱼ|² + ε²)^{3/2}`. The Plummer softening also keeps the two heavy nuclei from locking into a hard binary, so they coalesce into one.
- **Dark-matter halo** — a static logarithmic halo adds an inward pull `a = -v₀²·r / (|r|² + r_c²)` toward the origin. Its potential is unbounded, so nothing truly escapes — the system stays bound near the centre instead of dispersing forever.
- **Symplectic Euler** — computed in two passes per step (gravity, then integrate): velocity is updated, then position (`v += a·dt; x += v·dt`); this conserves energy far better than plain Euler.
- **Two galaxies** — each is a heavy central body plus a rotating disk of lighter stars, started on a bound, prograde approach so their spins and orbit share an axis and the remnant rotates. Close passages raise tidal spiral arms; then it merges.

Everything derives from a fixed RNG seed, so each load looks the same.

## Documentation

- [Architecture](docs/ARCHITECTURE.md) — how the code is organized and how one frame is produced
- [Diagrams](docs/diagrams/README.md) — Graphviz system-overview and frame-loop diagrams
- [Backlog](BACKLOG.md) — ordered next work and known constraints
- [Agent Notes](AGENTS.md) — workflow, verification, and architecture rules for agents

## Browser Support

| Browser         | Status   | Notes                                         |
| --------------- | -------- | --------------------------------------------- |
| **Chrome/Edge** | ✅ 113+  | WebGPU enabled by default                     |
| **Firefox**     | 🔧 113+  | Enable `dom.webgpu.enabled` in `about:config` |
| **Safari**      | ⚠️ 17.4+ | WebGPU support varies by version              |

## License

MIT License — see [LICENSE](LICENSE).

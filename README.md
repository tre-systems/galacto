# ⚫ Black Hole Accretion Disk Simulation

A GPU-accelerated black-hole accretion-disk simulation: ~131,000 particles orbiting a central singularity, with all physics running on the GPU. Written in **Rust**, compiled to **WebAssembly**, and rendered with **WebGPU** — it runs entirely in the browser.

**Live:** [galacto.tre.systems](https://galacto.tre.systems/) — needs a WebGPU-capable browser (Chrome / Edge 113+, or Firefox with `dom.webgpu.enabled`).

![Black Hole Simulation](screenshot.png)

## Features

- **GPU compute physics** — gravity and integration for every particle run in a WebGPU compute shader; the CPU never touches per-particle state.
- **Rust → WebAssembly** — the core compiles to WASM for near-native speed.
- **Interactive 3D camera** — orbit, pan, zoom, pause, and reset, with mouse, keyboard, and touch.
- **Velocity coloring** — particles shade blue (slow) → red (fast) with a speed-driven glow.
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
│   ├── simulation.rs     # Buffers, pipelines, particle init, compute/render dispatch
│   ├── camera.rs         # Orbit camera → view-projection matrix
│   ├── input.rs          # Mouse / touch / keyboard → camera
│   ├── utils.rs          # Panic hook, console_log!
│   └── shaders/
│       ├── update.wgsl   # Compute: gravity + integration
│       └── render.wgsl   # Vertex + fragment: project + velocity glow
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

One `requestAnimationFrame` callback updates the camera and simulation parameters, dispatches a single GPU **compute** pass that advances all particles in place, then issues one instanced **point** draw that reads the same buffer. Particle state lives only in GPU memory — there is no CPU readback. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full picture.

## Physics

The model is a simplified single-body gravitational system (particles are attracted to one fixed central mass, not to each other):

- **Gravity to a fixed center** — acceleration `a = -GM · pos / r³` toward the origin, with a small epsilon at `r → 0`.
- **Euler integration** — velocity is updated then position, with speed clamped to a maximum to keep the integrator stable; the time step is capped at ~0.033 s.
- **Inelastic boundary** — particles that reach the world bounds (`±600`) bounce back with ~90 % energy loss, so the cloud settles rather than ricocheting forever.

Initial conditions seed ~500 close-orbit "stars" in a flattened disk near the hole, plus a large injected **stream** of particles that the central gravity sweeps into the visible disk. Everything derives from a fixed RNG seed, so each load looks the same.

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

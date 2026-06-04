# 🌌 Interacting Galaxies

A GPU-accelerated **restricted N-body** simulation of two galaxies colliding: ~131,000 stars orbit two massive cores that swing through each other on a bound orbit, and gravity draws the disks out into tidal tails, bridges, and spiral arms. Written in **Rust**, compiled to **WebAssembly**, and rendered with **WebGPU** — it runs entirely in the browser.

**Live:** [galacto.tre.systems](https://galacto.tre.systems/) — needs a WebGPU-capable browser (Chrome / Edge 113+, or Firefox with `dom.webgpu.enabled`).

![Two interacting galaxies with tidal tails and spiral arms](screenshot.png)

## Features

- **GPU compute physics** — the galaxy cores and every test star are advanced in a WebGPU compute shader; the CPU never touches per-particle state.
- **Restricted N-body** — two massive cores move under their mutual gravity while the star disks fall through their combined, softened field, producing tidal tails and spiral arms.
- **Rust → WebAssembly** — the core compiles to WASM for near-native speed.
- **Interactive 3D camera** — orbit, pan, zoom, pause, and reset, with mouse, keyboard, and touch.
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
│       ├── update.wgsl   # Compute: core + test-particle gravity, symplectic integration
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

One `requestAnimationFrame` callback updates the camera, then per fixed step runs two GPU **compute** passes — first advancing the galaxy cores under mutual gravity, then every test star in the cores' field — and issues one instanced **billboard** draw that reads the same buffer. Particle state lives only in GPU memory — there is no CPU readback. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full picture.

## Physics

The model is a **restricted N-body** system: a few massive cores move under their mutual gravity, and the many stars are massless test particles that fall through the cores' combined field (they feel the cores but not each other).

- **Softened gravity** — each body's pull uses a Plummer softening, `a = G·m·d / (|d|² + ε²)^{3/2}`, so close passages stay finite and each disk keeps a soft glowing bulge.
- **Symplectic Euler** — velocity is updated, then position (`v += a·dt; x += v·dt`); this conserves orbital energy far better than plain Euler, with no velocity clamp and no boundary, so stars are free to stream into tidal tails and escape.
- **Two galaxy disks** — two cores start on a bound, eccentric orbit about their shared centre of mass; each carries a rotating disk of stars on softened-circular orbits. Repeated close passages raise spiral arms and fling out tidal tails.

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

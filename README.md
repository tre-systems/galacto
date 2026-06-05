# 🌌 Galaxy Sandbox

A GPU-accelerated **self-gravitating N-body** galaxy sandbox: ~16,000 bodies where every star pulls on every other. Switch between scenarios — a cold rotating disk that spontaneously grows **spiral arms**, or two galaxies that **merge** into one spinning remnant — and dial the disk temperature and speed live. Written in **Rust**, compiled to **WebAssembly**, and rendered with **WebGPU** — it runs entirely in the browser.

**Live:** [galacto.tre.systems](https://galacto.tre.systems/) — needs a [WebGPU-capable browser](#browser-support).

![A self-gravitating disk galaxy with spiral arms](screenshot.png)

## Features

- **GPU compute physics** — the all-pairs gravity for every body runs in a WebGPU compute shader (workgroup-tiled); the CPU never touches per-body state.
- **Self-gravity N-body** — every body attracts every other, so structure forms for real rather than being scripted.
- **Selectable scenarios** — a dropdown switches the initial conditions: a **spiral disk** that grows arms, plus five multi-galaxy setups — **merger**, **head-on collision**, **retrograde merger**, **minor merger** (a shredded satellite), and a **three-galaxy group**.
- **Live physics knobs** — gravity strength, dark-matter halo strength, and star size adjust the *running* simulation in real time (no restart); the galaxy collapses, disperses, or recolours as you drag.
- **Disk temperature** — sets the disk's velocity dispersion (≈ Toomre Q). It's a seed-time property, so it's *staged* and applied on **Restart** rather than disturbing the running sim.
- **Rust → WebAssembly** — the core compiles to WASM for near-native speed.
- **Interactive 3D camera** — orbit, zoom, pause, and reset, with mouse, keyboard, and touch.
- **Adjustable speed** — an on-screen slider scales the simulation from slow-motion up to 8× so the structure develops in seconds, with the fixed timestep keeping the physics frame-rate-independent.
- **Collapsible controls** — the control panel folds away to a small "Controls" tab so it stays out of the view.
- **Edge-deployed** — ships as a static site on Cloudflare Pages.

## Controls

### Desktop

| Input              | Action                              |
| ------------------ | ----------------------------------- |
| **Left-drag**        | Rotate (orbit) the camera           |
| **Mouse wheel**      | Zoom in and out                     |
| **Spacebar**         | Pause / resume the simulation       |
| **R**                | Reset the camera                    |
| **Scenario dropdown**| Choose the setup (spiral disk, or a merger / head-on / retrograde / minor / group collision) — re-seeds |
| **Speed slider**     | Scale simulation speed (0.25×–8×) — live |
| **Gravity slider**   | Scale gravity (0.25×–4×) — live; the galaxy collapses or disperses |
| **Halo slider**      | Dark-matter halo strength (0–2×) — live; confine or release the bodies |
| **Star-size slider** | On-screen star size — live |
| **Disk-temp slider** | Disk velocity dispersion (≈ Toomre Q, 0.02–2.0); staged, applied on **Restart** |
| **Restart** button   | Re-seed the current scenario from fresh initial conditions |

### Touch

| Input              | Action               |
| ------------------ | -------------------- |
| **One finger**     | Rotate the camera    |
| **Pinch**          | Zoom in and out      |

## Quick Start

### Prerequisites

- **Rust** — install from [rustup.rs](https://rustup.rs/)
- **Node.js** 20+ — for the build scripts
- **A WebGPU browser** — see [Browser Support](#browser-support)

### Installation

```bash
git clone https://github.com/tre-systems/galacto.git
cd galacto
npm run setup   # installs deps and adds the wasm32 target
npm run dev     # builds, then serves on http://localhost:8000
```

## Development

### Project Structure

- `src/` — Rust engine modules, with the WGSL shaders in `src/shaders/`
- `static/` — frontend assets (`index.html`, `styles.css`, `favicon.svg`, `_headers`)
- `docs/` — architecture writeup and diagrams
- `scripts/` — build cache-busting and diagram render/check
- `pkg/` — wasm-pack output, the deploy root (generated, git-ignored)

See [ARCHITECTURE § Repo Layout](docs/ARCHITECTURE.md#repo-layout) for the per-module breakdown.

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

A pre-commit hook and CI run the same gate — `fmt` / `clippy` / `test` / wasm `check` — and CI deploys on push to `main`. See [AGENTS § Verification](AGENTS.md#verification) for the exact commands.

## Architecture

![System overview](docs/diagrams/system-overview.png)

One `requestAnimationFrame` callback updates the camera, then per fixed step runs three GPU **compute** passes — a half-drift, an all-pairs gravity pass that sums each body's acceleration at the midpoint, then a kick + half-drift that advances it (a leapfrog step) — and issues one instanced **billboard** draw that reads the same buffer. Body state lives only in GPU memory — there is no CPU readback. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full picture.

## Physics

The model is a full **N-body** system: every body has mass and attracts every other (all-pairs gravity, O(N²)). The same solver drives both scenarios — only the initial conditions differ.

- **All-pairs gravity** — each body's acceleration is the Plummer-softened sum of the pull of every other body.
- **Dark-matter halo** — a static logarithmic halo adds an inward pull whose potential is unbounded, so the system stays bound (debris orbits back) with a flat outer rotation curve.
- **Symplectic leapfrog (drift–kick–drift)** — computed in three passes per step (half-drift, gravity at the midpoint, then kick + half-drift): `x += v·dt/2; v += a·dt; x += v·dt/2`. It is 2nd-order and conserves energy far better than plain Euler, so the cold disk and orbits hold their structure over many more rotations.
- **Spiral disk** — a heavy central bulge plus an exponential disk on near-circular prograde orbits, each given a random thermal kick scaled by the temperature slider (≈ Toomre Q). Cold fragments into clumps, hot stays a smooth smear, and **spiral arms** (swing-amplified density waves) live in between.
- **Galaxy merger** — two such disks, each anchored by a heavy core, on a bound prograde approach. Self-gravity and dynamical friction pull them together into one spinning remnant.

Everything derives from a fixed RNG seed, so a given scenario and temperature always evolve the same way. See [ARCHITECTURE § Simulation & Physics](docs/ARCHITECTURE.md#simulation--physics) for the kernels, the exact formulas, and the tuning constants.

## Documentation

- [Architecture](docs/ARCHITECTURE.md) — how the code is organized and how one frame is produced
- [Diagrams](docs/diagrams/README.md) — Graphviz system-overview, frame-loop, and GPU-buffer diagrams
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

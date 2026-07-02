# Galacto

A GPU-accelerated **self-gravitating N-body** galaxy sandbox: 16,384 bodies by default (adjustable up to 10×) where every body pulls on every other. Switch between spiral disks, galaxy collisions, and an M51-style flyby; tune Toomre Q, gas, bulge mass, speed, gravity, and the dark-matter halo; and hear a synthesized soundscape driven by the simulation. Written in **Rust**, compiled to **WebAssembly**, and rendered with **WebGPU** — it runs entirely in the browser.

**Live:** [galacto.org](https://galacto.org/) — needs a [WebGPU-capable browser](#browser-support).

![A grand-design spiral galaxy: a warm gold bulge, blue star-forming gas in the arms, and a companion drawing out a tidal bridge](screenshot.png)

## Features

- **GPU compute physics** — the all-pairs gravity for every body runs in a WebGPU compute shader (workgroup-tiled); the CPU never touches per-body state.
- **Self-gravity N-body** — every body attracts every other, so structure forms for real rather than being scripted.
- **Selectable scenarios** — a dropdown switches the initial conditions: a **spiral disk** that grows arms, plus six multi-galaxy setups — **merger**, **head-on collision**, **retrograde merger**, **minor merger** (a shredded satellite), a **three-galaxy group**, and a **grand-design (M51)** flyby whose companion tidally draws out a bridge and two-armed structure.
- **Adjustable body count** — a slider sets the number of bodies (16,384 by default, up to 10×), re-seeding the scenario at the new resolution. Per-body mass scales as 1/N, so more bodies refine the *same* galaxy rather than piling on mass. The top end is heavy — gravity is all-pairs O(N²), so high counts automatically cap simulation speed to keep the browser responsive.
- **Live physics knobs** — gravity strength, dark-matter halo strength and **size** (scale radius), and star size adjust the *running* simulation in real time (no restart); the galaxy collapses, disperses, or recolours as you drag, and the rotation curve reshapes with it. A **?** by every control explains what it does.
- **Galaxy-structure knobs** — beyond stability (Toomre Q), set how much of the disk is cold **gas** (the blue, star-forming arm component) and the **bulge** mass fraction (sweeping from disk-dominated late types to bulge-dominated early types). Both re-seed the galaxy.
- **Visualize the dark matter** — the otherwise-invisible halo can be toggled on as a soft violet glow centred on the galaxy, sized to the active profile's scale radius (broad for the logarithmic halo, tighter for NFW) so you can see the cloud the stars orbit within.
- **Live rotation curve** — an optional overlay plots the circular speed _v(r)_ in physical units (km/s vs kpc), decomposed into disk + bulge + dark-matter halo. The flat outer curve held up by the halo is the classic observational clue behind dark matter — drag the **Halo** or **Gravity** sliders and watch it respond. A clock shows the elapsed simulated time (the run is calibrated so one length unit ≈ 0.1 kpc and the default halo flattens at ~220 km/s).
- **Toomre Q (disk stability)** — the disk slider is the actual **Toomre stability parameter**: the radial velocity dispersion is set per-radius from Q (σ_R = Q·3.36GΣ/κ). Q≲1 fragments into clumps, Q≈1–2 swing-amplifies into spiral arms, Q≫2 stays a smooth smear — the textbook stability sequence, live. It's a seed-time property, so it's *staged* and applied on **Restart**.
- **Rust → WebAssembly** — the core compiles to WASM for near-native speed.
- **Generative soundscape** — starts on the first interaction and is fully synthesized with Web Audio (no samples). A pure Rust music engine maps camera state, controls, and a tiny GPU core-statistics readback into a calm cosmic-ambient bed: drone, sub-bass, starfield, reverb, delay, shimmer, and sparse notes. See [The Sound of Galacto](https://galacto.org/audio) for the research and design line.
- **Interactive 3D camera** — orbit, zoom, pause, and reset, with mouse, keyboard, and touch.
- **Adjustable speed** — an on-screen slider scales the simulation from slow-motion up to 8× so the structure develops in seconds, with the fixed timestep keeping the physics frame-rate-independent; very high body counts cap the effective speed for stability.
- **Collapsible controls** — the control panel folds away to a small ⚙ button so it stays out of the view.
- **Installable PWA** — a web manifest, maskable icons, and a service worker make it installable to the home screen and launchable offline; the precached app shell (glue, WASM, styles) also loads instantly on repeat visits, while navigation stays network-first so a new deploy shows immediately. When a new version is deployed, an in-app prompt offers a one-click reload, and an Open Graph card gives it a proper preview when shared.
- **Edge-deployed** — ships as a static site on Cloudflare Pages.

## Controls

### Desktop

| Input              | Action                              |
| ------------------ | ----------------------------------- |
| **Left-drag**        | Rotate (orbit) the camera           |
| **Mouse wheel**      | Zoom in and out                     |
| **Spacebar**         | Pause / resume the simulation       |
| **R**                | Reset the camera                    |
| **Scenario dropdown**| Choose the setup (spiral disk; a merger / head-on / retrograde / minor / group collision; or the grand-design M51 flyby) — re-seeds |
| **Bodies slider**    | Number of bodies (default 16,384, up to 10×) — re-seeds; high counts cap speed because gravity is all-pairs O(N²) |
| **Speed slider**     | Scale simulation speed (0.25×–8×) — live; reads out as Myr of simulated time per real second, with high-count caps shown as “max” |
| **Toomre Q slider** | Disk stability — the Toomre Q parameter (0.5–3.0): ≲1 clumps, ~1–2 spirals, ≫2 smooth; staged, applied on **Restart** |
| **Gas slider** | Fraction of the disk that is cold, star-forming gas (0–50%) — the blue arm component; re-seeds |
| **Bulge slider** | Central bulge's share of the mass (0–60%) — late-type (disk-dominated) to early-type (bulge-dominated); re-seeds |
| **Gravity slider**   | Scale gravity (0.25×–4×) — live; the galaxy collapses or disperses |
| **Dark matter halo** | A grouped section — **Model** (**Logarithmic**, flat curve, confines / **NFW**, rising-then-falling curve, debris can escape; re-seeds), **Strength** (0–2×, reads out in km/s — live), **Size** (the halo's scale radius in kpc — live; concentrated = steep inner rise, diffuse = gentle), **Show** (toggle a glowing violet overlay of the halo), and **Curve** (toggle the live rotation-curve chart) |
| **Star-size slider** | On-screen star size — live |
| **Restart** button   | Re-seed the current scenario from fresh initial conditions |
| **? icons**          | A click on the **?** by any control explains what it does |

### Touch

| Input              | Action               |
| ------------------ | -------------------- |
| **One finger**     | Rotate the camera    |
| **Pinch**          | Zoom in and out      |

## Quick Start

### Prerequisites

- **Rust** — install from [rustup.rs](https://rustup.rs/)
- **Node.js** 22+ — for the build scripts and Cloudflare Wrangler
- **librsvg** — for regenerating committed PWA icons and the Open Graph card (`brew install librsvg`)
- **A WebGPU browser** — see [Browser Support](#browser-support)

### Installation

From a local checkout:

```bash
cd galacto
npm run setup   # npm ci, wasm-pack 0.15.0, cargo-audit 0.22.2, and the wasm32 target
npm run dev     # builds, then serves on http://localhost:8000
```

## Development

### Project Structure

- `src/` — Rust engine modules, with the WGSL shaders in `src/shaders/`
- `static/` — frontend assets (`index.html`, `styles.css`, `favicon.svg`, `_headers`)
- `docs/` — architecture writeup and diagrams
- `scripts/` — build assembly/checks, local serving, smoke testing, diagrams, and production capture
- `pkg/` — raw wasm-pack output (generated, git-ignored)
- `dist/` — verified deploy root assembled from `static/` + `pkg/` (generated, git-ignored)

See [ARCHITECTURE § Repo Layout](docs/ARCHITECTURE.md#repo-layout) for the per-module breakdown.

### Key Commands

| Command                 | Description                                      |
| ----------------------- | ------------------------------------------------ |
| `npm run setup`         | Install lockfile dependencies, wasm-pack 0.15.0, cargo-audit 0.22.2, and the WASM target |
| `npm run build`         | Build WASM, assemble `dist/`, cache-bust, and verify the deploy artifact |
| `npm run dev`           | Build and serve on port 8000                     |
| `npm run deploy`        | Build, then deploy to Cloudflare Pages           |
| `npm run test`          | Run Rust tests                                   |
| `npm run lint`          | Run Clippy                                       |
| `npm run format`        | Format with rustfmt                              |
| `npm run check:js`      | Syntax-check repo-owned JavaScript scripts       |
| `npm run check:deploy-metadata` | Verify manifest identity, icon drift, and OG-card dimensions |
| `npm run verify:build`  | Check the generated `dist/` deploy artifact      |
| `npm run smoke`         | Browser-smoke the built site from `dist/`        |
| `npm run smoke:live`    | Browser-smoke the deployed site                  |
| `npm run audit`         | Run npm and RustSec dependency advisory checks   |
| `npm run diagrams`      | Render the architecture diagrams (needs Graphviz)|

A pre-commit hook runs the fast Rust gate — `fmt` / `clippy` / `test` / wasm `check` — and CI adds JavaScript syntax, dependency advisory, verified-build, and browser-smoke checks before deploying on push to `main`. See [AGENTS § Verification](AGENTS.md#verification) for the exact commands.

Deploy metadata is intentionally checked: `static/site.webmanifest` is the canonical PWA manifest, `static/manifest.json` is a byte-identical compatibility copy, and the committed PWA icons / Open Graph card are generated from `assets/` by `npm run icons`. `npm run check:deploy-metadata` enforces manifest identity, byte-for-byte icon drift, and the OG card's required 1200×630 PNG dimensions. The card contains text, so exact pixels are not compared across operating systems.

### Audio & video production

Finished pieces are rendered headlessly from a deterministic seed and duration:
the arrangement drives both the browser-captured visuals and the offline-rendered,
WASM-mastered audio. There is no in-app export UI.

```bash
npm run produce -- --seed 5 --duration 600
# -> a 10-minute MP4 (HEVC + AAC) with mastered audio and start/end captions
```

The default render uses 2x the interactive body count for a denser galaxy; per-particle size scales as `1/sqrt(count)`, so glow fill-rate stays roughly stable at 4K. See [Video production](docs/VIDEO_PRODUCTION.md).

## Architecture

![System overview](docs/diagrams/system-overview.png)

One `requestAnimationFrame` callback updates the camera, then per fixed step runs three GPU **compute** passes — a half-drift, an all-pairs gravity pass that sums each body's acceleration at the midpoint, then a kick + half-drift that advances it (a leapfrog step) — and issues one instanced **billboard** draw that reads the same buffer. Body state lives on the GPU; the only CPU readback is a tiny throttled aggregate used for audio. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full picture.

## Physics

The model is a full **N-body** system: every body has mass and attracts every other (all-pairs gravity, O(N²)). The same solver drives all scenarios — only the initial conditions differ.

- **All-pairs gravity** — each body's acceleration is the Plummer-softened sum of the pull of every other body.
- **Dynamical friction** — a Chandrasekhar drag against the dark-matter halo, scaled by each body's mass. It's negligible for the light disk stars but visibly decays the orbits of the heavy galaxy cores, so colliding galaxies lose orbital energy and **sink together into one remnant** instead of sailing past. It is one real mechanism that helps mergers finish instead of remaining long-lived flybys.
- **Dark-matter halo** — a static halo adds an inward pull, in one of two selectable profiles: a **logarithmic** halo (the default — an unbounded potential that keeps the system bound, with a flat outer rotation curve) or an **NFW** halo (the cold-dark-matter profile — a rotation curve that rises then falls, with a finite potential that lets fast debris escape). The spiral disk is seeded in equilibrium with whichever is active.
- **Symplectic leapfrog (drift–kick–drift)** — computed in three passes per step (half-drift, gravity at the midpoint, then kick + half-drift): `x += v·dt/2; v += a·dt; x += v·dt/2`. It is 2nd-order and conserves energy far better than plain Euler, so the cold disk and orbits hold their structure over many more rotations.
- **Spiral disk** — a compact central bulge plus an exponential disk on near-circular prograde orbits. The radial velocity dispersion is set per-radius from the **Toomre Q** slider (σ_R = Q·3.36GΣ/κ, with a softening/thickness correction for this finite, softened disk). Q≲1 fragments into clumps, Q≫2 stays a smooth smear, and **spiral arms** (swing-amplified density waves) live in between.
- **Cold gas** — about a quarter of the disk is a **dissipative gas** component: unlike the collisionless stars, it sheds random radial and vertical motion each step (a sticky-gas stand-in for shock cooling), so it stays a thin cold layer that piles up in the spiral arms. It is drawn bright blue, echoing the star-forming gas that traces real spiral arms; actual star formation is not simulated.
- **Galaxy merger** — two such disks, each anchored by a heavy core, on a bound prograde approach. Self-gravity and dynamical friction pull them together into one spinning remnant.

Initial conditions derive from a fixed RNG seed, so a given scenario and temperature start repeatably. Long-run paths are not promised bit-for-bit across GPUs. See [ARCHITECTURE § Simulation & Physics](docs/ARCHITECTURE.md#simulation--physics) for the kernels, the exact formulas, and the tuning constants.

## Documentation

Three illustrated pages for readers, live on the site:

- [**The Physics of Galacto**](https://galacto.org/physics) — the science: self-gravity, the leapfrog, dark-matter halos and flat rotation curves, the Toomre stability of spiral arms, dissipative gas, and dynamical friction — plus an honest account of what's real vs. illustrative
- [**Building Galacto**](https://galacto.org/engineering) — the engineering: a real-time GPU N-body in Rust + WebGPU, from the tiled all-pairs gravity kernel to the leapfrog compute passes and instanced rendering
- [**The Sound of Galacto**](https://galacto.org/audio) — the synthesized soundscape, how the galaxy drives it, and which design choices are evidence-backed vs. aesthetic

For contributors:

- [Architecture](docs/ARCHITECTURE.md) — how the code is organized and how one frame is produced
- [Diagrams](docs/diagrams/README.md) — Graphviz system-overview, frame-loop, and GPU-buffer diagrams
- [Video production](docs/VIDEO_PRODUCTION.md) — current one-command render workflow
- [Backlog](BACKLOG.md) — ordered next work and known constraints
- [Agent Notes](AGENTS.md) — workflow, verification, and architecture rules for agents

## Browser Support

WebGPU support changes quickly and still depends on OS, GPU, driver, and browser blocklists. For the best experience, use a current Chrome or Edge release on desktop. Current Firefox and Safari releases support WebGPU on some platforms, but availability is still more platform-dependent.

## License

The code is licensed under the GNU Affero General Public License v3.0 — see [LICENSE](LICENSE). You may use, study, modify, and share it; if you deploy a modified version (including as a network service), you must make your modified source available under the same license.

Copyright © 2025–2026 Multivibrator.

**Not licensed:** the Galacto and Multivibrator names, logos, and visual identity, and the released recordings and videos of composed pieces. Forks must not present themselves as Galacto or Multivibrator.

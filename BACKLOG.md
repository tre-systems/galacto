# Backlog

Forward-looking work, roughly ordered by what unblocks or de-risks the most — intent, not a changelog.

## P2 — Headless run mode

`cargo test` already covers the pure CPU logic that needs no GPU (camera math, scenario seeding, the `Particle` / `SimulationParams` buffer-layout contract — see [AGENTS § Tests](AGENTS.md#tests)).

The next step is an `examples/headless.rs` that steps the simulation without a browser — for profiling, and (paired with a CPU reference implementation of the gravity step) for validating the GPU solver: energy behaviour over a run, or regression-checking a scenario. The engine is FFI-free (`graphics` / `simulation` / `camera` / `scenarios` carry no `JsValue`), so a native harness only needs to stand up a headless `wgpu` device, or skip the GPU entirely for a CPU reference path. This also gates the tree-gravity work below — an approximate force can only be trusted against an exact reference.

## Roadmap — simulation depth

The solver in `src/shaders/update.wgsl` runs the same for every `Scenario` (`src/scenarios.rs`) — only the seeded initial conditions differ, and new setups are cheap to add through the shared `push_disk_star` / `seed_galaxy` helpers. Larger directions, roughly by effort:

- **Grand-design spirals (M51)** — a clean two-armed pattern wants a tidal driver: a scenario with a small companion on a prograde flyby past a cold disk that survives rather than fully merging.
- **Live particle halo** — the static halo (logarithmic or NFW, selectable) is a fixed background force; making it a *live* population of dark-matter particles would let it respond dynamically to the disk and to mergers, at O(N²) cost on the body budget.
- **Auto-replay** — periodically re-seed so an unattended demo keeps showing fresh structure (the disk heats and the arms fade over many rotations).

### Tree gravity — scale past the O(N²) ceiling

Gravity is the exact all-pairs sum (`compute_accel` in `src/shaders/update.wgsl`), `O(N²)` per step, which caps the count near ~16k for interactive speed (and leaves the arms a little grainy). Reaching 100k–1M bodies needs an approximate force.

Recommended approach: a **GPU LBVH / Barnes–Hut** tree — build a linear tree each step from Morton (Z-order) codes, then traverse stacklessly with a multipole opening criterion (θ). Alternatives fit worse: a particle-mesh/FFT solver is `O(N)` and simplest to build, but a fixed grid fights the sim's huge dynamic range and unbounded extent; a fixed-depth octree wastes cells on the dense bulge.

WebGPU crux (no recursion, no dynamic allocation, weak atomics): tree build is body-AABB → Morton-encode → **radix sort** the codes (the hard part — per-digit histogram + prefix-scan + scatter, several dispatches) → build internal nodes (Karras' branch-free LBVH) → bottom-up centre-of-mass pass; traversal uses a short fixed register stack or rope/skip-pointer trees.

Reality check: Barnes–Hut has high constant factors and divergent, scattered memory access — the opposite of the current branch-free, coalesced, tile-cooperative kernel, which is near-peak GPU efficiency. Honest crossover is ~`N = 100k–250k`; below ~64k the tiled all-pairs sum still wins, and it trades away a selling point (exact, every-pair self-gravity). Worth it only if 100k+ bodies becomes an explicit goal, and only after the headless + CPU-reference harness (P2) exists. **Effort: XL** (the GPU radix sort alone is M–L).

### Gas physics — star formation and mergers

The disk scenarios carry a dissipative gas component: a fraction of bodies are tagged as gas (via `vel.w`, gated by a per-scenario `has_gas` flag), cooled each step in `kick_drift_half` toward circular, in-plane orbits, and drawn blue. Cold gas therefore gathers in and sustains the spiral arms. What's missing is the physics this only gestures at:

- **Actual star formation** — convert gas to stars where it is densest, with fresh stars starting blue and reddening with age. Needs a per-body local-density estimate (the neighbour search below) plus an age field; the warm→blue colour ramp already exists.
- **Gas in mergers** — the multi-galaxy scenarios are collisionless (gas-free). Real mergers shock-compress gas into blue tidal tails and central starbursts. The merger render path uses `vel.w` for galaxy-of-origin tint, so merger gas needs a second flag to disambiguate it (the `aux` buffer below).
- **Velocity-mean drag** — the cooling damps each gas body's own non-circular motion (a stand-in); a truer sticky gas nudges each body toward its *neighbours'* bulk velocity, which also handles non-disk geometry (tails, mergers). Needs the neighbour search.
- **Full SPH** — kernel density + pressure + artificial viscosity + equation of state + cooling: the correct-physics version. **XL** and fiddly in single precision with no readback.

Neighbour-search crux (the shared prerequisite, the genuinely hard part on WebGPU): reuse the tiled `O(N²)` sweep in `compute_accel` to also accumulate a kernel-weighted density and mean velocity — one extra compute pass, same shared-memory tiling, no atomics. At N=16k a second `O(N²)` pass is affordable. A uniform spatial-hash grid (atomic per-cell counters + counting sort) is the scalable answer but is the hard WGSL piece; defer it until the body count outgrows the all-pairs sweep.

Data layout: gas rides in `vel.w` (shared with the colour tint), which is why merger gas would need disambiguating. A dedicated per-body state field (density, age, type) wants a parallel `aux` storage buffer (one `vec4`/body) plus a new compute bind-group entry. **Effort: M** for star formation given the neighbour pass; full SPH is a separate **XL**, with the spatial-hash grid an **L** prerequisite of its own.

## Audio — deeper coupling

The generative soundscape (`src/music.rs` + `src/audio.rs`) is driven by the visuals — the camera, the scenario, the live knobs, and the galaxy's own core dynamics (central mass + radial flux) from the throttled `reduce_core` GPU readback (see [ARCHITECTURE § Audio](docs/ARCHITECTURE.md#audio)). One direction remains to close the loop:

- **Audio-reactive visuals.** The reverse of the current coupling — let note onsets or the pad's energy feed a subtle visual response (a bloom/exposure pulse, or a brightness nudge) so the two reinforce each other, as in the sibling `geno` projects. Cheap once an audio energy signal exists; the care is in keeping it tasteful and not fighting the existing star-size / bloom look. **Effort: S–M.**

Richer core signals are a cheap extension if the sound wants more nuance: the `reduce_core` reduction already returns windowed central mass + radial flux, and could add velocity dispersion, net angular momentum, or a coarse radial histogram in the same pass.

## Production export

The YouTube-production path in [docs/VIDEO_PRODUCTION.md](docs/VIDEO_PRODUCTION.md) wants direct media export rather than browser screen capture:

- **Recording mode.** Hide UI, lock a camera path/timeline, and add clean start/end fades so browser capture can make a decent proof cut. **Effort: S.**
- **Offline audio export.** Reuse the pure `MusicEngine` to render 48 kHz WAV stems, MIDI/JSON note events, and automation curves for Logic, instead of recording the browser's mixed Web Audio output. This gives the biggest audio-production gain first. **Effort: M.**
- **Headless video export.** Add a native `wgpu` binary that runs the same simulation/camera timeline into an offscreen texture, reads back each tonemapped frame, and writes a PNG/TIFF sequence for ffmpeg. This avoids capture compression and frame drops, but is more engineering than audio export. **Effort: L.**

## Definition of Done

A change is done when:

- The verification gate passes (see [AGENTS § Verification](AGENTS.md#verification)); the pre-commit hook enforces it.
- Docs describing affected behaviour are updated to match.
- For user-visible changes: pushed, CI green, and smoke-tested on the live site.

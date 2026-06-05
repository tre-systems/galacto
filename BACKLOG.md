# Backlog

Forward-looking work, roughly ordered by what unblocks or de-risks the most — intent, not a changelog.

## P2 — Headless run mode

`cargo test` already covers the pure CPU logic that needs no GPU (camera math, scenario seeding, the `Particle` / `SimulationParams` buffer-layout contract — see [AGENTS § Tests](AGENTS.md#tests)).

The next step is an `examples/headless.rs` that steps the simulation without a browser — for profiling, and (paired with a CPU reference implementation of the gravity step) for validating the GPU solver: energy behaviour over a run, or regression-checking a scenario. The engine is FFI-free (`graphics` / `simulation` / `camera` / `scenarios` carry no `JsValue`), so a native harness only needs to stand up a headless `wgpu` device, or skip the GPU entirely for a CPU reference path. This also gates the tree-gravity work below — an approximate force can only be trusted against an exact reference.

## Roadmap — simulation depth

The solver in `src/shaders/update.wgsl` runs the same for every `Scenario` (`src/scenarios.rs`) — only the seeded initial conditions differ, and new setups are cheap to add through the shared `push_disk_star` / `seed_galaxy` helpers. Larger directions, roughly by effort:

- **Grand-design spirals (M51)** — a clean two-armed pattern wants a tidal driver: a scenario with a small companion on a prograde flyby past a cold disk that survives rather than fully merging.
- **Richer dark-matter halo** — the static logarithmic halo could gain an NFW profile (a rising-then-falling rotation curve), or become a *live* particle halo that responds dynamically (at O(N²) cost on the body budget).
- **Auto-replay** — periodically re-seed so an unattended demo keeps showing fresh structure (the disk heats and the arms fade over many rotations).

### Tree gravity — scale past the O(N²) ceiling

Gravity is the exact all-pairs sum (`compute_accel` in `src/shaders/update.wgsl`), `O(N²)` per step, which caps the count near ~16k for interactive speed (and leaves the arms a little grainy). Reaching 100k–1M bodies needs an approximate force.

Recommended approach: a **GPU LBVH / Barnes–Hut** tree — build a linear tree each step from Morton (Z-order) codes, then traverse stacklessly with a multipole opening criterion (θ). Alternatives fit worse: a particle-mesh/FFT solver is `O(N)` and simplest to build, but a fixed grid fights the sim's huge dynamic range and unbounded extent; a fixed-depth octree wastes cells on the dense bulge.

WebGPU crux (no recursion, no dynamic allocation, weak atomics): tree build is body-AABB → Morton-encode → **radix sort** the codes (the hard part — per-digit histogram + prefix-scan + scatter, several dispatches) → build internal nodes (Karras' branch-free LBVH) → bottom-up centre-of-mass pass; traversal uses a short fixed register stack or rope/skip-pointer trees.

Reality check: Barnes–Hut has high constant factors and divergent, scattered memory access — the opposite of the current branch-free, coalesced, tile-cooperative kernel, which is near-peak GPU efficiency. Honest crossover is ~`N = 100k–250k`; below ~64k the tiled all-pairs sum still wins, and it trades away a selling point (exact, every-pair self-gravity). Worth it only if 100k+ bodies becomes an explicit goal, and only after the headless + CPU-reference harness (P2) exists. **Effort: XL** (the GPU radix sort alone is M–L).

### Dissipative gas — keep the disk cold

Every body is collisionless (pure softened gravity + halo), so disks self-heat and the spiral arms fade; nothing shocks, cools, or settles into denser, star-forming structure.

Recommended first step — a **cheap local-dissipation hack, not full SPH**: tag a fraction of bodies as "gas" and each step nudge their velocity toward the local mean (a drag toward neighbours' bulk motion). That damps random motion — the thermodynamic role of cooling — for ~80% of the visual payoff (cooler, flatter, longer-lived arms) at a fraction of the cost. Full SPH (kernel density + pressure + artificial viscosity + equation of state + cooling) is the correct-physics version but is XL and fiddly to keep stable in single precision with no readback.

Neighbour-search crux (the genuinely hard part on WebGPU): reuse the existing tiled `O(N²)` sweep in `compute_accel` to also accumulate a kernel-weighted density and mean velocity — one extra compute pass, same shared-memory tiling, no atomics. At N=16k a second `O(N²)` pass is affordable. A uniform spatial-hash grid (atomic per-cell counters + counting sort) is the scalable answer but is the hard WGSL piece; defer it until the body count outgrows the all-pairs sweep.

Data layout: `Particle` is full — `pos_mass.w` is mass and `vel.w` is the colour tint (32 B, asserted by a layout test), so gas needs either a sign trick on `pos_mass.w` for the type flag or, cleaner, a parallel `aux` storage buffer (one `vec4`/body: type, density, scratch) plus a new compute bind-group entry. **Effort: M** for the hack (one extra pass, an aux buffer, a seed tweak, one slider); full SPH is a separate **XL**, with the spatial-hash grid an **L** prerequisite of its own.

## Definition of Done

A change is done when:

- The verification gate passes (see [AGENTS § Verification](AGENTS.md#verification)); the pre-commit hook enforces it.
- Docs describing affected behaviour are updated to match.
- For user-visible changes: pushed, CI green, and smoke-tested on the live site.

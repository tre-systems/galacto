# Backlog

Forward-looking work, roughly ordered by what unblocks or de-risks the most — intent, not a changelog.

## P2 — Headless run mode

`cargo test` already covers the pure CPU logic that needs no GPU (camera math, scenario seeding, the `Particle` / `SimulationParams` buffer-layout contract — see [AGENTS § Tests](AGENTS.md#tests)).

The next step is an `examples/headless.rs` that steps the simulation without a browser — for profiling, and (paired with a CPU reference implementation of the gravity step) for validating the GPU integrator: energy behaviour over a run, or regression-checking a scenario. The engine is FFI-free (`graphics` / `simulation` / `camera` / `scenarios` carry no `JsValue`), so a native harness only needs to stand up a headless `wgpu` device, or skip the GPU entirely for a CPU reference path.

## P3 — Dependency freshness

`wgpu` is pinned at 24 and `rand` at 0.8. Bumping `wgpu` (→ 25+) is mechanical churn in `graphics.rs`/`simulation.rs` (instance/adapter/device descriptor changes, surface-texture and render-pass field renames — the same shape evo's backlog scouts for its own bump). `rand` 0.8 → 0.9 touches `gen_range`. Low urgency: the current versions build clean and the toolchain tracks stable (`rust-toolchain.toml`), not a pinned version. Note: a transitive `block v0.1.6` future-incompat warning comes from `wgpu`'s macOS Metal backend and only affects native builds, not the WASM deploy — it clears when `wgpu` is bumped.

## Roadmap — simulation depth

A `Scenario` (`src/scenarios.rs`, on the tiled all-pairs solver in `src/shaders/update.wgsl`) selects the initial conditions: a self-gravitating cold disk that swing-amplifies into flocculent spiral arms (with a disk-temperature slider for the Toomre-Q regime), or a two-galaxy merger. New setups are cheap to add — both scenarios build their disks through the shared `push_disk_star` helper. Richer behaviour to consider, roughly by effort:

- **Scale up the body count** — the all-pairs sum is O(N²), which caps the count around ~16k for interactive speed; the spiral arms are flocculent and a bit grainy at that count. A Barnes-Hut tree or a particle-mesh/FFT solver would allow far more bodies (cleaner, sharper arms) at the cost of a much larger implementation.
- **Sustain the arms** — flocculent spirals self-heat the disk (Q rises), so the pattern fades over many rotations until a re-seed. A dissipative (gas) component that cools/circularizes a fraction of bodies would keep the disk cold and the arms alive.
- **Grand-design spirals (M51)** — a clean two-armed pattern wants a tidal driver: a new scenario with a small companion on a prograde flyby past a cold disk (it survives rather than fully merging).
- **More scenarios** — `Scenario` makes new setups cheap: unequal-mass or retrograde mergers, a head-on collision, or a small group of 3+ galaxies.
- **Richer dark-matter halo** — the static logarithmic halo could become an NFW profile or a *live* particle halo for more realistic dynamics.
- **Leapfrog (KDK) integrator** — the current step is symplectic Euler (one force eval/step). Kick-drift-kick leapfrog conserves energy better over long runs, at the cost of a second gravity pass per step.
- **Auto-replay** — periodically re-seed (the spiral disk heats and the arms fade over time), so the demo keeps showing fresh structure.

## Definition of Done

A change is done when:

- The verification gate passes (see [AGENTS § Verification](AGENTS.md#verification)); the pre-commit hook enforces it.
- Docs describing affected behaviour are updated to match.
- For user-visible changes: pushed, CI green, and smoke-tested on the live site.

# Backlog

Forward-looking work, roughly ordered by what unblocks or de-risks the most. Present tense; this is intent, not history.

## P2 — Tests + headless run mode

The crate is `cdylib`-only, so there is no native way to unit-test or profile, and `cargo test` runs zero tests today. Add `"rlib"` to `crate-type` and cover the pure logic that does not need a GPU:

- Camera math — `build_view_projection_matrix`, the `scale`/rotation clamps, `pan`/`zoom` behaviour.
- Disk initialization invariants — body count equals `NUM_PARTICLES` (a multiple of `WORKGROUP_SIZE`), every body has positive mass, disk radii stay within `DISK_RMAX`, the velocity dispersion scales with the temperature argument, and generation is deterministic (it seeds `StdRng` from a fixed `42`, so a given temperature is reproducible).

This also unlocks a future `examples/headless.rs` for stepping the sim without a browser, if a CPU reference path is ever wanted. The engine is now FFI-free (`graphics`/`simulation`/`camera` carry no `JsValue`), so the only thing between here and native tests is the `rlib` crate-type.

## P3 — Dependency freshness

`wgpu` is pinned at 24 and `rand` at 0.8. Bumping `wgpu` (→ 25+) is mechanical churn in `graphics.rs`/`simulation.rs` (instance/adapter/device descriptor changes, surface-texture and render-pass field renames — the same shape evo's backlog scouts for its own bump). `rand` 0.8 → 0.9 touches `gen_range`. Low urgency: the current versions build clean and the toolchain is stable, not pinned. Note: a transitive `block v0.1.6` future-incompat warning comes from `wgpu`'s macOS Metal backend and only affects native builds, not the WASM deploy — it clears when `wgpu` is bumped.

## Roadmap — simulation depth

A `Scenario` (`src/simulation.rs`, on the tiled all-pairs solver in `src/shaders/update.wgsl`) selects the initial conditions: a self-gravitating cold disk that swing-amplifies into flocculent spiral arms (with a disk-temperature slider for the Toomre-Q regime), or a two-galaxy merger. Richer behaviour to consider, roughly by effort:

- **Scale up the body count** — the all-pairs sum is O(N²), which caps the count around ~16k for interactive speed; the spiral arms are flocculent and a bit grainy at that count. A Barnes-Hut tree or a particle-mesh/FFT solver would allow far more bodies (cleaner, sharper arms) at the cost of a much larger implementation.
- **Sustain the arms** — flocculent spirals self-heat the disk (Q rises), so the pattern fades over many rotations until a re-seed. A dissipative (gas) component that cools/circularizes a fraction of bodies would keep the disk cold and the arms alive.
- **Grand-design spirals (M51)** — a clean two-armed pattern wants a tidal driver: a new scenario with a small companion on a prograde flyby past a cold disk (it survives rather than fully merging).
- **More scenarios** — `Scenario` makes new setups cheap: unequal-mass or retrograde mergers, a head-on collision, or a small group of 3+ galaxies.
- **Richer dark-matter halo** — the static logarithmic halo could become an NFW profile or a *live* particle halo for more realistic dynamics.
- **Leapfrog (KDK) integrator** — the current step is symplectic Euler (one force eval/step). Kick-drift-kick leapfrog conserves energy better over long runs, at the cost of a second gravity pass per step.
- **Auto-replay** — periodically re-seed (the spiral disk heats and the arms fade over time), so the demo keeps showing fresh structure.

## Definition of Done

A change is done when:

- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and `cargo check --target wasm32-unknown-unknown` all pass (the pre-commit hook enforces these).
- Docs describing affected behaviour are updated to match.
- For user-visible changes: pushed, CI green, and smoke-tested on the live site.

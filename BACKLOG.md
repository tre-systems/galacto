# Backlog

Forward-looking work, roughly ordered by what unblocks or de-risks the most. Present tense; this is intent, not history.

## P2 — Tests + headless run mode

The crate is `cdylib`-only, so there is no native way to unit-test or profile, and `cargo test` runs zero tests today. Add `"rlib"` to `crate-type` and cover the pure logic that does not need a GPU:

- Camera math — `build_view_projection_matrix`, the `scale`/rotation clamps, `pan`/`zoom` behaviour.
- Galaxy initialization invariants — total star count equals `NUM_PARTICLES` (the per-galaxy split sums exactly), core count equals `NUM_CORES`, disk radii stay in range, and generation is deterministic (it already seeds `StdRng` from a fixed `42`).

This also unlocks a future `examples/headless.rs` for stepping the sim without a browser, if a CPU reference path is ever wanted. The engine is now FFI-free (`graphics`/`simulation`/`camera` carry no `JsValue`), so the only thing between here and native tests is the `rlib` crate-type.

## P3 — Dependency freshness

`wgpu` is pinned at 24 and `rand` at 0.8. Bumping `wgpu` (→ 25+) is mechanical churn in `graphics.rs`/`simulation.rs` (instance/adapter/device descriptor changes, surface-texture and render-pass field renames — the same shape evo's backlog scouts for its own bump). `rand` 0.8 → 0.9 touches `gen_range`. Low urgency: the current versions build clean and the toolchain is stable, not pinned. Note: a transitive `block v0.1.6` future-incompat warning comes from `wgpu`'s macOS Metal backend and only affects native builds, not the WASM deploy — it clears when `wgpu` is bumped.

## Roadmap — simulation depth

The model is a restricted N-body interaction: two massive cores move under mutual gravity and the star disks are massless test particles in their softened field (`src/simulation.rs`, `src/shaders/update.wgsl`). The stars feel the cores but not each other, so the disks have no self-gravity and slowly disperse over many passages. Richer behaviour to consider, roughly by effort:

- **Static dark-matter halos** — give each core an NFW or logarithmic halo potential for a flat rotation curve, so disks hold together more like real galaxies. A few extra lines in the gravity sum.
- **Auto-replay** — after the interaction disperses, fade and re-seed from the initial conditions so the demo loops instead of emptying out.
- **More galaxies / mass ratios** — `NUM_CORES` already generalizes the cores and disks; expose unequal masses, retrograde disks, and 3+ galaxies (a small group) for variety.
- **True self-gravity** — let the stars attract each other (and the cores) for genuine N-body dynamics. Brute-force is O(N²) (tiled shared-memory reduction); a Barnes-Hut tree or a particle-mesh/FFT solver scales better but is a much larger change.
- **Render the cores** — draw the cores as distinct bright nuclei rather than relying on the dense inner disk to glow.

## Definition of Done

A change is done when:

- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and `cargo check --target wasm32-unknown-unknown` all pass (the pre-commit hook enforces these).
- Docs describing affected behaviour are updated to match.
- For user-visible changes: pushed, CI green, and smoke-tested on the live site.

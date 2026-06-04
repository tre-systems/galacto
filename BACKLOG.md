# Backlog

Forward-looking work, roughly ordered by what unblocks or de-risks the most. Present tense; this is intent, not history.

## P2 — Tests + headless run mode

The crate is `cdylib`-only, so there is no native way to unit-test or profile, and `cargo test` runs zero tests today. Add `"rlib"` to `crate-type` and cover the pure logic that does not need a GPU:

- Camera math — `build_view_projection_matrix`, the `scale`/rotation clamps, `pan`/`zoom` behaviour.
- Galaxy initialization invariants — body count equals `NUM_PARTICLES` (a multiple of `WORKGROUP_SIZE`, with the per-galaxy split summing exactly), every body has positive mass, disk radii stay in range, and generation is deterministic (it already seeds `StdRng` from a fixed `42`).

This also unlocks a future `examples/headless.rs` for stepping the sim without a browser, if a CPU reference path is ever wanted. The engine is now FFI-free (`graphics`/`simulation`/`camera` carry no `JsValue`), so the only thing between here and native tests is the `rlib` crate-type.

## P3 — Dependency freshness

`wgpu` is pinned at 24 and `rand` at 0.8. Bumping `wgpu` (→ 25+) is mechanical churn in `graphics.rs`/`simulation.rs` (instance/adapter/device descriptor changes, surface-texture and render-pass field renames — the same shape evo's backlog scouts for its own bump). `rand` 0.8 → 0.9 touches `gen_range`. Low urgency: the current versions build clean and the toolchain is stable, not pinned. Note: a transitive `block v0.1.6` future-incompat warning comes from `wgpu`'s macOS Metal backend and only affects native builds, not the WASM deploy — it clears when `wgpu` is bumped.

## Roadmap — simulation depth

The model is a full self-gravitating N-body merger: every body attracts every other through the tiled all-pairs sum, so two galaxies coalesce into one bound, rotating remnant (`src/simulation.rs`, `src/shaders/update.wgsl`). Richer behaviour to consider, roughly by effort:

- **Scale up the body count** — the all-pairs sum is O(N²), which caps the count around ~16k for interactive speed. A Barnes-Hut tree or a particle-mesh/FFT solver would allow far more bodies (denser, prettier galaxies) at the cost of a much larger implementation.
- **A dissipative (gas) component** — a collisionless merger relaxes into a puffy elliptical-like remnant. Letting some fraction of bodies shed energy (mimicking gas) would let a thin rotating disk and grand-design spiral arms re-form after the merger.
- **Tidally-induced spirals (M51)** — a single self-gravitating cold disk plus a companion flyby produces grand-design spiral arms without a full merger; needs a disk tuned near Toomre `Q ≈ 1.5`.
- **Richer dark-matter halo** — a single static logarithmic halo (centred at the origin) already binds the system; consider an NFW profile, a *live* (particle) halo, or per-galaxy halos that merge for more realistic dynamics.
- **More galaxies / mass ratios** — `NUM_GALAXIES` generalizes the setup; expose unequal masses, retrograde disks, and 3+ galaxies (a small group).
- **Leapfrog (KDK) integrator** — the current step is symplectic Euler (one force eval/step). Kick-drift-kick leapfrog conserves energy better over long runs, at the cost of a second gravity pass per step.
- **Auto-replay** — re-seed from the initial conditions once the remnant has settled, so the demo loops.

## Definition of Done

A change is done when:

- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and `cargo check --target wasm32-unknown-unknown` all pass (the pre-commit hook enforces these).
- Docs describing affected behaviour are updated to match.
- For user-visible changes: pushed, CI green, and smoke-tested on the live site.

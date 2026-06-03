# Backlog

Forward-looking work, roughly ordered by what unblocks or de-risks the most. Present tense; this is intent, not history.

## P2 — Tests + headless run mode

The crate is `cdylib`-only, so there is no native way to unit-test or profile, and `cargo test` runs zero tests today. Add `"rlib"` to `crate-type` and cover the pure logic that does not need a GPU:

- Camera math — `build_view_projection_matrix`, the `scale`/rotation clamps, `pan`/`zoom` behaviour.
- Particle initialization invariants — count equals `NUM_PARTICLES`, the close-star radii stay in range, the stream seeding is deterministic (it already seeds `StdRng` from a fixed `42`).

This also unlocks a future `examples/headless.rs` for stepping the sim without a browser, if a CPU reference path is ever wanted. The engine is now FFI-free (`graphics`/`simulation`/`camera` carry no `JsValue`), so the only thing between here and native tests is the `rlib` crate-type.

## P3 — Shrink the WASM bundle

`wasm-opt` is disabled (`wasm-opt = false` in `Cargo.toml`'s `[package.metadata.wasm-pack.profile.release]`, plus `--no-opt` on the build command), so the shipped `.wasm` is unoptimized for size. Re-enable `wasm-opt` (or run it as a build step) and confirm it does not miscompile the `wgpu` output, which is the usual reason a wgpu project disables it. Measure the before/after download size.

## P3 — Dependency freshness

`wgpu` is pinned at 24 and `rand` at 0.8. Bumping `wgpu` (→ 25+) is mechanical churn in `graphics.rs`/`simulation.rs` (instance/adapter/device descriptor changes, surface-texture and render-pass field renames — the same shape evo's backlog scouts for its own bump). `rand` 0.8 → 0.9 touches `gen_range`. Low urgency: the current versions build clean and the toolchain is stable, not pinned. Note: a transitive `block v0.1.6` future-incompat warning comes from `wgpu`'s macOS Metal backend and only affects native builds, not the WASM deploy — it clears when `wgpu` is bumped.

## Roadmap — simulation depth

The visual is driven mostly by one injected particle stream plus ~500 seeded close-orbit stars (`src/simulation.rs`); gravity is to a single fixed central mass (O(N), not N-body). Richer behaviour to consider: shape a genuine accretion disk with a spread of orbital radii, add a relativistic-style color/Doppler shift, an event-horizon cutoff that removes particles crossing a Schwarzschild-like radius, or a second body. Each is a self-contained shader + init change.

## Definition of Done

A change is done when:

- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and `cargo check --target wasm32-unknown-unknown` all pass (the pre-commit hook enforces these).
- Docs describing affected behaviour are updated to match.
- For user-visible changes: pushed, CI green, and smoke-tested on the live site.

# Backlog

Forward-looking work, roughly ordered by what unblocks or de-risks the most. Present tense; this is intent, not history.

## P2 — Tests + headless run mode

The crate is `cdylib`-only, so there is no native way to unit-test or profile, and `cargo test` runs zero tests today. Add `"rlib"` to `crate-type` and cover the pure logic that does not need a GPU:

- Camera math — `build_view_projection_matrix`, the `scale`/rotation clamps, `pan`/`zoom` behaviour.
- Particle initialization invariants — count equals `NUM_PARTICLES`, the close-star radii stay in range, the stream seeding is deterministic (it already seeds `StdRng` from a fixed `42`).

This also unlocks a future `examples/headless.rs` for stepping the sim without a browser, if a CPU reference path is ever wanted.

## P2 — Pattern consistency

Places that don't yet follow the pattern they belong to (see "Known consistency gaps" in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)):

- **Wire or remove resize.** `AppState::resize` / `Graphics::resize` / `Camera::set_aspect_ratio` exist but nothing calls them: there is no `resize` listener, the canvas is pinned at 1024×768, and the camera aspect stays 1.0 while CSS stretches the canvas to the viewport — so the render is distorted on any non-4:3 window and upscaled on large displays. Either add a `resize` handler (size to `clientWidth/Height × devicePixelRatio`, reconfigure the surface + depth texture, update the aspect) or delete the unused methods. Wiring it is the better call — it fixes a real visual bug.
- **Uniform `Result` at the boundary.** `Graphics::new` `panic!`s on a missing adapter and `unwrap()`s `window()`/`document()` instead of returning `Err(JsValue)`. Make the whole setup path return `Result` so `index.html`'s catch handler can show a real message — pair this with enabling the panic hook (below).
- **One logging path.** `start()` initializes the `log` facade via `console_log::init_with_level`, but every log uses the custom `console_log!` macro and nothing uses `log::*`. Either route logging through `log` (keep the facade) or drop the `log` + `console_log` dependencies and keep only the macro.
- **Lift physics constants into `SimulationParams`.** `max_velocity`, the world `boundary`, the restitution, and the color/brightness constants are hardcoded in the shaders while `dt`/`gm` live in the params uniform. Move the ones worth tuning into `SimulationParams` (the AGENTS.md rule), or scope the rule explicitly to runtime-varied scalars.

## P3 — Production panic visibility

`console_error_panic_hook` is an optional dependency and **not** in the `default` features (`Cargo.toml`), so `set_panic_hook()` (`src/utils.rs`) is a no-op in the deployed build — a panic in the browser surfaces as an opaque `RuntimeError` with no message. Either add `console_error_panic_hook` to `default` (small size cost, much better diagnostics) or remove the dead scaffolding. Enabling it is the better call while the project is still evolving.

## P3 — Shrink the WASM bundle

`wasm-opt` is disabled (`wasm-opt = false` in `Cargo.toml`'s `[package.metadata.wasm-pack.profile.release]`, plus `--no-opt` on the build command), so the shipped `.wasm` is unoptimized for size. Re-enable `wasm-opt` (or run it as a build step) and confirm it does not miscompile the `wgpu` output, which is the usual reason a wgpu project disables it. Measure the before/after download size.

## P3 — Drop `unsafe` from global state

`src/lib.rs` holds the app in `static mut APP_STATE` and reaches it through `unsafe` (`&raw const APP_STATE`). It is sound because WASM here is single-threaded, but a `thread_local! { static APP_STATE: RefCell<Option<Rc<RefCell<AppState>>>> }` (or a `OnceCell`) removes the `unsafe` entirely with no behaviour change.

## P3 — Dependency freshness

`wgpu` is pinned at 24 and `rand` at 0.8. Bumping `wgpu` (→ 25+) is mechanical churn in `graphics.rs`/`simulation.rs` (instance/adapter/device descriptor changes, surface-texture and render-pass field renames — the same shape evo's backlog scouts for its own bump). `rand` 0.8 → 0.9 touches `gen_range`. Low urgency: the current versions build clean and the toolchain is stable, not pinned. Note: a transitive `block v0.1.6` future-incompat warning comes from `wgpu`'s macOS Metal backend and only affects native builds, not the WASM deploy — it clears when `wgpu` is bumped.

## P3 — Adopt a fixed-timestep integration

Physics advances by the real frame delta (`dt`, capped at 0.033) fed straight into the Euler step, so trajectories depend on refresh rate — a 144 Hz and a 60 Hz display evolve the same seed differently. Adopt a **fixed-timestep accumulator**: accumulate real time and run a whole number of fixed-`dt` compute dispatches per frame, restoring frame-rate-independent, deterministic motion. Because galacto steps on the GPU every frame, it does not need the render-side interpolation a fixed-tick CPU sim (like evo) uses — unless sub-stepping makes motion look visibly discrete.

## P3 — Adopt an FFI-free core

`JsValue` and the wasm-bindgen boundary leak into `graphics.rs` and `simulation.rs` (both return `Result<_, JsValue>`). Confine the JS boundary to `lib.rs`: have the GPU/sim modules return a domain error (or `String` / `thiserror`) and convert to `JsValue` only at the `#[wasm_bindgen]` edge. Combined with the `rlib` crate-type from the tests item, this lets the engine be reasoned about and tested without the JS types. The related "safe-global" pattern (no `static mut`) is the *Drop `unsafe` from global state* item above.

## Roadmap — simulation depth

The visual is driven mostly by one injected particle stream plus ~500 seeded close-orbit stars (`src/simulation.rs`); gravity is to a single fixed central mass (O(N), not N-body). Richer behaviour to consider: shape a genuine accretion disk with a spread of orbital radii, add a relativistic-style color/Doppler shift, an event-horizon cutoff that removes particles crossing a Schwarzschild-like radius, or a second body. Each is a self-contained shader + init change.

## Definition of Done

A change is done when:

- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and `cargo check --target wasm32-unknown-unknown` all pass (the pre-commit hook enforces these).
- Docs describing affected behaviour are updated to match.
- For user-visible changes: pushed, CI green, and smoke-tested on the live site.

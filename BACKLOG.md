# Backlog

Forward-looking work, ordered by expected value and risk. This is an operating
list, not a changelog. Keep it short enough that the next useful change is obvious.

## P1 — Today-Sized Foundation Work

These are safe, behaviour-preserving improvements that reduce future risk without
changing the deployed experience.

| Item | Why it matters | Effort |
| --- | --- | --- |
| **Harden production scripts** | `npm run produce` works, but the capture path should fail earlier and recover better: pick free Chrome debug ports or retry on collision, make `serve-static` report `EADDRINUSE` clearly, preflight Chrome/ffmpeg/rsvg-convert, fail on zero recorded chunks, and add a portable caption codec option instead of hard-coding macOS VideoToolbox HEVC. | M |
| **Tighten small runtime edges** | `Graphics::resize` ignores zero-sized surfaces, but `AppState::resize` still updates postprocess/camera; make resize return whether reconfiguration happened. Add small `with_app` / `with_app_mut` helpers for repeated `APP_STATE.with(...)` exports. Factor shared additive render descriptors for particles and the halo. | S-M |
| **Make deploy metadata reproducible** | `site.webmanifest` and `manifest.json` are intentionally identical, and icons/OG images are committed outputs. Either generate/check these pairs in CI or document the compatibility copy and add a `check:icons` guard so generated assets cannot drift. | S-M |

**Recommended today:** start with production-script hardening. It is high value,
low product risk, and directly protects the video workflow that already exists.
If there is time left, do the resize/`APP_STATE` cleanup as a second small commit.

## P2 — Maintainability Once P1 Is Clear

| Item | Why it matters | Effort |
| --- | --- | --- |
| **Move frontend logic out of `index.html`** | The inline module is now the biggest unchecked frontend surface. Move it to `static/app.js` so `npm run check:js` covers it directly, leaving `index.html` as markup/bootstrap. Preserve PWA/service-worker behaviour. | M |
| **Extract shared article styling** | `audio.html`, `physics.html`, and `engineering.html` duplicate large reader-page CSS blocks. Move shared styling to a static CSS file while keeping page-specific art/diagrams local. | S-M |
| **Split large construction paths** | `Simulation::new` and `audio::Graph::build` are the Rust maintenance hotspots. Split them into private helper builders without changing subsystem boundaries. | M |
| **Make local setup fully reproducible** | CI uses `npm ci` and pins `cargo-audit`; local `npm run setup` still uses `npm install`. Align setup/docs with CI expectations. | S |
| **Decide legacy redirect ownership** | `static/_worker.js` redirects `galacto.tre.systems` to `galacto.org`. Keep it with a short comment explaining why, or remove it if the old domain no longer matters. | S |

## P3 — Product Polish

These improve the experience, but they are not prerequisites for keeping the app
healthy.

| Item | Why it matters | Effort |
| --- | --- | --- |
| **More flyby presets** | The M51-style scenario proves the tidal-driver path works. A few more prograde/retrograde flybys would broaden the visual catalogue without changing the solver. | S-M |
| **Auto-replay demo mode** | Periodically re-seed for unattended displays so the disk does not heat and fade indefinitely. | S |
| **Richer audio core signals** | `reduce_core` has room for one more aggregate lane. Velocity dispersion, net angular momentum, or a coarse radial signal could give the sound another genuinely emergent driver. Keep it aggregate-only, throttled, and one-way. | S |
| **Subtle audio-reactive visuals** | Let note onsets or pad energy drive a small bloom/exposure pulse so the audio and picture reinforce each other. Keep it tasteful and subordinate to the simulation. | S-M |

## P4 — Larger Research/Production Tracks

Do these only when the goal explicitly needs them. They are useful directions, not
near-term cleanup.

| Item | Why it matters | Effort |
| --- | --- | --- |
| **Headless simulation/reference harness** | Add an `examples/headless.rs` or equivalent native path for profiling and solver validation. This is the prerequisite for trusting approximate gravity or deeper physics changes. | M-L |
| **Headless video export** | A native `wgpu` renderer could run the arrangement timeline into an offscreen texture and write a PNG/TIFF sequence for ffmpeg, avoiding real-time browser capture. | L |
| **Stems + MIDI/automation export** | For a deliberately produced music release, export note events, automation curves, and per-layer stems for a DAW. The current one-command WAV/MP4 path is enough for normal production. | M |
| **Gas-model realism** | A density/mean-velocity pass could support star formation, merger gas, or truer sticky gas. Start with one extra tiled `O(N²)` aggregate pass before considering spatial hashing. | L |
| **Approximate gravity for 100k+ bodies** | A GPU LBVH/Barnes-Hut tree could move past the all-pairs `O(N²)` ceiling, but it trades away exact every-pair gravity and likely only wins above roughly 100k bodies. Do not start before the headless/reference harness exists. | XL |

## Not Active

These are intentionally out of the active backlog unless the product goal changes.

| Candidate | Decision |
| --- | --- |
| **Full SPH hydrodynamics** | Delete from active planning. It is a separate simulation project, not an incremental improvement to the current calm galaxy sandbox. |
| **Live particle dark-matter halo** | Delete from active planning for now. It would consume the body budget and complicate the central story; the current analytic halo is clearer for users and docs. |
| **Tree gravity before a reference harness** | Keep only as a research track. Starting it now would add a large, divergent GPU system before there is a trustworthy validation path. |

## Definition of Done

- The verification gate passes (see [AGENTS § Verification](AGENTS.md#verification)); the pre-commit hook enforces it.
- Docs describing affected behaviour are updated to match.
- For user-visible changes: pushed, CI green, and smoke-tested on the live site.

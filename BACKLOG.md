# Backlog

Forward-looking work, ordered by expected value and risk. This is an operating
list, not a changelog. Keep it short enough that the next useful change is obvious.

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
| **Tree gravity before a reference harness** | Keep only as a research track. Starting it now would add a large, divergent GPU system before there is a trustworthy validation path. |

## Definition of Done

- The verification gate passes (see [AGENTS § Verification](AGENTS.md#verification)); the pre-commit hook enforces it.
- Docs describing affected behaviour are updated to match.
- For user-visible changes: pushed, CI green, and smoke-tested on the live site.

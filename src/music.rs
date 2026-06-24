//! Generative, visuals-driven music engine for the galaxy soundscape.
//!
//! Pure CPU logic with no web/audio dependencies, so it unit-tests natively
//! (like `scenarios.rs`). It turns a per-frame [`GalaxyState`] — the camera and
//! the live sim knobs, plus the galaxy's own core dynamics (central mass and the
//! radial flux in and out of the centre) read back from the GPU — into two things
//! `audio.rs` renders with Web Audio oscillators:
//!
//! * a [`DroneTarget`]: the slow sustained pad (its voice pitches, brightness,
//!   level, and detune beating), and
//! * a stream of [`NoteEvent`]s: sparse, scale-quantised "starlight" notes whose
//!   density, register, and velocity follow how much is happening on screen.

use crate::scenarios::Scenario;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

/// Number of sustained drone-pad voices. `audio.rs` keeps exactly this many
/// oscillators running and retunes them each frame from [`DroneTarget::freqs`].
pub const DRONE_VOICES: usize = 3;

/// The pad sits this many semitones below the scenario's root — two octaves down,
/// a deep, warm foundation well under the melodic notes (which stay at the root)
/// and clear of the nagging low-midrange where a steady tone turns into a drone.
const PAD_OCTAVE_DOWN: f32 = 24.0;

/// A soft oscillator shape. The soundscape stays gentle — no saws — so the pad
/// and the bells never get harsh against the slow visuals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waveform {
    Sine,
    Triangle,
}

/// One scheduled note. `audio.rs` builds a fresh oscillator + envelope + panner
/// per event and discards them when the envelope ends.
#[derive(Clone, Copy, Debug)]
pub struct NoteEvent {
    pub freq: f32,
    /// Normalised loudness, 0..1 (mapped to the gain envelope's peak).
    pub velocity: f32,
    /// Envelope length in seconds (pad-like; long and soft).
    pub duration: f32,
    pub waveform: Waveform,
    /// Stereo position, -1 (left) .. 1 (right).
    pub pan: f32,
}

/// The sustained pad's target state for a frame. `audio.rs` ramps the running
/// drone oscillators and filter toward these values, so scenario/zoom changes
/// glide rather than click.
#[derive(Clone, Copy, Debug)]
pub struct DroneTarget {
    /// Target frequency (Hz) for each of the [`DRONE_VOICES`] oscillators.
    pub freqs: [f32; DRONE_VOICES],
    /// Low-pass cutoff (Hz) for the pad — its brightness.
    pub cutoff_hz: f32,
    /// Overall pad level, 0..1.
    pub gain: f32,
    /// Detune spread (cents) between voices — the slow beating that gives the
    /// pad its shimmer.
    pub detune_cents: f32,
}

/// A per-frame snapshot of the visuals, all normalised to 0..1 (except the
/// categorical `scenario` and the `paused` gate). Built by `AppState` from the
/// camera, the live simulation knobs, and the GPU core-statistics readback, fed to
/// the engine each frame.
#[derive(Clone, Copy, Debug)]
pub struct GalaxyState {
    pub scenario: Scenario,
    /// Camera zoom: 0 = pulled right back, 1 = deep in the core.
    pub zoom: f32,
    /// Recent camera rotation speed: 0 = still, 1 = whipping around.
    pub motion: f32,
    /// Simulation speed multiplier, normalised across its slider range.
    pub speed: f32,
    /// How many fixed steps ran this frame, normalised — how fast time flows.
    pub intensity: f32,
    /// Gravity slider, normalised. Higher = a tighter, brighter system.
    pub gravity: f32,
    /// Dark-matter halo strength, normalised. Higher = a fuller, deeper pad.
    pub halo: f32,
    /// Star glow halo extent, normalised — opens the pad/bell brightness and echo.
    pub glow: f32,
    /// On-screen star size, normalised — makes the sound a little fuller/brighter.
    pub star_size: f32,
    /// Central-mass concentration (0..1, adaptive): how much mass sits at the
    /// centre right now relative to its recent norm. Swells the pad's body and
    /// lifts note density.
    pub core_mass: f32,
    /// Signed radial flux of core matter: -1 = collapsing inward, +1 = dispersing
    /// outward — the galaxy "breathing" in and out of the centre. Drives the pad's
    /// pitch and brightness so a collapse rises into tension and a dispersal settles.
    pub core_flux: f32,
    /// Core churn (0..1): how fast central matter is moving radially. The main
    /// driver of note density and shimmer.
    pub core_activity: f32,
    /// Gas fraction, 0..1 (the gas slider): more cold gas brightens and airs out
    /// the pad, echoing the blue arms it draws on screen.
    pub gas: f32,
    /// Bulge mass fraction, normalised 0..1 (the bulge slider): more central mass
    /// gives the pad more body.
    pub bulge: f32,
    /// Body count, normalised 0..1 (the bodies slider): more stars, busier starlight.
    pub richness: f32,
    /// Disk stability — Toomre Q, normalised 0..1 (the Q slider): a less stable,
    /// clumpier disk shimmers and detunes more; a smooth one is calmer.
    pub stability: f32,
    /// Halo scale radius, normalised 0..1 (the halo-size slider): a larger, more
    /// diffuse halo opens up the reverb space.
    pub halo_size: f32,
    pub paused: bool,
}

/// A4 (MIDI 69) = 432 Hz — the slightly-lower reference favoured across ambient
/// and meditation music; it sits a touch warmer than concert pitch (440 Hz).
/// Monotonic, with +12 semitones doubling the frequency.
pub fn midi_to_hz(midi: f32) -> f32 {
    432.0 * 2.0_f32.powf((midi - 69.0) / 12.0)
}

// Scale degrees as semitone offsets within one octave. The engine wraps an
// integer walk over these, adding 12 per octave, so they need only one octave.
const PENTATONIC_MAJOR: &[i32] = &[0, 2, 4, 7, 9];
const LYDIAN: &[i32] = &[0, 2, 4, 6, 7, 9, 11];
const DORIAN: &[i32] = &[0, 2, 3, 5, 7, 9, 10];
const PHRYGIAN: &[i32] = &[0, 1, 3, 5, 7, 8, 10];
const AEOLIAN: &[i32] = &[0, 2, 3, 5, 7, 8, 10];

/// The musical "character" a scenario maps to: its scale and tonal centre, the
/// pad's chord (intervals above the root), and how busy / bright it feels.
/// Calm and consonant for the lone disks; darker, denser, and more dissonant
/// for the collisions.
struct Character {
    scale: &'static [i32],
    root_midi: f32,
    /// Semitone intervals above the root for the [`DRONE_VOICES`] pad voices.
    drone: [f32; DRONE_VOICES],
    /// Base note-trigger density multiplier — mergers are busier.
    activity: f32,
    /// Base spectral brightness, 0..1 — mergers are darker.
    brightness: f32,
}

fn character(scenario: Scenario) -> Character {
    match scenario {
        // The lone disk and the M51 flyby: serene, bright, consonant.
        Scenario::Spiral => Character {
            scale: PENTATONIC_MAJOR,
            root_midi: 50.0,
            drone: [0.0, 7.0, 12.0],
            activity: 0.7,
            brightness: 0.72,
        },
        Scenario::GrandDesign => Character {
            scale: LYDIAN,
            root_midi: 50.0,
            drone: [0.0, 7.0, 16.0],
            activity: 0.8,
            brightness: 0.82,
        },
        // The collisions: darker modes, busier, more tension in the pad.
        Scenario::Merger | Scenario::Group => Character {
            scale: DORIAN,
            root_midi: 48.0,
            drone: [0.0, 7.0, 15.0],
            activity: 1.0,
            brightness: 0.5,
        },
        Scenario::Retrograde => Character {
            scale: DORIAN,
            root_midi: 47.0,
            drone: [0.0, 7.0, 14.0],
            activity: 1.0,
            brightness: 0.45,
        },
        Scenario::MinorMerger => Character {
            scale: AEOLIAN,
            root_midi: 46.0,
            drone: [0.0, 7.0, 15.0],
            activity: 0.9,
            brightness: 0.42,
        },
        // The head-on smash: tense Phrygian, busiest, with a flat-second pad.
        Scenario::HeadOn => Character {
            scale: PHRYGIAN,
            root_midi: 45.0,
            drone: [0.0, 6.0, 13.0],
            activity: 1.2,
            brightness: 0.34,
        },
    }
}

/// Generative engine: a wandering, scale-quantised note source plus the drone
/// mapping. Holds a seeded RNG (no OS entropy, like `scenarios.rs`) and a slow
/// random walk over scale degrees, so a given seed + input stream is reproducible.
pub struct MusicEngine {
    rng: StdRng,
    step: u64,
    /// Position of the melodic random walk, as an index into the scale extended
    /// across octaves (negative = below the root).
    degree_walk: i32,
}

impl MusicEngine {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: StdRng::seed_from_u64(seed),
            step: 0,
            degree_walk: 0,
        }
    }

    /// The sustained pad's target for this frame: a deep, low chord (two octaves
    /// below the scenario root) that breathes — gravity/halo lean it and radial
    /// flux lifts it as the core collapses inward; brightness from zoom, glow, gas,
    /// star size, and core churn; level hushed when paused and swelling with the
    /// mass gathered at the centre (and the bulge's share of it); a detune spread
    /// that widens with core churn, camera motion, gas, and a less stable (low-Q)
    /// disk.
    pub fn drone(&self, state: &GalaxyState) -> DroneTarget {
        let c = character(state.scenario);
        let inflow = (-state.core_flux).max(0.0); // collapse strength, 0..1
                                                  // Pitch: gravity gives a clear lean, the halo adds weight, and radial flux
                                                  // makes the pad breathe — collapsing inward (flux < 0) lifts it into
                                                  // tension, matter streaming back out (flux > 0) lets it settle.
        let bend = (state.gravity - 0.5) * 4.5 + (state.halo - 0.5) * 1.2 - 3.4 * state.core_flux;
        let mut freqs = [0.0_f32; DRONE_VOICES];
        for (f, interval) in freqs.iter_mut().zip(c.drone.iter()) {
            *f = midi_to_hz(c.root_midi - PAD_OCTAVE_DOWN + interval + bend);
        }
        // Brightness in octaves above a low base: the scenario's tilt, zoom, glow,
        // star size, gas, core churn, disk instability, and an extra lift while the
        // core collapses inward. These slider paths are intentionally broad enough
        // to be audible on phone speakers.
        let octaves = c.brightness * 1.25
            + state.zoom * 1.45
            + state.glow * 1.05
            + state.star_size * 0.35
            + state.gas * 0.85
            + state.core_activity * 0.95
            + (1.0 - state.stability) * 0.25
            + inflow * 0.65;
        let cutoff_hz = (200.0 * 2.0_f32.powf(octaves)).clamp(150.0, 8000.0);
        // Its body swells with central mass, halo fullness, bulge, and star size.
        let gain = if state.paused {
            0.12
        } else {
            (0.18
                + 0.17 * state.core_mass
                + 0.10 * state.halo
                + 0.14 * state.bulge
                + 0.05 * state.star_size)
                .clamp(0.0, 0.70)
        };
        let detune_cents = 3.0
            + 11.0 * state.core_activity
            + 8.0 * state.motion
            + 13.0 * (1.0 - state.stability)
            + 4.0 * state.gas
            + 3.0 * state.glow;
        DroneTarget {
            freqs,
            cutoff_hz,
            gain,
            detune_cents,
        }
    }

    /// Seconds between note-grid steps: a faster simulation ticks a quicker grid
    /// (about 0.95 s when slow, down to ~0.25 s when fast).
    pub fn step_seconds(&self, state: &GalaxyState) -> f64 {
        (0.95 - 0.70 * state.speed.clamp(0.0, 1.0)).max(0.22) as f64
    }

    /// Generate the notes for one grid step, pushing any into `out`. Density and
    /// loudness rise with the core's churn and how much mass has gathered there
    /// (with the sim speed and camera motion on top) and the scenario's base
    /// liveliness; register rises as you zoom in. Returns nothing while paused —
    /// only the drone carries the paused sim.
    pub fn generate_step(&mut self, state: &GalaxyState, out: &mut Vec<NoteEvent>) {
        self.step = self.step.wrapping_add(1);
        if state.paused {
            return;
        }
        let c = character(state.scenario);
        // Note density follows the core, then leans into visible/user controls:
        // richness, speed, motion, gas, glow, disk instability, and halo strength.
        let energy = (0.08
            + 0.50 * state.core_activity
            + 0.20 * state.core_mass
            + 0.20 * state.richness
            + 0.16 * state.intensity
            + 0.10 * state.motion
            + 0.10 * state.gas
            + 0.08 * state.glow
            + 0.08 * (1.0 - state.stability)
            + 0.06 * state.halo)
            * c.activity;
        if self.rng.random_range(0.0_f32..1.0) >= energy.clamp(0.0, 0.95) {
            return;
        }

        // Wander the scale degree, softly bounded so it can't run away.
        let len = c.scale.len() as i32;
        self.degree_walk += self.rng.random_range(-2..3);
        self.degree_walk = self.degree_walk.clamp(-3 * len, 3 * len);
        let within = self.degree_walk.rem_euclid(len) as usize;
        let octave = self.degree_walk.div_euclid(len);

        // Register: pulled-back view sits low, deep zoom lifts it up, and brighter
        // stars/glow nudge the bells higher.
        let register =
            (24.0 * state.zoom - 8.0 + 4.0 * state.glow + 3.0 * state.star_size).round() as i32;
        let midi = (c.root_midi + (c.scale[within] + 12 * octave + register) as f32)
            .clamp(c.root_midi - 18.0, c.root_midi + 42.0);

        let velocity = (0.12
            + 0.38 * state.core_activity
            + 0.16 * state.core_mass
            + 0.09 * state.glow
            + 0.07 * state.gas
            + 0.05 * state.halo
            + 0.12 * self.rng.random_range(0.0_f32..1.0))
        .clamp(0.05, 0.85);
        // Slower sims breathe with longer notes.
        let duration = 1.6 + 2.6 * (1.0 - state.speed) + self.rng.random_range(0.0_f32..1.2);
        let pan =
            (self.rng.random_range(-1.0_f32..1.0) * (0.35 + 0.5 * state.motion)).clamp(-0.9, 0.9);
        let waveform = if self.step.is_multiple_of(4) {
            Waveform::Triangle
        } else {
            Waveform::Sine
        };

        out.push(NoteEvent {
            freq: midi_to_hz(midi),
            velocity,
            duration,
            waveform,
            pan,
        });

        // An occasional high, quiet sparkle — more likely with brighter, glowier
        // stars and gas-rich disks — panned opposite the main note for width.
        let sparkle =
            (0.08 + 0.34 * state.glow + 0.22 * state.gas + 0.10 * state.star_size).clamp(0.0, 0.85);
        if self.rng.random_range(0.0_f32..1.0) < sparkle {
            out.push(NoteEvent {
                freq: midi_to_hz((midi + 12.0).min(c.root_midi + 48.0)),
                velocity: velocity * 0.5,
                duration: duration * 0.6,
                waveform: Waveform::Sine,
                pan: -pan,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(intensity: f32, motion: f32, paused: bool) -> GalaxyState {
        GalaxyState {
            scenario: Scenario::Spiral,
            zoom: 0.5,
            motion,
            speed: 0.5,
            intensity,
            gravity: 0.5,
            halo: 0.5,
            glow: 0.5,
            star_size: 0.5,
            core_mass: 0.5,
            core_flux: 0.0,
            // Tie the test "activity" knob to the core, the primary density driver,
            // so the existing density/determinism tests exercise it.
            core_activity: intensity,
            gas: 0.3,
            bulge: 0.2,
            richness: 0.3,
            stability: 0.5,
            halo_size: 0.2,
            paused,
        }
    }

    #[test]
    fn midi_to_hz_anchors_and_octaves() {
        assert!((midi_to_hz(69.0) - 432.0).abs() < 1e-3);
        assert!((midi_to_hz(81.0) - 864.0).abs() < 1e-2);
        assert!((midi_to_hz(57.0) - 216.0).abs() < 1e-2);
    }

    #[test]
    fn paused_emits_no_notes() {
        let mut eng = MusicEngine::new(1);
        let mut out = Vec::new();
        for _ in 0..200 {
            eng.generate_step(&state(1.0, 1.0, true), &mut out);
        }
        assert!(
            out.is_empty(),
            "a paused sim should be silent but for the drone"
        );
    }

    #[test]
    fn busier_visuals_trigger_more_notes() {
        let count = |intensity, motion| {
            let mut eng = MusicEngine::new(7);
            let mut out = Vec::new();
            for _ in 0..500 {
                eng.generate_step(&state(intensity, motion, false), &mut out);
            }
            out.len()
        };
        assert!(count(1.0, 1.0) > count(0.0, 0.0));
    }

    #[test]
    fn notes_are_well_formed() {
        let mut eng = MusicEngine::new(3);
        let mut out = Vec::new();
        for s in [Scenario::Spiral, Scenario::HeadOn, Scenario::Merger] {
            for _ in 0..400 {
                let mut gs = state(0.8, 0.6, false);
                gs.scenario = s;
                eng.generate_step(&gs, &mut out);
            }
        }
        assert!(!out.is_empty());
        for ev in &out {
            assert!(
                ev.freq.is_finite() && ev.freq > 0.0,
                "freq {} invalid",
                ev.freq
            );
            assert!(
                (0.0..=1.0).contains(&ev.velocity),
                "velocity {} out of range",
                ev.velocity
            );
            assert!(ev.duration > 0.0, "duration {} invalid", ev.duration);
            assert!(
                (-1.0..=1.0).contains(&ev.pan),
                "pan {} out of range",
                ev.pan
            );
        }
    }

    #[test]
    fn drone_is_well_formed_for_every_scenario() {
        let eng = MusicEngine::new(0);
        for s in [
            Scenario::Spiral,
            Scenario::Merger,
            Scenario::HeadOn,
            Scenario::Retrograde,
            Scenario::MinorMerger,
            Scenario::Group,
            Scenario::GrandDesign,
        ] {
            let mut gs = state(0.5, 0.5, false);
            gs.scenario = s;
            let d = eng.drone(&gs);
            for f in d.freqs {
                assert!(f.is_finite() && f > 0.0, "drone freq {f} invalid for {s:?}");
            }
            assert!(
                (150.0..=8000.0).contains(&d.cutoff_hz),
                "cutoff out of range"
            );
            assert!(d.gain > 0.0 && d.gain <= 1.0);
        }
    }

    #[test]
    fn collapse_lifts_the_pad_above_dispersal() {
        let eng = MusicEngine::new(0);
        let mut inflow = state(0.5, 0.0, false);
        inflow.core_flux = -1.0; // matter falling into the centre
        let mut outflow = state(0.5, 0.0, false);
        outflow.core_flux = 1.0; // matter streaming back out
        assert!(
            eng.drone(&inflow).freqs[0] > eng.drone(&outflow).freqs[0],
            "a collapse should lift the pad into tension above a dispersal"
        );
    }

    #[test]
    fn slider_extremes_move_drone_targets() {
        let eng = MusicEngine::new(0);
        let mut low = state(0.2, 0.0, false);
        low.zoom = 0.3;
        low.gravity = 0.0;
        low.halo = 0.0;
        low.glow = 0.0;
        low.star_size = 0.0;
        low.gas = 0.0;
        low.bulge = 0.0;
        low.stability = 1.0;

        let mut high = low;
        high.gravity = 1.0;
        high.halo = 1.0;
        high.glow = 1.0;
        high.star_size = 1.0;
        high.gas = 1.0;
        high.bulge = 1.0;
        high.stability = 0.0;

        let quiet = eng.drone(&low);
        let bright = eng.drone(&high);
        assert!(
            bright.freqs[0] > quiet.freqs[0] * 1.25,
            "gravity/halo should audibly lift the pad pitch"
        );
        assert!(
            bright.cutoff_hz > quiet.cutoff_hz * 2.5,
            "glow/gas/star-size should clearly open the pad filter"
        );
        assert!(
            bright.gain > quiet.gain + 0.25,
            "halo/bulge/star-size should clearly swell the pad body"
        );
        assert!(
            bright.detune_cents > quiet.detune_cents + 15.0,
            "low-Q/gas/glow should clearly widen pad shimmer"
        );
    }

    #[test]
    fn generation_is_deterministic_for_a_seed() {
        let run = || {
            let mut eng = MusicEngine::new(42);
            let mut out = Vec::new();
            for _ in 0..300 {
                eng.generate_step(&state(0.7, 0.3, false), &mut out);
            }
            out.iter().map(|e| e.freq).collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }
}

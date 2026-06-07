//! Generative, visuals-driven music engine for the galaxy soundscape.
//!
//! Pure CPU logic with no web/audio dependencies, so it unit-tests natively
//! (like `scenarios.rs`). It turns a per-frame [`GalaxyState`] — derived entirely
//! from CPU-side visuals (the camera, the active scenario, the live sim knobs),
//! since the GPU body state is never read back — into two things `audio.rs`
//! renders with Web Audio oscillators:
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
/// camera and the live simulation knobs and fed to the engine each frame.
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
    /// On-screen star size, normalised — feeds the pad/bell brightness.
    pub glow: f32,
    pub paused: bool,
}

/// A4 (MIDI 69) = 440 Hz. Monotonic, with +12 semitones doubling the frequency.
pub fn midi_to_hz(midi: f32) -> f32 {
    440.0 * 2.0_f32.powf((midi - 69.0) / 12.0)
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

    /// The sustained pad's target for this frame: pitches from the scenario's
    /// chord (nudged up as gravity tightens the system), brightness from zoom +
    /// glow, level hushed when paused and fuller with a stronger halo, and a
    /// detune spread that widens with camera motion.
    pub fn drone(&self, state: &GalaxyState) -> DroneTarget {
        let c = character(state.scenario);
        // Gravity gently bends the whole pad: about -1 .. +1.5 semitones.
        let bend = (state.gravity - 0.4) * 2.5;
        let mut freqs = [0.0_f32; DRONE_VOICES];
        for (f, interval) in freqs.iter_mut().zip(c.drone.iter()) {
            *f = midi_to_hz(c.root_midi + interval + bend);
        }
        // Brightness in octaves above a low base: the scenario's own tilt plus
        // how far in you've zoomed and how glowy the stars are.
        let octaves = c.brightness * 1.6 + state.zoom * 2.0 + state.glow * 0.8;
        let cutoff_hz = (200.0 * 2.0_f32.powf(octaves)).clamp(150.0, 8000.0);
        let gain = if state.paused {
            0.16
        } else {
            (0.42 + 0.22 * state.halo).clamp(0.0, 0.7)
        };
        let detune_cents = 4.0 + 12.0 * state.motion + 6.0 * (1.0 - c.brightness);
        DroneTarget {
            freqs,
            cutoff_hz,
            gain,
            detune_cents,
        }
    }

    /// Seconds between note-grid steps: a faster simulation ticks a quicker grid
    /// (about 0.85 s when slow, down to ~0.3 s when fast).
    pub fn step_seconds(&self, state: &GalaxyState) -> f64 {
        (0.85 - 0.55 * state.speed.clamp(0.0, 1.0)).max(0.28) as f64
    }

    /// Generate the notes for one grid step, pushing any into `out`. Density and
    /// loudness rise with on-screen activity (substeps + camera motion + speed)
    /// and the scenario's base liveliness; register rises as you zoom in. Returns
    /// nothing while paused — only the drone carries the paused sim.
    pub fn generate_step(&mut self, state: &GalaxyState, out: &mut Vec<NoteEvent>) {
        self.step = self.step.wrapping_add(1);
        if state.paused {
            return;
        }
        let c = character(state.scenario);
        let energy =
            (0.16 + 0.5 * state.intensity + 0.3 * state.motion + 0.12 * state.speed) * c.activity;
        if self.rng.random_range(0.0_f32..1.0) >= energy.clamp(0.0, 0.95) {
            return;
        }

        // Wander the scale degree, softly bounded so it can't run away.
        let len = c.scale.len() as i32;
        self.degree_walk += self.rng.random_range(-2..3);
        self.degree_walk = self.degree_walk.clamp(-3 * len, 3 * len);
        let within = self.degree_walk.rem_euclid(len) as usize;
        let octave = self.degree_walk.div_euclid(len);

        // Register: pulled-back view sits low, deep zoom lifts it up to ~+18 st.
        let register = (24.0 * state.zoom - 6.0).round() as i32;
        let midi = (c.root_midi + (c.scale[within] + 12 * octave + register) as f32)
            .clamp(c.root_midi - 18.0, c.root_midi + 42.0);

        let velocity = (0.16
            + 0.45 * state.intensity
            + 0.22 * state.motion
            + 0.12 * self.rng.random_range(0.0_f32..1.0))
        .clamp(0.05, 0.8);
        // Slower sims breathe with longer notes.
        let duration = 1.8 + 2.4 * (1.0 - state.speed) + self.rng.random_range(0.0_f32..1.2);
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
        // stars — panned opposite the main note for width.
        if self.rng.random_range(0.0_f32..1.0) < 0.10 + 0.2 * state.glow {
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
            paused,
        }
    }

    #[test]
    fn midi_to_hz_anchors_and_octaves() {
        assert!((midi_to_hz(69.0) - 440.0).abs() < 1e-3);
        assert!((midi_to_hz(81.0) - 880.0).abs() < 1e-2);
        assert!((midi_to_hz(57.0) - 220.0).abs() < 1e-2);
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

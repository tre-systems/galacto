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
    /// Deep sub-bass foundation level (0..1): a sine an octave below the lowest pad
    /// voice. Swells with the mass gathered at the centre, the halo, and the bulge —
    /// the weight that makes the space feel huge.
    pub sub_gain: f32,
}

/// The ambient *texture* targets for a frame: the layers and effects that sit
/// around the pad and bells — the deep space itself. Like [`DroneTarget`], these
/// are glided toward by `audio.rs`. Split from the pad so the whole generative
/// mapping (what the space should sound like for a given galaxy) lives here and
/// stays native-testable; `audio.rs` only renders it.
#[derive(Clone, Copy, Debug)]
pub struct TextureTarget {
    /// High twinkling "starfield" layer level (0..1): autonomous shimmering tones
    /// tuned to the pad's upper harmonics. Density follows the body count, glow, and
    /// zoom — more, closer, brighter stars.
    pub star_gain: f32,
    /// Starfield brightness — its low-pass cutoff (Hz). Opens with glow, gas, zoom.
    pub star_cutoff_hz: f32,
    /// Octave-up shimmer send into the reverb (0..1): the signature cosmic sheen,
    /// opening with glow, gas, a close view, and a collapsing core.
    pub shimmer_gain: f32,
    /// Stereo bias for the pad + starfield (-1..1), following the camera orbit.
    pub field_pan: f32,
    /// Airy noise-bed level (0..1): soft background "air" lifted by gas and churn.
    pub noise_gain: f32,
    /// Reverb wet level — the size/presence of the surrounding space.
    pub reverb_wet: f32,
    /// Feedback-delay wet level — the long echoes trailing into the void.
    pub delay_wet: f32,
    /// Feedback-delay feedback amount — how many times the echoes repeat.
    pub delay_feedback: f32,
    /// Resonant pad-filter Q — the pad's vocal "shimmer", breathing on the LFO and
    /// lifting as the core collapses.
    pub pad_resonance: f32,
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
    /// Camera orbit mapped to a smooth stereo bias (-1 left .. 1 right): as the
    /// view circles the galaxy the whole soundscape swings across the field, so the
    /// pan is audibly tied to where the camera is looking.
    pub camera_pan: f32,
    /// Core coherence (0..1): is the central matter's radial motion *organized* — a
    /// unified collapse or expansion (→1) — or *random thermal churn* (→0)? A
    /// coherent event focuses the pad into a clear tone; disordered churn widens,
    /// beats, and detunes it. Derived from the same readback as the flux/activity.
    pub coherence: f32,
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
        let octaves = c.brightness * 1.1
            + state.zoom * 1.2
            + state.glow * 0.9
            + state.star_size * 0.3
            + state.gas * 0.7
            + state.core_activity * 0.85
            + (1.0 - state.stability) * 0.25
            + inflow * 0.6;
        // Ceiling held to 6 kHz so the whole mix keeps a gentle downward (brown-ish)
        // tilt and never goes bright-flat on top.
        let cutoff_hz = (200.0 * 2.0_f32.powf(octaves)).clamp(150.0, 6000.0);
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
        // Disordered core churn detunes far more than organized motion: an
        // incoherent, thermally hot core (coherence→0) beats and shimmers, while a
        // unified collapse or expansion (coherence→1) pulls the voices into focus.
        let churn_detune = state.core_activity * (6.0 + 12.0 * (1.0 - state.coherence));
        let detune_cents = 3.0
            + churn_detune
            + 8.0 * state.motion
            + 13.0 * (1.0 - state.stability)
            + 4.0 * state.gas
            + 3.0 * state.glow;
        // Sub-bass foundation: the weight that makes the space feel huge. Swells with
        // the mass gathered at the centre, the halo, the bulge, and star size; hushed
        // (but never gone) when paused, so the floor holds under the still galaxy.
        let sub_gain = if state.paused {
            0.05
        } else {
            (0.08
                + 0.11 * state.core_mass
                + 0.10 * state.halo
                + 0.10 * state.bulge
                + 0.04 * state.star_size)
                .clamp(0.0, 0.42)
        };
        DroneTarget {
            freqs,
            cutoff_hz,
            gain,
            detune_cents,
            sub_gain,
        }
    }

    /// The ambient texture targets for this frame: the starfield, the octave-up
    /// shimmer, the surrounding reverb/echo space, the airy noise bed, and the
    /// camera-driven stereo bias — everything around the pad that makes the galaxy
    /// sit in a vast space. `lfo_a`/`lfo_b` are slow free-running 0..1 oscillators
    /// (passed in so this stays pure and testable); pass 0.5 for a steady snapshot.
    pub fn texture(&self, state: &GalaxyState, lfo_a: f32, lfo_b: f32) -> TextureTarget {
        let inflow = (-state.core_flux).max(0.0); // collapse strength, 0..1
        let silent = state.paused;

        // Starfield: a high, twinkling layer that tracks how many stars there are
        // (body count) and how bright/close they read (glow, zoom, gas). Silent while
        // paused — the stars hold still.
        let star_gain = if silent {
            0.0
        } else {
            (0.01
                + 0.08 * state.richness
                + 0.05 * state.glow
                + 0.04 * state.zoom
                + 0.03 * state.gas
                + 0.02 * lfo_a)
                .clamp(0.0, 0.18)
        };
        // Warm low-pass kept well below the fatiguing presence band, so the starfield
        // stays a soft, rounded sparkle rather than a sharp, sustained tone.
        let star_cutoff_hz = (900.0
            * 2.0_f32.powf(0.8 * state.zoom + 0.5 * state.glow + 0.4 * state.gas))
        .clamp(600.0, 2400.0);

        // Octave-up shimmer into the reverb — the cosmic sheen. Reverb-diffused (never
        // a dry tone) and kept gentle so it adds air without sharpness. Opens with glow
        // and gas (the bright blue arms), a close view, and a collapsing core.
        let shimmer_gain = (0.03
            + 0.12 * state.glow
            + 0.08 * state.gas
            + 0.07 * state.zoom
            + 0.08 * inflow
            + 0.03 * lfo_b)
            .clamp(0.0, 0.24);

        // The whole pad + starfield image swings with the camera orbit.
        let field_pan = (state.camera_pan * 0.6).clamp(-0.85, 0.85);

        // Cavernous, washy reverb: wet by default so the space feels vast even when
        // the galaxy is still, deeper when pulled back, opening with core churn, halo
        // strength/size, and slowly swelling and ebbing on its own LFO.
        let reverb_wet = (0.52
            + 0.32 * (1.0 - state.zoom)
            + 0.22 * state.core_activity
            + 0.24 * lfo_b
            + 0.08 * state.motion
            + 0.55 * state.halo_size
            + 0.14 * state.halo)
            .clamp(0.0, 1.85);
        // Long, present echo with sustained, trailing repeats.
        let delay_wet =
            (0.18 + 0.28 * state.motion + 0.16 * state.speed + 0.10 * state.glow + 0.08 * lfo_a)
                .clamp(0.0, 0.78);
        let delay_feedback =
            (0.42 + 0.28 * state.motion + 0.10 * state.halo + 0.07 * state.speed).clamp(0.0, 0.86);

        // Brown-noise rumble bed: a warm, low background presence that breathes with
        // the core's churn and its own LFO, lifted by the gas fraction. A touch more
        // present than before, for the focus-friendly smoothed-brown character.
        let noise_gain = (0.035
            + 0.05 * state.core_activity
            + 0.018 * lfo_b
            + 0.09 * state.gas
            + 0.03 * state.glow
            + 0.015 * state.star_size)
            .clamp(0.0, 0.20);

        // Resonant pad filter: breathes on the LFO, lifts as the core collapses
        // inward, and widens with gas-rich / low-Q (unstable) disks.
        let pad_resonance =
            (0.7 + 1.8 * lfo_a + 1.35 * inflow + 1.0 * (1.0 - state.stability) + 0.45 * state.gas)
                .clamp(0.7, 3.2);

        TextureTarget {
            star_gain,
            star_cutoff_hz,
            shimmer_gain,
            field_pan,
            noise_gain,
            reverb_wet,
            delay_wet,
            delay_feedback,
            pad_resonance,
        }
    }

    /// Seconds between note-grid steps. Kept in the slow, meditative tempo range
    /// (~50 BPM / 1.3 s when calm, easing only to ~85 BPM / 0.7 s at full speed) so
    /// the soundscape never turns frantic — slow tempi near a resting heart rate are
    /// what the relaxation studies favour.
    pub fn step_seconds(&self, state: &GalaxyState) -> f64 {
        (1.3 - 0.6 * state.speed.clamp(0.0, 1.0)).max(0.7) as f64
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
        // Kept sparse on purpose — space and silence between sparse "starlight" notes
        // (over the continuous pad) read as calm; a wall of notes does not.
        let energy = (0.05
            + 0.34 * state.core_activity
            + 0.14 * state.core_mass
            + 0.14 * state.richness
            + 0.11 * state.intensity
            + 0.07 * state.motion
            + 0.07 * state.gas
            + 0.06 * state.glow
            + 0.06 * (1.0 - state.stability)
            + 0.04 * state.halo)
            * c.activity;
        if self.rng.random_range(0.0_f32..1.0) >= energy.clamp(0.0, 0.72) {
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
        // Long, sustained notes that fade in and out gently — soft, legato tones are
        // calming; short plucks are not. Slower sims breathe with even longer notes.
        let duration = 2.6 + 3.2 * (1.0 - state.speed) + self.rng.random_range(0.0_f32..1.6);
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
            camera_pan: 0.0,
            coherence: 0.5,
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
    fn texture_is_well_formed_for_every_scenario() {
        let eng = MusicEngine::new(0);
        for s in [
            Scenario::Spiral,
            Scenario::Merger,
            Scenario::HeadOn,
            Scenario::GrandDesign,
        ] {
            for &paused in &[false, true] {
                let mut gs = state(0.6, 0.4, paused);
                gs.scenario = s;
                for &(a, b) in &[(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)] {
                    let tx = eng.texture(&gs, a, b);
                    for v in [
                        tx.star_gain,
                        tx.shimmer_gain,
                        tx.noise_gain,
                        tx.reverb_wet,
                        tx.delay_wet,
                        tx.delay_feedback,
                        tx.pad_resonance,
                        tx.star_cutoff_hz,
                    ] {
                        assert!(v.is_finite() && v >= 0.0, "texture value {v} invalid");
                    }
                    assert!((-1.0..=1.0).contains(&tx.field_pan));
                    assert!((500.0..=3000.0).contains(&tx.star_cutoff_hz));
                }
            }
        }
    }

    #[test]
    fn texture_sliders_have_distinct_effects() {
        let eng = MusicEngine::new(0);
        let base = state(0.4, 0.2, false);
        let tx = |f: &dyn Fn(&mut GalaxyState)| {
            let mut gs = base;
            f(&mut gs);
            eng.texture(&gs, 0.5, 0.5)
        };
        let lo = eng.texture(&base, 0.5, 0.5);
        // Body count → starfield; glow → shimmer; halo size → reverb; gas → air.
        assert!(tx(&|g| g.richness = 1.0).star_gain > lo.star_gain + 0.04);
        assert!(tx(&|g| g.glow = 1.0).shimmer_gain > lo.shimmer_gain + 0.05);
        assert!(tx(&|g| g.halo_size = 1.0).reverb_wet > lo.reverb_wet + 0.2);
        assert!(tx(&|g| g.gas = 1.0).noise_gain > lo.noise_gain + 0.04);
    }

    #[test]
    fn camera_orbit_pans_the_field() {
        let eng = MusicEngine::new(0);
        let mut left = state(0.4, 0.2, false);
        left.camera_pan = -1.0;
        let mut right = state(0.4, 0.2, false);
        right.camera_pan = 1.0;
        assert!(eng.texture(&left, 0.5, 0.5).field_pan < -0.3);
        assert!(eng.texture(&right, 0.5, 0.5).field_pan > 0.3);
    }

    #[test]
    fn incoherent_churn_detunes_more_than_coherent() {
        let eng = MusicEngine::new(0);
        // A churning core (high activity): random/disordered motion (low coherence)
        // should beat and detune far more than an organized collapse (high coherence).
        let mut chaos = state(0.9, 0.0, false);
        chaos.coherence = 0.05;
        let mut order = state(0.9, 0.0, false);
        order.coherence = 0.95;
        assert!(
            eng.drone(&chaos).detune_cents > eng.drone(&order).detune_cents + 6.0,
            "disordered churn should widen the pad more than an organized event"
        );
    }

    #[test]
    fn sub_swells_with_central_mass_and_hushes_when_paused() {
        let eng = MusicEngine::new(0);
        let mut light = state(0.4, 0.0, false);
        light.core_mass = 0.0;
        light.halo = 0.0;
        light.bulge = 0.0;
        let mut heavy = light;
        heavy.core_mass = 1.0;
        heavy.bulge = 1.0;
        assert!(eng.drone(&heavy).sub_gain > eng.drone(&light).sub_gain + 0.1);
        // Paused keeps a floor but silences the starfield.
        let paused = state(1.0, 0.0, true);
        assert!(eng.drone(&paused).sub_gain > 0.0);
        assert_eq!(eng.texture(&paused, 0.5, 0.5).star_gain, 0.0);
    }

    #[test]
    fn step_seconds_stays_in_a_calm_tempo_range() {
        let eng = MusicEngine::new(0);
        // Across the whole speed range the note grid stays in the relaxing
        // ~50–85 BPM band (0.7–1.3 s) — never frantic, even at full sim speed.
        for speed in [0.0_f32, 0.5, 1.0] {
            let mut s = state(0.5, 0.0, false);
            s.speed = speed;
            let dt = eng.step_seconds(&s);
            assert!(
                (0.69..=1.31).contains(&dt),
                "step {dt} out of the calm range at speed {speed}"
            );
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

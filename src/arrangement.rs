//! Deterministic cinematic arrangement — composes a complete ambient piece's arc.
//!
//! Pure logic (no web/audio deps), native-testable like `music.rs`. One
//! `seed` + `duration` arc drives BOTH the visuals (camera + live physics, applied
//! in `lib.rs`) and the audio (a synthetic [`GalaxyState`] timeline rendered offline
//! by `audio::render_offline`), so the two stay locked when combined into a video.
//!
//! The shape is the classic ambient A→B→C form: a sparse, dark, distant intro; a
//! slow build that gathers and brightens (gravity rises so the galaxy visibly
//! contracts, the camera drifts in, layers enter); a serene awe peak about
//! two-thirds through; then a long resolution — gravity eases so the galaxy
//! disperses, the camera pulls back, and it settles to quiet space. Calm and awe,
//! never an abrupt crescendo.

use crate::music::GalaxyState;
use crate::scenarios::Scenario;
use std::f32::consts::TAU;

/// Smootherstep (Perlin): 0..1 → 0..1 with zero first/second derivatives at the
/// ends, for glide-free transitions.
fn smoother(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

/// A deterministic value in [0,1) from a seed and a salt — cheap, RNG-free variety.
fn hash01(seed: u32, salt: u32) -> f32 {
    let mut h = seed.wrapping_add(salt.wrapping_mul(0x9E37_79B9));
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h % 100_000) as f32 / 100_000.0
}

/// Camera pose for the visuals at a moment in the piece.
#[derive(Clone, Copy, Debug)]
pub struct CameraPose {
    pub scale: f32,
    pub rot_x: f32,
    pub rot_y: f32,
}

/// Normalised (0..1) live-physics targets for the visuals; `lib.rs` maps these to
/// the simulation's actual gravity/halo/glow/star-size when driving the running sim.
#[derive(Clone, Copy, Debug)]
pub struct Physics {
    pub gravity: f32,
    pub halo: f32,
    pub halo_size: f32,
    pub glow: f32,
    pub star_size: f32,
}

/// A deterministic arrangement over `duration` seconds for `scenario`, varied by `seed`.
#[derive(Clone, Copy, Debug)]
pub struct Arrangement {
    pub duration: f64,
    pub seed: u32,
    pub scenario: Scenario,
}

impl Arrangement {
    pub fn new(duration: f64, seed: u32, scenario: Scenario) -> Self {
        Self {
            duration: duration.max(1.0),
            seed,
            scenario,
        }
    }

    /// Normalised progress 0..1 at time `t` (seconds).
    fn progress(&self, t: f64) -> f32 {
        (t / self.duration).clamp(0.0, 1.0) as f32
    }

    /// The A→B→C arc energy at progress `u`: a smooth rise to a serene peak (around
    /// two-thirds through, nudged by the seed), then a long resolution to silence.
    fn arc(&self, u: f32) -> f32 {
        let peak = 0.6 + 0.12 * hash01(self.seed, 7);
        if u <= peak {
            smoother(u / peak)
        } else {
            smoother(1.0 - (u - peak) / (1.0 - peak))
        }
    }

    /// Orbit angle (radians): a slow continuous orbit completing ~1.1 turns over the
    /// whole piece (gentle, so the stereo image still averages roughly centred), with
    /// a seed phase.
    fn orbit(&self, t: f64) -> f32 {
        TAU * 1.1 * (t / self.duration) as f32 + TAU * hash01(self.seed, 19)
    }

    /// Camera pose at time `t`. The arc push-in carries the overall framing, but slow
    /// layered motion keeps the view evolving across the whole piece so it's never
    /// static: gentle zoom "breaths" on top of the arc, and a wandering tilt that
    /// carries the galaxy between face-on, oblique, and near-edge-on (where the disk
    /// reads as a dramatic thin blade). All cycles are a function of progress, so the
    /// pacing scales with the piece length and stays slow over a long render.
    pub fn camera(&self, t: f64) -> CameraPose {
        let ph = self.progress(t);
        let a = self.arc(ph);
        // ~3 gentle push/pull breaths over the piece, on top of the arc's push-in.
        let zoom_breath = 1.0 + 0.18 * (TAU * 3.3 * ph + TAU * hash01(self.seed, 23)).sin();
        // A slow wander through viewing angles (two decorrelated sweeps), amplitude
        // well within the ±1.5 rad tilt clamp; passes through face-on (≈0) and out
        // toward near-edge-on for variety.
        let tilt = 0.72 * (TAU * 2.2 * ph + TAU * hash01(self.seed, 13)).sin()
            + 0.45 * (TAU * 1.1 * ph + TAU * hash01(self.seed, 29)).sin();
        CameraPose {
            // Start already framed (0.55) rather than a distant speck, easing in to the
            // same immersive peak (~1.0), breathing in and out along the way.
            scale: 0.55 * 1.85_f32.powf(a) * zoom_breath,
            rot_x: tilt,
            rot_y: self.orbit(t),
        }
    }

    /// Normalised live-physics targets at time `t`.
    pub fn physics(&self, t: f64) -> Physics {
        let ph = self.progress(t);
        let a = self.arc(ph);
        // Slow undulations on top of the arc so the brightness and star texture keep
        // shifting through the piece (the galaxy's glow and sparkle gently breathe),
        // rather than rising once and holding.
        let glow_pulse = 0.12 * (TAU * 4.3 * ph + TAU * hash01(self.seed, 31)).sin();
        let star_pulse = 0.08 * (TAU * 3.1 * ph + TAU * hash01(self.seed, 37)).sin();
        Physics {
            // Gravity rises gently through the build so the galaxy gathers (~1.0×→1.8×
            // once mapped to the sim), and eases through the resolution so it disperses
            // — the visual swell the audio rides on, without a violent collapse.
            gravity: 0.2 + 0.22 * a,
            // A lean, steady halo so the pad's body comes from the arc (core mass),
            // not a constant floor — which keeps the intro sparse and the peak full.
            halo: 0.35,
            halo_size: 0.5,
            glow: (0.12 + 0.5 * a + glow_pulse).clamp(0.0, 1.0), // dim intro → bright, pulsing peak
            star_size: (0.4 + 0.25 * a + star_pulse).clamp(0.0, 1.0),
        }
    }

    /// The audio-side snapshot at time `t`, derived from the SAME arc as the camera
    /// and physics so the sound and picture line up. Returns the normalised
    /// [`GalaxyState`] the engine renders.
    pub fn galaxy_state(&self, t: f64) -> GalaxyState {
        let u = self.progress(t);
        let a = self.arc(u);
        // Arc slope → the galaxy "breathing": gathering (inflow) on the rise,
        // dispersing (outflow) on the fall; its magnitude reads as organised motion.
        let du = (0.5 / self.duration) as f32;
        let slope =
            (self.arc((u + du).min(1.0)) - self.arc((u - du).max(0.0))) / (2.0 * du.max(1e-4));
        let slope_n = (slope * 4.0).clamp(-1.0, 1.0);
        let p = self.physics(t);
        GalaxyState {
            scenario: self.scenario,
            // Wide arc so the journey is felt: distant/dark/sparse intro → close,
            // bright, full peak → back to quiet space. The loudness master then sets
            // the full peak near the target and lets the ends sit quieter beneath it.
            zoom: (0.1 + 0.7 * a).clamp(0.0, 1.0),
            motion: 0.12,
            speed: 0.2, // slow, meditative tempo
            intensity: (0.5 * a).clamp(0.0, 1.0),
            gravity: p.gravity,
            halo: p.halo,
            glow: p.glow,
            star_size: p.star_size,
            core_mass: (0.05 + 0.9 * a).clamp(0.0, 1.0),
            core_flux: (-slope_n).clamp(-1.0, 1.0), // rising arc = inflow (collapse)
            core_activity: (0.65 * a).clamp(0.0, 1.0),
            gas: 0.3,
            bulge: 0.12,
            richness: (0.2 + 0.45 * a).clamp(0.0, 1.0),
            stability: 0.7, // calm, smooth — low detune
            halo_size: p.halo_size,
            camera_pan: self.orbit(t).sin(),
            coherence: (0.45 + 0.9 * slope_n.abs()).clamp(0.0, 1.0),
            paused: false,
        }
    }

    /// Sample the audio timeline every `dt` seconds (plus a final sample at the end),
    /// for the offline render.
    pub fn timeline(&self, dt: f64) -> Vec<(f64, GalaxyState)> {
        let dt = dt.max(1.0 / 120.0);
        let mut out = Vec::with_capacity((self.duration / dt) as usize + 2);
        let mut t = 0.0;
        while t < self.duration {
            out.push((t, self.galaxy_state(t)));
            t += dt;
        }
        out.push((self.duration, self.galaxy_state(self.duration)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arr() -> Arrangement {
        Arrangement::new(300.0, 42, Scenario::GrandDesign)
    }

    #[test]
    fn timeline_is_well_formed() {
        let a = arr();
        let tl = a.timeline(1.0 / 30.0);
        assert!(tl.len() > 100);
        let mut last_t = -1.0;
        for (t, s) in &tl {
            assert!(*t >= last_t, "timeline times must be monotonic");
            last_t = *t;
            for v in [
                s.zoom,
                s.gravity,
                s.glow,
                s.star_size,
                s.core_mass,
                s.core_activity,
                s.halo,
                s.gas,
                s.bulge,
                s.richness,
                s.stability,
                s.halo_size,
            ] {
                assert!(
                    v.is_finite() && (0.0..=1.0).contains(&v),
                    "field {v} out of 0..1"
                );
            }
            assert!((-1.0..=1.0).contains(&s.core_flux));
            assert!((-1.0..=1.0).contains(&s.camera_pan));
            assert!(!s.paused);
        }
    }

    #[test]
    fn arc_builds_to_a_peak_then_resolves() {
        let a = arr();
        let intro = a.galaxy_state(a.duration * 0.05);
        let peak = a.galaxy_state(a.duration * 0.65);
        let outro = a.galaxy_state(a.duration * 0.97);
        // The middle is fuller (more mass gathered, busier, brighter) than the ends.
        assert!(peak.core_mass > intro.core_mass + 0.2);
        assert!(peak.core_mass > outro.core_mass + 0.2);
        assert!(peak.core_activity > intro.core_activity);
        assert!(peak.zoom > intro.zoom); // camera has drifted in
    }

    #[test]
    fn camera_zooms_in_toward_the_peak_and_orbits() {
        let a = arr();
        let intro = a.camera(a.duration * 0.05);
        let peak = a.camera(a.duration * 0.65);
        assert!(peak.scale > intro.scale, "should zoom in toward the peak");
        // The orbit advances monotonically.
        assert!(a.camera(a.duration * 0.5).rot_y > a.camera(0.0).rot_y);
    }

    #[test]
    fn flux_is_inflow_on_the_rise_and_outflow_on_the_fall() {
        let a = arr();
        // Rising arc (early) → collapse (negative flux); falling arc (late) → outflow.
        assert!(a.galaxy_state(a.duration * 0.3).core_flux < 0.0);
        assert!(a.galaxy_state(a.duration * 0.85).core_flux > 0.0);
    }

    #[test]
    fn is_deterministic_and_varies_by_seed() {
        let masses = |seed| {
            Arrangement::new(120.0, seed, Scenario::Spiral)
                .timeline(0.1)
                .iter()
                .map(|(_, s)| s.core_mass)
                .collect::<Vec<_>>()
        };
        assert_eq!(masses(1), masses(1)); // deterministic
        assert_ne!(masses(1), masses(2)); // seed varies the arc (peak position)
    }
}

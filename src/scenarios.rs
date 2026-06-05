//! Initial-condition scenarios: the bodies each setup seeds, plus the shared
//! galaxy/disk construction they have in common. The GPU solver in `simulation`
//! is identical for every scenario — only these initial conditions differ.

use crate::simulation::{Particle, G, HALO_RC, HALO_V0, NUM_PARTICLES};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::f32::consts::TAU;

// --- Spiral-disk scenario: a bulge body + a self-gravitating exponential disk
// whose own mass dominates its region (a "maximal disk"), which is spiral-prone.
const BULGE_MASS: f32 = 40_000.0;
const STAR_MASS: f32 = 21.0;
const DISK_RD: f32 = 35.0; // exponential scale length
const DISK_RMAX: f32 = 170.0; // clamp on the sampled disk radius
const DISK_THICKNESS: f32 = 4.0; // initial vertical scale
/// Softening for the spiral disk: small relative to the disk so self-gravity
/// stays "sharp" enough for spiral structure, but large enough to damp noise.
const SPIRAL_SOFTENING: f32 = 12.0;

// --- Merger scenario: two galaxies, each a heavy central body + a disk, on a
// bound approach so self-gravity merges them into one spinning remnant.
const CENTER_MASS: f32 = 300_000.0;
/// Larger softening so the two heavy nuclei coalesce into one soft core on
/// contact rather than locking into a hard, never-merging binary.
const MERGER_SOFTENING: f32 = 25.0;
const MERGER_SEP: f32 = 120.0; // each centre's distance from the origin, on x
const MERGER_APPROACH: f32 = 20.0; // each centre's prograde cross-speed, on y
const MERGER_DISK_RMIN: f32 = 4.0; // inner edge of each disk
const MERGER_DISK_RMAX: f32 = 120.0; // outer edge of each disk
const MERGER_DISK_EXP: f32 = 1.7; // radial concentration: r = rmin + (rmax−rmin)·t^EXP
const MERGER_THICKNESS: f32 = 4.0; // initial vertical half-extent
const HEADON_SPEED: f32 = 26.0; // closing speed for the head-on collision

/// Disk "temperature": the initial random velocity dispersion as a fraction of
/// the local circular speed, scaled by the temperature slider. Too cold and the
/// disk fragments into clumps; too hot and it stays a featureless smear; spiral
/// arms live in between. `DISP_FRAC` is tuned so temperature ≈ 1.0 sits in the
/// spiral sweet spot; the default (0.5) runs a touch colder for more structure,
/// and the slider explores either side.
const DISP_FRAC: f32 = 0.072;
pub const DEFAULT_TEMP: f32 = 0.5;

/// Initial velocity dispersion (a fraction of the local circular speed) for a
/// disk temperature; clamped non-negative. The single home for the temp→σ rule.
fn dispersion(temp: f32) -> f32 {
    DISP_FRAC * temp.max(0.0)
}

/// Which initial-condition scenario to seed. Chosen from the page's dropdown.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Scenario {
    /// A single self-gravitating disk that grows spiral arms.
    Spiral,
    /// Two equal galaxies on a bound prograde approach that merge.
    Merger,
    /// Two equal galaxies aimed straight at each other (no orbital spin).
    HeadOn,
    /// A merger whose second disk spins retrograde.
    Retrograde,
    /// A massive primary shredding an infalling quarter-mass satellite.
    MinorMerger,
    /// A small group of three galaxies that fall together.
    Group,
}

impl Scenario {
    pub fn from_id(id: u32) -> Self {
        match id {
            1 => Scenario::Merger,
            2 => Scenario::HeadOn,
            3 => Scenario::Retrograde,
            4 => Scenario::MinorMerger,
            5 => Scenario::Group,
            _ => Scenario::Spiral,
        }
    }

    /// Plummer softening length to use for this scenario. Every multi-galaxy
    /// setup uses the larger merger softening so heavy cores coalesce cleanly.
    pub fn softening(self) -> f32 {
        match self {
            Scenario::Spiral => SPIRAL_SOFTENING,
            _ => MERGER_SOFTENING,
        }
    }

    /// Generate the initial bodies for this scenario at a given disk temperature.
    pub fn generate(self, temp: f32) -> Vec<Particle> {
        match self {
            Scenario::Spiral => generate_disk(temp),
            Scenario::Merger => generate_merger(temp),
            Scenario::HeadOn => generate_head_on(temp),
            Scenario::Retrograde => generate_retrograde(temp),
            Scenario::MinorMerger => generate_minor(temp),
            Scenario::Group => generate_group(temp),
        }
    }
}

/// One disk star, ready for `push_disk_star`: positioned at `center + (r, θ, z)`
/// and moving at `bulk` plus a prograde circular velocity `vc` and a thermal kick
/// of dispersion `sigma`. `tint` (0..1) is stored in `vel.w`; the merger uses it
/// for galaxy-of-origin colour, while the spiral disk leaves it 0 and is coloured
/// by live radius in the render shader.
struct DiskStar {
    center: [f32; 3],
    bulk: [f32; 3],
    r: f32,
    theta: f32,
    z: f32,
    vc: f32,
    sigma: f32,
    tint: f32,
}

/// Circular speed at radius `r`, from the bulge + enclosed disk mass + halo.
/// The disk uses a spherical enclosed-mass approximation — not exact for a
/// flat disk, but close enough that the disk settles and then ripples.
fn circular_velocity(r: f32) -> f32 {
    let r = r.max(1.0);
    let r2 = r * r;
    let eps2 = SPIRAL_SOFTENING * SPIRAL_SOFTENING;
    let v_bulge2 = G * BULGE_MASS * r2 / (r2 + eps2).powf(1.5);
    let m_disk = (NUM_PARTICLES - 1) as f32 * STAR_MASS;
    let x = r / DISK_RD;
    let m_enc = m_disk * (1.0 - (1.0 + x) * (-x).exp());
    let v_disk2 = G * m_enc / r;
    let v_halo2 = HALO_V0 * HALO_V0 * r2 / (r2 + HALO_RC * HALO_RC);
    (v_bulge2 + v_disk2 + v_halo2).sqrt()
}

/// Standard-normal sample (Box–Muller).
fn gaussian(rng: &mut StdRng) -> f32 {
    let u1: f32 = rng.random_range(1e-6_f32..1.0);
    let u2: f32 = rng.random_range(0.0_f32..1.0);
    (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
}

/// Push one disk star (see `DiskStar`) into `out`. The vertical thermal kick is
/// smaller than the in-plane one to keep the disk thin.
fn push_disk_star(out: &mut Vec<Particle>, s: &DiskStar, rng: &mut StdRng) {
    let (st, ct) = (s.theta.sin(), s.theta.cos());
    out.push(Particle {
        pos_mass: [
            s.center[0] + s.r * ct,
            s.center[1] + s.r * st,
            s.center[2] + s.z,
            STAR_MASS,
        ],
        vel: [
            s.bulk[0] - s.vc * st + gaussian(rng) * s.sigma,
            s.bulk[1] + s.vc * ct + gaussian(rng) * s.sigma,
            s.bulk[2] + gaussian(rng) * s.sigma * 0.4,
            s.tint,
        ],
    });
}

/// Build a single galaxy: a heavy central bulge body plus a self-gravitating
/// exponential disk on near-circular prograde (+z) orbits, with a random thermal
/// velocity dispersion scaled by `temp` (the disk-temperature slider). The render
/// shader colours the disk by live galactocentric radius (warm core → blue arms).
fn generate_disk(temp: f32) -> Vec<Particle> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut particles = Vec::with_capacity(NUM_PARTICLES as usize);

    // Central bulge body, at rest at the origin (tint 0 → warm nucleus).
    particles.push(Particle {
        pos_mass: [0.0, 0.0, 0.0, BULGE_MASS],
        vel: [0.0, 0.0, 0.0, 0.0],
    });

    let disp = dispersion(temp);
    for _ in 1..NUM_PARTICLES {
        // Exponential disk: a gamma(2) radius gives surface density ∝ e^(-r/Rd).
        let u1: f32 = rng.random_range(1e-4_f32..1.0);
        let u2: f32 = rng.random_range(1e-4_f32..1.0);
        let r = (-DISK_RD * (u1 * u2).ln()).min(DISK_RMAX);
        let theta = rng.random_range(0.0_f32..TAU);
        let z = gaussian(&mut rng) * DISK_THICKNESS;
        let vc = circular_velocity(r);
        push_disk_star(
            &mut particles,
            &DiskStar {
                center: [0.0, 0.0, 0.0],
                bulk: [0.0, 0.0, 0.0],
                r,
                theta,
                z,
                vc,
                sigma: disp * vc,
                tint: 0.0,
            },
            &mut rng,
        );
    }

    particles
}

/// One galaxy in a multi-galaxy scenario: a heavy core plus a centrally-
/// concentrated disk in its own rest frame `bulk`, spinning prograde (`spin = 1`)
/// or retrograde (`spin = -1`). `count` is its body budget (core + disk); the
/// per-scenario counts must sum to `NUM_PARTICLES` so the buffer fills exactly.
struct Galaxy {
    center: [f32; 3],
    bulk: [f32; 3],
    core_mass: f32,
    radius: f32,
    count: u32,
    spin: f32,
    tint: f32,
}

impl Default for Galaxy {
    /// A prograde, equal-mass, full-radius, half-the-bodies galaxy at the origin.
    /// Scenarios override only the fields that differ.
    fn default() -> Self {
        Self {
            center: [0.0, 0.0, 0.0],
            bulk: [0.0, 0.0, 0.0],
            core_mass: CENTER_MASS,
            radius: MERGER_DISK_RMAX,
            count: NUM_PARTICLES / 2,
            spin: 1.0,
            tint: 0.0,
        }
    }
}

/// Seed one `Galaxy` — a heavy core plus a disk on near-circular orbits in the
/// core's softened point potential (the global halo is ignored; each disk is
/// balanced in its own frame) — into `out`.
fn seed_galaxy(out: &mut Vec<Particle>, g: &Galaxy, disp: f32, rng: &mut StdRng) {
    out.push(Particle {
        pos_mass: [g.center[0], g.center[1], g.center[2], g.core_mass],
        vel: [g.bulk[0], g.bulk[1], g.bulk[2], g.tint],
    });
    let sqrt_gm = (G * g.core_mass).sqrt();
    for _ in 1..g.count {
        let t: f32 = rng.random_range(0.0_f32..1.0);
        let r = MERGER_DISK_RMIN + (g.radius - MERGER_DISK_RMIN) * t.powf(MERGER_DISK_EXP);
        let theta = rng.random_range(0.0_f32..TAU);
        let z = rng.random_range(-MERGER_THICKNESS..MERGER_THICKNESS);
        // `spin` flips the orbital sense for retrograde galaxies.
        let vc = g.spin * sqrt_gm * r / (r * r + MERGER_SOFTENING * MERGER_SOFTENING).powf(0.75);
        push_disk_star(
            out,
            &DiskStar {
                center: g.center,
                bulk: g.bulk,
                r,
                theta,
                z,
                vc,
                sigma: disp * vc.abs(),
                tint: g.tint,
            },
            rng,
        );
    }
}

/// Seed a set of galaxies (the multi-galaxy scenarios). Their `count`s must sum
/// to `NUM_PARTICLES` so the body buffer is filled exactly.
fn generate_galaxies(galaxies: &[Galaxy], temp: f32) -> Vec<Particle> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut particles = Vec::with_capacity(NUM_PARTICLES as usize);
    let disp = dispersion(temp);
    for g in galaxies {
        seed_galaxy(&mut particles, g, disp, &mut rng);
    }
    debug_assert_eq!(particles.len(), NUM_PARTICLES as usize);
    particles
}

/// Two equal galaxies on a bound, prograde approach about the origin, so
/// self-gravity merges them into one spinning remnant. The two populations carry
/// distinct tints so you can watch them mix.
fn generate_merger(temp: f32) -> Vec<Particle> {
    generate_galaxies(
        &[
            Galaxy {
                center: [-MERGER_SEP, 0.0, 0.0],
                bulk: [0.0, -MERGER_APPROACH, 0.0],
                ..Default::default()
            },
            Galaxy {
                center: [MERGER_SEP, 0.0, 0.0],
                bulk: [0.0, MERGER_APPROACH, 0.0],
                tint: 1.0,
                ..Default::default()
            },
        ],
        temp,
    )
}

/// Two equal galaxies aimed straight at each other (no orbital angular momentum):
/// they interpenetrate and violently relax into one remnant.
fn generate_head_on(temp: f32) -> Vec<Particle> {
    generate_galaxies(
        &[
            Galaxy {
                center: [-MERGER_SEP, 0.0, 0.0],
                bulk: [HEADON_SPEED, 0.0, 0.0],
                ..Default::default()
            },
            Galaxy {
                center: [MERGER_SEP, 0.0, 0.0],
                bulk: [-HEADON_SPEED, 0.0, 0.0],
                tint: 1.0,
                ..Default::default()
            },
        ],
        temp,
    )
}

/// Like the merger, but the second disk spins retrograde — which suppresses the
/// long tidal bridge and tails a prograde pair throws off.
fn generate_retrograde(temp: f32) -> Vec<Particle> {
    generate_galaxies(
        &[
            Galaxy {
                center: [-MERGER_SEP, 0.0, 0.0],
                bulk: [0.0, -MERGER_APPROACH, 0.0],
                ..Default::default()
            },
            Galaxy {
                center: [MERGER_SEP, 0.0, 0.0],
                bulk: [0.0, MERGER_APPROACH, 0.0],
                spin: -1.0,
                tint: 1.0,
                ..Default::default()
            },
        ],
        temp,
    )
}

/// A minor merger: a massive primary and a quarter-mass satellite on an infalling
/// orbit. The satellite is tidally shredded into a stream around the primary.
fn generate_minor(temp: f32) -> Vec<Particle> {
    let satellite = NUM_PARTICLES / 4;
    generate_galaxies(
        &[
            Galaxy {
                center: [-30.0, 0.0, 0.0],
                bulk: [0.0, -8.0, 0.0],
                count: NUM_PARTICLES - satellite,
                ..Default::default()
            },
            Galaxy {
                center: [130.0, 0.0, 0.0],
                bulk: [0.0, 34.0, 0.0],
                core_mass: CENTER_MASS / 4.0,
                radius: 45.0,
                count: satellite,
                tint: 1.0,
                ..Default::default()
            },
        ],
        temp,
    )
}

/// A small group of three equal galaxies set rotating about their common centre,
/// which fall together and merge into a single system.
fn generate_group(temp: f32) -> Vec<Particle> {
    let a = NUM_PARTICLES / 3;
    generate_galaxies(
        &[
            Galaxy {
                center: [0.0, 105.0, 0.0],
                bulk: [-14.0, 0.0, 0.0],
                radius: 70.0,
                count: a,
                ..Default::default()
            },
            Galaxy {
                center: [-91.0, -52.0, 0.0],
                bulk: [7.0, -12.0, 0.0],
                radius: 70.0,
                count: a,
                tint: 0.5,
                ..Default::default()
            },
            Galaxy {
                center: [91.0, -52.0, 0.0],
                bulk: [7.0, 12.0, 0.0],
                radius: 70.0,
                count: NUM_PARTICLES - 2 * a,
                tint: 1.0,
                ..Default::default()
            },
        ],
        temp,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_id_maps_known_ids_and_defaults() {
        assert_eq!(Scenario::from_id(0), Scenario::Spiral);
        assert_eq!(Scenario::from_id(1), Scenario::Merger);
        assert_eq!(Scenario::from_id(2), Scenario::HeadOn);
        assert_eq!(Scenario::from_id(3), Scenario::Retrograde);
        assert_eq!(Scenario::from_id(4), Scenario::MinorMerger);
        assert_eq!(Scenario::from_id(5), Scenario::Group);
        // Unknown ids fall back to the spiral disk.
        assert_eq!(Scenario::from_id(99), Scenario::Spiral);
    }

    #[test]
    fn merger_softens_more_than_spiral() {
        assert!(Scenario::Spiral.softening() > 0.0);
        assert!(Scenario::Merger.softening() > Scenario::Spiral.softening());
    }

    #[test]
    fn circular_velocity_is_positive_and_finite() {
        for r in [0.0, 1.0, 10.0, 35.0, 100.0, 170.0, 500.0] {
            let v = circular_velocity(r);
            assert!(v.is_finite(), "v({r}) should be finite");
            assert!(v > 0.0, "v({r}) should be positive");
        }
    }

    fn assert_valid_bodies(particles: &[Particle]) {
        assert_eq!(particles.len(), NUM_PARTICLES as usize);
        for p in particles {
            assert!(p.pos_mass[3] > 0.0, "every body has positive mass");
            for c in p.pos_mass.iter().chain(p.vel.iter()) {
                assert!(c.is_finite(), "every component is finite");
            }
            let tint = p.vel[3];
            assert!((0.0..=1.0).contains(&tint), "tint {tint} is in [0, 1]");
        }
    }

    #[test]
    fn spiral_seeds_valid_bodies() {
        assert_valid_bodies(&Scenario::Spiral.generate(DEFAULT_TEMP));
    }

    /// Every scenario must fill the buffer exactly (galaxy counts summing to
    /// NUM_PARTICLES) with finite, positively-massed, in-range bodies.
    #[test]
    fn all_scenarios_seed_valid_bodies() {
        for s in [
            Scenario::Spiral,
            Scenario::Merger,
            Scenario::HeadOn,
            Scenario::Retrograde,
            Scenario::MinorMerger,
            Scenario::Group,
        ] {
            assert_valid_bodies(&s.generate(DEFAULT_TEMP));
        }
    }

    #[test]
    fn spiral_disk_radii_stay_within_bounds() {
        let bodies = Scenario::Spiral.generate(DEFAULT_TEMP);
        // Skip the bulge at index 0; disk radii are clamped to DISK_RMAX.
        for b in &bodies[1..] {
            let r = (b.pos_mass[0] * b.pos_mass[0] + b.pos_mass[1] * b.pos_mass[1]).sqrt();
            assert!(r <= DISK_RMAX + 1e-3, "disk radius {r} exceeds DISK_RMAX");
        }
    }

    #[test]
    fn seeding_is_deterministic() {
        // A fixed RNG seed means a given scenario+temperature is reproducible.
        let a = Scenario::Spiral.generate(1.0);
        let b = Scenario::Spiral.generate(1.0);
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&a),
            bytemuck::cast_slice::<_, u8>(&b)
        );
    }

    #[test]
    fn hotter_disk_has_more_vertical_motion() {
        // Temperature is the velocity dispersion: the vertical kick is purely
        // thermal for the spiral disk, so a hot disk spreads in vz far more.
        let mean_abs_vz = |temp| {
            let bodies = Scenario::Spiral.generate(temp);
            let disk = &bodies[1..]; // skip the bulge
            disk.iter().map(|b| b.vel[2].abs()).sum::<f32>() / disk.len() as f32
        };
        assert!(mean_abs_vz(2.0) > mean_abs_vz(0.05) * 3.0);
    }
}

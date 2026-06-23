//! Initial-condition scenarios: the bodies each setup seeds, plus the shared
//! galaxy/disk construction they have in common. The GPU solver in `simulation`
//! is identical for every scenario — only these initial conditions differ.

use crate::simulation::{
    HaloKind, Particle, G, HALO_RC, HALO_V0, NFW_G_MAX, NFW_RS, NUM_PARTICLES,
};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::f32::consts::TAU;

// --- Spiral-disk scenario: a compact bulge spheroid + a self-gravitating
// exponential disk whose own mass dominates its region (a "maximal disk"), which is
// spiral-prone.
// The bulge is a star population whose mass share is the bulge fraction (see
// `bulge_mass` / `disk_mass` and `seed_bulge`).
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

// --- Grand-design (M51) flyby: a cold main spiral disk (halo-supported, at the
// origin) perturbed by a compact companion on a close prograde passage, which
// drives a tidal two-arm pattern. The companion is self-bound (seeded in its own
// core's softened potential) and takes about an eighth of the bodies.
const FLYBY_COMPANION_MASS: f32 = 90_000.0;
const FLYBY_COMPANION_RADIUS: f32 = 36.0;
const FLYBY_COMPANION_CENTER: [f32; 3] = [210.0, 150.0, 25.0];
const FLYBY_COMPANION_BULK: [f32; 3] = [-30.0, 12.0, -4.0];

/// The disk "temperature" is expressed as the **Toomre stability parameter Q** (the
/// disk-Q slider). Q≲1 → the disk fragments into clumps; Q≈1–2 → it swing-amplifies
/// into spiral arms; Q≫2 → a smooth, featureless disk. The exponential spiral disk
/// sets its radial velocity dispersion from Q via [`toomre_sigma`]; the compact
/// merger disks (point-mass-dominated, with no clean Q) just scale their dispersion
/// linearly with it.
const DISP_FRAC: f32 = 0.0277; // merger-disk σ as a fraction of v_c, per unit Q
pub const DEFAULT_TEMP: f32 = 1.3; // default Toomre Q — the spiral sweet spot

/// Default fraction of the spiral disk seeded as dissipative gas (tagged via
/// `vel.w`). The gas cools onto the plane (see the kick kernel) and concentrates in
/// the spiral arms — the cold, blue, star-forming component over the older stellar
/// disk. ~20% is galaxy-plausible; the gas-fraction slider overrides it live.
pub const DEFAULT_GAS_FRACTION: f32 = 0.24;
/// No gas inside this radius: the bulge-dominated centre is gas-poor, so it stays
/// warm gold rather than being speckled blue (sharpening the arm/centre contrast).
const GAS_R_MIN: f32 = 22.0;

/// Default bulge fraction (bulge mass / total) — ~10%, a Milky-Way-like disk
/// galaxy. The bulge slider sweeps it from a disk-dominated late type to a
/// bulge-dominated early type.
pub const DEFAULT_BULGE_FRAC: f32 = 0.104;

/// Spatial scale of the central bulge spheroid (sim units; ≈ 1.2 kpc) — much
/// smaller than the disk scale `DISK_RD`, so it reads as a compact central knot.
const BULGE_SCALE: f32 = 12.0;

/// The galaxy's total baryonic mass (bulge + disk), held constant as the bulge
/// fraction redistributes it: the bulge slider changes the galaxy's *shape* (a
/// disk-dominated late type ↔ a bulge-dominated early type), not its mass.
fn reference_galaxy_mass() -> f32 {
    NUM_PARTICLES as f32 * STAR_MASS
}

#[derive(Copy, Clone)]
struct DiskMass {
    total: f32,
}

impl DiskMass {
    fn reference() -> Self {
        Self {
            total: reference_galaxy_mass(),
        }
    }

    fn for_seeded_bodies(count: u32, star_mass: f32) -> Self {
        Self {
            total: count as f32 * star_mass,
        }
    }

    fn bulge(self, bulge_frac: f32) -> f32 {
        self.total * bulge_frac.clamp(0.0, 0.95)
    }

    fn disk(self, bulge_frac: f32) -> f32 {
        self.total * (1.0 - bulge_frac.clamp(0.0, 0.95))
    }
}

/// Bulge and disk mass for a bulge fraction `f`: the bulge takes a fraction `f` of
/// the total, the disk the rest. Per-body mass is uniform, so the bulge's share of
/// the *bodies* equals its share of the mass (see `seed_spiral_disk`).
fn bulge_mass(bulge_frac: f32) -> f32 {
    DiskMass::reference().bulge(bulge_frac)
}
fn disk_mass(bulge_frac: f32) -> f32 {
    DiskMass::reference().disk(bulge_frac)
}

/// Softening + thickness + finite-N stability correction. Razor-thin Toomre theory
/// understates this disk's stability (the Plummer softening, the finite disk
/// thickness, and the modest particle count all damp the instability), so a given
/// visual regime corresponds to a higher Q than the thin-disk formula returns.
/// Calibrated so Q ≈ 1.3 sits in the spiral sweet spot.
const TOOMRE_SOFT: f32 = 11.0;

/// Velocity dispersion (a fraction of the local circular speed) for the merger
/// disks at Toomre parameter `q`; clamped non-negative.
fn dispersion(q: f32) -> f32 {
    DISP_FRAC * q.max(0.0)
}

/// Radial velocity dispersion (sim units) for a spiral-disk star at radius `r`,
/// set from a target Toomre parameter `q`: `σ_R = Q · 3.36 G Σ(r) / (κ(r)·C)`, with
/// the exponential-disk surface density `Σ(r) = Σ₀ e^(−r/Rd)` (`Σ₀ = M_disk/2πRd²`),
/// the epicyclic frequency `κ² = 2Ω(Ω + dv_c/dr)` from the rotation curve, and the
/// softening/thickness correction `C` ([`TOOMRE_SOFT`]). This makes the disk-Q
/// slider read a true (effective) Toomre Q.
fn toomre_sigma(r: f32, q: f32, bulge_frac: f32, halo_kind: HaloKind, mass: DiskMass) -> f32 {
    let r = r.max(1.0);
    let sigma0 = mass.disk(bulge_frac) / (TAU * DISK_RD * DISK_RD);
    let surface = sigma0 * (-r / DISK_RD).exp();
    // Epicyclic frequency from a finite-difference of the circular-velocity curve.
    let dr = 0.5;
    let vc = circular_velocity_with_mass(r, bulge_frac, halo_kind, mass);
    let dvdr = (circular_velocity_with_mass(r + dr, bulge_frac, halo_kind, mass) - vc) / dr;
    let omega = vc / r;
    let kappa = (2.0 * omega * (omega + dvdr)).max(1e-4).sqrt();
    (q.max(0.0) * 3.36 * G * surface / (kappa * TOOMRE_SOFT)).max(0.0)
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
    /// A cold spiral disk perturbed by a companion on a close prograde flyby
    /// (M51-like), driving a tidal grand-design two-arm pattern.
    GrandDesign,
}

impl Scenario {
    pub fn from_id(id: u32) -> Self {
        match id {
            1 => Scenario::Merger,
            2 => Scenario::HeadOn,
            3 => Scenario::Retrograde,
            4 => Scenario::MinorMerger,
            5 => Scenario::Group,
            6 => Scenario::GrandDesign,
            _ => Scenario::Spiral,
        }
    }

    /// Plummer softening length to use for this scenario. Every multi-galaxy
    /// setup uses the larger merger softening so heavy cores coalesce cleanly.
    pub fn softening(self) -> f32 {
        match self {
            // The grand-design flyby keeps the sharp spiral softening so the main
            // disk still swing-amplifies; its companion is a lone core, not a second
            // heavy nucleus that would need the larger merger softening.
            Scenario::Spiral | Scenario::GrandDesign => SPIRAL_SOFTENING,
            _ => MERGER_SOFTENING,
        }
    }

    /// Whether this scenario seeds a dissipative gas population. Only the
    /// halo-supported exponential disks (the spiral and the M51 main disk) do — the
    /// gas cools onto the plane and sharpens/sustains the spiral arms. The compact
    /// merger disks are treated as gas-free (collisionless stars only).
    pub fn has_gas(self) -> bool {
        matches!(self, Scenario::Spiral | Scenario::GrandDesign)
    }

    /// Generate `count` initial bodies for this scenario at a given disk
    /// temperature. `halo_kind` matters only to the spiral disk, whose circular
    /// velocities are balanced against the global halo; the multi-galaxy disks orbit
    /// their own cores and treat the halo as a background, so they ignore it.
    ///
    /// Per-body mass scales as `NUM_PARTICLES / count`, holding each system's total
    /// disk mass constant: raising the count refines the same galaxy rather than
    /// piling on mass, so the dynamics and timescales stay put. Multi-galaxy core
    /// masses stay fixed; disk-scenario bulge mass is redistributed from the same
    /// constant baryonic mass as the disk.
    pub fn generate(self, count: u32, temp: f32, halo_kind: HaloKind) -> Vec<Particle> {
        self.generate_with(
            count,
            temp,
            DEFAULT_GAS_FRACTION,
            DEFAULT_BULGE_FRAC,
            halo_kind,
        )
    }

    /// As [`generate`], but with explicit gas and bulge fractions (the gas-fraction
    /// and bulge sliders). Only the disk scenarios use them; the mergers ignore them.
    pub fn generate_with(
        self,
        count: u32,
        temp: f32,
        gas_fraction: f32,
        bulge_frac: f32,
        halo_kind: HaloKind,
    ) -> Vec<Particle> {
        let star_mass = STAR_MASS * NUM_PARTICLES as f32 / count as f32;
        match self {
            Scenario::Spiral => {
                generate_disk(count, temp, gas_fraction, bulge_frac, halo_kind, star_mass)
            }
            Scenario::Merger => generate_merger(count, temp, star_mass),
            Scenario::HeadOn => generate_head_on(count, temp, star_mass),
            Scenario::Retrograde => generate_retrograde(count, temp, star_mass),
            Scenario::MinorMerger => generate_minor(count, temp, star_mass),
            Scenario::Group => generate_group(count, temp, star_mass),
            Scenario::GrandDesign => {
                generate_grand_design(count, temp, gas_fraction, bulge_frac, halo_kind, star_mass)
            }
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
#[cfg(test)]
fn circular_velocity(r: f32, bulge_frac: f32, halo_kind: HaloKind) -> f32 {
    circular_velocity_with_mass(r, bulge_frac, halo_kind, DiskMass::reference())
}

fn circular_velocity_with_mass(
    r: f32,
    bulge_frac: f32,
    halo_kind: HaloKind,
    mass: DiskMass,
) -> f32 {
    let r = r.max(1.0);
    let r2 = r * r;
    let eps2 = SPIRAL_SOFTENING * SPIRAL_SOFTENING;
    let v_bulge2 = G * mass.bulge(bulge_frac) * r2 / (r2 + eps2).powf(1.5);
    let x = r / DISK_RD;
    let m_enc = mass.disk(bulge_frac) * (1.0 - (1.0 + x) * (-x).exp());
    let v_disk2 = G * m_enc / r;
    (v_bulge2 + v_disk2 + halo_velocity_sq(r, halo_kind)).sqrt()
}

/// The halo's contribution to circular velocity squared at radius `r`, for the
/// seed-time default halo strength (`HALO_V0`). Used when seeding disks in
/// equilibrium.
fn halo_velocity_sq(r: f32, halo_kind: HaloKind) -> f32 {
    halo_velocity_sq_at(r, HALO_V0, 1.0, halo_kind)
}

/// Halo circular velocity squared at `r` for an arbitrary characteristic speed
/// `halo_v0` (the live strength) and scale-radius multiplier `rc_scale` (the live
/// concentration: <1 = more concentrated, >1 = more diffuse). Must match the force
/// in `update.wgsl` for each profile (which reads the scaled `halo_rc2`).
fn halo_velocity_sq_at(r: f32, halo_v0: f32, rc_scale: f32, halo_kind: HaloKind) -> f32 {
    match halo_kind {
        // Logarithmic: v_halo² = v0²·r² / (r² + rc²).
        HaloKind::Logarithmic => {
            let rc = HALO_RC * rc_scale;
            halo_v0 * halo_v0 * r * r / (r * r + rc * rc)
        }
        // NFW: v_halo² = v0²·[ln(1+x) − x/(1+x)] / (x·NFW_G_MAX), with x = r/rs and
        // rs = NFW_RS; normalised so v0 is the halo's peak circular speed.
        HaloKind::Nfw => {
            let x = r / (NFW_RS * rc_scale);
            let mass_factor = (1.0 + x).ln() - x / (1.0 + x);
            halo_v0 * halo_v0 * mass_factor / (x * NFW_G_MAX)
        }
    }
}

/// Decompose the disk circular velocity at radius `r` into its [bulge, disk, halo]
/// components (sim velocity units), under the *live* gravity and halo strength (the
/// gravity / halo sliders). The quadrature sum of the three is the total circular
/// speed; drives the rotation-curve overlay. Same bulge + enclosed-disk + halo
/// model as [`circular_velocity`], so the curve reflects the same physics the disk
/// was built on.
pub fn rotation_components(
    r: f32,
    gravity: f32,
    halo_v0: f32,
    rc_scale: f32,
    bulge_frac: f32,
    halo_kind: HaloKind,
) -> [f32; 3] {
    let r = r.max(1.0);
    let r2 = r * r;
    let eps2 = SPIRAL_SOFTENING * SPIRAL_SOFTENING;
    let v_bulge2 = gravity * bulge_mass(bulge_frac) * r2 / (r2 + eps2).powf(1.5);
    let x = r / DISK_RD;
    let m_enc = disk_mass(bulge_frac) * (1.0 - (1.0 + x) * (-x).exp());
    let v_disk2 = gravity * m_enc / r;
    let v_halo2 = halo_velocity_sq_at(r, halo_v0, rc_scale, halo_kind);
    [v_bulge2.sqrt(), v_disk2.sqrt(), v_halo2.sqrt()]
}

/// Standard-normal sample (Box–Muller).
fn gaussian(rng: &mut StdRng) -> f32 {
    let u1: f32 = rng.random_range(1e-6_f32..1.0);
    let u2: f32 = rng.random_range(0.0_f32..1.0);
    (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
}

/// Push one disk star (see `DiskStar`) into `out` with per-body mass `star_mass`.
/// The vertical thermal kick is smaller than the in-plane one to keep the disk thin.
fn push_disk_star(out: &mut Vec<Particle>, s: &DiskStar, star_mass: f32, rng: &mut StdRng) {
    let (st, ct) = (s.theta.sin(), s.theta.cos());
    out.push(Particle {
        pos_mass: [
            s.center[0] + s.r * ct,
            s.center[1] + s.r * st,
            s.center[2] + s.z,
            star_mass,
        ],
        vel: [
            s.bulk[0] - s.vc * st + gaussian(rng) * s.sigma,
            s.bulk[1] + s.vc * ct + gaussian(rng) * s.sigma,
            s.bulk[2] + gaussian(rng) * s.sigma * 0.4,
            s.tint,
        ],
    });
}

/// The spiral-disk scenario: one self-gravitating exponential disk at the origin,
/// balanced against the global halo, which swing-amplifies into spiral arms.
fn generate_disk(
    count: u32,
    temp: f32,
    gas_fraction: f32,
    bulge_frac: f32,
    halo_kind: HaloKind,
    star_mass: f32,
) -> Vec<Particle> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut particles = Vec::with_capacity(count as usize);
    seed_spiral_disk(
        &mut particles,
        count,
        temp,
        gas_fraction,
        bulge_frac,
        halo_kind,
        star_mass,
        &mut rng,
    );
    particles
}

/// Seed a dispersion-supported central **bulge**: `n` old (warm) stars in a slightly
/// flattened spheroid (scale [`BULGE_SCALE`], radius-capped so none is flung far
/// out), pressure-supported by isotropic random velocities (≈ v_circ/√2) with no net
/// rotation — so it reads as a round, gold central knot rather than another disk
/// (tint 0 → coloured warm by its small radius). The disk is balanced against the
/// same bulge mass via `circular_velocity`, so the two stay in rough equilibrium.
fn seed_bulge(
    out: &mut Vec<Particle>,
    n: u32,
    bulge_frac: f32,
    halo_kind: HaloKind,
    mass: DiskMass,
    star_mass: f32,
    rng: &mut StdRng,
) {
    let r_max = 4.0 * BULGE_SCALE;
    for _ in 0..n {
        let mut x = gaussian(rng) * BULGE_SCALE;
        let mut y = gaussian(rng) * BULGE_SCALE;
        let mut z = gaussian(rng) * BULGE_SCALE * 0.7; // mildly flattened
        let r0 = (x * x + y * y + z * z).sqrt();
        if r0 > r_max {
            let s = r_max / r0;
            x *= s;
            y *= s;
            z *= s;
        }
        let r = (x * x + y * y + z * z).sqrt().max(1.0);
        let sigma = circular_velocity_with_mass(r, bulge_frac, halo_kind, mass) * 0.7;
        out.push(Particle {
            pos_mass: [x, y, z, star_mass],
            vel: [
                gaussian(rng) * sigma,
                gaussian(rng) * sigma,
                gaussian(rng) * sigma,
                0.0,
            ],
        });
    }
}

/// Seed a halo-supported exponential spiral disk into `out`: a central bulge
/// spheroid (`seed_bulge`) plus disk stars on near-circular prograde (+z) orbits,
/// balanced against the global halo (`circular_velocity`), with a random thermal
/// dispersion scaled by `temp`. Centred at the origin, at rest — shared by the
/// spiral scenario
/// and the M51 flyby's main galaxy. The render shader colours it by live
/// galactocentric radius (warm core → blue arms).
// Eight genuinely-distinct seeding inputs; bundling them into a single-use struct
// would add indirection without clarity.
#[allow(clippy::too_many_arguments)]
fn seed_spiral_disk(
    out: &mut Vec<Particle>,
    count: u32,
    temp: f32,
    gas_fraction: f32,
    bulge_frac: f32,
    halo_kind: HaloKind,
    star_mass: f32,
    rng: &mut StdRng,
) {
    let mass = DiskMass::for_seeded_bodies(count, star_mass);
    // Split the bodies between a central bulge spheroid and the disk by mass
    // fraction (uniform per-body mass → the bulge's body-share equals its mass
    // share). Raising the bulge fraction visibly grows the warm central bulge and
    // thins the disk — a disk-dominated late type sweeping to a bulge-dominated
    // early type.
    let bulge_count = ((count as f32) * bulge_frac.clamp(0.0, 0.95)).round() as u32;
    seed_bulge(
        out,
        bulge_count,
        bulge_frac,
        halo_kind,
        mass,
        star_mass,
        rng,
    );

    // The exponential, near-circular spiral disk fills the rest.
    // `temp` is the target Toomre Q; the dispersion is set per-radius from it.
    for _ in 0..count.saturating_sub(bulge_count) {
        // Exponential disk: a gamma(2) radius gives surface density ∝ e^(-r/Rd).
        let u1: f32 = rng.random_range(1e-4_f32..1.0);
        let u2: f32 = rng.random_range(1e-4_f32..1.0);
        let r = (-DISK_RD * (u1 * u2).ln()).min(DISK_RMAX);
        let theta = rng.random_range(0.0_f32..TAU);
        let z = gaussian(rng) * DISK_THICKNESS;
        let vc = circular_velocity_with_mass(r, bulge_frac, halo_kind, mass);
        // A fraction is tagged as gas (vel.w = 1): the kick kernel cools it and the
        // render shader draws it blue. Stars keep vel.w = 0 (coloured by radius).
        // The gas-poor inner bulge stays gas-free, so the centre reads warm gold
        // against the blue, star-forming arms (the contrast of a real spiral).
        let is_gas = r > GAS_R_MIN && rng.random_range(0.0_f32..1.0) < gas_fraction;
        push_disk_star(
            out,
            &DiskStar {
                center: [0.0, 0.0, 0.0],
                bulk: [0.0, 0.0, 0.0],
                r,
                theta,
                z,
                vc,
                sigma: toomre_sigma(r, temp, bulge_frac, halo_kind, mass),
                tint: if is_gas { 1.0 } else { 0.0 },
            },
            star_mass,
            rng,
        );
    }
}

/// One galaxy in a multi-galaxy scenario: a heavy core plus a centrally-
/// concentrated disk in its own rest frame `bulk`, spinning prograde (`spin = 1`)
/// or retrograde (`spin = -1`). `count` is its body budget (core + disk); the
/// per-scenario counts must sum to the requested total so the buffer fills exactly.
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
fn seed_galaxy(out: &mut Vec<Particle>, g: &Galaxy, disp: f32, star_mass: f32, rng: &mut StdRng) {
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
            star_mass,
            rng,
        );
    }
}

/// Seed a set of galaxies (the multi-galaxy scenarios). Their `count`s must sum to
/// `count` so the body buffer is filled exactly.
fn generate_galaxies(galaxies: &[Galaxy], count: u32, temp: f32, star_mass: f32) -> Vec<Particle> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut particles = Vec::with_capacity(count as usize);
    let disp = dispersion(temp);
    for g in galaxies {
        seed_galaxy(&mut particles, g, disp, star_mass, &mut rng);
    }
    debug_assert_eq!(particles.len(), count as usize);
    particles
}

/// Two equal galaxies on a bound, prograde approach about the origin, so
/// self-gravity merges them into one spinning remnant. The two populations carry
/// distinct tints so you can watch them mix.
fn generate_merger(count: u32, temp: f32, star_mass: f32) -> Vec<Particle> {
    generate_galaxies(
        &[
            Galaxy {
                center: [-MERGER_SEP, 0.0, 0.0],
                bulk: [0.0, -MERGER_APPROACH, 0.0],
                count: count / 2,
                ..Default::default()
            },
            Galaxy {
                center: [MERGER_SEP, 0.0, 0.0],
                bulk: [0.0, MERGER_APPROACH, 0.0],
                count: count - count / 2,
                tint: 1.0,
                ..Default::default()
            },
        ],
        count,
        temp,
        star_mass,
    )
}

/// Two equal galaxies aimed straight at each other (no orbital angular momentum):
/// they interpenetrate and violently relax into one remnant.
fn generate_head_on(count: u32, temp: f32, star_mass: f32) -> Vec<Particle> {
    generate_galaxies(
        &[
            Galaxy {
                center: [-MERGER_SEP, 0.0, 0.0],
                bulk: [HEADON_SPEED, 0.0, 0.0],
                count: count / 2,
                ..Default::default()
            },
            Galaxy {
                center: [MERGER_SEP, 0.0, 0.0],
                bulk: [-HEADON_SPEED, 0.0, 0.0],
                count: count - count / 2,
                tint: 1.0,
                ..Default::default()
            },
        ],
        count,
        temp,
        star_mass,
    )
}

/// Like the merger, but the second disk spins retrograde — which suppresses the
/// long tidal bridge and tails a prograde pair throws off.
fn generate_retrograde(count: u32, temp: f32, star_mass: f32) -> Vec<Particle> {
    generate_galaxies(
        &[
            Galaxy {
                center: [-MERGER_SEP, 0.0, 0.0],
                bulk: [0.0, -MERGER_APPROACH, 0.0],
                count: count / 2,
                ..Default::default()
            },
            Galaxy {
                center: [MERGER_SEP, 0.0, 0.0],
                bulk: [0.0, MERGER_APPROACH, 0.0],
                count: count - count / 2,
                spin: -1.0,
                tint: 1.0,
                ..Default::default()
            },
        ],
        count,
        temp,
        star_mass,
    )
}

/// A minor merger: a massive primary and a quarter-mass satellite on an infalling
/// orbit. The satellite is tidally shredded into a stream around the primary.
fn generate_minor(count: u32, temp: f32, star_mass: f32) -> Vec<Particle> {
    let satellite = count / 4;
    generate_galaxies(
        &[
            Galaxy {
                center: [-30.0, 0.0, 0.0],
                bulk: [0.0, -8.0, 0.0],
                count: count - satellite,
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
        count,
        temp,
        star_mass,
    )
}

/// A small group of three equal galaxies set rotating about their common centre,
/// which fall together and merge into a single system.
fn generate_group(count: u32, temp: f32, star_mass: f32) -> Vec<Particle> {
    let a = count / 3;
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
                count: count - 2 * a,
                tint: 1.0,
                ..Default::default()
            },
        ],
        count,
        temp,
        star_mass,
    )
}

/// A cold main spiral disk perturbed by a compact companion on a close prograde
/// flyby — the M51 mechanism for a grand-design two-arm pattern. The main disk is
/// halo-supported (like the spiral scenario); the companion is a small self-bound
/// galaxy that sweeps past rather than immediately merging.
fn generate_grand_design(
    count: u32,
    temp: f32,
    gas_fraction: f32,
    bulge_frac: f32,
    halo_kind: HaloKind,
    star_mass: f32,
) -> Vec<Particle> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut particles = Vec::with_capacity(count as usize);
    let companion = count / 8;
    seed_spiral_disk(
        &mut particles,
        count - companion,
        temp,
        gas_fraction,
        bulge_frac,
        halo_kind,
        star_mass,
        &mut rng,
    );
    seed_galaxy(
        &mut particles,
        &Galaxy {
            center: FLYBY_COMPANION_CENTER,
            bulk: FLYBY_COMPANION_BULK,
            core_mass: FLYBY_COMPANION_MASS,
            radius: FLYBY_COMPANION_RADIUS,
            count: companion,
            tint: 1.0,
            ..Default::default()
        },
        dispersion(temp),
        star_mass,
        &mut rng,
    );
    debug_assert_eq!(particles.len(), count as usize);
    particles
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
        assert_eq!(Scenario::from_id(6), Scenario::GrandDesign);
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
        for halo in [HaloKind::Logarithmic, HaloKind::Nfw] {
            for r in [0.0, 1.0, 10.0, 35.0, 100.0, 170.0, 500.0] {
                let v = circular_velocity(r, DEFAULT_BULGE_FRAC, halo);
                assert!(v.is_finite(), "v({r}) for {halo:?} should be finite");
                assert!(v > 0.0, "v({r}) for {halo:?} should be positive");
            }
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
        assert_valid_bodies(&Scenario::Spiral.generate(
            NUM_PARTICLES,
            DEFAULT_TEMP,
            HaloKind::Logarithmic,
        ));
        assert_valid_bodies(&Scenario::Spiral.generate(NUM_PARTICLES, DEFAULT_TEMP, HaloKind::Nfw));
    }

    const ALL_SCENARIOS: [Scenario; 7] = [
        Scenario::Spiral,
        Scenario::Merger,
        Scenario::HeadOn,
        Scenario::Retrograde,
        Scenario::MinorMerger,
        Scenario::Group,
        Scenario::GrandDesign,
    ];

    /// Every scenario must fill the buffer exactly (galaxy counts summing to
    /// NUM_PARTICLES) with finite, positively-massed, in-range bodies — under
    /// either halo profile.
    #[test]
    fn all_scenarios_seed_valid_bodies() {
        for s in ALL_SCENARIOS {
            for halo in [HaloKind::Logarithmic, HaloKind::Nfw] {
                assert_valid_bodies(&s.generate(NUM_PARTICLES, DEFAULT_TEMP, halo));
            }
        }
    }

    /// The body-count slider passes any tile-multiple count; every scenario must
    /// seed exactly that many bodies (the per-galaxy splits sum to the total).
    #[test]
    fn every_scenario_seeds_the_requested_count() {
        for s in ALL_SCENARIOS {
            for count in [256, 2048, NUM_PARTICLES, 32_768, NUM_PARTICLES * 10] {
                let bodies = s.generate(count, DEFAULT_TEMP, HaloKind::Logarithmic);
                assert_eq!(bodies.len(), count as usize, "{s:?} at count {count}");
            }
        }
    }

    /// Raising the count holds each system's total mass ~constant (per-body mass
    /// scales as 1/count), so more bodies refine the same galaxy.
    #[test]
    fn count_scales_per_body_mass_inversely() {
        let total = |count| {
            Scenario::Spiral
                .generate(count, DEFAULT_TEMP, HaloKind::Logarithmic)
                .iter()
                .map(|p| p.pos_mass[3] as f64)
                .sum::<f64>()
        };
        let (base, dense) = (total(NUM_PARTICLES), total(NUM_PARTICLES * 4));
        assert!(
            (dense - base).abs() / base < 0.001,
            "total mass should stay ~constant: {base} vs {dense}"
        );
    }

    #[test]
    fn grand_design_main_disk_uses_its_allocated_mass() {
        let count = NUM_PARTICLES;
        let star_mass = STAR_MASS * NUM_PARTICLES as f32 / count as f32;
        let companion = count / 8;
        let main_mass = DiskMass::for_seeded_bodies(count - companion, star_mass);
        let ratio = main_mass.total / reference_galaxy_mass();
        assert!(
            (ratio - 0.875).abs() < 1e-6,
            "M51 main disk should be seeded against its 7/8 body allocation, got {ratio}"
        );
    }

    #[test]
    fn spiral_disk_radii_stay_within_bounds() {
        let bodies = Scenario::Spiral.generate(NUM_PARTICLES, DEFAULT_TEMP, HaloKind::Logarithmic);
        // Disk radii are clamped to DISK_RMAX; the bulge spheroid is radius-capped
        // well within it — so every body stays inside DISK_RMAX.
        for b in &bodies {
            let r = (b.pos_mass[0] * b.pos_mass[0] + b.pos_mass[1] * b.pos_mass[1]).sqrt();
            assert!(r <= DISK_RMAX + 1e-3, "radius {r} exceeds DISK_RMAX");
        }
    }

    #[test]
    fn seeding_is_deterministic() {
        // A fixed RNG seed means a given scenario+temperature is reproducible.
        let a = Scenario::Spiral.generate(NUM_PARTICLES, 1.0, HaloKind::Nfw);
        let b = Scenario::Spiral.generate(NUM_PARTICLES, 1.0, HaloKind::Nfw);
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
            let bodies = Scenario::Spiral.generate(NUM_PARTICLES, temp, HaloKind::Logarithmic);
            // Disk stars only (out beyond the central bulge), where vz is the
            // temperature-set thermal kick rather than the bulge's pressure support.
            let disk: Vec<_> = bodies
                .iter()
                .filter(|b| {
                    let r2 = b.pos_mass[0] * b.pos_mass[0] + b.pos_mass[1] * b.pos_mass[1];
                    (50.0..150.0).contains(&r2.sqrt())
                })
                .collect();
            disk.iter().map(|b| b.vel[2].abs()).sum::<f32>() / disk.len() as f32
        };
        assert!(mean_abs_vz(2.0) > mean_abs_vz(0.05) * 3.0);
    }
}

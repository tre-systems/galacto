// Compute shaders for a self-gravitating N-body galaxy sandbox.
//
// Every particle has mass and attracts every other, so structure forms for real:
// a cold disk swing-amplifies into spiral arms, and two galaxies fall together
// and violently relax into a single rotating remnant. Gravity is the all-pairs
// sum, evaluated with workgroup-shared "tiles" to amortise global memory reads.
//
// Each step is a drift–kick–drift (leapfrog) integration, recorded as three
// passes: `drift_half` advances positions by half a step, `compute_accel`
// evaluates the all-pairs sum at that midpoint (a separate pass so the gravity
// sum never reads positions while they are being written), and `kick_drift_half`
// applies the full velocity kick plus the second half-drift. Leapfrog is
// symplectic and 2nd-order, so orbits and the cold disk hold their structure far
// longer than the 1st-order Euler step would allow.

struct Particle {
    pos_mass: vec4<f32>, // xyz = position, w = mass
    vel: vec4<f32>,      // xyz = velocity, w = colour tint
}

struct Params {
    dt: f32,
    g: f32,          // gravitational constant
    softening: f32,  // Plummer softening length
    particle_count: u32,
    halo_v0_sq: f32, // dark-matter halo: squared characteristic circular speed
    halo_rc2: f32,   // dark-matter halo: squared core / scale radius
    halo_kind: u32,  // dark-matter halo profile: 0 = logarithmic, 1 = NFW
    _pad1: u32,
}

@group(0) @binding(0) var<storage, read_write> particles: array<Particle>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> accel: array<vec4<f32>>;
// One vec4 per workgroup of partial sums for the core-statistics reduction
// (`reduce_core`): x = windowed mass, y = windowed mass·radial-velocity (signed
// flux, + outward), z = windowed mass·|radial velocity|. Summed on the CPU after
// an async readback to drive the audio — the sim itself never reads it.
@group(0) @binding(3) var<storage, read_write> reductions: array<vec4<f32>>;

// Tile size == workgroup size. `particle_count` must be a multiple of TILE so
// the tile loop never reads out of bounds (see NUM_PARTICLES / WORKGROUP_SIZE).
const TILE: u32 = 256u;
var<workgroup> shared_pm: array<vec4<f32>, 256>;

@compute @workgroup_size(256)
fn compute_accel(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) lidx: u32,
) {
    let i = gid.x;
    let pi = particles[i].pos_mass.xyz;
    let soft2 = params.softening * params.softening;
    let n = params.particle_count;

    var a = vec3<f32>(0.0);
    var base = 0u;
    loop {
        if base >= n {
            break;
        }
        // Cooperatively stage a tile of bodies into workgroup memory, then have
        // every thread accumulate the tile's pull on its own particle.
        shared_pm[lidx] = particles[base + lidx].pos_mass;
        workgroupBarrier();
        for (var j = 0u; j < TILE; j = j + 1u) {
            let pmj = shared_pm[j];
            let d = pmj.xyz - pi;
            let r2 = dot(d, d) + soft2;
            let inv = inverseSqrt(r2);
            // Self term (d == 0) contributes nothing, so no need to skip it.
            a += params.g * pmj.w * d * (inv * inv * inv);
        }
        workgroupBarrier();
        base = base + TILE;
    }

    // Static dark-matter halo centred at the origin; the profile is chosen by
    // halo_kind (mirrored by the scenario seeding so disks start in equilibrium).
    if params.halo_kind == 0u {
        // Logarithmic: a = -v0^2 · pos / (|pos|^2 + rc^2). Its potential grows
        // without bound, so the system stays bound — debris orbits back instead of
        // escaping — and the outer rotation curve is flat.
        let rh2 = dot(pi, pi) + params.halo_rc2;
        a = a - params.halo_v0_sq * pi / rh2;
    } else {
        // NFW (cold dark matter): the enclosed-mass pull of a rho ~ 1/(r(1+r/rs)^2)
        // halo. With x = r/rs and rs = sqrt(rc2), the circular-velocity shape
        // [ln(1+x) - x/(1+x)]/x is normalised by its peak (0.2162 = NFW_G_MAX) so
        // v0 is the *peak* speed. Unlike the log halo, its potential is finite, so
        // fast debris can escape. The mass factor ~ x^2/2 near the centre cancels
        // the r^3, so |a| stays finite at the origin (the cusp is in density only);
        // the floored r^3 just guards the exact-origin 0/0.
        let rs = sqrt(params.halo_rc2);
        let x = length(pi) / rs;
        let mass_factor = log(1.0 + x) - x / (1.0 + x);
        let r3 = max(dot(pi, pi) * length(pi), 1e-3);
        a = a - params.halo_v0_sq * rs * mass_factor / (0.2162 * r3) * pi;
    }

    accel[i] = vec4<f32>(a, 0.0);
}

// Leapfrog, part 1 — half-drift: advance position by half a step using the
// current velocity, moving each body to the interval midpoint where the kick is
// sampled. This reads no acceleration, so a freshly seeded scenario needs no
// primed accel buffer: the very next pass evaluates gravity at this midpoint.
@compute @workgroup_size(256)
fn drift_half(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= params.particle_count {
        return;
    }
    let p = particles[i];
    let x = p.pos_mass.xyz + p.vel.xyz * (0.5 * params.dt);
    particles[i] = Particle(vec4<f32>(x, p.pos_mass.w), p.vel);
}

// Leapfrog, part 2 — full kick + second half-drift: with acceleration now sampled
// at the midpoint, apply the whole-step velocity kick, then drift the second half
// with the updated velocity. vel.w carries the colour tint, so preserve it.
@compute @workgroup_size(256)
fn kick_drift_half(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= params.particle_count {
        return;
    }
    let p = particles[i];
    let v = p.vel.xyz + accel[i].xyz * params.dt;
    let x = p.pos_mass.xyz + v * (0.5 * params.dt);
    particles[i] = Particle(vec4<f32>(x, p.pos_mass.w), vec4<f32>(v, p.vel.w));
}

// Soft window (Gaussian) radius defining "the centre" for the audio reduction.
// Bodies within ~this distance of the origin count toward the core statistics.
const CORE_R: f32 = 60.0;

var<workgroup> sh_mass: array<f32, 256>;
var<workgroup> sh_flux: array<f32, 256>;
var<workgroup> sh_act: array<f32, 256>;

// Core-statistics reduction: per workgroup, sum each body's window-weighted mass,
// signed radial flux, and radial speed into shared memory, then write the
// workgroup's partial sums to `reductions[workgroup]`. The CPU sums the (few)
// partials after an async readback and maps them to sound — how much mass sits at
// the centre and how fast it is moving in or out. Read-only on the bodies, so it
// is safe to run alongside the render pass.
@compute @workgroup_size(256)
fn reduce_core(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) lidx: u32,
    @builtin(workgroup_id) wid: vec3<u32>,
) {
    let i = gid.x;
    var m = 0.0;
    var f = 0.0;
    var a = 0.0;
    if i < params.particle_count {
        let p = particles[i].pos_mass.xyz;
        let mass = particles[i].pos_mass.w;
        let v = particles[i].vel.xyz;
        let r = length(p);
        let w = exp(-(r * r) / (2.0 * CORE_R * CORE_R));
        let vr = dot(v, p) / max(r, 1e-3); // radial velocity, + outward
        m = w * mass;
        f = w * mass * vr;
        a = w * mass * abs(vr);
    }
    sh_mass[lidx] = m;
    sh_flux[lidx] = f;
    sh_act[lidx] = a;
    workgroupBarrier();

    // Tree reduction over the workgroup.
    var stride = TILE / 2u;
    loop {
        if stride == 0u {
            break;
        }
        if lidx < stride {
            sh_mass[lidx] = sh_mass[lidx] + sh_mass[lidx + stride];
            sh_flux[lidx] = sh_flux[lidx] + sh_flux[lidx + stride];
            sh_act[lidx] = sh_act[lidx] + sh_act[lidx + stride];
        }
        workgroupBarrier();
        stride = stride / 2u;
    }

    if lidx == 0u {
        reductions[wid.x] = vec4<f32>(sh_mass[0], sh_flux[0], sh_act[0], 0.0);
    }
}

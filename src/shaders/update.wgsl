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
    halo_v0_sq: f32, // dark-matter halo: squared asymptotic circular speed
    halo_rc2: f32,   // dark-matter halo: squared core radius
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<storage, read_write> particles: array<Particle>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> accel: array<vec4<f32>>;

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

    // Static logarithmic dark-matter halo centred at the origin: a gentle inward
    // pull, a = -v0^2 · pos / (|pos|^2 + rc^2). Its potential grows without bound,
    // so the system stays gravitationally bound — debris orbits back instead of
    // escaping to infinity — and it adds a flat outer rotation curve.
    let rh2 = dot(pi, pi) + params.halo_rc2;
    a = a - params.halo_v0_sq * pi / rh2;

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

// Compute shaders for a restricted N-body galaxy interaction.
//
// A few massive "cores" (galaxy centres) move under their mutual gravity
// (`update_cores`); the many particles are massless test stars that move in the
// cores' combined, Plummer-softened field (`update_particles`). Integration is
// symplectic Euler (kick then drift) — far better orbit energy conservation
// than explicit Euler, with no velocity clamp or boundary.

struct Particle {
    position: vec3<f32>,
    velocity: vec3<f32>,
}

// xyz = position, w = mass; second vec4 xyz = velocity (vec4 for unambiguous layout).
struct Core {
    pos_mass: vec4<f32>,
    vel: vec4<f32>,
}

struct Params {
    dt: f32,
    g: f32,         // gravitational constant
    softening: f32, // Plummer softening length
    particle_count: u32,
    num_cores: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<storage, read_write> particles: array<Particle>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> cores: array<Core>;

// Plummer-softened acceleration toward a body of `mass`, where `d = body - point`.
fn accel(d: vec3<f32>, mass: f32) -> vec3<f32> {
    let r2 = dot(d, d) + params.softening * params.softening;
    let inv = inverseSqrt(r2);
    return params.g * mass * d * (inv * inv * inv);
}

@compute @workgroup_size(64)
fn update_particles(@builtin(global_invocation_id) gid: vec3<u32>) {
    let index = gid.x;
    if index >= params.particle_count {
        return;
    }
    var p = particles[index];

    var a = vec3<f32>(0.0);
    for (var i = 0u; i < params.num_cores; i = i + 1u) {
        a += accel(cores[i].pos_mass.xyz - p.position, cores[i].pos_mass.w);
    }

    // Symplectic Euler: kick, then drift.
    p.velocity += a * params.dt;
    p.position += p.velocity * params.dt;
    particles[index] = p;
}

const MAX_CORES: u32 = 8u;

// A single invocation integrates all cores (tiny N) to avoid read/write races.
@compute @workgroup_size(1)
fn update_cores() {
    let n = min(params.num_cores, MAX_CORES);
    var pos: array<vec3<f32>, 8>;
    var vel: array<vec3<f32>, 8>;
    var mass: array<f32, 8>;
    for (var i = 0u; i < n; i = i + 1u) {
        pos[i] = cores[i].pos_mass.xyz;
        vel[i] = cores[i].vel.xyz;
        mass[i] = cores[i].pos_mass.w;
    }
    // Kick every core in the others' field (start-of-step positions).
    for (var i = 0u; i < n; i = i + 1u) {
        var a = vec3<f32>(0.0);
        for (var j = 0u; j < n; j = j + 1u) {
            if j != i {
                a += accel(pos[j] - pos[i], mass[j]);
            }
        }
        vel[i] += a * params.dt;
    }
    // Drift and write back.
    for (var i = 0u; i < n; i = i + 1u) {
        pos[i] += vel[i] * params.dt;
        cores[i] = Core(vec4<f32>(pos[i], mass[i]), vec4<f32>(vel[i], 0.0));
    }
}

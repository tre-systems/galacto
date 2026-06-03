// Compute shader for updating particle positions and velocities
struct Particle {
    position: vec3<f32>,
    velocity: vec3<f32>,
}

struct Params {
    dt: f32,
    gm: f32,            // Gravitational parameter (G * central_mass)
    max_velocity: f32,  // Speed clamp for integrator stability
    boundary: f32,      // World half-extent
    restitution: f32,   // Boundary bounce energy retention
    particle_count: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<storage, read_write> particles: array<Particle>;
@group(0) @binding(1) var<uniform> params: Params;

@compute @workgroup_size(64)
fn update_particles(@builtin(global_invocation_id) gid: vec3<u32>) {
    let index = gid.x;
    if index >= params.particle_count {
        return;
    }

    var particle = particles[index];
    
    // Calculate distance from center (0, 0, 0)
    let r2 = dot(particle.position, particle.position) + 1e-6; // Add small epsilon to avoid division by zero
    let r = sqrt(r2);
    let inv_r = 1.0 / r;
    let inv_r3 = inv_r * inv_r * inv_r;
    
    // Gravitational acceleration towards center: a = -GM/r^3 * position_vector
    let acceleration = -params.gm * inv_r3 * particle.position;

    let drag = 1.00; // No energy loss to maintain stable orbits
    
    // Euler integration
    particle.velocity = particle.velocity * drag + acceleration * params.dt;
    
    // Clamp velocity to maximum speed
    let current_speed = length(particle.velocity);
    if current_speed > params.max_velocity {
        particle.velocity = normalize(particle.velocity) * params.max_velocity;
    }

    particle.position = particle.position + particle.velocity * params.dt;
    
    // Boundary conditions - bounce off edges in 3D
    if abs(particle.position.x) > params.boundary {
        particle.position.x = sign(particle.position.x) * params.boundary;
        particle.velocity.x = -particle.velocity.x * params.restitution;
    }
    if abs(particle.position.y) > params.boundary {
        particle.position.y = sign(particle.position.y) * params.boundary;
        particle.velocity.y = -particle.velocity.y * params.restitution;
    }
    if abs(particle.position.z) > params.boundary {
        particle.position.z = sign(particle.position.z) * params.boundary;
        particle.velocity.z = -particle.velocity.z * params.restitution;
    }

    particles[index] = particle;
}
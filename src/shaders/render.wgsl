// Render each particle as a camera-facing billboard quad with a soft radial
// glow. The quads are drawn instanced (4 verts × N particles) and blended
// additively, so overlapping particles accumulate brightness.

struct Particle {
    position: vec3<f32>,
    velocity: vec3<f32>,
}

struct Camera {
    transform: mat4x4<f32>,
    size: f32,          // billboard half-extent in NDC.y (screen-constant)
    aspect: f32,        // viewport width / height (keeps quads square)
    galaxy_split: f32,  // instances below this index belong to galaxy A, the rest to B
    _pad1: f32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) offset: vec2<f32>, // quad corner in [-1, 1], for the radial falloff
}

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> particles: array<Particle>;

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    // Triangle-strip quad corners.
    var corners = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex_index];

    let particle = particles[instance_index];
    var clip = camera.transform * vec4<f32>(particle.position, 1.0);

    // Offset in clip space so the billboard is a constant size on screen
    // regardless of depth; divide x by aspect to keep it square.
    clip.x += corner.x * camera.size * clip.w / camera.aspect;
    clip.y += corner.y * camera.size * clip.w;

    // Color by galaxy of origin so tidal tails and mixing stay legible: A is a
    // cool blue-white, B a warm amber. A faint speed term lets fast (inner /
    // shocked) stars read slightly brighter.
    let is_a = f32(instance_index) < camera.galaxy_split;
    let base = select(vec3<f32>(1.0, 0.62, 0.30), vec3<f32>(0.45, 0.65, 1.0), is_a);
    let speed = length(particle.velocity);
    let boost = 1.0 + min(speed / 220.0, 0.6);
    let color = base * boost;

    var out: VertexOutput;
    out.clip_position = clip;
    out.color = color;
    out.offset = corner;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Soft round falloff from the quad center.
    let d = length(in.offset);
    let glow = max(0.0, 1.0 - d);
    let intensity = glow * glow;

    // Additive blending sums these into the framebuffer; keep per-particle
    // brightness modest so dense regions accumulate into a bright core.
    let rgb = in.color * intensity * 0.3;
    return vec4<f32>(rgb, intensity);
}

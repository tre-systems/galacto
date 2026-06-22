// Dark-matter halo overlay: a single camera-facing quad centred on the origin,
// sized to the active halo's scale radius, shaded as a soft radial cloud. Drawn
// additively (like the particles) behind the stars and bloomed, so the halo reads
// as a diffuse glowing sphere. Off unless the "Show" toggle is on.

struct Camera {
    transform: mat4x4<f32>,
    size: f32,
    aspect: f32,
    color_mode: f32,
    _spare1: f32,
}

struct HaloViz {
    right: vec4<f32>,  // xyz = camera right in world space
    up: vec4<f32>,     // xyz = camera up in world space
    color: vec4<f32>,  // rgb = halo colour, a = intensity
    radius: f32,       // world half-extent of the quad
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> halo: HaloViz;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) offset: vec2<f32>, // quad corner in [-1, 1], for the radial falloff
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var corners = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex_index];

    // Billboard the quad at the origin so it always faces the camera (a sphere
    // projects to a circle from any angle).
    let world = halo.right.xyz * (corner.x * halo.radius)
        + halo.up.xyz * (corner.y * halo.radius);

    var out: VertexOutput;
    out.clip_position = camera.transform * vec4<f32>(world, 1.0);
    out.offset = corner;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Soft radial falloff from the centre → a diffuse cloud, brightest at the core.
    let d = length(in.offset);
    let glow = max(0.0, 1.0 - d);
    let a = glow * glow * halo.color.a;
    return vec4<f32>(halo.color.rgb * a, a);
}

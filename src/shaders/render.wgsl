// Render each particle as a camera-facing billboard quad with a soft radial
// glow. The quads are drawn instanced (4 verts × N particles) and blended
// additively, so overlapping particles accumulate brightness.

struct Particle {
    pos_mass: vec4<f32>, // xyz = position, w = mass
    vel: vec4<f32>,      // xyz = velocity, w = colour tint in [0, 1]
}

struct Camera {
    transform: mat4x4<f32>,
    size: f32,        // billboard half-extent in NDC.y (screen-constant)
    aspect: f32,      // viewport width / height (keeps quads square)
    color_mode: f32,  // 0 = tint by live radius (spiral), 1 = tint by vel.w (merger)
    _spare1: f32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) offset: vec2<f32>, // quad corner in [-1, 1], for the radial falloff
}

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> particles: array<Particle>;

// Appearance tuning (render-only; not mirrored from Rust).
const TINT_RADIUS: f32 = 90.0;    // disk radius mapped to the cool end of the ramp
const SPEED_REF: f32 = 260.0;     // speed at which the brightness boost saturates
const SPEED_BOOST_MAX: f32 = 0.5; // peak extra brightness from speed
const GLOW_GAIN: f32 = 0.45;      // per-particle additive glow gain

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
    var clip = camera.transform * vec4<f32>(particle.pos_mass.xyz, 1.0);

    // Offset in clip space so the billboard is a constant size on screen
    // regardless of depth; divide x by aspect to keep it square.
    clip.x += corner.x * camera.size * clip.w / camera.aspect;
    clip.y += corner.y * camera.size * clip.w;

    // Warm yellow-white core fading to cool blue. The spiral disk tints by live
    // galactocentric radius (a warm bulge → blue arms, like a real spiral); the
    // merger tints by each body's vel.w (galaxy of origin) so the two populations
    // stay distinguishable as they mix. In disk scenarios, vel.w > 0.5 flags the
    // cold gas (star-forming) population, drawn a bright cyan-blue.
    var base: vec3<f32>;
    if camera.color_mode < 0.5 && particle.vel.w > 0.5 {
        base = vec3<f32>(0.25, 0.6, 1.35);
    } else {
        var tint = clamp(particle.vel.w, 0.0, 1.0);
        if camera.color_mode < 0.5 {
            // Steepen the radial ramp (pow > 1) so the bulge holds a saturated gold
            // and only the outer disk cools to blue — strong centre/arm contrast.
            let rr = clamp(length(particle.pos_mass.xy) / TINT_RADIUS, 0.0, 1.0);
            tint = pow(rr, 1.7);
        }
        base = mix(vec3<f32>(1.0, 0.68, 0.26), vec3<f32>(0.45, 0.6, 1.0), tint);
    }
    let speed = length(particle.vel.xyz);
    let boost = 1.0 + min(speed / SPEED_REF, SPEED_BOOST_MAX);
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

    // Additive blending sums these into the framebuffer. With fewer bodies than a
    // test-particle sim, each one carries a bit more glow so dense regions still
    // build into a bright core.
    let rgb = in.color * intensity * GLOW_GAIN;
    return vec4<f32>(rgb, intensity);
}

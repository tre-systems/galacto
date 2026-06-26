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
    glow: f32,        // halo reach / falloff control (0..1) from the Glow slider
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
const SPEED_REF: f32 = 260.0;     // speed scale for the brightness boost
const SPEED_BOOST_MAX: f32 = 0.5; // peak extra brightness from speed
const GLOW_GAIN: f32 = 0.45;      // per-particle additive glow gain
// Glow shaping, driven by the Glow slider via `camera.glow` (0..1). The quad grows
// with glow so the faint halo can reach further, while the bright core is held to a
// constant on-screen size (its offset radius shrinks as the quad grows) — so more
// glow spreads the halo outward without ever fattening the central point.
const GLOW_REACH: f32 = 2.0;      // halo reach at glow = 1 (quad up to ×3)
const GLOW_CORE_FRAC: f32 = 0.16; // bright-core radius as a fraction of the base size
const GLOW_HALO_POW: f32 = 2.5;   // halo falloff exponent — higher drops off quicker
const GLOW_HALO_AMP: f32 = 0.45;  // halo brightness relative to the core

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
    // regardless of depth; divide x by aspect to keep it square. The quad grows with
    // the glow control so the faint halo has room to reach further out.
    let reach = 1.0 + GLOW_REACH * camera.glow;
    let half = camera.size * reach;
    clip.x += corner.x * half * clip.w / camera.aspect;
    clip.y += corner.y * half * clip.w;

    // Warm yellow-white core fading to cool blue. The spiral disk tints by live
    // galactocentric radius (a warm bulge → blue arms, like a real spiral); the
    // merger tints by each body's vel.w (galaxy of origin) so the two populations
    // stay distinguishable as they mix. In disk scenarios, vel.w > 0.5 flags the
    // cold gas (star-forming) population, drawn a bright cyan-blue.
    var base: vec3<f32>;
    if camera.color_mode < 0.5 {
        // Disk scenarios: the stellar disk is warm (a saturated gold bulge fading to
        // a warm cream) and is never blue — so all the blue comes from the cold gas
        // (vel.w > 0.5), which has cooled and gathered into the arms. The result is
        // a warm centre with blue, star-forming arms standing out against it.
        if particle.vel.w > 0.5 {
            base = vec3<f32>(0.30, 0.62, 1.35);
        } else {
            let rr = clamp(length(particle.pos_mass.xy) / TINT_RADIUS, 0.0, 1.0);
            let tint = pow(rr, 1.6);
            base = mix(vec3<f32>(1.0, 0.66, 0.24), vec3<f32>(0.95, 0.84, 0.70), tint);
        }
    } else {
        // Merger: tint by each body's galaxy of origin (vel.w) so the two
        // populations stay distinguishable as they mix.
        let tint = clamp(particle.vel.w, 0.0, 1.0);
        base = mix(vec3<f32>(1.0, 0.85, 0.55), vec3<f32>(0.45, 0.6, 1.0), tint);
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
    let d = length(in.offset);
    let reach = 1.0 + GLOW_REACH * camera.glow;

    // Bright central point: a tight Gaussian whose on-screen size stays constant as
    // the quad grows (its offset radius shrinks with reach), so more glow never
    // fattens the core — it only spreads the halo.
    let rc = GLOW_CORE_FRAC / reach;
    let core = exp(-(d * d) / (rc * rc));

    // Halo: a fainter glow that drops off quickly but reaches all the way to the
    // (grown) quad edge, fading to zero there so there is no square cutoff.
    let halo = pow(clamp(1.0 - d, 0.0, 1.0), GLOW_HALO_POW);

    let intensity = core + GLOW_HALO_AMP * halo;

    // Additive blending sums these into the framebuffer; dense regions build into a
    // bright core, and the bloom in post picks up the brightest centres.
    let rgb = in.color * intensity * GLOW_GAIN;
    return vec4<f32>(rgb, intensity);
}

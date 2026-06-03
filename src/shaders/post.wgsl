// Post-processing: bright-pass + separable Gaussian blur + tonemapped composite.
// All passes draw a single fullscreen triangle. The scene is rendered to an HDR
// (rgba16float) target; bloom is computed at reduced resolution and added back.

const THRESHOLD: f32 = 0.8;   // brightness above which a pixel blooms
const BLOOM_STRENGTH: f32 = 0.35;
const EXPOSURE: f32 = 1.3;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// group 0: the input being sampled (scene or a bloom buffer) + a linear sampler.
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
// group 1: the blurred bloom buffer (only used by the composite pass).
@group(1) @binding(0) var bloom_src: texture_2d<f32>;

@vertex
fn fs_vert(@builtin(vertex_index) vi: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let p = positions[vi];
    var out: VsOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x * 0.5 + 0.5, 1.0 - (p.y * 0.5 + 0.5));
    return out;
}

// Downsample the HDR scene with a 4-tap box (reduces aliasing of thin bright
// features) and keep only the part above the threshold.
@fragment
fn bright_pass(in: VsOut) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(src));
    var c = textureSample(src, samp, in.uv + texel * vec2<f32>(-0.5, -0.5)).rgb;
    c += textureSample(src, samp, in.uv + texel * vec2<f32>(0.5, -0.5)).rgb;
    c += textureSample(src, samp, in.uv + texel * vec2<f32>(-0.5, 0.5)).rgb;
    c += textureSample(src, samp, in.uv + texel * vec2<f32>(0.5, 0.5)).rgb;
    c *= 0.25;
    let bright = max(c - vec3<f32>(THRESHOLD), vec3<f32>(0.0));
    return vec4<f32>(bright, 1.0);
}

// 9-tap Gaussian along `dir` (one texel step per sample), reading `src`.
fn gaussian(uv: vec2<f32>, dir: vec2<f32>) -> vec3<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(src));
    var weights = array<f32, 5>(0.227027, 0.1945946, 0.1216216, 0.054054, 0.016216);
    var result = textureSample(src, samp, uv).rgb * weights[0];
    for (var i = 1; i < 5; i = i + 1) {
        let offset = dir * texel * f32(i);
        result += textureSample(src, samp, uv + offset).rgb * weights[i];
        result += textureSample(src, samp, uv - offset).rgb * weights[i];
    }
    return result;
}

@fragment
fn blur_h(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(gaussian(in.uv, vec2<f32>(1.0, 0.0)), 1.0);
}

@fragment
fn blur_v(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(gaussian(in.uv, vec2<f32>(0.0, 1.0)), 1.0);
}

// Add bloom to the HDR scene, tonemap to LDR, and output to the swapchain.
@fragment
fn composite(in: VsOut) -> @location(0) vec4<f32> {
    let scene = textureSample(src, samp, in.uv).rgb;
    let bloom = textureSample(bloom_src, samp, in.uv).rgb;
    let hdr = scene + bloom * BLOOM_STRENGTH;
    let mapped = vec3<f32>(1.0) - exp(-hdr * EXPOSURE);
    return vec4<f32>(mapped, 1.0);
}

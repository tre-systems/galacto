use crate::scenarios::{Scenario, DEFAULT_TEMP};
use crate::utils::console_log;
use bytemuck::{Pod, Zeroable};
use std::cell::RefCell;
use std::rc::Rc;
use wgpu::util::DeviceExt;

/// Default body count, and the reference total against which per-body mass is
/// scaled in `scenarios.rs` (so changing the count refines the same galaxy rather
/// than changing its total mass). Self-gravity is all-pairs (O(N²)), so this is
/// far smaller than a test-particle sim would allow. Must be a multiple of
/// `WORKGROUP_SIZE` so the tiled gravity kernel never reads out of bounds.
pub(crate) const NUM_PARTICLES: u32 = 16384;
/// Upper bound for the body-count slider (10× the default). The GPU buffers are
/// allocated at this size once, so the count can grow live without reallocating
/// them or the bind groups — only the active `count` is ever dispatched or drawn.
pub(crate) const MAX_PARTICLES: u32 = NUM_PARTICLES * 10;
/// Compute workgroup size, equal to `TILE` in `update.wgsl`. The active body count
/// must stay a multiple of this so the tiled gravity kernel tiles evenly.
const WORKGROUP_SIZE: u32 = 256;
/// Workgroups at the maximum body count — sizes the core-statistics buffers.
const MAX_WORKGROUPS: u32 = MAX_PARTICLES / WORKGROUP_SIZE;
/// Most bytes a core-statistics readback can produce: one `vec4<f32>` per
/// workgroup at the maximum body count.
const MAX_REDUCTION_BYTES: u64 = MAX_WORKGROUPS as u64 * 16;

/// Round a requested body count to a value the solver accepts: a multiple of the
/// tile size, at least one tile and at most [`MAX_PARTICLES`]. The body-count
/// slider passes already-stepped values; this is the authoritative guard.
pub(crate) fn clamp_particle_count(requested: u32) -> u32 {
    // Bound first so the round-to-nearest add can't overflow on a huge request.
    let requested = requested.min(MAX_PARTICLES);
    let tiles = ((requested + WORKGROUP_SIZE / 2) / WORKGROUP_SIZE).max(1);
    (tiles * WORKGROUP_SIZE).min(MAX_PARTICLES)
}

/// Fixed simulation timestep (seconds). Physics advances in whole steps of this
/// size regardless of display refresh rate; see the accumulator in `lib.rs`.
pub const FIXED_DT: f32 = 1.0 / 60.0;

/// Gravitational constant for the sim's arbitrary unit system. Used both in the
/// params uniform and by the scenario seeding (`scenarios.rs`) for disk balance.
pub(crate) const G: f32 = 1.0;

/// Static dark-matter halo centred at the origin. `HALO_V0` is its characteristic
/// circular speed — the logarithmic halo's asymptote, or the NFW halo's peak. It
/// anchors each disk's rotation curve and, for the logarithmic profile, confines
/// the system. Set `HALO_V0 = 0` to disable. Fed to the kernel via the params
/// uniform and used by the scenario seeding to set each disk's circular velocity.
pub(crate) const HALO_V0: f32 = 75.0;
/// Logarithmic-halo core radius (the rotation curve flattens beyond it).
pub(crate) const HALO_RC: f32 = 150.0;
/// NFW scale radius — deliberately smaller than `HALO_RC` so the NFW peak (at
/// `r ≈ 2.16·rs`) and its subsequent decline fall inside the disk / merger field
/// of view, and so the outer potential is shallow enough that fast merger debris
/// escapes (the visible contrast with the confining logarithmic halo).
pub(crate) const NFW_RS: f32 = 70.0;

/// The NFW circular-velocity shape `[ln(1+x) − x/(1+x)] / x` peaks at this value
/// (near `x = r/rs ≈ 2.16`). Dividing by it normalises the NFW halo so `HALO_V0`
/// is its *peak* circular speed, directly comparable to the logarithmic halo's
/// asymptote. Mirrored as a literal in `update.wgsl`'s NFW branch.
pub(crate) const NFW_G_MAX: f32 = 0.2162;

/// Dark-matter halo radial profile. Both are static background forces; they differ
/// in the rotation curve they impose. Selected live from the page's halo dropdown.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HaloKind {
    /// Logarithmic potential: a flat outer rotation curve, and a potential that
    /// grows without bound, so the system stays bound — debris always orbits back.
    Logarithmic,
    /// Navarro–Frenk–White, the cold-dark-matter profile: a rotation curve that
    /// rises to a peak then declines. Its potential is finite, so fast debris can
    /// escape — tidal tails fly off instead of returning.
    Nfw,
}

impl HaloKind {
    /// Map the dropdown's value (0 = logarithmic, 1 = NFW; default logarithmic).
    pub fn from_id(id: u32) -> Self {
        match id {
            1 => HaloKind::Nfw,
            _ => HaloKind::Logarithmic,
        }
    }

    /// The discriminant written into the params uniform for the shader's branch.
    fn as_u32(self) -> u32 {
        match self {
            HaloKind::Logarithmic => 0,
            HaloKind::Nfw => 1,
        }
    }
}

/// Default billboard half-extent for each particle, in NDC.y (screen-constant, so
/// it is independent of zoom and depth). The star-size slider overrides it live.
pub(crate) const DEFAULT_PARTICLE_SIZE: f32 = 0.016;

/// One body. `pos_mass` packs position in xyz and mass in w; `vel` packs velocity
/// in xyz and a 0..1 colour tint in w (read by the render shader). Packing as two
/// vec4s (not vec3) sidesteps WGSL's 16-byte vec3 stride, keeping the layout
/// unambiguous.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Particle {
    pub pos_mass: [f32; 4],
    pub vel: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SimulationParams {
    pub dt: f32,
    pub g: f32,         // gravitational constant
    pub softening: f32, // Plummer softening length
    pub particle_count: u32,
    pub halo_v0_sq: f32, // dark-matter halo: squared characteristic circular speed
    pub halo_rc2: f32,   // dark-matter halo: squared core / scale radius
    pub halo_kind: u32,  // dark-matter halo profile: 0 = logarithmic, 1 = NFW
    pub has_gas: u32,    // 1 if this scenario has a dissipative gas population
}

/// Uniform for the dark-matter halo overlay shader (`halo.wgsl`). `right`/`up` are
/// the camera's world axes (for billboarding); `color` packs the halo colour in
/// rgb and the on/off intensity in a; `radius` is the quad's world half-extent.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct HaloVizParams {
    pub right: [f32; 4],
    pub up: [f32; 4],
    pub color: [f32; 4],
    pub radius: f32,
    pub _pad: [f32; 3],
}

/// Aggregate core statistics read back from the GPU on a throttled, async cadence
/// to drive the audio (`src/audio.rs`) — never consumed by the simulation itself.
/// `mass` is window-weighted mass near the origin (how much sits at the centre);
/// `flux` is window-weighted mass × signed radial velocity (+ outward, so the sign
/// is matter moving out of vs into the centre); `activity` uses |radial velocity|
/// (core churn). The CPU sums the per-workgroup partials in [`Simulation::map_core_readback`].
#[derive(Copy, Clone, Debug, Default)]
pub struct CoreStats {
    pub mass: f32,
    pub flux: f32,
    pub activity: f32,
}

/// Async-readback bookkeeping. `in_flight` is true while the staging buffer is
/// mapped, so we never copy into it or re-map until the previous read completes —
/// which naturally throttles readbacks to roughly the GPU round-trip rate.
#[derive(Default)]
struct ReduceState {
    in_flight: bool,
    stats: CoreStats,
}

/// Inputs to [`Simulation::reseed`]: what to seed (scenario, disk temperature, body
/// count) plus the live physics the fresh bodies must be balanced against (gravity
/// and halo). Bundled so the call site reads as named fields rather than a long
/// positional argument list.
pub struct Reseed {
    pub scenario: Scenario,
    pub temp: f32,
    pub gas_fraction: f32,
    pub bulge_frac: f32,
    pub count: u32,
    pub gravity: f32,
    pub halo_v0: f32,
    pub halo_rc_scale: f32,
    pub halo_kind: HaloKind,
    /// Composed-piece length (s) — places the `Flyby` intruder for a mid-piece hit.
    pub duration_secs: f32,
}

pub struct Simulation {
    /// Active body count (a multiple of `WORKGROUP_SIZE`, ≤ `MAX_PARTICLES`). The
    /// buffers are sized for the maximum; only these many bodies are dispatched,
    /// drawn, and read back. Set by `reseed` from the body-count slider.
    count: u32,
    /// Whether the current scenario has a dissipative gas population (the disk
    /// scenarios); carried into the params uniform so the kick kernel cools the gas.
    has_gas: bool,
    // Written at init and re-uploaded on reseed (temperature changes).
    particle_buffer: wgpu::Buffer,
    #[expect(dead_code)] // held only to keep the GPU resource alive
    accel_buffer: wgpu::Buffer,
    // Written at init and re-uploaded on reseed (scenario softening changes).
    params_buffer: wgpu::Buffer,
    accel_pipeline: wgpu::ComputePipeline,
    // Leapfrog integration is split into two position passes around gravity.
    drift_pipeline: wgpu::ComputePipeline,
    kick_pipeline: wgpu::ComputePipeline,
    render_pipeline: wgpu::RenderPipeline,
    compute_bind_group: wgpu::BindGroup,
    render_bind_group: wgpu::BindGroup,
    camera_buffer: wgpu::Buffer,
    // Optional dark-matter halo overlay (the "Show" toggle): a single billboard
    // glow at the origin, sized to the active halo's scale radius.
    halo_pipeline: wgpu::RenderPipeline,
    halo_viz_buffer: wgpu::Buffer,
    halo_bind_group: wgpu::BindGroup,
    // Core-statistics reduction for the audio: a compute pass writes per-workgroup
    // partials, copied to a mappable staging buffer and read back asynchronously.
    reduce_pipeline: wgpu::ComputePipeline,
    reductions_buffer: wgpu::Buffer,
    reduction_staging: wgpu::Buffer,
    reduce_state: Rc<RefCell<ReduceState>>,
}

struct GpuBuffers {
    particle: wgpu::Buffer,
    accel: wgpu::Buffer,
    params: wgpu::Buffer,
    camera: wgpu::Buffer,
    reductions: wgpu::Buffer,
    reduction_staging: wgpu::Buffer,
}

struct ShaderModules {
    compute: wgpu::ShaderModule,
    render: wgpu::ShaderModule,
}

struct BindGroupLayouts {
    compute: wgpu::BindGroupLayout,
    render: wgpu::BindGroupLayout,
}

struct ComputePipelines {
    accel: wgpu::ComputePipeline,
    drift: wgpu::ComputePipeline,
    kick: wgpu::ComputePipeline,
    reduce: wgpu::ComputePipeline,
}

struct BindGroups {
    compute: wgpu::BindGroup,
    render: wgpu::BindGroup,
}

struct HaloRenderResources {
    pipeline: wgpu::RenderPipeline,
    viz_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl Simulation {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        console_log!("Creating simulation...");

        // The tiled gravity kernel reads bodies in whole WORKGROUP_SIZE-sized
        // tiles with no tail guard, so every count must divide evenly.
        debug_assert_eq!(NUM_PARTICLES % WORKGROUP_SIZE, 0);
        debug_assert_eq!(MAX_PARTICLES % WORKGROUP_SIZE, 0);

        // Start in the grand-design (M51) flyby at the default temperature and body
        // count, balanced against the default (logarithmic) halo — the first thing a
        // visitor sees, so it leads with the most dynamic scenario.
        let scenario = Scenario::GrandDesign;
        let buffers = Self::create_buffers(device, scenario);
        let shaders = Self::create_shaders(device);
        let layouts = Self::create_bind_group_layouts(device);
        let compute_pipelines =
            Self::create_compute_pipelines(device, &shaders.compute, &layouts.compute);
        let render_pipeline =
            Self::create_render_pipeline(device, color_format, &shaders.render, &layouts.render);
        let bind_groups = Self::create_bind_groups(device, &layouts, &buffers);
        let halo = Self::create_halo_resources(device, color_format, &buffers.camera);

        console_log!(
            "✨ Self-gravitating galaxy ({} bodies) initialized!",
            NUM_PARTICLES
        );
        console_log!("🌌 Pick a scenario and tweak the sliders.");

        Self {
            count: NUM_PARTICLES,
            has_gas: scenario.has_gas(),
            particle_buffer: buffers.particle,
            accel_buffer: buffers.accel,
            params_buffer: buffers.params,
            accel_pipeline: compute_pipelines.accel,
            drift_pipeline: compute_pipelines.drift,
            kick_pipeline: compute_pipelines.kick,
            render_pipeline,
            compute_bind_group: bind_groups.compute,
            render_bind_group: bind_groups.render,
            camera_buffer: buffers.camera,
            halo_pipeline: halo.pipeline,
            halo_viz_buffer: halo.viz_buffer,
            halo_bind_group: halo.bind_group,
            reduce_pipeline: compute_pipelines.reduce,
            reductions_buffer: buffers.reductions,
            reduction_staging: buffers.reduction_staging,
            reduce_state: Rc::new(RefCell::new(ReduceState::default())),
        }
    }

    fn create_buffers(device: &wgpu::Device, scenario: Scenario) -> GpuBuffers {
        let particles = scenario.generate(NUM_PARTICLES, DEFAULT_TEMP, HaloKind::Logarithmic);

        // The particle buffer is sized for MAX_PARTICLES (zero-padded past the active
        // count) so the body-count slider can grow the sim without reallocating it.
        let mut initial = vec![Particle::zeroed(); MAX_PARTICLES as usize];
        initial[..particles.len()].copy_from_slice(&particles);
        let particle = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Particle Buffer"),
            contents: bytemuck::cast_slice(&initial),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Scratch accel buffer, rewritten every step; no initial contents needed.
        let accel = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Accel Buffer"),
            size: (MAX_PARTICLES as u64) * 16, // vec4<f32> per body
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let params = Self::build_params(
            scenario.softening(),
            G,
            HALO_V0,
            1.0,
            HaloKind::Logarithmic,
            NUM_PARTICLES,
            scenario.has_gas(),
        );
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Params Buffer"),
            contents: bytemuck::cast_slice(&[params]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Camera Buffer"),
            size: 80, // mat4 (64) + vec4 params (size, aspect, color_mode, glow)
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Core-statistics reduction output (one vec4 per workgroup) plus a mappable
        // staging copy for the async readback that feeds the audio. Sized for the
        // maximum body count; only the active prefix is copied and read each frame.
        let reductions = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Core Reduction Buffer"),
            size: MAX_REDUCTION_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let reduction_staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Core Reduction Staging"),
            size: MAX_REDUCTION_BYTES,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        GpuBuffers {
            particle,
            accel,
            params,
            camera,
            reductions,
            reduction_staging,
        }
    }

    fn create_shaders(device: &wgpu::Device) -> ShaderModules {
        // Compute shader holds the drift, gravity, kick, and audio-reduction kernels.
        let compute = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Compute Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/update.wgsl").into()),
        });
        let render = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Render Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/render.wgsl").into()),
        });
        ShaderModules { compute, render }
    }

    fn create_bind_group_layouts(device: &wgpu::Device) -> BindGroupLayouts {
        // Compute bind group: particles (rw), params (uniform), accel (rw). All
        // compute kernels share this one layout and bind group.
        let compute = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Compute Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Binding 3: core-statistics partials, written only by `reduce_core`;
                // the simulation kernels ignore it.
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let render = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Render Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    // The camera uniform is read in both stages: the vertex shader
                    // for transform/size, the fragment shader for glow falloff.
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        BindGroupLayouts { compute, render }
    }

    fn create_compute_pipelines(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        layout: &wgpu::BindGroupLayout,
    ) -> ComputePipelines {
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Compute Pipeline Layout"),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });

        ComputePipelines {
            // All-pairs gravity: reads positions, writes accelerations.
            accel: Self::create_compute_pipeline(
                device,
                &pipeline_layout,
                shader,
                "Accel Pipeline",
                "compute_accel",
            ),
            // Leapfrog half-drift: advance positions to the step midpoint.
            drift: Self::create_compute_pipeline(
                device,
                &pipeline_layout,
                shader,
                "Drift Pipeline",
                "drift_half",
            ),
            // Leapfrog kick + half-drift: apply the midpoint acceleration kick, then
            // drift the second half-step.
            kick: Self::create_compute_pipeline(
                device,
                &pipeline_layout,
                shader,
                "Kick Pipeline",
                "kick_drift_half",
            ),
            // Core-statistics reduction: writes per-workgroup partials for audio.
            reduce: Self::create_compute_pipeline(
                device,
                &pipeline_layout,
                shader,
                "Core Reduce Pipeline",
                "reduce_core",
            ),
        }
    }

    fn create_compute_pipeline(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        label: &'static str,
        entry_point: &'static str,
    ) -> wgpu::ComputePipeline {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            module: shader,
            entry_point: Some(entry_point),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        })
    }

    fn create_render_pipeline(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        shader: &wgpu::ShaderModule,
        bind_group_layout: &wgpu::BindGroupLayout,
    ) -> wgpu::RenderPipeline {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Render Pipeline Layout"),
            bind_group_layouts: &[Some(bind_group_layout)],
            immediate_size: 0,
        });
        Self::create_additive_billboard_pipeline(
            device,
            "Render Pipeline",
            &layout,
            shader,
            color_format,
        )
    }

    fn create_bind_groups(
        device: &wgpu::Device,
        layouts: &BindGroupLayouts,
        buffers: &GpuBuffers,
    ) -> BindGroups {
        let compute = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Compute Bind Group"),
            layout: &layouts.compute,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffers.particle.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buffers.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buffers.accel.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: buffers.reductions.as_entire_binding(),
                },
            ],
        });

        let render = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Render Bind Group"),
            layout: &layouts.render,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffers.camera.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buffers.particle.as_entire_binding(),
                },
            ],
        });

        BindGroups { compute, render }
    }

    fn create_halo_resources(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        camera_buffer: &wgpu::Buffer,
    ) -> HaloRenderResources {
        // Dark-matter halo overlay: camera uniform + a small halo uniform, one
        // additive billboard quad. Shares the HDR target and blend with the
        // particles; drawn only when the page enables it.
        let viz_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Halo Viz Buffer"),
            size: std::mem::size_of::<HaloVizParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Halo Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/halo.wgsl").into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Halo Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Halo Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = Self::create_additive_billboard_pipeline(
            device,
            "Halo Pipeline",
            &pipeline_layout,
            &shader,
            color_format,
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Halo Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: viz_buffer.as_entire_binding(),
                },
            ],
        });

        HaloRenderResources {
            pipeline,
            viz_buffer,
            bind_group,
        }
    }

    fn create_additive_billboard_pipeline(
        device: &wgpu::Device,
        label: &'static str,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        color_format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        let targets = [Some(Self::additive_color_target(color_format))];
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_main"),
                targets: &targets,
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            cache: None,
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            // No depth buffer: additive glow is order-independent and there is no
            // opaque geometry to occlude against.
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview_mask: None,
        })
    }

    fn additive_color_target(format: wgpu::TextureFormat) -> wgpu::ColorTargetState {
        wgpu::ColorTargetState {
            format,
            // Additive: overlapping glowing particles or halo pixels accumulate
            // brightness (order-independent, correct for points on black).
            blend: Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            write_mask: wgpu::ColorWrites::ALL,
        }
    }

    /// The `SimulationParams` uniform. `softening` varies per scenario; `gravity`
    /// and `halo_v0` are live knobs driven by their sliders; `halo_kind` selects
    /// the halo profile (and is mirrored by the spiral seeding so the disk stays in
    /// equilibrium).
    fn build_params(
        softening: f32,
        gravity: f32,
        halo_v0: f32,
        halo_rc_scale: f32,
        halo_kind: HaloKind,
        count: u32,
        has_gas: bool,
    ) -> SimulationParams {
        // The shader reads halo_rc2 as the active profile's squared radius (the log
        // core radius or the smaller NFW scale radius), scaled by the live
        // concentration knob (<1 = more concentrated, >1 = more diffuse).
        let base_rc = match halo_kind {
            HaloKind::Logarithmic => HALO_RC,
            HaloKind::Nfw => NFW_RS,
        };
        let rc = base_rc * halo_rc_scale;
        SimulationParams {
            dt: FIXED_DT,
            g: gravity,
            softening,
            particle_count: count,
            halo_v0_sq: halo_v0 * halo_v0,
            halo_rc2: rc * rc,
            halo_kind: halo_kind.as_u32(),
            has_gas: has_gas as u32,
        }
    }

    /// Regenerate from fresh initial conditions and upload them, restarting the
    /// galaxy. Also rewrites the params uniform (scenarios use different softening;
    /// gravity/halo carry the current live values).
    pub fn reseed(&mut self, queue: &wgpu::Queue, r: Reseed) {
        // Adopt the new body count (drives dispatch, draw, and readback extents),
        // then regenerate. The spiral disk balances its circular velocities against
        // the active halo, so seeding takes `halo_kind` too — born in equilibrium.
        self.count = clamp_particle_count(r.count);
        self.has_gas = r.scenario.has_gas();
        let particles = r.scenario.generate_with(
            self.count,
            r.temp,
            r.gas_fraction,
            r.bulge_frac,
            r.halo_kind,
            r.duration_secs,
        );
        queue.write_buffer(&self.particle_buffer, 0, bytemuck::cast_slice(&particles));
        self.set_physics(
            queue,
            r.scenario.softening(),
            r.gravity,
            r.halo_v0,
            r.halo_rc_scale,
            r.halo_kind,
        );
    }

    /// Rewrite only the params uniform (live gravity / halo changes), leaving the
    /// running bodies and the active count untouched.
    pub fn set_physics(
        &self,
        queue: &wgpu::Queue,
        softening: f32,
        gravity: f32,
        halo_v0: f32,
        halo_rc_scale: f32,
        halo_kind: HaloKind,
    ) {
        let params = Self::build_params(
            softening,
            gravity,
            halo_v0,
            halo_rc_scale,
            halo_kind,
            self.count,
            self.has_gas,
        );
        queue.write_buffer(&self.params_buffer, 0, bytemuck::cast_slice(&[params]));
    }

    /// Record one leapfrog (drift–kick–drift) step: half-drift, recompute the
    /// all-pairs accelerations at the midpoint, then kick + the second half-drift.
    /// Each kernel is its own compute pass so the GPU orders every pass's reads
    /// after the previous pass's writes — in particular the gravity sum never reads
    /// a body's position while another pass is moving it.
    pub fn compute_pass(&self, encoder: &mut wgpu::CommandEncoder) {
        self.dispatch(encoder, &self.drift_pipeline, "Drift Pass");
        self.dispatch(encoder, &self.accel_pipeline, "Accel Pass");
        self.dispatch(encoder, &self.kick_pipeline, "Kick Pass");
    }

    /// Record the core-statistics reduction for the audio: a compute pass that
    /// writes per-workgroup partials, then a copy into the mappable staging buffer.
    /// Skipped (returns `false`) while a previous readback is still mapped, which
    /// throttles it to roughly the GPU round-trip rate. It runs after simulation
    /// writes and reads bodies only, so there is no particle-write hazard. Pair
    /// with [`map_core_readback`] after submit.
    pub fn record_core_reduction(&self, encoder: &mut wgpu::CommandEncoder) -> bool {
        if self.reduce_state.borrow().in_flight {
            return false;
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Core Reduce Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.reduce_pipeline);
            pass.set_bind_group(0, &self.compute_bind_group, &[]);
            pass.dispatch_workgroups(self.count / WORKGROUP_SIZE, 1, 1);
        }
        encoder.copy_buffer_to_buffer(
            &self.reductions_buffer,
            0,
            &self.reduction_staging,
            0,
            self.reduction_bytes(),
        );
        true
    }

    /// Bytes the core-statistics reduction produces at the current body count: one
    /// `vec4<f32>` per workgroup. Only this active prefix of the staging buffer is
    /// copied and read each frame.
    fn reduction_bytes(&self) -> u64 {
        (self.count / WORKGROUP_SIZE) as u64 * 16
    }

    /// Kick off the async map of the staging buffer (call after the queue submit
    /// that ran [`record_core_reduction`]). The map callback sums the per-workgroup
    /// partials into `CoreStats` and clears the in-flight flag; in the browser the
    /// runtime drives the map to completion, so no manual device poll is needed.
    ///
    /// Only the wasm target maps: the callback captures an `Rc` (not `Send`), which
    /// the browser's single-threaded `map_async` allows but the native build's
    /// `Send` bound forbids. Native is test-only and never runs this GPU path, so
    /// it is a no-op there.
    #[cfg(target_arch = "wasm32")]
    pub fn map_core_readback(&self) {
        let staging = self.reduction_staging.clone();
        let state = self.reduce_state.clone();
        // Map only the active prefix — the count may have shrunk, leaving stale
        // data in the tail that must not be summed.
        let bytes = self.reduction_bytes();
        self.reduce_state.borrow_mut().in_flight = true;
        self.reduction_staging
            .slice(0..bytes)
            .map_async(wgpu::MapMode::Read, move |res| {
                let mut st = state.borrow_mut();
                if res.is_ok() {
                    let (mut mass, mut flux, mut activity) = (0.0_f32, 0.0_f32, 0.0_f32);
                    {
                        let data = staging.slice(0..bytes).get_mapped_range();
                        for p in bytemuck::cast_slice::<u8, f32>(&data[..]).chunks_exact(4) {
                            mass += p[0];
                            flux += p[1];
                            activity += p[2];
                        }
                    }
                    staging.unmap();
                    st.stats = CoreStats {
                        mass,
                        flux,
                        activity,
                    };
                }
                st.in_flight = false;
            });
    }

    /// No-op on non-wasm (native test) builds — see the wasm variant above.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn map_core_readback(&self) {}

    /// The most recent core statistics read back from the GPU (see [`CoreStats`]).
    pub fn core_stats(&self) -> CoreStats {
        self.reduce_state.borrow().stats
    }

    /// Run one compute kernel over every body, one workgroup per tile.
    fn dispatch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        label: &str,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(label),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.compute_bind_group, &[]);
        pass.dispatch_workgroups(self.count / WORKGROUP_SIZE, 1, 1);
    }

    pub fn render_pass<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_bind_group(0, &self.render_bind_group, &[]);
        // One triangle-strip quad (4 verts) per particle, instanced.
        render_pass.draw(0..4, 0..self.count);
    }

    /// Update the halo-overlay uniform with the camera's billboard basis and the
    /// active halo's colour/size. Cheap (a 64-byte write); call each frame the
    /// overlay is shown.
    pub fn update_halo_view(
        &self,
        queue: &wgpu::Queue,
        right: [f32; 3],
        up: [f32; 3],
        radius: f32,
        color: [f32; 3],
        intensity: f32,
    ) {
        let params = HaloVizParams {
            right: [right[0], right[1], right[2], 0.0],
            up: [up[0], up[1], up[2], 0.0],
            color: [color[0], color[1], color[2], intensity],
            radius,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.halo_viz_buffer, 0, bytemuck::cast_slice(&[params]));
    }

    /// Draw the dark-matter halo overlay — one additive billboard at the origin.
    /// Call inside the particle render pass (before `render_pass`, so the stars
    /// draw over it) when the overlay is enabled.
    pub fn render_halo<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        render_pass.set_pipeline(&self.halo_pipeline);
        render_pass.set_bind_group(0, &self.halo_bind_group, &[]);
        render_pass.draw(0..4, 0..1);
    }

    pub fn update_camera(
        &self,
        queue: &wgpu::Queue,
        camera: &crate::camera::Camera,
        scenario: Scenario,
        particle_size: f32,
        glow: f32,
    ) {
        let matrix = camera.build_view_projection_matrix();
        let matrix_array: &[f32; 16] = matrix.as_ref();
        let color_mode = match scenario {
            // The spiral and the M51 flyby colour by live galactocentric radius
            // (warm core → cool arms); the multi-galaxy collisions colour by vel.w
            // (galaxy of origin).
            Scenario::Spiral | Scenario::GrandDesign | Scenario::Flyby => 0.0,
            _ => 1.0,
        };
        // mat4 (16 floats) then the vec4 of params: billboard size, aspect, colour
        // mode (0 = radius tint for the spiral, 1 = vel.w tint for the merger), and
        // the glow halo control.
        let mut data = [0f32; 20];
        data[..16].copy_from_slice(matrix_array);
        data[16..].copy_from_slice(&[particle_size, camera.aspect_ratio, color_mode, glow]);
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&data));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_layout_matches_wgsl() {
        // Particle and SimulationParams are mirrored byte-for-byte by the structs
        // in update.wgsl. vec4 packing keeps the layout unambiguous; a drift here
        // is the classic stride bug (a vec3 field pads to 16 bytes WGSL-side).
        assert_eq!(std::mem::size_of::<Particle>(), 32);
        assert_eq!(std::mem::size_of::<SimulationParams>(), 32);
    }

    #[test]
    fn particle_count_is_a_tile_multiple() {
        // The tiled gravity kernel reads whole WORKGROUP_SIZE tiles, no tail guard,
        // so the default, the maximum, and any clamped count must tile evenly.
        assert_eq!(NUM_PARTICLES % WORKGROUP_SIZE, 0);
        assert_eq!(MAX_PARTICLES % WORKGROUP_SIZE, 0);
        assert_eq!(MAX_PARTICLES, NUM_PARTICLES * 10);
    }

    #[test]
    fn clamp_particle_count_tiles_and_bounds() {
        // Always a tile multiple, never below one tile, never above the maximum.
        for req in [0, 1, 100, 200, 16_384, 100_000, MAX_PARTICLES, u32::MAX] {
            let c = clamp_particle_count(req);
            assert_eq!(c % WORKGROUP_SIZE, 0, "clamp({req}) = {c} must tile evenly");
            assert!((WORKGROUP_SIZE..=MAX_PARTICLES).contains(&c));
        }
        assert_eq!(clamp_particle_count(0), WORKGROUP_SIZE);
        assert_eq!(clamp_particle_count(NUM_PARTICLES), NUM_PARTICLES);
        assert_eq!(clamp_particle_count(u32::MAX), MAX_PARTICLES);
    }
}

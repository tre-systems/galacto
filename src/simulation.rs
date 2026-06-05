use crate::scenarios::{Scenario, DEFAULT_TEMP};
use crate::utils::console_log;
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Total bodies in the simulation. Self-gravity is all-pairs (O(N²)), so this is
/// far smaller than a test-particle sim would allow. Must be a multiple of
/// `WORKGROUP_SIZE` so the tiled gravity kernel never reads out of bounds.
pub(crate) const NUM_PARTICLES: u32 = 16384;
/// Compute workgroup size, equal to `TILE` in `update.wgsl`.
const WORKGROUP_SIZE: u32 = 256;

/// Fixed simulation timestep (seconds). Physics advances in whole steps of this
/// size regardless of display refresh rate; see the accumulator in `lib.rs`.
pub const FIXED_DT: f32 = 1.0 / 60.0;

/// Gravitational constant for the sim's arbitrary unit system. Used both in the
/// params uniform and by the scenario seeding (`scenarios.rs`) for disk balance.
pub(crate) const G: f32 = 1.0;

/// Static dark-matter halo centred at the origin. `HALO_V0` is its characteristic
/// circular speed (the logarithmic halo's asymptote, or the NFW halo's peak) and
/// `HALO_RC` its core / scale radius. It anchors each disk's rotation curve and,
/// for the logarithmic profile, confines the system. Set `HALO_V0 = 0` to disable.
/// Fed to the kernel via the params uniform and used by the scenario seeding to
/// set each disk's circular velocity.
pub(crate) const HALO_V0: f32 = 75.0;
pub(crate) const HALO_RC: f32 = 150.0;

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
    pub _pad1: u32,
}

pub struct Simulation {
    // Written at init and re-uploaded on reseed (temperature changes).
    particle_buffer: wgpu::Buffer,
    #[expect(dead_code)] // held only to keep the GPU resource alive
    accel_buffer: wgpu::Buffer,
    // Written at init and re-uploaded on reseed (scenario softening changes).
    params_buffer: wgpu::Buffer,
    accel_pipeline: wgpu::ComputePipeline,
    // Leapfrog is two integrate kernels: a half-drift before gravity, then the
    // kick + second half-drift after it (`compute_pass`).
    drift_pipeline: wgpu::ComputePipeline,
    kick_pipeline: wgpu::ComputePipeline,
    render_pipeline: wgpu::RenderPipeline,
    compute_bind_group: wgpu::BindGroup,
    render_bind_group: wgpu::BindGroup,
    camera_buffer: wgpu::Buffer,
}

impl Simulation {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        console_log!("Creating simulation...");

        // The tiled gravity kernel reads bodies in whole WORKGROUP_SIZE-sized
        // tiles with no tail guard, so the count must divide evenly.
        debug_assert_eq!(NUM_PARTICLES % WORKGROUP_SIZE, 0);

        // Start in the spiral-disk scenario at the default temperature, balanced
        // against the default (logarithmic) halo.
        let scenario = Scenario::Spiral;
        let particles = scenario.generate(DEFAULT_TEMP, HaloKind::Logarithmic);

        let particle_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Particle Buffer"),
            contents: bytemuck::cast_slice(&particles),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Scratch accel buffer, rewritten every step; no initial contents needed.
        let accel_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Accel Buffer"),
            size: (NUM_PARTICLES as u64) * 16, // vec4<f32> per body
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let params = Self::build_params(scenario.softening(), G, HALO_V0, HaloKind::Logarithmic);

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Params Buffer"),
            contents: bytemuck::cast_slice(&[params]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Camera Buffer"),
            size: 80, // mat4 (64) + vec4 params (size, aspect, color_mode, pad)
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Compute shader holds both the accel and integrate kernels.
        let compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Compute Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/update.wgsl").into()),
        });

        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Render Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/render.wgsl").into()),
        });

        // Compute bind group: particles (rw), params (uniform), accel (rw). Both
        // kernels share this one layout and bind group.
        let compute_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
                ],
            });

        let render_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Render Bind Group Layout"),
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

        let compute_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Compute Pipeline Layout"),
                bind_group_layouts: &[Some(&compute_bind_group_layout)],
                immediate_size: 0,
            });

        // All-pairs gravity: reads positions, writes accelerations.
        let accel_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Accel Pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: Some("compute_accel"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        // Leapfrog half-drift (part 1): advance positions to the step midpoint.
        let drift_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Drift Pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: Some("drift_half"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        // Leapfrog kick + half-drift (part 2): apply the velocity kick from the
        // midpoint accelerations, then drift the second half-step.
        let kick_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Kick Pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: Some("kick_drift_half"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Render Pipeline Layout"),
                bind_group_layouts: &[Some(&render_bind_group_layout)],
                immediate_size: 0,
            });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &render_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &render_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    // Additive: overlapping glowing particles accumulate brightness
                    // (order-independent, correct for points on black).
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
                })],
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
        });

        let compute_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Compute Bind Group"),
            layout: &compute_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: particle_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: accel_buffer.as_entire_binding(),
                },
            ],
        });

        let render_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Render Bind Group"),
            layout: &render_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: particle_buffer.as_entire_binding(),
                },
            ],
        });

        console_log!(
            "✨ Self-gravitating galaxy ({} bodies) initialized!",
            NUM_PARTICLES
        );
        console_log!("🌌 Pick a scenario (spiral disk or merger) and tweak the sliders.");

        Self {
            particle_buffer,
            accel_buffer,
            params_buffer,
            accel_pipeline,
            drift_pipeline,
            kick_pipeline,
            render_pipeline,
            compute_bind_group,
            render_bind_group,
            camera_buffer,
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
        halo_kind: HaloKind,
    ) -> SimulationParams {
        SimulationParams {
            dt: FIXED_DT,
            g: gravity,
            softening,
            particle_count: NUM_PARTICLES,
            halo_v0_sq: halo_v0 * halo_v0,
            halo_rc2: HALO_RC * HALO_RC,
            halo_kind: halo_kind.as_u32(),
            _pad1: 0,
        }
    }

    /// Regenerate from fresh initial conditions for `scenario` at `temp` and upload
    /// them, restarting the galaxy. Also rewrites the params uniform (scenarios use
    /// different softening; gravity/halo carry the current live values).
    pub fn reseed(
        &self,
        queue: &wgpu::Queue,
        scenario: Scenario,
        temp: f32,
        gravity: f32,
        halo_v0: f32,
        halo_kind: HaloKind,
    ) {
        // The spiral disk balances its circular velocities against the active halo,
        // so seeding takes `halo_kind` too — the disk is born in equilibrium.
        let particles = scenario.generate(temp, halo_kind);
        queue.write_buffer(&self.particle_buffer, 0, bytemuck::cast_slice(&particles));
        self.set_physics(queue, scenario.softening(), gravity, halo_v0, halo_kind);
    }

    /// Rewrite only the params uniform (live gravity / halo changes), leaving the
    /// running bodies untouched.
    pub fn set_physics(
        &self,
        queue: &wgpu::Queue,
        softening: f32,
        gravity: f32,
        halo_v0: f32,
        halo_kind: HaloKind,
    ) {
        let params = Self::build_params(softening, gravity, halo_v0, halo_kind);
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
        pass.dispatch_workgroups(NUM_PARTICLES / WORKGROUP_SIZE, 1, 1);
    }

    pub fn render_pass<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_bind_group(0, &self.render_bind_group, &[]);
        // One triangle-strip quad (4 verts) per particle, instanced.
        render_pass.draw(0..4, 0..NUM_PARTICLES);
    }

    pub fn update_camera(
        &self,
        queue: &wgpu::Queue,
        camera: &crate::camera::Camera,
        scenario: Scenario,
        particle_size: f32,
    ) {
        let matrix = camera.build_view_projection_matrix();
        let matrix_array: &[f32; 16] = matrix.as_ref();
        // Spiral colours by live galactocentric radius; every multi-galaxy
        // scenario colours by each body's vel.w (galaxy of origin).
        let color_mode = match scenario {
            Scenario::Spiral => 0.0,
            _ => 1.0,
        };
        // mat4 (16 floats) then the vec4 of params: billboard size, aspect, colour
        // mode (0 = radius tint for the spiral, 1 = vel.w tint for the merger), and
        // one spare slot.
        let mut data = [0f32; 20];
        data[..16].copy_from_slice(matrix_array);
        data[16..].copy_from_slice(&[particle_size, camera.aspect_ratio, color_mode, 0.0]);
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
        // The tiled gravity kernel reads whole WORKGROUP_SIZE tiles, no tail guard.
        assert_eq!(NUM_PARTICLES % WORKGROUP_SIZE, 0);
    }
}

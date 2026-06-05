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

/// Static logarithmic dark-matter halo centred at the origin: `HALO_V0` is its
/// asymptotic circular speed and `HALO_RC` its core radius. It confines the disk
/// (nothing escapes) and sets the flat outer rotation curve. Set `HALO_V0 = 0`
/// to disable. Fed to the kernel via the params uniform and used by the scenario
/// seeding to set each disk's circular velocity.
pub(crate) const HALO_V0: f32 = 75.0;
pub(crate) const HALO_RC: f32 = 150.0;

/// Billboard half-extent for each particle, in NDC.y (screen-constant, so it is
/// independent of zoom and depth). Tuned for a soft, overlapping additive glow.
const PARTICLE_SIZE: f32 = 0.02;

/// One body. `pos_mass` packs position in xyz and mass in w; `vel` packs velocity
/// in xyz and a 0..1 colour tint in w (read by the render shader). vec4 packing
/// keeps the Rust/WGSL storage layout unambiguous.
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
    pub halo_v0_sq: f32, // dark-matter halo: squared asymptotic circular speed
    pub halo_rc2: f32,   // dark-matter halo: squared core radius
    pub _pad2: u32,
    pub _pad3: u32,
}

pub struct Simulation {
    // Written at init and re-uploaded on reseed (temperature changes).
    particle_buffer: wgpu::Buffer,
    #[allow(dead_code)] // held only to keep the GPU resource alive
    accel_buffer: wgpu::Buffer,
    // Written at init and re-uploaded on reseed (scenario softening changes).
    params_buffer: wgpu::Buffer,
    pub accel_pipeline: wgpu::ComputePipeline,
    pub integrate_pipeline: wgpu::ComputePipeline,
    pub render_pipeline: wgpu::RenderPipeline,
    pub compute_bind_group: wgpu::BindGroup,
    pub render_bind_group: wgpu::BindGroup,
    pub camera_buffer: wgpu::Buffer,
}

impl Simulation {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        console_log!("Creating simulation...");

        // The tiled gravity kernel reads bodies in whole WORKGROUP_SIZE-sized
        // tiles with no tail guard, so the count must divide evenly.
        debug_assert_eq!(NUM_PARTICLES % WORKGROUP_SIZE, 0);

        // Start in the spiral-disk scenario at the default temperature.
        let scenario = Scenario::Spiral;
        let particles = scenario.generate(DEFAULT_TEMP);

        let particle_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Particle Buffer"),
            contents: bytemuck::cast_slice(&particles),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });

        // Scratch acceleration buffer: written by the accel pass, read by the
        // integrate pass each step. Contents need no initialization.
        let accel_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Accel Buffer"),
            size: (NUM_PARTICLES as u64) * 16, // vec4<f32> per body
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let params = Self::build_params(scenario.softening());

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Params Buffer"),
            contents: bytemuck::cast_slice(&[params]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Camera Buffer"),
            size: 80, // mat4 (64) + vec4 params (size, aspect, galaxy_split, pad)
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
                bind_group_layouts: &[&compute_bind_group_layout],
                push_constant_ranges: &[],
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

        // Advances velocity and position from the accelerations.
        let integrate_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Integrate Pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: Some("integrate"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Render Pipeline Layout"),
                bind_group_layouts: &[&render_bind_group_layout],
                push_constant_ranges: &[],
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
            multiview: None,
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
            integrate_pipeline,
            render_pipeline,
            compute_bind_group,
            render_bind_group,
            camera_buffer,
        }
    }

    /// The `SimulationParams` uniform; only `softening` varies (per scenario).
    fn build_params(softening: f32) -> SimulationParams {
        SimulationParams {
            dt: FIXED_DT,
            g: G,
            softening,
            particle_count: NUM_PARTICLES,
            halo_v0_sq: HALO_V0 * HALO_V0,
            halo_rc2: HALO_RC * HALO_RC,
            _pad2: 0,
            _pad3: 0,
        }
    }

    /// Regenerate from fresh initial conditions for `scenario` at `temp` and
    /// upload them, restarting the galaxy. Also updates the params uniform, since
    /// scenarios use different softening. Driven by the scenario / temperature UI.
    pub fn reseed(&self, queue: &wgpu::Queue, scenario: Scenario, temp: f32) {
        let particles = scenario.generate(temp);
        queue.write_buffer(&self.particle_buffer, 0, bytemuck::cast_slice(&particles));
        let params = Self::build_params(scenario.softening());
        queue.write_buffer(&self.params_buffer, 0, bytemuck::cast_slice(&[params]));
    }

    pub fn compute_pass(&self, encoder: &mut wgpu::CommandEncoder) {
        let workgroups = NUM_PARTICLES / WORKGROUP_SIZE;

        // All-pairs gravity into the accel buffer, then integrate. Separate passes
        // so the integrate pass sees the freshly written accelerations, and so the
        // accel pass reads positions that are not being modified concurrently.
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Accel Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.accel_pipeline);
            pass.set_bind_group(0, &self.compute_bind_group, &[]);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Integrate Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.integrate_pipeline);
            pass.set_bind_group(0, &self.compute_bind_group, &[]);
            pass.dispatch_workgroups(workgroups, 1, 1);
        }
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
    ) {
        let matrix = camera.build_view_projection_matrix();
        let matrix_array: &[f32; 16] = matrix.as_ref();
        // mat4 (16 floats) followed by vec4 params: billboard size, aspect ratio,
        // and the colour mode (0 = tint by live radius for the spiral, 1 = tint by
        // each body's vel.w for the merger); the last slot is spare.
        let mut data = [0f32; 20];
        data[..16].copy_from_slice(matrix_array);
        data[16] = PARTICLE_SIZE;
        data[17] = camera.aspect_ratio;
        data[18] = match scenario {
            Scenario::Merger => 1.0,
            Scenario::Spiral => 0.0,
        };
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

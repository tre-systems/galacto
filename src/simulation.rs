use crate::utils::console_log;
use bytemuck::{Pod, Zeroable};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use wgpu::util::DeviceExt;

const NUM_PARTICLES: u32 = 131072;
const WORKGROUP_SIZE: u32 = 64;

/// Fixed simulation timestep (seconds). Physics advances in whole steps of this
/// size regardless of display refresh rate; see the accumulator in `lib.rs`.
pub const FIXED_DT: f32 = 1.0 / 60.0;

/// Billboard half-extent for each particle, in NDC.y (screen-constant, so it is
/// independent of zoom and depth). Tuned for a soft, overlapping additive glow.
const PARTICLE_SIZE: f32 = 0.016;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Particle {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SimulationParams {
    pub dt: f32,
    pub gm: f32,           // Gravitational parameter (G * central_mass)
    pub max_velocity: f32, // Speed clamp for integrator stability
    pub boundary: f32,     // World half-extent; particles bounce here
    pub restitution: f32,  // Boundary bounce energy retention (inelastic)
    pub particle_count: u32,
    pub _pad0: u32,
    pub _pad1: u32,
}

pub struct Simulation {
    #[allow(dead_code)]
    particle_buffer: wgpu::Buffer,
    #[allow(dead_code)] // held only to keep the params uniform's GPU resource alive
    params_buffer: wgpu::Buffer,
    pub compute_pipeline: wgpu::ComputePipeline,
    pub render_pipeline: wgpu::RenderPipeline,
    pub compute_bind_group: wgpu::BindGroup,
    pub render_bind_group: wgpu::BindGroup,
    pub camera_buffer: wgpu::Buffer,
}

impl Simulation {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        console_log!("Creating simulation...");

        // Generate initial particle data
        let particles = Self::generate_initial_particles();

        // Create particle buffer
        let particle_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Particle Buffer"),
            contents: bytemuck::cast_slice(&particles),
            usage: wgpu::BufferUsages::STORAGE,
        });

        // Create simulation parameters
        let params = SimulationParams {
            dt: FIXED_DT,        // fixed simulation timestep
            gm: 40000.0,         // Gravitational parameter (G * central_mass)
            max_velocity: 140.0, // Speed clamp for integrator stability
            boundary: 600.0,     // World half-extent; particles bounce here
            restitution: 0.1,    // Boundary bounce energy retention (inelastic)
            particle_count: NUM_PARTICLES,
            _pad0: 0,
            _pad1: 0,
        };

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Params Buffer"),
            contents: bytemuck::cast_slice(&[params]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Create camera buffer
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Camera Buffer"),
            size: 80, // mat4 (64) + vec4 params (size, aspect, pad, pad)
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Load and create compute shader
        let compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Compute Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/update.wgsl").into()),
        });

        // Load and create render shader
        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Render Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/render.wgsl").into()),
        });

        // Create compute bind group layout
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
                ],
            });

        // Create render bind group layout
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

        // Create compute pipeline
        let compute_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Compute Pipeline Layout"),
                bind_group_layouts: &[&compute_bind_group_layout],
                push_constant_ranges: &[],
            });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Compute Pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: Some("update_particles"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        // Create render pipeline
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
                    format: surface_format,
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

        // Create bind groups
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

        console_log!("⚫ Black Hole Simulation initialized!");
        console_log!(
            "📊 Particle count: {} ({}K)",
            NUM_PARTICLES,
            NUM_PARTICLES / 1000
        );
        console_log!(
            "⚡ Workgroups: {} ({} particles per workgroup)",
            NUM_PARTICLES.div_ceil(WORKGROUP_SIZE),
            WORKGROUP_SIZE
        );
        console_log!("🎯 Ready to simulate gravitational dynamics!");

        Self {
            particle_buffer,
            params_buffer,
            compute_pipeline,
            render_pipeline,
            compute_bind_group,
            render_bind_group,
            camera_buffer,
        }
    }

    fn generate_initial_particles() -> Vec<Particle> {
        let mut rng = StdRng::seed_from_u64(42);
        let mut particles = Vec::with_capacity(NUM_PARTICLES as usize);

        // Add scattered stars close to the black hole (first 500 particles)
        let num_close_stars = 500u32;
        for _ in 0..num_close_stars {
            // Random position in a sphere near the black hole
            let radius = rng.gen_range(20.0..80.0);
            let theta = rng.gen_range(0.0..std::f32::consts::TAU); // Angle around Y axis
            let phi: f32 = rng.gen_range(-0.5..0.5); // Elevation angle (flatten to disk-ish)

            let x = radius * theta.cos() * phi.cos();
            let y = radius * phi.sin() * 0.3; // Flatten vertically
            let z = radius * theta.sin() * phi.cos();

            // Calculate orbital velocity (perpendicular to radius, for roughly circular orbit)
            let speed = (40000.0 / radius).sqrt() * 0.8; // Slightly slower than orbital
            let vx = -theta.sin() * speed;
            let vz = theta.cos() * speed;

            particles.push(Particle {
                position: [x, y, z],
                velocity: [vx, 0.0, vz],
            });
        }

        // Add the main particle stream
        for _i in num_close_stars..NUM_PARTICLES {
            let z = 100.0;
            let x = 10.0;
            let y = rng.gen_range(-150.0..150.0);

            // Calculate perpendicular velocity (tangential to radius)
            let vx = 150.0;

            particles.push(Particle {
                position: [x, y, z],
                velocity: [vx, 0.0, 0.0],
            });
        }

        console_log!("✅ Generated {} particles", NUM_PARTICLES);
        particles
    }

    pub fn compute_pass(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Compute Pass"),
            timestamp_writes: None,
        });

        compute_pass.set_pipeline(&self.compute_pipeline);
        compute_pass.set_bind_group(0, &self.compute_bind_group, &[]);
        let workgroups = NUM_PARTICLES.div_ceil(WORKGROUP_SIZE);
        compute_pass.dispatch_workgroups(workgroups, 1, 1);
    }

    pub fn render_pass<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_bind_group(0, &self.render_bind_group, &[]);
        // One triangle-strip quad (4 verts) per particle, instanced.
        render_pass.draw(0..4, 0..NUM_PARTICLES);
    }

    pub fn update_camera(&self, queue: &wgpu::Queue, camera: &crate::camera::Camera) {
        let matrix = camera.build_view_projection_matrix();
        let matrix_array: &[f32; 16] = matrix.as_ref();
        // mat4 (16 floats) followed by vec4 params: billboard size + aspect ratio.
        let mut data = [0f32; 20];
        data[..16].copy_from_slice(matrix_array);
        data[16] = PARTICLE_SIZE;
        data[17] = camera.aspect_ratio;
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&data));
    }
}

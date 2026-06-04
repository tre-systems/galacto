use crate::utils::console_log;
use bytemuck::{Pod, Zeroable};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::f32::consts::TAU;
use wgpu::util::DeviceExt;

const NUM_PARTICLES: u32 = 131072;
const WORKGROUP_SIZE: u32 = 64;

/// Number of massive galaxy cores. Each carries a disk of test particles; the
/// cores move under their mutual gravity and the particles fall through their
/// combined field. Must not exceed `MAX_CORES` in `update.wgsl`.
const NUM_CORES: u32 = 2;

/// Fixed simulation timestep (seconds). Physics advances in whole steps of this
/// size regardless of display refresh rate; see the accumulator in `lib.rs`.
pub const FIXED_DT: f32 = 1.0 / 60.0;

/// Gravitational constant for the sim's arbitrary unit system.
const G: f32 = 1.0;
/// Mass of each galaxy core (sets the depth of its potential well).
const CORE_MASS: f32 = 450_000.0;
/// Plummer softening length: smooths the core potential so inner orbits stay
/// finite and the disk has a soft glowing bulge instead of a singular spike.
const SOFTENING: f32 = 12.0;

/// Billboard half-extent for each particle, in NDC.y (screen-constant, so it is
/// independent of zoom and depth). Tuned for a soft, overlapping additive glow.
const PARTICLE_SIZE: f32 = 0.016;

/// One test star. WGSL lays out `vec3<f32>` on a 16-byte alignment, so a
/// `{ position: vec3, velocity: vec3 }` storage struct has `velocity` at offset
/// 16 and a 32-byte stride. The explicit pads make the Rust layout match exactly
/// (a tightly packed 24-byte struct would scatter velocity bytes into position).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Particle {
    pub position: [f32; 3],
    pub _pad0: f32,
    pub velocity: [f32; 3],
    pub _pad1: f32,
}

/// A massive body. `pos_mass` packs position in xyz and mass in w; `vel` packs
/// velocity in xyz (w unused). vec4 packing keeps the GPU layout unambiguous.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Core {
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
    pub num_cores: u32,
    pub _pad0: u32,
    pub _pad1: u32,
    pub _pad2: u32,
}

pub struct Simulation {
    #[allow(dead_code)] // held only to keep the GPU resource alive
    particle_buffer: wgpu::Buffer,
    #[allow(dead_code)] // held only to keep the GPU resource alive
    core_buffer: wgpu::Buffer,
    #[allow(dead_code)] // held only to keep the GPU resource alive
    params_buffer: wgpu::Buffer,
    pub cores_pipeline: wgpu::ComputePipeline,
    pub compute_pipeline: wgpu::ComputePipeline,
    pub render_pipeline: wgpu::RenderPipeline,
    pub compute_bind_group: wgpu::BindGroup,
    pub render_bind_group: wgpu::BindGroup,
    pub camera_buffer: wgpu::Buffer,
}

impl Simulation {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        console_log!("Creating simulation...");

        // Generate initial galaxies: massive cores and their disks of test stars.
        let (particles, cores) = Self::generate_initial_galaxies();

        let particle_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Particle Buffer"),
            contents: bytemuck::cast_slice(&particles),
            usage: wgpu::BufferUsages::STORAGE,
        });

        // Cores are read AND written on the GPU (they move under mutual gravity),
        // so the buffer is plain STORAGE with no CPU upload after init.
        let core_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Core Buffer"),
            contents: bytemuck::cast_slice(&cores),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let params = SimulationParams {
            dt: FIXED_DT,
            g: G,
            softening: SOFTENING,
            particle_count: NUM_PARTICLES,
            num_cores: NUM_CORES,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
        };

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Params Buffer"),
            contents: bytemuck::cast_slice(&[params]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Create camera buffer
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Camera Buffer"),
            size: 80, // mat4 (64) + vec4 params (size, aspect, galaxy_split, pad)
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Load and create compute shader (holds both the particle and core kernels)
        let compute_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Compute Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/update.wgsl").into()),
        });

        // Load and create render shader
        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Render Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/render.wgsl").into()),
        });

        // Compute bind group layout: particles (rw), params (uniform), cores (rw).
        // Both compute kernels share this layout and one bind group; the particle
        // kernel only reads `cores`, the core kernel ignores `particles`.
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

        // Advances the galaxy cores under their mutual gravity.
        let cores_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Cores Pipeline"),
            layout: Some(&compute_pipeline_layout),
            module: &compute_shader,
            entry_point: Some("update_cores"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        // Advances every test particle in the cores' combined field.
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
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: core_buffer.as_entire_binding(),
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

        console_log!("✨ Galaxy interaction initialized!");
        console_log!(
            "📊 {} test stars across {} galaxies ({}K each)",
            NUM_PARTICLES,
            NUM_CORES,
            NUM_PARTICLES / NUM_CORES / 1000
        );
        console_log!(
            "⚡ Workgroups: {} ({} particles per workgroup)",
            NUM_PARTICLES.div_ceil(WORKGROUP_SIZE),
            WORKGROUP_SIZE
        );
        console_log!("🌌 Ready to simulate a galaxy flyby!");

        Self {
            particle_buffer,
            core_buffer,
            params_buffer,
            cores_pipeline,
            compute_pipeline,
            render_pipeline,
            compute_bind_group,
            render_bind_group,
            camera_buffer,
        }
    }

    /// Build two rotating disks of test stars around two massive cores set on a
    /// grazing approach. Their mutual pull draws out tidal tails and bridges.
    fn generate_initial_galaxies() -> (Vec<Particle>, Vec<Core>) {
        let mut rng = StdRng::seed_from_u64(42);

        // Two cores on a bound, eccentric orbit about their shared centre of mass
        // (at the origin). They start near apocenter on the x-axis with opposed
        // tangential velocities along y, so they swing through a deep pericenter
        // passage and back, repeatedly — a galaxy "dance" that stays in frame
        // (semi-major axis ~275, period ~30 s) rather than flying apart.
        let cores = vec![
            Core {
                pos_mass: [-213.0, 0.0, 0.0, CORE_MASS],
                vel: [0.0, 15.4, 0.0, 0.0],
            },
            Core {
                pos_mass: [213.0, 0.0, 0.0, CORE_MASS],
                vel: [0.0, -15.4, 0.0, 0.0],
            },
        ];

        let mut particles = Vec::with_capacity(NUM_PARTICLES as usize);
        let per_galaxy = NUM_PARTICLES / NUM_CORES;
        let sqrt_gm = (G * CORE_MASS).sqrt();

        for (c, core) in cores.iter().enumerate() {
            let center = [core.pos_mass[0], core.pos_mass[1], core.pos_mass[2]];
            let bulk = [core.vel[0], core.vel[1], core.vel[2]];
            // Give the last galaxy any remainder so the counts always sum to NUM_PARTICLES.
            let count = if c as u32 == NUM_CORES - 1 {
                NUM_PARTICLES - per_galaxy * (NUM_CORES - 1)
            } else {
                per_galaxy
            };

            for _ in 0..count {
                // Centrally concentrated radius (dense bulge -> sparse outskirts),
                // a thin disk in the x-y plane.
                let t: f32 = rng.gen_range(0.0_f32..1.0);
                let r = 4.0 + 116.0 * t.powf(1.7);
                let theta = rng.gen_range(0.0_f32..TAU);
                let z = rng.gen_range(-4.0_f32..4.0);

                let position = [
                    center[0] + r * theta.cos(),
                    center[1] + r * theta.sin(),
                    center[2] + z,
                ];

                // Circular speed in the Plummer-softened potential:
                //   v_circ^2 = G M r^2 / (r^2 + eps^2)^{3/2}
                // so inner stars don't orbit unphysically fast inside the soft core.
                let denom = (r * r + SOFTENING * SOFTENING).powf(0.75);
                let v_circ = sqrt_gm * r / denom;

                // Prograde (counter-clockwise) tangential velocity plus the core's
                // bulk motion, so the whole disk drifts with its galaxy.
                let velocity = [
                    bulk[0] - v_circ * theta.sin(),
                    bulk[1] + v_circ * theta.cos(),
                    bulk[2],
                ];

                particles.push(Particle {
                    position,
                    _pad0: 0.0,
                    velocity,
                    _pad1: 0.0,
                });
            }
        }

        console_log!("✅ Generated {} test stars", particles.len());
        (particles, cores)
    }

    pub fn compute_pass(&self, encoder: &mut wgpu::CommandEncoder) {
        // Cores first, in their own pass so the write is visible to the particle
        // pass that reads it (pass boundaries act as the storage barrier).
        {
            let mut cores_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Cores Pass"),
                timestamp_writes: None,
            });
            cores_pass.set_pipeline(&self.cores_pipeline);
            cores_pass.set_bind_group(0, &self.compute_bind_group, &[]);
            cores_pass.dispatch_workgroups(1, 1, 1);
        }

        {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Compute Pass"),
                timestamp_writes: None,
            });
            compute_pass.set_pipeline(&self.compute_pipeline);
            compute_pass.set_bind_group(0, &self.compute_bind_group, &[]);
            let workgroups = NUM_PARTICLES.div_ceil(WORKGROUP_SIZE);
            compute_pass.dispatch_workgroups(workgroups, 1, 1);
        }
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
        // mat4 (16 floats) followed by vec4 params: billboard size, aspect ratio,
        // and the galaxy split index used by the render shader to tint by galaxy.
        let mut data = [0f32; 20];
        data[..16].copy_from_slice(matrix_array);
        data[16] = PARTICLE_SIZE;
        data[17] = camera.aspect_ratio;
        data[18] = (NUM_PARTICLES / NUM_CORES) as f32;
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&data));
    }
}

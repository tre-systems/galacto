use crate::utils::console_log;
use bytemuck::{Pod, Zeroable};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::f32::consts::TAU;
use wgpu::util::DeviceExt;

/// Total bodies in the simulation. Self-gravity is all-pairs (O(N²)), so this is
/// far smaller than a test-particle sim would allow. Must be a multiple of
/// `WORKGROUP_SIZE` so the tiled gravity kernel never reads out of bounds.
const NUM_PARTICLES: u32 = 16384;
/// Compute workgroup size, equal to `TILE` in `update.wgsl`.
const WORKGROUP_SIZE: u32 = 256;

/// Fixed simulation timestep (seconds). Physics advances in whole steps of this
/// size regardless of display refresh rate; see the accumulator in `lib.rs`.
pub const FIXED_DT: f32 = 1.0 / 60.0;

/// Gravitational constant for the sim's arbitrary unit system.
const G: f32 = 1.0;

// --- Spiral-disk scenario: a bulge body + a self-gravitating exponential disk
// whose own mass dominates its region (a "maximal disk"), which is spiral-prone.
const BULGE_MASS: f32 = 40_000.0;
const STAR_MASS: f32 = 21.0;
const DISK_RD: f32 = 35.0; // exponential scale length
const DISK_RMAX: f32 = 170.0; // clamp on the sampled disk radius
const DISK_THICKNESS: f32 = 4.0; // initial vertical scale
/// Softening for the spiral disk: small relative to the disk so self-gravity
/// stays "sharp" enough for spiral structure, but large enough to damp noise.
const SPIRAL_SOFTENING: f32 = 12.0;

// --- Merger scenario: two galaxies, each a heavy central body + a disk, on a
// bound approach so self-gravity merges them into one spinning remnant.
const CENTER_MASS: f32 = 300_000.0;
/// Larger softening so the two heavy nuclei coalesce into one soft core on
/// contact rather than locking into a hard, never-merging binary.
const MERGER_SOFTENING: f32 = 25.0;

/// Static logarithmic dark-matter halo centred at the origin: `HALO_V0` is its
/// asymptotic circular speed and `HALO_RC` its core radius. It confines the disk
/// (nothing escapes) and sets the flat outer rotation curve. Set `HALO_V0 = 0`
/// to disable.
const HALO_V0: f32 = 75.0;
const HALO_RC: f32 = 150.0;

/// Disk "temperature": the initial random velocity dispersion as a fraction of
/// the local circular speed, scaled by the temperature slider. Too cold and the
/// disk fragments into clumps; too hot and it stays a featureless smear; spiral
/// arms live in between. `DISP_FRAC` is tuned so the default temperature (1.0)
/// lands in the spiral sweet spot; the slider then explores either side.
const DISP_FRAC: f32 = 0.072;
pub const DEFAULT_TEMP: f32 = 1.0;

/// Billboard half-extent for each particle, in NDC.y (screen-constant, so it is
/// independent of zoom and depth). Tuned for a soft, overlapping additive glow.
const PARTICLE_SIZE: f32 = 0.02;

/// One body. `pos_mass` packs position in xyz and mass in w; `vel` packs velocity
/// in xyz (w unused). vec4 packing keeps the Rust/WGSL storage layout unambiguous.
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

/// Which initial-condition scenario to seed. Chosen from the page's dropdown.
#[derive(Copy, Clone, Debug)]
pub enum Scenario {
    /// A single self-gravitating disk that grows spiral arms.
    Spiral,
    /// Two galaxies on a bound approach that merge into one spinning remnant.
    Merger,
}

impl Scenario {
    pub fn from_id(id: u32) -> Self {
        match id {
            1 => Scenario::Merger,
            _ => Scenario::Spiral,
        }
    }

    /// Plummer softening length to use for this scenario.
    fn softening(self) -> f32 {
        match self {
            Scenario::Spiral => SPIRAL_SOFTENING,
            Scenario::Merger => MERGER_SOFTENING,
        }
    }
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

        // Start in the spiral-disk scenario at the default temperature.
        let scenario = Scenario::Spiral;
        let particles = Self::generate(scenario, DEFAULT_TEMP);

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

    /// Generate the initial bodies for a scenario at a given disk temperature.
    fn generate(scenario: Scenario, temp: f32) -> Vec<Particle> {
        match scenario {
            Scenario::Spiral => Self::generate_disk(temp),
            Scenario::Merger => Self::generate_merger(temp),
        }
    }

    /// Circular speed at radius `r`, from the bulge + enclosed disk mass + halo.
    /// The disk uses a spherical enclosed-mass approximation — not exact for a
    /// flat disk, but close enough that the disk settles and then ripples.
    fn circular_velocity(r: f32) -> f32 {
        let r = r.max(1.0);
        let r2 = r * r;
        let eps2 = SPIRAL_SOFTENING * SPIRAL_SOFTENING;
        let v_bulge2 = G * BULGE_MASS * r2 / (r2 + eps2).powf(1.5);
        let m_disk = (NUM_PARTICLES - 1) as f32 * STAR_MASS;
        let x = r / DISK_RD;
        let m_enc = m_disk * (1.0 - (1.0 + x) * (-x).exp());
        let v_disk2 = G * m_enc / r;
        let v_halo2 = HALO_V0 * HALO_V0 * r2 / (r2 + HALO_RC * HALO_RC);
        (v_bulge2 + v_disk2 + v_halo2).sqrt()
    }

    /// Standard-normal sample (Box–Muller).
    fn gaussian(rng: &mut StdRng) -> f32 {
        let u1: f32 = rng.gen_range(1e-6_f32..1.0);
        let u2: f32 = rng.gen_range(0.0_f32..1.0);
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }

    /// Build a single galaxy: a heavy central bulge body plus a self-gravitating
    /// exponential disk on near-circular prograde (+z) orbits, with a random
    /// thermal velocity dispersion scaled by `temp` (the disk-temperature slider).
    fn generate_disk(temp: f32) -> Vec<Particle> {
        let mut rng = StdRng::seed_from_u64(42);
        let mut particles = Vec::with_capacity(NUM_PARTICLES as usize);

        // Central bulge body, at rest at the origin.
        particles.push(Particle {
            pos_mass: [0.0, 0.0, 0.0, BULGE_MASS],
            vel: [0.0, 0.0, 0.0, 0.0],
        });

        let disp = DISP_FRAC * temp.max(0.0);
        for _ in 1..NUM_PARTICLES {
            // Exponential disk: a gamma(2) radius gives surface density ∝ e^(-r/Rd).
            let u1: f32 = rng.gen_range(1e-4_f32..1.0);
            let u2: f32 = rng.gen_range(1e-4_f32..1.0);
            let r = (-DISK_RD * (u1 * u2).ln()).min(DISK_RMAX);
            let theta = rng.gen_range(0.0_f32..TAU);
            let (st, ct) = (theta.sin(), theta.cos());
            let z = Self::gaussian(&mut rng) * DISK_THICKNESS;

            let vc = Self::circular_velocity(r);
            let sigma = disp * vc;
            // Prograde circular velocity plus a random thermal kick (the
            // "temperature"); the vertical kick is smaller to keep the disk thin.
            let vx = -vc * st + Self::gaussian(&mut rng) * sigma;
            let vy = vc * ct + Self::gaussian(&mut rng) * sigma;
            let vz = Self::gaussian(&mut rng) * sigma * 0.4;

            particles.push(Particle {
                pos_mass: [r * ct, r * st, z, STAR_MASS],
                vel: [vx, vy, vz, 0.0],
            });
        }

        particles
    }

    /// Build two galaxies (a heavy central body + a disk each) on a bound,
    /// prograde approach about the origin, so self-gravity merges them into one
    /// spinning remnant. `temp` sets each disk's initial dispersion.
    fn generate_merger(temp: f32) -> Vec<Particle> {
        let mut rng = StdRng::seed_from_u64(42);
        // (centre position, centre velocity) — deeply bound, so they fall
        // together and merge in a couple of passages; spins and orbit share +z.
        let galaxies = [
            ([-120.0_f32, 0.0, 0.0], [0.0_f32, -20.0, 0.0]),
            ([120.0_f32, 0.0, 0.0], [0.0_f32, 20.0, 0.0]),
        ];

        let mut particles = Vec::with_capacity(NUM_PARTICLES as usize);
        let per_galaxy = NUM_PARTICLES / 2;
        let sqrt_gm = (G * CENTER_MASS).sqrt();
        let disp = DISP_FRAC * temp.max(0.0);

        for (center, bulk) in galaxies {
            // Heavy central body.
            particles.push(Particle {
                pos_mass: [center[0], center[1], center[2], CENTER_MASS],
                vel: [bulk[0], bulk[1], bulk[2], 0.0],
            });

            for _ in 1..per_galaxy {
                // Centrally concentrated disk in the centre's softened potential.
                let t: f32 = rng.gen_range(0.0_f32..1.0);
                let r = 4.0 + 116.0 * t.powf(1.7);
                let theta = rng.gen_range(0.0_f32..TAU);
                let (st, ct) = (theta.sin(), theta.cos());
                let z = rng.gen_range(-4.0_f32..4.0);
                let vc = sqrt_gm * r / (r * r + MERGER_SOFTENING * MERGER_SOFTENING).powf(0.75);
                let sigma = disp * vc;

                particles.push(Particle {
                    pos_mass: [
                        center[0] + r * ct,
                        center[1] + r * st,
                        center[2] + z,
                        STAR_MASS,
                    ],
                    vel: [
                        bulk[0] - vc * st + Self::gaussian(&mut rng) * sigma,
                        bulk[1] + vc * ct + Self::gaussian(&mut rng) * sigma,
                        bulk[2] + Self::gaussian(&mut rng) * sigma * 0.4,
                        0.0,
                    ],
                });
            }
        }

        particles
    }

    /// Regenerate from fresh initial conditions for `scenario` at `temp` and
    /// upload them, restarting the galaxy. Also updates the params uniform, since
    /// scenarios use different softening. Driven by the scenario / temperature UI.
    pub fn reseed(&self, queue: &wgpu::Queue, scenario: Scenario, temp: f32) {
        let particles = Self::generate(scenario, temp);
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

    pub fn update_camera(&self, queue: &wgpu::Queue, camera: &crate::camera::Camera) {
        let matrix = camera.build_view_projection_matrix();
        let matrix_array: &[f32; 16] = matrix.as_ref();
        // mat4 (16 floats) followed by vec4 params: billboard size + aspect ratio
        // (the 4th slot is spare; the render shader tints by radius, not index).
        let mut data = [0f32; 20];
        data[..16].copy_from_slice(matrix_array);
        data[16] = PARTICLE_SIZE;
        data[17] = camera.aspect_ratio;
        queue.write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&data));
    }
}

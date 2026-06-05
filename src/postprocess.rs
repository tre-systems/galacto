//! HDR scene target + bloom post-processing.
//!
//! Particles are rendered additively into an HDR (`rgba16float`) scene texture
//! so bright regions can exceed 1.0. Bloom is then extracted (bright-pass),
//! blurred separably at reduced resolution, and added back during a tonemapped
//! composite into the swapchain. See `src/shaders/post.wgsl`.

/// Format of the offscreen scene and bloom targets. The particle render pipeline
/// targets this format (not the surface format); only the composite writes the
/// surface. `rgba16float` is a renderable, blendable, filterable WebGPU format.
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Bloom buffers are this fraction of the scene resolution — cheaper and wider.
const BLOOM_DIV: u32 = 4;

/// Size-dependent views and bind groups, rebuilt on resize.
struct Targets {
    scene_view: wgpu::TextureView,
    bloom_a_view: wgpu::TextureView,
    bloom_b_view: wgpu::TextureView,
    scene_bg: wgpu::BindGroup, // scene + sampler (group 0); used by bright + composite
    bloom_a_bg: wgpu::BindGroup, // bloom A + sampler (group 0); used by blur_h
    bloom_b_bg: wgpu::BindGroup, // bloom B + sampler (group 0); used by blur_v
    bloom_a_tex_bg: wgpu::BindGroup, // bloom A texture only (group 1); used by composite
}

pub struct PostProcess {
    blur_layout: wgpu::BindGroupLayout,
    bloom_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bright_pipeline: wgpu::RenderPipeline,
    blur_h_pipeline: wgpu::RenderPipeline,
    blur_v_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    targets: Targets,
}

impl PostProcess {
    pub fn new(
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        size: (u32, u32),
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Post Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/post.wgsl").into()),
        });

        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Post Blur Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let bloom_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Post Bloom Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });

        let blur_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Post Blur Pipeline Layout"),
            bind_group_layouts: &[Some(&blur_layout)],
            immediate_size: 0,
        });
        let composite_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Post Composite Pipeline Layout"),
            bind_group_layouts: &[Some(&blur_layout), Some(&bloom_layout)],
            immediate_size: 0,
        });

        let pipeline = |entry: &str, layout: &wgpu::PipelineLayout, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("fs_vert"),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let bright_pipeline = pipeline("bright_pass", &blur_pl, HDR_FORMAT);
        let blur_h_pipeline = pipeline("blur_h", &blur_pl, HDR_FORMAT);
        let blur_v_pipeline = pipeline("blur_v", &blur_pl, HDR_FORMAT);
        let composite_pipeline = pipeline("composite", &composite_pl, surface_format);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Post Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let targets = Self::make_targets(device, &blur_layout, &bloom_layout, &sampler, size);

        Self {
            blur_layout,
            bloom_layout,
            sampler,
            bright_pipeline,
            blur_h_pipeline,
            blur_v_pipeline,
            composite_pipeline,
            targets,
        }
    }

    fn make_targets(
        device: &wgpu::Device,
        blur_layout: &wgpu::BindGroupLayout,
        bloom_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        size: (u32, u32),
    ) -> Targets {
        let target = |w: u32, h: u32, label: &str| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: w.max(1),
                        height: h.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: HDR_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };

        let scene_view = target(size.0, size.1, "Scene HDR");
        let bloom_a_view = target(size.0 / BLOOM_DIV, size.1 / BLOOM_DIV, "Bloom A");
        let bloom_b_view = target(size.0 / BLOOM_DIV, size.1 / BLOOM_DIV, "Bloom B");

        let blur_bg = |view: &wgpu::TextureView, label: &str| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: blur_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
        };

        let bloom_a_tex_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Bloom A Texture BG"),
            layout: bloom_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&bloom_a_view),
            }],
        });

        Targets {
            scene_bg: blur_bg(&scene_view, "Scene BG"),
            bloom_a_bg: blur_bg(&bloom_a_view, "Bloom A BG"),
            bloom_b_bg: blur_bg(&bloom_b_view, "Bloom B BG"),
            bloom_a_tex_bg,
            scene_view,
            bloom_a_view,
            bloom_b_view,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        self.targets = Self::make_targets(
            device,
            &self.blur_layout,
            &self.bloom_layout,
            &self.sampler,
            size,
        );
    }

    /// The HDR target the particle pass renders into.
    pub fn scene_view(&self) -> &wgpu::TextureView {
        &self.targets.scene_view
    }

    /// Run bright-pass → blur (H, V) → tonemapped composite into `output`.
    pub fn run(&self, encoder: &mut wgpu::CommandEncoder, output: &wgpu::TextureView) {
        self.pass(
            encoder,
            &self.targets.bloom_a_view,
            &self.bright_pipeline,
            &self.targets.scene_bg,
            None,
            "Bloom Bright",
        );
        self.pass(
            encoder,
            &self.targets.bloom_b_view,
            &self.blur_h_pipeline,
            &self.targets.bloom_a_bg,
            None,
            "Bloom Blur H",
        );
        self.pass(
            encoder,
            &self.targets.bloom_a_view,
            &self.blur_v_pipeline,
            &self.targets.bloom_b_bg,
            None,
            "Bloom Blur V",
        );
        self.pass(
            encoder,
            output,
            &self.composite_pipeline,
            &self.targets.scene_bg,
            Some(&self.targets.bloom_a_tex_bg),
            "Composite",
        );
    }

    fn pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        bind0: &wgpu::BindGroup,
        bind1: Option<&wgpu::BindGroup>,
        label: &str,
    ) {
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rp.set_pipeline(pipeline);
        rp.set_bind_group(0, bind0, &[]);
        if let Some(b1) = bind1 {
            rp.set_bind_group(1, b1, &[]);
        }
        rp.draw(0..3, 0..1);
    }
}

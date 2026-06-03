use crate::error::AppError;
use crate::utils::console_log;

pub struct Graphics {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub size: (u32, u32),
    pub depth_texture: wgpu::Texture,
    pub depth_view: wgpu::TextureView,
}

impl Graphics {
    pub async fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, AppError> {
        console_log!("Setting up WebGPU...");

        // Create WebGPU instance
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            flags: wgpu::InstanceFlags::default(),
            backend_options: wgpu::BackendOptions::default(),
        });

        let surface = create_canvas_surface(&instance, &canvas)?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| AppError::Graphics("no appropriate WebGPU adapter".into()))?;

        console_log!("Adapter: {:?}", adapter.get_info());

        // Try using Default trait to get minimal device descriptor
        console_log!("Using Default::default() for DeviceDescriptor");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await
            .map_err(|e| AppError::Graphics(format!("create device: {e:?}")))?;

        // Configure the surface
        let size = (canvas.width().max(1), canvas.height().max(1));
        let surface_caps = surface.get_capabilities(&adapter);

        let surface_format = surface_caps
            .formats
            .iter()
            .find(|f| !f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.0,
            height: size.1,
            present_mode: surface_caps.present_modes[0],
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        surface.configure(&device, &config);

        // Create depth texture
        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Depth Texture"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

        console_log!("WebGPU initialized successfully!");

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
            depth_texture,
            depth_view,
        })
    }

    pub fn resize(&mut self, new_width: u32, new_height: u32) {
        if new_width > 0 && new_height > 0 {
            self.size.0 = new_width;
            self.size.1 = new_height;
            self.config.width = new_width;
            self.config.height = new_height;
            self.surface.configure(&self.device, &self.config);

            // Recreate depth texture for new size
            self.depth_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Depth Texture"),
                size: wgpu::Extent3d {
                    width: new_width,
                    height: new_height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });

            self.depth_view = self
                .depth_texture
                .create_view(&wgpu::TextureViewDescriptor::default());
        }
    }
}

/// Create a WebGPU surface from the page canvas. wgpu's safe `SurfaceTarget::Canvas`
/// exists only on the web target; the native stub lets the crate type-check for
/// `cargo clippy`/`test` on the host (it is never reached — galacto only runs in a browser).
#[cfg(target_arch = "wasm32")]
fn create_canvas_surface(
    instance: &wgpu::Instance,
    canvas: &web_sys::HtmlCanvasElement,
) -> Result<wgpu::Surface<'static>, AppError> {
    instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
        .map_err(|e| AppError::Graphics(format!("create surface: {e:?}")))
}

#[cfg(not(target_arch = "wasm32"))]
fn create_canvas_surface(
    _instance: &wgpu::Instance,
    _canvas: &web_sys::HtmlCanvasElement,
) -> Result<wgpu::Surface<'static>, AppError> {
    Err(AppError::Graphics("canvas surface is web-only".into()))
}

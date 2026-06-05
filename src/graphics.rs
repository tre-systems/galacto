use crate::error::AppError;
use crate::utils::console_log;

pub struct Graphics {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub size: (u32, u32),
}

impl Graphics {
    pub async fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, AppError> {
        console_log!("Setting up WebGPU...");

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..Default::default()
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

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await
            .map_err(|e| AppError::Graphics(format!("create device: {e:?}")))?;

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

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
        })
    }

    pub fn resize(&mut self, new_width: u32, new_height: u32) {
        if new_width > 0 && new_height > 0 {
            self.size.0 = new_width;
            self.size.1 = new_height;
            self.config.width = new_width;
            self.config.height = new_height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Re-apply the current surface configuration after a Lost/Outdated surface.
    pub fn reconfigure(&self) {
        self.surface.configure(&self.device, &self.config);
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

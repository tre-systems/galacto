use wasm_bindgen::prelude::*;

mod camera;
mod graphics;
mod input;
mod simulation;
mod utils;

// Import the console_log macro from utils
#[allow(unused_imports)]
use utils::console_log;

use camera::Camera;
use graphics::Graphics;
use input::InputHandler;
use simulation::Simulation;
use utils::set_panic_hook;

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen_futures::spawn_local;

// Global application state
pub struct AppState {
    graphics: Graphics,
    simulation: Simulation,
    camera: Camera,
    input_handler: InputHandler,
    paused: bool,
    last_time: f32,
}

impl AppState {
    pub async fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, JsValue> {
        console_log!("Initializing Black Hole Simulation...");

        let graphics = Graphics::new(canvas).await?;
        let simulation =
            Simulation::new(&graphics.device, &graphics.queue, graphics.config.format)?;
        let mut camera = Camera::new();
        camera.set_aspect_ratio(graphics.size.0 as f32 / graphics.size.1 as f32);
        let input_handler = InputHandler::new()?;

        Ok(Self {
            graphics,
            simulation,
            camera,
            input_handler,
            paused: false,
            last_time: 0.0,
        })
    }

    pub fn update(&mut self, current_time: f32) {
        // requestAnimationFrame provides time in milliseconds
        let dt = if self.last_time > 0.0 {
            (current_time - self.last_time) / 1000.0 // Convert to seconds
        } else {
            0.016 // Default to ~60fps for first frame
        };
        self.last_time = current_time;

        // Update camera based on input
        self.input_handler.update_camera(&mut self.camera);

        // Check for pause toggle first
        if self.input_handler.pause_toggled() {
            self.paused = !self.paused;
            console_log!(
                "Simulation {}",
                if self.paused { "paused" } else { "resumed" }
            );
        }

        // Update simulation if not paused
        if !self.paused {
            self.simulation.update(&self.graphics.queue, dt);
        }
    }

    pub fn render(&mut self) -> Result<(), wasm_bindgen::JsValue> {
        let frame = self
            .graphics
            .surface
            .get_current_texture()
            .map_err(|e| JsValue::from_str(&format!("Failed to get surface texture: {e:?}")))?;

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder =
            self.graphics
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Render Encoder"),
                });

        // Run compute pass if not paused
        if !self.paused {
            self.simulation.compute_pass(&mut encoder);
        }

        // Run render pass
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.01,
                            g: 0.01,
                            b: 0.05,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.graphics.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Update camera uniforms before rendering
            self.simulation
                .update_camera(&self.graphics.queue, &self.camera);
            self.simulation.render_pass(&mut render_pass);
        }

        self.graphics
            .queue
            .submit(std::iter::once(encoder.finish()));
        frame.present();

        Ok(())
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.graphics.resize(width, height);
        self.camera.set_aspect_ratio(width as f32 / height as f32);
    }
}

// Global state wrapped in Rc<RefCell<>> for sharing between closures
static mut APP_STATE: Option<Rc<RefCell<AppState>>> = None;

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    set_panic_hook();

    console_log!("Starting Black Hole Simulation...");

    spawn_local(async {
        if let Err(e) = run().await {
            console_log!("Error running application: {:?}", e);
        }
    });

    Ok(())
}

/// Physical (device-pixel) size to render the canvas at, derived from its CSS
/// layout size and the device pixel ratio. Falls back to 1024x768 before layout.
fn canvas_physical_size(
    window: &web_sys::Window,
    canvas: &web_sys::HtmlCanvasElement,
) -> (u32, u32) {
    let dpr = window.device_pixel_ratio().max(1.0);
    let css_w = canvas.client_width();
    let css_h = canvas.client_height();
    let w = if css_w > 0 {
        (css_w as f64 * dpr).round() as u32
    } else {
        1024
    };
    let h = if css_h > 0 {
        (css_h as f64 * dpr).round() as u32
    } else {
        768
    };
    (w.max(1), h.max(1))
}

async fn run() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window object"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document object"))?;

    let canvas = document
        .get_element_by_id("gpu-canvas")
        .ok_or_else(|| JsValue::from_str("canvas element #gpu-canvas not found"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;

    // Fill the viewport; size the drawing buffer to the displayed size in device pixels.
    canvas.style().set_property("width", "100vw")?;
    canvas.style().set_property("height", "100vh")?;
    let (width, height) = canvas_physical_size(&window, &canvas);
    canvas.set_width(width);
    canvas.set_height(height);

    // Initialize application state
    let app_state = AppState::new(canvas.clone()).await?;
    let app_state_rc = Rc::new(RefCell::new(app_state));

    // Set up input handlers
    {
        let mut app_state_borrow = app_state_rc.borrow_mut();
        app_state_borrow
            .input_handler
            .setup_event_listeners(canvas.clone())?;
    }

    // Store global state for animation loop
    unsafe {
        APP_STATE = Some(app_state_rc.clone());
    }

    // Keep the drawing buffer matched to the displayed size on window resize.
    {
        let resize_canvas = canvas;
        let resize_window = window.clone();
        let closure = Closure::wrap(Box::new(move |_event: web_sys::Event| {
            let (w, h) = canvas_physical_size(&resize_window, &resize_canvas);
            resize_canvas.set_width(w);
            resize_canvas.set_height(h);
            unsafe {
                if let Some(Some(app_state)) = (&raw const APP_STATE).as_ref() {
                    app_state.borrow_mut().resize(w, h);
                }
            }
        }) as Box<dyn FnMut(web_sys::Event)>);
        window.add_event_listener_with_callback("resize", closure.as_ref().unchecked_ref())?;
        closure.forget();
    }

    // Start the render loop
    request_animation_frame();

    Ok(())
}

fn request_animation_frame() {
    let closure = Closure::once_into_js(Box::new(|time: f64| {
        animation_frame(time as f32);
    }));

    web_sys::window()
        .unwrap()
        .request_animation_frame(closure.as_ref().unchecked_ref())
        .unwrap();
}

fn animation_frame(time: f32) {
    unsafe {
        if let Some(Some(app_state)) = (&raw const APP_STATE).as_ref() {
            let mut app = app_state.borrow_mut();
            app.update(time);
            if let Err(e) = app.render() {
                console_log!("Render error: {:?}", e);
            }
        }
    }

    // Request next frame
    request_animation_frame();
}

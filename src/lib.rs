use wasm_bindgen::prelude::*;

mod camera;
mod error;
mod graphics;
mod input;
mod postprocess;
mod simulation;
mod utils;

// Import the console_log macro from utils
#[allow(unused_imports)]
use utils::console_log;

use camera::Camera;
use graphics::Graphics;
use input::InputHandler;
use postprocess::PostProcess;
use simulation::Simulation;
use utils::set_panic_hook;

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen_futures::spawn_local;

/// Cap on simulation substeps run in a single frame. Bounds both a long-stall
/// catch-up burst and the top simulation speed: at 60 fps this many substeps per
/// frame is the highest speed multiplier the slider can actually realize.
const MAX_SUBSTEPS: u32 = 128;

/// Clamp for a single frame's elapsed time before it feeds the accumulator.
const MAX_FRAME_DT: f32 = 0.25;

// Global application state
pub struct AppState {
    graphics: Graphics,
    simulation: Simulation,
    camera: Camera,
    input_handler: InputHandler,
    postprocess: PostProcess,
    paused: bool,
    last_time: f32,
    accumulator: f32,
    steps_this_frame: u32,
    /// Simulation speed multiplier (1.0 = real time); driven by the page's speed slider.
    speed: f32,
}

impl AppState {
    pub async fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, JsValue> {
        console_log!("Initializing Galaxy Simulation...");

        // Graphics is the only fallible step; convert its domain error to a
        // JsValue here at the wasm-bindgen boundary so the engine stays FFI-free.
        let graphics = Graphics::new(canvas)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let postprocess = PostProcess::new(&graphics.device, graphics.config.format, graphics.size);
        let simulation = Simulation::new(&graphics.device, postprocess::HDR_FORMAT);
        let mut camera = Camera::new();
        camera.set_aspect_ratio(graphics.size.0 as f32 / graphics.size.1 as f32);
        let input_handler = InputHandler::new();

        Ok(Self {
            graphics,
            simulation,
            camera,
            input_handler,
            postprocess,
            paused: false,
            last_time: 0.0,
            accumulator: 0.0,
            steps_this_frame: 0,
            speed: 1.0,
        })
    }

    pub fn update(&mut self, current_time: f32) {
        // requestAnimationFrame provides time in milliseconds.
        let frame_dt = if self.last_time > 0.0 {
            (current_time - self.last_time) / 1000.0
        } else {
            simulation::FIXED_DT
        };
        self.last_time = current_time;

        self.input_handler.update_camera(&mut self.camera);

        if self.input_handler.pause_toggled() {
            self.paused = !self.paused;
            console_log!(
                "Simulation {}",
                if self.paused { "paused" } else { "resumed" }
            );
        }

        // Fixed-timestep accumulator: advance the sim in whole FIXED_DT steps so
        // physics is independent of the display's frame rate. render() consumes
        // the step count scheduled here.
        if self.paused {
            self.steps_this_frame = 0;
            self.accumulator = 0.0;
            return;
        }
        self.accumulator += frame_dt.clamp(0.0, MAX_FRAME_DT) * self.speed;
        let mut steps = (self.accumulator / simulation::FIXED_DT) as u32;
        if steps > MAX_SUBSTEPS {
            steps = MAX_SUBSTEPS;
            self.accumulator = 0.0;
        } else {
            self.accumulator -= steps as f32 * simulation::FIXED_DT;
        }
        self.steps_this_frame = steps;
    }

    pub fn render(&mut self) -> Result<(), wasm_bindgen::JsValue> {
        let frame = match self.graphics.surface.get_current_texture() {
            Ok(frame) => frame,
            // The surface goes stale on resize or tab switches; reconfigure and
            // skip this frame rather than erroring.
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                self.graphics.reconfigure();
                return Ok(());
            }
            Err(e) => return Err(JsValue::from_str(&format!("surface error: {e:?}"))),
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder =
            self.graphics
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Render Encoder"),
                });

        // Advance the simulation by the fixed substeps scheduled this frame.
        // Each step is its own compute pass so the GPU orders step N+1's reads
        // after step N's writes.
        for _ in 0..self.steps_this_frame {
            self.simulation.compute_pass(&mut encoder);
        }

        // Render the particles additively into the HDR scene target.
        self.simulation
            .update_camera(&self.graphics.queue, &self.camera);
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Particle Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.postprocess.scene_view(),
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
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.simulation.render_pass(&mut render_pass);
        }

        // Bloom + tonemap the HDR scene into the swapchain.
        self.postprocess.run(&mut encoder, &view);

        self.graphics
            .queue
            .submit(std::iter::once(encoder.finish()));
        frame.present();

        Ok(())
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.graphics.resize(width, height);
        self.postprocess
            .resize(&self.graphics.device, (width, height));
        self.camera.set_aspect_ratio(width as f32 / height as f32);
    }
}

thread_local! {
    // Shared between the rAF loop and the resize handler. A thread-local is the
    // safe single-threaded-WASM stand-in for a mutable global (no `unsafe`).
    static APP_STATE: RefCell<Option<Rc<RefCell<AppState>>>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    set_panic_hook();

    console_log!("Starting Galaxy Simulation...");

    spawn_local(async {
        if let Err(e) = run().await {
            console_log!("Error running application: {:?}", e);
        }
    });

    Ok(())
}

/// Set the simulation speed multiplier (1.0 = real time). Called from the page's
/// speed slider. No-ops until `AppState` has finished async initialization.
#[wasm_bindgen]
pub fn set_speed(speed: f32) {
    let speed = speed.clamp(0.0, MAX_SUBSTEPS as f32);
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().speed = speed;
        }
    });
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

    // Store global state for the animation loop and resize handler.
    APP_STATE.with(|cell| {
        *cell.borrow_mut() = Some(app_state_rc.clone());
    });

    // Keep the drawing buffer matched to the displayed size on window resize.
    {
        let resize_canvas = canvas;
        let resize_window = window.clone();
        let closure = Closure::wrap(Box::new(move |_event: web_sys::Event| {
            let (w, h) = canvas_physical_size(&resize_window, &resize_canvas);
            resize_canvas.set_width(w);
            resize_canvas.set_height(h);
            let app_state = APP_STATE.with(|cell| cell.borrow().clone());
            if let Some(app_state) = app_state {
                app_state.borrow_mut().resize(w, h);
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
    let app_state = APP_STATE.with(|cell| cell.borrow().clone());
    if let Some(app_state) = app_state {
        let mut app = app_state.borrow_mut();
        app.update(time);
        if let Err(e) = app.render() {
            console_log!("Render error: {:?}", e);
        }
    }

    // Request next frame
    request_animation_frame();
}

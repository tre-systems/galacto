use wasm_bindgen::prelude::*;

mod audio;
mod camera;
mod error;
mod graphics;
mod input;
mod music;
mod postprocess;
mod scenarios;
mod simulation;
mod utils;

use audio::AudioEngine;
use camera::Camera;
use graphics::Graphics;
use input::InputHandler;
use postprocess::PostProcess;
use scenarios::Scenario;
use simulation::{HaloKind, Simulation};
use utils::{console_log, set_panic_hook};

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen_futures::spawn_local;

/// Cap on simulation substeps run in a single frame, bounding a long-stall
/// catch-up burst (each substep is an all-pairs O(N²) gravity pass, so a frame
/// shouldn't run too many). Sits well above `MAX_SPEED` so the headroom lets a
/// low frame rate still catch up to the requested speed.
const MAX_SUBSTEPS: u32 = 32;

/// Maximum speed multiplier the UI may request; the page's speed slider tops out
/// here. Separate from `MAX_SUBSTEPS` (a per-frame catch-up bound).
const MAX_SPEED: f32 = 8.0;

/// Clamp for a single frame's elapsed time before it feeds the accumulator.
const MAX_FRAME_DT: f32 = 0.25;

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
    /// Current scenario and the disk temperature staged for the next (re)seed.
    scenario: Scenario,
    disk_temp: f32,
    /// Dark-matter halo profile; switching it re-seeds so the disk stays balanced.
    halo_kind: HaloKind,
    /// Live physics/visual knobs (no re-seed): gravity, halo speed, star size.
    gravity: f32,
    halo_v0: f32,
    particle_size: f32,
    /// Generative soundscape, lazily created on first enable (so the AudioContext
    /// starts inside a user gesture). None until then, or if audio is unavailable.
    audio: Option<AudioEngine>,
    /// Camera rotation last frame and a smoothed rotation speed, so the audio can
    /// react to how fast the view is being stirred.
    prev_rotation: (f32, f32),
    motion: f32,
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
            scenario: Scenario::GrandDesign,
            disk_temp: scenarios::DEFAULT_TEMP,
            halo_kind: HaloKind::Logarithmic,
            gravity: simulation::G,
            halo_v0: simulation::HALO_V0,
            particle_size: simulation::DEFAULT_PARTICLE_SIZE,
            audio: None,
            prev_rotation: (0.0, 0.0),
            motion: 0.0,
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
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(JsValue::from_str("surface validation error"));
            }
            // Outdated / Lost / Timeout / Occluded (or any future status): the
            // surface is stale (resize, tab switch); reconfigure and skip the frame.
            _ => {
                self.graphics.reconfigure();
                return Ok(());
            }
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
        self.simulation.update_camera(
            &self.graphics.queue,
            &self.camera,
            self.scenario,
            self.particle_size,
        );
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Particle Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.postprocess.scene_view(),
                    depth_slice: None,
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
                multiview_mask: None,
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

    /// Stage the disk temperature for the next (re)seed (the disk-temperature
    /// slider). It does not disturb the running sim — Restart or a scenario switch
    /// applies it.
    pub fn set_temperature(&mut self, temp: f32) {
        self.disk_temp = temp;
    }

    /// Switch scenario (the dropdown) and re-seed from its initial conditions.
    pub fn set_scenario(&mut self, id: u32) {
        self.scenario = Scenario::from_id(id);
        self.reseed();
    }

    /// Re-seed the current scenario from fresh initial conditions (the Restart
    /// button). The live gravity / halo / star-size knobs carry over.
    pub fn restart(&mut self) {
        self.reseed();
    }

    fn reseed(&self) {
        self.simulation.reseed(
            &self.graphics.queue,
            self.scenario,
            self.disk_temp,
            self.gravity,
            self.halo_v0,
            self.halo_kind,
        );
    }

    /// Live gravity strength (the gravity slider): rewrites the params uniform
    /// without re-seeding, so the running galaxy responds immediately.
    pub fn set_gravity(&mut self, gravity: f32) {
        self.gravity = gravity;
        self.push_physics();
    }

    /// Live dark-matter halo strength (the halo slider). Live, no re-seed.
    pub fn set_halo(&mut self, halo_v0: f32) {
        self.halo_v0 = halo_v0;
        self.push_physics();
    }

    fn push_physics(&self) {
        self.simulation.set_physics(
            &self.graphics.queue,
            self.scenario.softening(),
            self.gravity,
            self.halo_v0,
            self.halo_kind,
        );
    }

    /// Switch the dark-matter halo profile (the halo-model dropdown) and re-seed,
    /// so the disk is born balanced against the new halo rather than drifting.
    pub fn set_halo_profile(&mut self, id: u32) {
        self.halo_kind = HaloKind::from_id(id);
        self.reseed();
    }

    /// Live on-screen star size (the star-size slider); applied each frame in
    /// `update_camera`.
    pub fn set_particle_size(&mut self, size: f32) {
        self.particle_size = size;
    }

    /// Build the per-frame visual snapshot that drives the soundscape, entirely
    /// from CPU-side state (the camera and the live sim knobs) — the GPU body
    /// state is never read back. Also advances the smoothed camera-motion estimate.
    fn galaxy_state(&mut self) -> music::GalaxyState {
        // Zoom: log-map the camera scale (0.001..5.0) to 0 (far) .. 1 (close).
        let (lo, hi) = (0.001_f32.ln(), 5.0_f32.ln());
        let zoom = ((self.camera.scale.ln() - lo) / (hi - lo)).clamp(0.0, 1.0);

        // Camera rotation speed this frame, smoothed so it lingers musically.
        let (rx, ry) = (self.camera.rotation_x, self.camera.rotation_y);
        let delta =
            ((rx - self.prev_rotation.0).powi(2) + (ry - self.prev_rotation.1).powi(2)).sqrt();
        self.prev_rotation = (rx, ry);
        let raw_motion = (delta / 0.06).clamp(0.0, 1.0);
        self.motion = self.motion * 0.85 + raw_motion * 0.15;

        music::GalaxyState {
            scenario: self.scenario,
            zoom,
            motion: self.motion,
            speed: ((self.speed - 0.25) / (MAX_SPEED - 0.25)).clamp(0.0, 1.0),
            intensity: (self.steps_this_frame as f32 / 8.0).clamp(0.0, 1.0),
            gravity: ((self.gravity - 0.25) / (4.0 - 0.25)).clamp(0.0, 1.0),
            halo: (self.halo_v0 / 150.0).clamp(0.0, 1.0),
            glow: ((self.particle_size - 0.006) / (0.026 - 0.006)).clamp(0.0, 1.0),
            paused: self.paused,
        }
    }

    /// Drive the soundscape for this frame (a no-op until sound is enabled).
    pub fn update_audio(&mut self) {
        if self.audio.is_none() {
            return;
        }
        let state = self.galaxy_state();
        if let Some(audio) = &mut self.audio {
            audio.update(&state);
        }
    }

    /// Toggle the soundscape (the page's 🔊 button). The first enable builds the
    /// audio engine inside the click's user gesture, so the AudioContext may start.
    pub fn set_sound(&mut self, on: bool) {
        if on && self.audio.is_none() {
            self.audio = AudioEngine::new();
            if self.audio.is_none() {
                console_log!("Audio unavailable in this browser/context.");
            }
        }
        if let Some(audio) = &mut self.audio {
            audio.set_enabled(on);
        }
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
    let speed = speed.clamp(0.0, MAX_SPEED);
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().speed = speed;
        }
    });
}

/// Stage the disk "temperature" for the next (re)seed (the disk-temperature
/// slider). Doesn't restart the running sim — Restart or a scenario switch applies
/// it. No-ops until ready.
#[wasm_bindgen]
pub fn set_disk_temperature(temp: f32) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_temperature(temp);
        }
    });
}

/// Switch the initial-condition scenario (0 = spiral disk, 1–5 = the multi-galaxy
/// setups), re-seeding from its initial conditions. Called by the scenario dropdown.
#[wasm_bindgen]
pub fn set_scenario(id: u32) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_scenario(id);
        }
    });
}

/// Re-seed the current scenario from fresh initial conditions (the Restart
/// button). No-ops until ready.
#[wasm_bindgen]
pub fn restart() {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().restart();
        }
    });
}

/// Live gravity strength (the gravity slider). No-ops until ready.
#[wasm_bindgen]
pub fn set_gravity(gravity: f32) {
    let gravity = gravity.clamp(0.0, 10.0);
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_gravity(gravity);
        }
    });
}

/// Live dark-matter halo strength (the halo slider). No-ops until ready.
#[wasm_bindgen]
pub fn set_halo(halo_v0: f32) {
    let halo_v0 = halo_v0.clamp(0.0, 400.0);
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_halo(halo_v0);
        }
    });
}

/// Switch the dark-matter halo profile (0 = logarithmic, 1 = NFW), re-seeding so
/// the disk starts in equilibrium with it. Called by the halo-model dropdown.
/// No-ops until ready.
#[wasm_bindgen]
pub fn set_halo_profile(id: u32) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_halo_profile(id);
        }
    });
}

/// Toggle the generative soundscape on or off (the page's 🔊 button). The first
/// call builds the audio engine within the click gesture so the browser allows
/// the AudioContext to start. No-ops until ready.
#[wasm_bindgen]
pub fn set_sound_enabled(on: bool) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_sound(on);
        }
    });
}

/// Live on-screen star size (the star-size slider). No-ops until ready.
#[wasm_bindgen]
pub fn set_particle_size(size: f32) {
    let size = size.clamp(0.002, 0.06);
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_particle_size(size);
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

    let app_state = AppState::new(canvas.clone()).await?;
    let app_state_rc = Rc::new(RefCell::new(app_state));

    {
        let mut app_state_borrow = app_state_rc.borrow_mut();
        app_state_borrow
            .input_handler
            .setup_event_listeners(canvas.clone())?;
    }

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
        // One forever-listener, so forget() (the app-lifetime variant of the
        // retained-closures pattern) is intentional rather than the _closures Vec.
        closure.forget();
    }

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
        app.update_audio();
        if let Err(e) = app.render() {
            console_log!("Render error: {:?}", e);
        }
    }

    request_animation_frame();
}

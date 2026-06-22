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
mod units;
mod utils;

use audio::AudioEngine;
use camera::Camera;
use graphics::Graphics;
use input::InputHandler;
use postprocess::PostProcess;
use scenarios::Scenario;
use simulation::{HaloKind, Reseed, Simulation};
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

/// Characteristic radial speed (sim units) that normalises the core flux/activity
/// signals to ~0..1: ordinary disk churn sits low (~0.4), while a merger infall or
/// a close flyby pushes it toward the top. Tunable.
const CORE_V_SCALE: f32 = 110.0;

/// Frame-rate-independent eased follow with a hard slew cap: the value glides
/// toward `target` (exponential, time-constant `tau`) but can move no faster than
/// `max_rate` per second, so a sudden jump in the simulation can never make the
/// sound lurch — it always eases, for a cinematic feel.
fn ease_slew(current: f32, target: f32, dt: f32, tau: f32, max_rate: f32) -> f32 {
    let alpha = 1.0 - (-dt / tau.max(1e-3)).exp();
    let eased = current + (target - current) * alpha;
    let max_step = max_rate * dt;
    current + (eased - current).clamp(-max_step, max_step)
}

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
    /// Total simulated time since the last (re)seed, in sim time units (each step
    /// advances `FIXED_DT`). Surfaced as a physical clock via `units`.
    sim_time: f32,
    /// Clamped real seconds elapsed last frame, reused for frame-rate-independent
    /// audio smoothing.
    frame_dt: f32,
    /// Simulation speed multiplier (1.0 = real time); driven by the page's speed slider.
    speed: f32,
    /// Current scenario and the disk temperature staged for the next (re)seed.
    scenario: Scenario,
    disk_temp: f32,
    /// Active body count, set by the body-count slider and carried into every
    /// reseed (changing it re-seeds the scenario at the new resolution).
    particle_count: u32,
    /// Dark-matter halo profile; switching it re-seeds so the disk stays balanced.
    halo_kind: HaloKind,
    /// Whether to draw the dark-matter halo overlay (the "Show" toggle).
    halo_visible: bool,
    /// Live physics/visual knobs (no re-seed): gravity, halo speed, star size.
    gravity: f32,
    halo_v0: f32,
    particle_size: f32,
    /// Generative soundscape, lazily created on first enable (so the AudioContext
    /// starts inside a user gesture). None until then, or if audio is unavailable.
    audio: Option<AudioEngine>,
    /// User volume (0..1) and mute, held here so they survive until the engine is
    /// built (on first interaction) and are re-applied to it then.
    sound_volume: f32,
    sound_muted: bool,
    /// Camera rotation last frame and a smoothed rotation speed, so the audio can
    /// react to how fast the view is being stirred.
    prev_rotation: (f32, f32),
    motion: f32,
    /// Smoothed, normalised core signals derived from the GPU readback
    /// (`CoreStats`): central-mass concentration, signed radial flux (matter
    /// moving out of / into the centre), and core churn — the soundscape's primary
    /// drivers. `core_mass_ref` / `core_mass_dev` are the slow adaptive baseline
    /// the concentration is measured against.
    core_initialized: bool,
    core_mass_ref: f32,
    core_mass_dev: f32,
    core_mass_s: f32,
    core_flux_s: f32,
    core_activity_s: f32,
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
            sim_time: 0.0,
            frame_dt: simulation::FIXED_DT,
            speed: 1.0,
            scenario: Scenario::GrandDesign,
            disk_temp: scenarios::DEFAULT_TEMP,
            particle_count: simulation::NUM_PARTICLES,
            halo_kind: HaloKind::Logarithmic,
            halo_visible: false,
            gravity: simulation::G,
            halo_v0: simulation::HALO_V0,
            particle_size: simulation::DEFAULT_PARTICLE_SIZE,
            audio: None,
            // Start at 10% of full volume so the soundscape opens gently; matches
            // the page's volume slider default (32% → ~0.10 on its squared taper).
            sound_volume: 0.10,
            sound_muted: false,
            prev_rotation: (0.0, 0.0),
            motion: 0.0,
            core_initialized: false,
            core_mass_ref: 0.0,
            core_mass_dev: 0.0,
            core_mass_s: 0.0,
            core_flux_s: 0.0,
            core_activity_s: 0.0,
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
        self.frame_dt = frame_dt.clamp(0.0, MAX_FRAME_DT);

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
        // Each scheduled step advances the physics by FIXED_DT, regardless of the
        // speed multiplier (which only changes how many steps run per real second).
        self.sim_time += steps as f32 * simulation::FIXED_DT;
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

        // Record the throttled core-statistics reduction (drives the audio) on the
        // post-step positions; whether it actually ran (not already in flight)
        // gates the async map after submit below.
        let did_reduce = self.simulation.record_core_reduction(&mut encoder);

        // Render the particles additively into the HDR scene target.
        self.simulation.update_camera(
            &self.graphics.queue,
            &self.camera,
            self.scenario,
            self.particle_size,
        );
        // Dark-matter halo overlay (when shown): size it to the active profile's
        // scale radius (log confines broadly; NFW is more concentrated), violet so
        // it reads as dark matter, distinct from the white/blue stars.
        if self.halo_visible {
            let (right, up) = self.camera.billboard_basis();
            let radius = match self.halo_kind {
                HaloKind::Logarithmic => simulation::HALO_RC,
                HaloKind::Nfw => simulation::NFW_RS,
            } * 3.5;
            self.simulation.update_halo_view(
                &self.graphics.queue,
                right,
                up,
                radius,
                [0.55, 0.30, 1.0],
                0.6,
            );
        }
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
            // Halo behind the stars (additive, so they glow over it).
            if self.halo_visible {
                self.simulation.render_halo(&mut render_pass);
            }
            self.simulation.render_pass(&mut render_pass);
        }

        // Bloom + tonemap the HDR scene into the swapchain.
        self.postprocess.run(&mut encoder, &view);

        self.graphics
            .queue
            .submit(std::iter::once(encoder.finish()));
        frame.present();

        // Map the core-statistics readback (async; resolves a few frames later and
        // updates the CoreStats the audio reads).
        if did_reduce {
            self.simulation.map_core_readback();
        }

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

    fn reseed(&mut self) {
        self.sim_time = 0.0; // a fresh galaxy restarts the clock
        self.simulation.reseed(
            &self.graphics.queue,
            Reseed {
                scenario: self.scenario,
                temp: self.disk_temp,
                count: self.particle_count,
                gravity: self.gravity,
                halo_v0: self.halo_v0,
                halo_kind: self.halo_kind,
            },
        );
    }

    /// Set the body count (the body-count slider) and re-seed the current scenario
    /// at the new resolution. Clamped to a valid tile-multiple within bounds.
    pub fn set_count(&mut self, count: u32) {
        self.particle_count = simulation::clamp_particle_count(count);
        self.reseed();
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

    /// Show or hide the dark-matter halo overlay (the "Show" toggle). Render-only —
    /// `render` reads this flag; no re-seed.
    pub fn set_halo_visible(&mut self, visible: bool) {
        self.halo_visible = visible;
    }

    /// Live on-screen star size (the star-size slider); applied each frame in
    /// `update_camera`.
    pub fn set_particle_size(&mut self, size: f32) {
        self.particle_size = size;
    }

    /// Build the per-frame snapshot that drives the soundscape — the camera and
    /// live sim knobs plus the galaxy's core dynamics (central mass + radial flux)
    /// from the GPU readback. Advances the smoothed motion estimate and slew-limits
    /// the core signals so the sound eases between states rather than jumping.
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

        // Core dynamics read back from the GPU — how much mass sits at the centre
        // and how fast it is moving in or out. These are the primary drivers, so
        // the sound reacts to the galaxy itself, not just the camera.
        let stats = self.simulation.core_stats();
        if !self.core_initialized {
            self.core_mass_ref = stats.mass;
            self.core_mass_dev = stats.mass.abs().max(1.0) * 0.1;
            self.core_initialized = true;
        }
        // Slow adaptive baseline (a few seconds): the absolute mass sum is bulge-/
        // scenario-dominated, so concentration tracks mass building up or draining
        // relative to its recent norm — which is what the eye actually registers.
        let dt = self.frame_dt;
        let ref_alpha = 1.0 - (-dt / 4.0).exp();
        self.core_mass_ref += (stats.mass - self.core_mass_ref) * ref_alpha;
        self.core_mass_dev +=
            ((stats.mass - self.core_mass_ref).abs() - self.core_mass_dev) * ref_alpha;
        let concentration = (0.5
            + 0.5 * (stats.mass - self.core_mass_ref) / (3.0 * self.core_mass_dev + 1.0))
            .clamp(0.0, 1.0);
        // Mass-weighted mean radial velocity → signed flux (+ outward) and unsigned
        // churn, normalised by a characteristic speed.
        let inv_mass = 1.0 / stats.mass.max(1.0);
        let flux = (stats.flux * inv_mass / CORE_V_SCALE).clamp(-1.0, 1.0);
        let activity = (stats.activity * inv_mass / CORE_V_SCALE).clamp(0.0, 1.0);
        // Glide toward the targets with a hard slew cap, so a sudden event (a
        // collision spike) eases in over ~2 s rather than snapping — cinematic.
        self.core_mass_s = ease_slew(self.core_mass_s, concentration, dt, 0.7, 0.5);
        self.core_activity_s = ease_slew(self.core_activity_s, activity, dt, 0.7, 0.5);
        self.core_flux_s = ease_slew(self.core_flux_s, flux, dt, 0.9, 0.6);

        music::GalaxyState {
            scenario: self.scenario,
            zoom,
            motion: self.motion,
            speed: ((self.speed - 0.25) / (MAX_SPEED - 0.25)).clamp(0.0, 1.0),
            intensity: (self.steps_this_frame as f32 / 8.0).clamp(0.0, 1.0),
            gravity: ((self.gravity - 0.25) / (4.0 - 0.25)).clamp(0.0, 1.0),
            halo: (self.halo_v0 / 150.0).clamp(0.0, 1.0),
            glow: ((self.particle_size - 0.006) / (0.026 - 0.006)).clamp(0.0, 1.0),
            core_mass: self.core_mass_s,
            core_flux: self.core_flux_s,
            core_activity: self.core_activity_s,
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
            match &mut self.audio {
                Some(audio) => {
                    // Carry the user's volume/mute (set before the engine existed)
                    // into the freshly built engine.
                    audio.set_volume(self.sound_volume);
                    audio.set_muted(self.sound_muted);
                }
                None => {
                    console_log!("Audio unavailable in this browser/context.");
                }
            }
        }
        if let Some(audio) = &mut self.audio {
            audio.set_enabled(on);
        }
    }

    /// Set the user volume (0..1). Remembered even before the audio engine exists.
    pub fn set_volume(&mut self, volume: f32) {
        self.sound_volume = volume.clamp(0.0, 1.0);
        if let Some(audio) = &mut self.audio {
            audio.set_volume(self.sound_volume);
        }
    }

    /// Mute or unmute the soundscape. Remembered even before the engine exists.
    pub fn set_muted(&mut self, muted: bool) {
        self.sound_muted = muted;
        if let Some(audio) = &mut self.audio {
            audio.set_muted(muted);
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

/// Set the body count (the body-count slider), re-seeding the current scenario at
/// the new resolution. The value is clamped to a tile multiple within bounds.
/// No-ops until ready.
#[wasm_bindgen]
pub fn set_particle_count(count: u32) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_count(count);
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

/// Show or hide the dark-matter halo overlay (the halo "Show" toggle). Render-only.
/// No-ops until ready.
#[wasm_bindgen]
pub fn set_halo_visible(visible: bool) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_halo_visible(visible);
        }
    });
}

/// Sample the disk's rotation curve under the current live gravity/halo, in
/// physical units, for the rotation-curve overlay. Returns a flat array of
/// `samples` groups of five: `[radius_kpc, v_bulge, v_disk, v_halo, v_total]`, the
/// velocities in km/s. The page recomputes it whenever the gravity/halo controls
/// change. Empty until ready.
#[wasm_bindgen]
pub fn rotation_curve(samples: u32) -> Vec<f32> {
    let n = samples.max(2);
    APP_STATE.with(|cell| {
        let borrow = cell.borrow();
        let Some(app) = borrow.as_ref() else {
            return Vec::new();
        };
        let app = app.borrow();
        // Sample out past the disk edge so the flat, halo-supported part is visible.
        let r_max = 260.0_f32;
        let mut out = Vec::with_capacity(n as usize * 5);
        for i in 0..n {
            let r = 1.0 + (r_max - 1.0) * (i as f32 / (n - 1) as f32);
            let [vb, vd, vh] =
                scenarios::rotation_components(r, app.gravity, app.halo_v0, app.halo_kind);
            let vt = (vb * vb + vd * vd + vh * vh).sqrt();
            out.push(r * units::KPC_PER_UNIT);
            out.push(vb * units::KMS_PER_UNIT);
            out.push(vd * units::KMS_PER_UNIT);
            out.push(vh * units::KMS_PER_UNIT);
            out.push(vt * units::KMS_PER_UNIT);
        }
        out
    })
}

/// Simulated time since the last (re)seed, in megayears — for the on-screen clock.
/// 0 until ready.
#[wasm_bindgen]
pub fn elapsed_myr() -> f32 {
    APP_STATE.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|app| app.borrow().sim_time * units::MYR_PER_UNIT)
            .unwrap_or(0.0)
    })
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

/// Set the soundscape volume (0..1) from the page's volume slider. Remembered
/// until the audio engine is built, then applied live. No-ops until ready.
#[wasm_bindgen]
pub fn set_volume(volume: f32) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_volume(volume);
        }
    });
}

/// Mute or unmute the soundscape (the page's mute button). No-ops until ready.
#[wasm_bindgen]
pub fn set_muted(muted: bool) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_muted(muted);
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

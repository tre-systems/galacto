use wasm_bindgen::prelude::*;

mod arrangement;
mod audio;
mod camera;
mod error;
mod graphics;
mod input;
mod mastering;
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

/// Cap on simulation substeps run in a single default-count frame, bounding a
/// long-stall catch-up burst (each substep is an all-pairs O(N²) gravity pass).
/// Higher particle counts get a lower dynamic cap via [`substep_cap_for_count`].
const MAX_SUBSTEPS: u32 = 32;

/// Maximum speed multiplier the UI may request; the page's speed slider tops out
/// here. Separate from `MAX_SUBSTEPS` (a per-frame catch-up bound).
const MAX_SPEED: f32 = 8.0;

/// Page slider ranges used to normalise values for the soundscape. Keep these in
/// sync with `static/index.html`; the sim setters may accept wider developer-console
/// values, but the musical mapping should use the user's visible 0..1 travel.
const UI_MIN_SPEED: f32 = 0.25;
const UI_MIN_GRAVITY: f32 = 0.25;
const UI_MAX_GRAVITY: f32 = 4.0;
const UI_MAX_HALO_V0: f32 = simulation::HALO_V0 * 2.0;
const UI_MAX_GAS_FRACTION: f32 = 0.5;
const UI_MAX_BULGE_FRAC: f32 = 0.6;
const UI_MIN_DISK_TEMP: f32 = 0.5;
const UI_MAX_DISK_TEMP: f32 = 3.0;
const UI_MIN_HALO_RC_SCALE: f32 = 0.4;
const UI_MAX_HALO_RC_SCALE: f32 = 2.5;
const UI_MIN_PARTICLE_SIZE: f32 = 0.006;
const UI_MAX_PARTICLE_SIZE: f32 = 0.026;

/// Clamp for a single frame's elapsed time before it feeds the accumulator.
const MAX_FRAME_DT: f32 = 0.25;

/// Characteristic radial speed (sim units) that normalises the core flux/activity
/// signals to ~0..1: ordinary disk churn sits low (~0.4), while a merger infall or
/// a close flyby pushes it toward the top. Tunable.
const CORE_V_SCALE: f32 = 110.0;

/// Audio export render sample rate. 48 kHz keeps the BS.1770 loudness coefficients
/// exact (`src/mastering.rs`) and is a standard streaming delivery rate.
const EXPORT_SAMPLE_RATE: u32 = 48_000;
/// Capture cadence for the export timeline (~30 Hz is ample for smooth automation,
/// and keeps the offline scheduling cheap).
const RECORD_DT: f64 = 1.0 / 30.0;
/// Maximum length (seconds) for a recorded export or a composed piece — bounds the
/// offline render's memory and time. 15 min covers a full-length ambient piece while
/// staying within a browser tab's memory for the offline render.
const MAX_RECORD_SEC: f64 = 900.0;

/// Count-aware per-frame step cap. The gravity cost scales as N², so the catch-up
/// budget shrinks with the square of the active body-count ratio; at the 10× public
/// maximum this deliberately permits only one step per frame rather than queuing a
/// watchdog-scale burst.
fn substep_cap_for_count(count: u32) -> u32 {
    let count = simulation::clamp_particle_count(count).max(1) as f32;
    let ratio = simulation::NUM_PARTICLES as f32 / count;
    ((MAX_SUBSTEPS as f32 * ratio * ratio).floor() as u32).clamp(1, MAX_SUBSTEPS)
}

/// UI-visible speed ceiling that matches the scheduler cap. At 60Hz, speed `N`
/// requests roughly `N` fixed steps per frame, so the substep cap is also the safe
/// speed cap for expensive high-count runs.
fn max_speed_for_count(count: u32) -> f32 {
    (substep_cap_for_count(count) as f32).clamp(1.0, MAX_SPEED)
}

fn norm_range(value: f32, min: f32, max: f32) -> f32 {
    ((value - min) / (max - min)).clamp(0.0, 1.0)
}

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
    /// Exponentially-smoothed frames-per-second, exposed via [`fps`] so production
    /// tooling can confirm a capture is holding a smooth rate at a given body count.
    fps_ema: f32,
    /// Simulation speed multiplier (1.0 = real time); driven by the page's speed slider.
    speed: f32,
    /// Current scenario and the disk temperature staged for the next (re)seed.
    scenario: Scenario,
    disk_temp: f32,
    /// Fraction of the disk seeded as gas, staged for the next (re)seed (the
    /// gas-fraction slider). Only the disk scenarios use it.
    gas_fraction: f32,
    /// Bulge mass fraction (bulge / [bulge + disk]), staged for the next (re)seed
    /// (the bulge slider). Sets the central bulge point mass.
    bulge_frac: f32,
    /// Active body count, set by the body-count slider and carried into every
    /// reseed (changing it re-seeds the scenario at the new resolution).
    particle_count: u32,
    /// Latest body-count slider value for the soundscape while a drag is in
    /// progress. The active sim count is only committed on release, so the
    /// scheduler never assumes a cheaper count before the expensive reseed happens.
    audio_particle_count: u32,
    /// Dark-matter halo profile; switching it re-seeds so the disk stays balanced.
    halo_kind: HaloKind,
    /// Whether to draw the dark-matter halo overlay (the "Show" toggle).
    halo_visible: bool,
    /// Live physics/visual knobs (no re-seed): gravity, halo speed, halo
    /// concentration (scale-radius multiplier, 1 = default), star size.
    gravity: f32,
    halo_v0: f32,
    halo_rc_scale: f32,
    particle_size: f32,
    /// Glow halo extent (0..1, the Glow slider): how far each star's faint halo
    /// reaches around its bright point. It never touches the simulation, but it
    /// also colours the soundscape as a brightness/space cue.
    glow_extent: f32,
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
    /// Smoothed stereo bias from the camera azimuth (sin of the orbit angle, -1..1),
    /// so the soundscape swings across the field as the view circles the galaxy.
    camera_pan_s: f32,
    /// Cinematic autopilot: a slow self-driving camera orbit + glide, on by default
    /// until the user grabs the view (the page's toggle turns it back on). Its clock
    /// advances only while it runs, so resuming continues smoothly.
    autopilot: bool,
    autopilot_t: f32,
    /// Autopilot speed multiplier (the page's autopilot slider), scaling the base
    /// orbit / glide / nod rates.
    autopilot_speed: f32,
    /// Active cinematic arrangement (the composed A→B→C piece) and its elapsed time.
    /// While set, it drives the camera + live physics so the visuals perform the same
    /// arc the offline audio is rendered from — keeping picture and sound locked.
    arrangement: Option<arrangement::Arrangement>,
    arrangement_t: f64,
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
    /// Smoothed core coherence (0..1): organized infall/expansion vs. random churn,
    /// from `|flux| / activity` of the readback. Focuses or widens the pad.
    core_coherence_s: f32,
    /// Rolling capture of the live `GalaxyState` timeline — `(seconds_from_start,
    /// state)` — replayed offline by the WAV export. Captured at [`RECORD_DT`] while
    /// `recording_on`, starting from `record_start_ms` on the rAF clock.
    recording: Vec<(f64, music::GalaxyState)>,
    recording_on: bool,
    record_start_ms: f32,
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
            fps_ema: 60.0,
            speed: 1.0,
            scenario: Scenario::GrandDesign,
            disk_temp: scenarios::DEFAULT_TEMP,
            gas_fraction: scenarios::DEFAULT_GAS_FRACTION,
            bulge_frac: scenarios::DEFAULT_BULGE_FRAC,
            particle_count: simulation::NUM_PARTICLES,
            audio_particle_count: simulation::NUM_PARTICLES,
            halo_kind: HaloKind::Logarithmic,
            halo_visible: false,
            gravity: simulation::G,
            halo_v0: simulation::HALO_V0,
            halo_rc_scale: 1.0,
            particle_size: simulation::DEFAULT_PARTICLE_SIZE,
            glow_extent: 0.4,
            audio: None,
            // Start at 10% of full volume so the soundscape opens gently; matches
            // the page's volume slider default (32% → ~0.10 on its squared taper).
            sound_volume: 0.10,
            sound_muted: false,
            prev_rotation: (0.0, 0.0),
            motion: 0.0,
            camera_pan_s: 0.0,
            autopilot: true,
            autopilot_t: 0.0,
            autopilot_speed: 0.4,
            arrangement: None,
            arrangement_t: 0.0,
            core_initialized: false,
            core_mass_ref: 0.0,
            core_mass_dev: 0.0,
            core_mass_s: 0.0,
            core_flux_s: 0.0,
            core_activity_s: 0.0,
            core_coherence_s: 0.0,
            recording: Vec::new(),
            recording_on: false,
            record_start_ms: 0.0,
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
        // Smooth the instantaneous rate so the meter is steady to read.
        if frame_dt > 1e-4 {
            let inst = (1.0 / frame_dt).clamp(0.0, 240.0);
            self.fps_ema += 0.1 * (inst - self.fps_ema);
        }

        self.input_handler.update_camera(&mut self.camera);

        // A composed cinematic arrangement (for a complete piece / video capture)
        // drives the camera and the live physics through an intentional arc; without
        // one, the free autopilot gently self-drives the view like a movie.
        if self.arrangement.is_some() {
            self.advance_arrangement();
        } else if self.autopilot {
            // The phase clock advances at the scaled rate, so changing the speed
            // never jumps the glide; `autopilot_step` scales the orbit to match.
            self.autopilot_t += self.frame_dt * self.autopilot_speed;
            self.camera
                .autopilot_step(self.frame_dt, self.autopilot_t, self.autopilot_speed);
        }

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
        let step_cap = substep_cap_for_count(self.particle_count);
        if steps > step_cap {
            steps = step_cap;
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
        let did_reduce = if self
            .audio
            .as_ref()
            .is_some_and(AudioEngine::wants_core_stats)
            && !self.paused
        {
            self.simulation.record_core_reduction(&mut encoder)
        } else {
            false
        };

        // Render the particles additively into the HDR scene target. Shrink each
        // billboard as the body count grows (∝ 1/√count), so the total glow area —
        // the 4K fill-rate cost, and the additive brightness pile-up — stays roughly
        // constant. More bodies then read as *finer detail* at a steady frame rate
        // rather than a heavier, brighter blob. At the default count the scale is 1.0,
        // so the established look is unchanged.
        let size_scale = (simulation::NUM_PARTICLES as f32 / self.particle_count.max(1) as f32)
            .sqrt()
            .clamp(0.3, 1.0);
        self.simulation.update_camera(
            &self.graphics.queue,
            &self.camera,
            self.scenario,
            self.particle_size * size_scale,
            self.glow_extent,
        );
        // Dark-matter halo overlay (when shown): size it to the active profile's
        // scale radius (log confines broadly; NFW is more concentrated), violet so
        // it reads as dark matter, distinct from the white/blue stars.
        if self.halo_visible {
            let (right, up) = self.camera.billboard_basis();
            let radius = match self.halo_kind {
                HaloKind::Logarithmic => simulation::HALO_RC,
                HaloKind::Nfw => simulation::NFW_RS,
            } * 3.5
                * self.halo_rc_scale;
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

    /// Stage the disk's gas fraction immediately so the soundscape and overlays
    /// follow the slider while dragging. The running particle distribution is only
    /// rebuilt by [`set_gas_fraction`].
    pub fn stage_gas_fraction(&mut self, fraction: f32) {
        self.gas_fraction = fraction.clamp(0.0, 1.0);
    }

    /// Set the disk's gas fraction (the gas-fraction slider) and re-seed, so the
    /// blue star-forming component grows or thins. A seed-time property, like the
    /// body count.
    pub fn set_gas_fraction(&mut self, fraction: f32) {
        self.stage_gas_fraction(fraction);
        self.reseed();
    }

    /// Stage the bulge mass fraction immediately so the soundscape and rotation
    /// curve follow the slider while dragging. The running galaxy is rebuilt by
    /// [`set_bulge_fraction`].
    pub fn stage_bulge_fraction(&mut self, fraction: f32) {
        self.bulge_frac = fraction.clamp(0.0, 0.8);
    }

    /// Set the bulge mass fraction (the bulge slider) and re-seed, shifting the
    /// galaxy between disk-dominated (late-type) and bulge-dominated (early-type).
    pub fn set_bulge_fraction(&mut self, fraction: f32) {
        self.stage_bulge_fraction(fraction);
        self.reseed();
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
                gas_fraction: self.gas_fraction,
                bulge_frac: self.bulge_frac,
                count: self.particle_count,
                gravity: self.gravity,
                halo_v0: self.halo_v0,
                halo_rc_scale: self.halo_rc_scale,
                halo_kind: self.halo_kind,
            },
        );
    }

    /// Preview the body-count slider for the soundscape while dragging. The active
    /// simulation count is committed by [`set_count`] on release.
    pub fn stage_count_for_audio(&mut self, count: u32) {
        self.audio_particle_count = simulation::clamp_particle_count(count);
    }

    /// Set the body count (the body-count slider) and re-seed the current scenario
    /// at the new resolution. Clamped to a valid tile-multiple within bounds.
    pub fn set_count(&mut self, count: u32) {
        self.particle_count = simulation::clamp_particle_count(count);
        self.audio_particle_count = self.particle_count;
        self.speed = self.speed.min(max_speed_for_count(self.particle_count));
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

    /// Live dark-matter halo concentration — the scale-radius multiplier (the
    /// halo-size slider; <1 = more concentrated, >1 = more diffuse). Live: it
    /// reshapes the halo force (and the rotation curve) without re-seeding.
    pub fn set_halo_concentration(&mut self, scale: f32) {
        self.halo_rc_scale = scale.clamp(0.1, 5.0);
        self.push_physics();
    }

    fn push_physics(&self) {
        self.simulation.set_physics(
            &self.graphics.queue,
            self.scenario.softening(),
            self.gravity,
            self.halo_v0,
            self.halo_rc_scale,
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

    /// Set the star glow halo extent (the Glow slider). It reshapes the billboard
    /// falloff and colours the soundscape, but never touches the simulation or its
    /// speed.
    pub fn set_glow(&mut self, glow: f32) {
        self.glow_extent = glow.clamp(0.0, 1.0);
    }

    /// Enable or disable the cinematic autopilot (the page's Autopilot toggle, and
    /// auto-off when the user grabs the view).
    pub fn set_autopilot(&mut self, on: bool) {
        self.autopilot = on;
    }

    /// Set the cinematic autopilot speed multiplier (the page's autopilot slider).
    pub fn set_autopilot_speed(&mut self, speed: f32) {
        self.autopilot_speed = speed.clamp(0.0, 4.0);
    }

    /// Begin a composed cinematic arrangement: re-seed for a clean start, then let the
    /// arc drive the camera + live physics for `duration` seconds. The matching audio
    /// is produced separately by [`generate_piece`] from the same `seed`/`duration`.
    pub fn start_arrangement(&mut self, duration: f64, seed: u32) {
        self.autopilot = false;
        self.arrangement = Some(arrangement::Arrangement::new(duration, seed, self.scenario));
        self.arrangement_t = 0.0;
        self.reseed();
    }

    /// Stop the arrangement, leaving the view where it ended.
    pub fn stop_arrangement(&mut self) {
        self.arrangement = None;
    }

    /// One frame of the arrangement: set the camera pose absolutely and push the live
    /// physics so the galaxy performs the arc (gathers toward the peak, disperses in
    /// the resolution). Ends — and releases the camera — when the duration elapses.
    fn advance_arrangement(&mut self) {
        let Some(arr) = self.arrangement else {
            return;
        };
        self.arrangement_t += self.frame_dt as f64;
        let pose = arr.camera(self.arrangement_t);
        self.camera.scale = pose.scale.clamp(0.001, 5.0);
        self.camera.rotation_x = pose.rot_x.clamp(-1.5, 1.5);
        self.camera.rotation_y = pose.rot_y;
        let p = arr.physics(self.arrangement_t);
        let denorm = |n: f32, lo: f32, hi: f32| lo + n.clamp(0.0, 1.0) * (hi - lo);
        self.gravity = denorm(p.gravity, UI_MIN_GRAVITY, UI_MAX_GRAVITY);
        self.halo_v0 = (p.halo.clamp(0.0, 1.0) * UI_MAX_HALO_V0).max(0.0);
        self.halo_rc_scale = denorm(p.halo_size, UI_MIN_HALO_RC_SCALE, UI_MAX_HALO_RC_SCALE);
        self.glow_extent = p.glow.clamp(0.0, 1.0);
        self.particle_size = denorm(p.star_size, UI_MIN_PARTICLE_SIZE, UI_MAX_PARTICLE_SIZE);
        self.push_physics();
        if self.arrangement_t >= arr.duration {
            self.arrangement = None;
        }
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

        // Stereo bias from the orbit angle: sin(azimuth) swings smoothly L↔R through a
        // full cycle per revolution, so circling the galaxy pans the whole soundscape.
        // Lightly slewed so a fast flick still glides rather than snapping.
        self.camera_pan_s = ease_slew(self.camera_pan_s, ry.sin(), self.frame_dt, 0.3, 2.5);

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
        // Coherence = how organized the radial motion is, |Σ m·vr| / Σ m·|vr| ∈ [0,1]:
        // 1 = a unified collapse or expansion, 0 = random thermal churn. Taken from the
        // raw window sums (where the ratio is exact), independent of the scale factors.
        let coherence = (stats.flux.abs() / stats.activity.max(1e-3)).clamp(0.0, 1.0);
        // Glide toward the targets with a hard slew cap, so a sudden event (a
        // collision spike) eases in over ~2 s rather than snapping — cinematic.
        self.core_mass_s = ease_slew(self.core_mass_s, concentration, dt, 0.7, 0.5);
        self.core_activity_s = ease_slew(self.core_activity_s, activity, dt, 0.7, 0.5);
        self.core_flux_s = ease_slew(self.core_flux_s, flux, dt, 0.9, 0.6);
        self.core_coherence_s = ease_slew(self.core_coherence_s, coherence, dt, 0.8, 0.6);

        music::GalaxyState {
            scenario: self.scenario,
            zoom,
            motion: self.motion,
            speed: norm_range(self.speed, UI_MIN_SPEED, MAX_SPEED),
            intensity: (self.steps_this_frame as f32 / 8.0).clamp(0.0, 1.0),
            gravity: norm_range(self.gravity, UI_MIN_GRAVITY, UI_MAX_GRAVITY),
            halo: (self.halo_v0 / UI_MAX_HALO_V0).clamp(0.0, 1.0),
            glow: self.glow_extent.clamp(0.0, 1.0),
            star_size: norm_range(
                self.particle_size,
                UI_MIN_PARTICLE_SIZE,
                UI_MAX_PARTICLE_SIZE,
            ),
            core_mass: self.core_mass_s,
            core_flux: self.core_flux_s,
            core_activity: self.core_activity_s,
            // Sim sliders colour the sound the moment they move (even the ones that
            // re-seed the visuals on release): gas → air/brightness, bulge → body,
            // body count → starlight density, Toomre Q → pad stability, halo size →
            // reverb space.
            gas: (self.gas_fraction / UI_MAX_GAS_FRACTION).clamp(0.0, 1.0),
            bulge: (self.bulge_frac / UI_MAX_BULGE_FRAC).clamp(0.0, 1.0),
            richness: (self.audio_particle_count as f32 / simulation::MAX_PARTICLES as f32)
                .clamp(0.0, 1.0),
            stability: norm_range(self.disk_temp, UI_MIN_DISK_TEMP, UI_MAX_DISK_TEMP),
            halo_size: norm_range(
                self.halo_rc_scale,
                UI_MIN_HALO_RC_SCALE,
                UI_MAX_HALO_RC_SCALE,
            ),
            camera_pan: self.camera_pan_s,
            coherence: self.core_coherence_s,
            paused: self.paused,
        }
    }

    /// Drive the soundscape for this frame (a no-op until sound is enabled).
    pub fn update_audio(&mut self) {
        if self.audio.is_none() {
            return;
        }
        let state = self.galaxy_state();
        self.capture_frame(&state);
        if let Some(audio) = &mut self.audio {
            audio.update(&state);
        }
    }

    /// Append the current state to the export timeline when recording, throttled to
    /// [`RECORD_DT`] and capped at [`MAX_RECORD_SEC`] so memory stays bounded.
    fn capture_frame(&mut self, state: &music::GalaxyState) {
        if !self.recording_on {
            return;
        }
        let t = ((self.last_time - self.record_start_ms) / 1000.0).max(0.0) as f64;
        if let Some(&(last, _)) = self.recording.last() {
            if t - last < RECORD_DT {
                return;
            }
        }
        self.recording.push((t, *state));
        if t >= MAX_RECORD_SEC {
            self.recording_on = false; // stop at the cap; the buffer is kept to export
        }
    }

    /// Start or stop capturing the export timeline (the page's Record control). A
    /// fresh start clears the previous take and re-anchors the clock.
    pub fn set_recording(&mut self, on: bool) {
        if on && !self.recording_on {
            self.recording.clear();
            self.record_start_ms = self.last_time;
        }
        self.recording_on = on;
    }

    /// Resume the AudioContext if the engine exists — iOS suspends it when the PWA
    /// is backgrounded, and it must be resumed on return to make sound again. Leaves
    /// enabled / muted / volume untouched (the master gain already reflects them).
    pub fn resume_audio(&self) {
        if let Some(audio) = &self.audio {
            audio.resume();
        }
    }

    /// Whether the soundscape's AudioContext is actually running — lets the page keep
    /// retrying the start on later gestures until iOS lets it through.
    pub fn audio_running(&self) -> bool {
        self.audio.as_ref().is_some_and(AudioEngine::is_running)
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
            show_init_error(&e);
        }
    });

    Ok(())
}

/// Surface a fatal async init failure (e.g. WebGPU device creation failing *after*
/// the page's adapter pre-check passed) in the existing `#error` panel, so the user
/// sees a clear message instead of a silently black canvas.
fn show_init_error(err: &JsValue) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if let Some(el) = document
        .get_element_by_id("loading")
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = el.style().set_property("display", "none");
    }
    if let Some(el) = document
        .get_element_by_id("error")
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = el.style().set_property("display", "block");
    }
    if let Some(details) = document.get_element_by_id("error-details") {
        let msg = err.as_string().unwrap_or_else(|| format!("{err:?}"));
        details.set_text_content(Some(&format!("Error: {msg}")));
    }
}

/// Set the simulation speed multiplier (1.0 = real time). Called from the page's
/// speed slider. No-ops until `AppState` has finished async initialization.
#[wasm_bindgen]
pub fn set_speed(speed: f32) {
    let speed = speed.clamp(0.0, MAX_SPEED);
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            let mut app = app.borrow_mut();
            app.speed = speed.min(max_speed_for_count(app.particle_count));
        }
    });
}

/// Public helper for the page: the safe speed ceiling for a proposed body count,
/// matching the Rust scheduler's count-aware substep budget.
#[wasm_bindgen]
pub fn max_speed_for_particle_count(count: u32) -> f32 {
    max_speed_for_count(count)
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

/// Stage the gas fraction for audio/overlays while the slider is dragged. The
/// expensive reseed still happens on `set_gas_fraction`.
#[wasm_bindgen]
pub fn stage_gas_fraction(fraction: f32) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().stage_gas_fraction(fraction);
        }
    });
}

/// Set the disk's gas fraction (the gas-fraction slider, 0..1) and re-seed the
/// current scenario so the blue star-forming gas grows or thins. No-ops until ready.
#[wasm_bindgen]
pub fn set_gas_fraction(fraction: f32) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_gas_fraction(fraction);
        }
    });
}

/// Stage the bulge fraction for audio/rotation-curve feedback while the slider is
/// dragged. The expensive reseed still happens on `set_bulge_fraction`.
#[wasm_bindgen]
pub fn stage_bulge_fraction(fraction: f32) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().stage_bulge_fraction(fraction);
        }
    });
}

/// Set the bulge mass fraction (the bulge slider, 0..0.8) and re-seed, shifting the
/// galaxy between disk-dominated and bulge-dominated. No-ops until ready.
#[wasm_bindgen]
pub fn set_bulge_fraction(fraction: f32) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_bulge_fraction(fraction);
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

/// Preview the body-count slider in the soundscape while dragging. The active GPU
/// buffers are still committed by `set_particle_count` on release.
#[wasm_bindgen]
pub fn stage_particle_count(count: u32) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().stage_count_for_audio(count);
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

/// Live dark-matter halo concentration — the scale-radius multiplier (the
/// halo-size slider; <1 = more concentrated, >1 = more diffuse). Reshapes the halo
/// force and the rotation curve without re-seeding. No-ops until ready.
#[wasm_bindgen]
pub fn set_halo_concentration(scale: f32) {
    let scale = scale.clamp(0.1, 5.0);
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_halo_concentration(scale);
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

#[wasm_bindgen]
pub fn set_autopilot(on: bool) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_autopilot(on);
        }
    });
}

#[wasm_bindgen]
pub fn set_autopilot_speed(speed: f32) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_autopilot_speed(speed);
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
            let [vb, vd, vh] = scenarios::rotation_components(
                r,
                app.gravity,
                app.halo_v0,
                app.halo_rc_scale,
                app.bulge_frac,
                app.halo_kind,
            );
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

/// Megayears of simulated time per simulation time-unit. At ×1 speed the sim
/// advances one time-unit per real second, so the speed slider reads `speed ×
/// this` as Myr/s. A fixed display constant.
#[wasm_bindgen]
pub fn myr_per_unit_time() -> f32 {
    units::MYR_PER_UNIT
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

/// Resume the AudioContext after the PWA returns to the foreground (iOS suspends it
/// in the background). No-ops until the engine exists; leaves on/off + volume alone.
#[wasm_bindgen]
pub fn resume_audio() {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow().resume_audio();
        }
    });
}

/// Whether the soundscape's AudioContext is actually running (not just created).
#[wasm_bindgen]
pub fn audio_running() -> bool {
    APP_STATE.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|app| app.borrow().audio_running())
    })
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

/// Live star glow halo extent, 0..1 (the Glow slider). Render-only. No-ops until ready.
#[wasm_bindgen]
pub fn set_glow(glow: f32) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_glow(glow);
        }
    });
}

/// Start or stop capturing the soundscape timeline for the WAV export (the page's
/// Record control). Recording captures the live `GalaxyState` each frame, so the
/// export reproduces exactly what drove the sound. No-ops until ready.
#[wasm_bindgen]
pub fn set_recording(on: bool) {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().set_recording(on);
        }
    });
}

/// Whether a take is currently being captured.
#[wasm_bindgen]
pub fn is_recording() -> bool {
    APP_STATE.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|app| app.borrow().recording_on)
            .unwrap_or(false)
    })
}

/// Seconds captured in the current take (for the page's recording readout).
#[wasm_bindgen]
pub fn recording_seconds() -> f32 {
    APP_STATE.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|app| app.borrow().recording.last().map(|&(t, _)| t as f32))
            .unwrap_or(0.0)
    })
}

/// Render the recorded take to a mastered 24-bit / 48 kHz WAV and return it with an
/// analysis report. Stops recording, renders the timeline offline (faster than real
/// time, glitch-free), masters it (`src/mastering.rs`: subsonic HP, mono bass, BS.1770
/// loudness to `target_lufs`, a -1 dBTP true-peak limiter, fades), then hands the page
/// a `{ wav, report, lufs, truePeakDb, durationSec, sampleRate }` object. The page
/// makes the download. Rejects with a message if there's nothing to render.
#[wasm_bindgen]
pub async fn export_audio(target_lufs: f32) -> Result<JsValue, JsValue> {
    // Snapshot the timeline under a short borrow, released before the async render.
    let timeline = APP_STATE.with(|cell| {
        cell.borrow().as_ref().map(|app| {
            let mut a = app.borrow_mut();
            a.recording_on = false;
            a.recording.clone()
        })
    });
    let Some(timeline) = timeline else {
        return Err(JsValue::from_str("Audio is not ready yet."));
    };
    if timeline.len() < 8 {
        return Err(JsValue::from_str(
            "Nothing recorded yet — enable sound, press Record, let it play, then Export.",
        ));
    }
    let sr = EXPORT_SAMPLE_RATE;
    let (left, right) =
        audio::render_offline(&timeline, sr, audio::MASTER_LEVEL, audio::ENGINE_SEED)
            .await
            .ok_or_else(|| JsValue::from_str("Offline audio render failed in this browser."))?;
    let settings = mastering::MasterSettings {
        sample_rate: sr,
        target_lufs,
        true_peak_ceiling_db: -1.0,
    };
    let (ml, mr, report) = mastering::master(&left, &right, &settings);
    let wav = mastering::encode_wav_24(&ml, &mr, sr);
    Ok(build_export_result(&wav, &report, sr))
}

/// Render a complete, *composed* ambient piece — a deterministic A→B→C arrangement
/// (`src/arrangement.rs`) for the current scenario, varied by `seed` — to a mastered
/// 24-bit / 48 kHz WAV, with no recording needed. The matching visuals come from
/// playing the same `seed`/`duration` arrangement (`start_arrangement`), so audio and
/// picture stay locked for the video. Returns the same `{ wav, report, ... }` object
/// as [`export_audio`].
#[wasm_bindgen]
pub async fn generate_piece(
    duration_secs: f32,
    seed: u32,
    target_lufs: f32,
) -> Result<JsValue, JsValue> {
    let duration = (duration_secs as f64).clamp(20.0, MAX_RECORD_SEC);
    let scenario = APP_STATE.with(|cell| cell.borrow().as_ref().map(|app| app.borrow().scenario));
    let Some(scenario) = scenario else {
        return Err(JsValue::from_str("Audio is not ready yet."));
    };
    let arr = arrangement::Arrangement::new(duration, seed, scenario);
    let timeline = arr.timeline(RECORD_DT);
    let sr = EXPORT_SAMPLE_RATE;
    let (left, right) = audio::render_offline(&timeline, sr, audio::MASTER_LEVEL, seed as u64)
        .await
        .ok_or_else(|| JsValue::from_str("Offline audio render failed in this browser."))?;
    let settings = mastering::MasterSettings {
        sample_rate: sr,
        target_lufs,
        true_peak_ceiling_db: -1.0,
    };
    let (ml, mr, report) = mastering::master(&left, &right, &settings);
    let wav = mastering::encode_wav_24(&ml, &mr, sr);
    Ok(build_export_result(&wav, &report, sr))
}

/// Start playing a composed cinematic arrangement (drives the camera + galaxy through
/// the arc) — used to preview it and to drive the canvas for video capture. The page
/// passes the same `seed`/`duration` to [`generate_piece`] for the matching audio.
#[wasm_bindgen]
pub fn start_arrangement(duration_secs: f32, seed: u32) {
    let duration = (duration_secs as f64).clamp(20.0, MAX_RECORD_SEC);
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().start_arrangement(duration, seed);
        }
    });
}

/// Stop the cinematic arrangement, releasing the camera.
#[wasm_bindgen]
pub fn stop_arrangement() {
    APP_STATE.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            app.borrow_mut().stop_arrangement();
        }
    });
}

/// Whether a cinematic arrangement is currently playing (false once it finishes).
#[wasm_bindgen]
pub fn arrangement_active() -> bool {
    APP_STATE.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|app| app.borrow().arrangement.is_some())
    })
}

/// Whether the async GPU/app init has completed and the engine is live. The
/// `?compose` auto-play and the production tooling poll this before driving the
/// engine, since setters silently no-op until the app exists.
#[wasm_bindgen]
pub fn is_ready() -> bool {
    APP_STATE.with(|cell| cell.borrow().is_some())
}

/// Smoothed frames-per-second of the render loop. Production tooling reads this to
/// confirm a capture is holding a steady rate at a chosen body count.
#[wasm_bindgen]
pub fn fps() -> f32 {
    APP_STATE.with(|cell| {
        cell.borrow()
            .as_ref()
            .map_or(0.0, |app| app.borrow().fps_ema)
    })
}

/// A human-readable mastering summary for the export panel.
fn format_report(r: &mastering::MasterReport, sr: u32) -> String {
    format!(
        "{:.0}s · {} kHz / 24-bit WAV\nLoudness: {:.1} → {:.1} LUFS ({:+.1} dB)\nTrue peak: {:.1} dBTP · Sample peak: {:.1} dBFS\nStereo: {:.2} correlation · Tonal tilt: {:+.1} dB\n{}",
        r.duration_secs,
        sr / 1000,
        r.lufs_in,
        r.lufs_out,
        r.gain_db,
        r.true_peak_db,
        r.sample_peak_db,
        r.stereo_correlation,
        r.spectral_tilt_db,
        if r.limited {
            "Peak limiter engaged to hold the -1 dBTP ceiling."
        } else {
            "Clean headroom — no limiting needed."
        }
    )
}

/// Package the WAV bytes and report into a JS object for the page to download/show.
fn build_export_result(wav: &[u8], report: &mastering::MasterReport, sr: u32) -> JsValue {
    let obj = js_sys::Object::new();
    let bytes = js_sys::Uint8Array::new_with_length(wav.len() as u32);
    bytes.copy_from(wav);
    let _ = js_sys::Reflect::set(&obj, &"wav".into(), &bytes.into());
    let _ = js_sys::Reflect::set(&obj, &"report".into(), &format_report(report, sr).into());
    let _ = js_sys::Reflect::set(
        &obj,
        &"lufs".into(),
        &JsValue::from_f64(report.lufs_out as f64),
    );
    let _ = js_sys::Reflect::set(
        &obj,
        &"truePeakDb".into(),
        &JsValue::from_f64(report.true_peak_db as f64),
    );
    let _ = js_sys::Reflect::set(
        &obj,
        &"durationSec".into(),
        &JsValue::from_f64(report.duration_secs as f64),
    );
    let _ = js_sys::Reflect::set(&obj, &"sampleRate".into(), &JsValue::from_f64(sr as f64));
    obj.into()
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

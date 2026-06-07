//! Web Audio rendering for the generative galaxy soundscape.
//!
//! Everything is synthesized — oscillators, a code-generated reverb impulse, a
//! feedback delay, a compressor — so there are no sample files or external
//! sources. The [`MusicEngine`](crate::music) decides *what* to play from the
//! per-frame [`GalaxyState`]; this module is the *how*: it owns the AudioContext
//! and the node graph, holds a steady detuned drone pad, and schedules the
//! engine's notes ahead on the audio clock for click-free, frame-rate-independent
//! timing.
//!
//! The graph (sources → master → output):
//! ```text
//!   drone oscs ─▶ drone gain ─▶ drone LP ─┐
//!   note osc ─▶ env ─▶ panner ────────────┼─▶ master gain ─▶ master LP ─▶ comp ─▶ out
//!                          ├─▶ reverb in ─▶ convolver ─▶ reverb wet ─┤
//!                          └─▶ delay in ─▶ delay ⇄ feedback ─▶ wet ──┘
//! ```
//! Only nodes that are modulated each frame (or that notes connect to) are kept
//! in the struct; the fixed processing nodes stay alive through their graph
//! connections once wired, so they need no Rust handle.

use crate::music::{DroneTarget, GalaxyState, MusicEngine, NoteEvent, Waveform, DRONE_VOICES};
use crate::utils::console_log;
use web_sys::{
    AudioBuffer, AudioContext, BiquadFilterNode, BiquadFilterType, ConvolverNode,
    DynamicsCompressorNode, GainNode, OscillatorNode, OscillatorType, StereoPannerNode,
};

/// Look-ahead window (seconds): notes are scheduled this far in advance of the
/// audio clock, so timing is sample-accurate and independent of frame jitter.
const LOOKAHEAD_SEC: f64 = 0.25;
/// Cap on grid steps scheduled in one frame, bounding the catch-up burst after a
/// stall (e.g. a backgrounded, throttled tab).
const MAX_STEPS_PER_FRAME: u32 = 12;
/// Master level when sound is on. Kept gentle — the soundscape sits under the
/// visuals rather than over them; the compressor catches peaks.
const MASTER_LEVEL: f32 = 0.32;

/// Owns the AudioContext, the persistent node graph, and the generative engine.
pub struct AudioEngine {
    ctx: AudioContext,
    master_gain: GainNode,
    /// Global brightness filter — its cutoff tracks camera zoom.
    master_lp: BiquadFilterNode,
    reverb_in: GainNode,
    reverb_wet: GainNode,
    delay_in: GainNode,
    delay_feedback: GainNode,
    delay_wet: GainNode,
    drone_oscs: Vec<OscillatorNode>,
    drone_gain: GainNode,
    drone_lp: BiquadFilterNode,
    engine: MusicEngine,
    /// Next grid-step time on the audio clock — the scheduler's cursor.
    next_note_time: f64,
    enabled: bool,
}

impl AudioEngine {
    /// Build the audio graph and start the (silent) drone. Returns `None` if the
    /// browser denies an AudioContext (e.g. a headless or locked-down context),
    /// so the caller can run the sim silently. Must be called from within a user
    /// gesture so the context is allowed to start.
    pub fn new() -> Option<Self> {
        let ctx = AudioContext::new().ok()?;
        let _ = ctx.resume();

        // Master chain: gain → low-pass (brightness) → compressor → output.
        let master_gain = gain(&ctx, 0.0)?; // silent until enabled, then ramped up
        let master_lp = lowpass(&ctx, 1400.0, 0.7)?;
        let comp = compressor(&ctx)?;
        connect(&master_gain, &master_lp);
        connect(&master_lp, &comp);
        let _ = comp.connect_with_audio_node(&ctx.destination());

        // Reverb bus: a short, bright, procedurally-generated impulse response.
        let reverb_in = gain(&ctx, 1.0)?;
        let convolver = ConvolverNode::new(&ctx).ok()?;
        convolver.set_normalize(true);
        if let Some(ir) = make_impulse_response(&ctx, 4.5) {
            convolver.set_buffer(Some(&ir));
        }
        let reverb_wet = gain(&ctx, 0.6)?;
        connect(&reverb_in, &convolver);
        let _ = convolver.connect_with_audio_node(&reverb_wet);
        connect(&reverb_wet, &master_gain);

        // Delay bus: a band-limited feedback echo for movement.
        let delay_in = gain(&ctx, 1.0)?;
        let delay = ctx.create_delay_with_max_delay_time(2.0).ok()?;
        delay.delay_time().set_value(0.38);
        let delay_tone = lowpass(&ctx, 2200.0, 0.7)?;
        let delay_feedback = gain(&ctx, 0.4)?;
        let delay_wet = gain(&ctx, 0.22)?;
        connect(&delay_in, &delay);
        let _ = delay.connect_with_audio_node(&delay_tone);
        connect(&delay_tone, &delay_feedback);
        let _ = delay_feedback.connect_with_audio_node(&delay);
        connect(&delay_tone, &delay_wet);
        connect(&delay_wet, &master_gain);

        // Drone pad: a small set of detuned oscillators through their own filter,
        // also feeding the reverb so the pad sits in the same space as the bells.
        let drone_gain = gain(&ctx, 0.0)?;
        let drone_lp = lowpass(&ctx, 600.0, 0.6)?;
        connect(&drone_gain, &drone_lp);
        connect(&drone_lp, &master_gain);
        connect(&drone_gain, &reverb_in);
        let mut drone_oscs = Vec::with_capacity(DRONE_VOICES);
        for i in 0..DRONE_VOICES {
            let osc = OscillatorNode::new(&ctx).ok()?;
            osc.set_type(if i == 0 {
                OscillatorType::Sine
            } else {
                OscillatorType::Triangle
            });
            osc.frequency().set_value(110.0);
            osc.detune().set_value((i as f32 - 1.0) * 5.0);
            connect(&osc, &drone_gain);
            let _ = osc.start();
            drone_oscs.push(osc);
        }

        let next_note_time = ctx.current_time();
        console_log!("🔊 Audio engine ready.");
        Some(Self {
            ctx,
            master_gain,
            master_lp,
            reverb_in,
            reverb_wet,
            delay_in,
            delay_feedback,
            delay_wet,
            drone_oscs,
            drone_gain,
            drone_lp,
            engine: MusicEngine::new(0x6A1AC701),
            next_note_time,
            enabled: false,
        })
    }

    /// Turn sound on or off. Ramps the master level rather than cutting, and
    /// resumes the context on enable. The graph stays built either way.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        let now = self.ctx.current_time();
        if on {
            let _ = self.ctx.resume();
        }
        let target = if on { MASTER_LEVEL } else { 0.0 };
        // A slow fade in/out, so sound arrives and leaves gently.
        let _ = self.master_gain.gain().set_target_at_time(target, now, 0.8);
    }

    /// Apply this frame's visual state: glide the drone and global FX toward the
    /// engine's targets, then schedule any due notes ahead on the audio clock.
    pub fn update(&mut self, state: &GalaxyState) {
        let now = self.ctx.current_time();

        // Drone pad follows the scenario's chord, zoom brightness, and motion.
        let d = self.engine.drone(state);
        self.apply_drone(&d, now);

        // Whole-mix brightness from zoom (close = bright, far = muffled), and
        // reverb/delay that open up as you pull back and stir with motion.
        ramp(&self.master_lp.frequency(), d.cutoff_hz, now);
        // Generous, washy reverb — deeper still when pulled back, and opening with
        // core churn so collapses bloom.
        let reverb =
            (0.5 + 0.4 * (1.0 - state.zoom) + 0.2 * state.core_activity + 0.1 * state.motion)
                .clamp(0.0, 1.1);
        ramp(&self.reverb_wet.gain(), reverb, now);
        let delay = (0.12 + 0.3 * state.motion).clamp(0.0, 0.55);
        ramp(&self.delay_wet.gain(), delay, now);
        let feedback = (0.3 + 0.35 * state.motion).clamp(0.0, 0.7);
        ramp(&self.delay_feedback.gain(), feedback, now);

        if self.enabled && !state.paused {
            self.schedule_ahead(now, state);
        } else if self.next_note_time < now {
            // Keep the grid anchored just ahead so resuming doesn't burst a
            // backlog of missed steps.
            self.next_note_time = now + 0.1;
        }
    }

    fn apply_drone(&self, d: &DroneTarget, now: f64) {
        for (i, osc) in self.drone_oscs.iter().enumerate() {
            ramp(&osc.frequency(), d.freqs[i], now);
            let _ = osc
                .detune()
                .set_target_at_time((i as f32 - 1.0) * d.detune_cents, now, 0.6);
        }
        ramp(
            &self.drone_lp.frequency(),
            (d.cutoff_hz * 0.6).clamp(120.0, 4000.0),
            now,
        );
        let gain = if self.enabled { d.gain } else { 0.0 };
        let _ = self.drone_gain.gain().set_target_at_time(gain, now, 0.6);
    }

    /// Generate and fire every grid step inside the look-ahead window, advancing
    /// the cursor and re-anchoring if it has fallen behind (first frame, a stall,
    /// or resume). A per-frame cap bounds any catch-up burst.
    fn schedule_ahead(&mut self, now: f64, state: &GalaxyState) {
        if self.next_note_time < now {
            self.next_note_time = now + 0.05;
        }
        let horizon = now + LOOKAHEAD_SEC;
        let mut notes = Vec::new();
        let mut steps = 0;
        while self.next_note_time < horizon && steps < MAX_STEPS_PER_FRAME {
            let t = self.next_note_time;
            notes.clear();
            self.engine.generate_step(state, &mut notes);
            for ev in &notes {
                self.trigger(ev, t);
            }
            self.next_note_time = t + self.engine.step_seconds(state);
            steps += 1;
        }
    }

    /// Render one note: a fresh oscillator through a soft ADSR-ish envelope and a
    /// stereo panner, into the master bus plus the reverb and delay sends. The
    /// nodes are released once stopped — the browser keeps them alive until the
    /// envelope ends.
    fn trigger(&self, ev: &NoteEvent, when: f64) {
        let t0 = when.max(self.ctx.current_time() + 0.005);
        let (osc, env, pan) = match (
            OscillatorNode::new(&self.ctx),
            GainNode::new(&self.ctx),
            StereoPannerNode::new(&self.ctx),
        ) {
            (Ok(o), Ok(g), Ok(p)) => (o, g, p),
            _ => return,
        };
        osc.set_type(match ev.waveform {
            Waveform::Sine => OscillatorType::Sine,
            Waveform::Triangle => OscillatorType::Triangle,
        });
        let _ = osc.frequency().set_value_at_time(ev.freq, t0);
        pan.pan().set_value(ev.pan.clamp(-1.0, 1.0));

        let dur = ev.duration as f64;
        // A soft, swelling attack so notes fade in rather than pluck.
        let attack = (dur * 0.4).min(0.35);
        let peak = (ev.velocity * 0.4).max(0.0008);
        let g = env.gain();
        let _ = g.set_value_at_time(0.0001, t0);
        let _ = g.linear_ramp_to_value_at_time(peak, t0 + attack);
        let _ = g.exponential_ramp_to_value_at_time(0.0006, t0 + dur);

        connect(&osc, &env);
        connect(&env, &pan);
        connect(&pan, &self.master_gain);
        connect(&pan, &self.reverb_in);
        connect(&pan, &self.delay_in);

        let _ = osc.start_with_when(t0);
        let _ = osc.stop_with_when(t0 + dur + 0.1);
    }
}

// --- Web Audio helpers --------------------------------------------------------

/// Connect `a → b`, ignoring the (only-on-cycle) error like the rest of the graph.
fn connect(a: &web_sys::AudioNode, b: &web_sys::AudioNode) {
    let _ = a.connect_with_audio_node(b);
}

/// A gain node preset to `value`.
fn gain(ctx: &AudioContext, value: f32) -> Option<GainNode> {
    let g = GainNode::new(ctx).ok()?;
    g.gain().set_value(value);
    Some(g)
}

/// A low-pass biquad at `freq` Hz with quality `q`.
fn lowpass(ctx: &AudioContext, freq: f32, q: f32) -> Option<BiquadFilterNode> {
    let f = BiquadFilterNode::new(ctx).ok()?;
    f.set_type(BiquadFilterType::Lowpass);
    f.frequency().set_value(freq);
    f.q().set_value(q);
    Some(f)
}

/// A gentle master compressor that tames peaks from overlapping notes.
fn compressor(ctx: &AudioContext) -> Option<DynamicsCompressorNode> {
    let c = DynamicsCompressorNode::new(ctx).ok()?;
    c.threshold().set_value(-20.0);
    c.knee().set_value(18.0);
    c.ratio().set_value(3.0);
    c.attack().set_value(0.004);
    c.release().set_value(0.30);
    Some(c)
}

/// Smoothly chase an `AudioParam` toward `value` with `set_target_at_time`
/// instead of stepping it each frame (which zippers). `now` is the audio clock.
fn ramp(param: &web_sys::AudioParam, value: f32, now: f64) {
    // A long time constant so every modulation glides — the soundscape eases
    // between states rather than snapping, for a cinematic feel.
    let _ = param.set_target_at_time(value, now, 0.6);
}

/// Build a short, bright stereo reverb impulse procedurally: per-channel
/// deterministic noise under an exponential decay with a soft early emphasis.
/// No sample file — the reverb is generated in code at the context sample rate.
fn make_impulse_response(ctx: &AudioContext, seconds: f32) -> Option<AudioBuffer> {
    let sr = ctx.sample_rate();
    let len = (sr * seconds) as u32;
    let ir = ctx.create_buffer(2, len, sr).ok()?;
    let dt = 1.0 / sr;
    for ch in 0..2 {
        let mut buf = vec![0.0_f32; len as usize];
        let mut t = 0.0_f32;
        // Per-channel xorshift32 state for deterministic, decorrelated noise.
        let mut seed: u32 = if ch == 0 { 0x1234_ABCD } else { 0x7890_FEDC };
        for sample in buf.iter_mut() {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let n = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            let decay = (-t / 1.5).exp();
            let early = (1.0 - (t / 0.35)).clamp(0.0, 1.0);
            *sample = n * decay * (0.4 + 0.6 * early);
            t += dt;
        }
        let _ = ir.copy_to_channel(&buf, ch);
    }
    Some(ir)
}

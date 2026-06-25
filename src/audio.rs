//! Web Audio rendering for the generative galaxy soundscape.
//!
//! Everything is synthesized — oscillators, a noise bed, a code-generated reverb
//! impulse, a feedback delay, a compressor — so there are no sample files or
//! external sources. The [`MusicEngine`](crate::music) decides *what* to play from the
//! per-frame [`GalaxyState`]; this module is the *how*: it owns the AudioContext
//! and the node graph, holds a steady detuned drone pad, and schedules the
//! engine's notes ahead on the audio clock for click-free, frame-rate-independent
//! timing.
//!
//! The graph (sources → master → output):
//! ```text
//!   drone oscs ─▶ voice pans ─▶ drone gain ─▶ drone LP ─┬─▶ field pan ─┐
//!                                       (tap) └▶ shimmer shaper ─▶ BP ─▶ shimmer ─▶ reverb in
//!   star oscs ─▶ voice gains ─▶ star LP ─▶ star gain ───┘ (field pan) │
//!   sub osc ─▶ sub LP ─▶ sub gain ──────────────────────────────────┐│
//!   noise src ─▶ noise LP ─▶ noise gain ────────────────────────────┤│
//!   note osc ─▶ env ─▶ panner ───────────────────────────────────────┼─▶ master gain ─▶ master LP ─▶ comp ─▶ out
//!                          ├─▶ reverb in ─▶ convolver ─▶ reverb LP ─▶ reverb wet ─┤
//!                          └─▶ delay in ─▶ delay ⇄ feedback ─▶ delay wet ─────────┘
//! ```
//! The pad and starfield share a `field pan` that swings with the camera orbit; the
//! sub-bass and noise bed stay centred (mono low end). Only nodes that are modulated
//! each frame (or that notes connect to) are kept in the struct; the fixed processing
//! nodes stay alive through their graph connections once wired.

use crate::music::{
    DroneTarget, GalaxyState, MusicEngine, NoteEvent, TextureTarget, Waveform, DRONE_VOICES,
};
use crate::utils::console_log;
use web_sys::{
    AudioBuffer, AudioBufferSourceNode, AudioContext, BiquadFilterNode, BiquadFilterType,
    ConvolverNode, DynamicsCompressorNode, GainNode, OscillatorNode, OscillatorType,
    StereoPannerNode, WaveShaperNode,
};

/// Look-ahead window (seconds): notes are scheduled this far in advance of the
/// audio clock, so timing is sample-accurate and independent of frame jitter.
const LOOKAHEAD_SEC: f64 = 0.25;
/// Cap on grid steps scheduled in one frame, bounding the catch-up burst after a
/// stall (e.g. a backgrounded, throttled tab).
const MAX_STEPS_PER_FRAME: u32 = 12;
/// Master level at full volume. Kept gentle — the soundscape sits under the
/// visuals rather than over them; the compressor catches peaks. The user's
/// volume control scales this, and the page defaults that control below full.
const MASTER_LEVEL: f32 = 0.32;

/// High twinkling "starfield" voices. Each is an oscillator tuned to an upper
/// harmonic of the pad, breathing on its own slow LFO — a field of stars.
const STAR_VOICES: usize = 5;
/// Frequency multipliers (relative to the lowest pad voice, ~2 octaves below the
/// root) for the starfield — octaves and fifths high above the root, so the shimmer
/// is always consonant and tracks the scenario/gravity as the pad retunes.
const STAR_MULT: [f32; STAR_VOICES] = [32.0, 48.0, 64.0, 96.0, 128.0];

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
    /// Stereo bias for the pad + starfield, swinging with the camera orbit so the
    /// image is audibly tied to the view.
    field_pan: StereoPannerNode,
    /// Deep sub-bass foundation: one sine an octave below the lowest pad voice,
    /// retuned with the pad and swelling with the mass at the centre.
    sub_osc: OscillatorNode,
    sub_gain: GainNode,
    /// High twinkling starfield: per-voice oscillators (retuned to the pad's upper
    /// harmonics each frame) and a shared brightness filter + level.
    star_oscs: Vec<OscillatorNode>,
    star_lp: BiquadFilterNode,
    star_gain: GainNode,
    /// The slow twinkle LFOs — held only to keep them alive in the graph.
    _star_lfos: Vec<OscillatorNode>,
    /// Octave-up shimmer send into the reverb — the cosmic sheen on the pad.
    shimmer_gain: GainNode,
    /// Soft, low-passed noise bed under the pad; its level breathes with the core.
    noise_gain: GainNode,
    /// The looping noise source — held only to keep it alive in the graph.
    _noise_src: AudioBufferSourceNode,
    engine: MusicEngine,
    /// Next grid-step time on the audio clock — the scheduler's cursor.
    next_note_time: f64,
    enabled: bool,
    /// User volume in 0..1, scaling [`MASTER_LEVEL`]; driven by the volume slider.
    volume: f32,
    /// User mute, independent of `volume` and `enabled`; driven by the mute button.
    muted: bool,
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
        connect(&comp, &ctx.destination());

        // Field bus: the pad + starfield route through one stereo panner that swings
        // with the camera orbit, into the master mix and the reverb send. The sub and
        // noise stay centred (mono low end), so only the "sources" move in the space.
        let field_pan = StereoPannerNode::new(&ctx).ok()?;
        connect(&field_pan, &master_gain);

        // Reverb bus: a long, dark, diffuse impulse response — a huge cavern / the
        // void of deep space. A low-pass on the return rolls off the tail's highs so
        // it reads as a vast, distant room rather than a bright plate.
        let reverb_in = gain(&ctx, 1.0)?;
        connect(&field_pan, &reverb_in); // pad + starfield sit in the same space
        let convolver = ConvolverNode::new(&ctx).ok()?;
        convolver.set_normalize(true);
        if let Some(ir) = make_impulse_response(&ctx, 8.0) {
            convolver.set_buffer(Some(&ir));
        }
        let reverb_lp = lowpass(&ctx, 2600.0, 0.5)?;
        let reverb_wet = gain(&ctx, 0.6)?;
        connect(&reverb_in, &convolver);
        connect(&convolver, &reverb_lp);
        connect(&reverb_lp, &reverb_wet);
        connect(&reverb_wet, &master_gain);

        // Delay bus: a long, band-limited feedback echo — widely-spaced repeats that
        // trail off into the cavern. The low-pass in the loop darkens each pass.
        let delay_in = gain(&ctx, 1.0)?;
        let delay = ctx.create_delay_with_max_delay_time(2.0).ok()?;
        delay.delay_time().set_value(0.55);
        let delay_tone = lowpass(&ctx, 1600.0, 0.7)?;
        let delay_feedback = gain(&ctx, 0.4)?;
        let delay_wet = gain(&ctx, 0.22)?;
        connect(&delay_in, &delay);
        connect(&delay, &delay_tone);
        connect(&delay_tone, &delay_feedback);
        connect(&delay_feedback, &delay);
        connect(&delay_tone, &delay_wet);
        connect(&delay_wet, &master_gain);

        // Drone pad: a small set of detuned oscillators, each hard-panned for width,
        // through their own filter into the field bus (so the whole pad swings with
        // the orbit and sits in the reverb). The fundamental stays centred; the
        // detuned harmonics spread left and right for a wide, lush bed.
        let drone_gain = gain(&ctx, 0.0)?;
        let drone_lp = lowpass(&ctx, 600.0, 0.6)?;
        connect(&drone_gain, &drone_lp);
        connect(&drone_lp, &field_pan);
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
            let vp = panner(&ctx, voice_spread(i))?;
            connect(&osc, &vp);
            connect(&vp, &drone_gain);
            let _ = osc.start();
            drone_oscs.push(osc);
        }

        // Shimmer: tap the (near-sinusoidal, low-passed) pad through a frequency
        // doubler (the `2x²−1` waveshaper turns a sine into its octave) and a band-pass
        // that keeps only the high sheen, sent into the reverb — the signature cosmic
        // shimmer that rises as the core collapses and the disk brightens.
        let shimmer_shaper = octave_up_shaper(&ctx)?;
        let shimmer_bp = bandpass(&ctx, 2600.0, 0.6)?;
        let shimmer_gain = gain(&ctx, 0.0)?;
        connect(&drone_lp, &shimmer_shaper);
        connect(&shimmer_shaper, &shimmer_bp);
        connect(&shimmer_bp, &shimmer_gain);
        connect(&shimmer_gain, &reverb_in);

        // Sub-bass foundation: one sine an octave below the lowest pad voice, low-passed
        // hard and kept centred — the deep weight that makes the space feel huge.
        let sub_osc = OscillatorNode::new(&ctx).ok()?;
        sub_osc.set_type(OscillatorType::Sine);
        sub_osc.frequency().set_value(55.0);
        let sub_lp = lowpass(&ctx, 120.0, 0.5)?;
        let sub_gain = gain(&ctx, 0.0)?;
        connect(&sub_osc, &sub_lp);
        connect(&sub_lp, &sub_gain);
        connect(&sub_gain, &master_gain);
        let _ = sub_osc.start();

        // Starfield: high voices tuned to the pad's upper harmonics, each twinkling on
        // its own slow LFO (a sine into its gain), through a shared brightness filter
        // and level into the field bus — a shimmering field of stars.
        let star_lp = lowpass(&ctx, 4000.0, 0.5)?;
        let star_gain = gain(&ctx, 0.0)?;
        connect(&star_lp, &star_gain);
        connect(&star_gain, &field_pan);
        let mut star_oscs = Vec::with_capacity(STAR_VOICES);
        let mut star_lfos = Vec::with_capacity(STAR_VOICES);
        for i in 0..STAR_VOICES {
            let osc = OscillatorNode::new(&ctx).ok()?;
            osc.set_type(OscillatorType::Sine);
            osc.frequency().set_value(880.0);
            osc.detune().set_value((i as f32 - 2.0) * 4.0);
            // Per-voice gain: a base level the LFO swings around for the twinkle.
            let voice_gain = gain(&ctx, 0.55)?;
            connect(&osc, &voice_gain);
            connect(&voice_gain, &star_lp);
            let lfo = OscillatorNode::new(&ctx).ok()?;
            lfo.set_type(OscillatorType::Sine);
            lfo.frequency().set_value(0.05 + 0.031 * i as f32); // decorrelated, slow
            let depth = gain(&ctx, 0.42)?;
            connect(&lfo, &depth);
            connect_param(&depth, &voice_gain.gain());
            let _ = lfo.start();
            let _ = osc.start();
            star_oscs.push(osc);
            star_lfos.push(lfo);
        }

        // Noise bed: a soft rumble of deterministic noise under the pad — organic
        // "air" so the pure oscillators never sound sterile. A low-pass kills the
        // hiss and the level stays low, so it sits well under the visuals.
        let noise_src = AudioBufferSourceNode::new(&ctx).ok()?;
        if let Some(buf) = make_noise_buffer(&ctx, 4.0) {
            noise_src.set_buffer(Some(&buf));
        }
        noise_src.set_loop(true);
        let noise_lp = lowpass(&ctx, 420.0, 0.4)?;
        let noise_gain = gain(&ctx, 0.0)?;
        connect(&noise_src, &noise_lp);
        connect(&noise_lp, &noise_gain);
        connect(&noise_gain, &master_gain);
        let _ = noise_src.start();

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
            field_pan,
            sub_osc,
            sub_gain,
            star_oscs,
            star_lp,
            star_gain,
            _star_lfos: star_lfos,
            shimmer_gain,
            noise_gain,
            _noise_src: noise_src,
            engine: MusicEngine::new(0x6A1AC701),
            next_note_time,
            enabled: false,
            volume: 1.0,
            muted: false,
        })
    }

    /// Target master gain: the full level scaled by the user volume, but silent
    /// while disabled or muted. Everything routes through `master_gain`, so this
    /// governs the whole mix.
    fn target_level(&self) -> f32 {
        if self.enabled && !self.muted {
            MASTER_LEVEL * self.volume
        } else {
            0.0
        }
    }

    /// Resume the AudioContext (iOS suspends it when the PWA is backgrounded). Does
    /// not touch enabled / muted / volume — the master gain already reflects them.
    pub fn resume(&self) {
        let _ = self.ctx.resume();
    }

    /// Whether the AudioContext is actually running (vs suspended/closed). iOS can
    /// leave it suspended after the first gesture, so the page keeps nudging it on
    /// later interactions until this is true.
    pub fn is_running(&self) -> bool {
        self.ctx.state() == web_sys::AudioContextState::Running
    }

    /// Turn sound on or off. Ramps the master level rather than cutting, and
    /// resumes the context on enable. The graph stays built either way.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        let now = self.ctx.current_time();
        if on {
            let _ = self.ctx.resume();
        }
        // A slow fade in/out, so sound arrives and leaves gently.
        let _ = self
            .master_gain
            .gain()
            .set_target_at_time(self.target_level(), now, 0.8);
    }

    /// Set the user volume (0..1), scaling the master level. Eased quickly so a
    /// slider drag feels responsive without zippering.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        let now = self.ctx.current_time();
        let _ = self
            .master_gain
            .gain()
            .set_target_at_time(self.target_level(), now, 0.1);
    }

    /// Mute or unmute, independent of volume. Eased so it doesn't click.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        let now = self.ctx.current_time();
        let _ = self
            .master_gain
            .gain()
            .set_target_at_time(self.target_level(), now, 0.1);
    }

    /// Whether the visual core-statistics readback is currently useful. When audio
    /// is disabled or muted, skip that GPU readback entirely.
    pub fn wants_core_stats(&self) -> bool {
        self.enabled && !self.muted
    }

    /// Apply this frame's visual state: glide the drone and global FX toward the
    /// engine's targets, then schedule any due notes ahead on the audio clock.
    pub fn update(&mut self, state: &GalaxyState) {
        let now = self.ctx.current_time();

        // Slow, free-running LFOs on independent periods, so the space keeps drifting
        // even when the galaxy is momentarily still.
        let t = now as f32;
        let lfo_a = lfo(t, 0.085, 0.0); // ~74 s period
        let lfo_b = lfo(t, 0.047, 1.7); // ~134 s period, offset

        // The pad (drone + sub) and the surrounding texture both come from the engine,
        // so all of "what the galaxy should sound like" lives in `music.rs`.
        let d = self.engine.drone(state);
        self.apply_drone(&d, now);
        let tx = self.engine.texture(state, lfo_a, lfo_b);
        self.apply_texture(&d, &tx, now);

        // Whole-mix brightness from zoom (close = bright, far = muffled).
        ramp(&self.master_lp.frequency(), d.cutoff_hz, now);

        if self.enabled && !state.paused {
            self.schedule_ahead(now, state);
        } else if self.next_note_time < now {
            // Keep the grid anchored just ahead so resuming doesn't burst a
            // backlog of missed steps.
            self.next_note_time = now + 0.1;
        }
    }

    /// Glide the pad (its voices, brightness, detune, level) and the sub-bass toward
    /// this frame's [`DroneTarget`]. Held silent while sound is disabled.
    fn apply_drone(&self, d: &DroneTarget, now: f64) {
        for (i, osc) in self.drone_oscs.iter().enumerate() {
            ramp(&osc.frequency(), d.freqs[i], now);
            ramp(&osc.detune(), (i as f32 - 1.0) * d.detune_cents, now);
        }
        ramp(
            &self.drone_lp.frequency(),
            // Keep the pad warm: darker and capped lower than the global brightness,
            // so the deep hum never opens up into a buzz.
            (d.cutoff_hz * 0.45).clamp(110.0, 2400.0),
            now,
        );
        let gain = if self.enabled { d.gain } else { 0.0 };
        ramp(&self.drone_gain.gain(), gain, now);
        // Sub-bass an octave below the lowest pad voice, swelling with the pad's body.
        ramp(
            &self.sub_osc.frequency(),
            (d.freqs[0] * 0.5).clamp(20.0, 120.0),
            now,
        );
        let sub = if self.enabled { d.sub_gain } else { 0.0 };
        ramp(&self.sub_gain.gain(), sub, now);
    }

    /// Glide the ambient texture — starfield, octave-up shimmer, reverb/echo space,
    /// noise bed, pad resonance, and the orbit-driven stereo bias — toward this
    /// frame's [`TextureTarget`]. Sources are held at zero while sound is disabled.
    fn apply_texture(&self, d: &DroneTarget, tx: &TextureTarget, now: f64) {
        let on = self.enabled;
        ramp(&self.drone_lp.q(), tx.pad_resonance, now);
        ramp(&self.reverb_wet.gain(), tx.reverb_wet, now);
        ramp(&self.delay_wet.gain(), tx.delay_wet, now);
        ramp(&self.delay_feedback.gain(), tx.delay_feedback, now);
        ramp(&self.noise_gain.gain(), tx.noise_gain, now);
        ramp(&self.field_pan.pan(), tx.field_pan, now);
        ramp(
            &self.shimmer_gain.gain(),
            if on { tx.shimmer_gain } else { 0.0 },
            now,
        );

        // Starfield: track the pad's pitch (so it shifts with scenario and gravity),
        // open with the texture's brightness, and set the overall twinkle level.
        for (osc, mult) in self.star_oscs.iter().zip(STAR_MULT) {
            ramp(
                &osc.frequency(),
                (d.freqs[0] * mult).clamp(200.0, 16000.0),
                now,
            );
        }
        ramp(&self.star_lp.frequency(), tx.star_cutoff_hz, now);
        ramp(
            &self.star_gain.gain(),
            if on { tx.star_gain } else { 0.0 },
            now,
        );
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

/// Connect a node's output to an `AudioParam` (audio-rate modulation), e.g. an LFO
/// driving a gain — the signal adds to the param's intrinsic value.
fn connect_param(a: &web_sys::AudioNode, p: &web_sys::AudioParam) {
    let _ = a.connect_with_audio_param(p);
}

/// A stereo panner preset to `pan` (-1 left .. 1 right).
fn panner(ctx: &AudioContext, pan: f32) -> Option<StereoPannerNode> {
    let p = StereoPannerNode::new(ctx).ok()?;
    p.pan().set_value(pan.clamp(-1.0, 1.0));
    Some(p)
}

/// A band-pass biquad at `freq` Hz with quality `q` — keeps a narrow band, used to
/// isolate the high octave-up sheen of the shimmer.
fn bandpass(ctx: &AudioContext, freq: f32, q: f32) -> Option<BiquadFilterNode> {
    let f = BiquadFilterNode::new(ctx).ok()?;
    f.set_type(BiquadFilterType::Bandpass);
    f.frequency().set_value(freq);
    f.q().set_value(q);
    Some(f)
}

/// A waveshaper whose transfer curve is `y = 2x² − 1`: feeding it a sine doubles the
/// frequency (−cos 2θ), so a near-sinusoidal pad gains a clean octave above — the
/// basis of the shimmer reverb send.
fn octave_up_shaper(ctx: &AudioContext) -> Option<WaveShaperNode> {
    let shaper = WaveShaperNode::new(ctx).ok()?;
    let n = 1024usize;
    let mut curve = vec![0.0_f32; n];
    for (i, c) in curve.iter_mut().enumerate() {
        let x = -1.0 + 2.0 * i as f32 / (n as f32 - 1.0);
        *c = 2.0 * x * x - 1.0;
    }
    shaper.set_curve_opt_f32_slice(Some(&mut curve));
    Some(shaper)
}

/// Static stereo placement for pad voice `i`: the fundamental (0) centred, the
/// detuned harmonics spread left and right for a wide, lush bed.
fn voice_spread(i: usize) -> f32 {
    match i {
        0 => 0.0,
        1 => -0.55,
        _ => 0.55,
    }
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
    // A medium time constant keeps sliders responsive while still preventing
    // zippering and abrupt jumps in the soundscape.
    let _ = param.set_target_at_time(value, now, 0.35);
}

/// A free-running unipolar LFO in 0..1: a sine of angular rate `rate` (rad/s on the
/// audio clock) at phase `phase`, folded to 0..1. Lets the space drift on its own,
/// independent of the simulation.
fn lfo(t: f32, rate: f32, phase: f32) -> f32 {
    0.5 + 0.5 * (t * rate + phase).sin()
}

/// A tiny deterministic white-noise source: xorshift32 yielding bipolar samples in
/// [-1, 1). Seeded per channel so the two stereo sides decorrelate. Endless — the
/// buffer builders take exactly as many samples as they need.
struct Noise(u32);

impl Iterator for Noise {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        Some(self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0)
    }
}

/// Build a long, diffuse stereo reverb impulse procedurally: per-channel white
/// noise under a slow exponential decay — a big, cavernous tail. The decay time
/// constant is long (a vast space) and the early emphasis is gentle, so the energy
/// is spread out and distant rather than front-loaded and bright. No sample file —
/// the reverb is generated in code at the context sample rate.
fn make_impulse_response(ctx: &AudioContext, seconds: f32) -> Option<AudioBuffer> {
    let sr = ctx.sample_rate();
    let len = (sr * seconds) as u32;
    let ir = ctx.create_buffer(2, len, sr).ok()?;
    let dt = 1.0 / sr;
    // Two seeds → two decorrelated channels.
    for (ch, seed) in [0x1234_ABCDu32, 0x7890_FEDC].into_iter().enumerate() {
        let mut buf = vec![0.0_f32; len as usize];
        let mut t = 0.0_f32;
        for (sample, n) in buf.iter_mut().zip(Noise(seed)) {
            // Long ~3.2 s decay → a huge, slowly-fading space; a gentle early
            // emphasis over the first half-second softens the onset.
            let decay = (-t / 3.2).exp();
            let early = (1.0 - t / 0.5).clamp(0.0, 1.0);
            *sample = n * decay * (0.5 + 0.5 * early);
            t += dt;
        }
        let _ = ir.copy_to_channel(&buf, ch as i32);
    }
    Some(ir)
}

/// A short stereo noise buffer for the looping bed: per-channel white noise
/// (decorrelated L/R for width) that loops seamlessly, which the graph's low-pass
/// then warms into a soft, hiss-free rumble.
fn make_noise_buffer(ctx: &AudioContext, seconds: f32) -> Option<AudioBuffer> {
    let sr = ctx.sample_rate();
    let len = (sr * seconds) as u32;
    let buf = ctx.create_buffer(2, len, sr).ok()?;
    for (ch, seed) in [0x2545_F491u32, 0x9E37_79B9].into_iter().enumerate() {
        let data: Vec<f32> = Noise(seed).take(len as usize).collect();
        let _ = buf.copy_to_channel(&data, ch as i32);
    }
    Some(buf)
}

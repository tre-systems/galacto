//! Web Audio rendering for the generative galaxy soundscape.
//!
//! Everything is synthesized — oscillators, a noise bed, a code-generated reverb
//! impulse, a feedback delay, a compressor — so there are no sample files or
//! external sources. The [`MusicEngine`](crate::music) decides *what* to play from the
//! per-frame [`GalaxyState`]; this module is the *how*: it owns the node graph and
//! schedules notes ahead on the audio clock for click-free, frame-rate-independent
//! timing.
//!
//! The node graph itself ([`Graph`]) is independent of *which* context drives it, so
//! the same synthesis runs live in an `AudioContext` (the [`AudioEngine`]) and
//! offline in an `OfflineAudioContext` ([`render_offline`], for the composed-piece
//! render — faster than real time and glitch-free).
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
//! sub-bass and noise bed stay centred (mono low end).

use crate::music::{
    DroneTarget, GalaxyState, MusicEngine, NoteEvent, TextureTarget, Waveform, DRONE_VOICES,
};
use crate::utils::console_log;
use wasm_bindgen::JsCast;
use web_sys::{
    AudioBuffer, AudioBufferSourceNode, AudioContext, BaseAudioContext, BiquadFilterNode,
    BiquadFilterType, ConvolverNode, DynamicsCompressorNode, GainNode, OfflineAudioContext,
    OscillatorNode, OscillatorType, OverSampleType, StereoPannerNode, WaveShaperNode,
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
pub const MASTER_LEVEL: f32 = 0.32;
/// Generative engine seed — fixed so a given visual sequence sounds reproducible.
pub const ENGINE_SEED: u64 = 0x6A1AC701;
/// Extra seconds rendered past the recorded timeline so the reverb/echo tail rings
/// out instead of being chopped off at the end of an export.
const EXPORT_TAIL_SEC: f64 = 6.0;
/// Breathing-pacer rate (rad/s): a ~0.1 Hz / 10 s swell on the sustained bed, the
/// cardiovascular resonance frequency (≈6 breaths per minute) that maximises HRV
/// and parasympathetic tone — a passive cue that gently slows the listener's breath.
const BREATH_RATE: f32 = std::f32::consts::TAU / 10.0;

/// High twinkling "starfield" voices. Each is an oscillator tuned to an upper
/// harmonic of the pad, breathing on its own slow LFO — a field of stars.
const STAR_VOICES: usize = 5;
/// Frequency multipliers (relative to the lowest pad voice, ~2 octaves below the
/// root) for the starfield — octaves and fifths above the root, so it stays
/// consonant and tracks the scenario/gravity as the pad retunes. Kept below the
/// ear's fatiguing 2–5 kHz sensitivity peak (≈0.4–1.6 kHz at the default root): a
/// warm, bell-like sparkle rather than a piercing whine.
const STAR_MULT: [f32; STAR_VOICES] = [8.0, 12.0, 16.0, 24.0, 32.0];

/// Depth (cents) of the slow per-voice "analog drift" on the drone oscillators — a
/// few cents of continuous wander so the pad never sits sterilely, perfectly in tune
/// the way a digital oscillator does.
const ANALOG_DRIFT_CENTS: f32 = 4.0;

/// The persistent Web Audio node graph, independent of whether a real-time
/// `AudioContext` or an `OfflineAudioContext` drives it. Only nodes that are
/// modulated (or that notes connect to) are kept; fixed nodes stay alive through
/// their graph connections.
struct Graph {
    master_gain: GainNode,
    /// Global brightness filter — its cutoff tracks camera zoom.
    master_lp: BiquadFilterNode,
    /// Post-compressor output gain, swelling on the ~0.1 Hz breath so the whole mix
    /// breathes coherently.
    breath_gain: GainNode,
    reverb_in: GainNode,
    reverb_wet: GainNode,
    delay_in: GainNode,
    delay_feedback: GainNode,
    delay_wet: GainNode,
    drone_oscs: Vec<OscillatorNode>,
    drone_gain: GainNode,
    drone_lp: BiquadFilterNode,
    /// Stereo bias for the pad + starfield, swinging with the camera orbit.
    field_pan: StereoPannerNode,
    /// Deep sub-bass foundation, retuned with the pad.
    sub_osc: OscillatorNode,
    sub_gain: GainNode,
    /// High twinkling starfield: per-voice oscillators (retuned each frame) and a
    /// shared brightness filter + level.
    star_oscs: Vec<OscillatorNode>,
    star_lp: BiquadFilterNode,
    star_gain: GainNode,
    /// Octave-up shimmer send into the reverb — the cosmic sheen on the pad.
    shimmer_gain: GainNode,
    /// Soft, low-passed noise bed under the pad; its level breathes with the core.
    noise_gain: GainNode,
    /// Sources held only to keep them alive in the graph (twinkle LFOs, noise loop).
    _holds: Vec<AudioBufferSourceNode>,
    _lfos: Vec<OscillatorNode>,
}

impl Graph {
    /// Build the full node graph on `ctx` (real-time or offline). The master gain
    /// starts silent; the caller ramps or sets it.
    fn build(ctx: &BaseAudioContext) -> Option<Self> {
        // Master chain: gain → tape saturation → low-pass (brightness) → compressor →
        // breath → output.
        let master_gain = gain(ctx, 0.0)?;
        // Gentle tape/console saturation for analog warmth — placed before the
        // brightness low-pass so its harmonics are tamed and never turn shrill.
        let saturator = tape_saturator(ctx)?;
        let master_lp = lowpass(ctx, 1400.0, 0.7)?;
        let comp = compressor(ctx)?;
        // Post-compressor breath gain: a gentle ~0.1 Hz swell of the whole output so
        // the breathing pacer is coherent across the entire mix (placed after the
        // compressor so the swell isn't squashed). Starts at unity.
        let breath_gain = gain(ctx, 1.0)?;
        connect(&master_gain, &saturator);
        connect(&saturator, &master_lp);
        connect(&master_lp, &comp);
        connect(&comp, &breath_gain);
        connect(&breath_gain, &ctx.destination());

        // Field bus: the pad + starfield route through one stereo panner that swings
        // with the camera orbit, into the master mix and the reverb send. The sub and
        // noise stay centred (mono low end), so only the "sources" move in the space.
        let field_pan = StereoPannerNode::new(ctx).ok()?;
        connect(&field_pan, &master_gain);

        // Reverb bus: a long, dark, diffuse impulse response with early reflections —
        // a huge cavern / the void of deep space. A low-pass on the return rolls off
        // the tail's highs so it reads as a vast, distant room rather than a bright plate.
        let reverb_in = gain(ctx, 1.0)?;
        connect(&field_pan, &reverb_in); // pad + starfield sit in the same space
        let convolver = ConvolverNode::new(ctx).ok()?;
        convolver.set_normalize(true);
        if let Some(ir) = make_impulse_response(ctx, 8.0) {
            convolver.set_buffer(Some(&ir));
        }
        let reverb_lp = lowpass(ctx, 1900.0, 0.5)?;
        let reverb_wet = gain(ctx, 0.6)?;
        connect(&reverb_in, &convolver);
        connect(&convolver, &reverb_lp);
        connect(&reverb_lp, &reverb_wet);
        connect(&reverb_wet, &master_gain);

        // Delay bus: a long, band-limited feedback echo — widely-spaced repeats that
        // trail off into the cavern. The low-pass in the loop darkens each pass.
        let delay_in = gain(ctx, 1.0)?;
        let delay = ctx.create_delay_with_max_delay_time(2.0).ok()?;
        delay.delay_time().set_value(0.55);
        let delay_tone = lowpass(ctx, 1600.0, 0.7)?;
        let delay_feedback = gain(ctx, 0.4)?;
        let delay_wet = gain(ctx, 0.22)?;
        connect(&delay_in, &delay);
        connect(&delay, &delay_tone);
        connect(&delay_tone, &delay_feedback);
        connect(&delay_feedback, &delay);
        connect(&delay_tone, &delay_wet);
        connect(&delay_wet, &master_gain);

        // Drone pad: detuned oscillators, each hard-panned for width, through their
        // own filter into the field bus. The fundamental stays centred; the harmonics
        // spread left and right for a wide, lush bed.
        let drone_gain = gain(ctx, 0.0)?;
        let drone_lp = lowpass(ctx, 600.0, 0.6)?;
        connect(&drone_gain, &drone_lp);
        connect(&drone_lp, &field_pan);
        let mut drone_oscs = Vec::with_capacity(DRONE_VOICES);
        // Slow free-running LFOs (drift + starfield twinkle), kept alive together.
        let mut lfos = Vec::new();
        for i in 0..DRONE_VOICES {
            let osc = OscillatorNode::new(ctx).ok()?;
            osc.set_type(if i == 0 {
                OscillatorType::Sine
            } else {
                OscillatorType::Triangle
            });
            osc.frequency().set_value(110.0);
            osc.detune().set_value((i as f32 - 1.0) * 5.0);
            // Analog oscillator drift: a slow, per-voice wander of a few cents, summed
            // onto the set detune, so the pad is never perfectly, sterilely in tune.
            let drift = OscillatorNode::new(ctx).ok()?;
            drift.set_type(OscillatorType::Sine);
            drift.frequency().set_value(0.027 + 0.019 * i as f32); // ~0.03–0.07 Hz, decorrelated
            let drift_depth = gain(ctx, ANALOG_DRIFT_CENTS)?;
            connect(&drift, &drift_depth);
            connect_param(&drift_depth, &osc.detune());
            let _ = drift.start();
            lfos.push(drift);
            let vp = panner(ctx, voice_spread(i))?;
            connect(&osc, &vp);
            connect(&vp, &drone_gain);
            let _ = osc.start();
            drone_oscs.push(osc);
        }

        // Shimmer: tap the (near-sinusoidal, low-passed) pad through an oversampled
        // frequency doubler (the `2x²−1` waveshaper turns a sine into its octave) and a
        // band-pass that keeps only the high sheen, sent into the reverb.
        let shimmer_shaper = octave_up_shaper(ctx)?;
        // Sit the sheen up in the "air" band rather than the forward 2–3 kHz presence
        // region, so it shimmers without the fatigue that tires the ear over time.
        let shimmer_bp = bandpass(ctx, 2600.0, 0.5)?;
        let shimmer_gain = gain(ctx, 0.0)?;
        connect(&drone_lp, &shimmer_shaper);
        connect(&shimmer_shaper, &shimmer_bp);
        connect(&shimmer_bp, &shimmer_gain);
        connect(&shimmer_gain, &reverb_in);

        // Sub-bass foundation: one sine an octave below the lowest pad voice, low-passed
        // hard and kept centred — the deep weight that makes the space feel huge.
        let sub_osc = OscillatorNode::new(ctx).ok()?;
        sub_osc.set_type(OscillatorType::Sine);
        sub_osc.frequency().set_value(55.0);
        let sub_lp = lowpass(ctx, 120.0, 0.5)?;
        let sub_gain = gain(ctx, 0.0)?;
        connect(&sub_osc, &sub_lp);
        connect(&sub_lp, &sub_gain);
        connect(&sub_gain, &master_gain);
        let _ = sub_osc.start();

        // Starfield: high voices tuned to the pad's upper harmonics, each twinkling on
        // its own slow LFO (a sine into its gain), through a shared brightness filter
        // and level into the field bus.
        let star_lp = lowpass(ctx, 2400.0, 0.5)?;
        let star_gain = gain(ctx, 0.0)?;
        connect(&star_lp, &star_gain);
        connect(&star_gain, &field_pan);
        let mut star_oscs = Vec::with_capacity(STAR_VOICES);
        for i in 0..STAR_VOICES {
            let osc = OscillatorNode::new(ctx).ok()?;
            osc.set_type(OscillatorType::Sine);
            osc.frequency().set_value(880.0);
            osc.detune().set_value((i as f32 - 2.0) * 4.0);
            // Base ≈ depth, so each star's deep tremolo dips all the way to silence and
            // back — an intermittent twinkle, not a steady tone. A sustained pure tone
            // reads as a whine; one that comes and goes reads as a sparkling star.
            let voice_gain = gain(ctx, 0.5)?; // base the LFO swings around
            connect(&osc, &voice_gain);
            connect(&voice_gain, &star_lp);
            let lfo = OscillatorNode::new(ctx).ok()?;
            lfo.set_type(OscillatorType::Sine);
            lfo.frequency().set_value(0.05 + 0.031 * i as f32); // decorrelated, slow
            let depth = gain(ctx, 0.5)?;
            connect(&lfo, &depth);
            connect_param(&depth, &voice_gain.gain());
            let _ = lfo.start();
            let _ = osc.start();
            star_oscs.push(osc);
            lfos.push(lfo);
        }

        // Noise bed: a soft rumble of deterministic noise — organic "air" so the pure
        // oscillators never sound sterile. A low-pass kills the hiss.
        let noise_src = AudioBufferSourceNode::new(ctx).ok()?;
        if let Some(buf) = make_noise_buffer(ctx, 4.0) {
            noise_src.set_buffer(Some(&buf));
        }
        noise_src.set_loop(true);
        // Two cascaded low-passes turn the bed into a deep, smoothed *brown*-noise
        // rumble (~-24 dB/oct above the corner) — low-frequency-heavy with no harsh
        // top, the focus-friendly character, rather than a faint hiss-shelf.
        let noise_lp = lowpass(ctx, 240.0, 0.4)?;
        let noise_lp2 = lowpass(ctx, 480.0, 0.4)?;
        let noise_gain = gain(ctx, 0.0)?;
        connect(&noise_src, &noise_lp);
        connect(&noise_lp, &noise_lp2);
        connect(&noise_lp2, &noise_gain);
        connect(&noise_gain, &master_gain);
        let _ = noise_src.start();

        Some(Self {
            master_gain,
            master_lp,
            breath_gain,
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
            shimmer_gain,
            noise_gain,
            _holds: vec![noise_src],
            _lfos: lfos,
        })
    }

    /// Glide the pad (voices, brightness, detune, level) and the sub-bass toward this
    /// frame's [`DroneTarget`]. `on` gates the sources silent (live, while disabled).
    fn apply_drone(&self, d: &DroneTarget, on: bool, now: f64) {
        // A slow ~0.1 Hz (6 breaths/min) amplitude swell on the sustained bed — the
        // cardiovascular resonance frequency — a passive breathing pacer that nudges
        // the listener toward slow, parasympathetic breathing.
        let breath = 0.85 + 0.15 * breathing(now as f32);
        for (i, osc) in self.drone_oscs.iter().enumerate() {
            ramp(&osc.frequency(), d.freqs[i], now);
            ramp(&osc.detune(), (i as f32 - 1.0) * d.detune_cents, now);
        }
        ramp(
            &self.drone_lp.frequency(),
            // Keep the pad warm: darker and capped lower than the global brightness.
            (d.cutoff_hz * 0.45).clamp(110.0, 2400.0),
            now,
        );
        ramp(
            &self.drone_gain.gain(),
            if on { d.gain * breath } else { 0.0 },
            now,
        );
        ramp(
            &self.sub_osc.frequency(),
            // Floor at 36 Hz: below that it's infrasonic — felt by nothing, reproduced
            // by nothing, and just wasted headroom that darkens the balance.
            (d.freqs[0] * 0.5).clamp(36.0, 120.0),
            now,
        );
        ramp(
            &self.sub_gain.gain(),
            if on { d.sub_gain * breath } else { 0.0 },
            now,
        );
    }

    /// Glide the ambient texture — starfield, shimmer, reverb/echo space, noise bed,
    /// pad resonance, and the orbit-driven stereo bias — toward this frame's target.
    fn apply_texture(&self, d: &DroneTarget, tx: &TextureTarget, on: bool, now: f64) {
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
        for (osc, mult) in self.star_oscs.iter().zip(STAR_MULT) {
            ramp(
                &osc.frequency(),
                // Hold the sparkle below the ear's fatiguing 2–5 kHz peak, even when
                // the pad pitch bends up.
                (d.freqs[0] * mult).clamp(120.0, 2200.0),
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

    /// Whole-mix brightness from zoom (close = bright, far = muffled), plus the
    /// coherent ~0.1 Hz breath: a gentle swell of the whole output and a matching
    /// softening of the cutoff, so the entire mix breathes together as the pacer.
    fn apply_master_brightness(&self, cutoff_hz: f32, now: f64) {
        let b = breathing(now as f32);
        ramp(
            &self.master_lp.frequency(),
            cutoff_hz * (0.92 + 0.08 * b),
            now,
        );
        ramp(&self.breath_gain.gain(), 0.9 + 0.1 * b, now);
    }

    /// Render one note: a fresh oscillator through a soft envelope and a stereo
    /// panner, into the master bus plus the reverb and delay sends. The browser
    /// reclaims the nodes once the envelope ends. `ctx` creates the per-note nodes.
    fn trigger(&self, ctx: &BaseAudioContext, ev: &NoteEvent, when: f64) {
        let t0 = when.max(ctx.current_time() + 0.005);
        let (osc, env, pan) = match (
            OscillatorNode::new(ctx),
            GainNode::new(ctx),
            StereoPannerNode::new(ctx),
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
        // A long, soft fade-in (no pluck) — gentle onsets avoid the startle/orienting
        // response that sharp transients trigger, keeping the texture calming.
        let attack = (dur * 0.5).min(0.6);
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

/// Owns the real-time AudioContext, the node graph, and the generative engine.
pub struct AudioEngine {
    ctx: AudioContext,
    graph: Graph,
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
    /// browser denies an AudioContext. Must be called from within a user gesture.
    pub fn new() -> Option<Self> {
        let ctx = AudioContext::new().ok()?;
        let _ = ctx.resume();
        let graph = Graph::build(&ctx)?;
        let next_note_time = ctx.current_time();
        console_log!("🔊 Audio engine ready.");
        Some(Self {
            ctx,
            graph,
            engine: MusicEngine::new(ENGINE_SEED),
            next_note_time,
            enabled: false,
            volume: 1.0,
            muted: false,
        })
    }

    /// Target master gain: the full level scaled by the user volume, but silent
    /// while disabled or muted.
    fn target_level(&self) -> f32 {
        if self.enabled && !self.muted {
            MASTER_LEVEL * self.volume
        } else {
            0.0
        }
    }

    /// Resume the AudioContext (iOS suspends it when the PWA is backgrounded).
    pub fn resume(&self) {
        let _ = self.ctx.resume();
    }

    /// Whether the AudioContext is actually running (vs suspended/closed).
    pub fn is_running(&self) -> bool {
        self.ctx.state() == web_sys::AudioContextState::Running
    }

    /// Turn sound on or off. Ramps the master level rather than cutting.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        let now = self.ctx.current_time();
        if on {
            let _ = self.ctx.resume();
        }
        let _ = self
            .graph
            .master_gain
            .gain()
            .set_target_at_time(self.target_level(), now, 0.8);
    }

    /// Set the user volume (0..1), scaling the master level.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        let now = self.ctx.current_time();
        let _ = self
            .graph
            .master_gain
            .gain()
            .set_target_at_time(self.target_level(), now, 0.1);
    }

    /// Mute or unmute, independent of volume.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        let now = self.ctx.current_time();
        let _ = self
            .graph
            .master_gain
            .gain()
            .set_target_at_time(self.target_level(), now, 0.1);
    }

    /// Whether the visual core-statistics readback is currently useful.
    pub fn wants_core_stats(&self) -> bool {
        self.enabled && !self.muted
    }

    /// Apply this frame's visual state: glide the drone and texture toward the
    /// engine's targets, then schedule any due notes ahead on the audio clock.
    pub fn update(&mut self, state: &GalaxyState) {
        let now = self.ctx.current_time();
        let t = now as f32;
        let lfo_a = lfo(t, 0.085, 0.0); // ~74 s period
        let lfo_b = lfo(t, 0.047, 1.7); // ~134 s period, offset

        let d = self.engine.drone(state);
        self.graph.apply_drone(&d, self.enabled, now);
        let tx = self.engine.texture(state, lfo_a, lfo_b);
        self.graph.apply_texture(&d, &tx, self.enabled, now);
        self.graph.apply_master_brightness(d.cutoff_hz, now);

        if self.enabled && !state.paused {
            self.schedule_ahead(now, state);
        } else if self.next_note_time < now {
            self.next_note_time = now + 0.1;
        }
    }

    /// Generate and fire every grid step inside the look-ahead window, advancing the
    /// cursor and re-anchoring if it has fallen behind. A per-frame cap bounds catch-up.
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
                self.graph.trigger(&self.ctx, ev, t);
            }
            self.next_note_time = t + self.engine.step_seconds(state);
            steps += 1;
        }
    }
}

/// Render a composed-piece automation timeline to stereo `f32` offline — faster than
/// real time and glitch-free — for the composed-piece WAV. The timeline is a sequence
/// of `(seconds_from_start, GalaxyState)` from the arrangement; this replays it
/// through the same [`Graph`] at `sample_rate`, scheduling the drone/texture
/// automation and the generative notes across the whole duration up front.
///
/// `render_level` is the fixed master gain to render at (the mix level before the
/// offline master in `mastering.rs`). Returns `(left, right)`, or `None` if the
/// browser can't create the offline context.
pub async fn render_offline(
    timeline: &[(f64, GalaxyState)],
    sample_rate: u32,
    render_level: f32,
    note_seed: u64,
) -> Option<(Vec<f32>, Vec<f32>)> {
    if timeline.len() < 2 {
        return None;
    }
    let duration = timeline[timeline.len() - 1].0;
    let length = ((duration + EXPORT_TAIL_SEC) * sample_rate as f64).ceil() as u32;
    let octx = OfflineAudioContext::new_with_number_of_channels_and_length_and_sample_rate(
        2,
        length,
        sample_rate as f32,
    )
    .ok()?;
    let base: &BaseAudioContext = octx.as_ref();
    let graph = Graph::build(base)?;
    graph.master_gain.gain().set_value(render_level);

    let mut engine = MusicEngine::new(note_seed);

    // Automation pass: schedule the pad + texture targets across the timeline. The
    // engine's `drone`/`texture` don't touch its note RNG, so this is independent of
    // the note pass below.
    for (t, state) in timeline {
        let tf = *t as f32;
        let lfo_a = lfo(tf, 0.085, 0.0);
        let lfo_b = lfo(tf, 0.047, 1.7);
        let d = engine.drone(state);
        graph.apply_drone(&d, true, *t);
        let tx = engine.texture(state, lfo_a, lfo_b);
        graph.apply_texture(&d, &tx, true, *t);
        graph.apply_master_brightness(d.cutoff_hz, *t);
    }

    // Note pass: walk the note grid across the duration, generating from the state
    // active at each step and scheduling the notes at their exact times.
    let mut next = timeline[0].0;
    let mut idx = 0usize;
    let mut notes = Vec::new();
    while next < duration {
        while idx + 1 < timeline.len() && timeline[idx + 1].0 <= next {
            idx += 1;
        }
        let state = &timeline[idx].1;
        notes.clear();
        engine.generate_step(state, &mut notes);
        for ev in &notes {
            graph.trigger(base, ev, next);
        }
        next += engine.step_seconds(state);
    }

    let rendered = wasm_bindgen_futures::JsFuture::from(octx.start_rendering().ok()?)
        .await
        .ok()?;
    let buffer: AudioBuffer = rendered.dyn_into().ok()?;
    let left = buffer.get_channel_data(0).ok()?;
    let right = buffer.get_channel_data(1).ok()?;
    Some((left, right))
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

/// A gain node preset to `value`.
fn gain(ctx: &BaseAudioContext, value: f32) -> Option<GainNode> {
    let g = GainNode::new(ctx).ok()?;
    g.gain().set_value(value);
    Some(g)
}

/// A low-pass biquad at `freq` Hz with quality `q`.
fn lowpass(ctx: &BaseAudioContext, freq: f32, q: f32) -> Option<BiquadFilterNode> {
    let f = BiquadFilterNode::new(ctx).ok()?;
    f.set_type(BiquadFilterType::Lowpass);
    f.frequency().set_value(freq);
    f.q().set_value(q);
    Some(f)
}

/// A stereo panner preset to `pan` (-1 left .. 1 right).
fn panner(ctx: &BaseAudioContext, pan: f32) -> Option<StereoPannerNode> {
    let p = StereoPannerNode::new(ctx).ok()?;
    p.pan().set_value(pan.clamp(-1.0, 1.0));
    Some(p)
}

/// A band-pass biquad at `freq` Hz with quality `q` — keeps a narrow band, used to
/// isolate the high octave-up sheen of the shimmer.
fn bandpass(ctx: &BaseAudioContext, freq: f32, q: f32) -> Option<BiquadFilterNode> {
    let f = BiquadFilterNode::new(ctx).ok()?;
    f.set_type(BiquadFilterType::Bandpass);
    f.frequency().set_value(freq);
    f.q().set_value(q);
    Some(f)
}

/// A waveshaper whose transfer curve is `y = 2x² − 1`: feeding it a sine doubles the
/// frequency (−cos 2θ), so a near-sinusoidal pad gains a clean octave above. 4×
/// oversampled so the doubling doesn't alias.
/// A gentle "tape / console" soft-saturation curve: `tanh(drive·x)/drive` — unity for
/// small signals, softly rounding louder peaks and adding the low-order harmonic
/// warmth (and subtle glue) that makes a clean digital mix read as analog. Oversampled
/// so the added harmonics don't alias.
fn tape_saturator(ctx: &BaseAudioContext) -> Option<WaveShaperNode> {
    const DRIVE: f32 = 2.5;
    let shaper = WaveShaperNode::new(ctx).ok()?;
    let n = 1024usize;
    let mut curve = vec![0.0_f32; n];
    for (i, c) in curve.iter_mut().enumerate() {
        let x = -1.0 + 2.0 * i as f32 / (n as f32 - 1.0);
        *c = (DRIVE * x).tanh() / DRIVE;
    }
    shaper.set_curve_opt_f32_slice(Some(&mut curve));
    shaper.set_oversample(OverSampleType::N4x);
    Some(shaper)
}

fn octave_up_shaper(ctx: &BaseAudioContext) -> Option<WaveShaperNode> {
    let shaper = WaveShaperNode::new(ctx).ok()?;
    let n = 1024usize;
    let mut curve = vec![0.0_f32; n];
    for (i, c) in curve.iter_mut().enumerate() {
        let x = -1.0 + 2.0 * i as f32 / (n as f32 - 1.0);
        *c = 2.0 * x * x - 1.0;
    }
    shaper.set_curve_opt_f32_slice(Some(&mut curve));
    shaper.set_oversample(OverSampleType::N4x);
    Some(shaper)
}

/// A gentle master compressor that tames peaks from overlapping notes.
fn compressor(ctx: &BaseAudioContext) -> Option<DynamicsCompressorNode> {
    let c = DynamicsCompressorNode::new(ctx).ok()?;
    c.threshold().set_value(-20.0);
    c.knee().set_value(18.0);
    c.ratio().set_value(3.0);
    c.attack().set_value(0.004);
    c.release().set_value(0.30);
    Some(c)
}

/// Smoothly chase an `AudioParam` toward `value` with `set_target_at_time` instead
/// of stepping it (which zippers). `now` is the audio clock.
fn ramp(param: &web_sys::AudioParam, value: f32, now: f64) {
    let _ = param.set_target_at_time(value, now, 0.35);
}

/// A free-running unipolar LFO in 0..1: a sine of angular rate `rate` at phase
/// `phase`, folded to 0..1. Lets the space drift independent of the simulation.
fn lfo(t: f32, rate: f32, phase: f32) -> f32 {
    0.5 + 0.5 * (t * rate + phase).sin()
}

/// The breathing-pacer LFO in 0..1 at [`BREATH_RATE`] (~0.1 Hz, 6 breaths/min) — the
/// cardiovascular resonance frequency that maximises HRV and parasympathetic tone.
fn breathing(t: f32) -> f32 {
    lfo(t, BREATH_RATE, 0.0)
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

/// A tiny deterministic white-noise source: xorshift32 yielding bipolar samples in
/// [-1, 1). Seeded per channel so the two stereo sides decorrelate.
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

/// Build a long, diffuse stereo reverb impulse procedurally: a sparse set of early
/// reflections (decorrelated per channel, for spatial definition) over a slow
/// exponential noise tail — a big, cavernous space. No sample file.
fn make_impulse_response(ctx: &BaseAudioContext, seconds: f32) -> Option<AudioBuffer> {
    let sr = ctx.sample_rate();
    let len = (sr * seconds) as u32;
    let ir = ctx.create_buffer(2, len, sr).ok()?;
    let dt = 1.0 / sr;
    // Early reflections: time (s) and gain, alternating sign for diffusion. A dense,
    // slightly irregular cluster in the first ~170 ms reads as a real room with depth
    // rather than a single slap echo.
    let early = [
        (0.007, 0.70),
        (0.013, 0.55),
        (0.019, 0.62),
        (0.027, 0.45),
        (0.037, 0.50),
        (0.049, 0.38),
        (0.063, 0.42),
        (0.079, 0.30),
        (0.097, 0.33),
        (0.119, 0.24),
        (0.143, 0.26),
        (0.171, 0.19),
    ];
    for (ch, seed, jitter) in [(0usize, 0x1234_ABCDu32, 0.0_f32), (1, 0x7890_FEDC, 1.0)] {
        let mut buf = vec![0.0_f32; len as usize];
        // A one-pole low-pass whose cutoff falls over time, so the diffuse tail grows
        // darker as it decays — the defining cue of a real hall (high frequencies are
        // absorbed faster than lows). The filtering also smooths the raw noise from a
        // grainy hiss into a soft, continuous wash.
        let mut lp = 0.0_f32;
        for (i, n) in Noise(seed).take(len as usize).enumerate() {
            let t = i as f32 * dt;
            // Coefficient closes from bright (~0.54) toward dark (~0.04) over ~1 s.
            let cutoff = 0.04 + 0.5 * (-t / 1.1).exp();
            lp += cutoff * (n - lp);
            // Long ~3.2 s decay → a huge, slowly-fading space; a smooth ~15 ms onset
            // avoids a click at the very start of the impulse.
            let decay = (-t / 3.2).exp();
            let onset = 1.0 - (-t / 0.015).exp();
            buf[i] = lp * decay * onset;
        }
        // Overlay the early reflections, nudged per channel so L/R decorrelate.
        for (k, (time, g)) in early.iter().enumerate() {
            let idx = ((time + jitter * 0.0017 * (k as f32 + 1.0)) * sr) as usize;
            if idx < buf.len() {
                buf[idx] += g * if k % 2 == 0 { 1.0 } else { -1.0 };
            }
        }
        let _ = ir.copy_to_channel(&buf, ch as i32);
    }
    Some(ir)
}

/// A short stereo noise buffer for the looping bed: per-channel white noise
/// (decorrelated L/R for width) that loops seamlessly, which the graph's low-pass
/// then warms into a soft, hiss-free rumble.
fn make_noise_buffer(ctx: &BaseAudioContext, seconds: f32) -> Option<AudioBuffer> {
    let sr = ctx.sample_rate();
    let len = (sr * seconds) as u32;
    let buf = ctx.create_buffer(2, len, sr).ok()?;
    for (ch, seed) in [0x2545_F491u32, 0x9E37_79B9].into_iter().enumerate() {
        let data: Vec<f32> = Noise(seed).take(len as usize).collect();
        let _ = buf.copy_to_channel(&data, ch as i32);
    }
    Some(buf)
}

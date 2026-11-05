//! Optional live-controller bridge. The simulation, presets and offline renderer
//! keep their existing defaults and paths; composed playback rejects live edits.
use crate::{audio::AudioEngine, with_app, with_app_mut};
use wasm_bindgen::prelude::*;

fn orbit_delta(current: f32, position: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (position * tau - current + tau / 2.0).rem_euclid(tau) - tau / 2.0
}

#[wasm_bindgen]
pub fn controller_snapshot() -> Vec<f64> {
    with_app(Vec::new(), |app| {
        let mut values = vec![
            f64::from(
                app.camera.rotation_y.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU,
            ),
            f64::from((app.camera.rotation_x + 1.5) / 3.0),
            f64::from((app.camera.scale / 0.001).ln() / 5000.0_f32.ln()),
            f64::from(app.paused),
            f64::from(app.autopilot),
        ];
        values.extend(app.audio.as_ref().map_or([0.0; 4], AudioEngine::transport));
        values
    })
}

#[wasm_bindgen]
pub fn controller_camera(axis: u32, position: f32) {
    if axis > 2 || !position.is_finite() {
        return;
    }
    with_app_mut((), |app| {
        if app.arrangement.is_some() {
            return;
        }
        let position = position.clamp(0.0, 1.0);
        app.set_autopilot(false);
        match axis {
            0 => app.camera.rotation_y += orbit_delta(app.camera.rotation_y, position),
            1 => app.camera.rotation_x = position * 3.0 - 1.5,
            _ => app.camera.scale = 0.001 * 5000.0_f32.powf(position),
        }
    });
}

#[wasm_bindgen]
pub fn controller_pause(paused: bool) {
    with_app_mut((), |app| {
        if app.arrangement.is_none() {
            app.paused = paused;
        }
    });
}

#[wasm_bindgen]
pub fn controller_home() {
    with_app_mut((), |app| {
        if app.arrangement.is_none() {
            app.set_autopilot(false);
            app.camera.reset();
        }
    });
}

/// Unlock Web Audio from the Connect click without starting the music.
#[wasm_bindgen]
pub fn controller_arm() -> bool {
    with_app_mut(false, |app| {
        if app.arrangement.is_some() {
            return false;
        }
        if app.audio.is_none() {
            app.audio = AudioEngine::new();
            if let Some(audio) = &mut app.audio {
                audio.set_volume(app.sound_volume);
                audio.set_muted(app.sound_muted);
            }
        }
        app.resume_audio();
        app.audio.is_some()
    })
}

#[wasm_bindgen]
pub fn controller_play() {
    with_app_mut((), |app| {
        if app.arrangement.is_none() {
            app.paused = false;
            app.set_sound(true);
        }
    });
}

#[wasm_bindgen]
pub fn controller_fade() {
    with_app_mut((), |app| {
        if app.arrangement.is_none() {
            if let Some(audio) = &mut app.audio {
                audio.fade_out();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_wrap_uses_nearest_turn_without_a_motion_spike() {
        let tau = std::f32::consts::TAU;
        assert!((orbit_delta(tau - 0.01, 0.0) - 0.01).abs() < 1e-5);
        assert!((orbit_delta(0.01, 1.0) + 0.01).abs() < 1e-5);
        for turn in -50..50 {
            let current = turn as f32 * tau + 0.3;
            assert!(orbit_delta(current, 0.5).abs() <= tau / 2.0);
        }
    }
}

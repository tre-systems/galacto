// Optional lifecycle for a shared-context performance host.
use super::*;

#[wasm_bindgen]
pub async fn host_prepare(
    canvas: web_sys::HtmlCanvasElement,
    context: web_sys::AudioContext,
    output: web_sys::AudioNode,
) -> Result<(), JsValue> {
    if is_ready() {
        return Ok(());
    }
    let mut app = AppState::new(canvas).await?;
    app.audio = AudioEngine::hosted(context, &output);
    let Some(audio) = app.audio.as_mut() else {
        return Err(JsValue::from_str("Galacto audio initialization failed"));
    };
    audio.set_volume(app.sound_volume);
    audio.set_muted(app.sound_muted);
    APP_STATE.with(|cell| *cell.borrow_mut() = Some(Rc::new(RefCell::new(app))));
    Ok(())
}

#[wasm_bindgen]
pub fn host_tick(time: f32) -> Result<(), JsValue> {
    with_app_mut(Ok(()), |app| {
        app.update(time);
        app.update_audio();
        app.render()
    })
}

#[wasm_bindgen]
pub fn host_sound(enabled: bool) {
    with_app_mut((), |app| {
        app.last_time = 0.0;
        app.accumulator = 0.0;
        app.set_sound(enabled);
    });
}

#[wasm_bindgen]
pub fn host_resize(width: u32, height: u32) {
    with_app_mut((), |app| app.resize(width, height));
}

#[wasm_bindgen]
pub fn host_values() -> Vec<f64> {
    with_app(Vec::new(), |app| {
        vec![
            app.speed as f64,
            app.gravity as f64,
            app.halo_v0 as f64,
            app.halo_rc_scale as f64,
            app.particle_size as f64,
            app.glow_extent as f64,
            app.autopilot_speed as f64,
            app.sound_volume as f64,
            app.particle_count as f64,
            app.disk_temp as f64,
            app.gas_fraction as f64,
            app.bulge_frac as f64,
        ]
    })
}

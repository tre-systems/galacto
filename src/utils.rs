//! Panic hook and the `console_log!` bridge to the browser console.

pub fn set_panic_hook() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

#[allow(unused_macros)]
macro_rules! console_log {
    ( $( $t:tt )* ) => {
        web_sys::console::log_1(&format!( $( $t )* ).into());
    }
}

pub(crate) use console_log;

/// Report only numeric readback dimensions, never adapter descriptions or input.
/// Monitoring is optional and cannot interrupt the render loop.
#[cfg(target_arch = "wasm32")]
pub(crate) fn report_gpu_readback_failure(count: u32, bytes: u64) {
    use wasm_bindgen::{JsCast, JsValue};
    let global = js_sys::global();
    let reporter = js_sys::Reflect::get(&global, &"galactoReportGpuReadbackFailure".into())
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::Function>().ok());
    if let Some(reporter) = reporter {
        let _ = reporter.call2(
            &global,
            &JsValue::from(count),
            &JsValue::from_f64(bytes as f64),
        );
    }
}

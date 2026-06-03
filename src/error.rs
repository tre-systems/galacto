//! Error type for the FFI-free core.

use std::fmt;

/// Errors produced by the engine modules (`graphics`, `simulation`). The core
/// stays free of `wasm_bindgen::JsValue`; the boundary in `lib.rs` converts an
/// `AppError` to a `JsValue` so only that one file depends on the JS value type.
#[derive(Debug)]
pub enum AppError {
    /// WebGPU setup failed (surface, adapter, or device).
    Graphics(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Graphics(msg) => write!(f, "graphics init: {msg}"),
        }
    }
}

impl std::error::Error for AppError {}

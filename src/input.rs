use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, KeyboardEvent, MouseEvent, TouchEvent, WheelEvent};

/// Per-frame input accumulator: DOM event handlers write here, and the frame
/// loop drains it once via `update_camera` / `pause_toggled`.
#[derive(Default)]
struct InputState {
    mouse_pos: (f32, f32),
    last_mouse_pos: (f32, f32),
    is_rotating: bool,
    zoom_delta: f32,
    pause_pressed: bool,
    reset_pressed: bool,
    touch_count: u32,
    last_pinch_distance: f32,
}

fn get_pinch_distance(event: &TouchEvent) -> f32 {
    let touches = event.touches();
    if let (Some(t1), Some(t2)) = (touches.get(0), touches.get(1)) {
        let dx = t2.client_x() as f32 - t1.client_x() as f32;
        let dy = t2.client_y() as f32 - t1.client_y() as f32;
        (dx * dx + dy * dy).sqrt()
    } else {
        0.0
    }
}

/// Register `handler` as a listener for `event` on `target`, retaining the
/// closure in `closures` so it is not dropped (dropping it unregisters the
/// listener). Centralises the wrap/register/retain ceremony for every listener.
fn register(
    closures: &mut Vec<Closure<dyn FnMut(web_sys::Event)>>,
    target: &web_sys::EventTarget,
    event: &str,
    handler: impl FnMut(web_sys::Event) + 'static,
) -> Result<(), JsValue> {
    let closure = Closure::wrap(Box::new(handler) as Box<dyn FnMut(web_sys::Event)>);
    target.add_event_listener_with_callback(event, closure.as_ref().unchecked_ref())?;
    closures.push(closure);
    Ok(())
}

pub struct InputHandler {
    state: Rc<RefCell<InputState>>,
    _closures: Vec<Closure<dyn FnMut(web_sys::Event)>>,
}

impl InputHandler {
    pub fn new() -> Self {
        Self {
            state: Rc::new(RefCell::new(InputState::default())),
            _closures: Vec::new(),
        }
    }

    pub fn setup_event_listeners(&mut self, canvas: HtmlCanvasElement) -> Result<(), JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("window is unavailable"))?;
        let document = window
            .document()
            .ok_or_else(|| JsValue::from_str("document is unavailable"))?;
        let closures = &mut self._closures;

        // Left-drag orbits: arm on press, track on move, release anywhere.
        let s = self.state.clone();
        register(closures, canvas.as_ref(), "mousedown", move |event| {
            let Ok(me) = event.dyn_into::<MouseEvent>() else {
                return;
            };
            if me.button() == 0 {
                let mut state = s.borrow_mut();
                state.is_rotating = true;
                state.last_mouse_pos = (me.client_x() as f32, me.client_y() as f32);
                state.mouse_pos = state.last_mouse_pos;
            }
        })?;

        let s = self.state.clone();
        register(closures, canvas.as_ref(), "mousemove", move |event| {
            let Ok(me) = event.dyn_into::<MouseEvent>() else {
                return;
            };
            s.borrow_mut().mouse_pos = (me.client_x() as f32, me.client_y() as f32);
        })?;

        let s = self.state.clone();
        register(closures, document.as_ref(), "mouseup", move |_event| {
            s.borrow_mut().is_rotating = false;
        })?;

        let s = self.state.clone();
        register(closures, canvas.as_ref(), "wheel", move |event| {
            let Ok(we) = event.dyn_into::<WheelEvent>() else {
                return;
            };
            we.prevent_default();
            s.borrow_mut().zoom_delta = -we.delta_y() as f32;
        })?;

        // One finger orbits; two-finger pinch zooms.
        let s = self.state.clone();
        register(closures, canvas.as_ref(), "touchstart", move |event| {
            event.prevent_default();
            let Ok(te) = event.dyn_into::<TouchEvent>() else {
                return;
            };
            let mut state = s.borrow_mut();
            let touches = te.touches();
            state.touch_count = touches.length();
            if let Some(touch) = touches.get(0) {
                state.last_mouse_pos = (touch.client_x() as f32, touch.client_y() as f32);
                state.mouse_pos = state.last_mouse_pos;
                state.is_rotating = state.touch_count == 1;
            }
            if state.touch_count >= 2 {
                state.last_pinch_distance = get_pinch_distance(&te);
            }
        })?;

        let s = self.state.clone();
        register(closures, canvas.as_ref(), "touchmove", move |event| {
            event.prevent_default();
            let Ok(te) = event.dyn_into::<TouchEvent>() else {
                return;
            };
            let mut state = s.borrow_mut();
            let touches = te.touches();
            if touches.length() == 1 {
                if let Some(touch) = touches.get(0) {
                    state.mouse_pos = (touch.client_x() as f32, touch.client_y() as f32);
                }
            } else if touches.length() >= 2 {
                let new_distance = get_pinch_distance(&te);
                if state.last_pinch_distance > 0.0 {
                    state.zoom_delta = (new_distance - state.last_pinch_distance) * 5.0;
                }
                state.last_pinch_distance = new_distance;
            }
        })?;

        let s = self.state.clone();
        register(closures, canvas.as_ref(), "touchend", move |event| {
            event.prevent_default();
            let Ok(te) = event.dyn_into::<TouchEvent>() else {
                return;
            };
            let mut state = s.borrow_mut();
            state.touch_count = te.touches().length();
            if state.touch_count == 0 {
                state.is_rotating = false;
                state.last_pinch_distance = 0.0;
            }
        })?;

        // Space toggles pause, R resets the camera. Ignore OS key-repeat so a
        // held key doesn't queue repeated toggles.
        let s = self.state.clone();
        register(closures, document.as_ref(), "keydown", move |event| {
            let Ok(ke) = event.dyn_into::<KeyboardEvent>() else {
                return;
            };
            if ke.repeat() {
                return;
            }
            let mut state = s.borrow_mut();
            match ke.code().as_str() {
                "Space" => {
                    ke.prevent_default();
                    state.pause_pressed = true;
                }
                "KeyR" => state.reset_pressed = true,
                _ => {}
            }
        })?;

        Ok(())
    }

    pub fn update_camera(&self, camera: &mut crate::camera::Camera) {
        let mut state = self.state.borrow_mut();

        if state.is_rotating {
            let dx = state.mouse_pos.0 - state.last_mouse_pos.0;
            let dy = state.mouse_pos.1 - state.last_mouse_pos.1;
            if dx.abs() > 0.1 || dy.abs() > 0.1 {
                camera.rotate(dx * 0.01, dy * 0.01);
                state.last_mouse_pos = state.mouse_pos;
            }
        }

        if state.zoom_delta.abs() > 0.1 {
            camera.zoom(state.zoom_delta);
            state.zoom_delta = 0.0;
        }

        if state.reset_pressed {
            camera.reset();
            state.reset_pressed = false;
        }
    }

    pub fn pause_toggled(&self) -> bool {
        let mut state = self.state.borrow_mut();
        if state.pause_pressed {
            state.pause_pressed = false;
            true
        } else {
            false
        }
    }
}

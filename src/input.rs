use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, KeyboardEvent, MouseEvent, TouchEvent, WheelEvent};

pub struct InputState {
    pub mouse_pos: (f32, f32),
    pub last_mouse_pos: (f32, f32),
    pub is_dragging: bool,
    pub is_rotating: bool,
    pub zoom_delta: f32,
    pub pause_pressed: bool,
    pub reset_pressed: bool,
    // Touch state
    pub touch_count: u32,
    pub last_pinch_distance: f32,
}

impl InputState {
    pub fn new() -> Self {
        Self {
            mouse_pos: (0.0, 0.0),
            last_mouse_pos: (0.0, 0.0),
            is_dragging: false,
            is_rotating: false,
            zoom_delta: 0.0,
            pause_pressed: false,
            reset_pressed: false,
            touch_count: 0,
            last_pinch_distance: 0.0,
        }
    }
}

fn get_pinch_distance(event: &TouchEvent) -> f32 {
    let touches = event.touches();
    if touches.length() >= 2 {
        let t1 = touches.get(0).unwrap();
        let t2 = touches.get(1).unwrap();
        let dx = t2.client_x() as f32 - t1.client_x() as f32;
        let dy = t2.client_y() as f32 - t1.client_y() as f32;
        (dx * dx + dy * dy).sqrt()
    } else {
        0.0
    }
}

pub struct InputHandler {
    state: Rc<RefCell<InputState>>,
    _closures: Vec<Closure<dyn FnMut(web_sys::Event)>>,
}

impl InputHandler {
    pub fn new() -> Self {
        Self {
            state: Rc::new(RefCell::new(InputState::new())),
            _closures: Vec::new(),
        }
    }

    pub fn setup_event_listeners(&mut self, canvas: HtmlCanvasElement) -> Result<(), JsValue> {
        let window = web_sys::window().unwrap();
        let document = window.document().unwrap();

        // Mouse down
        {
            let state = self.state.clone();
            let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
                let mouse_event = event.dyn_into::<MouseEvent>().unwrap();
                let mut state = state.borrow_mut();

                if mouse_event.button() == 0 {
                    state.is_rotating = true;
                } else if mouse_event.button() == 2 {
                    state.is_dragging = true;
                }

                state.last_mouse_pos =
                    (mouse_event.client_x() as f32, mouse_event.client_y() as f32);
                state.mouse_pos = state.last_mouse_pos;
            }) as Box<dyn FnMut(web_sys::Event)>);

            canvas
                .add_event_listener_with_callback("mousedown", closure.as_ref().unchecked_ref())?;
            self._closures.push(closure);
        }

        // Mouse move
        {
            let state = self.state.clone();
            let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
                let mouse_event = event.dyn_into::<MouseEvent>().unwrap();
                let mut state = state.borrow_mut();
                state.mouse_pos = (mouse_event.client_x() as f32, mouse_event.client_y() as f32);
            }) as Box<dyn FnMut(web_sys::Event)>);

            canvas
                .add_event_listener_with_callback("mousemove", closure.as_ref().unchecked_ref())?;
            self._closures.push(closure);
        }

        // Mouse up
        {
            let state = self.state.clone();
            let closure = Closure::wrap(Box::new(move |_event: web_sys::Event| {
                let mut state = state.borrow_mut();
                state.is_dragging = false;
                state.is_rotating = false;
            }) as Box<dyn FnMut(web_sys::Event)>);

            document
                .add_event_listener_with_callback("mouseup", closure.as_ref().unchecked_ref())?;
            self._closures.push(closure);
        }

        // Prevent context menu
        {
            let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
                event.prevent_default();
            }) as Box<dyn FnMut(web_sys::Event)>);

            canvas.add_event_listener_with_callback(
                "contextmenu",
                closure.as_ref().unchecked_ref(),
            )?;
            self._closures.push(closure);
        }

        // Wheel zoom
        {
            let state = self.state.clone();
            let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
                let wheel_event = event.dyn_into::<WheelEvent>().unwrap();
                wheel_event.prevent_default();
                let mut state = state.borrow_mut();
                state.zoom_delta = -wheel_event.delta_y() as f32;
            }) as Box<dyn FnMut(web_sys::Event)>);

            canvas.add_event_listener_with_callback("wheel", closure.as_ref().unchecked_ref())?;
            self._closures.push(closure);
        }

        // Touch start
        {
            let state = self.state.clone();
            let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
                event.prevent_default();
                let touch_event = event.dyn_into::<TouchEvent>().unwrap();
                let mut state = state.borrow_mut();
                let touches = touch_event.touches();
                state.touch_count = touches.length();

                if let Some(touch) = touches.get(0) {
                    state.last_mouse_pos = (touch.client_x() as f32, touch.client_y() as f32);
                    state.mouse_pos = state.last_mouse_pos;
                    state.is_rotating = state.touch_count == 1;
                }

                if state.touch_count >= 2 {
                    state.last_pinch_distance = get_pinch_distance(&touch_event);
                }
            }) as Box<dyn FnMut(web_sys::Event)>);

            canvas
                .add_event_listener_with_callback("touchstart", closure.as_ref().unchecked_ref())?;
            self._closures.push(closure);
        }

        // Touch move
        {
            let state = self.state.clone();
            let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
                event.prevent_default();
                let touch_event = event.dyn_into::<TouchEvent>().unwrap();
                let mut state = state.borrow_mut();
                let touches = touch_event.touches();

                if touches.length() == 1 {
                    // Single touch: rotate
                    if let Some(touch) = touches.get(0) {
                        state.mouse_pos = (touch.client_x() as f32, touch.client_y() as f32);
                    }
                } else if touches.length() >= 2 {
                    // Pinch to zoom
                    let new_distance = get_pinch_distance(&touch_event);
                    if state.last_pinch_distance > 0.0 {
                        let delta = new_distance - state.last_pinch_distance;
                        state.zoom_delta = delta * 5.0; // Scale for sensitivity
                    }
                    state.last_pinch_distance = new_distance;
                }
            }) as Box<dyn FnMut(web_sys::Event)>);

            canvas
                .add_event_listener_with_callback("touchmove", closure.as_ref().unchecked_ref())?;
            self._closures.push(closure);
        }

        // Touch end
        {
            let state = self.state.clone();
            let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
                event.prevent_default();
                let touch_event = event.dyn_into::<TouchEvent>().unwrap();
                let mut state = state.borrow_mut();
                state.touch_count = touch_event.touches().length();
                if state.touch_count == 0 {
                    state.is_rotating = false;
                    state.last_pinch_distance = 0.0;
                }
            }) as Box<dyn FnMut(web_sys::Event)>);

            canvas
                .add_event_listener_with_callback("touchend", closure.as_ref().unchecked_ref())?;
            self._closures.push(closure);
        }

        // Keyboard
        {
            let state = self.state.clone();
            let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
                let keyboard_event = event.dyn_into::<KeyboardEvent>().unwrap();
                let mut state = state.borrow_mut();

                match keyboard_event.code().as_str() {
                    "Space" => {
                        keyboard_event.prevent_default();
                        state.pause_pressed = true;
                    }
                    "KeyR" => state.reset_pressed = true,
                    _ => {}
                }
            }) as Box<dyn FnMut(web_sys::Event)>);

            document
                .add_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref())?;
            self._closures.push(closure);
        }

        Ok(())
    }

    pub fn update_camera(&self, camera: &mut crate::camera::Camera) {
        let mut state = self.state.borrow_mut();

        if state.is_rotating {
            let delta_x = state.mouse_pos.0 - state.last_mouse_pos.0;
            let delta_y = state.mouse_pos.1 - state.last_mouse_pos.1;

            if delta_x.abs() > 0.1 || delta_y.abs() > 0.1 {
                camera.rotate(delta_x * 0.01, delta_y * 0.01);
                state.last_mouse_pos = state.mouse_pos;
            }
        }

        if state.is_dragging {
            let delta_x = state.mouse_pos.0 - state.last_mouse_pos.0;
            let delta_y = state.mouse_pos.1 - state.last_mouse_pos.1;

            if delta_x.abs() > 0.1 || delta_y.abs() > 0.1 {
                camera.pan(delta_x, delta_y);
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

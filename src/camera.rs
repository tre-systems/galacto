use cgmath::{perspective, Deg, EuclideanSpace, Matrix4, Point3, Vector3};

// Cinematic autopilot tuning at 1.0× speed — the page's autopilot slider scales
// these rates. A slow orbit, a gentle glide in and out, and a slow nod, all eased
// so switching the mode on or off never snaps the view.
const AUTOPILOT_SPIN: f32 = 0.05; // rad/s at 1.0× — ~125 s per revolution
const AUTOPILOT_ZOOM_MID: f32 = 0.7; // centre scale (matches the default view)
const AUTOPILOT_ZOOM_AMP: f32 = 0.55; // log-amplitude of the in/out glide
const AUTOPILOT_ZOOM_RATE: f32 = 0.10; // rad/s — ~63 s glide cycle
const AUTOPILOT_NOD_AMP: f32 = 0.35; // rad — gentle tilt either side of face-on
const AUTOPILOT_NOD_RATE: f32 = 0.07; // rad/s — ~90 s nod cycle
const AUTOPILOT_EASE_TAU: f32 = 1.5; // s — how gently scale/tilt chase their targets

/// Frame-rate-independent exponential ease of `current` toward `target` (time
/// constant `tau` seconds), so the autopilot glides regardless of frame rate.
fn ease(current: f32, target: f32, dt: f32, tau: f32) -> f32 {
    current + (target - current) * (1.0 - (-dt / tau.max(1e-3)).exp())
}

pub struct Camera {
    pub scale: f32,
    pub aspect_ratio: f32,
    pub rotation_x: f32,
    pub rotation_y: f32,
}

impl Camera {
    pub fn new() -> Self {
        Self {
            // Zoomed out and face-on (looking down the disk normal) so the
            // whole galactic disk sits in frame.
            scale: 0.7,
            aspect_ratio: 1.0,
            rotation_x: 0.0,
            rotation_y: 0.0,
        }
    }

    pub fn set_aspect_ratio(&mut self, aspect_ratio: f32) {
        self.aspect_ratio = aspect_ratio;
    }

    pub fn rotate(&mut self, delta_x: f32, delta_y: f32) {
        self.rotation_y += delta_x;
        self.rotation_x += delta_y;
        self.rotation_x = self.rotation_x.clamp(-1.5, 1.5);
    }

    pub fn zoom(&mut self, delta: f32) {
        // `delta` is the raw wheel/pinch delta, which varies wildly by device (a
        // few units on some mice, ~100+ on others). Map it through a bounded
        // exponential step: the bound stops big-delta mice from overshooting or
        // inverting the scale, and the coefficient keeps low-delta devices from
        // needing dozens of notches to cross the (now wide) zoom range. ~4-6
        // notches span the full range either way.
        let step = (delta * 0.1).clamp(-0.8, 0.8);
        self.scale *= step.exp();
        // Min 0.001 (distance 800000) pulls right back to a wide field of view
        // for watching the whole debris cloud disperse; max 5.0 zooms into a core.
        self.scale = self.scale.clamp(0.001, 5.0);
    }

    pub fn reset(&mut self) {
        self.scale = 0.7;
        self.rotation_x = 0.0;
        self.rotation_y = 0.0;
    }

    /// One frame of the cinematic autopilot: a slow continuous orbit, a gentle
    /// glide in and out, and a slow nod, eased toward their targets so toggling the
    /// mode never snaps the view. `dt` is the frame delta (seconds), `t` the
    /// speed-scaled phase clock (seconds), and `speed` the slider's rate multiplier.
    pub fn autopilot_step(&mut self, dt: f32, t: f32, speed: f32) {
        // Orbit: incremental at the scaled rate, so it carries on from wherever the
        // camera is and a speed change never jumps it.
        self.rotation_y += AUTOPILOT_SPIN * speed * dt;
        // Glide the zoom around a comfortable mid scale (multiplicative, so it feels
        // even across the wide range), easing toward the slowly-moving target.
        let target_scale =
            AUTOPILOT_ZOOM_MID * (AUTOPILOT_ZOOM_AMP * (t * AUTOPILOT_ZOOM_RATE).sin()).exp();
        self.scale = ease(self.scale, target_scale, dt, AUTOPILOT_EASE_TAU).clamp(0.001, 5.0);
        // A slow nod on the tilt for a more three-dimensional drift.
        let target_x = AUTOPILOT_NOD_AMP * (t * AUTOPILOT_NOD_RATE + 1.3).sin();
        self.rotation_x = ease(self.rotation_x, target_x, dt, AUTOPILOT_EASE_TAU).clamp(-1.5, 1.5);
    }

    /// The camera's right and up axes in world space — the basis for billboarding
    /// a quad to face the camera (used by the halo overlay). Mirrors the look-at
    /// basis built in `build_view_projection_matrix`.
    pub fn billboard_basis(&self) -> ([f32; 3], [f32; 3]) {
        use cgmath::InnerSpace;
        let distance = 800.0 / self.scale;
        let rot_x = cgmath::Matrix3::from_angle_x(cgmath::Rad(self.rotation_x));
        let rot_y = cgmath::Matrix3::from_angle_y(cgmath::Rad(self.rotation_y));
        let rotation = rot_y * rot_x;
        let eye = rotation * Vector3::new(0.0, 0.0, distance);
        let forward = (-eye).normalize(); // toward the origin (the camera target)
        let right = forward.cross(Vector3::unit_y()).normalize();
        let up = right.cross(forward).normalize();
        (right.into(), up.into())
    }

    pub fn build_view_projection_matrix(&self) -> Matrix4<f32> {
        let distance = 800.0 / self.scale;

        let rot_x = cgmath::Matrix3::from_angle_x(cgmath::Rad(self.rotation_x));
        let rot_y = cgmath::Matrix3::from_angle_y(cgmath::Rad(self.rotation_y));
        let rotation = rot_y * rot_x;

        let rotated_position = rotation * Vector3::new(0.0, 0.0, distance);
        let camera_pos = Point3::from_vec(rotated_position);

        let view = Matrix4::look_at_rh(camera_pos, Point3::new(0.0, 0.0, 0.0), Vector3::unit_y());
        // Far plane is generous (the rendering has no depth buffer, so there is
        // no precision cost) so escaped stars and far zoom-out never clip.
        let proj = perspective(Deg(45.0), self.aspect_ratio, 0.1, 10_000_000.0);

        proj * view
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_clamps_to_range() {
        let mut c = Camera::new();
        for _ in 0..200 {
            c.zoom(-1000.0); // zoom out hard
        }
        assert!(c.scale >= 0.001 - f32::EPSILON, "scale floor not honoured");
        for _ in 0..200 {
            c.zoom(1000.0); // zoom in hard
        }
        assert!(c.scale <= 5.0 + f32::EPSILON, "scale ceiling not honoured");
    }

    #[test]
    fn rotation_x_is_clamped() {
        let mut c = Camera::new();
        c.rotate(0.0, 100.0);
        assert!(c.rotation_x <= 1.5);
        c.rotate(0.0, -100.0);
        assert!(c.rotation_x >= -1.5);
    }

    #[test]
    fn reset_restores_defaults() {
        let mut c = Camera::new();
        c.zoom(50.0);
        c.rotate(1.0, 1.0);
        c.reset();
        assert_eq!(c.scale, 0.7);
        assert_eq!(c.rotation_x, 0.0);
        assert_eq!(c.rotation_y, 0.0);
    }

    #[test]
    fn view_projection_is_finite() {
        let mut c = Camera::new();
        c.set_aspect_ratio(16.0 / 9.0);
        let matrix = c.build_view_projection_matrix();
        let m: &[f32; 16] = matrix.as_ref();
        assert!(m.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn autopilot_stays_bounded_and_finite() {
        // Across the slider's whole speed range, scale and tilt stay in range and
        // finite over ten seconds at 60 fps.
        for &speed in &[0.0_f32, 0.4, 2.0] {
            let mut c = Camera::new();
            let mut t = 0.0;
            for _ in 0..600 {
                c.autopilot_step(1.0 / 60.0, t, speed);
                t += (1.0 / 60.0) * speed;
                assert!(
                    (0.001..=5.0).contains(&c.scale),
                    "scale {} out of range at speed {speed}",
                    c.scale
                );
                assert!((-1.5..=1.5).contains(&c.rotation_x), "tilt out of range");
                assert!(c.rotation_y.is_finite() && c.scale.is_finite());
            }
        }
    }
}

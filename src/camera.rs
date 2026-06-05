use cgmath::{perspective, Deg, EuclideanSpace, Matrix4, Point3, Vector3};

pub struct Camera {
    pub position: Vector3<f32>,
    pub scale: f32,
    pub aspect_ratio: f32,
    pub rotation_x: f32,
    pub rotation_y: f32,
}

impl Camera {
    pub fn new() -> Self {
        Self {
            position: Vector3::new(0.0, 0.0, 800.0),
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

    pub fn pan(&mut self, delta_x: f32, delta_y: f32) {
        let pan_scale = 1.0 / self.scale;
        self.position.x -= delta_x * pan_scale;
        self.position.y += delta_y * pan_scale;
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
        self.position = Vector3::new(0.0, 0.0, 800.0);
        self.scale = 0.7;
        self.rotation_x = 0.0;
        self.rotation_y = 0.0;
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
    fn pan_scales_inversely_with_zoom() {
        let mut c = Camera::new();
        c.scale = 1.0;
        c.pan(10.0, 0.0);
        // pan_scale = 1/scale = 1, so x shifts by -delta_x.
        assert!((c.position.x + 10.0).abs() < 1e-4);
    }

    #[test]
    fn reset_restores_defaults() {
        let mut c = Camera::new();
        c.zoom(50.0);
        c.rotate(1.0, 1.0);
        c.pan(10.0, 10.0);
        c.reset();
        assert_eq!(c.scale, 0.7);
        assert_eq!(c.rotation_x, 0.0);
        assert_eq!(c.rotation_y, 0.0);
        assert_eq!(c.position, Vector3::new(0.0, 0.0, 800.0));
    }

    #[test]
    fn view_projection_is_finite() {
        let mut c = Camera::new();
        c.set_aspect_ratio(16.0 / 9.0);
        let matrix = c.build_view_projection_matrix();
        let m: &[f32; 16] = matrix.as_ref();
        assert!(m.iter().all(|v| v.is_finite()));
    }
}

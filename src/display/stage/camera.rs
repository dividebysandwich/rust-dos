//! The window's camera, flown with the mouse while Ctrl+Shift is held,
//! and the projections of it and of a headset's eyes.

use super::scene::Spawn;
use glam::{Mat4, Vec3};

/// The nearest and farthest anything is drawn, in metres.
pub const NEAR: f32 = 0.05;
pub const FAR: f32 = 300.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlyCamera {
    pub position: Vec3,
    /// Radians: 0 looks along -Z, more turns left.
    pub yaw: f32,
    /// Radians up.
    pub pitch: f32,
    /// The view's height, in radians.
    pub fov_y: f32,
}

/// How far a pixel of mouse motion turns the camera, in radians, and moves
/// it, in metres.
const TURN: f32 = 0.003;
const PAN: f32 = 0.004;
const DOLLY: f32 = 0.01;

impl FlyCamera {
    pub fn at(spawn: Spawn) -> Self {
        FlyCamera { position: spawn.position, yaw: spawn.yaw, pitch: spawn.pitch, fov_y: 60f32.to_radians() }
    }

    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, sp, -cy * cp)
    }

    /// To the right, level.
    pub fn right(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        Vec3::new(cy, 0.0, -sy)
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_to_rh(self.position, self.forward(), Vec3::Y)
    }

    pub fn projection(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh_gl(self.fov_y, aspect.max(0.01), NEAR, FAR)
    }

    /// Turn by mouse motion: right turns right, down looks down.
    pub fn look(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * TURN;
        self.pitch = (self.pitch - dy * TURN).clamp(-1.55, 1.55);
    }

    /// Slide sideways and up with the mouse, as if dragging the scene.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let up = self.right().cross(self.forward());
        self.position += self.right() * (-dx * PAN) + up * (dy * PAN);
    }

    /// Move ahead (or back, below 0) by mouse motion.
    pub fn dolly(&mut self, d: f32) {
        self.position += self.forward() * (d * DOLLY);
    }

    /// Move up (or down) by `metres`.
    pub fn rise(&mut self, metres: f32) {
        self.position.y += metres;
    }
}

/// A headset eye's projection from the tangents of its field of view's
/// angles, which needn't be symmetric: left and down are negative.
pub fn fov_projection(left: f32, right: f32, up: f32, down: f32) -> Mat4 {
    let (l, r, u, d) = (left.tan(), right.tan(), up.tan(), down.tan());
    let (w, h) = (r - l, u - d);
    let (n, f) = (NEAR, FAR);
    Mat4::from_cols_array(&[
        2.0 / w,
        0.0,
        0.0,
        0.0,
        0.0,
        2.0 / h,
        0.0,
        0.0,
        (r + l) / w,
        (u + d) / h,
        -(f + n) / (f - n),
        -1.0,
        0.0,
        0.0,
        -2.0 * f * n / (f - n),
        0.0,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec4;

    #[test]
    fn symmetric_fov_is_the_usual_perspective() {
        let half = 45f32.to_radians();
        let ours = fov_projection(-half, half, half, -half);
        let usual = Mat4::perspective_rh_gl(2.0 * half, 1.0, NEAR, FAR);
        assert!(ours.abs_diff_eq(usual, 1e-5), "{:?}\n{:?}", ours, usual);
    }

    #[test]
    fn asymmetric_fov_maps_its_edges_to_the_viewport_edges() {
        let (l, r, u, d) = (-50f32.to_radians(), 40f32.to_radians(), 45f32.to_radians(), -55f32.to_radians());
        let p = fov_projection(l, r, u, d);
        let at = |x: f32, y: f32| {
            let c = p * Vec4::new(x, y, -1.0, 1.0);
            (c.x / c.w, c.y / c.w)
        };
        let (left, _) = at(l.tan(), 0.0);
        let (right, _) = at(r.tan(), 0.0);
        let (_, up) = at(0.0, u.tan());
        let (_, down) = at(0.0, d.tan());
        for (got, want) in [(left, -1.0), (right, 1.0), (up, 1.0), (down, -1.0)] {
            assert!((got - want).abs() < 1e-5, "{} {}", got, want);
        }
    }

    #[test]
    fn the_camera_turns_the_way_the_mouse_goes() {
        let mut camera = FlyCamera::at(Spawn { position: Vec3::ZERO, yaw: 0.0, pitch: 0.0 });
        assert!(camera.forward().abs_diff_eq(Vec3::NEG_Z, 1e-6));
        camera.look(100.0, 0.0);
        assert!(camera.forward().x > 0.0);
        camera.look(0.0, 100.0);
        assert!(camera.forward().y < 0.0);
        let before = camera.position;
        camera.pan(-10.0, 0.0);
        assert!((camera.position - before).dot(camera.right()) > 0.0);
    }
}

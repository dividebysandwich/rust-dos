//! Where a ray (from the eye through the mouse) meets the screen, as a
//! point of the picture.

use super::scene::Screen;
use glam::{Mat4, Vec2, Vec3, Vec4};

/// The ray through a point of a view, `ndc` from -1 to 1 across and up,
/// from the inverse of its view-projection: its origin and direction.
pub fn ray(inverse_view_projection: Mat4, ndc: Vec2) -> (Vec3, Vec3) {
    let point = |z: f32| {
        let p = inverse_view_projection * Vec4::new(ndc.x, ndc.y, z, 1.0);
        p.truncate() / p.w
    };
    let (near, far) = (point(-1.0), point(1.0));
    (near, (far - near).normalize_or_zero())
}

/// How far along the ray it meets the triangle, and where on it (the
/// weights of the second and third corner), from either side.
fn hit_triangle(origin: Vec3, dir: Vec3, [a, b, c]: [Vec3; 3]) -> Option<(f32, f32, f32)> {
    let (e1, e2) = (b - a, c - a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some((t, u, v))
}

/// The point of the picture (0 to 1 across and down) the ray meets first
/// on the screen, if it meets it.
pub fn pick(screen: &Screen, origin: Vec3, dir: Vec3) -> Option<Vec2> {
    pick_hit(screen, origin, dir).map(|(_, uv)| uv)
}

/// `pick`, with how far along the ray (in its direction's lengths) the
/// screen is.
pub fn pick_hit(screen: &Screen, origin: Vec3, dir: Vec3) -> Option<(f32, Vec2)> {
    let mut best: Option<(f32, Vec2)> = None;
    for &[(a, ta), (b, tb), (c, tc)] in &screen.triangles {
        if let Some((t, u, v)) = hit_triangle(origin, dir, [a, b, c])
            && best.is_none_or(|(nearest, _)| t < nearest)
        {
            best = Some((t, ta * (1.0 - u - v) + tb * u + tc * v));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::super::scene::Scene;
    use super::*;

    #[test]
    fn rays_find_the_point_of_the_picture() {
        let room = Scene::test_room();
        let screen = &room.screen;
        // Straight at the middle, and at the top left corner's quarter.
        let eye = Vec3::new(0.0, 1.4, 0.0);
        let uv = pick(screen, eye, Vec3::NEG_Z).unwrap();
        assert!(uv.abs_diff_eq(Vec2::new(0.5, 0.5), 1e-4), "{:?}", uv);
        let quarter = screen.center + Vec3::new(-0.4, 0.3, 0.0);
        let uv = pick(screen, eye, (quarter - eye).normalize()).unwrap();
        assert!(uv.abs_diff_eq(Vec2::new(0.25, 0.25), 1e-4), "{:?}", uv);
        // Away from it, or behind.
        assert!(pick(screen, eye, Vec3::new(1.0, 0.0, -1.0).normalize()).is_none());
        assert!(pick(screen, eye, Vec3::Z).is_none());
    }

    #[test]
    fn rays_go_through_the_view() {
        let view = Mat4::look_to_rh(Vec3::new(0.0, 1.4, 0.0), Vec3::NEG_Z, Vec3::Y);
        let projection = Mat4::perspective_rh_gl(1.0, 1.5, 0.05, 300.0);
        let (origin, dir) = ray((projection * view).inverse(), Vec2::ZERO);
        assert!(dir.abs_diff_eq(Vec3::NEG_Z, 1e-4), "{:?}", dir);
        assert!((origin - Vec3::new(0.0, 1.4, -0.05)).length() < 1e-3, "{:?}", origin);
        let (_, right) = ray((projection * view).inverse(), Vec2::new(1.0, 0.0));
        assert!(right.x > 0.0);
    }
}

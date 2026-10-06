//! Shadows: how far the nearest surface is, seen from each light and from
//! the screen, drawn once into the layers of one depth texture, since
//! nothing that casts a shadow moves. Directional lights see the scene's
//! focus (`Scene::focus_bounds`) along their direction, narrow spots
//! through their cone, and points, wide spots and the screen through the
//! faces of a cube around them.

use super::scene::{LightKind, Scene};
use glam::{Mat3, Mat4, Vec3};

/// At most this many of the scene's lights cast shadows.
pub const MAX_SHADOWED: usize = 4;

/// How far in front of the glass's middle the screen's light is seen from.
pub const SCREEN_OFFSET: f32 = 0.01;

/// A cube's faces are this much wider than a quarter turn, so that the
/// samples around a point near an edge are all on its face.
const CUBE_FOV: f32 = 1.66;

/// What one layer of the texture sees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    pub view_projection: Mat4,
    /// A texel's width on what it sees, in metres: this much, plus the
    /// second times the distance from the light.
    pub texel: [f32; 2],
}

/// How a light (or the screen) is shadowed: by one layer, or by six from
/// its position along the axes of `basis` (+x, -x, +y, -y, +z, -z), of
/// which those not drawn are `None` and never in shadow.
#[derive(Clone, Debug, PartialEq)]
pub enum Caster {
    Single(Layer),
    Cube { basis: Mat3, faces: [Option<Layer>; 6] },
}

/// The shadows' layers for `scene`: those of its first `MAX_SHADOWED`
/// shadowed lights, by the light's index, and the screen's.
pub fn plan(scene: &Scene) -> (Vec<(usize, Caster)>, Option<Caster>) {
    if !scene.shadows {
        return (Vec::new(), None);
    }
    let (low, high) = scene.focus_bounds();
    let corners: Vec<Vec3> =
        (0..8).map(|i| Vec3::new([low.x, high.x][i & 1], [low.y, high.y][(i >> 1) & 1], [low.z, high.z][i >> 2])).collect();
    let mut lights = Vec::new();
    for (i, light) in scene.lights.iter().enumerate() {
        if !light.shadow || lights.len() == MAX_SHADOWED {
            continue;
        }
        let caster = match light.kind {
            LightKind::Directional => directional(light.direction, &corners),
            LightKind::Spot { outer_cos, .. } if outer_cos > 70f32.to_radians().cos() => {
                let fov = (outer_cos.clamp(-1.0, 1.0).acos() * 2.0 + 2f32.to_radians()).min(150f32.to_radians());
                Caster::Single(perspective(light.position, light.direction, fov, &corners, low, high))
            }
            _ => cube(light.position, Mat3::IDENTITY, [true; 6], &corners, low, high),
        };
        lights.push((i, caster));
    }
    let screen = &scene.screen;
    let screen = (screen.size.x > 0.0).then(|| {
        let basis = Mat3::from_cols(screen.right, screen.up(), screen.normal);
        // Nothing behind the glass is lit by it.
        let faces = [true, true, true, true, true, false];
        cube(screen.center + screen.normal * SCREEN_OFFSET, basis, faces, &corners, low, high)
    });
    (lights, screen)
}

/// Some direction square to `d`, for the up of a view along it.
fn any_up(d: Vec3) -> Vec3 {
    if d.y.abs() < 0.95 { Vec3::Y } else { Vec3::Z }
}

/// Parallel light along `direction`, over the whole of the corners' box.
fn directional(direction: Vec3, corners: &[Vec3]) -> Caster {
    let direction = direction.normalize_or(Vec3::NEG_Y);
    let middle = corners.iter().copied().sum::<Vec3>() / corners.len() as f32;
    let view = Mat4::look_to_rh(middle, direction, any_up(direction));
    let (mut low, mut high) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for &c in corners {
        let p = view.transform_point3(c);
        low = low.min(p);
        high = high.max(p);
    }
    // Looking down -z: the nearest is the highest z.
    let projection = Mat4::orthographic_rh_gl(low.x, high.x, low.y, high.y, -high.z - 0.1, -low.z + 0.1);
    let texel = (high.x - low.x).max(high.y - low.y);
    Caster::Single(Layer { view_projection: projection * view, texel: [texel, 0.0] })
}

/// The near and far distances for a view from `at` that takes in the box.
fn depth_range(at: Vec3, corners: &[Vec3], low: Vec3, high: Vec3) -> (f32, f32) {
    let far = corners.iter().map(|c| c.distance(at)).fold(1.0f32, f32::max) * 1.05;
    let outside = (low - at).max(at - high).max(Vec3::ZERO).length();
    let near = (outside * 0.9).max(0.005);
    (near, far.max(near * 2.0))
}

/// A view from `at` along `direction`, `fov` wide both ways.
fn perspective(at: Vec3, direction: Vec3, fov: f32, corners: &[Vec3], low: Vec3, high: Vec3) -> Layer {
    let direction = direction.normalize_or(Vec3::NEG_Y);
    let (near, far) = depth_range(at, corners, low, high);
    let view = Mat4::look_to_rh(at, direction, any_up(direction));
    let projection = Mat4::perspective_rh_gl(fov, 1.0, near, far);
    Layer { view_projection: projection * view, texel: [0.0, 2.0 * (fov / 2.0).tan()] }
}

/// The faces of a cube at `at`, turned by `basis`, that are `drawn`.
fn cube(at: Vec3, basis: Mat3, drawn: [bool; 6], corners: &[Vec3], low: Vec3, high: Vec3) -> Caster {
    let axes = [basis.x_axis, -basis.x_axis, basis.y_axis, -basis.y_axis, basis.z_axis, -basis.z_axis];
    let mut faces = [None; 6];
    for (face, axis) in axes.into_iter().enumerate() {
        if drawn[face] {
            faces[face] = Some(perspective(at, axis, CUBE_FOV, corners, low, high));
        }
    }
    Caster::Cube { basis, faces }
}

impl Caster {
    /// Its layers in order, the cube's faces not drawn left out.
    pub fn layers(&self) -> Vec<Layer> {
        match self {
            Caster::Single(layer) => vec![*layer],
            Caster::Cube { faces, .. } => faces.iter().flatten().copied().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_room_is_shadowed_by_the_sun_and_the_screen() {
        let scene = Scene::test_room();
        let (lights, screen) = plan(&scene);
        assert_eq!(lights.len(), 1);
        assert!(matches!(lights[0].1, Caster::Single(_)));
        let Some(Caster::Cube { faces, .. }) = screen else { panic!("the screen has no cube") };
        assert_eq!(faces.iter().flatten().count(), 5);
        assert!(faces[5].is_none());
    }

    #[test]
    fn the_sun_sees_the_whole_focus() {
        let scene = Scene::test_room();
        let (lights, _) = plan(&scene);
        let Caster::Single(layer) = lights[0].1 else { panic!() };
        let (low, high) = scene.focus_bounds();
        for i in 0..8 {
            let c = Vec3::new([low.x, high.x][i & 1], [low.y, high.y][(i >> 1) & 1], [low.z, high.z][i >> 2]);
            let p = layer.view_projection.project_point3(c);
            assert!(p.abs().max_element() <= 1.0001, "{:?} is outside: {:?}", c, p);
        }
    }

    #[test]
    fn the_screen_sees_what_is_in_front_of_it() {
        let scene = Scene::test_room();
        let (_, Some(Caster::Cube { faces, .. })) = plan(&scene) else { panic!() };
        // Straight ahead of the screen: its front face.
        let ahead = scene.screen.center + scene.screen.normal * 1.5;
        let p = faces[4].unwrap().view_projection.project_point3(ahead);
        assert!(p.x.abs() < 1e-3 && p.y.abs() < 1e-3 && p.z.abs() < 1.0, "{:?}", p);
    }

    #[test]
    fn unshadowed_lights_and_scenes_have_none() {
        let mut scene = Scene::test_room();
        scene.lights[0].shadow = false;
        assert!(plan(&scene).0.is_empty());
        scene.shadows = false;
        assert_eq!(plan(&scene), (Vec::new(), None));
    }
}

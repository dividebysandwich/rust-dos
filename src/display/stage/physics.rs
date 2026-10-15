//! What is let go falls: a box that tumbles, bounces off and slides on the
//! scene's surfaces and the other props, until it lies still.
//!
//! Its corners meet what is between them and its middle (so that it rests
//! on its faces and edges), and its middle doesn't pass through anything
//! (so that a quick throw doesn't go through the desk).

use super::pick::hit_triangle;
use super::scene::Scene;
use glam::{Mat3, Mat4, Quat, Vec3};
use std::collections::HashMap;

const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);
/// The time a step simulates, and the most simulated at a frame.
pub const STEP: f32 = 1.0 / 360.0;
pub const MAX_LAG: f32 = 0.05;
/// How much of its speed into a surface bounces back, above `BOUNCING`;
/// how hard it rubs along it.
const BOUNCE: f32 = 0.3;
const BOUNCING: f32 = 0.6;
const FRICTION: f32 = 0.5;
/// How many times over the pushes at the points it touches are worked out.
const ITERATIONS: usize = 8;
/// How much of its speed and spin it loses a second while it touches
/// something.
const DRAG: f32 = 2.0;
/// Slower than this, and for that long, touching something, it lies still.
const STILL_SPEED: f32 = 0.05;
const STILL_SPIN: f32 = 0.5;
const STILL_TIME: f32 = 0.25;
/// It is stopped where it is after this long anyway.
const LIFE: f32 = 20.0;
/// The fastest it is thrown, in metres and radians a second.
pub const MAX_SPEED: f32 = 8.0;
pub const MAX_SPIN: f32 = 25.0;
/// How far out of what it met it is put, and how far off what it is about
/// to meet is looked for.
const SKIN: f32 = 0.0005;
const NEARBY: f32 = 0.005;
/// The side of the cubes the scene's triangles are sorted into.
const CELL: f32 = 0.1;
const LARGE: i64 = 512;

/// The scene's surfaces (those that cast shadows), and the boxes around
/// them, sorted into cubes.
#[derive(Default)]
pub struct Colliders {
    triangles: Vec<[Vec3; 3]>,
    boxes: Vec<(Vec3, Vec3)>,
    cells: HashMap<(i32, i32, i32), Vec<u32>>,
    /// Those over more than `LARGE` cubes, looked at always.
    large: Vec<u32>,
    /// Below the lowest of them.
    pub bottom: f32,
}

impl Colliders {
    pub fn new(scene: &Scene) -> Self {
        let mut colliders = Colliders { bottom: f32::MAX, ..Colliders::default() };
        for mesh in scene.meshes.iter().filter(|m| scene.materials[m.material].casts_shadow()) {
            for tri in mesh.indices.as_chunks::<3>().0 {
                let corners = [tri[0], tri[1], tri[2]].map(|i| Vec3::from(mesh.vertices[i as usize].position));
                let index = colliders.triangles.len() as u32;
                let low = corners[0].min(corners[1]).min(corners[2]);
                let high = corners[0].max(corners[1]).max(corners[2]);
                colliders.triangles.push(corners);
                colliders.boxes.push((low, high));
                colliders.bottom = colliders.bottom.min(low.y);
                let ((x0, y0, z0), (x1, y1, z1)) = (cell(low), cell(high));
                let cubes = (x1 - x0 + 1) as i64 * (y1 - y0 + 1) as i64 * (z1 - z0 + 1) as i64;
                if cubes > LARGE {
                    colliders.large.push(index);
                    continue;
                }
                for x in x0..=x1 {
                    for y in y0..=y1 {
                        for z in z0..=z1 {
                            colliders.cells.entry((x, y, z)).or_default().push(index);
                        }
                    }
                }
            }
        }
        if colliders.triangles.is_empty() {
            colliders.bottom = 0.0;
        }
        colliders.bottom -= 2.0;
        colliders
    }

    /// The first surface the line from `from` to `to` crosses: how far
    /// along it (0 to 1), and the surface's normal, facing `from`.
    pub fn cross(&self, from: Vec3, to: Vec3) -> Option<(f32, Vec3)> {
        let along = to - from;
        let length = along.length();
        if length < 1e-7 {
            return None;
        }
        let dir = along / length;
        let (low, high) = (from.min(to), from.max(to));
        let ((x0, y0, z0), (x1, y1, z1)) = (cell(low), cell(high));
        let mut nearest: Option<(f32, u32)> = None;
        let mut meet = |i: u32| {
            let (l, h) = self.boxes[i as usize];
            if l.cmpgt(high).any() || h.cmplt(low).any() {
                return;
            }
            if let Some((t, _, _)) = hit_triangle(from, dir, self.triangles[i as usize])
                && t <= length
                && nearest.is_none_or(|(n, _)| t < n)
            {
                nearest = Some((t, i));
            }
        };
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    self.cells.get(&(x, y, z)).into_iter().flatten().for_each(|&i| meet(i));
                }
            }
        }
        self.large.iter().for_each(|&i| meet(i));
        let (t, i) = nearest?;
        let [a, b, c] = self.triangles[i as usize];
        let normal = (b - a).cross(c - a).normalize_or_zero();
        Some((t / length, if normal.dot(dir) > 0.0 { -normal } else { normal }))
    }
}

fn cell(p: Vec3) -> (i32, i32, i32) {
    let c = (p / CELL).floor();
    (c.x as i32, c.y as i32, c.z as i32)
}

/// A box that is in the way: where it is, and its corners in that frame.
#[derive(Clone, Copy, Debug)]
pub struct Obstacle {
    pub pose: Mat4,
    pub low: Vec3,
    pub high: Vec3,
}

/// The box a thing is to the simulation, in its frame.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub middle: Vec3,
    pub half: Vec3,
}

impl Shape {
    pub fn of_box(low: Vec3, high: Vec3) -> Self {
        Shape { middle: (low + high) / 2.0, half: ((high - low) / 2.0).max(Vec3::splat(0.0005)) }
    }
}

/// How a step left it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Moved {
    Moving,
    Still,
    /// Fallen out of the scene.
    Lost,
}

/// Something falling: its middle and the way it is turned, and how fast
/// those change.
#[derive(Clone, Copy, Debug)]
pub struct Body {
    shape: Shape,
    rotation: Quat,
    middle: Vec3,
    velocity: Vec3,
    spin: Vec3,
    still: f32,
    age: f32,
}

impl Body {
    /// `shape` at `pose`, its middle moving at `velocity` and turning by
    /// `spin` (about an axis, radians a second).
    pub fn new(shape: Shape, pose: Mat4, velocity: Vec3, spin: Vec3) -> Self {
        let (_, rotation, position) = pose.to_scale_rotation_translation();
        Body {
            shape,
            rotation,
            middle: position + rotation * shape.middle,
            velocity: velocity.clamp_length_max(MAX_SPEED),
            spin: spin.clamp_length_max(MAX_SPIN),
            still: 0.0,
            age: 0.0,
        }
    }

    pub fn pose(&self) -> Mat4 {
        Mat4::from_rotation_translation(self.rotation, self.middle - self.rotation * self.shape.middle)
    }

    fn corners(half: Vec3) -> impl Iterator<Item = Vec3> {
        (0..8)
            .map(move |c| Vec3::new([-half.x, half.x][c & 1], [-half.y, half.y][c >> 1 & 1], [-half.z, half.z][c >> 2]))
    }

    /// The inverse of its inertia, turned as it is (for a mass of 1).
    fn inverse_inertia(&self) -> Mat3 {
        let h = self.shape.half * self.shape.half;
        let inertia = Vec3::new(h.y + h.z, h.x + h.z, h.x + h.y) / 3.0;
        let turn = Mat3::from_quat(self.rotation);
        turn * Mat3::from_diagonal(inertia.recip()) * turn.transpose()
    }

    /// `STEP` (or `dt`) on: it falls, and meets the surfaces of
    /// `colliders` and the boxes of `obstacles`.
    pub fn step(&mut self, colliders: &Colliders, obstacles: &[Obstacle], dt: f32) -> Moved {
        self.age += dt;
        self.velocity += GRAVITY * dt;
        let from = self.middle;
        self.middle += self.velocity * dt;
        let angle = self.spin.length() * dt;
        if angle > 1e-7 {
            self.rotation = (Quat::from_axis_angle(self.spin.normalize(), angle) * self.rotation).normalize();
        }
        if let Some((t, normal)) = colliders.cross(from, self.middle) {
            self.middle = from.lerp(self.middle, t) + normal * SKIN;
            self.velocity -= normal * self.velocity.dot(normal).min(0.0) * (1.0 + BOUNCE);
        }
        let mut contacts = Vec::new();
        let margin = Vec3::splat(NEARBY);
        for corner in Self::corners(self.shape.half) {
            // (Looked for a little further: what is about to touch.)
            let r = self.rotation * corner;
            let point = self.middle + r;
            let further = self.middle + self.rotation * (corner + corner.signum() * NEARBY);
            if let Some((t, normal)) = colliders.cross(self.middle, further) {
                let met = self.middle.lerp(further, t);
                contacts.push(Contact { r, normal, depth: (met - point).dot(normal) });
            }
            for obstacle in obstacles {
                let local = obstacle.pose.inverse().transform_point3(point);
                if let Some((axis, depth)) = inside(local, obstacle.low - margin, obstacle.high + margin) {
                    let normal = obstacle.pose.transform_vector3(axis).normalize_or_zero();
                    contacts.push(Contact { r, normal, depth: depth - NEARBY });
                }
            }
        }
        // The obstacles' corners in it.
        for obstacle in obstacles {
            let (low, high) = (obstacle.low, obstacle.high);
            for c in 0..8 {
                let local = Vec3::new([low.x, high.x][c & 1], [low.y, high.y][c >> 1 & 1], [low.z, high.z][c >> 2]);
                let point = obstacle.pose.transform_point3(local);
                let mine = self.rotation.inverse() * (point - self.middle);
                if let Some((axis, depth)) = inside(mine, -self.shape.half - margin, self.shape.half + margin) {
                    contacts.push(Contact {
                        r: point - self.middle,
                        normal: -(self.rotation * axis),
                        depth: depth - NEARBY,
                    });
                }
            }
        }
        contacts.retain(|c| c.normal != Vec3::ZERO);
        let touching = contacts.iter().any(|c| c.depth > -2.0 * SKIN);
        self.solve(&contacts, dt);
        if touching {
            let keep = (1.0 - DRAG * dt).max(0.0);
            self.velocity *= keep;
            self.spin *= keep;
        }
        let slow = self.velocity.length() < STILL_SPEED && self.spin.length() < STILL_SPIN;
        self.still = if touching && slow { self.still + dt } else { 0.0 };
        if self.middle.y < colliders.bottom {
            Moved::Lost
        } else if self.still >= STILL_TIME || self.age >= LIFE {
            Moved::Still
        } else {
            Moved::Moving
        }
    }

    /// Out of what it is in, `SKIN` away, and bounced and rubbed at the
    /// points it touches.
    fn solve(&mut self, contacts: &[Contact], dt: f32) {
        // Moved out as far as the deepest of those facing the same way.
        let mut moved = Vec3::ZERO;
        for c in contacts {
            let out = c.depth.max(-SKIN) + SKIN - moved.dot(c.normal);
            if out > 0.0 {
                moved += c.normal * out;
            }
        }
        self.middle += moved;
        // The pushes at the points worked out together, a few times over,
        // each never pulling; one a little way off lets it come no
        // closer than touching in a step.
        let inverse = self.inverse_inertia();
        let resist = |r: Vec3, dir: Vec3| 1.0 + dir.dot((inverse * r.cross(dir)).cross(r));
        let targets: Vec<f32> = contacts
            .iter()
            .map(|c| {
                let into = (self.velocity + self.spin.cross(c.r)).dot(c.normal);
                let gap = (-(c.depth + SKIN)).max(0.0);
                if gap > 0.0 {
                    -gap / dt
                } else if into < -BOUNCING {
                    -BOUNCE * into
                } else {
                    0.0
                }
            })
            .collect();
        let mut pushes = vec![0.0f32; contacts.len()];
        let mut rubs = vec![Vec3::ZERO; contacts.len()];
        for _ in 0..ITERATIONS {
            for (k, c) in contacts.iter().enumerate() {
                let speed = self.velocity + self.spin.cross(c.r);
                let total = (pushes[k] + (targets[k] - speed.dot(c.normal)) / resist(c.r, c.normal)).max(0.0);
                let push = total - pushes[k];
                pushes[k] = total;
                self.velocity += c.normal * push;
                self.spin += inverse * c.r.cross(c.normal * push);
                let speed = self.velocity + self.spin.cross(c.r);
                let along = speed - c.normal * speed.dot(c.normal);
                let sliding = along.length();
                if sliding > 1e-7 {
                    let dir = along / sliding;
                    let total = (rubs[k] + dir * sliding / resist(c.r, dir)).clamp_length_max(FRICTION * pushes[k]);
                    let rub = total - rubs[k];
                    rubs[k] = total;
                    self.velocity -= rub;
                    self.spin -= inverse * c.r.cross(rub);
                }
            }
        }
    }
}

/// A point `r` from its middle, `depth` into something whose surface faces
/// `normal` there (less than nothing: that far out of it).
struct Contact {
    r: Vec3,
    normal: Vec3,
    depth: f32,
}

/// Whether `p` is in the box from `low` to `high`: the way out of it that
/// is nearest (an axis of the box, outwards), and how far that is.
fn inside(p: Vec3, low: Vec3, high: Vec3) -> Option<(Vec3, f32)> {
    if p.cmplt(low).any() || p.cmpgt(high).any() {
        return None;
    }
    let ways = [
        (Vec3::NEG_X, p.x - low.x),
        (Vec3::X, high.x - p.x),
        (Vec3::NEG_Y, p.y - low.y),
        (Vec3::Y, high.y - p.y),
        (Vec3::NEG_Z, p.z - low.z),
        (Vec3::Z, high.z - p.z),
    ];
    ways.into_iter().min_by(|a, b| a.1.total_cmp(&b.1))
}

/// How fast something moved from `from` to `to` in `dt`: its middle (at
/// `middle` in its frame), and its turn.
pub fn motion(from: Mat4, to: Mat4, middle: Vec3, dt: f32) -> (Vec3, Vec3) {
    if dt <= 1e-4 {
        return (Vec3::ZERO, Vec3::ZERO);
    }
    let velocity = (to.transform_point3(middle) - from.transform_point3(middle)) / dt;
    let (_, a, _) = from.to_scale_rotation_translation();
    let (_, b, _) = to.to_scale_rotation_translation();
    let mut turn = b * a.inverse();
    if turn.w < 0.0 {
        turn = -turn;
    }
    let (axis, angle) = turn.to_axis_angle();
    (velocity, axis * angle / dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fall(body: &mut Body, colliders: &Colliders, obstacles: &[Obstacle]) -> Moved {
        for _ in 0..(10.0 / STEP) as usize {
            let moved = body.step(colliders, obstacles, STEP);
            if moved != Moved::Moving {
                return moved;
            }
        }
        Moved::Moving
    }

    #[test]
    fn a_box_falls_onto_the_desk_and_lies_flat() {
        let room = Scene::test_room();
        let colliders = Colliders::new(&room);
        let shape = Shape::of_box(Vec3::new(-0.05, 0.0, -0.08), Vec3::new(0.05, 0.02, 0.08));
        // Let go tilted, 30 cm above the desk.
        let pose = Mat4::from_translation(Vec3::new(0.0, 1.02, -0.5))
            * Mat4::from_rotation_x(0.6)
            * Mat4::from_rotation_z(0.3);
        let mut body = Body::new(shape, pose, Vec3::ZERO, Vec3::ZERO);
        assert_eq!(fall(&mut body, &colliders, &[]), Moved::Still);
        let (_, rotation, at) = body.pose().to_scale_rotation_translation();
        assert!((at.y - 0.72).abs() < 0.005, "on the desk: {:?}", at);
        assert!((rotation * Vec3::Y).dot(Vec3::Y) > 0.99, "flat: {:?}", rotation * Vec3::Y);
    }

    #[test]
    fn a_throw_goes_on_and_falls() {
        let room = Scene::test_room();
        let colliders = Colliders::new(&room);
        let shape = Shape::of_box(Vec3::splat(-0.02), Vec3::splat(0.02));
        let start = Vec3::new(0.3, 1.0, -0.55);
        let mut body = Body::new(shape, Mat4::from_translation(start), Vec3::new(2.5, 0.0, 0.0), Vec3::ZERO);
        fall(&mut body, &colliders, &[]);
        let at = body.pose().transform_point3(Vec3::ZERO);
        assert!(at.x > 0.6, "{:?}", at);
        assert!(at.y < 0.72, "off the desk: {:?}", at);
    }

    #[test]
    fn things_stack() {
        let room = Scene::test_room();
        let colliders = Colliders::new(&room);
        let under = Obstacle {
            pose: Mat4::from_translation(Vec3::new(0.0, 0.72, -0.5)),
            low: Vec3::new(-0.1, 0.0, -0.1),
            high: Vec3::new(0.1, 0.05, 0.1),
        };
        let shape = Shape::of_box(Vec3::new(-0.04, 0.0, -0.04), Vec3::new(0.04, 0.02, 0.04));
        let mut body = Body::new(shape, Mat4::from_translation(Vec3::new(0.0, 1.0, -0.5)), Vec3::ZERO, Vec3::ZERO);
        assert_eq!(fall(&mut body, &colliders, &[under]), Moved::Still);
        let at = body.pose().transform_point3(Vec3::ZERO);
        assert!((at.y - 0.77).abs() < 0.005, "on the box: {:?}", at);
    }

    #[test]
    fn motion_is_how_fast_it_moved_and_turned() {
        let from = Mat4::IDENTITY;
        let to = Mat4::from_translation(Vec3::new(0.1, 0.0, 0.0)) * Mat4::from_rotation_y(0.2);
        let (velocity, spin) = motion(from, to, Vec3::ZERO, 0.1);
        assert!((velocity - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-4);
        assert!((spin - Vec3::new(0.0, 2.0, 0.0)).length() < 1e-3, "{:?}", spin);
    }
}

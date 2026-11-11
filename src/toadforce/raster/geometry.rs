//! Transform and lighting: a draw's vertices from the space its
//! program has them in to the screen ("Transform and lighting" in
//! chapter 9 of `docs/toadforce-spec.md`).
//!
//! `Setup` is what a draw's state comes to for its vertices: the
//! matrices multiplied out, the lights in camera space, the planes
//! primitives are cut at. `Setup::vertex` takes one vertex through it,
//! and `Setup::project` puts the result on the screen. Between the two
//! `clip` cuts the primitives.
//!
//! Vectors are rows and a matrix is 16 numbers row by row, as
//! Direct3D has them: a vertex times the world matrix, times the view
//! matrix, times the projection matrix.

use super::light::Lighting;
use super::math;
use super::pipeline::{Coord, SLOTS};
use super::texel::{self, Rgba};
use super::vertex::{Fvf, ScreenVertex};
use crate::toadforce::level1::states::*;
use crate::toadforce::level1::{
    CLIP_PLANES, Level1, TRANSFORM_PROJECTION, TRANSFORM_TEXTURE, TRANSFORM_VIEW, TRANSFORM_WORLD,
};

pub type Matrix = [f32; 16];

/// The planes a primitive is cut at: `PLANE_W` to `PLANE_BOTTOM`, then
/// the clip planes of the state.
pub const PLANES: usize = PLANE_USER + CLIP_PLANES;
/// w at least `W_MIN`: in front of the eye.
pub const PLANE_W: usize = 0;
pub const PLANE_NEAR: usize = 1;
pub const PLANE_FAR: usize = 2;
pub const PLANE_LEFT: usize = 3;
pub const PLANE_RIGHT: usize = 4;
pub const PLANE_TOP: usize = 5;
pub const PLANE_BOTTOM: usize = 6;
pub const PLANE_USER: usize = 7;

/// The planes that are cut at whatever `CLIPPING` says, since what is
/// past them has no place on the screen.
const ALWAYS: u16 = 1 << PLANE_W | 1 << PLANE_NEAR | 0xF << PLANE_LEFT;

/// The smallest w a vertex on the screen has.
pub const W_MIN: f32 = 1.0 / 65536.0;

/// How far from the render target's top left a vertex may be, in
/// pixels either way. Primitives are cut there, and the pixels between
/// the viewport and there are left out as they are drawn.
pub const BAND: f32 = 16384.0;

/// A vertex through transform and lighting, before the division by w.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub clip: [f32; 4],
    /// Red, green, blue and alpha, 0 to 1.
    pub diffuse: [f32; 4],
    /// The alpha is the fog factor.
    pub specular: [f32; 4],
    /// The two colours of the back of a triangle, where both sides are
    /// lit.
    pub back: [[f32; 4]; 2],
    pub psize: f32,
    /// The coordinates of each slot of the draw.
    pub tex: [[f32; 4]; SLOTS],
    /// How far inside each plane the vertex is, below 0 outside.
    pub dist: [f32; PLANES],
    /// A bit for each plane in force the vertex is outside of.
    pub out: u16,
}

impl Vertex {
    /// The vertex the share `t` of the way to `o`.
    pub fn lerp(&self, o: &Vertex, t: f32) -> Vertex {
        let l = |a: f32, b: f32| a + (b - a) * t;
        let l4 = |a: [f32; 4], b: [f32; 4]| std::array::from_fn(|i| l(a[i], b[i]));
        Vertex {
            clip: l4(self.clip, o.clip),
            diffuse: l4(self.diffuse, o.diffuse),
            specular: l4(self.specular, o.specular),
            back: [l4(self.back[0], o.back[0]), l4(self.back[1], o.back[1])],
            psize: l(self.psize, o.psize),
            tex: std::array::from_fn(|i| l4(self.tex[i], o.tex[i])),
            dist: std::array::from_fn(|i| l(self.dist[i], o.dist[i])),
            out: 0,
        }
    }

    /// The vertex with the colours of its back.
    pub fn turned(mut self) -> Vertex {
        self.diffuse = self.back[0];
        self.specular = self.back[1];
        self
    }
}

pub fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    std::array::from_fn(|i| {
        let (row, col) = (i / 4, i % 4);
        (0..4).map(|k| a[row * 4 + k] * b[k * 4 + col]).sum()
    })
}

pub fn transform(v: [f32; 4], m: &Matrix) -> [f32; 4] {
    std::array::from_fn(|col| (0..4).map(|row| v[row] * m[row * 4 + col]).sum())
}

/// A direction through the matrix's upper left three by three.
pub fn direction(v: [f32; 3], m: &Matrix) -> [f32; 3] {
    std::array::from_fn(|col| (0..3).map(|row| v[row] * m[row * 4 + col]).sum())
}

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

/// `a` with a length of 1, or as it is when it has none.
pub fn normalize(a: [f32; 3]) -> [f32; 3] {
    let len = length(a);
    if len > 0.0 && len.is_finite() {
        a.map(|c| c / len)
    } else {
        a
    }
}

/// What takes a normal where `m` takes a position: the inverse of
/// `m`'s upper left three by three, transposed. All 0 for a matrix
/// that has no inverse.
fn normal_matrix(m: &Matrix) -> [[f32; 3]; 3] {
    let row = |r: usize| [m[r * 4], m[r * 4 + 1], m[r * 4 + 2]];
    let (r0, r1, r2) = (row(0), row(1), row(2));
    let cofactors = [cross(r1, r2), cross(r2, r0), cross(r0, r1)];
    let det = dot(r0, cofactors[0]);
    if det == 0.0 || !det.is_finite() {
        return [[0.0; 3]; 3];
    }
    cofactors.map(|c| c.map(|v| v / det))
}

fn times(n: [f32; 3], m: &[[f32; 3]; 3]) -> [f32; 3] {
    std::array::from_fn(|col| n[0] * m[0][col] + n[1] * m[1][col] + n[2] * m[2][col])
}

/// A colour of 0 to 1 a channel, red first, as the rasteriser has
/// one.
fn bytes(c: [f32; 4]) -> Rgba {
    // A channel that is no number becomes 0.
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let mut out = [0; 4];
    out[texel::R] = byte(c[0]);
    out[texel::G] = byte(c[1]);
    out[texel::B] = byte(c[2]);
    out[texel::A] = byte(c[3]);
    out
}

/// A colour in one word as red, green, blue and alpha of 0 to 1.
pub fn colour(word: u32) -> [f32; 4] {
    let c = texel::from_word(word);
    [texel::R, texel::G, texel::B, texel::A].map(|i| c[i] as f32 / 255.0)
}

/// Fog worked out at the vertices.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fog {
    mode: u32,
    start: f32,
    end: f32,
    density: f32,
    /// By the distance from the eye, and not by the depth.
    range: bool,
}

impl Fog {
    fn factor(&self, eye: [f32; 3]) -> f32 {
        let d = if self.range {
            length(eye)
        } else {
            eye[2].abs()
        };
        let f = match self.mode {
            1 => math::exp_neg(self.density * d),
            2 => math::exp_neg((self.density * d) * (self.density * d)),
            _ => (self.end - d) / (self.end - self.start),
        };
        // No number, as with a start equal to the end, is no fog.
        if f.is_nan() { 1.0 } else { f.clamp(0.0, 1.0) }
    }
}

/// Where a slot's coordinates come from, and the matrix they go
/// through.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Slot {
    /// Where the vertex has the set and how many numbers it has, for
    /// a slot of the vertex's own coordinates.
    set: (u8, u8),
    generate: u8,
    /// The stage's texture matrix and how many coordinates it gives.
    matrix: Option<(Matrix, u8)>,
}

/// What a draw's state comes to for its vertices.
#[derive(Clone, Debug)]
pub struct Setup {
    fvf: Fvf,
    /// The world matrices the vertex's weights blend, less one: 0 for
    /// a vertex in one.
    blend: usize,
    world: [Matrix; 4],
    world_view: [Matrix; 4],
    normals: [[[f32; 3]; 3]; 4],
    projection: Matrix,
    /// World, view and projection in one, for a vertex that isn't
    /// blended.
    all: Matrix,
    lighting: Option<Lighting>,
    /// `TWO_SIDED_LIGHTING` where vertices are lit.
    two_sided: u32,
    normalize: bool,
    fog: Option<Fog>,
    slots: Vec<Slot>,
    /// `POINTSIZE`, or None where the points have the size the
    /// rasteriser gives them.
    point_size: Option<f32>,
    /// The viewport's height and `POINTSCALE_A` to `_C`.
    point_scale: Option<[f32; 4]>,
    /// The clip planes in force, by their number.
    user: Vec<(usize, [f32; 4])>,
    /// The band's edges as shares of w: left, right, bottom, top.
    band: [f32; 4],
    /// The planes vertices are told apart by.
    planes: u16,
    /// Whether primitives are cut at the planes, or left out when they
    /// cross one of `ALWAYS`.
    pub clipping: bool,
    /// Screen x = `x[0]` + clip x / w * `x[1]`, and so y and z.
    x: [f32; 2],
    y: [f32; 2],
    z: [f32; 2],
    /// Whether the eye-space position or normal is needed.
    eye: bool,
}

impl Setup {
    /// The setup of the state `l` for vertices laid out as `fvf`, which
    /// isn't a transformed layout, with the coordinates of `coords` in
    /// the draw's slots.
    pub fn new(l: &Level1, coords: &[Coord], fvf: &Fvf) -> Setup {
        let on = |id: u32| l.state(id) != 0;
        let view = &l.transforms[TRANSFORM_VIEW as usize];
        let projection = l.transforms[TRANSFORM_PROJECTION as usize];
        let blend = (l.state(RS_VERTEXBLEND) as usize).min(fvf.weights as usize);
        let world: [Matrix; 4] =
            std::array::from_fn(|i| l.transforms[TRANSFORM_WORLD as usize + i]);
        let world_view = world.map(|w| multiply(&w, view));
        let lighting = on(RS_LIGHTING).then(|| Lighting::new(l, fvf));
        let table = l.state(RS_FOGTABLEMODE) != 0;
        let fog = (on(RS_FOGENABLE) && !table && l.state(RS_FOGVERTEXMODE) != 0).then(|| Fog {
            mode: l.state(RS_FOGVERTEXMODE),
            start: l.state_f(RS_FOGSTART),
            end: l.state_f(RS_FOGEND),
            density: l.state_f(RS_FOGDENSITY),
            range: on(RS_RANGEFOGENABLE),
        });
        let slots: Vec<Slot> = coords
            .iter()
            .map(|c| Slot {
                set: fvf.sets[c.set as usize],
                generate: c.generate,
                matrix: (c.count > 0).then(|| {
                    let matrix = l.transforms[TRANSFORM_TEXTURE as usize + c.stage as usize];
                    (matrix, c.count)
                }),
            })
            .collect();
        let scale = on(RS_POINTSCALEENABLE);
        let v = &l.viewport;
        let (vx, vy) = (v.x as f32, v.y as f32);
        let (half_w, half_h) = (v.width as f32 / 2.0, v.height as f32 / 2.0);
        let clipping = on(RS_CLIPPING);
        let user: Vec<(usize, [f32; 4])> = (0..CLIP_PLANES)
            .filter(|i| clipping && l.state(RS_CLIPPLANEENABLE) >> i & 1 != 0)
            .map(|i| (i, l.clip_planes[i]))
            .collect();
        let mut planes = ALWAYS;
        if clipping {
            planes |= 1 << PLANE_FAR;
            for (i, _) in &user {
                planes |= 1 << (PLANE_USER + i);
            }
        }
        let generated = slots.iter().any(|s| s.generate != 0);
        Setup {
            fvf: *fvf,
            blend,
            world,
            world_view,
            normals: world_view.map(|m| normal_matrix(&m)),
            projection,
            all: multiply(&world_view[0], &projection),
            two_sided: match &lighting {
                Some(_) => l.state(RS_TWO_SIDED_LIGHTING),
                None => 0,
            },
            normalize: on(RS_NORMALIZENORMALS),
            eye: lighting.is_some() || fog.is_some() || generated || scale || blend > 0,
            lighting,
            fog,
            slots,
            point_size: (scale || fvf.psize.is_some()).then(|| l.state_f(RS_POINTSIZE)),
            point_scale: scale.then(|| {
                [
                    v.height as f32,
                    l.state_f(RS_POINTSCALE_A),
                    l.state_f(RS_POINTSCALE_B),
                    l.state_f(RS_POINTSCALE_C),
                ]
            }),
            user,
            // Screen x is vx + (x / w + 1) * half_w, and y goes down
            // where clip y goes up.
            band: [
                (-BAND - vx) / half_w - 1.0,
                (BAND - vx) / half_w - 1.0,
                1.0 - (BAND - vy) / half_h,
                1.0 - (-BAND - vy) / half_h,
            ],
            planes,
            clipping,
            x: [vx + half_w, half_w],
            y: [vy + half_h, -half_h],
            z: [v.min_z, v.max_z - v.min_z],
        }
    }

    /// Whether the triangle of `v` shows its back, which has colours
    /// of its own.
    pub fn back(&self, v: [&Vertex; 3]) -> bool {
        match self.two_sided {
            0 => false,
            // The back is counterclockwise with 1 and clockwise with 2.
            side => clockwise(v).is_some_and(|clockwise| clockwise == (side == 2)),
        }
    }

    /// The vertex whose words `word` gives through transform, lighting,
    /// fog and the making of its texture coordinates.
    pub fn vertex(&self, word: impl Fn(usize) -> u32) -> Vertex {
        let number = |at: usize| f32::from_bits(word(at));
        let three = |at: usize| [number(at), number(at + 1), number(at + 2)];
        let p = [number(0), number(1), number(2), 1.0];
        let n = self.fvf.normal.map_or([0.0; 3], |at| three(at as usize));
        let mut world = [0.0; 4];
        let mut eye4 = [0.0; 4];
        let mut normal = [0.0; 3];
        let clip;
        if self.blend == 0 {
            clip = transform(p, &self.all);
            if self.eye {
                eye4 = transform(p, &self.world_view[0]);
                normal = times(n, &self.normals[0]);
            }
            if !self.user.is_empty() {
                world = transform(p, &self.world[0]);
            }
        } else {
            // The last weight is what the others leave of 1.
            let mut weights = [0.0f32; 4];
            let mut left = 1.0;
            for (i, w) in weights.iter_mut().enumerate().take(self.blend) {
                *w = number(3 + i);
                left -= *w;
            }
            weights[self.blend] = left;
            for (i, &w) in weights.iter().enumerate().take(self.blend + 1) {
                let e = transform(p, &self.world_view[i]);
                let nn = times(n, &self.normals[i]);
                let ww = transform(p, &self.world[i]);
                for k in 0..4 {
                    eye4[k] += w * e[k];
                    world[k] += w * ww[k];
                }
                for k in 0..3 {
                    normal[k] += w * nn[k];
                }
            }
            clip = transform(eye4, &self.projection);
        }
        let eye = [eye4[0], eye4[1], eye4[2]];
        if self.normalize {
            normal = normalize(normal);
        }

        let own_diffuse = self.fvf.diffuse.map(|at| colour(word(at as usize)));
        let own_specular = self.fvf.specular.map(|at| colour(word(at as usize)));
        let mut v = Vertex {
            clip,
            ..Vertex::default()
        };
        match &self.lighting {
            Some(lighting) => {
                (v.diffuse, v.specular) = lighting.light(eye, normal, own_diffuse, own_specular);
                if self.two_sided != 0 {
                    let (d, s) = lighting.light(eye, normal.map(|c| -c), own_diffuse, own_specular);
                    v.back = [d, s];
                }
            }
            None => {
                v.diffuse = own_diffuse.unwrap_or([1.0; 4]);
                v.specular = own_specular.unwrap_or([0.0, 0.0, 0.0, 1.0]);
            }
        }
        if let Some(fog) = &self.fog {
            v.specular[3] = fog.factor(eye);
        }
        if self.two_sided != 0 {
            v.back[1][3] = v.specular[3];
        }

        for (slot, out) in self.slots.iter().zip(&mut v.tex) {
            let (from, numbers): ([f32; 4], usize) = match slot.generate {
                0 => {
                    let (at, numbers) = slot.set;
                    let mut c = [0.0; 4];
                    for (i, c) in c.iter_mut().enumerate().take(numbers as usize) {
                        *c = number(at as usize + i);
                    }
                    (c, numbers as usize)
                }
                1 => ([normal[0], normal[1], normal[2], 0.0], 3),
                2 => ([eye[0], eye[1], eye[2], 0.0], 3),
                generate => {
                    // The eye's line of sight to the vertex, thrown
                    // back by the surface.
                    let sight = normalize(eye);
                    let twice = 2.0 * dot(normal, sight);
                    let r: [f32; 3] = std::array::from_fn(|i| sight[i] - twice * normal[i]);
                    if generate == 3 {
                        ([r[0], r[1], r[2], 0.0], 3)
                    } else {
                        let z = 1.0 - r[2];
                        let m = 2.0 * (r[0] * r[0] + r[1] * r[1] + z * z).sqrt();
                        ([r[0] / m + 0.5, r[1] / m + 0.5, 0.0, 0.0], 2)
                    }
                }
            };
            *out = match &slot.matrix {
                Some((matrix, count)) => {
                    // The number after a set's last is 1.
                    let mut c = from;
                    if numbers < 4 {
                        c[numbers] = 1.0;
                    }
                    let c = transform(c, matrix);
                    std::array::from_fn(|i| match i {
                        i if i < *count as usize => c[i],
                        3 => 1.0,
                        _ => 0.0,
                    })
                }
                None => {
                    let mut c = from;
                    if numbers < 4 {
                        c[3] = 1.0;
                    }
                    c
                }
            };
        }

        if let Some(size) = self.point_size {
            v.psize = self.fvf.psize.map_or(size, |at| number(at as usize));
            if let Some([height, a, b, c]) = self.point_scale {
                let d = length(eye);
                let under = a + b * d + c * d * d;
                if under > 0.0 {
                    v.psize *= height * (1.0 / under).sqrt();
                }
            }
        }

        let [x, y, z, w] = clip;
        v.dist[PLANE_W] = w - W_MIN;
        v.dist[PLANE_NEAR] = z;
        v.dist[PLANE_FAR] = w - z;
        v.dist[PLANE_LEFT] = x - self.band[0] * w;
        v.dist[PLANE_RIGHT] = self.band[1] * w - x;
        v.dist[PLANE_BOTTOM] = y - self.band[2] * w;
        v.dist[PLANE_TOP] = self.band[3] * w - y;
        for (i, plane) in &self.user {
            v.dist[PLANE_USER + i] = plane[0] * world[0]
                + plane[1] * world[1]
                + plane[2] * world[2]
                + plane[3] * world[3];
        }
        for (i, d) in v.dist.iter().enumerate() {
            // A distance that is no number is outside.
            if self.planes >> i & 1 != 0 && (d.is_nan() || *d < 0.0) {
                v.out |= 1 << i;
            }
        }
        v
    }

    /// Where `v` is on the screen, with its values as the rasteriser
    /// takes them: the coordinates of slot `n` as its set `n`.
    pub fn project(&self, v: &Vertex) -> ScreenVertex {
        let rhw = 1.0 / v.clip[3];
        let mut tex = ScreenVertex::default().tex;
        tex[..SLOTS].copy_from_slice(&v.tex);
        ScreenVertex {
            x: self.x[0] + v.clip[0] * rhw * self.x[1],
            y: self.y[0] + v.clip[1] * rhw * self.y[1],
            z: self.z[0] + v.clip[2] * rhw * self.z[1],
            // A vertex that isn't on the screen is asked for its
            // colours only.
            rhw: if rhw > 0.0 && rhw.is_finite() {
                rhw
            } else {
                1.0
            },
            diffuse: bytes(v.diffuse),
            specular: bytes(v.specular),
            psize: self.point_size.map(|_| v.psize),
            tex,
        }
    }
}

/// Whether the triangle of `v` is clockwise on the screen, taken
/// through the eye's plane where it crosses that; None for one with no
/// area.
pub fn clockwise(v: [&Vertex; 3]) -> Option<bool> {
    let [a, b, c] = v.map(|v| [v.clip[0] as f64, v.clip[1] as f64, v.clip[3] as f64]);
    let det = a[0] * (b[1] * c[2] - c[1] * b[2]) - a[1] * (b[0] * c[2] - c[0] * b[2])
        + a[2] * (b[0] * c[1] - c[0] * b[1]);
    // With y up, as in the picture, a positive one is
    // counterclockwise.
    (det != 0.0 && det.is_finite()).then_some(det < 0.0)
}

//! The geometry pipeline: what Windows' Direct3D did on the processor
//! for cards without transform and lighting of their own, all of 3dfx's
//! among them. Vertices go from the program's coordinates through the
//! world, view and projection matrices, are lit and fogged, clipped
//! against the view volume, the user's clip planes and the card's guard
//! band, culled, and come out as screen vertices of triangles, lines and
//! points for the card's rasterizer.
//!
//! It is plain Rust on plain data: the device (`draw.rs`) makes a
//! `Setup` of its states, reads the program's vertices into `Vertex`es
//! with a `Layout`, and gives the primitives that come out to a
//! `Rasterizer`. The matrices are Direct3D's: row-major, and a row
//! vector is transformed by multiplying it on their left.

use crate::fixed_vec::FixedVec;

/// A 4x4 matrix, row by row (D3DMATRIX's _11 to _44).
pub type Matrix = [f32; 16];

pub const IDENTITY: Matrix = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// The product `a`·`b`: what transforms a vector by `a` and then by `b`.
pub fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut m = [0.0; 16];
    for i in 0..4 {
        for j in 0..4 {
            m[i * 4 + j] = a[i * 4] * b[j]
                + a[i * 4 + 1] * b[4 + j]
                + a[i * 4 + 2] * b[8 + j]
                + a[i * 4 + 3] * b[12 + j];
        }
    }
    m
}

/// The row vector `v` transformed by `m`.
pub fn transform(v: [f32; 4], m: &Matrix) -> [f32; 4] {
    let mut r = [0.0; 4];
    for (j, r) in r.iter_mut().enumerate() {
        *r = v[0] * m[j] + v[1] * m[4 + j] + v[2] * m[8 + j] + v[3] * m[12 + j];
    }
    r
}

fn point(v: [f32; 3], m: &Matrix) -> [f32; 3] {
    let r = transform([v[0], v[1], v[2], 1.0], m);
    [r[0], r[1], r[2]]
}

fn vector(v: [f32; 3], m: &Matrix) -> [f32; 3] {
    let r = transform([v[0], v[1], v[2], 0.0], m);
    [r[0], r[1], r[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

/// `a` made 1 long; a vector of length 0 stays as it is.
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = length(a);
    if l > 0.0 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        a
    }
}

/// The matrix that transforms normals as `m` transforms points: the
/// inverse of its upper 3x3 part, transposed (its cofactors over its
/// determinant). A singular matrix transforms them as itself.
fn normal_matrix(m: &Matrix) -> [[f32; 3]; 3] {
    let r0 = [m[0], m[1], m[2]];
    let r1 = [m[4], m[5], m[6]];
    let r2 = [m[8], m[9], m[10]];
    let c0 = cross(r1, r2);
    let det = dot(r0, c0);
    if det.abs() < 1e-30 || !det.is_finite() {
        return [r0, r1, r2];
    }
    let c1 = cross(r2, r0);
    let c2 = cross(r0, r1);
    let s = |c: [f32; 3]| [c[0] / det, c[1] / det, c[2] / det];
    [s(c0), s(c1), s(c2)]
}

fn times(n: [f32; 3], m: &[[f32; 3]; 3]) -> [f32; 3] {
    let mut r = [0.0; 3];
    for (j, r) in r.iter_mut().enumerate() {
        *r = n[0] * m[0][j] + n[1] * m[1][j] + n[2] * m[2][j];
    }
    r
}

// Flexible vertex formats.

pub const FVF_XYZ: u32 = 0x002;
pub const FVF_XYZRHW: u32 = 0x004;
const FVF_POSITION_MASK: u32 = 0x00E;
pub const FVF_NORMAL: u32 = 0x010;
const FVF_RESERVED1: u32 = 0x020;
pub const FVF_DIFFUSE: u32 = 0x040;
pub const FVF_SPECULAR: u32 = 0x080;
const FVF_RESERVED0: u32 = 0x001;
const FVF_RESERVED2: u32 = 0xF000;
/// Direct3D 8's D3DFVF_LASTBETA_UBYTE4: the last beta of an XYZBn
/// position is the four bytes of the indices of the world matrices.
/// Direct3D 8.1 has no D3DFVF_LASTBETA_D3DCOLOR; Direct3D 9 added it.
pub const FVF_LASTBETA_UBYTE4: u32 = 0x1000;

/// D3DVERTEX, D3DLVERTEX and D3DTLVERTEX, as FVF codes.
pub const FVF_VERTEX: u32 = 0x112;
pub const FVF_LVERTEX: u32 = 0x1E2;
pub const FVF_TLVERTEX: u32 = 0x1C4;

/// Where the parts of a vertex are in it, by its FVF code: byte offsets,
/// and the size of one vertex.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub fvf: u32,
    pub size: u32,
    /// Whether it is transformed already: x, y, z and rhw on the screen.
    pub rhw: bool,
    pub normal: Option<u32>,
    pub diffuse: Option<u32>,
    pub specular: Option<u32>,
    /// The texture coordinate sets: offset and number of floats of each.
    pub textures: usize,
    pub tex: [(u32, u32); 8],
    /// The blending weights after x, y and z: how many floats.
    pub weights: u32,
    /// Where the indices of the world matrices are, as four bytes.
    pub indices: Option<u32>,
}

impl Layout {
    /// The layout of the FVF code `fvf`, if it is one of Direct3D 7's
    /// or before.
    pub fn of(fvf: u32) -> Option<Layout> {
        if fvf & FVF_RESERVED2 != 0 {
            return None;
        }
        Layout::of_d3d8(fvf)
    }

    /// The layout of the FVF code `fvf`, if it is one of Direct3D 8's,
    /// which may have D3DFVF_LASTBETA_UBYTE4. With it the last of the
    /// XYZBn betas holds the matrix indices and the others are weights:
    /// XYZB2 has one weight and the indices. Without betas the flag has
    /// nothing to change, and is let be.
    pub fn of_d3d8(fvf: u32) -> Option<Layout> {
        if fvf & (FVF_RESERVED0 | (FVF_RESERVED2 & !FVF_LASTBETA_UBYTE4)) != 0 {
            return None;
        }
        let position = fvf & FVF_POSITION_MASK;
        let (mut at, rhw) = match position {
            FVF_XYZ => (12, false),
            FVF_XYZRHW => (16, true),
            // XYZB1 to XYZB5: x, y, z and the blending weights.
            0x6..=0xE => (12 + 4 * (position - 4) / 2, false),
            _ => return None,
        };
        let mut l = Layout {
            fvf,
            rhw,
            ..Layout::default()
        };
        if !rhw {
            let betas = (at - 12) / 4;
            l.weights = betas;
            if betas > 0 && fvf & FVF_LASTBETA_UBYTE4 != 0 {
                l.weights = betas - 1;
                l.indices = Some(at - 4);
            }
        }
        if fvf & FVF_NORMAL != 0 {
            l.normal = Some(at);
            at += 12;
        }
        if fvf & FVF_RESERVED1 != 0 {
            at += 4;
        }
        if fvf & FVF_DIFFUSE != 0 {
            l.diffuse = Some(at);
            at += 4;
        }
        if fvf & FVF_SPECULAR != 0 {
            l.specular = Some(at);
            at += 4;
        }
        let count = (fvf >> 8 & 0xF) as usize;
        if count > 8 {
            return None;
        }
        l.textures = count;
        for i in 0..count {
            let dims = match fvf >> (16 + 2 * i) & 3 {
                0 => 2,
                1 => 3,
                2 => 4,
                _ => 1,
            };
            l.tex[i] = (at, dims);
            at += 4 * dims;
        }
        l.size = at;
        Some(l)
    }

    /// The vertex at the start of `bytes`, which has `size` of them.
    pub fn read(&self, bytes: &[u8]) -> Vertex {
        let f = |at: u32| -> f32 {
            let at = at as usize;
            bytes
                .get(at..at + 4)
                .map_or(0.0, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let d = |at: u32| -> u32 {
            let at = at as usize;
            bytes
                .get(at..at + 4)
                .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let mut v = Vertex {
            position: [f(0), f(4), f(8), if self.rhw { f(12) } else { 1.0 }],
            ..Vertex::default()
        };
        if let Some(at) = self.normal {
            v.normal = [f(at), f(at + 4), f(at + 8)];
        }
        for k in 0..self.weights.min(4) {
            v.weights[k as usize] = f(12 + 4 * k);
        }
        if let Some(at) = self.indices {
            v.indices = d(at).to_le_bytes();
        }
        v.diffuse = self.diffuse.map(d);
        v.specular = self.specular.map(d);
        for i in 0..self.textures {
            let (at, dims) = self.tex[i];
            for k in 0..dims {
                v.tex[i][k as usize] = f(at + 4 * k);
            }
        }
        v
    }
}

/// A vertex as the program gave it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    /// x, y, z, and rhw for a transformed one (1 for others).
    pub position: [f32; 4],
    pub normal: [f32; 3],
    pub diffuse: Option<u32>,
    pub specular: Option<u32>,
    pub tex: [[f32; 4]; 8],
    /// The blending weights (the first four the vertex has, 0 for those
    /// it hasn't) and the indices of the world matrices they weigh.
    pub weights: [f32; 4],
    pub indices: [u8; 4],
    /// The second position and normal a declaration's vertex can have,
    /// for tweening (D3DVSDE_POSITION2 and NORMAL2).
    pub position2: Option<[f32; 3]>,
    pub normal2: Option<[f32; 3]>,
}

/// A D3DCOLOR (ARGB) as red, green, blue and alpha from 0 to 1.
pub fn rgba(c: u32) -> [f32; 4] {
    let b = |s: u32| (c >> s & 0xFF) as f32 / 255.0;
    [b(16), b(8), b(0), b(24)]
}

/// Red, green, blue and alpha as a D3DCOLOR, clamped.
pub fn color(c: [f32; 4]) -> u32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    b(c[3]) << 24 | b(c[0]) << 16 | b(c[1]) << 8 | b(c[2])
}

// What the device sets up.

/// How clip coordinates map to the screen: x/w, y/w and z/w inside
/// `bounds` (x from, to, y from, to, z from, to) are in view, and go to
/// sx = ox + x/w * sx, sy = oy - y/w * sy, sz = oz + z/w * sz.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewMap {
    pub ox: f32,
    pub oy: f32,
    pub oz: f32,
    pub sx: f32,
    pub sy: f32,
    pub sz: f32,
    pub bounds: [f32; 6],
}

impl ViewMap {
    /// Direct3D 7's D3DVIEWPORT7: the cube from (-1, -1, 0) to (1, 1,
    /// 1) onto the rectangle, z onto MinZ to MaxZ.
    pub fn dx7(x: u32, y: u32, w: u32, h: u32, min_z: f32, max_z: f32) -> ViewMap {
        let (w, h) = (w as f32, h as f32);
        ViewMap {
            ox: x as f32 + w / 2.0,
            oy: y as f32 + h / 2.0,
            oz: min_z,
            sx: w / 2.0,
            sy: h / 2.0,
            sz: max_z - min_z,
            bounds: [-1.0, 1.0, -1.0, 1.0, 0.0, 1.0],
        }
    }

    /// D3DVIEWPORT: x/w and y/w scaled by dvScaleX and dvScaleY from the
    /// rectangle's middle, in view to dvMaxX and dvMaxY either way.
    #[allow(clippy::too_many_arguments)]
    pub fn v1(x: u32, y: u32, w: u32, h: u32, scale: [f32; 2], max: [f32; 2]) -> ViewMap {
        ViewMap {
            ox: x as f32 + w as f32 / 2.0,
            oy: y as f32 + h as f32 / 2.0,
            oz: 0.0,
            sx: scale[0],
            sy: scale[1],
            sz: 1.0,
            bounds: [-max[0], max[0], -max[1], max[1], 0.0, 1.0],
        }
    }

    /// D3DVIEWPORT2: the clip volume from dvClipX to dvClipX +
    /// dvClipWidth, dvClipY - dvClipHeight to dvClipY and dvMinZ to
    /// dvMaxZ onto the rectangle, z onto 0 to 1.
    pub fn v2(rect: [u32; 4], clip: [f32; 4], min_z: f32, max_z: f32) -> ViewMap {
        let [x, y, w, h] = rect.map(|v| v as f32);
        let [cx, cy, mut cw, mut ch] = clip;
        if cw == 0.0 {
            cw = 1.0;
        }
        if ch == 0.0 {
            ch = 1.0;
        }
        let depth = if max_z != min_z { max_z - min_z } else { 1.0 };
        let (sx, sy) = (w / cw, h / ch);
        ViewMap {
            ox: x - cx * sx,
            oy: y + cy * sy,
            oz: -min_z / depth,
            sx,
            sy,
            sz: 1.0 / depth,
            bounds: [cx, cx + cw, cy - ch, cy, min_z, max_z],
        }
    }

    /// Where clip coordinates `c` are on the screen: x, y, z and rhw.
    pub fn screen(&self, c: [f32; 4]) -> [f32; 4] {
        let rhw = 1.0 / c[3];
        [
            self.ox + c[0] * rhw * self.sx,
            self.oy - c[1] * rhw * self.sy,
            self.oz + c[2] * rhw * self.sz,
            rhw,
        ]
    }

    /// The clip coordinates `screen` puts at `s` (x, y, z and rhw), if
    /// any do: an rhw of 0, infinite or not a number has none, and nor
    /// does a viewport with no width, height or depth.
    fn clip(&self, s: [f32; 4]) -> Option<[f32; 4]> {
        let w = 1.0 / s[3];
        if !w.is_finite() || w == 0.0 || self.sx == 0.0 || self.sy == 0.0 || self.sz == 0.0 {
            return None;
        }
        Some([
            (s[0] - self.ox) / self.sx * w,
            (self.oy - s[1]) / self.sy * w,
            (s[2] - self.oz) / self.sz * w,
            w,
        ])
    }

    /// The guard band (left, top, right, bottom on the screen) as bounds
    /// of x/w and y/w: from, to for each.
    fn guard(&self, band: [f32; 4]) -> [f32; 4] {
        let x = |s: f32| (s - self.ox) / self.sx;
        let y = |s: f32| (self.oy - s) / self.sy;
        let (x0, x1) = (x(band[0]), x(band[2]));
        let (y0, y1) = (y(band[3]), y(band[1]));
        [x0.min(x1), x0.max(x1), y0.min(y1), y0.max(y1)]
    }
}

// D3DCLIP_ flags.
pub const CLIP_LEFT: u32 = 0x1;
pub const CLIP_RIGHT: u32 = 0x2;
pub const CLIP_TOP: u32 = 0x4;
pub const CLIP_BOTTOM: u32 = 0x8;
pub const CLIP_FRONT: u32 = 0x10;
pub const CLIP_BACK: u32 = 0x20;
pub const CLIP_GEN0: u32 = 0x40;

/// A light, in world space, in Direct3D 7's terms (D3DLIGHT7).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Light {
    /// D3DLIGHT_POINT 1, SPOT 2, DIRECTIONAL 3, PARALLELPOINT 4 (and
    /// Direct3D 5's GLSPOT 5, a spot light).
    pub kind: u32,
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    pub ambient: [f32; 4],
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub range: f32,
    pub falloff: f32,
    pub attenuation: [f32; 3],
    pub theta: f32,
    pub phi: f32,
}

pub const LIGHT_SPOT: u32 = 2;
pub const LIGHT_DIRECTIONAL: u32 = 3;
pub const LIGHT_PARALLELPOINT: u32 = 4;
pub const LIGHT_GLSPOT: u32 = 5;

/// A material (D3DMATERIAL7's colors and power).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Material {
    pub diffuse: [f32; 4],
    pub ambient: [f32; 4],
    pub specular: [f32; 4],
    pub emissive: [f32; 4],
    pub power: f32,
}

/// Where a lit vertex takes a color from (D3DMCS_), if not the material.
pub const MCS_COLOR1: u32 = 1;
pub const MCS_COLOR2: u32 = 2;

/// How vertices are lit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lighting {
    pub lights: Vec<Light>,
    pub material: Material,
    /// The ambient light (D3DRENDERSTATE_AMBIENT).
    pub ambient: [f32; 4],
    pub specular: bool,
    pub local_viewer: bool,
    /// Whether the vertex's colors stand for the material's, and which
    /// for each of diffuse, specular, ambient and emissive (`MCS_`).
    pub color_vertex: bool,
    pub sources: [u32; 4],
}

/// D3DFOG_ modes.
pub const FOG_NONE: u32 = 0;
pub const FOG_EXP: u32 = 1;
pub const FOG_EXP2: u32 = 2;
pub const FOG_LINEAR: u32 = 3;

/// Fog computed for each vertex, into its specular color's alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fog {
    pub mode: u32,
    pub start: f32,
    pub end: f32,
    pub density: f32,
    /// The distance from the eye, instead of the depth.
    pub range: bool,
}

/// How much of a color is left after fog of `mode` at `depth`: 1 none
/// of it fogged, 0 all fog. Table fog uses it for each pixel.
pub fn fog_factor(mode: u32, start: f32, end: f32, density: f32, depth: f32) -> f32 {
    let f = match mode {
        FOG_EXP => (-density * depth).exp(),
        FOG_EXP2 => (-(density * depth) * (density * depth)).exp(),
        FOG_LINEAR => {
            if end == start {
                if depth < end { 1.0 } else { 0.0 }
            } else {
                (end - depth) / (end - start)
            }
        }
        _ => 1.0,
    };
    if f.is_nan() { 1.0 } else { f.clamp(0.0, 1.0) }
}

/// Geometry blending, Direct3D 8's D3DRS_VERTEXBLEND, which the
/// runtime's software vertex processing does whatever the card can.
#[derive(Clone, Debug, PartialEq)]
pub enum Blend {
    /// Each vertex goes through the world matrices `matrices` in
    /// proportion to its weights: `count` of them (0 to 3) from the
    /// vertex, and one more that is 1 minus their sum. The position
    /// that comes out is the sum of the vertex times each matrix times
    /// its weight; the normal is that of the normal through each
    /// matrix's inverse transpose (with the view's), as Wine's
    /// `process_vertices_strided` has it. With `indexed` the vertex's
    /// indices name the matrices among `matrices`, and an index past
    /// them names the last, which the caller makes the identity that
    /// every matrix not set is; without, they are the first 4.
    Weights {
        count: usize,
        indexed: bool,
        matrices: Vec<Matrix>,
    },
    /// D3DVBF_TWEENING: the position and normal are those of the
    /// vertex's first and second, the second's share the factor
    /// (D3DRS_TWEENFACTOR): position = position1 * (1 - factor) +
    /// position2 * factor, as DirectX documents it.
    Tween(f32),
}

/// D3DCULL_ modes.
#[cfg(test)]
pub const CULL_NONE: u32 = 1;
pub const CULL_CW: u32 = 2;
pub const CULL_CCW: u32 = 3;

/// D3DFILL_ and D3DSHADE_ modes.
pub const FILL_POINT: u32 = 1;
pub const FILL_WIREFRAME: u32 = 2;
pub const SHADE_FLAT: u32 = 1;

/// What the pipeline does with vertices and primitives: the device's
/// states as it needs them.
#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    pub world: Matrix,
    pub view: Matrix,
    pub projection: Matrix,
    pub view_map: ViewMap,
    /// The user's clip planes, in world space: a point is in when
    /// a x + b y + c z + d >= 0.
    pub planes: Vec<[f32; 4]>,
    pub lighting: Option<Lighting>,
    pub fog: Option<Fog>,
    /// Whether primitives are clipped at the view volume's near and far
    /// planes and the user's (D3DRENDERSTATE_CLIPPING, D3DDP_DONOTCLIP).
    /// They are always clipped to the guard band, past which the card
    /// can't take coordinates.
    pub clip: bool,
    /// The band primitives are always clipped to: left, top, right,
    /// bottom. The card's is `hal::vertex_band`, at or a little inside
    /// the guard band the device reports.
    pub guard_band: [f32; 4],
    pub cull: u32,
    pub fill: u32,
    pub shade: u32,
    /// What each of a processed vertex's eight slots of texture
    /// coordinates holds. For a draw, slot n is stage n's coordinates
    /// (`Coord::of`); for ProcessVertices, set n of the vertices it
    /// writes (`Setup::for_sets`).
    pub coords: [Coord; 8],
    /// D3DRENDERSTATE_NORMALIZENORMALS, for the normals that light
    /// vertices and that texture coordinates are made from.
    pub normalize: bool,
    /// Geometry blending, if the vertices are blended: `world` is then
    /// the first world matrix only for tweening.
    pub blend: Option<Blend>,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            world: IDENTITY,
            view: IDENTITY,
            projection: IDENTITY,
            view_map: ViewMap::dx7(0, 0, 1, 1, 0.0, 1.0),
            planes: Vec::new(),
            lighting: None,
            fog: None,
            clip: true,
            guard_band: super::hal::voodoo::VERTEX_BAND,
            cull: CULL_CCW,
            fill: 3,
            shade: 2,
            coords: std::array::from_fn(Coord::set),
            normalize: false,
            blend: None,
        }
    }
}

/// D3DTTFF_PROJECTED in D3DTSS_TEXTURETRANSFORMFLAGS.
pub const TTFF_PROJECTED: u32 = 0x100;

/// Where the texture coordinates in a slot of a processed vertex come
/// from: a stage's D3DTSS_TEXCOORDINDEX and
/// D3DTSS_TEXTURETRANSFORMFLAGS, and its texture matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coord {
    /// The vertex's coordinate set.
    pub set: usize,
    /// D3DTSS_TCI_, the index's upper half: 0 for the set, or the
    /// camera-space normal (1), position (2) or reflection vector (3),
    /// or Direct3D 8's sphere map (4), which Direct3D makes the
    /// coordinates from. A transformed vertex has none of them, and
    /// keeps its set.
    pub generate: u32,
    /// The texture matrix the coordinates go through, and how many
    /// come out (D3DTTFF_COUNT1 to 4). Not for a transformed vertex.
    pub matrix: Option<(Matrix, usize)>,
    /// D3DTTFF_PROJECTED: the coordinate the others are divided by at
    /// each pixel, which is the last one counted, or the fourth with
    /// no count.
    pub projected: Option<usize>,
}

impl Coord {
    /// The vertex's set `set` as it is.
    pub fn set(set: usize) -> Coord {
        Coord {
            set,
            generate: 0,
            matrix: None,
            projected: None,
        }
    }

    /// From a stage's TEXCOORDINDEX `index`, its TEXTURETRANSFORMFLAGS
    /// `flags` and its texture matrix.
    pub fn of(index: u32, flags: u32, matrix: &Matrix) -> Coord {
        let count = (flags & 0xFF).min(4) as usize;
        Coord {
            set: (index & 7) as usize,
            generate: index >> 16,
            matrix: (count > 0).then_some((*matrix, count)),
            projected: (flags & TTFF_PROJECTED != 0).then_some(match count {
                0 => 3,
                count => count - 1,
            }),
        }
    }

    /// Whether the slot is more than a set of the vertex.
    fn made(&self) -> bool {
        self.generate != 0 || self.matrix.is_some()
    }

    /// The coordinates of the slot for the vertex `v` at `eye` in
    /// camera space with the normal `normal` there, as the ToadForce
    /// makes them (`toadforce::raster::geometry::coordinates`): the set
    /// or what is generated, with a 1 after its last number, through
    /// the matrix. Of what comes out the first `count` stay.
    fn make(&self, layout: &Layout, v: &Vertex, eye: [f32; 3], normal: [f32; 3]) -> [f32; 4] {
        // A slot that is no more than a set, as nearly every one of
        // every draw is, is a copy.
        if !self.made() {
            return self.keep(layout, v);
        }
        let matrix = self.matrix.as_ref().map(|(m, count)| (m, *count));
        let set = (v.tex[self.set & 7], self.numbers(layout));
        crate::toadforce::raster::geometry::coordinates(self.generate, set, eye, normal, matrix)
    }

    /// How many numbers the slot's set has in a vertex of `layout`: 0
    /// for a set the vertex doesn't have.
    fn numbers(&self, layout: &Layout) -> usize {
        match self.set < layout.textures {
            true => layout.tex[self.set].1.clamp(1, 4) as usize,
            false => 0,
        }
    }

    /// The vertex's set for the slot, as a transformed vertex keeps it
    /// and as `coordinates` gives a set with no matrix: its numbers,
    /// 0 for those it doesn't have, and 1 for a fourth it doesn't have.
    fn keep(&self, layout: &Layout, v: &Vertex) -> [f32; 4] {
        let numbers = self.numbers(layout);
        let mut c = [0.0, 0.0, 0.0, 1.0];
        c[..numbers].copy_from_slice(&v.tex[self.set & 7][..numbers]);
        c
    }

    /// The coordinates `c` of the slot as a card takes a stage's: the
    /// two (or, for a cube, three) it samples at, and in the fourth
    /// what it divides them by at each pixel, 1 unless the stage is
    /// projected.
    fn sampled(&self, c: [f32; 4]) -> [f32; 4] {
        match self.projected {
            None => [c[0], c[1], c[2], 1.0],
            Some(by) => std::array::from_fn(|i| match i {
                3 => c[by],
                i if i < by => c[i],
                _ => 0.0,
            }),
        }
    }
}

impl Setup {
    /// The setup for ProcessVertices, whose vertices keep their sets:
    /// slot n is set n. A stage that generates its coordinates or has
    /// a texture matrix puts them in the set its index names, where a
    /// draw of the vertices it wrote, which are transformed, finds
    /// them. The first such stage has a set.
    pub fn for_sets(mut self) -> Setup {
        let stages = self.coords;
        self.coords = std::array::from_fn(Coord::set);
        for stage in stages.iter().filter(|c| c.made()) {
            let slot = &mut self.coords[stage.set & 7];
            if !slot.made() {
                *slot = Coord {
                    projected: None,
                    ..*stage
                };
            }
        }
        self
    }
}

/// A vertex through transform and lighting.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Processed {
    /// Clip coordinates; for a transformed vertex, the screen's x, y and
    /// z with w 1.
    pub clip: [f32; 4],
    /// A transformed vertex's rhw.
    pub rhw: f32,
    pub tl: bool,
    /// Its distances from the user's clip planes.
    pub planes: [f32; 6],
    pub diffuse: [f32; 4],
    /// The specular color, its alpha the fog factor.
    pub specular: [f32; 4],
    /// The texture coordinates of each slot (`Setup::coords`).
    pub tex: [[f32; 4]; 8],
    /// The planes it is outside of (D3DCLIP_).
    pub code: u32,
}

impl Processed {
    /// Take the colors of `provoking`, as flat shading does: its diffuse
    /// color and the red, green and blue of its specular one. The fog
    /// factor in the specular alpha stays this vertex's own, so fog is
    /// still interpolated across the face. D3DSHADE_FLAT is documented
    /// so ("the specular alpha is interpolated"), Wine has it so (the
    /// fog coordinate, which OpenGL's flat shading leaves alone), and a
    /// Voodoo iterates fog through W or Z, never flat.
    fn take_flat_colors(&mut self, provoking: &Processed) {
        let fog = self.specular[3];
        self.diffuse = provoking.diffuse;
        self.specular = provoking.specular;
        self.specular[3] = fog;
    }

    fn lerp(&self, o: &Processed, t: f32) -> Processed {
        let l = |a: f32, b: f32| a + (b - a) * t;
        let l4 =
            |a: [f32; 4], b: [f32; 4]| [l(a[0], b[0]), l(a[1], b[1]), l(a[2], b[2]), l(a[3], b[3])];
        let mut p = Processed {
            clip: l4(self.clip, o.clip),
            rhw: l(self.rhw, o.rhw),
            tl: self.tl,
            diffuse: l4(self.diffuse, o.diffuse),
            specular: l4(self.specular, o.specular),
            code: 0,
            ..Processed::default()
        };
        for i in 0..6 {
            p.planes[i] = l(self.planes[i], o.planes[i]);
        }
        // Transformed vertices are cut on the screen, where the card
        // iterates the texture coordinates divided by w, and rhw: the
        // point on the edge gets the coordinates the card would have
        // drawn there. The colors, z and rhw, which the card iterates as
        // they are, come from the straight line. Cut straight, the
        // coordinates of a floor whose near corners lie far past the
        // guard band stretch across what is left of it (Omikron, #280).
        // The HAL draws every texture in perspective, whatever
        // D3DRENDERSTATE_TEXTUREPERSPECTIVE says; a cut has to follow it
        // if it ever draws one without (#838).
        let rhw = [self.rhw, o.rhw];
        let perspective = self.tl && o.tl && rhw.iter().all(|w| w.is_finite() && *w > 0.0);
        for i in 0..8 {
            p.tex[i] = if perspective {
                let (a, b) = (self.tex[i], o.tex[i]);
                std::array::from_fn(|k| l(a[k] * rhw[0], b[k] * rhw[1]) / p.rhw)
            } else {
                l4(self.tex[i], o.tex[i])
            };
        }
        p
    }
}

/// A vertex on the screen, as the card's triangle setup takes it: x, y,
/// z (0 to 1), rhw, the colors (the specular's alpha the fog factor),
/// and the coordinates of the textures of the first `SCREEN_STAGES`
/// stages, each with the number it is divided by at each pixel in the
/// fourth place (1 unless the stage's coordinates are projected).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScreenVertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub rhw: f32,
    pub diffuse: u32,
    pub specular: u32,
    pub tex: [[f32; 4]; SCREEN_STAGES],
}

/// The stages whose texture coordinates a screen vertex carries: the
/// most textures a card draws with at once, the ToadForce's four. A
/// Voodoo takes those of its two units.
pub const SCREEN_STAGES: usize = 4;

/// What draws the primitives that come out of the pipeline: the card.
/// Coordinates are inside the guard band, not the viewport: the
/// rasterizer clips to the viewport's rectangle itself.
pub trait Rasterizer {
    fn triangle(&mut self, v: &[ScreenVertex; 3]);
    fn line(&mut self, v: &[ScreenVertex; 2]);
    fn point(&mut self, v: &ScreenVertex);
}

/// A rasterizer that only counts what it is given.
#[derive(Clone, Debug, Default)]
pub struct Counter {
    pub triangles: u32,
    pub lines: u32,
    pub points: u32,
}

impl Rasterizer for Counter {
    fn triangle(&mut self, _: &[ScreenVertex; 3]) {
        self.triangles += 1;
    }
    fn line(&mut self, _: &[ScreenVertex; 2]) {
        self.lines += 1;
    }
    fn point(&mut self, _: &ScreenVertex) {
        self.points += 1;
    }
}

// Transform and lighting.

/// The lights in camera space, with what lighting needs of them.
struct EyeLight {
    light: Light,
    position: [f32; 3],
    /// For a directional or parallel point light: toward the light.
    toward: [f32; 3],
    /// The way a spot light points.
    axis: [f32; 3],
}

/// Run `input`, of `layout`, through transform, lighting and fog.
pub fn process(setup: &Setup, input: &[Vertex], layout: &Layout) -> Vec<Processed> {
    if layout.rhw {
        return input
            .iter()
            .map(|v| transformed(setup, v, layout))
            .collect();
    }
    let world_view = multiply(&setup.world, &setup.view);
    let all = multiply(&world_view, &setup.projection);
    let normals = normal_matrix(&world_view);
    let lights: Vec<EyeLight> = setup
        .lighting
        .as_ref()
        .map(|l| {
            let origin = point([0.0; 3], &world_view);
            l.lights
                .iter()
                .map(|light| {
                    let position = point(light.position, &setup.view);
                    let toward = match light.kind {
                        LIGHT_DIRECTIONAL => {
                            normalize(vector(light.direction, &setup.view).map(|c| -c))
                        }
                        LIGHT_PARALLELPOINT => normalize(sub(position, origin)),
                        _ => [0.0; 3],
                    };
                    EyeLight {
                        light: *light,
                        position,
                        toward,
                        axis: normalize(vector(light.direction, &setup.view)),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let mut blended = Blended::of(setup);
    let tween = match setup.blend {
        Some(Blend::Tween(f)) => Some(f),
        _ => None,
    };
    // The normal is worked out for lighting and for the coordinates
    // made from it.
    let lit = setup.lighting.is_some();
    let normal = lit || setup.coords.iter().any(|c| matches!(c.generate, 1 | 3 | 4));
    input
        .iter()
        .map(|v| {
            let tweened;
            let v = match tween {
                Some(f) => {
                    tweened = tween_vertex(v, f);
                    &tweened
                }
                None => v,
            };
            let p = [v.position[0], v.position[1], v.position[2], 1.0];
            // Where the vertex is in the world, its clip coordinates,
            // where it is in camera space and its normal there.
            let (world, clip, eye, n) = match blended.as_mut() {
                Some(b) => {
                    let (w, n) = b.place(v, normal);
                    let e = transform(w, &setup.view);
                    (w, transform(w, &b.view_projection), [e[0], e[1], e[2]], n)
                }
                None => {
                    let n = if normal {
                        times(v.normal, &normals)
                    } else {
                        [0.0; 3]
                    };
                    let w = if setup.planes.is_empty() {
                        p
                    } else {
                        transform(p, &setup.world)
                    };
                    let eye = point([p[0], p[1], p[2]], &world_view);
                    (w, transform(p, &all), eye, n)
                }
            };
            let n = if setup.normalize { normalize(n) } else { n };
            let mut out = Processed {
                clip,
                tex: std::array::from_fn(|slot| setup.coords[slot].make(layout, v, eye, n)),
                ..Processed::default()
            };
            for (i, plane) in setup.planes.iter().take(6).enumerate() {
                out.planes[i] = plane[0] * world[0]
                    + plane[1] * world[1]
                    + plane[2] * world[2]
                    + plane[3] * world[3];
            }
            match &setup.lighting {
                Some(l) => {
                    let (d, s) = light_vertex(l, &lights, v, eye, n);
                    out.diffuse = d;
                    out.specular = s;
                }
                None => {
                    out.diffuse = rgba(v.diffuse.unwrap_or(0xFFFF_FFFF));
                    out.specular = rgba(v.specular.unwrap_or(0xFF00_0000));
                }
            }
            if let Some(f) = &setup.fog {
                let depth = if f.range { length(eye) } else { eye[2].abs() };
                out.specular[3] = fog_factor(f.mode, f.start, f.end, f.density, depth);
            }
            out.code = codes(setup, &out);
            out
        })
        .collect()
}

/// The vertex `v` tweened by `factor`: its position, and its normal,
/// moved that share of the way to its second ones. A vertex without a
/// second one keeps its first.
fn tween_vertex(v: &Vertex, factor: f32) -> Vertex {
    let mix =
        |a: [f32; 3], b: [f32; 3]| std::array::from_fn(|k| a[k] * (1.0 - factor) + b[k] * factor);
    let mut out = *v;
    if let Some(p2) = v.position2 {
        let p = mix([v.position[0], v.position[1], v.position[2]], p2);
        out.position = [p[0], p[1], p[2], v.position[3]];
    }
    if let Some(n2) = v.normal2 {
        out.normal = mix(v.normal, n2);
    }
    out
}

/// What blending by weights needs while a draw's vertices go through
/// it: the setup's matrices, and the normal matrices of those the
/// vertices use, made when the first one does.
struct Blended<'a> {
    count: usize,
    indexed: bool,
    matrices: &'a [Matrix],
    view: Matrix,
    view_projection: Matrix,
    normals: Vec<Option<[[f32; 3]; 3]>>,
}

impl<'a> Blended<'a> {
    fn of(setup: &'a Setup) -> Option<Blended<'a>> {
        let Some(Blend::Weights {
            count,
            indexed,
            matrices,
        }) = &setup.blend
        else {
            return None;
        };
        if matrices.is_empty() {
            return None;
        }
        Some(Blended {
            count: (*count).min(3),
            indexed: *indexed,
            matrices,
            view: setup.view,
            view_projection: multiply(&setup.view, &setup.projection),
            normals: vec![None; matrices.len()],
        })
    }

    /// The vertex `v` blended: where it is in the world, and with `lit`
    /// its normal in camera space.
    fn place(&mut self, v: &Vertex, lit: bool) -> ([f32; 4], [f32; 3]) {
        let p = [v.position[0], v.position[1], v.position[2], 1.0];
        let n = self.count;
        let mut weights = [0.0f32; 4];
        weights[..n].copy_from_slice(&v.weights[..n]);
        weights[n] = v.weights[..n].iter().fold(1.0, |last, w| last - w);
        let mut world = [0.0f32; 4];
        let mut normal = [0.0f32; 3];
        for (k, &weight) in weights.iter().enumerate().take(self.count + 1) {
            // A matrix of no weight adds nothing.
            if weight == 0.0 {
                continue;
            }
            // An index past the matrices names the last (see `Blend`).
            let m = if self.indexed {
                v.indices[k] as usize
            } else {
                k
            }
            .min(self.matrices.len() - 1);
            let t = transform(p, &self.matrices[m]);
            for (w, t) in world.iter_mut().zip(t) {
                *w += weight * t;
            }
            if lit {
                let view = &self.view;
                let matrix = &self.matrices[m];
                let nm =
                    self.normals[m].get_or_insert_with(|| normal_matrix(&multiply(matrix, view)));
                let n = times(v.normal, nm);
                for (a, n) in normal.iter_mut().zip(n) {
                    *a += weight * n;
                }
            }
        }
        (world, normal)
    }
}

/// The transformed vertex `p` at the clip coordinates `clip` it was
/// transformed from, `planes` its distances from the user's clip planes,
/// so that it is clipped as a vertex the pipeline transforms is: one of
/// a vertex buffer that ProcessVertices clipped. Its colors and texture
/// coordinates stay the buffer's.
pub fn at_clip_coordinates(
    setup: &Setup,
    p: &Processed,
    clip: [f32; 4],
    planes: [f32; 6],
) -> Processed {
    let mut out = Processed {
        clip,
        planes,
        rhw: 0.0,
        tl: false,
        ..*p
    };
    out.code = codes(setup, &out);
    out
}

/// A transformed vertex, as it is, with its sets in the slots that
/// take them.
fn transformed(setup: &Setup, v: &Vertex, layout: &Layout) -> Processed {
    Processed {
        clip: [v.position[0], v.position[1], v.position[2], 1.0],
        rhw: v.position[3],
        tl: true,
        diffuse: rgba(v.diffuse.unwrap_or(0xFFFF_FFFF)),
        specular: rgba(v.specular.unwrap_or(0xFF00_0000)),
        tex: std::array::from_fn(|slot| setup.coords[slot].keep(layout, v)),
        ..Processed::default()
    }
}

/// The planes a vertex is outside of.
pub fn codes(setup: &Setup, p: &Processed) -> u32 {
    if p.tl {
        return 0;
    }
    let [x, y, z, w] = p.clip;
    let b = setup.view_map.bounds;
    let mut code = 0;
    if x < b[0] * w {
        code |= CLIP_LEFT;
    }
    if x > b[1] * w {
        code |= CLIP_RIGHT;
    }
    if y < b[2] * w {
        code |= CLIP_BOTTOM;
    }
    if y > b[3] * w {
        code |= CLIP_TOP;
    }
    if z < b[4] * w {
        code |= CLIP_FRONT;
    }
    if z > b[5] * w {
        code |= CLIP_BACK;
    }
    for i in 0..setup.planes.len().min(6) {
        if p.planes[i] < 0.0 {
            code |= CLIP_GEN0 << i;
        }
    }
    code
}

/// The diffuse and specular colors of a vertex at `eye` (camera space)
/// with the normal `n`.
fn light_vertex(
    l: &Lighting,
    lights: &[EyeLight],
    v: &Vertex,
    eye: [f32; 3],
    n: [f32; 3],
) -> ([f32; 4], [f32; 4]) {
    let source = |which: usize, material: [f32; 4]| -> [f32; 4] {
        if !l.color_vertex {
            return material;
        }
        match l.sources[which] {
            MCS_COLOR1 => v.diffuse.map_or(material, rgba),
            MCS_COLOR2 => v.specular.map_or(material, rgba),
            _ => material,
        }
    };
    let m_diffuse = source(0, l.material.diffuse);
    let m_specular = source(1, l.material.specular);
    let m_ambient = source(2, l.material.ambient);
    let m_emissive = source(3, l.material.emissive);
    let mut ambient = [l.ambient[0], l.ambient[1], l.ambient[2]];
    let mut diffuse = [0.0f32; 3];
    let mut specular = [0.0f32; 3];
    let to_eye = if l.local_viewer {
        normalize(eye.map(|c| -c))
    } else {
        [0.0, 0.0, -1.0]
    };
    for e in lights {
        let light = &e.light;
        let (dir, atten) = match light.kind {
            LIGHT_DIRECTIONAL | LIGHT_PARALLELPOINT => (e.toward, 1.0),
            _ => {
                let to = sub(e.position, eye);
                let d = length(to);
                if light.range > 0.0 && d > light.range {
                    continue;
                }
                let a = &light.attenuation;
                let denom = a[0] + a[1] * d + a[2] * d * d;
                let atten = if denom > 0.0 { 1.0 / denom } else { 1.0 };
                (normalize(to), atten)
            }
        };
        let spot = if matches!(light.kind, LIGHT_SPOT | LIGHT_GLSPOT) {
            let rho = -dot(dir, e.axis);
            let (inner, outer) = ((light.theta / 2.0).cos(), (light.phi / 2.0).cos());
            // Outside the outer cone first, as the card has it: a
            // light whose inner cone is the wider one, which Direct3D
            // doesn't define, then lights the same on both.
            if rho <= outer {
                0.0
            } else if rho > inner {
                1.0
            } else if inner > outer {
                let f = (rho - outer) / (inner - outer);
                if light.falloff == 1.0 {
                    f
                } else {
                    f.powf(light.falloff)
                }
            } else {
                1.0
            }
        } else {
            1.0
        };
        let factor = atten * spot;
        if factor <= 0.0 || !factor.is_finite() {
            continue;
        }
        for (a, l) in ambient.iter_mut().zip(light.ambient) {
            *a += l * factor;
        }
        let nl = dot(n, dir);
        if nl > 0.0 {
            for (d, l) in diffuse.iter_mut().zip(light.diffuse) {
                *d += l * nl * factor;
            }
            if l.specular {
                let h = normalize([dir[0] + to_eye[0], dir[1] + to_eye[1], dir[2] + to_eye[2]]);
                let nh = dot(n, h);
                if nh > 0.0 {
                    let p = nh.powf(l.material.power);
                    if p.is_finite() {
                        for (s, l) in specular.iter_mut().zip(light.specular) {
                            *s += l * p * factor;
                        }
                    }
                }
            }
        }
    }
    let mut out_d = [0.0; 4];
    let mut out_s = [0.0; 4];
    for c in 0..3 {
        out_d[c] =
            (m_emissive[c] + m_ambient[c] * ambient[c] + m_diffuse[c] * diffuse[c]).clamp(0.0, 1.0);
        out_s[c] = (m_specular[c] * specular[c]).clamp(0.0, 1.0);
    }
    out_d[3] = m_diffuse[3].clamp(0.0, 1.0);
    out_s[3] = 1.0;
    (out_d, out_s)
}

// Primitives.

/// D3DPRIMITIVETYPE.
pub const POINTLIST: u32 = 1;
pub const LINELIST: u32 = 2;
pub const LINESTRIP: u32 = 3;
pub const TRIANGLELIST: u32 = 4;
pub const TRIANGLESTRIP: u32 = 5;
pub const TRIANGLEFAN: u32 = 6;

/// A primitive, by the places of its vertices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prim {
    Point(u32),
    /// The vertices from and to, and the one flat shading takes the
    /// colors of.
    Line(u32, u32, u32),
    /// The vertices in order, the one flat shading takes the colors of,
    /// and the edges wireframe draws (bit 0 the first to the second, 1
    /// the second to the third, 2 the third to the first).
    Triangle([u32; 3], u32, u8),
}

/// The primitives of `kind` that the vertices `order` (places in the
/// vertices) make, or None for a kind there isn't.
pub fn assemble(kind: u32, order: &[u32]) -> Option<Vec<Prim>> {
    let n = order.len();
    let mut out = Vec::new();
    match kind {
        POINTLIST => out.extend(order.iter().map(|&i| Prim::Point(i))),
        LINELIST => out.extend(
            order
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| Prim::Line(c[0], c[1], c[0])),
        ),
        LINESTRIP => out.extend(order.windows(2).map(|c| Prim::Line(c[0], c[1], c[0]))),
        TRIANGLELIST => out.extend(
            order
                .as_chunks::<3>()
                .0
                .iter()
                .map(|c| Prim::Triangle([c[0], c[1], c[2]], c[0], 7)),
        ),
        TRIANGLESTRIP => {
            for i in 0..n.saturating_sub(2) {
                let (a, b, c) = (order[i], order[i + 1], order[i + 2]);
                let v = if i % 2 == 0 { [a, b, c] } else { [b, a, c] };
                out.push(Prim::Triangle(v, a, 7));
            }
        }
        TRIANGLEFAN => {
            for i in 1..n.saturating_sub(1) {
                out.push(Prim::Triangle(
                    [order[0], order[i], order[i + 1]],
                    order[i],
                    7,
                ));
            }
        }
        _ => return None,
    }
    Some(out)
}

/// How many vertices a primitive of `kind` needs at least.
pub fn min_vertices(kind: u32) -> usize {
    match kind {
        POINTLIST => 1,
        LINELIST | LINESTRIP => 2,
        _ => 3,
    }
}

/// What a draw drew.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drawn {
    pub triangles: u32,
    pub lines: u32,
    pub points: u32,
}

/// A plane of homogeneous clipping, as how far inside it a vertex is
/// (`Plane::distance`). Each kind is one expression of the clip
/// coordinates, `c` a coordinate's index and `k` a constant.
#[derive(Clone, Copy, Debug)]
enum Plane {
    /// clip[c] - k
    Above(usize, f32),
    /// k - clip[c]
    Below(usize, f32),
    /// clip[c] - k * w
    AboveW(usize, f32),
    /// k * w - clip[c]
    BelowW(usize, f32),
    /// The vertex's distance from user clip plane `i`.
    User(usize),
}

impl Plane {
    fn distance(self, p: &Processed) -> f32 {
        match self {
            Plane::Above(c, k) => p.clip[c] - k,
            Plane::Below(c, k) => k - p.clip[c],
            Plane::AboveW(c, k) => p.clip[c] - k * p.clip[3],
            Plane::BelowW(c, k) => k * p.clip[3] - p.clip[c],
            Plane::User(i) => p.planes[i],
        }
    }
}

/// The most clip planes: near, far, the 6 user planes and the guard
/// band's 4.
const MAX_PLANES: usize = 12;

/// The planes a draw clips at (`clip_planes`), kept in place.
type Planes = FixedVec<Plane, MAX_PLANES>;

/// The planes primitives are clipped at: the near and far planes and
/// the user's when the setup clips, and the guard band always. A
/// transformed vertex's near and far planes are z 0 and 1, as Wine's
/// depth_clamp_test has them on Windows.
fn clip_planes(setup: &Setup, tl: bool) -> Planes {
    let mut planes = Planes::new(Plane::User(0));
    let band = setup.guard_band;
    if tl {
        if setup.clip {
            planes.push(Plane::Above(2, 0.0));
            planes.push(Plane::Below(2, 1.0));
        }
        planes.push(Plane::Above(0, band[0]));
        planes.push(Plane::Below(0, band[2]));
        planes.push(Plane::Above(1, band[1]));
        planes.push(Plane::Below(1, band[3]));
        return planes;
    }
    let b = setup.view_map.bounds;
    if setup.clip {
        planes.push(Plane::AboveW(2, b[4]));
        planes.push(Plane::BelowW(2, b[5]));
        for i in 0..setup.planes.len().min(6) {
            planes.push(Plane::User(i));
        }
    } else {
        // Nothing behind the eye can be divided by its w.
        planes.push(Plane::Above(3, 1e-6));
    }
    let g = setup.view_map.guard(band);
    planes.push(Plane::AboveW(0, g[0]));
    planes.push(Plane::BelowW(0, g[1]));
    planes.push(Plane::AboveW(1, g[2]));
    planes.push(Plane::BelowW(1, g[3]));
    planes
}

/// The most vertices a clipped triangle keeps in place: each plane cuts
/// off a corner, adding one vertex at most, as long as the rounding of
/// the distances doesn't make the polygon cross a plane more than twice
/// (`clip_polygon` falls back to `clip_spilled` then).
const MAX_POLYGON: usize = 3 + MAX_PLANES;

/// A convex polygon, each vertex with whether the edge from it to the
/// next is to be drawn in wireframe, kept in place.
type Polygon = FixedVec<(Processed, bool), MAX_POLYGON>;

/// The two polygons a draw clips its triangles between
/// (`clip_polygon`), made once for all of them: each is a few kilobytes.
fn clip_scratch() -> [Polygon; 2] {
    [Polygon::new((Processed::default(), false)); 2]
}

/// Clip the triangle `tri` at `planes`, from one polygon of `scratch`
/// into the other and back: the polygon left. None if a plane would
/// leave more vertices than a polygon has room for: the caller clips
/// the triangle again with `clip_spilled`, which gives the same polygon.
fn clip_polygon<'a>(
    tri: &[(Processed, bool); 3],
    planes: &[Plane],
    scratch: &'a mut [Polygon; 2],
) -> Option<&'a Polygon> {
    let [poly, out] = scratch;
    let (mut poly, mut out) = (poly, out);
    poly.clear();
    poly.extend_from_slice(tri);
    for &plane in planes {
        if poly.is_empty() {
            break;
        }
        let mut d = [0.0f32; MAX_POLYGON];
        for (slot, (v, _)) in d.iter_mut().zip(poly.iter()) {
            *slot = plane.distance(v);
        }
        let d = &d[..poly.len()];
        if d.iter().all(|&d| d >= 0.0) {
            continue;
        }
        // Each vertex inside stays, and each edge across the plane adds
        // the point where it crosses.
        let kept = (0..poly.len())
            .map(|i| {
                let (inside, next) = (d[i] >= 0.0, d[(i + 1) % poly.len()] >= 0.0);
                inside as usize + (inside != next) as usize
            })
            .sum::<usize>();
        if kept > MAX_POLYGON {
            return None;
        }
        out.clear();
        for i in 0..poly.len() {
            let j = (i + 1) % poly.len();
            let (a, edge) = poly[i];
            let (b, _) = poly[j];
            let (da, db) = (d[i], d[j]);
            if da >= 0.0 {
                out.push((a, edge));
                if db < 0.0 {
                    out.push((a.lerp(&b, da / (da - db)), false));
                }
            } else if db >= 0.0 {
                out.push((a.lerp(&b, da / (da - db)), edge));
            }
        }
        // The references swap, not the polygons.
        std::mem::swap(&mut poly, &mut out);
    }
    Some(poly)
}

/// `clip_polygon` with no limit on the vertices, for a triangle that
/// rounding makes cross a plane more than twice.
#[cold]
fn clip_spilled(mut poly: Vec<(Processed, bool)>, planes: &[Plane]) -> Vec<(Processed, bool)> {
    for &plane in planes {
        if poly.is_empty() {
            break;
        }
        let d: Vec<f32> = poly.iter().map(|(v, _)| plane.distance(v)).collect();
        if d.iter().all(|&d| d >= 0.0) {
            continue;
        }
        let mut out = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let j = (i + 1) % poly.len();
            let (a, edge) = poly[i];
            let (b, _) = poly[j];
            let (da, db) = (d[i], d[j]);
            if da >= 0.0 {
                out.push((a, edge));
                if db < 0.0 {
                    out.push((a.lerp(&b, da / (da - db)), false));
                }
            } else if db >= 0.0 {
                out.push((a.lerp(&b, da / (da - db)), edge));
            }
        }
        poly = out;
    }
    poly
}

/// Where a processed vertex is on the screen.
fn screen(setup: &Setup, p: &Processed) -> ScreenVertex {
    let [x, y, z, rhw] = if p.tl {
        [p.clip[0], p.clip[1], p.clip[2], p.rhw]
    } else {
        setup.view_map.screen(p.clip)
    };
    ScreenVertex {
        x,
        y,
        z,
        rhw,
        diffuse: color(p.diffuse),
        specular: color(p.specular),
        tex: std::array::from_fn(|stage| setup.coords[stage].sampled(p.tex[stage])),
    }
}

/// Whether a vertex can be clipped and divided: no NaN or infinity.
fn finite(p: &Processed) -> bool {
    p.clip.iter().all(|c| c.is_finite())
}

/// Draw the primitives `prims` of the vertices `verts` on `raster`.
pub fn draw(
    setup: &Setup,
    verts: &[Processed],
    prims: &[Prim],
    raster: &mut dyn Rasterizer,
) -> Drawn {
    let mut drawn = Drawn::default();
    let planes = clip_planes(setup, false);
    let tl_planes = clip_planes(setup, true);
    let mut scratch = clip_scratch();
    let flat = setup.shade == SHADE_FLAT;
    for prim in prims {
        match *prim {
            Prim::Point(i) => {
                let Some(v) = verts.get(i as usize) else {
                    continue;
                };
                let planes = if v.tl { &tl_planes } else { &planes };
                if !finite(v) || planes.iter().any(|p| p.distance(v) < 0.0) {
                    continue;
                }
                raster.point(&screen(setup, v));
                drawn.points += 1;
            }
            Prim::Line(a, b, provoking) => {
                let (Some(va), Some(vb)) = (verts.get(a as usize), verts.get(b as usize)) else {
                    continue;
                };
                let (mut va, mut vb) = (*va, *vb);
                if flat && let Some(p) = verts.get(provoking as usize) {
                    va.take_flat_colors(p);
                    vb.take_flat_colors(p);
                }
                if line(setup, &va, &vb, &planes, &tl_planes, raster) {
                    drawn.lines += 1;
                }
            }
            Prim::Triangle(idx, provoking, edges) => {
                if idx.iter().any(|&i| i as usize >= verts.len()) {
                    continue;
                }
                let mut tri = idx.map(|i| verts[i as usize]);
                if flat && let Some(p) = verts.get(provoking as usize) {
                    for t in tri.iter_mut() {
                        t.take_flat_colors(p);
                    }
                }
                let planes = [&planes[..], &tl_planes[..]];
                triangle(setup, tri, edges, planes, &mut scratch, raster, &mut drawn);
            }
        }
    }
    drawn
}

/// Draw a line from `a` to `b`, clipped: whether any of it was drawn.
fn line(
    setup: &Setup,
    a: &Processed,
    b: &Processed,
    planes: &[Plane],
    tl_planes: &[Plane],
    raster: &mut dyn Rasterizer,
) -> bool {
    if !finite(a) || !finite(b) {
        return false;
    }
    let ([a, b], tl) = one_kind(setup, [*a, *b]);
    let planes = if tl { tl_planes } else { planes };
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for plane in planes {
        let (da, db) = (plane.distance(&a), plane.distance(&b));
        if da < 0.0 && db < 0.0 {
            return false;
        }
        if da < 0.0 {
            t0 = t0.max(da / (da - db));
        } else if db < 0.0 {
            t1 = t1.min(da / (da - db));
        }
    }
    if t0 > t1 {
        return false;
    }
    let (ca, cb) = (a.lerp(&b, t0), a.lerp(&b, t1));
    raster.line(&[screen(setup, &ca), screen(setup, &cb)]);
    true
}

/// The vertices of a primitive as all of one kind, and whether they are
/// transformed ones. A primitive that mixes transformed vertices with
/// vertices at clip coordinates (an execute buffer's, or a vertex
/// buffer's that ProcessVertices kept) is clipped at the view volume,
/// its transformed vertices taken back to the clip coordinates the
/// viewport puts where they are: a vertex behind the eye has no screen
/// position it could be clipped on the screen from. Only when a
/// transformed vertex has no such coordinates (`ViewMap::clip`) is the
/// primitive clipped on the screen.
fn one_kind<const N: usize>(setup: &Setup, verts: [Processed; N]) -> ([Processed; N], bool) {
    let tl = verts.iter().filter(|v| v.tl).count();
    if tl == 0 || tl == N {
        return (verts, tl != 0);
    }
    let clip = verts.map(|v| as_clip(setup, &v));
    if clip.iter().all(Option::is_some) {
        return (clip.map(Option::unwrap_or_default), false);
    }
    (verts.map(|v| as_tl(setup, &v)), true)
}

/// A transformed vertex at the clip coordinates that the viewport puts
/// where it is, if it has them. Its rhw is 1/w, so the screen position
/// gives w back with its sign: a vertex behind the eye has a negative w,
/// and its clip coordinates clip like any other. It is on the user's
/// clip planes, not outside them: transformed vertices aren't clipped at
/// them.
fn as_clip(setup: &Setup, p: &Processed) -> Option<Processed> {
    if !p.tl {
        return Some(*p);
    }
    let [x, y, z, _] = p.clip;
    let clip = setup.view_map.clip([x, y, z, p.rhw])?;
    let out = at_clip_coordinates(setup, p, clip, [0.0; 6]);
    finite(&out).then_some(out)
}

/// A vertex as a transformed one, for a primitive that mixes them and
/// can't be clipped at the view volume.
fn as_tl(setup: &Setup, p: &Processed) -> Processed {
    if p.tl {
        return *p;
    }
    let s = setup.view_map.screen(p.clip);
    Processed {
        clip: [s[0], s[1], s[2], 1.0],
        rhw: s[3],
        tl: true,
        ..*p
    }
}

/// Draw the triangle `tri`, clipped at `planes` (those of vertices at
/// clip coordinates, and those of transformed ones) between the
/// polygons of `scratch`.
fn triangle(
    setup: &Setup,
    tri: [Processed; 3],
    edges: u8,
    [planes, tl_planes]: [&[Plane]; 2],
    scratch: &mut [Polygon; 2],
    raster: &mut dyn Rasterizer,
    drawn: &mut Drawn,
) {
    if !tri.iter().all(finite) {
        return;
    }
    let (tri, tl) = one_kind(setup, tri);
    // All outside one plane of the view volume: nothing to draw. With
    // no clipping, depth past the near and far planes is clamped, and
    // only the sides count.
    let sides = CLIP_LEFT | CLIP_RIGHT | CLIP_TOP | CLIP_BOTTOM;
    let mask = if setup.clip { u32::MAX } else { sides };
    if !tl && tri[0].code & tri[1].code & tri[2].code & mask != 0 {
        return;
    }
    let corners = [
        (tri[0], edges & 1 != 0),
        (tri[1], edges & 2 != 0),
        (tri[2], edges & 4 != 0),
    ];
    let planes = if tl { tl_planes } else { planes };
    // Most triangles are inside every plane, which leaves them whole.
    // The planes before the first with a corner outside it leave the
    // triangle as it is, so clipping starts at that one.
    let inside = |p: &Plane| corners.iter().all(|(v, _)| p.distance(v) >= 0.0);
    let Some(first) = planes.iter().position(|p| !inside(p)) else {
        polygon(setup, &corners, raster, drawn);
        return;
    };
    let planes = &planes[first..];
    match clip_polygon(&corners, planes, scratch) {
        Some(poly) => polygon(setup, poly, raster, drawn),
        None => {
            let spilled = clip_spilled(corners.to_vec(), planes);
            polygon(setup, &spilled, raster, drawn)
        }
    }
}

/// Draw the clipped polygon `poly` of a triangle, as the fill mode says,
/// unless it is culled.
fn polygon(
    setup: &Setup,
    poly: &[(Processed, bool)],
    raster: &mut dyn Rasterizer,
    drawn: &mut Drawn,
) {
    if poly.len() < 3 {
        return;
    }
    let mut kept = [ScreenVertex::default(); MAX_POLYGON];
    let spilled: Vec<ScreenVertex>;
    let screens = if poly.len() <= MAX_POLYGON {
        for (s, (v, _)) in kept.iter_mut().zip(poly) {
            *s = screen(setup, v);
        }
        &kept[..poly.len()]
    } else {
        spilled = poly.iter().map(|(v, _)| screen(setup, v)).collect();
        &spilled[..]
    };
    let mut area = 0.0;
    for i in 0..screens.len() {
        let (a, b) = (&screens[i], &screens[(i + 1) % screens.len()]);
        area += a.x * b.y - b.x * a.y;
    }
    let culled = match setup.cull {
        CULL_CW => area > 0.0,
        CULL_CCW => area < 0.0,
        _ => false,
    };
    if culled {
        return;
    }
    match setup.fill {
        FILL_POINT => {
            for v in screens {
                raster.point(v);
                drawn.points += 1;
            }
        }
        FILL_WIREFRAME => {
            for i in 0..screens.len() {
                if poly[i].1 {
                    raster.line(&[screens[i], screens[(i + 1) % screens.len()]]);
                    drawn.lines += 1;
                }
            }
        }
        _ => {
            for i in 1..screens.len() - 1 {
                raster.triangle(&[screens[0], screens[i], screens[i + 1]]);
            }
            drawn.triangles += 1;
        }
    }
}

/// The planes all of `verts` are outside of, and those any is: for the
/// clip status.
pub fn clip_union(verts: &[Processed]) -> (u32, u32) {
    let mut union = 0;
    let mut intersection = if verts.is_empty() { 0 } else { u32::MAX };
    for v in verts {
        union |= v.code;
        intersection &= v.code;
    }
    (intersection, union)
}

/// The bounding rectangle of the vertices on the screen: x from, to, y
/// from, to.
pub fn extents(setup: &Setup, verts: &[Processed]) -> Option<[f32; 4]> {
    let mut e: Option<[f32; 4]> = None;
    for v in verts
        .iter()
        .filter(|v| finite(v) && (v.tl || v.clip[3] > 0.0))
    {
        let s = screen(setup, v);
        e = Some(match e {
            None => [s.x, s.x, s.y, s.y],
            Some(e) => [e[0].min(s.x), e[1].max(s.x), e[2].min(s.y), e[3].max(s.y)],
        });
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        triangles: Vec<[ScreenVertex; 3]>,
        lines: Vec<[ScreenVertex; 2]>,
        points: Vec<ScreenVertex>,
    }

    impl Rasterizer for Recorder {
        fn triangle(&mut self, v: &[ScreenVertex; 3]) {
            self.triangles.push(*v);
        }
        fn line(&mut self, v: &[ScreenVertex; 2]) {
            self.lines.push(*v);
        }
        fn point(&mut self, v: &ScreenVertex) {
            self.points.push(*v);
        }
    }

    fn vertex(x: f32, y: f32, z: f32) -> Vertex {
        Vertex {
            position: [x, y, z, 1.0],
            ..Vertex::default()
        }
    }

    /// A matrix that is 0 but for `set`: each a place and its number.
    fn sparse(set: &[(usize, f32)]) -> Matrix {
        let mut m = [0.0; 16];
        for &(at, n) in set {
            m[at] = n;
        }
        m
    }

    #[test]
    fn a_slot_takes_a_set_or_what_is_made_from_the_vertex() {
        // XYZ, a normal and one set of two coordinates.
        let layout = Layout::of(0x002 | 0x010 | 0x100).unwrap();
        let mut v = vertex(1.0, 2.0, 4.0);
        v.normal = [0.0, 0.0, -1.0];
        v.tex[0] = [0.25, 0.75, 0.0, 0.0];
        // The view moves the vertex 1 along x. The texture matrix
        // doubles what goes in and adds its fourth number, which is the
        // 1 after a position's three: a set of two has its 1 in the
        // third place, where this matrix adds nothing.
        let double = sparse(&[
            (0, 2.0),
            (5, 2.0),
            (10, 2.0),
            (12, 1.0),
            (13, 1.0),
            (14, 1.0),
        ]);
        let setup = Setup {
            view: sparse(&[(0, 1.0), (5, 1.0), (10, 1.0), (15, 1.0), (12, 1.0)]),
            coords: [
                Coord::of(0, 0, &double),
                Coord::of(0, 2, &double),
                // The camera-space position, and the normal.
                Coord::of(0x2_0000, 3, &double),
                Coord::of(0x1_0000, 0, &double),
                // A set the vertex doesn't have.
                Coord::of(1, 0, &double),
                Coord::set(5),
                Coord::set(6),
                Coord::set(7),
            ],
            ..Setup::default()
        };
        let p = process(&setup, &[v], &layout)[0];
        assert_eq!(p.tex[0], [0.25, 0.75, 0.0, 1.0]);
        assert_eq!(p.tex[1], [0.5, 1.5, 0.0, 1.0]);
        assert_eq!(p.tex[2], [5.0, 5.0, 9.0, 1.0]);
        assert_eq!(p.tex[3], [0.0, 0.0, -1.0, 1.0]);
        assert_eq!(p.tex[4], [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn a_projected_stage_gives_the_card_its_divisor_last() {
        let m = Setup::default().world;
        // D3DTTFF_COUNT3 | PROJECTED divides by the third, and
        // PROJECTED alone by the fourth.
        assert_eq!(
            Coord::of(0, 3 | 0x100, &m).sampled([2.0, 4.0, 8.0, 1.0]),
            [2.0, 4.0, 0.0, 8.0]
        );
        assert_eq!(
            Coord::of(0, 0x100, &m).sampled([2.0, 4.0, 8.0, 16.0]),
            [2.0, 4.0, 8.0, 16.0]
        );
        assert_eq!(
            Coord::of(0, 3, &m).sampled([2.0, 4.0, 8.0, 16.0]),
            [2.0, 4.0, 8.0, 1.0]
        );
    }

    #[test]
    fn process_vertices_puts_made_coordinates_in_the_set_the_stage_names() {
        let m = Setup::default().world;
        let mut stages: [Coord; 8] = std::array::from_fn(Coord::set);
        // Stage 0 takes set 1 as it is, stage 1 makes set 1 from the
        // position, and stage 2 would make it from the normal.
        stages[0] = Coord::of(1, 0, &m);
        stages[1] = Coord::of(1 | 0x2_0000, 2 | 0x100, &m);
        stages[2] = Coord::of(1 | 0x1_0000, 0, &m);
        let setup = Setup {
            coords: stages,
            ..Setup::default()
        }
        .for_sets();
        assert_eq!(setup.coords[0], Coord::set(0));
        assert_eq!(
            setup.coords[1],
            Coord {
                projected: None,
                ..stages[1]
            }
        );
        assert_eq!(setup.coords[2], Coord::set(2));
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn layouts_of_the_old_vertex_types() {
        let v = Layout::of(FVF_VERTEX).unwrap();
        assert_eq!((v.size, v.normal, v.textures), (32, Some(12), 1));
        let l = Layout::of(FVF_LVERTEX).unwrap();
        assert_eq!(
            (l.size, l.diffuse, l.specular, l.tex[0].0),
            (32, Some(16), Some(20), 24)
        );
        let t = Layout::of(FVF_TLVERTEX).unwrap();
        assert!(t.rhw);
        assert_eq!((t.size, t.diffuse, t.tex[0].0), (32, Some(16), 24));
        assert_eq!(Layout::of(0x6).unwrap().size, 16);
        assert_eq!(Layout::of(0), None);
        assert_eq!(Layout::of(0x002 | 0x900), None);
    }

    #[test]
    fn the_viewport_maps_the_unit_cube() {
        let setup = Setup {
            view_map: ViewMap::dx7(10, 5, 246, 130, -2.0, 4.0),
            ..Setup::default()
        };
        let out = process(
            &setup,
            &[vertex(1.0, 1.0, 1.0), vertex(0.0, 0.0, 0.0)],
            &Layout::of(FVF_XYZ).unwrap(),
        );
        let s = setup.view_map.screen(out[0].clip);
        assert!(near(s[0], 256.0) && near(s[1], 5.0) && near(s[2], 4.0) && near(s[3], 1.0));
        let s = setup.view_map.screen(out[1].clip);
        assert!(near(s[0], 133.0) && near(s[1], 70.0) && near(s[2], -2.0));
    }

    #[test]
    fn a_screen_position_gives_its_clip_coordinates_back() {
        let m = ViewMap::dx7(0, 0, 100, 100, 0.0, 1.0);
        for c in [[0.5, -0.25, 1.0, 2.0], [0.5, -0.25, -1.0, -2.0]] {
            let back = m.clip(m.screen(c)).unwrap();
            assert!((0..4).all(|i| near(back[i], c[i])), "{c:?} {back:?}");
        }
        for rhw in [0.0, f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            assert_eq!(m.clip([1.0, 2.0, 0.5, rhw]), None, "rhw {rhw}");
        }
    }

    #[test]
    fn a_triangle_through_the_near_plane_is_clipped() {
        let setup = Setup {
            view_map: ViewMap::dx7(0, 0, 100, 100, 0.0, 1.0),
            cull: CULL_NONE,
            ..Setup::default()
        };
        let layout = Layout::of(FVF_XYZ).unwrap();
        let verts = process(
            &setup,
            &[
                vertex(-0.5, -0.5, -0.5),
                vertex(0.5, -0.5, 0.5),
                vertex(0.0, 0.5, 0.5),
            ],
            &layout,
        );
        let prims = assemble(TRIANGLELIST, &[0, 1, 2]).unwrap();
        let mut r = Recorder::default();
        let drawn = draw(&setup, &verts, &prims, &mut r);
        assert_eq!(drawn.triangles, 1);
        // A quadrilateral: two triangles, none of their z below 0.
        assert_eq!(r.triangles.len(), 2);
        assert!(r.triangles.iter().flatten().all(|v| v.z >= -1e-6));
    }

    /// A perspective projection, near plane 1 and far plane 10, and a
    /// triangle with a corner behind the eye: its vertices through the
    /// pipeline, and the screen vertices ProcessVertices writes of them,
    /// the last one's from a negative w.
    fn behind_the_eye() -> (Setup, Vec<Processed>, Vec<Processed>) {
        let q = 10.0 / 9.0;
        let setup = Setup {
            view_map: ViewMap::dx7(0, 0, 100, 100, 0.0, 1.0),
            projection: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, q, 1.0, 0.0, 0.0, -q, 0.0,
            ],
            cull: CULL_NONE,
            ..Setup::default()
        };
        let input = [
            vertex(-1.0, -1.0, 2.0),
            vertex(1.0, -1.0, 2.0),
            vertex(0.0, 1.0, -2.0),
        ];
        let direct = process(&setup, &input, &Layout::of(FVF_XYZ).unwrap());
        let written: Vec<Vertex> = direct
            .iter()
            .map(|p| Vertex {
                position: setup.view_map.screen(p.clip),
                ..Vertex::default()
            })
            .collect();
        let tl = process(&setup, &written, &Layout::of(FVF_XYZRHW).unwrap());
        (setup, direct, tl)
    }

    fn in_front(v: &ScreenVertex) -> bool {
        v.z >= -1e-6 && v.rhw > 0.0
    }

    #[test]
    fn processed_vertices_are_clipped_at_their_clip_coordinates() {
        let (setup, direct, tl) = behind_the_eye();
        let kept: Vec<Processed> = tl
            .iter()
            .zip(&direct)
            .map(|(p, d)| at_clip_coordinates(&setup, p, d.clip, d.planes))
            .collect();
        let prims = assemble(TRIANGLELIST, &[0, 1, 2]).unwrap();
        let (mut a, mut b) = (Recorder::default(), Recorder::default());
        draw(&setup, &direct, &prims, &mut a);
        draw(&setup, &kept, &prims, &mut b);
        // Clipped at the near plane: the part in front of the eye, as
        // the untransformed vertices draw it.
        assert!(!a.triangles.is_empty());
        assert_eq!(a.triangles, b.triangles);
        assert!(b.triangles.iter().flatten().all(in_front));
    }

    /// The triangles of `b` are those of `a`, with their vertices close,
    /// and all in front of the eye.
    fn same_triangles(a: &Recorder, b: &Recorder) {
        assert!(!a.triangles.is_empty());
        assert_eq!(a.triangles.len(), b.triangles.len());
        let close = |a: f32, b: f32| (a - b).abs() <= 1e-4 * (1.0 + a.abs());
        for (va, vb) in a
            .triangles
            .iter()
            .flatten()
            .zip(b.triangles.iter().flatten())
        {
            assert!(close(va.x, vb.x) && close(va.y, vb.y), "{va:?} {vb:?}");
            assert!(close(va.z, vb.z) && close(va.rhw, vb.rhw), "{va:?} {vb:?}");
        }
        assert!(b.triangles.iter().flatten().all(in_front));
    }

    #[test]
    fn a_triangle_mixing_screen_and_clip_vertices_is_clipped_at_the_near_plane() {
        // The first corner drawn from its screen position alone, as one
        // the program wrote over: the triangle is still clipped at the
        // near plane, as the untransformed vertices draw it.
        let (setup, direct, tl) = behind_the_eye();
        let mut mixed = vec![tl[0]];
        for (p, d) in tl.iter().zip(&direct).skip(1) {
            mixed.push(at_clip_coordinates(&setup, p, d.clip, d.planes));
        }
        let prims = assemble(TRIANGLELIST, &[0, 1, 2]).unwrap();
        let (mut a, mut b) = (Recorder::default(), Recorder::default());
        draw(&setup, &direct, &prims, &mut a);
        draw(&setup, &mixed, &prims, &mut b);
        same_triangles(&a, &b);
    }

    #[test]
    fn a_corner_behind_the_eye_from_its_screen_position_is_clipped_at_the_near_plane() {
        // The corner behind the eye, drawn from its screen position alone
        // (its rhw is negative), beside two kept at clip coordinates. Its
        // clip coordinates are the ones the viewport maps it from, so the
        // triangle is clipped at the near plane as well.
        let (setup, direct, tl) = behind_the_eye();
        assert!(tl[2].rhw < 0.0);
        let mixed = [
            at_clip_coordinates(&setup, &tl[0], direct[0].clip, direct[0].planes),
            at_clip_coordinates(&setup, &tl[1], direct[1].clip, direct[1].planes),
            tl[2],
        ];
        let prims = assemble(TRIANGLELIST, &[0, 1, 2]).unwrap();
        let (mut a, mut b) = (Recorder::default(), Recorder::default());
        draw(&setup, &direct, &prims, &mut a);
        draw(&setup, &mixed, &prims, &mut b);
        same_triangles(&a, &b);
    }

    #[test]
    fn counterclockwise_triangles_are_culled() {
        let setup = Setup {
            view_map: ViewMap::dx7(0, 0, 100, 100, 0.0, 1.0),
            ..Setup::default()
        };
        let layout = Layout::of(FVF_XYZ).unwrap();
        // Clockwise on the screen: up-left, up-right, down.
        let verts = process(
            &setup,
            &[
                vertex(-0.5, 0.5, 0.5),
                vertex(0.5, 0.5, 0.5),
                vertex(0.0, -0.5, 0.5),
            ],
            &layout,
        );
        let mut r = Recorder::default();
        draw(
            &setup,
            &verts,
            &assemble(TRIANGLELIST, &[0, 1, 2]).unwrap(),
            &mut r,
        );
        assert_eq!(r.triangles.len(), 1);
        let mut r = Recorder::default();
        draw(
            &setup,
            &verts,
            &assemble(TRIANGLELIST, &[0, 2, 1]).unwrap(),
            &mut r,
        );
        assert!(r.triangles.is_empty());
    }

    #[test]
    fn a_directional_light_lights_what_faces_it() {
        let lighting = Lighting {
            lights: vec![Light {
                kind: LIGHT_DIRECTIONAL,
                diffuse: [1.0, 0.5, 0.25, 1.0],
                direction: [0.0, 0.0, 1.0],
                ..Light::default()
            }],
            material: Material {
                diffuse: [1.0, 1.0, 1.0, 0.5],
                ..Material::default()
            },
            ..Lighting::default()
        };
        let setup = Setup {
            lighting: Some(lighting),
            ..Setup::default()
        };
        let mut v = vertex(0.0, 0.0, 0.5);
        v.normal = [0.0, 0.0, -1.0];
        let mut away = v;
        away.normal = [0.0, 0.0, 1.0];
        let out = process(
            &setup,
            &[v, away],
            &Layout::of(FVF_XYZ | FVF_NORMAL).unwrap(),
        );
        assert_eq!(color(out[0].diffuse), 0x80FF_8040);
        assert_eq!(color(out[1].diffuse), 0x8000_0000);
    }

    #[test]
    fn linear_fog_goes_from_start_to_end() {
        assert_eq!(fog_factor(FOG_LINEAR, 10.0, 20.0, 0.0, 15.0), 0.5);
        assert_eq!(fog_factor(FOG_LINEAR, 10.0, 20.0, 0.0, 30.0), 0.0);
        assert_eq!(fog_factor(FOG_EXP, 0.0, 0.0, 0.0, 30.0), 1.0);
    }

    #[test]
    fn strips_keep_their_winding() {
        let prims = assemble(TRIANGLESTRIP, &[0, 1, 2, 3]).unwrap();
        assert_eq!(
            prims,
            vec![
                Prim::Triangle([0, 1, 2], 0, 7),
                Prim::Triangle([2, 1, 3], 1, 7)
            ]
        );
    }

    #[test]
    fn flat_shading_takes_the_colors_but_not_the_fog() {
        let setup = Setup {
            shade: SHADE_FLAT,
            cull: CULL_NONE,
            ..Setup::default()
        };
        // Transformed vertices, each its own colors and fog factor.
        let at = |x: f32, y: f32, diffuse: u32, specular: u32| Processed {
            clip: [x, y, 0.5, 1.0],
            rhw: 1.0,
            tl: true,
            diffuse: rgba(diffuse),
            specular: rgba(specular),
            ..Processed::default()
        };
        let verts = [
            at(0.0, 0.0, 0xFFFF_0000, 0x0010_2030),
            at(10.0, 0.0, 0xFF00_FF00, 0x8040_5060),
            at(0.0, 10.0, 0xFF00_00FF, 0xFF70_8090),
        ];
        let mut r = Recorder::default();
        // A fan's triangle takes the colors of its second vertex.
        draw(
            &setup,
            &verts,
            &assemble(TRIANGLEFAN, &[0, 1, 2]).unwrap(),
            &mut r,
        );
        draw(&setup, &verts, &[Prim::Line(0, 2, 0)], &mut r);
        let colors = |v: &[ScreenVertex]| -> Vec<(u32, u32)> {
            v.iter().map(|v| (v.diffuse, v.specular)).collect()
        };
        assert_eq!(
            colors(&r.triangles[0]),
            [
                (0xFF00_FF00, 0x0040_5060),
                (0xFF00_FF00, 0x8040_5060),
                (0xFF00_FF00, 0xFF40_5060),
            ]
        );
        assert_eq!(
            colors(&r.lines[0]),
            [(0xFFFF_0000, 0x0010_2030), (0xFFFF_0000, 0xFF10_2030)]
        );
    }

    #[test]
    fn clipping_in_place_gives_the_polygons_clipping_into_vectors_gave() {
        // The guard band's planes, a near and far plane, and two user
        // planes, against random triangles, most of them across some:
        // `clip_polygon` leaves the same vertices, to the bit, as
        // `clip_spilled`, the version that clips into vectors.
        let mut planes = Planes::new(Plane::User(0));
        for plane in [
            Plane::AboveW(2, 0.0),
            Plane::BelowW(2, 1.0),
            Plane::User(0),
            Plane::User(1),
            Plane::AboveW(0, -1.5),
            Plane::BelowW(0, 1.5),
            Plane::AboveW(1, -1.25),
            Plane::BelowW(1, 1.25),
        ] {
            planes.push(plane);
        }
        let mut seed = 0x2545_F491_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32 * 6.0 - 3.0
        };
        let mut cut = 0;
        let mut scratch = clip_scratch();
        for _ in 0..20_000 {
            let mut vertex = || Processed {
                clip: [next(), next(), next(), next().abs() + 0.1],
                planes: [next(), next(), 0.0, 0.0, 0.0, 0.0],
                diffuse: [next(), next(), next(), next()],
                tex: [[next(), next(), 0.0, 0.0]; 8],
                ..Processed::default()
            };
            let corners = [(vertex(), true), (vertex(), false), (vertex(), true)];
            let spilled = clip_spilled(corners.to_vec(), &planes);
            let poly = clip_polygon(&corners, &planes, &mut scratch).expect("room");
            assert_eq!(format!("{:?}", &poly[..]), format!("{spilled:?}"));
            if format!("{spilled:?}") != format!("{:?}", &corners[..]) {
                cut += 1;
            }
        }
        assert!(cut > 1000, "{cut} cut");
    }

    #[test]
    fn a_transformed_triangle_cut_at_the_guard_band_keeps_its_texture_in_perspective() {
        // A floor's near corner far right of the guard band, as Omikron
        // draws one when the camera is close: rhw 100 times the others'.
        // Every corner of what is left has the coordinates the card would
        // iterate there across the whole triangle: u*rhw and rhw are
        // linear on the screen.
        let corner = |x: f32, y: f32, rhw: f32, u: f32| {
            let mut p = Processed {
                clip: [x, y, 0.5, 1.0],
                rhw,
                tl: true,
                diffuse: [1.0; 4],
                ..Processed::default()
            };
            p.tex[0] = [u, 1.0 - u, 0.0, 0.0];
            p
        };
        let tri = [
            corner(0.0, 0.0, 0.01, 0.0),
            corner(20000.0, 100.0, 1.0, 50.0),
            corner(0.0, 200.0, 0.01, 1.0),
        ];
        let setup = Setup {
            cull: CULL_NONE,
            ..Setup::default()
        };
        let mut r = Recorder::default();
        draw(
            &setup,
            &tri,
            &assemble(TRIANGLELIST, &[0, 1, 2]).unwrap(),
            &mut r,
        );
        assert!(!r.triangles.is_empty());
        // Barycentric weights of a point in the uncut triangle.
        let [a, b, c] = tri.map(|p| (p.clip[0], p.clip[1]));
        let area = (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1);
        let weights = |x: f32, y: f32| {
            let wb = ((x - a.0) * (c.1 - a.1) - (c.0 - a.0) * (y - a.1)) / area;
            let wc = ((b.0 - a.0) * (y - a.1) - (x - a.0) * (b.1 - a.1)) / area;
            [1.0 - wb - wc, wb, wc]
        };
        let mut cut = 0;
        for v in r.triangles.iter().flatten() {
            assert!(v.x <= setup.guard_band[2] + 0.01, "{v:?}");
            let w = weights(v.x, v.y);
            let rhw: f32 = (0..3).map(|i| w[i] * tri[i].rhw).sum();
            assert!((v.rhw - rhw).abs() <= 1e-4 * rhw.max(1.0), "{v:?}: rhw {rhw}");
            for k in 0..2 {
                let uw: f32 = (0..3).map(|i| w[i] * tri[i].tex[0][k] * tri[i].rhw).sum();
                let want = uw / rhw;
                assert!((v.tex[0][k] - want).abs() < 1e-3, "{v:?}: {k} {want}");
            }
            cut += (v.x > 1.0) as usize;
        }
        assert!(cut > 0);
    }

    fn translation(x: f32, y: f32, z: f32) -> Matrix {
        let mut m = IDENTITY;
        m[12] = x;
        m[13] = y;
        m[14] = z;
        m
    }

    fn blended(count: usize, indexed: bool, matrices: Vec<Matrix>) -> Setup {
        Setup {
            blend: Some(Blend::Weights {
                count,
                indexed,
                matrices,
            }),
            ..Setup::default()
        }
    }

    #[test]
    fn xyzb_layouts_with_ubyte4_indices() {
        // Max Payne's: XYZB2, LASTBETA_UBYTE4, NORMAL, TEX1.
        let l = Layout::of_d3d8(0x1118).unwrap();
        assert_eq!((l.weights, l.indices, l.normal), (1, Some(16), Some(20)));
        assert_eq!((l.tex[0].0, l.size), (32, 40));
        // XYZB1 with LASTBETA_UBYTE4: no weights, only the indices.
        let l = Layout::of_d3d8(0x1116).unwrap();
        assert_eq!(
            (l.weights, l.indices, l.normal, l.size),
            (0, Some(12), Some(16), 36)
        );
        // XYZB3 without: three weights.
        let l = Layout::of_d3d8(0x00A).unwrap();
        assert_eq!((l.weights, l.indices, l.size), (3, None, 24));
        // Direct3D 7 has no LASTBETA_UBYTE4.
        assert_eq!(Layout::of(0x1118), None);
        assert_eq!(Layout::of_d3d8(0x2118), None);

        let mut bytes = Vec::new();
        for f in [1.0f32, 2.0, 3.0, 0.25] {
            bytes.extend(f.to_le_bytes());
        }
        bytes.extend([7, 200, 0, 0]);
        bytes.extend([0; 20]);
        let v = Layout::of_d3d8(0x1118).unwrap().read(&bytes);
        assert_eq!(v.position, [1.0, 2.0, 3.0, 1.0]);
        assert_eq!((v.weights[0], v.indices), (0.25, [7, 200, 0, 0]));
    }

    #[test]
    fn blending_weighs_the_world_matrices() {
        // Matrix 0 moves x by 2 and matrix 1 by 4. With one weight of
        // 0.25 the second gets 0.75: x = 0.25 * 2 + 0.75 * 4 = 3.5.
        let setup = blended(
            1,
            false,
            vec![translation(2.0, 0.0, 0.0), translation(4.0, 0.0, 0.0)],
        );
        let mut v = vertex(0.0, 1.0, 0.5);
        v.weights[0] = 0.25;
        let out = process(&setup, &[v], &Layout::of_d3d8(0x6).unwrap());
        assert_eq!(out[0].clip, [3.5, 1.0, 0.5, 1.0]);
        // Three weights of 0.25, 0.25 and 0.5 leave none for matrix 3.
        let setup = blended(
            3,
            false,
            (0..4).map(|k| translation(0.0, k as f32, 0.0)).collect(),
        );
        v.weights = [0.25, 0.25, 0.5, 9.0];
        let out = process(&setup, &[v], &Layout::of_d3d8(0xC).unwrap());
        // y = 1 + 0.25 * 0 + 0.25 * 1 + 0.5 * 2.
        assert_eq!(out[0].clip[1], 2.25);
    }

    #[test]
    fn indexed_blending_takes_the_matrices_the_indices_name() {
        let mut matrices = vec![IDENTITY; 256];
        matrices[7] = translation(0.0, 1.0, 0.0);
        matrices[200] = translation(0.0, 3.0, 0.0);
        // The first weight, 0.75, is matrix 200's (the lowest byte's
        // index); the last, 0.25, matrix 7's: y = 0.75 * 3 + 0.25 * 1.
        let setup = blended(1, true, matrices.clone());
        let mut v = vertex(0.0, 0.0, 0.5);
        v.weights[0] = 0.75;
        v.indices = [200, 7, 0, 0];
        let out = process(&setup, &[v], &Layout::of_d3d8(0x1008).unwrap());
        assert_eq!(out[0].clip, [0.0, 2.5, 0.5, 1.0]);
        // D3DVBF_0WEIGHTS: the one matrix the first index names.
        let setup = blended(0, true, matrices);
        v.indices = [7, 200, 0, 0];
        let out = process(&setup, &[v], &Layout::of_d3d8(0x1006).unwrap());
        assert_eq!(out[0].clip, [0.0, 1.0, 0.5, 1.0]);
    }

    #[test]
    fn blended_normals_go_through_each_matrix_and_light_the_vertex() {
        // A light from +x. Matrix 0 turns x to y (90 degrees about z),
        // matrix 1 is the identity: half of each makes the normal (1, 0,
        // 0) into (0.5, 0.5, 0), which lights 0.5 of the diffuse color
        // as it is and 0.707 of it normalized.
        let light = Light {
            kind: LIGHT_DIRECTIONAL,
            diffuse: [1.0, 1.0, 1.0, 1.0],
            direction: [-1.0, 0.0, 0.0],
            ..Light::default()
        };
        let lighting = Lighting {
            lights: vec![light],
            material: Material {
                diffuse: [1.0, 1.0, 1.0, 1.0],
                ..Material::default()
            },
            ..Lighting::default()
        };
        let turn = [
            0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let mut setup = blended(1, false, vec![turn, IDENTITY]);
        let mut v = vertex(0.0, 0.0, 0.5);
        v.normal = [1.0, 0.0, 0.0];
        v.weights[0] = 0.5;
        let layout = Layout::of_d3d8(0x6 | FVF_NORMAL).unwrap();
        setup.lighting = Some(lighting.clone());
        let out = process(&setup, &[v], &layout);
        assert!(near(out[0].diffuse[0], 0.5));
        setup.normalize = true;
        let out = process(&setup, &[v], &layout);
        assert!(near(out[0].diffuse[0], 0.5f32.sqrt()));

        // A normal goes through the inverse transpose: x scaled by 2
        // scales a normal's x by 0.5, which lights 0.5 of the color.
        let mut stretch = IDENTITY;
        stretch[0] = 2.0;
        let mut setup = blended(1, false, vec![stretch, IDENTITY]);
        setup.lighting = Some(lighting);
        v.weights[0] = 1.0;
        let out = process(&setup, &[v], &layout);
        assert!(near(out[0].diffuse[0], 0.5));
    }

    #[test]
    fn tweening_moves_the_factor_of_the_way_to_the_second_position() {
        let setup = Setup {
            blend: Some(Blend::Tween(0.25)),
            ..Setup::default()
        };
        let mut v = vertex(0.0, 1.0, 0.5);
        v.position2 = Some([2.0, 1.0, 0.5]);
        let out = process(&setup, &[v], &Layout::default());
        assert_eq!(out[0].clip, [0.5, 1.0, 0.5, 1.0]);
        // A vertex without a second position stays where it is.
        v.position2 = None;
        let out = process(&setup, &[v], &Layout::default());
        assert_eq!(out[0].clip, [0.0, 1.0, 0.5, 1.0]);
    }
}

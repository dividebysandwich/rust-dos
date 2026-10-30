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
}

impl Layout {
    /// The layout of the FVF code `fvf`, if it is one.
    pub fn of(fvf: u32) -> Option<Layout> {
        if fvf & (FVF_RESERVED0 | FVF_RESERVED2) != 0 {
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
    pub normalize: bool,
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
    /// bottom. The card's is `hal::VERTEX_BAND`, a pixel inside the
    /// guard band the device reports.
    pub guard_band: [f32; 4],
    pub cull: u32,
    pub fill: u32,
    pub shade: u32,
    /// The texture coordinates each of the card's two stages takes.
    pub tex_index: [usize; 2],
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
            guard_band: super::hal::VERTEX_BAND,
            cull: CULL_CCW,
            fill: 3,
            shade: 2,
            tex_index: [0, 1],
        }
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
        for i in 0..8 {
            p.tex[i] = l4(self.tex[i], o.tex[i]);
        }
        p
    }
}

/// A vertex on the screen, as the card's triangle setup takes it: x, y,
/// z (0 to 1), rhw, the colors (the specular's alpha the fog factor),
/// and the coordinates of the textures of the card's two stages.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScreenVertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub rhw: f32,
    pub diffuse: u32,
    pub specular: u32,
    pub tex: [[f32; 2]; 2],
}

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
}

/// Run `input`, of `layout`, through transform, lighting and fog.
pub fn process(setup: &Setup, input: &[Vertex], layout: &Layout) -> Vec<Processed> {
    if layout.rhw {
        return input.iter().map(|v| transformed(v, layout)).collect();
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
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    input
        .iter()
        .map(|v| {
            let p = [v.position[0], v.position[1], v.position[2], 1.0];
            let clip = transform(p, &all);
            let mut out = Processed {
                clip,
                tex: v.tex,
                ..Processed::default()
            };
            if !setup.planes.is_empty() {
                let w = transform(p, &setup.world);
                for (i, plane) in setup.planes.iter().take(6).enumerate() {
                    out.planes[i] =
                        plane[0] * w[0] + plane[1] * w[1] + plane[2] * w[2] + plane[3] * w[3];
                }
            }
            let eye = point([p[0], p[1], p[2]], &world_view);
            match &setup.lighting {
                Some(l) => {
                    let n = times(v.normal, &normals);
                    let n = if l.normalize { normalize(n) } else { n };
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

/// A transformed vertex, as it is.
fn transformed(v: &Vertex, _layout: &Layout) -> Processed {
    Processed {
        clip: [v.position[0], v.position[1], v.position[2], 1.0],
        rhw: v.position[3],
        tl: true,
        diffuse: rgba(v.diffuse.unwrap_or(0xFFFF_FFFF)),
        specular: rgba(v.specular.unwrap_or(0xFF00_0000)),
        tex: v.tex,
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
            let axis = normalize(vector(light.direction, &IDENTITY));
            let rho = -dot(dir, axis);
            let (inner, outer) = ((light.theta / 2.0).cos(), (light.phi / 2.0).cos());
            if rho > inner {
                1.0
            } else if rho <= outer {
                0.0
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

/// A plane of homogeneous clipping: how far inside a vertex is.
type Plane = Box<dyn Fn(&Processed) -> f32>;

/// The planes primitives are clipped at: the near and far planes and
/// the user's when the setup clips, and the guard band always. A
/// transformed vertex's near and far planes are z 0 and 1, as Wine's
/// depth_clamp_test has them on Windows.
fn clip_planes(setup: &Setup, tl: bool) -> Vec<Plane> {
    let mut planes: Vec<Plane> = Vec::new();
    let band = setup.guard_band;
    if tl {
        if setup.clip {
            planes.push(Box::new(|p: &Processed| p.clip[2]));
            planes.push(Box::new(|p: &Processed| 1.0 - p.clip[2]));
        }
        planes.push(Box::new(move |p: &Processed| p.clip[0] - band[0]));
        planes.push(Box::new(move |p: &Processed| band[2] - p.clip[0]));
        planes.push(Box::new(move |p: &Processed| p.clip[1] - band[1]));
        planes.push(Box::new(move |p: &Processed| band[3] - p.clip[1]));
        return planes;
    }
    let b = setup.view_map.bounds;
    if setup.clip {
        planes.push(Box::new(move |p: &Processed| p.clip[2] - b[4] * p.clip[3]));
        planes.push(Box::new(move |p: &Processed| b[5] * p.clip[3] - p.clip[2]));
        for i in 0..setup.planes.len().min(6) {
            planes.push(Box::new(move |p: &Processed| p.planes[i]));
        }
    } else {
        // Nothing behind the eye can be divided by its w.
        planes.push(Box::new(|p: &Processed| p.clip[3] - 1e-6));
    }
    let g = setup.view_map.guard(band);
    planes.push(Box::new(move |p: &Processed| p.clip[0] - g[0] * p.clip[3]));
    planes.push(Box::new(move |p: &Processed| g[1] * p.clip[3] - p.clip[0]));
    planes.push(Box::new(move |p: &Processed| p.clip[1] - g[2] * p.clip[3]));
    planes.push(Box::new(move |p: &Processed| g[3] * p.clip[3] - p.clip[1]));
    planes
}

/// Clip a polygon, each vertex with whether the edge from it to the
/// next is to be drawn in wireframe, at `planes`.
fn clip_polygon(mut poly: Vec<(Processed, bool)>, planes: &[Plane]) -> Vec<(Processed, bool)> {
    for plane in planes {
        if poly.is_empty() {
            break;
        }
        let d: Vec<f32> = poly.iter().map(|(v, _)| plane(v)).collect();
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
    let tex = |stage: usize| {
        let t = p.tex[setup.tex_index[stage] & 7];
        [t[0], t[1]]
    };
    ScreenVertex {
        x,
        y,
        z,
        rhw,
        diffuse: color(p.diffuse),
        specular: color(p.specular),
        tex: [tex(0), tex(1)],
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
    let flat = setup.shade == SHADE_FLAT;
    for prim in prims {
        match *prim {
            Prim::Point(i) => {
                let Some(v) = verts.get(i as usize) else {
                    continue;
                };
                let planes = if v.tl { &tl_planes } else { &planes };
                if !finite(v) || planes.iter().any(|p| p(v) < 0.0) {
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
                triangle(setup, tri, edges, &planes, &tl_planes, raster, &mut drawn);
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
    let tl = a.tl || b.tl;
    let (a, b) = if tl {
        (as_tl(setup, a), as_tl(setup, b))
    } else {
        (*a, *b)
    };
    let planes = if tl { tl_planes } else { planes };
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for plane in planes {
        let (da, db) = (plane(&a), plane(&b));
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

/// A vertex as a transformed one, for a primitive that mixes them.
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

fn triangle(
    setup: &Setup,
    tri: [Processed; 3],
    edges: u8,
    planes: &[Plane],
    tl_planes: &[Plane],
    raster: &mut dyn Rasterizer,
    drawn: &mut Drawn,
) {
    if !tri.iter().all(finite) {
        return;
    }
    let tl = tri.iter().any(|v| v.tl);
    let tri = if tl {
        tri.map(|v| as_tl(setup, &v))
    } else {
        tri
    };
    // All outside one plane of the view volume: nothing to draw. With
    // no clipping, depth past the near and far planes is clamped, and
    // only the sides count.
    let sides = CLIP_LEFT | CLIP_RIGHT | CLIP_TOP | CLIP_BOTTOM;
    let mask = if setup.clip { u32::MAX } else { sides };
    if !tl && tri[0].code & tri[1].code & tri[2].code & mask != 0 {
        return;
    }
    let poly = vec![
        (tri[0], edges & 1 != 0),
        (tri[1], edges & 2 != 0),
        (tri[2], edges & 4 != 0),
    ];
    let poly = clip_polygon(poly, if tl { tl_planes } else { planes });
    if poly.len() < 3 {
        return;
    }
    let screens: Vec<ScreenVertex> = poly.iter().map(|(v, _)| screen(setup, v)).collect();
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
            for v in &screens {
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

    #[test]
    fn processed_vertices_are_clipped_at_their_clip_coordinates() {
        // A perspective projection, near plane 1 and far plane 10, and a
        // triangle with a corner behind the eye.
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
        // What ProcessVertices writes: screen vertices, the last one's
        // from a negative w.
        let written: Vec<Vertex> = direct
            .iter()
            .map(|p| Vertex {
                position: setup.view_map.screen(p.clip),
                ..Vertex::default()
            })
            .collect();
        let tl = process(&setup, &written, &Layout::of(FVF_XYZRHW).unwrap());
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
        let in_front = |v: &ScreenVertex| v.z >= -1e-6 && v.rhw > 0.0;
        assert!(b.triangles.iter().flatten().all(in_front));
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
}

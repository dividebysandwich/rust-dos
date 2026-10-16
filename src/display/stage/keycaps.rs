//! Rust-DOS's own keyboard for the desk (`PropKind::Keyboard`): a 101-key
//! PC keyboard of the late eighties, in a beige case with light and dark
//! grey keys and the three lock lights over the keypad. Each key is a cap
//! of its own, to go down when it is pressed; their legends are drawn
//! with the VGA's font into a picture of their own.
//!
//! Its frame is the props': the middle of its underside at the origin, its
//! rows along +X, the function keys away from the viewer (-Z).

use super::scene::{
    Image, Led, Material, Mesh, PartRole, Shading, TextureRef, Vertex, cuboid, srgb,
};
use glam::{Vec2, Vec3};

/// A key's width, and the rows' pitch.
pub const U: f32 = 0.019;
/// How far a key goes down when it is pressed.
pub const TRAVEL: f32 = 0.0035;
/// The case's height below the keys, and of its rim round them.
const PLATE: f32 = 0.012;
const RIM: f32 = 0.018;
/// A cap's height from the plate, how much narrower its top is to each
/// side, and the gap between two caps.
const CAP: f32 = 0.0115;
const INSET: f32 = 0.0022;
const GAP: f32 = 0.0012;
/// The case's margins round the keys: at the sides and the front, and at
/// the back.
const MARGIN: f32 = 0.012;
const BACK: f32 = 0.02;
/// The keys' field, in keys: across, and deep.
const FIELD: Vec2 = Vec2::new(22.5, 6.5);
/// Where each row's top edge is, in keys from the back.
const ROWS: [f32; 6] = [0.0, 1.5, 2.5, 3.5, 4.5, 5.5];
/// The legends' pixels to a metre of a cap's top.
const PIXELS: f32 = 7200.0;
/// The picture's size.
const ATLAS: usize = 2048;
const ATLAS_HEIGHT: usize = 1024;

/// The colours: the case, the plate between the keys, the light and the
/// dark keys, the legends, and the lock lights'.
const CASE: u32 = 0xb4ad9a;
const PLATE_COLOR: u32 = 0x2e2d2a;
const LIGHT: u32 = 0xc9c4b5;
const DARK: u32 = 0x86837a;
const INK: u32 = 0x161616;
const LAMP: [f32; 3] = [0.15, 1.0, 0.2];

/// A key of it: what it sends (its scan code, and whether E0 comes
/// first), where its cap's middle is (x, z) and half its size there, how
/// high its top is, its row, and whether it is a shift key (Shift, Ctrl,
/// Alt), which clicking keeps down for the next key.
#[derive(Clone, Debug, PartialEq)]
pub struct Key {
    pub scan: Option<(u8, bool)>,
    pub center: Vec2,
    pub half: Vec2,
    pub top: f32,
    pub row: usize,
    pub shift: bool,
    pub legend: &'static str,
}

impl Key {
    /// Whether `p` (in the keyboard's frame) is over its cap, `slack`
    /// beyond its edges.
    pub fn over(&self, p: Vec3, slack: f32) -> bool {
        let d = (Vec2::new(p.x, p.z) - self.center).abs() - self.half;
        d.x <= slack && d.y <= slack
    }
}

/// A key as laid out: its legend's lines, what it sends, where its left
/// edge is (in keys), its row, its width and height (in keys), and
/// whether it is dark.
struct Spec {
    legend: &'static str,
    scan: Option<(u8, bool)>,
    x: f32,
    row: usize,
    size: Vec2,
    dark: bool,
}

/// The US layout of a 101-key keyboard.
fn layout() -> Vec<Spec> {
    let mut keys = Vec::new();
    let mut put = |row: usize,
                   x: f32,
                   w: f32,
                   h: f32,
                   legend: &'static str,
                   scan: Option<(u8, bool)>,
                   dark: bool| {
        keys.push(Spec {
            legend,
            scan,
            x,
            row,
            size: Vec2::new(w, h),
            dark,
        });
    };
    let plain = |scan: u8| Some((scan, false));
    let grey = |scan: u8| Some((scan, true));
    // The function keys, and Print Screen, Scroll Lock and Pause (which
    // sends nothing).
    put(0, 0.0, 1.0, 1.0, "Esc", plain(0x01), true);
    for i in 0..12 {
        let x = 2.0 + i as f32 + (i / 4) as f32 * 0.5;
        let legend = [
            "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
        ][i];
        let scan = if i < 10 {
            0x3B + i as u8
        } else {
            0x57 + (i - 10) as u8
        };
        put(0, x, 1.0, 1.0, legend, plain(scan), false);
    }
    put(0, 15.25, 1.0, 1.0, "Print\nScrn", grey(0x37), true);
    put(0, 16.25, 1.0, 1.0, "Scroll\nLock", plain(0x46), true);
    put(0, 17.25, 1.0, 1.0, "Pause", None, true);
    // The rows of the typewriter keys.
    let numbers = [
        "~\n`", "!\n1", "@\n2", "#\n3", "$\n4", "%\n5", "^\n6", "&\n7", "*\n8", "(\n9", ")\n0",
        "_\n-", "+\n=",
    ];
    let scans = [
        0x29, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D,
    ];
    for (i, (legend, scan)) in numbers.into_iter().zip(scans).enumerate() {
        put(1, i as f32, 1.0, 1.0, legend, plain(scan), false);
    }
    put(1, 13.0, 2.0, 1.0, "\u{1b} Backspace", plain(0x0E), true);
    put(2, 0.0, 1.5, 1.0, "Tab", plain(0x0F), true);
    let top = [
        "Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P", "{\n[", "}\n]",
    ];
    for (i, legend) in top.into_iter().enumerate() {
        put(
            2,
            1.5 + i as f32,
            1.0,
            1.0,
            legend,
            plain(0x10 + i as u8),
            false,
        );
    }
    put(2, 13.5, 1.5, 1.0, "|\n\\", plain(0x2B), false);
    put(3, 0.0, 1.75, 1.0, "Caps Lock", plain(0x3A), true);
    let home = ["A", "S", "D", "F", "G", "H", "J", "K", "L", ":\n;", "\"\n'"];
    for (i, legend) in home.into_iter().enumerate() {
        put(
            3,
            1.75 + i as f32,
            1.0,
            1.0,
            legend,
            plain(0x1E + i as u8),
            false,
        );
    }
    put(3, 12.75, 2.25, 1.0, "Enter", plain(0x1C), true);
    put(4, 0.0, 2.25, 1.0, "Shift", plain(0x2A), true);
    let bottom = ["Z", "X", "C", "V", "B", "N", "M", "<\n,", ">\n.", "?\n/"];
    for (i, legend) in bottom.into_iter().enumerate() {
        put(
            4,
            2.25 + i as f32,
            1.0,
            1.0,
            legend,
            plain(0x2C + i as u8),
            false,
        );
    }
    put(4, 12.25, 2.75, 1.0, "Shift", plain(0x36), true);
    put(5, 0.0, 1.5, 1.0, "Ctrl", plain(0x1D), true);
    put(5, 2.5, 1.5, 1.0, "Alt", plain(0x38), true);
    put(5, 4.0, 7.0, 1.0, "", plain(0x39), false);
    put(5, 11.0, 1.5, 1.0, "Alt", grey(0x38), true);
    put(5, 13.5, 1.5, 1.0, "Ctrl", grey(0x1D), true);
    // The editing keys and the arrows.
    for (i, (legend, scan)) in [("Insert", 0x52), ("Home", 0x47), ("Page\nUp", 0x49)]
        .into_iter()
        .enumerate()
    {
        put(1, 15.25 + i as f32, 1.0, 1.0, legend, grey(scan), true);
    }
    for (i, (legend, scan)) in [("Delete", 0x53), ("End", 0x4F), ("Page\nDown", 0x51)]
        .into_iter()
        .enumerate()
    {
        put(2, 15.25 + i as f32, 1.0, 1.0, legend, grey(scan), true);
    }
    put(4, 16.25, 1.0, 1.0, "\u{18}", grey(0x48), true);
    for (i, (legend, scan)) in [("\u{1b}", 0x4B), ("\u{19}", 0x50), ("\u{1a}", 0x4D)]
        .into_iter()
        .enumerate()
    {
        put(5, 15.25 + i as f32, 1.0, 1.0, legend, grey(scan), true);
    }
    // The keypad.
    put(1, 18.5, 1.0, 1.0, "Num\nLock", plain(0x45), true);
    put(1, 19.5, 1.0, 1.0, "/", grey(0x35), true);
    put(1, 20.5, 1.0, 1.0, "*", plain(0x37), true);
    put(1, 21.5, 1.0, 1.0, "-", plain(0x4A), true);
    let pad = [
        (2, 0, "7\nHome", 0x47),
        (2, 1, "8\n\u{18}", 0x48),
        (2, 2, "9\nPgUp", 0x49),
        (3, 0, "4\n\u{1b}", 0x4B),
        (3, 1, "5", 0x4C),
        (3, 2, "6\n\u{1a}", 0x4D),
        (4, 0, "1\nEnd", 0x4F),
        (4, 1, "2\n\u{19}", 0x50),
        (4, 2, "3\nPgDn", 0x51),
    ];
    for (row, column, legend, scan) in pad {
        put(
            row,
            18.5 + column as f32,
            1.0,
            1.0,
            legend,
            plain(scan),
            false,
        );
    }
    put(2, 21.5, 1.0, 2.0, "+", plain(0x4E), true);
    put(4, 21.5, 1.0, 2.0, "Enter", grey(0x1C), true);
    put(5, 18.5, 2.0, 1.0, "0\nIns", plain(0x52), false);
    put(5, 20.5, 1.0, 1.0, ".\nDel", plain(0x53), false);
    keys
}

/// The keyboard's model: its materials (their pictures the one returned,
/// image 0), its parts in its frame and its keys. The parts are the case
/// (`Body`), each row's caps together (`KeyRow`), drawn while none of the
/// row's keys is down, and each cap on its own (`Key`).
pub struct Model {
    pub materials: Vec<Material>,
    pub image: Image,
    pub parts: Vec<(PartRole, Mesh)>,
    pub keys: Vec<Key>,
}

/// The model's materials, by their index in `Model::materials`.
const M_CASE: usize = 0;
const M_PLATE: usize = 1;
const M_CAPS: usize = 2;
const M_LIGHTS: [usize; 3] = [3, 4, 5];

pub fn model() -> Model {
    let specs = layout();
    let size = Vec2::new(FIELD.x * U + 2.0 * MARGIN, FIELD.y * U + MARGIN + BACK);
    // The field's back left corner.
    let origin = Vec2::new(-size.x / 2.0 + MARGIN, -size.y / 2.0 + BACK);
    let mut atlas = Atlas::new();
    let swatches = [LIGHT, DARK, CASE].map(|color| atlas.swatch(color));
    let caps = Material {
        base_texture: Some(TextureRef {
            image: 0,
            repeat: false,
        }),
        ..Material::plain([1.0; 3], Shading::Lit)
    };
    // (Off, the lights are dark, not green.)
    let light = |led| Material {
        base_color: [0.012, 0.03, 0.014, 1.0],
        ..Material::led(led, LAMP)
    };
    let lights = [Led::NumLock, Led::CapsLock, Led::ScrollLock].map(light);
    let mut materials = vec![
        Material::plain(srgb(CASE), Shading::Lit),
        Material::plain(srgb(PLATE_COLOR), Shading::Lit),
        caps,
    ];
    materials.extend(lights);
    let mut parts = Vec::new();
    // The case: its bottom up to the plate, the plate under the keys, and
    // the rim round them.
    parts.push((
        PartRole::Body,
        cuboid(
            Vec3::new(0.0, PLATE / 2.0, 0.0),
            Vec3::new(size.x, PLATE, size.y),
            M_CASE,
        ),
    ));
    let field = FIELD * U;
    let middle = origin + field / 2.0;
    let plate = Vec3::new(middle.x, PLATE + 0.0003, middle.y);
    parts.push((
        PartRole::Body,
        cuboid(plate, Vec3::new(field.x, 0.0006, field.y), M_PLATE),
    ));
    let rim = RIM - PLATE;
    let mut wall = |center: Vec2, extent: Vec2| {
        let at = Vec3::new(center.x, PLATE + rim / 2.0, center.y);
        parts.push((
            PartRole::Body,
            cuboid(at, Vec3::new(extent.x, rim, extent.y), M_CASE),
        ));
    };
    let (left, right) = (-size.x / 2.0, size.x / 2.0);
    let (back, front) = (-size.y / 2.0, size.y / 2.0);
    wall(
        Vec2::new(0.0, (back + origin.y) / 2.0),
        Vec2::new(size.x, BACK),
    );
    wall(
        Vec2::new(0.0, (front + origin.y + field.y) / 2.0),
        Vec2::new(size.x, MARGIN),
    );
    wall(
        Vec2::new((left + origin.x) / 2.0, middle.y),
        Vec2::new(MARGIN, field.y),
    );
    wall(
        Vec2::new((right + origin.x + field.x) / 2.0, middle.y),
        Vec2::new(MARGIN, field.y),
    );
    // Over the keypad, beside the function keys: the lock lights, each
    // with its name below it.
    let panel = Vec2::new(origin.x + 18.5 * U + 2.0 * U, origin.y + 0.5 * U);
    wall(panel, Vec2::new(4.0 * U, U));
    for (i, name) in ["Num\nLock", "Caps\nLock", "Scroll\nLock"]
        .into_iter()
        .enumerate()
    {
        let x = origin.x + (18.5 + 0.7 + i as f32 * 1.3) * U;
        let lamp = Vec3::new(x, RIM + 0.0005, panel.y - 0.25 * U);
        parts.push((
            PartRole::Body,
            cuboid(lamp, Vec3::new(0.005, 0.001, 0.003), M_LIGHTS[i]),
        ));
        let label = Vec2::new(1.25 * U, 0.5 * U);
        let cell = atlas.cell(label, name, CASE);
        let corner = Vec3::new(x - label.x / 2.0, RIM + 0.0002, panel.y + 0.02 * U);
        parts.push((PartRole::Body, decal(corner, label, cell, M_CAPS)));
    }
    let mut keys = Vec::new();
    let mut rows: Vec<Mesh> = (0..ROWS.len())
        .map(|_| Mesh {
            vertices: Vec::new(),
            indices: Vec::new(),
            material: M_CAPS,
        })
        .collect();
    let mut singles = Vec::new();
    for spec in &specs {
        let low = origin + Vec2::new(spec.x, ROWS[spec.row]) * U;
        let foot = spec.size * U - Vec2::splat(GAP);
        let center = low + spec.size * U / 2.0;
        let top = foot - Vec2::splat(2.0 * INSET);
        let color = if spec.dark { DARK } else { LIGHT };
        let cell = atlas.cell(top, spec.legend, color);
        let side = swatches[spec.dark as usize];
        let mesh = keycap(center, foot / 2.0, top / 2.0, cell, side, M_CAPS);
        append(&mut rows[spec.row], &mesh);
        singles.push((PartRole::Key(keys.len()), mesh));
        let shift = matches!(spec.scan, Some((0x2A | 0x36 | 0x1D | 0x38, _)));
        let half = foot / 2.0;
        keys.push(Key {
            scan: spec.scan,
            center,
            half,
            top: PLATE + CAP,
            row: spec.row,
            shift,
            legend: spec.legend,
        });
    }
    parts.extend(
        rows.into_iter()
            .enumerate()
            .map(|(row, mesh)| (PartRole::KeyRow(row), mesh)),
    );
    parts.extend(singles);
    Model {
        materials,
        image: atlas.finish(),
        parts,
        keys,
    }
}

/// A key's cap: from its foot round `center` (`foot` half its size) on the
/// plate up to its top (`top` half its size), the top showing the
/// picture's `cell` and the sides its `side` (UV corners: top left and
/// bottom right).
fn keycap(
    center: Vec2,
    foot: Vec2,
    top: Vec2,
    cell: [Vec2; 2],
    side: Vec2,
    material: usize,
) -> Mesh {
    let (low, high) = (PLATE, PLATE + CAP);
    let at = |half: Vec2, sx: f32, sz: f32, y: f32| {
        Vec3::new(center.x + half.x * sx, y, center.y + half.y * sz)
    };
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
        material,
    };
    // The top, its back edge the legend's top.
    let [a, b] = cell;
    let corners = [
        (-1.0, -1.0, Vec2::new(a.x, a.y)),
        (-1.0, 1.0, Vec2::new(a.x, b.y)),
        (1.0, 1.0, b),
        (1.0, -1.0, Vec2::new(b.x, a.y)),
    ];
    let quad = corners.map(|(sx, sz, uv)| (at(top, sx, sz, high), uv));
    push_quad(&mut mesh, quad, Vec3::Y);
    // The four sides, sloping in.
    let ring = [(-1.0, -1.0), (-1.0, 1.0), (1.0, 1.0), (1.0, -1.0)];
    let middle = Vec3::new(center.x, (low + high) / 2.0, center.y);
    for i in 0..4 {
        let ((x0, z0), (x1, z1)) = (ring[i], ring[(i + 1) % 4]);
        let corners = [
            at(foot, x0, z0, low),
            at(foot, x1, z1, low),
            at(top, x1, z1, high),
            at(top, x0, z0, high),
        ];
        let outward = (corners[0] + corners[1] + corners[2] + corners[3]) / 4.0 - middle;
        let normal = (corners[1] - corners[0])
            .cross(corners[3] - corners[0])
            .normalize_or_zero();
        let normal = if normal.dot(outward) < 0.0 {
            -normal
        } else {
            normal
        };
        push_quad(&mut mesh, corners.map(|p| (p, side)), normal);
    }
    mesh
}

/// A flat label lying face up, `size` across and deep from its back left
/// `corner`, showing the picture's `cell`.
fn decal(corner: Vec3, size: Vec2, [a, b]: [Vec2; 2], material: usize) -> Mesh {
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
        material,
    };
    let at = |x: f32, z: f32| corner + Vec3::new(x * size.x, 0.0, z * size.y);
    let quad = [
        (at(0.0, 0.0), Vec2::new(a.x, a.y)),
        (at(0.0, 1.0), Vec2::new(a.x, b.y)),
        (at(1.0, 1.0), b),
        (at(1.0, 0.0), Vec2::new(b.x, a.y)),
    ];
    push_quad(&mut mesh, quad, Vec3::Y);
    mesh
}

/// A quad facing `normal`, its corners in either order round it.
fn push_quad(mesh: &mut Mesh, corners: [(Vec3, Vec2); 4], normal: Vec3) {
    let base = mesh.vertices.len() as u32;
    for (p, uv) in corners {
        mesh.vertices.push(Vertex {
            position: p.to_array(),
            normal: normal.to_array(),
            uv: uv.to_array(),
        });
    }
    let (p0, p1, p2) = (corners[0].0, corners[1].0, corners[2].0);
    let facing = (p1 - p0).cross(p2 - p0).dot(normal) >= 0.0;
    mesh.indices.extend(
        if facing {
            [0, 1, 2, 0, 2, 3]
        } else {
            [0, 2, 1, 0, 3, 2]
        }
        .map(|i| base + i),
    );
}

/// `from`'s triangles added to `to`'s.
fn append(to: &mut Mesh, from: &Mesh) {
    let base = to.vertices.len() as u32;
    to.vertices.extend_from_slice(&from.vertices);
    to.indices.extend(from.indices.iter().map(|i| base + i));
}

/// The legends' picture, filled a shelf of cells at a time.
struct Atlas {
    rgba: Vec<u8>,
    /// Where the next cell goes, and the shelf's height so far.
    x: usize,
    y: usize,
    shelf: usize,
}

impl Atlas {
    fn new() -> Self {
        Atlas {
            rgba: vec![0; ATLAS * ATLAS_HEIGHT * 4],
            x: 0,
            y: 0,
            shelf: 0,
        }
    }

    /// Room for a cell `w` by `h` pixels: where it goes.
    fn place(&mut self, w: usize, h: usize) -> (usize, usize) {
        if self.x + w > ATLAS {
            self.x = 0;
            self.y += self.shelf;
            self.shelf = 0;
        }
        let at = (self.x, self.y);
        self.x += w;
        self.shelf = self.shelf.max(h);
        assert!(
            self.y + h <= ATLAS_HEIGHT && w <= ATLAS,
            "the keyboard's legends don't fit their picture"
        );
        at
    }

    fn fill(&mut self, (x, y): (usize, usize), (w, h): (usize, usize), color: u32) {
        let rgb = [(color >> 16) as u8, (color >> 8) as u8, color as u8, 255];
        for row in y..y + h {
            for column in x..x + w {
                let i = (row * ATLAS + column) * 4;
                self.rgba[i..i + 4].copy_from_slice(&rgb);
            }
        }
    }

    /// The UV of a pixel.
    fn uv(x: usize, y: usize) -> Vec2 {
        Vec2::new(x as f32 / ATLAS as f32, y as f32 / ATLAS_HEIGHT as f32)
    }

    /// A cell of one colour: the UV of its middle.
    fn swatch(&mut self, color: u32) -> Vec2 {
        let at = self.place(16, 16);
        self.fill(at, (16, 16), color);
        Self::uv(at.0 + 8, at.1 + 8)
    }

    /// A cell for a cap's top (or a label) `size` big in metres, in `color`
    /// with `legend` written on it from its top left: one character big,
    /// words small. Its UV corners, top left and bottom right.
    fn cell(&mut self, size: Vec2, legend: &str, color: u32) -> [Vec2; 2] {
        let (w, h) = (
            (size.x * PIXELS).round().max(8.0) as usize,
            (size.y * PIXELS).round().max(8.0) as usize,
        );
        let at = self.place(w, h);
        self.fill(at, (w, h), color);
        let lines: Vec<&str> = legend.split('\n').collect();
        let margin = h.min(w) / 9;
        // A letter big, one character over another smaller, and words as
        // big as they fit.
        let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        let fits = |scale: usize| {
            longest * 8 * scale + 2 * margin <= w && lines.len() * (16 * scale + 2) + margin <= h
        };
        let scale = [3, 2]
            .into_iter()
            .find(|&s| longest <= 1 && fits(s))
            .unwrap_or(if fits(2) { 2 } else { 1 });
        let font = rust_dos::video::font_8x16();
        let gap = scale * 2;
        for (i, line) in lines.iter().enumerate() {
            let top = at.1 + margin + i * (16 * scale + gap);
            for (j, c) in line.chars().enumerate() {
                let glyph = &font[(c as usize & 0xFF) * 16..][..16];
                let left = at.0 + margin + j * 8 * scale;
                self.glyph(glyph, (left, top), scale, (at.0 + w, at.1 + h));
            }
        }
        // (Half a pixel in, so that the neighbours don't bleed in.)
        let (a, b) = (Self::uv(at.0, at.1), Self::uv(at.0 + w, at.1 + h));
        let half = Self::uv(1, 1) / 2.0;
        [a + half, b - half]
    }

    /// A glyph of the font at `at`, `scale` times its size, clipped to
    /// `end`.
    fn glyph(&mut self, rows: &[u8], at: (usize, usize), scale: usize, end: (usize, usize)) {
        let ink = [(INK >> 16) as u8, (INK >> 8) as u8, INK as u8, 255];
        for (r, bits) in rows.iter().enumerate() {
            for c in 0..8 {
                if bits & 0x80 >> c == 0 {
                    continue;
                }
                // (A pixel wider, bolder, as printed.)
                for dy in 0..scale {
                    for dx in 0..scale + 1 {
                        let (x, y) = (at.0 + c * scale + dx, at.1 + r * scale + dy);
                        if x < end.0 && y < end.1 {
                            let i = (y * ATLAS + x) * 4;
                            self.rgba[i..i + 4].copy_from_slice(&ink);
                        }
                    }
                }
            }
        }
    }

    fn finish(self) -> Image {
        Image {
            width: ATLAS as u32,
            height: ATLAS_HEIGHT as u32,
            rgba: self.rgba,
        }
    }
}

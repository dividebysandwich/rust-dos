//! The current vertex attributes: glColor, glTexCoord,
//! glMultiTexCoordARB, glNormal, and glVertex, which makes a vertex of
//! them between glBegin and glEnd. Each function is one of `attribute!`
//! for its kind, size and type, and whether it takes an array.
//!
//! A display list that keeps a glBegin to glEnd run as vertices
//! (`lists::Batch`) reads its recorded calls with the same conversions:
//! `spec` tells which of these functions an export is, and `given`
//! what a call of it gives.

use super::texture::GL_TEXTURE0;
use super::{CAP_COLOR_MATERIAL, GL_INVALID_ENUM, UNITS, ctx, error, signed_unit};
use crate::cpu::Cpu;

/// What a function sets.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Vertex,
    Color,
    TexCoord,
    MultiTexCoord,
    Normal,
}

/// The types of the values: signed and unsigned bytes, shorts and
/// ints, floats and doubles.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Type {
    B,
    UB,
    S,
    US,
    I,
    UI,
    F,
    D,
}

impl Type {
    /// The bytes of a value in memory.
    fn size(self) -> u32 {
        match self {
            Type::B | Type::UB => 1,
            Type::S | Type::US => 2,
            Type::D => 8,
            _ => 4,
        }
    }

    /// A value from its bytes or its argument's words; a color's
    /// integers go from 0 to 1 (or -1 to 1 signed), and a normal's from
    /// -1 to 1 as table 2.6 converts signed integers, (2c + 1) / (2^b -
    /// 1).
    fn value(self, lo: u32, hi: u32, kind: Kind) -> f32 {
        if kind == Kind::Normal {
            return match self {
                Type::B => signed_unit(lo as u8 as i8 as f64, 8),
                Type::S => signed_unit(lo as u16 as i16 as f64, 16),
                Type::I => signed_unit(lo as i32 as f64, 32),
                _ => self.value(lo, hi, Kind::Vertex),
            };
        }
        let color = kind == Kind::Color;
        let (v, most) = match self {
            Type::B => (lo as u8 as i8 as f32, 127.0),
            Type::UB => (lo as u8 as f32, 255.0),
            Type::S => (lo as u16 as i16 as f32, 32767.0),
            Type::US => (lo as u16 as f32, 65535.0),
            Type::I => (lo as i32 as f32, i32::MAX as f32),
            Type::UI => (lo as f32, u32::MAX as f32),
            Type::F => return f32::from_bits(lo),
            Type::D => return f64::from_bits((hi as u64) << 32 | lo as u64) as f32,
        };
        if color { v / most } else { v }
    }
}

/// The `n` values of a call: from its arguments after `skip` of them,
/// or from the array its argument after them points at.
fn values(
    cpu: &mut Cpu,
    args: &[u32],
    skip: usize,
    n: usize,
    t: Type,
    array: bool,
    kind: Kind,
) -> Option<[f32; 4]> {
    if !array {
        return decode(args, skip, n, t, None, kind);
    }
    let at = *args.get(skip)?;
    let len = n * t.size() as usize;
    if at == 0 || !cpu.bus.guest_probe(at, len, false) {
        return None;
    }
    let mut b = vec![0u8; len];
    cpu.bus.guest_read_bytes(at, &mut b);
    decode(args, skip, n, t, Some(&b), kind)
}

/// The `n` values of a call from the bytes of its array, or without
/// them from its arguments after `skip` of them.
fn decode(
    args: &[u32],
    skip: usize,
    n: usize,
    t: Type,
    array: Option<&[u8]>,
    kind: Kind,
) -> Option<[f32; 4]> {
    let mut out = [0.0; 4];
    if let Some(b) = array {
        let size = t.size();
        if b.len() < n * size as usize {
            return None;
        }
        for (i, o) in out.iter_mut().enumerate().take(n) {
            let p = &b[i * size as usize..];
            let word = |k: usize| {
                let mut w = [0u8; 4];
                let have = (size as usize).saturating_sub(4 * k).min(4);
                w[..have].copy_from_slice(&p[4 * k..4 * k + have]);
                u32::from_le_bytes(w)
            };
            *o = t.value(word(0), if t == Type::D { word(1) } else { 0 }, kind);
        }
    } else {
        let words = if t == Type::D { 2 } else { 1 };
        for (i, o) in out.iter_mut().enumerate().take(n) {
            let at = skip + i * words;
            let lo = *args.get(at)?;
            let hi = if t == Type::D { *args.get(at + 1)? } else { 0 };
            *o = t.value(lo, hi, kind);
        }
    }
    Some(out)
}

/// Set an attribute, or make a vertex.
fn set(cpu: &mut Cpu, kind: Kind, n: usize, t: Type, array: bool, args: &[u32]) {
    let skip = (kind == Kind::MultiTexCoord) as usize;
    let Some(v) = values(cpu, args, skip, n, t, array, kind) else {
        return;
    };
    match kind {
        Kind::Vertex => super::draw::vertex(cpu, position(n, v)),
        Kind::Color => {
            if let Some(c) = ctx(cpu) {
                c.color = color(n, v);
                // The material properties glColorMaterial tracks follow
                // the current color (section 2.13.3).
                if c.caps & CAP_COLOR_MATERIAL != 0 {
                    c.lighting.track(c.color);
                }
            }
        }
        Kind::Normal => {
            if let Some(c) = ctx(cpu) {
                c.normal = [v[0], v[1], v[2]];
            }
        }
        Kind::TexCoord | Kind::MultiTexCoord => {
            let Some(unit) = unit(kind, args) else {
                error(cpu, GL_INVALID_ENUM);
                return;
            };
            if let Some(c) = ctx(cpu) {
                c.tex_coord[unit] = tex_coord(n, v);
            }
        }
    }
}

/// A vertex of `n` coordinates: z is 0 and w is 1 where it has none.
fn position(n: usize, v: [f32; 4]) -> [f32; 4] {
    let w = if n == 4 { v[3] } else { 1.0 };
    let z = if n >= 3 { v[2] } else { 0.0 };
    [v[0], v[1], z, w]
}

/// A color of `n` components: opaque without an alpha.
fn color(n: usize, v: [f32; 4]) -> [f32; 4] {
    [v[0], v[1], v[2], if n == 4 { v[3] } else { 1.0 }]
}

/// Texture coordinates of `n` values: t and r are 0 and q is 1 where
/// there is none.
fn tex_coord(n: usize, v: [f32; 4]) -> [f32; 4] {
    [
        v[0],
        if n >= 2 { v[1] } else { 0.0 },
        if n >= 3 { v[2] } else { 0.0 },
        if n == 4 { v[3] } else { 1.0 },
    ]
}

/// The texture unit a call gives coordinates for: the first, or the
/// one glMultiTexCoord's first argument names. None for a unit there
/// isn't.
fn unit(kind: Kind, args: &[u32]) -> Option<usize> {
    if kind == Kind::TexCoord {
        return Some(0);
    }
    let unit = args.first()?.checked_sub(GL_TEXTURE0)? as usize;
    (unit < UNITS).then_some(unit)
}

/// One of the functions here: what it sets, from how many values of
/// which type, and whether it takes them in an array.
#[derive(Clone, Copy)]
pub struct Spec {
    kind: Kind,
    n: usize,
    t: Type,
    array: bool,
}

impl Spec {
    /// The place of the argument that points at the values, for a
    /// function that takes an array.
    pub fn pointer(&self) -> Option<usize> {
        self.array
            .then_some((self.kind == Kind::MultiTexCoord) as usize)
    }

    /// How many argument words the function has.
    pub fn words(&self) -> usize {
        let values = match (self.array, self.t) {
            (true, _) => 1,
            (false, Type::D) => 2 * self.n,
            (false, _) => self.n,
        };
        (self.kind == Kind::MultiTexCoord) as usize + values
    }
}

/// The function the export `name` is, None for one that isn't one of
/// these: glColor4ubv sets the color from an array of four unsigned
/// bytes, and glMultiTexCoord2fARB a unit's coordinates from two
/// floats.
pub fn spec(name: &str) -> Option<Spec> {
    const KINDS: [(&str, Kind); 5] = [
        ("glVertex", Kind::Vertex),
        ("glColor", Kind::Color),
        ("glTexCoord", Kind::TexCoord),
        ("glMultiTexCoord", Kind::MultiTexCoord),
        ("glNormal", Kind::Normal),
    ];
    const TYPES: [(&str, Type); 8] = [
        ("ub", Type::UB),
        ("us", Type::US),
        ("ui", Type::UI),
        ("b", Type::B),
        ("s", Type::S),
        ("i", Type::I),
        ("f", Type::F),
        ("d", Type::D),
    ];
    let (rest, kind) = KINDS
        .iter()
        .find_map(|&(start, kind)| Some((name.strip_prefix(start)?, kind)))?;
    let rest = match kind {
        Kind::MultiTexCoord => rest.strip_suffix("ARB")?,
        _ => rest,
    };
    let n = rest.chars().next()?.to_digit(10)? as usize;
    let rest = &rest[1..];
    let (rest, array) = match rest.strip_suffix('v') {
        Some(rest) => (rest, true),
        None => (rest, false),
    };
    let &(_, t) = TYPES.iter().find(|(letters, _)| *letters == rest)?;
    Some(Spec { kind, n, t, array })
}

/// What a call of one of these functions gives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Given {
    Vertex([f32; 4]),
    Color([f32; 4]),
    Normal([f32; 3]),
    TexCoord(usize, [f32; 4]),
}

/// What the call of `spec`'s function with the arguments `args` gives,
/// `array` being the bytes its pointer points at. None when there are
/// too few of them, and for a texture unit there isn't.
pub fn given(spec: &Spec, args: &[u32], array: Option<&[u8]>) -> Option<Given> {
    let Spec { kind, n, t, .. } = *spec;
    if spec.array != array.is_some() {
        return None;
    }
    let skip = (kind == Kind::MultiTexCoord) as usize;
    let v = decode(args, skip, n, t, array, kind)?;
    Some(match kind {
        Kind::Vertex => Given::Vertex(position(n, v)),
        Kind::Color => Given::Color(color(n, v)),
        Kind::Normal => Given::Normal([v[0], v[1], v[2]]),
        Kind::TexCoord | Kind::MultiTexCoord => Given::TexCoord(unit(kind, args)?, tex_coord(n, v)),
    })
}

macro_rules! attribute {
    ($name:ident, $kind:ident, $n:literal, $t:ident, $array:literal) => {
        pub fn $name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
            set(cpu, Kind::$kind, $n, Type::$t, $array, args);
            Some(0)
        }
    };
}

attribute!(color3b, Color, 3, B, false);
attribute!(color3bv, Color, 3, B, true);
attribute!(color3d, Color, 3, D, false);
attribute!(color3dv, Color, 3, D, true);
attribute!(color3f, Color, 3, F, false);
attribute!(color3fv, Color, 3, F, true);
attribute!(color3i, Color, 3, I, false);
attribute!(color3iv, Color, 3, I, true);
attribute!(color3s, Color, 3, S, false);
attribute!(color3sv, Color, 3, S, true);
attribute!(color3ub, Color, 3, UB, false);
attribute!(color3ubv, Color, 3, UB, true);
attribute!(color3ui, Color, 3, UI, false);
attribute!(color3uiv, Color, 3, UI, true);
attribute!(color3us, Color, 3, US, false);
attribute!(color3usv, Color, 3, US, true);
attribute!(color4b, Color, 4, B, false);
attribute!(color4bv, Color, 4, B, true);
attribute!(color4d, Color, 4, D, false);
attribute!(color4dv, Color, 4, D, true);
attribute!(color4f, Color, 4, F, false);
attribute!(color4fv, Color, 4, F, true);
attribute!(color4i, Color, 4, I, false);
attribute!(color4iv, Color, 4, I, true);
attribute!(color4s, Color, 4, S, false);
attribute!(color4sv, Color, 4, S, true);
attribute!(color4ub, Color, 4, UB, false);
attribute!(color4ubv, Color, 4, UB, true);
attribute!(color4ui, Color, 4, UI, false);
attribute!(color4uiv, Color, 4, UI, true);
attribute!(color4us, Color, 4, US, false);
attribute!(color4usv, Color, 4, US, true);
attribute!(normal3b, Normal, 3, B, false);
attribute!(normal3bv, Normal, 3, B, true);
attribute!(normal3d, Normal, 3, D, false);
attribute!(normal3dv, Normal, 3, D, true);
attribute!(normal3f, Normal, 3, F, false);
attribute!(normal3fv, Normal, 3, F, true);
attribute!(normal3i, Normal, 3, I, false);
attribute!(normal3iv, Normal, 3, I, true);
attribute!(normal3s, Normal, 3, S, false);
attribute!(normal3sv, Normal, 3, S, true);
attribute!(tex_coord1d, TexCoord, 1, D, false);
attribute!(tex_coord1dv, TexCoord, 1, D, true);
attribute!(tex_coord1f, TexCoord, 1, F, false);
attribute!(tex_coord1fv, TexCoord, 1, F, true);
attribute!(tex_coord1i, TexCoord, 1, I, false);
attribute!(tex_coord1iv, TexCoord, 1, I, true);
attribute!(tex_coord1s, TexCoord, 1, S, false);
attribute!(tex_coord1sv, TexCoord, 1, S, true);
attribute!(tex_coord2d, TexCoord, 2, D, false);
attribute!(tex_coord2dv, TexCoord, 2, D, true);
attribute!(tex_coord2f, TexCoord, 2, F, false);
attribute!(tex_coord2fv, TexCoord, 2, F, true);
attribute!(tex_coord2i, TexCoord, 2, I, false);
attribute!(tex_coord2iv, TexCoord, 2, I, true);
attribute!(tex_coord2s, TexCoord, 2, S, false);
attribute!(tex_coord2sv, TexCoord, 2, S, true);
attribute!(tex_coord3d, TexCoord, 3, D, false);
attribute!(tex_coord3dv, TexCoord, 3, D, true);
attribute!(tex_coord3f, TexCoord, 3, F, false);
attribute!(tex_coord3fv, TexCoord, 3, F, true);
attribute!(tex_coord3i, TexCoord, 3, I, false);
attribute!(tex_coord3iv, TexCoord, 3, I, true);
attribute!(tex_coord3s, TexCoord, 3, S, false);
attribute!(tex_coord3sv, TexCoord, 3, S, true);
attribute!(tex_coord4d, TexCoord, 4, D, false);
attribute!(tex_coord4dv, TexCoord, 4, D, true);
attribute!(tex_coord4f, TexCoord, 4, F, false);
attribute!(tex_coord4fv, TexCoord, 4, F, true);
attribute!(tex_coord4i, TexCoord, 4, I, false);
attribute!(tex_coord4iv, TexCoord, 4, I, true);
attribute!(tex_coord4s, TexCoord, 4, S, false);
attribute!(tex_coord4sv, TexCoord, 4, S, true);
attribute!(vertex2d, Vertex, 2, D, false);
attribute!(vertex2dv, Vertex, 2, D, true);
attribute!(vertex2f, Vertex, 2, F, false);
attribute!(vertex2fv, Vertex, 2, F, true);
attribute!(vertex2i, Vertex, 2, I, false);
attribute!(vertex2iv, Vertex, 2, I, true);
attribute!(vertex2s, Vertex, 2, S, false);
attribute!(vertex2sv, Vertex, 2, S, true);
attribute!(vertex3d, Vertex, 3, D, false);
attribute!(vertex3dv, Vertex, 3, D, true);
attribute!(vertex3f, Vertex, 3, F, false);
attribute!(vertex3fv, Vertex, 3, F, true);
attribute!(vertex3i, Vertex, 3, I, false);
attribute!(vertex3iv, Vertex, 3, I, true);
attribute!(vertex3s, Vertex, 3, S, false);
attribute!(vertex3sv, Vertex, 3, S, true);
attribute!(vertex4d, Vertex, 4, D, false);
attribute!(vertex4dv, Vertex, 4, D, true);
attribute!(vertex4f, Vertex, 4, F, false);
attribute!(vertex4fv, Vertex, 4, F, true);
attribute!(vertex4i, Vertex, 4, I, false);
attribute!(vertex4iv, Vertex, 4, I, true);
attribute!(vertex4s, Vertex, 4, S, false);
attribute!(vertex4sv, Vertex, 4, S, true);
attribute!(multi_tex_coord1d, MultiTexCoord, 1, D, false);
attribute!(multi_tex_coord1dv, MultiTexCoord, 1, D, true);
attribute!(multi_tex_coord1f, MultiTexCoord, 1, F, false);
attribute!(multi_tex_coord1fv, MultiTexCoord, 1, F, true);
attribute!(multi_tex_coord1i, MultiTexCoord, 1, I, false);
attribute!(multi_tex_coord1iv, MultiTexCoord, 1, I, true);
attribute!(multi_tex_coord1s, MultiTexCoord, 1, S, false);
attribute!(multi_tex_coord1sv, MultiTexCoord, 1, S, true);
attribute!(multi_tex_coord2d, MultiTexCoord, 2, D, false);
attribute!(multi_tex_coord2dv, MultiTexCoord, 2, D, true);
attribute!(multi_tex_coord2f, MultiTexCoord, 2, F, false);
attribute!(multi_tex_coord2fv, MultiTexCoord, 2, F, true);
attribute!(multi_tex_coord2i, MultiTexCoord, 2, I, false);
attribute!(multi_tex_coord2iv, MultiTexCoord, 2, I, true);
attribute!(multi_tex_coord2s, MultiTexCoord, 2, S, false);
attribute!(multi_tex_coord2sv, MultiTexCoord, 2, S, true);
attribute!(multi_tex_coord3d, MultiTexCoord, 3, D, false);
attribute!(multi_tex_coord3dv, MultiTexCoord, 3, D, true);
attribute!(multi_tex_coord3f, MultiTexCoord, 3, F, false);
attribute!(multi_tex_coord3fv, MultiTexCoord, 3, F, true);
attribute!(multi_tex_coord3i, MultiTexCoord, 3, I, false);
attribute!(multi_tex_coord3iv, MultiTexCoord, 3, I, true);
attribute!(multi_tex_coord3s, MultiTexCoord, 3, S, false);
attribute!(multi_tex_coord3sv, MultiTexCoord, 3, S, true);
attribute!(multi_tex_coord4d, MultiTexCoord, 4, D, false);
attribute!(multi_tex_coord4dv, MultiTexCoord, 4, D, true);
attribute!(multi_tex_coord4f, MultiTexCoord, 4, F, false);
attribute!(multi_tex_coord4fv, MultiTexCoord, 4, F, true);
attribute!(multi_tex_coord4i, MultiTexCoord, 4, I, false);
attribute!(multi_tex_coord4iv, MultiTexCoord, 4, I, true);
attribute!(multi_tex_coord4s, MultiTexCoord, 4, S, false);
attribute!(multi_tex_coord4sv, MultiTexCoord, 4, S, true);

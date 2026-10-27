//! Texture objects, their images and parameters, the texture
//! environments, and GL_ARB_multitexture's units.
//!
//! A texture's levels are a DirectDraw texture surface with its mipmap
//! chain, made when level 0 is given: 5-6-5 for a texture without alpha
//! and 4-4-4-4 for one with it, the formats 3dfx's ICD kept textures in
//! for the card. It is in video memory while there is room past what
//! the HAL's texture heap needs, where the card samples it in place, or
//! in system memory, which the HAL copies into its heap as it draws.
//! The program's texels are converted as they are given.

use super::super::ddraw::directdraw;
use super::super::ddraw::surface::{
    self as dd_surface, DDPF_ALPHAPIXELS, DDPF_RGB, DDSCAPS_COMPLEX, DDSCAPS_MIPMAP,
    DDSCAPS_SYSTEMMEMORY, DDSCAPS_TEXTURE, DDSCAPS_VIDEOMEMORY, DDSD_CAPS, DDSD_HEIGHT,
    DDSD_MIPMAPCOUNT, DDSD_PIXELFORMAT, DDSD_WIDTH, Desc,
};
use super::super::ddraw::{self, Format, V_SURFACE7};
use super::{
    Context, GL_INVALID_ENUM, GL_INVALID_OPERATION, GL_INVALID_VALUE, GL_OUT_OF_MEMORY,
    MAX_TEXTURE_SIZE, Texture, UNITS, ctx, error, f, read_floats, state,
};
use crate::cpu::Cpu;

pub const GL_TEXTURE0: u32 = 0x84C0;
const GL_TEXTURE_2D: u32 = 0x0DE1;

// Parameters.
const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
const GL_TEXTURE_WRAP_S: u32 = 0x2802;
const GL_TEXTURE_WRAP_T: u32 = 0x2803;
const GL_TEXTURE_BORDER_COLOR: u32 = 0x1004;
const GL_TEXTURE_PRIORITY: u32 = 0x8066;
pub const GL_NEAREST: u32 = 0x2600;
pub const GL_LINEAR: u32 = 0x2601;
const GL_NEAREST_MIPMAP_NEAREST: u32 = 0x2700;
const GL_LINEAR_MIPMAP_LINEAR: u32 = 0x2703;
pub const GL_CLAMP: u32 = 0x2900;
pub const GL_REPEAT: u32 = 0x2901;
const GL_CLAMP_TO_EDGE: u32 = 0x812F;

// The environment.
const GL_TEXTURE_ENV: u32 = 0x2300;
const GL_TEXTURE_ENV_MODE: u32 = 0x2200;
const GL_TEXTURE_ENV_COLOR: u32 = 0x2201;
pub const GL_MODULATE: u32 = 0x2100;
pub const GL_DECAL: u32 = 0x2101;
pub const GL_BLEND: u32 = 0x0BE2;
pub const GL_REPLACE: u32 = 0x1E01;
pub const GL_ADD: u32 = 0x0104;

// Formats and types of texels.
const GL_COLOR_INDEX: u32 = 0x1900;
const GL_RED: u32 = 0x1903;
const GL_GREEN: u32 = 0x1904;
const GL_BLUE: u32 = 0x1905;
const GL_ALPHA: u32 = 0x1906;
const GL_RGB: u32 = 0x1907;
const GL_RGBA: u32 = 0x1908;
const GL_LUMINANCE: u32 = 0x1909;
const GL_LUMINANCE_ALPHA: u32 = 0x190A;
const GL_BGR_EXT: u32 = 0x80E0;
const GL_BGRA_EXT: u32 = 0x80E1;
const GL_BYTE: u32 = 0x1400;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_SHORT: u32 = 0x1402;
const GL_UNSIGNED_SHORT: u32 = 0x1403;
const GL_INT: u32 = 0x1404;
const GL_UNSIGNED_INT: u32 = 0x1405;
const GL_FLOAT: u32 = 0x1406;

/// What a texture's texels are made of, by its internal format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Base {
    Alpha,
    Luminance,
    LuminanceAlpha,
    Intensity,
    Rgb,
    Rgba,
}

/// The base format of the internal format `internal`, None for one
/// there isn't.
fn base_of(internal: u32) -> Option<Base> {
    Some(match internal {
        GL_ALPHA | 0x803B..=0x803E => Base::Alpha,
        1 | GL_LUMINANCE | 0x803F..=0x8042 => Base::Luminance,
        2 | GL_LUMINANCE_ALPHA | 0x8043..=0x8048 => Base::LuminanceAlpha,
        0x8049..=0x804D => Base::Intensity,
        3 | GL_RGB | 0x2A10 | 0x804F..=0x8054 => Base::Rgb,
        4 | GL_RGBA | 0x8055..=0x805B => Base::Rgba,
        _ => return None,
    })
}

fn has_alpha(base: Base) -> bool {
    matches!(
        base,
        Base::Alpha | Base::LuminanceAlpha | Base::Intensity | Base::Rgba
    )
}

/// The components of a texel of `format`, None for a format there
/// isn't.
fn components(format: u32) -> Option<u32> {
    Some(match format {
        GL_RED | GL_GREEN | GL_BLUE | GL_ALPHA | GL_LUMINANCE | GL_COLOR_INDEX => 1,
        GL_LUMINANCE_ALPHA => 2,
        GL_RGB | GL_BGR_EXT => 3,
        GL_RGBA | GL_BGRA_EXT => 4,
        _ => return None,
    })
}

/// The bytes of a component of `kind`.
fn type_size(kind: u32) -> Option<u32> {
    Some(match kind {
        GL_BYTE | GL_UNSIGNED_BYTE => 1,
        GL_SHORT | GL_UNSIGNED_SHORT => 2,
        GL_INT | GL_UNSIGNED_INT | GL_FLOAT => 4,
        _ => return None,
    })
}

/// A component of `kind` from its bytes, 0 to 1.
fn component(b: &[u8], kind: u32) -> f32 {
    match kind {
        GL_UNSIGNED_BYTE => b[0] as f32 / 255.0,
        GL_BYTE => (b[0] as i8 as f32 / 127.0).max(0.0),
        GL_UNSIGNED_SHORT => u16::from_le_bytes([b[0], b[1]]) as f32 / 65535.0,
        GL_SHORT => (i16::from_le_bytes([b[0], b[1]]) as f32 / 32767.0).max(0.0),
        GL_UNSIGNED_INT => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / u32::MAX as f32,
        GL_INT => (i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / i32::MAX as f32).max(0.0),
        _ => f32::from_le_bytes([b[0], b[1], b[2], b[3]]).clamp(0.0, 1.0),
    }
}

/// An image the program gives: its size, format and type, and where
/// its rows start.
struct Source {
    width: u32,
    height: u32,
    format: u32,
    kind: u32,
    at: u32,
}

/// The texels of `src` as red, green, blue and alpha bytes, row by row
/// from the first the program gave, with the unpack state of `c`
/// (alignment, row length, rows and pixels skipped).
fn unpack(cpu: &mut Cpu, unpack: [u32; 4], src: &Source) -> Option<Vec<[u8; 4]>> {
    let n = components(src.format)?;
    let size = type_size(src.kind)?;
    let pixel = n * size;
    let row_pixels = if unpack[1] > 0 { unpack[1] } else { src.width };
    let align = unpack[0].max(1);
    let stride = row_pixels
        .checked_mul(pixel)?
        .checked_next_multiple_of(align)?;
    let skip = unpack[2]
        .checked_mul(stride)?
        .checked_add(unpack[3].checked_mul(pixel)?)?;
    let start = src.at.checked_add(skip)?;
    let len = src
        .height
        .saturating_sub(1)
        .checked_mul(stride)?
        .checked_add(src.width.checked_mul(pixel)?)? as usize;
    if src.at == 0 || len > 0x400_0000 || !cpu.bus.guest_probe(start, len, false) {
        return None;
    }
    let mut bytes = vec![0u8; len];
    cpu.bus.guest_read_bytes(start, &mut bytes);
    let mut out = Vec::with_capacity((src.width * src.height) as usize);
    for y in 0..src.height {
        let row = &bytes[(y * stride) as usize..];
        for x in 0..src.width {
            let p = &row[(x * pixel) as usize..];
            let c = |i: u32| {
                let at = (i * size) as usize;
                (component(&p[at..at + size as usize], src.kind) * 255.0 + 0.5) as u8
            };
            let rgba = match src.format {
                GL_RGB => [c(0), c(1), c(2), 255],
                GL_RGBA => [c(0), c(1), c(2), c(3)],
                GL_BGR_EXT => [c(2), c(1), c(0), 255],
                GL_BGRA_EXT => [c(2), c(1), c(0), c(3)],
                GL_LUMINANCE | GL_COLOR_INDEX => [c(0), c(0), c(0), 255],
                GL_LUMINANCE_ALPHA => [c(0), c(0), c(0), c(1)],
                GL_ALPHA => [0, 0, 0, c(0)],
                GL_RED => [c(0), 0, 0, 255],
                GL_GREEN => [0, c(0), 0, 255],
                _ => [0, 0, c(0), 255],
            };
            out.push(rgba);
        }
    }
    Some(out)
}

/// A texel as the texture keeps it: the components of `base` taken
/// from red, green, blue and alpha, 5-6-5 or 4-4-4-4.
fn encode(rgba: [u8; 4], base: Base) -> u16 {
    let [r, g, b, a] = rgba.map(u16::from);
    let (r, g, b, a) = match base {
        Base::Alpha => (255, 255, 255, a),
        Base::Luminance => (r, r, r, 255),
        Base::LuminanceAlpha => (r, r, r, a),
        Base::Intensity => (r, r, r, r),
        Base::Rgb => (r, g, b, 255),
        Base::Rgba => (r, g, b, a),
    };
    if has_alpha(base) {
        (a >> 4) << 12 | (r >> 4) << 8 | (g >> 4) << 4 | b >> 4
    } else {
        (r >> 3) << 11 | (g >> 2) << 5 | b >> 3
    }
}

/// The texture named `name` of the context `c`, made if it isn't there
/// (the default texture, 0, and names glGenTextures gave or not).
fn texture_mut(c: &mut Context, name: u32) -> &mut Texture {
    match c.textures.iter().position(|t| t.name == name) {
        Some(i) => &mut c.textures[i],
        None => {
            c.textures.push(Texture {
                name,
                min_filter: GL_NEAREST_MIPMAP_LINEAR,
                mag_filter: GL_LINEAR,
                wrap_s: GL_REPEAT,
                wrap_t: GL_REPEAT,
                ..Texture::default()
            });
            c.textures.last_mut().expect("just pushed")
        }
    }
}

const GL_NEAREST_MIPMAP_LINEAR: u32 = 0x2702;

/// The texture bound to the active unit.
fn bound(c: &mut Context) -> &mut Texture {
    let name = c.bound[c.active_unit as usize];
    texture_mut(c, name)
}

/// The texture `name` of the current context, as it is now.
pub fn texture(c: &Context, name: u32) -> Option<&Texture> {
    c.textures.iter().find(|t| t.name == name)
}

/// glGenTextures(n, textures).
pub fn gen_textures(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [n, at] = args[..] else {
        return Some(0);
    };
    let n = n as i32;
    if n < 0 {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    if n == 0 || at == 0 || !cpu.bus.guest_probe(at, n as usize * 4, true) {
        return Some(0);
    }
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    let mut names = Vec::with_capacity(n as usize);
    let mut next = c.textures.iter().map(|t| t.name).max().unwrap_or(0);
    for _ in 0..n {
        next += 1;
        names.push(next);
        texture_mut(c, next);
    }
    for (i, name) in names.into_iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, name);
    }
    Some(0)
}

/// Let go of the surfaces of the texture `t`.
fn release_surface(cpu: &mut Cpu, t: &Texture) {
    if t.iface != 0 {
        ddraw::release(cpu, t.iface);
    }
}

/// glDeleteTextures(n, textures): the units they are bound to have the
/// default texture again.
pub fn delete_textures(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [n, at] = args[..] else {
        return Some(0);
    };
    let n = n as i32;
    if n < 0 {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    if n == 0 || at == 0 || !cpu.bus.guest_probe(at, n as usize * 4, false) {
        return Some(0);
    }
    let names: Vec<u32> = (0..n as u32)
        .map(|i| cpu.bus.guest_read_32(at + 4 * i))
        .collect();
    let mut gone = Vec::new();
    if let Some(c) = ctx(cpu) {
        for name in names.into_iter().filter(|&n| n != 0) {
            if let Some(i) = c.textures.iter().position(|t| t.name == name) {
                gone.push(c.textures.remove(i));
            }
            for b in c.bound.iter_mut() {
                if *b == name {
                    *b = 0;
                }
            }
        }
    }
    for t in &gone {
        release_surface(cpu, t);
    }
    Some(0)
}

/// Let go of the textures of the context at `index`.
pub fn release_all(cpu: &mut Cpu, index: usize) {
    let textures = match state(cpu).contexts.get_mut(index) {
        Some(Some(c)) => std::mem::take(&mut c.textures),
        _ => return,
    };
    for t in &textures {
        release_surface(cpu, t);
    }
}

/// glBindTexture(target, texture).
pub fn bind_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] != GL_TEXTURE_2D {
        if args[0] != 0x0DE0 {
            error(cpu, GL_INVALID_ENUM);
        }
        return Some(0);
    }
    if let Some(c) = ctx(cpu) {
        let unit = c.active_unit as usize;
        c.bound[unit] = args[1];
        texture_mut(c, args[1]);
    }
    Some(0)
}

/// glIsTexture(texture).
pub fn is_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    Some((args[0] != 0 && texture(c, args[0]).is_some()) as u32)
}

/// The levels a texture of `width` by `height` has, down to 1 by 1.
fn level_count(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

/// Whether video memory has room for `size` more bytes and the HAL's
/// texture heap after them.
fn video_room(cpu: &mut Cpu, size: u32) -> bool {
    const KEEP: u32 = 5 << 20;
    let (_, free) = super::super::ddraw::display::video_memory(cpu);
    free >= size + KEEP
}

/// Make the surfaces of the bound texture for a level 0 of `width` by
/// `height`, with alpha or without, as the context's DirectDraw
/// object's: their ids and the interface of the first.
fn make_surface(
    cpu: &mut Cpu,
    dd: u32,
    width: u32,
    height: u32,
    alpha: bool,
) -> Option<(Vec<u32>, u32)> {
    let levels = level_count(width, height);
    let format = if alpha {
        Format {
            flags: DDPF_RGB | DDPF_ALPHAPIXELS,
            bits: 16,
            r: 0x0F00,
            g: 0x00F0,
            b: 0x000F,
            a: 0xF000,
            ..Format::default()
        }
    } else {
        Format {
            flags: DDPF_RGB,
            bits: 16,
            r: 0xF800,
            g: 0x07E0,
            b: 0x001F,
            ..Format::default()
        }
    };
    let size: u32 = (0..levels)
        .map(|l| (width >> l).max(1) * (height >> l).max(1) * 2)
        .sum();
    let memory = if video_room(cpu, size) {
        DDSCAPS_VIDEOMEMORY
    } else {
        DDSCAPS_SYSTEMMEMORY
    };
    let mut desc = Desc {
        flags: DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT,
        width,
        height,
        format,
        caps: [DDSCAPS_TEXTURE | memory, 0, 0, 0],
        ..Desc::default()
    };
    if levels > 1 {
        desc.caps[0] |= DDSCAPS_MIPMAP | DDSCAPS_COMPLEX;
        desc.flags |= DDSD_MIPMAPCOUNT;
        desc.mipmaps = levels;
    }
    let level = directdraw::ddraw(cpu, dd).map_or(0, |d| d.level);
    let ids = match dd_surface::make(cpu, dd, 7, level, &desc) {
        Ok(ids) => ids,
        Err(_) if memory == DDSCAPS_VIDEOMEMORY => {
            desc.caps[0] = desc.caps[0] & !DDSCAPS_VIDEOMEMORY | DDSCAPS_SYSTEMMEMORY;
            dd_surface::make(cpu, dd, 7, level, &desc).ok()?
        }
        Err(_) => return None,
    };
    let iface = ddraw::iface(cpu, ids[0], V_SURFACE7);
    if iface == 0 {
        ddraw::destroy(cpu, ids[0]);
        return None;
    }
    ddraw::add_ref(cpu, iface);
    Some((ids, iface))
}

/// Whether `len` texels from `at` are in a level `size` texels wide or
/// high (a negative `at` from the program is not).
fn fits(at: u32, len: u32, size: u32) -> bool {
    at.checked_add(len).is_some_and(|end| end <= size)
}

/// Write `texels` (16 bits each), `w` by `h`, into level surface `id`
/// at (`x`, `y`).
fn write_level(cpu: &mut Cpu, id: u32, x: u32, y: u32, w: u32, h: u32, texels: &[u16]) {
    let Some(s) = ddraw::state(cpu).surface(id).cloned() else {
        return;
    };
    if !fits(x, w, s.width) || !fits(y, h, s.height) {
        return;
    }
    let mut row = Vec::with_capacity(w as usize * 2);
    for r in 0..h {
        row.clear();
        for t in &texels[(r * w) as usize..((r + 1) * w) as usize] {
            row.extend_from_slice(&t.to_le_bytes());
        }
        dd_surface::write_bytes(cpu, &s, (y + r) * s.pitch + x * 2, &row);
    }
    dd_surface::changed(cpu, id);
}

/// The arguments of glTexImage2D after the target and level.
struct Image {
    level: u32,
    internal: u32,
    width: u32,
    height: u32,
    border: u32,
    format: u32,
    kind: u32,
    at: u32,
}

/// Define level `img.level` of the bound texture, with the texels at
/// `img.at` (none for NULL), or with `texels` already converted.
fn define(cpu: &mut Cpu, img: &Image, texels: Option<Vec<[u8; 4]>>) {
    let Some(base) = base_of(img.internal) else {
        error(cpu, GL_INVALID_VALUE);
        return;
    };
    let (w, h, level) = (img.width, img.height, img.level);
    let pow2 = w.is_power_of_two() && h.is_power_of_two();
    if img.border != 0 || !pow2 || w > MAX_TEXTURE_SIZE || h > MAX_TEXTURE_SIZE || level > 8 {
        error(cpu, GL_INVALID_VALUE);
        return;
    }
    let alpha = has_alpha(base);
    let Some(c) = ctx(cpu) else {
        return;
    };
    let (dd, t) = (c.dd, bound(c).clone());
    let unpacked = match texels {
        Some(t) => Some(t),
        None if img.at == 0 => None,
        None => {
            let src = Source {
                width: w,
                height: h,
                format: img.format,
                kind: img.kind,
                at: img.at,
            };
            let store = ctx(cpu).map_or([4, 0, 0, 0], |c| c.unpack);
            match unpack(cpu, store, &src) {
                Some(t) => Some(t),
                None => {
                    error(cpu, GL_INVALID_ENUM);
                    return;
                }
            }
        }
    };
    let mut t = t;
    if level == 0 {
        // A new size or format: new surfaces, and the other levels go.
        if t.levels.is_empty() || t.width != w || t.height != h || t.alpha != alpha {
            release_surface(cpu, &t);
            let Some((ids, iface)) = make_surface(cpu, dd, w, h, alpha) else {
                t.levels.clear();
                t.iface = 0;
                t.defined = 0;
                store(cpu, t);
                error(cpu, GL_OUT_OF_MEMORY);
                return;
            };
            t.levels = ids;
            t.iface = iface;
            t.width = w;
            t.height = h;
            t.alpha = alpha;
            t.defined = 0;
        }
        t.internal = img.internal;
    } else {
        let fits = (t.width >> level).max(1) == w && (t.height >> level).max(1) == h;
        if !fits || t.alpha != alpha || t.levels.len() <= level as usize {
            // A level that doesn't follow level 0: the texture isn't
            // complete with it, so the card draws without mipmaps.
            t.defined &= !(1 << level);
            store(cpu, t);
            return;
        }
    }
    t.defined |= 1 << level;
    let id = t.levels[level as usize];
    store(cpu, t);
    if let Some(texels) = unpacked {
        let words: Vec<u16> = texels.into_iter().map(|p| encode(p, base)).collect();
        write_level(cpu, id, 0, 0, w, h, &words);
    }
}

/// Put `t` back as the bound texture of the active unit.
fn store(cpu: &mut Cpu, t: Texture) {
    if let Some(c) = ctx(cpu) {
        *bound(c) = t;
    }
}

/// glTexImage2D(target, level, internal, width, height, border, format,
/// type, pixels).
pub fn tex_image_2d(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [
        target,
        level,
        internal,
        width,
        height,
        border,
        format,
        kind,
        at,
    ] = args[..]
    else {
        return Some(0);
    };
    if target != GL_TEXTURE_2D {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if ctx(cpu).is_none() {
        return Some(0);
    }
    let img = Image {
        level,
        internal,
        width,
        height,
        border,
        format,
        kind,
        at,
    };
    define(cpu, &img, None);
    Some(0)
}

/// glTexSubImage2D(target, level, x, y, width, height, format, type,
/// pixels).
pub fn tex_sub_image_2d(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [target, level, x, y, width, height, format, kind, at] = args[..] else {
        return Some(0);
    };
    if target != GL_TEXTURE_2D {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    let unpack_state = c.unpack;
    let t = bound(c).clone();
    let Some(&id) = t.levels.get(level as usize) else {
        error(cpu, GL_INVALID_OPERATION);
        return Some(0);
    };
    let inside = ddraw::state(cpu)
        .surface(id)
        .is_some_and(|s| fits(x, width, s.width) && fits(y, height, s.height));
    if !inside {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    let src = Source {
        width,
        height,
        format,
        kind,
        at,
    };
    let Some(texels) = unpack(cpu, unpack_state, &src) else {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    };
    let base = base_of(t.internal).unwrap_or(if t.alpha { Base::Rgba } else { Base::Rgb });
    let words: Vec<u16> = texels.into_iter().map(|p| encode(p, base)).collect();
    write_level(cpu, id, x, y, width, height, &words);
    Some(0)
}

/// The pixels of the color buffer from (`x`, `y`), from the bottom
/// left, `w` by `h`, as red, green, blue and alpha, from the bottom row
/// up.
pub fn read_frame(cpu: &mut Cpu, x: i32, y: i32, w: u32, h: u32) -> Option<Vec<[u8; 4]>> {
    let (target, height) = ctx(cpu).map(|c| (c.target, c.height))?;
    super::super::ddraw::d3d::shadow::expose(cpu, target);
    let s = ddraw::state(cpu).surface(target)?.clone();
    let mut out = Vec::with_capacity(w as usize * h as usize);
    let mut row = vec![0u8; s.pitch as usize];
    for r in 0..i64::from(h) {
        let sy = i64::from(height) - 1 - (i64::from(y) + r);
        if sy < 0 || sy >= i64::from(s.height) {
            out.extend(std::iter::repeat_n([0, 0, 0, 255], w as usize));
            continue;
        }
        dd_surface::read_bytes(cpu, &s, sy as u32 * s.pitch, &mut row);
        for i in 0..i64::from(w) {
            let sx = i64::from(x) + i;
            if sx < 0 || sx >= i64::from(s.width) {
                out.push([0, 0, 0, 255]);
                continue;
            }
            let p = u16::from_le_bytes([row[sx as usize * 2], row[sx as usize * 2 + 1]]);
            out.push(rgb565(p));
        }
    }
    Some(out)
}

/// A 5-6-5 pixel as red, green, blue and alpha bytes.
pub fn rgb565(p: u16) -> [u8; 4] {
    let r = (p >> 11) as u8;
    let g = (p >> 5 & 0x3F) as u8;
    let b = (p & 0x1F) as u8;
    [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2, 255]
}

/// glCopyTexImage2D(target, level, internal, x, y, width, height,
/// border).
pub fn copy_tex_image_2d(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [target, level, internal, x, y, width, height, border] = args[..] else {
        return Some(0);
    };
    if target != GL_TEXTURE_2D {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    // No texture is bigger than the card's: a larger size isn't read.
    if width > MAX_TEXTURE_SIZE || height > MAX_TEXTURE_SIZE {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    let Some(texels) = read_frame(cpu, x as i32, y as i32, width, height) else {
        return Some(0);
    };
    let img = Image {
        level,
        internal,
        width,
        height,
        border,
        format: GL_RGBA,
        kind: GL_UNSIGNED_BYTE,
        at: 0,
    };
    define(cpu, &img, Some(texels));
    Some(0)
}

/// glCopyTexSubImage2D(target, level, xoffset, yoffset, x, y, width,
/// height).
pub fn copy_tex_sub_image_2d(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [target, level, xo, yo, x, y, width, height] = args[..] else {
        return Some(0);
    };
    if target != GL_TEXTURE_2D {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if width > MAX_TEXTURE_SIZE || height > MAX_TEXTURE_SIZE {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    let Some(texels) = read_frame(cpu, x as i32, y as i32, width, height) else {
        return Some(0);
    };
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    let t = bound(c).clone();
    let Some(&id) = t.levels.get(level as usize) else {
        error(cpu, GL_INVALID_OPERATION);
        return Some(0);
    };
    let base = base_of(t.internal).unwrap_or(Base::Rgb);
    let words: Vec<u16> = texels.into_iter().map(|p| encode(p, base)).collect();
    write_level(cpu, id, xo, yo, width, height, &words);
    Some(0)
}

/// Set the bound texture's parameter `p` to `v`.
fn parameter(cpu: &mut Cpu, target: u32, p: u32, v: f32) {
    if target != GL_TEXTURE_2D {
        error(cpu, GL_INVALID_ENUM);
        return;
    }
    let Some(c) = ctx(cpu) else {
        return;
    };
    let t = bound(c);
    let e = v as u32;
    let ok = match p {
        GL_TEXTURE_MIN_FILTER => {
            let ok = e == GL_NEAREST
                || e == GL_LINEAR
                || (GL_NEAREST_MIPMAP_NEAREST..=GL_LINEAR_MIPMAP_LINEAR).contains(&e);
            if ok {
                t.min_filter = e;
            }
            ok
        }
        GL_TEXTURE_MAG_FILTER => {
            let ok = e == GL_NEAREST || e == GL_LINEAR;
            if ok {
                t.mag_filter = e;
            }
            ok
        }
        GL_TEXTURE_WRAP_S | GL_TEXTURE_WRAP_T => {
            let ok = [GL_CLAMP, GL_REPEAT, GL_CLAMP_TO_EDGE].contains(&e);
            let e = if e == GL_CLAMP_TO_EDGE { GL_CLAMP } else { e };
            if ok && p == GL_TEXTURE_WRAP_S {
                t.wrap_s = e;
            } else if ok {
                t.wrap_t = e;
            }
            ok
        }
        GL_TEXTURE_BORDER_COLOR | GL_TEXTURE_PRIORITY => true,
        // GL_TEXTURE_MIN_LOD and the rest of OpenGL 1.2, and the
        // anisotropy of later cards.
        0x813A..=0x813D | 0x84FE => true,
        _ => false,
    };
    if !ok {
        error(cpu, GL_INVALID_ENUM);
    }
}

/// glTexParameterf(target, pname, param).
pub fn tex_parameterf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    parameter(cpu, args[0], args[1], f(args, 2));
    Some(0)
}

/// glTexParameteri(target, pname, param).
pub fn tex_parameteri(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    parameter(cpu, args[0], args[1], args[2] as i32 as f32);
    Some(0)
}

/// glTexParameterfv(target, pname, params).
pub fn tex_parameterfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(v) = read_floats(cpu, args[2], 1) {
        parameter(cpu, args[0], args[1], v[0]);
    }
    Some(0)
}

/// glTexParameteriv(target, pname, params).
pub fn tex_parameteriv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[2] != 0 && cpu.bus.guest_probe(args[2], 4, false) {
        let v = cpu.bus.guest_read_32(args[2]) as i32;
        parameter(cpu, args[0], args[1], v as f32);
    }
    Some(0)
}

/// The bound texture's parameter `p`.
fn get_parameter(cpu: &mut Cpu, p: u32) -> Option<f32> {
    let c = ctx(cpu)?;
    let t = bound(c);
    let v = match p {
        GL_TEXTURE_MIN_FILTER => t.min_filter,
        GL_TEXTURE_MAG_FILTER => t.mag_filter,
        GL_TEXTURE_WRAP_S => t.wrap_s,
        GL_TEXTURE_WRAP_T => t.wrap_t,
        GL_TEXTURE_PRIORITY => return Some(1.0),
        // GL_TEXTURE_RESIDENT.
        0x8067 => 1,
        _ => {
            error(cpu, GL_INVALID_ENUM);
            return None;
        }
    };
    Some(v as f32)
}

/// glGetTexParameterfv(target, pname, params).
pub fn get_tex_parameterfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(v) = get_parameter(cpu, args[1])
        && cpu.bus.guest_probe(args[2], 4, true)
    {
        cpu.bus.guest_write_32(args[2], v.to_bits());
    }
    Some(0)
}

/// glGetTexParameteriv(target, pname, params).
pub fn get_tex_parameteriv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(v) = get_parameter(cpu, args[1])
        && cpu.bus.guest_probe(args[2], 4, true)
    {
        cpu.bus.guest_write_32(args[2], v as i32 as u32);
    }
    Some(0)
}

/// The bound texture's level parameter `p` of `level`.
fn level_parameter(cpu: &mut Cpu, level: u32, p: u32) -> Option<f32> {
    let c = ctx(cpu)?;
    let t = bound(c).clone();
    let defined = t.defined & 1 << level.min(31) != 0;
    let (w, h) = if defined {
        ((t.width >> level).max(1), (t.height >> level).max(1))
    } else {
        (0, 0)
    };
    let bits = |alpha: u32, color: u32| {
        if !defined {
            0
        } else if t.alpha {
            alpha
        } else {
            color
        }
    };
    Some(match p {
        // GL_TEXTURE_WIDTH, GL_TEXTURE_HEIGHT, GL_TEXTURE_COMPONENTS,
        // GL_TEXTURE_BORDER.
        0x1000 => w as f32,
        0x1001 => h as f32,
        0x1003 => {
            if defined {
                t.internal as f32
            } else {
                1.0
            }
        }
        0x1005 => 0.0,
        // GL_TEXTURE_RED_SIZE, GREEN, BLUE, ALPHA, LUMINANCE, INTENSITY.
        0x805C | 0x805E => bits(4, 5) as f32,
        0x805D => bits(4, 6) as f32,
        0x805F => bits(4, 0) as f32,
        0x8060 | 0x8061 => 0.0,
        _ => {
            error(cpu, GL_INVALID_ENUM);
            return None;
        }
    })
}

/// glGetTexLevelParameteriv(target, level, pname, params).
pub fn get_tex_level_parameteriv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(v) = level_parameter(cpu, args[1], args[2])
        && cpu.bus.guest_probe(args[3], 4, true)
    {
        cpu.bus.guest_write_32(args[3], v as i32 as u32);
    }
    Some(0)
}

/// glGetTexLevelParameterfv(target, level, pname, params).
pub fn get_tex_level_parameterfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(v) = level_parameter(cpu, args[1], args[2])
        && cpu.bus.guest_probe(args[3], 4, true)
    {
        cpu.bus.guest_write_32(args[3], v.to_bits());
    }
    Some(0)
}

/// Set the active unit's environment parameter `p` to `v`.
fn env(cpu: &mut Cpu, target: u32, p: u32, v: &[f32]) {
    if target != GL_TEXTURE_ENV {
        error(cpu, GL_INVALID_ENUM);
        return;
    }
    let Some(c) = ctx(cpu) else {
        return;
    };
    let unit = c.active_unit as usize;
    match p {
        GL_TEXTURE_ENV_MODE => {
            let mode = v.first().copied().unwrap_or(0.0) as u32;
            if [GL_MODULATE, GL_DECAL, GL_BLEND, GL_REPLACE, GL_ADD].contains(&mode) {
                c.env_mode[unit] = mode;
            } else {
                error(cpu, GL_INVALID_ENUM);
            }
        }
        GL_TEXTURE_ENV_COLOR if v.len() >= 4 => {
            c.env_color[unit] = [v[0], v[1], v[2], v[3]].map(|x| x.clamp(0.0, 1.0));
        }
        _ => error(cpu, GL_INVALID_ENUM),
    }
}

/// glTexEnvf(target, pname, param).
pub fn tex_envf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    env(cpu, args[0], args[1], &[f(args, 2)]);
    Some(0)
}

/// glTexEnvi(target, pname, param).
pub fn tex_envi(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    env(cpu, args[0], args[1], &[args[2] as i32 as f32]);
    Some(0)
}

/// glTexEnvfv(target, pname, params).
pub fn tex_envfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let n = if args[1] == GL_TEXTURE_ENV_COLOR {
        4
    } else {
        1
    };
    if let Some(v) = read_floats(cpu, args[2], n) {
        env(cpu, args[0], args[1], &v);
    }
    Some(0)
}

/// glTexEnviv(target, pname, params): a color's integers from 0 to the
/// largest as 0 to 1.
pub fn tex_enviv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let color = args[1] == GL_TEXTURE_ENV_COLOR;
    let n = if color { 4 } else { 1 };
    let at = args[2];
    if at == 0 || !cpu.bus.guest_probe(at, n * 4, false) {
        return Some(0);
    }
    let v: Vec<f32> = (0..n as u32)
        .map(|i| {
            let x = cpu.bus.guest_read_32(at + 4 * i) as i32;
            if color {
                x as f32 / i32::MAX as f32
            } else {
                x as f32
            }
        })
        .collect();
    env(cpu, args[0], args[1], &v);
    Some(0)
}

/// The active unit's environment parameter `p`.
fn get_env(cpu: &mut Cpu, p: u32) -> Option<Vec<f32>> {
    let c = ctx(cpu)?;
    let unit = c.active_unit as usize;
    match p {
        GL_TEXTURE_ENV_MODE => Some(vec![c.env_mode[unit] as f32]),
        GL_TEXTURE_ENV_COLOR => Some(c.env_color[unit].to_vec()),
        _ => {
            error(cpu, GL_INVALID_ENUM);
            None
        }
    }
}

/// glGetTexEnvfv(target, pname, params).
pub fn get_tex_envfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(v) = get_env(cpu, args[1])
        && cpu.bus.guest_probe(args[2], v.len() * 4, true)
    {
        for (i, x) in v.iter().enumerate() {
            cpu.bus.guest_write_32(args[2] + 4 * i as u32, x.to_bits());
        }
    }
    Some(0)
}

/// glGetTexEnviv(target, pname, params).
pub fn get_tex_enviv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let color = args[1] == GL_TEXTURE_ENV_COLOR;
    if let Some(v) = get_env(cpu, args[1])
        && cpu.bus.guest_probe(args[2], v.len() * 4, true)
    {
        for (i, x) in v.iter().enumerate() {
            let n = if color {
                (*x as f64 * i32::MAX as f64) as i32
            } else {
                *x as i32
            };
            cpu.bus.guest_write_32(args[2] + 4 * i as u32, n as u32);
        }
    }
    Some(0)
}

/// glAreTexturesResident(n, textures, residences): all of them are.
pub fn are_textures_resident(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

/// glPrioritizeTextures(n, textures, priorities).
pub fn prioritize_textures(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// The unit of GL_TEXTUREn_ARB `e`, if it is one there is.
fn unit_of(e: u32) -> Option<u32> {
    let u = e.checked_sub(GL_TEXTURE0)?;
    ((u as usize) < UNITS).then_some(u)
}

/// glActiveTextureARB(texture).
pub fn active_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match unit_of(args[0]) {
        Some(u) => {
            if let Some(c) = ctx(cpu) {
                c.active_unit = u;
            }
        }
        None => error(cpu, GL_INVALID_ENUM),
    }
    Some(0)
}

/// glClientActiveTextureARB(texture).
pub fn client_active_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match unit_of(args[0]) {
        Some(u) => {
            if let Some(c) = ctx(cpu) {
                c.client_unit = u;
            }
        }
        None => error(cpu, GL_INVALID_ENUM),
    }
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texels_keep_the_components_of_their_format() {
        assert_eq!(encode([255, 0, 0, 0], Base::Rgb), 0xF800);
        assert_eq!(encode([0, 0, 0, 255], Base::Rgba), 0xF000);
        // Luminance from red, and alpha textures white.
        assert_eq!(encode([255, 0, 0, 255], Base::Luminance), 0xFFFF);
        assert_eq!(encode([0, 0, 0, 0x80], Base::Alpha), 0x8FFF);
    }

    #[test]
    fn levels_go_down_to_one_texel() {
        assert_eq!(level_count(256, 256), 9);
        assert_eq!(level_count(64, 8), 7);
        assert_eq!(level_count(1, 1), 1);
    }

    #[test]
    fn rgb565_widens_to_full_bytes() {
        assert_eq!(rgb565(0xFFFF), [255, 255, 255, 255]);
        assert_eq!(rgb565(0xF800), [255, 0, 0, 255]);
    }
}

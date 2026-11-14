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
    GL_INVALID_ENUM, GL_INVALID_OPERATION, GL_INVALID_VALUE, GL_OUT_OF_MEMORY, Settings, State,
    Texture, UNITS, ctx, current, error, f, lists, max_texture_size, read_floats, state,
};
use super::wgl::PackedPixels;
use crate::cpu::Cpu;

pub const GL_TEXTURE0: u32 = 0x84C0;
pub const GL_TEXTURE_2D: u32 = 0x0DE1;

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

/// How a pixel of `format` and `kind` is kept: its bytes, the bytes of
/// one of its elements, and the packed type (`packed_type`) when `kind`
/// is one the driver takes (`types`). The error is OpenGL's:
/// GL_INVALID_ENUM for a format or type there isn't, and
/// GL_INVALID_OPERATION for a packed type whose format has another
/// number of components (the three-component types take GL_RGB alone).
pub(super) fn pixel_layout(
    format: u32,
    kind: u32,
    types: PackedPixels,
) -> Result<(u32, u32, Option<Packed>), u32> {
    let n = components(format).ok_or(GL_INVALID_ENUM)?;
    match packed_type(kind).filter(|_| types.takes(kind)) {
        Some(packed) => {
            let fits = match packed.1.len() {
                3 => format == GL_RGB,
                _ => format == GL_RGBA || format == GL_BGRA_EXT,
            };
            if !fits {
                return Err(GL_INVALID_OPERATION);
            }
            Ok((packed.0, packed.0, Some(packed)))
        }
        None => {
            let size = type_size(kind).ok_or(GL_INVALID_ENUM)?;
            Ok((n * size, size, None))
        }
    }
}

/// The most bytes an image the program gives may take.
pub(super) const IMAGE_MOST: usize = 0x400_0000;

/// Where an image of `width` by `height` pixels of `pixel` bytes is
/// under the unpack state `unpack` (alignment, row length, rows and
/// pixels skipped): the bytes from one row to the next, the bytes
/// from the address the program gave to the first pixel, and the
/// bytes from there to the end of the last. None when they don't fit
/// the address space. glTexImage2D reads its texels by it, and a
/// display list copies them by it (`lists`).
pub(super) fn span(
    unpack: [u32; 4],
    width: u32,
    height: u32,
    pixel: u32,
) -> Option<(u32, u32, usize)> {
    let row_pixels = if unpack[1] > 0 { unpack[1] } else { width };
    let align = unpack[0].max(1);
    let stride = row_pixels
        .checked_mul(pixel)?
        .checked_next_multiple_of(align)?;
    let skip = unpack[2]
        .checked_mul(stride)?
        .checked_add(unpack[3].checked_mul(pixel)?)?;
    let len = height
        .saturating_sub(1)
        .checked_mul(stride)?
        .checked_add(width.checked_mul(pixel)?)? as usize;
    Some((stride, skip, len))
}

/// The texels of `src` as red, green, blue and alpha bytes, row by row
/// from the first the program gave, with the unpack state of `c`
/// (alignment, row length, rows and pixels skipped). The error is
/// OpenGL's: `pixel_layout`'s for the format and type, and
/// GL_INVALID_ENUM for texels that can't be read.
fn unpack(cpu: &mut Cpu, unpack: [u32; 4], src: &Source) -> Result<Vec<[u8; 4]>, u32> {
    let types = super::wgl::packed_pixels(cpu);
    let (pixel, size, packed) = pixel_layout(src.format, src.kind, types)?;
    let place = || {
        let (stride, skip, len) = span(unpack, src.width, src.height, pixel)?;
        Some((stride, src.at.checked_add(skip)?, len))
    };
    let (stride, start, len) = place().ok_or(GL_INVALID_ENUM)?;
    if src.at == 0 || len > IMAGE_MOST || !cpu.bus.guest_probe(start, len, false) {
        return Err(GL_INVALID_ENUM);
    }
    let mut bytes = vec![0u8; len];
    cpu.bus.guest_read_bytes(start, &mut bytes);
    let mut out = Vec::with_capacity((src.width * src.height) as usize);
    for y in 0..src.height {
        let row = &bytes[(y * stride) as usize..];
        for x in 0..src.width {
            let p = &row[(x * pixel) as usize..];
            out.push(match packed {
                Some((_, widths, rev)) => {
                    let mut v = [0u8; 4];
                    v[..size as usize].copy_from_slice(&p[..size as usize]);
                    let v = u32::from_le_bytes(v);
                    rgba_of(src.format, |i| packed_component(v, widths, rev, i))
                }
                None => rgba_of(src.format, |i| {
                    let at = (i * size) as usize;
                    (component(&p[at..at + size as usize], src.kind) * 255.0 + 0.5) as u8
                }),
            });
        }
    }
    Ok(out)
}

/// A packed pixel type: the bytes of its element, the bits of each
/// component and whether the first is lowest (`packed_type`).
pub(super) type Packed = (u32, &'static [u32], bool);

/// A packed pixel type of OpenGL 1.2 (GL_UNSIGNED_SHORT_5_6_5 and the
/// others): the bytes of the one element that holds a whole pixel, the
/// bits of each component in the order the format names them, and
/// whether the first component is in the lowest bits (the types ending
/// in _REV) or in the highest.
fn packed_type(kind: u32) -> Option<Packed> {
    Some(match kind {
        0x8032 => (1, &[3, 3, 2], false),
        0x8362 => (1, &[3, 3, 2], true),
        0x8363 => (2, &[5, 6, 5], false),
        0x8364 => (2, &[5, 6, 5], true),
        0x8033 => (2, &[4, 4, 4, 4], false),
        0x8365 => (2, &[4, 4, 4, 4], true),
        0x8034 => (2, &[5, 5, 5, 1], false),
        0x8366 => (2, &[5, 5, 5, 1], true),
        0x8035 => (4, &[8, 8, 8, 8], false),
        0x8367 => (4, &[8, 8, 8, 8], true),
        0x8036 => (4, &[10, 10, 10, 2], false),
        0x8368 => (4, &[10, 10, 10, 2], true),
        _ => return None,
    })
}

/// The packed pixel of `packed` that holds `rgba` with the components
/// in the order `format` names them (GL_RGB, GL_RGBA or GL_BGRA), as
/// glReadPixels writes it: each byte scaled to its bits, rounded.
pub(super) fn pack_pixel(rgba: [u8; 4], format: u32, packed: Packed) -> u32 {
    let (_, widths, rev) = packed;
    let [r, g, b, a] = rgba;
    let ordered = if format == GL_BGRA_EXT {
        [b, g, r, a]
    } else {
        [r, g, b, a]
    };
    let mut v = 0;
    for (i, &bits) in widths.iter().enumerate() {
        let shift: u32 = if rev {
            widths[..i].iter().sum()
        } else {
            widths[i + 1..].iter().sum()
        };
        let max = (1u32 << bits) - 1;
        v |= ((ordered[i] as u32 * max + 127) / 255) << shift;
    }
    v
}

/// Component `i` of the packed pixel `v`, as a byte.
fn packed_component(v: u32, widths: &[u32], rev: bool, i: u32) -> u8 {
    let i = i as usize;
    let shift: u32 = if rev {
        widths[..i].iter().sum()
    } else {
        widths[i + 1..].iter().sum()
    };
    let max = (1u32 << widths[i]) - 1;
    ((((v >> shift) & max) * 255 + max / 2) / max) as u8
}

/// A texel of `format` as red, green, blue and alpha, from its
/// components as bytes, `c(i)` the `i`th.
pub fn rgba_of(format: u32, c: impl Fn(u32) -> u8) -> [u8; 4] {
    match format {
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
    }
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

// Where the textures are. The one named 0, the default texture, is a
// context's own (`Settings::default_texture`). The named ones belong
// to the context's group, with its display lists (`lists::Shared`): a
// context starts with a group of its own, and after wglShareLists two
// contexts have the same textures under the same names, as contexts
// of OpenGL 1.1 that share their lists do. A texture's surfaces are of
// the DirectDraw object of the context that gave it its image.

/// A texture named `name` as a new one is: no image, and OpenGL's
/// filters and wrap modes.
pub(super) fn new(name: u32) -> Texture {
    Texture {
        name,
        min_filter: GL_NEAREST_MIPMAP_LINEAR,
        mag_filter: GL_LINEAR,
        wrap_s: GL_REPEAT,
        wrap_t: GL_REPEAT,
        ..Texture::default()
    }
}

const GL_NEAREST_MIPMAP_LINEAR: u32 = 0x2702;

/// The texture named `name` of a context with the settings `c` whose
/// group's textures are `shared`, as it is now.
pub fn find<'a>(c: &'a Settings, shared: &'a [Texture], name: u32) -> Option<&'a Texture> {
    if name == 0 {
        return Some(&c.default_texture);
    }
    shared.iter().find(|t| t.name == name)
}

/// Whether the context at `index` has a texture named `name`.
fn exists(s: &State, index: usize, name: u32) -> bool {
    let Some(Some(c)) = s.contexts.get(index) else {
        return false;
    };
    find(c, s.lists.textures(c.lists.group), name).is_some()
}

/// The texture named `name` of the context at `index`, to change, made
/// if it isn't there (names glGenTextures gave or not). Every context
/// that has the texture draws with it as it is after the change.
fn texture_mut(s: &mut State, index: usize, name: u32) -> Option<&mut Texture> {
    if name == 0 {
        return Some(&mut s.contexts.get_mut(index)?.as_mut()?.default_texture);
    }
    let textures = lists::textures_mut(s, index)?;
    let at = match textures.iter().position(|t| t.name == name) {
        Some(at) => at,
        None => {
            textures.push(new(name));
            textures.len() - 1
        }
    };
    textures.get_mut(at)
}

/// The texture bound to the active unit of the current context.
fn bound(cpu: &mut Cpu) -> Option<&mut Texture> {
    let (index, _) = current(cpu)?;
    let s = state(cpu);
    let c = s.contexts.get(index)?.as_ref()?;
    let name = c.bound[c.active_unit as usize];
    texture_mut(s, index, name)
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
    let Some((index, _)) = current(cpu) else {
        return Some(0);
    };
    let Some(textures) = lists::textures_mut(state(cpu), index) else {
        return Some(0);
    };
    let mut names = Vec::with_capacity(n as usize);
    let mut next = textures.iter().map(|t| t.name).max().unwrap_or(0);
    for _ in 0..n {
        next += 1;
        names.push(next);
        textures.push(new(next));
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
/// default texture again. The textures are gone for every context that
/// had them; a unit of another context that one was bound to draws
/// untextured until it binds a texture again.
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
    if let Some((index, _)) = current(cpu) {
        let s = state(cpu);
        let named = |name: u32| name != 0 && names.contains(&name);
        if let Some(textures) = lists::textures_mut(s, index) {
            gone.extend(textures.extract_if(.., |t| named(t.name)));
        }
        if let Some(Some(c)) = s.contexts.get_mut(index) {
            for b in c.bound.iter_mut().filter(|b| named(**b)) {
                *b = 0;
            }
        }
    }
    release(cpu, &gone);
    Some(0)
}

/// Let go of the surfaces of `textures`, which no context has any
/// more.
pub fn release(cpu: &mut Cpu, textures: &[Texture]) {
    for t in textures {
        release_surface(cpu, t);
    }
}

/// Let go of the default texture of the context at `index`.
pub fn release_default(cpu: &mut Cpu, index: usize) {
    let texture = match state(cpu).contexts.get_mut(index) {
        Some(Some(c)) => std::mem::replace(&mut c.default_texture, new(0)),
        _ => return,
    };
    release_surface(cpu, &texture);
}

/// glBindTexture(target, texture).
pub fn bind_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] != GL_TEXTURE_2D {
        if args[0] != 0x0DE0 {
            error(cpu, GL_INVALID_ENUM);
        }
        return Some(0);
    }
    let Some((index, _)) = current(cpu) else {
        return Some(0);
    };
    let s = state(cpu);
    if let Some(Some(c)) = s.contexts.get_mut(index) {
        let unit = c.active_unit as usize;
        c.bound[unit] = args[1];
    }
    // A name that has no texture yet gets one. One that has is left
    // as it is, and so are the contexts that share it.
    if !exists(s, index, args[1]) {
        texture_mut(s, index, args[1]);
    }
    Some(0)
}

/// glIsTexture(texture).
pub fn is_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((index, _)) = current(cpu) else {
        return Some(0);
    };
    Some((args[0] != 0 && exists(state(cpu), index, args[0])) as u32)
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
    // On the Voodoo2 they are in system memory, and the HAL copies them
    // into the texture units' memory as it draws with them.
    let memory = if !directdraw::is_card(cpu, dd) && video_room(cpu, size) {
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
    // The surface holds its DirectDraw object, as one IDirectDraw7's
    // CreateSurface gave does: a texture another context shares keeps
    // its texels when the context that made it is deleted.
    let creator = ddraw::state(cpu).object(ids[0]).map_or(0, |o| o.creator);
    if creator != 0
        && ddraw::add_ref(cpu, creator) != 0
        && let Some(o) = ddraw::state(cpu).object_mut(ids[0])
    {
        o.holds_creator = true;
    }
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
    // No texture is bigger than the card's largest, which has the most
    // levels.
    let max = max_texture_size(cpu);
    if img.border != 0 || !pow2 || w > max || h > max || level >= level_count(max, max) {
        error(cpu, GL_INVALID_VALUE);
        return;
    }
    // The format and type are checked with no texels (a NULL pointer)
    // too.
    if let Err(e) = pixel_layout(img.format, img.kind, super::wgl::packed_pixels(cpu)) {
        error(cpu, e);
        return;
    }
    let alpha = has_alpha(base);
    let Some(dd) = ctx(cpu).map(|c| c.dd) else {
        return;
    };
    let Some(t) = bound(cpu).cloned() else {
        return;
    };
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
                Ok(t) => Some(t),
                Err(e) => {
                    error(cpu, e);
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
    if let Some(bound) = bound(cpu) {
        *bound = t;
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

/// glTexImage2D of the level `level` of the bound 2D texture, with
/// texels already red, green, blue and alpha bytes, `width` by
/// `height` from the first row the program gave: GLU32's
/// gluBuild2DMipmaps gives its levels this way.
pub fn tex_image_rgba(
    cpu: &mut Cpu,
    level: u32,
    internal: u32,
    width: u32,
    height: u32,
    texels: Vec<[u8; 4]>,
) {
    if ctx(cpu).is_none() {
        return;
    }
    let img = Image {
        level,
        internal,
        width,
        height,
        border: 0,
        format: GL_RGBA,
        kind: GL_UNSIGNED_BYTE,
        at: 0,
    };
    define(cpu, &img, Some(texels));
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
    let Some(t) = bound(cpu).cloned() else {
        return Some(0);
    };
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
    let texels = match unpack(cpu, unpack_state, &src) {
        Ok(t) => t,
        Err(e) => {
            error(cpu, e);
            return Some(0);
        }
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
    // 5-6-5, or X8R8G8B8 (`wgl::color_bits`).
    let bytes = s.bytes_per_pixel() as usize;
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
            let at = sx as usize * bytes;
            out.push(match bytes {
                4 => [row[at + 2], row[at + 1], row[at], 255],
                _ => rgb565(u16::from_le_bytes([row[at], row[at + 1]])),
            });
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
    if width.max(height) > max_texture_size(cpu) {
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
    if width.max(height) > max_texture_size(cpu) {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    let Some(texels) = read_frame(cpu, x as i32, y as i32, width, height) else {
        return Some(0);
    };
    let Some(t) = bound(cpu).cloned() else {
        return Some(0);
    };
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
    let Some(t) = bound(cpu) else {
        return;
    };
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

/// glPopAttrib's textures: each of `params` is a texture's name and
/// the filters and wrap modes it had at the push, which the texture
/// of that name has again if it is still there.
pub fn restore_params(cpu: &mut Cpu, params: &[[u32; 5]]) {
    let Some((index, _)) = current(cpu) else {
        return;
    };
    let s = state(cpu);
    for &[name, min, mag, wrap_s, wrap_t] in params {
        if !exists(s, index, name) {
            continue;
        }
        if let Some(t) = texture_mut(s, index, name) {
            t.min_filter = min;
            t.mag_filter = mag;
            t.wrap_s = wrap_s;
            t.wrap_t = wrap_t;
        }
    }
}

/// The bound texture's parameter `p`.
fn get_parameter(cpu: &mut Cpu, p: u32) -> Option<f32> {
    let t = bound(cpu)?;
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
    let t = bound(cpu)?.clone();
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
    let n = super::pname::values(args[1]);
    if let Some(v) = read_floats(cpu, args[2], n) {
        env(cpu, args[0], args[1], &v);
    }
    Some(0)
}

/// glTexEnviv(target, pname, params): a color's integers from 0 to the
/// largest as 0 to 1.
pub fn tex_enviv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let color = args[1] == GL_TEXTURE_ENV_COLOR;
    let n = super::pname::values(args[1]);
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

    /// OpenGL 1.2's table of packed types: the first component is in
    /// the highest bits, or in the lowest for a type ending in _REV.
    #[test]
    fn packed_pixels_hold_their_components_in_the_types_bit_order() {
        let texel = |kind: u32, format: u32, v: u32| {
            let (_, widths, rev) = packed_type(kind).unwrap();
            rgba_of(format, |i| packed_component(v, widths, rev, i))
        };
        // GL_UNSIGNED_SHORT_5_6_5 and its _REV.
        assert_eq!(texel(0x8363, GL_RGB, 0xF800), [255, 0, 0, 255]);
        assert_eq!(texel(0x8364, GL_RGB, 0xF800), [0, 0, 255, 255]);
        // GL_UNSIGNED_SHORT_1_5_5_5_REV with GL_BGRA: the 1-5-5-5
        // pixels of a DirectDraw surface. GL_UNSIGNED_SHORT_5_5_5_1
        // has the alpha bit lowest.
        assert_eq!(texel(0x8366, GL_BGRA_EXT, 0xFC00), [255, 0, 0, 255]);
        assert_eq!(texel(0x8034, GL_RGBA, 0xF801), [255, 0, 0, 255]);
        // GL_UNSIGNED_INT_8_8_8_8_REV with GL_BGRA is blue, green, red
        // and alpha bytes in memory, as GL_UNSIGNED_BYTE is; without
        // _REV the bytes come the other way round.
        assert_eq!(
            texel(0x8367, GL_BGRA_EXT, 0x8011_2233),
            [0x11, 0x22, 0x33, 0x80]
        );
        assert_eq!(
            texel(0x8035, GL_BGRA_EXT, 0x3322_1180),
            [0x11, 0x22, 0x33, 0x80]
        );
        assert_eq!(texel(0x8365, GL_RGBA, 0xF00F), [255, 0, 0, 255]);
        assert_eq!(texel(0x8368, GL_RGBA, 0xC000_03FF), [255, 0, 0, 255]);
        assert_eq!(texel(0x8032, GL_RGB, 0xE3), [255, 0, 255, 255]);
        assert_eq!(texel(0x8362, GL_RGB, 0xC7), [255, 0, 255, 255]);
        // GL_UNSIGNED_SHORT_4_4_4_4 and GL_UNSIGNED_INT_10_10_10_2: red
        // highest, alpha lowest.
        assert_eq!(texel(0x8033, GL_RGBA, 0xF00F), [255, 0, 0, 255]);
        assert_eq!(texel(0x8033, GL_BGRA_EXT, 0x0F0F), [0, 255, 0, 255]);
        assert_eq!(texel(0x8036, GL_RGBA, 0xFFC0_0003), [255, 0, 0, 255]);
        assert_eq!(texel(0x8036, GL_RGBA, 0x0000_0FFC), [0, 0, 255, 0]);
        assert_eq!(packed_type(GL_UNSIGNED_BYTE), None);
    }

    /// Each of the twelve packed types gives back, read, what was packed
    /// into it, in each format it takes: the bit order is the same both
    /// ways. Values that fit the narrowest bits (0 and 255) come back
    /// whole.
    #[test]
    fn packing_a_pixel_and_reading_it_back_agree() {
        let kinds = (0x8032..=0x8036).chain(0x8362..=0x8368);
        let mut seen = 0;
        for kind in kinds {
            let packed = packed_type(kind).unwrap();
            let formats: &[u32] = if packed.1.len() == 3 {
                &[GL_RGB]
            } else {
                &[GL_RGBA, GL_BGRA_EXT]
            };
            for &format in formats {
                for rgba in [
                    [255, 0, 0, 255],
                    [0, 255, 0, 0],
                    [0, 0, 255, 255],
                    [255, 255, 0, 0],
                ] {
                    let rgba = if packed.1.len() == 3 {
                        [rgba[0], rgba[1], rgba[2], 255]
                    } else {
                        rgba
                    };
                    let v = pack_pixel(rgba, format, packed);
                    let back = rgba_of(format, |i| packed_component(v, packed.1, packed.2, i));
                    assert_eq!(back, rgba, "{kind:#x} {format:#x}");
                    seen += 1;
                }
            }
        }
        assert_eq!(seen, 4 * (4 + 8 * 2));
        // GL_UNSIGNED_INT_8_8_8_8_REV with GL_BGRA is the bytes blue,
        // green, red, alpha in memory.
        let v = pack_pixel(
            [0x11, 0x22, 0x33, 0x80],
            GL_BGRA_EXT,
            packed_type(0x8367).unwrap(),
        );
        assert_eq!(v.to_le_bytes(), [0x33, 0x22, 0x11, 0x80]);
    }

    /// OpenGL 1.2: a packed type of three components takes GL_RGB, one
    /// of four GL_RGBA or GL_BGRA, and any other format is
    /// GL_INVALID_OPERATION. A driver that doesn't take the type answers
    /// GL_INVALID_ENUM, as for any type it hasn't.
    #[test]
    fn a_packed_type_takes_formats_of_as_many_components() {
        let bytes = |format, kind| {
            pixel_layout(format, kind, PackedPixels::Core12).map(|(pixel, _, _)| pixel)
        };
        let ext = |format, kind| pixel_layout(format, kind, PackedPixels::Extension).is_ok();
        assert!(ext(GL_RGBA, 0x8035));
        assert_eq!(
            pixel_layout(GL_BGRA_EXT, 0x8367, PackedPixels::Extension).err(),
            Some(GL_INVALID_ENUM)
        );
        assert_eq!(
            pixel_layout(GL_RGB, 0x8032, PackedPixels::None).err(),
            Some(GL_INVALID_ENUM)
        );
        assert_eq!(bytes(GL_RGB, 0x8363), Ok(2));
        assert_eq!(bytes(GL_BGRA_EXT, 0x8367), Ok(4));
        assert_eq!(bytes(GL_RGBA, 0x8034), Ok(2));
        assert_eq!(bytes(GL_BGR_EXT, 0x8363), Err(GL_INVALID_OPERATION));
        assert_eq!(bytes(GL_RGBA, 0x8363), Err(GL_INVALID_OPERATION));
        assert_eq!(bytes(GL_RGB, 0x8367), Err(GL_INVALID_OPERATION));
        assert_eq!(bytes(GL_LUMINANCE, 0x8033), Err(GL_INVALID_OPERATION));
        assert_eq!(bytes(GL_RGB, GL_UNSIGNED_SHORT), Ok(6));
        assert_eq!(bytes(0, 0x8367), Err(GL_INVALID_ENUM));
        assert_eq!(bytes(GL_RGB, 0), Err(GL_INVALID_ENUM));
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

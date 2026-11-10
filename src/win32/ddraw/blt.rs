//! Blt and BltFast: copying rectangles of pixels between surfaces of one
//! depth, stretched and mirrored, through color keys, and filling them.
//! A destination with a clipper is drawn on only where its clip list
//! says; BltFast takes no clipper. On the 3dfx device's card, fills
//! are the card's fastfills, and copies go through its linear frame
//! buffer. On a Banshee or Voodoo3, a fill of a 16-bit depth buffer is
//! the 3D engine's fastfill, as the HAL's Clear. While the OpenGL
//! renderer draws a Voodoo3, other blits within its memory are the
//! renderer's (`gpu_blt`). Blits from and to DXT surfaces are
//! `dxt_blt`'s.

use std::borrow::Cow;

use super::dxt_blt::{self, Plan};
use super::surface::{self, KEY_DEST_BLT, KEY_SRC_BLT, Surface, surface_of};
use super::{
    DD_OK, DDERR_BLTFASTCANTCLIP, DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS,
    DDERR_INVALIDPIXELFORMAT, DDERR_INVALIDRECT, DDERR_NOALPHAHW, DDERR_NORASTEROPHW,
    DDERR_NOROTATIONHW, DDERR_NOZBUFFERHW, DDERR_SURFACEBUSY, DDERR_SURFACELOST, DDERR_UNSUPPORTED,
    state,
};
use crate::cpu::Cpu;
use crate::win32::gdi32::{Rect, Region};

const DDBLT_ALPHA: u32 = 0x1FF;
const DDBLT_COLORFILL: u32 = 0x400;
const DDBLT_DDFX: u32 = 0x800;
const DDBLT_DDROPS: u32 = 0x1000;
const DDBLT_KEYDEST: u32 = 0x2000;
const DDBLT_KEYDESTOVERRIDE: u32 = 0x4000;
const DDBLT_KEYSRC: u32 = 0x8000;
const DDBLT_KEYSRCOVERRIDE: u32 = 0x1_0000;
const DDBLT_ROP: u32 = 0x2_0000;
const DDBLT_ROTATIONANGLE: u32 = 0x4_0000;
const DDBLT_ZBUFFER: u32 = 0x8_0000 | 0x10_0000 | 0x20_0000 | 0x40_0000 | 0x80_0000;
const DDBLT_DEPTHFILL: u32 = 0x200_0000;

const DDBLTFX_MIRRORLEFTRIGHT: u32 = 0x2;
const DDBLTFX_MIRRORUPDOWN: u32 = 0x4;
const DDBLTFX_ROTATE: u32 = 0x10 | 0x20 | 0x40;

const DDBLTFAST_SRCCOLORKEY: u32 = 0x1;
const DDBLTFAST_DESTCOLORKEY: u32 = 0x2;

const SRCCOPY: u32 = 0x00CC_0020;
const BLACKNESS: u32 = 0x0000_0042;
const WHITENESS: u32 = 0x00FF_0062;

/// DDBLTFX's size, and where its fields are.
const FX_SIZE: u32 = 100;
const FX_DDFX: u32 = 0x04;
const FX_ROP: u32 = 0x08;
const FX_FILL: u32 = 0x50;
const FX_DEST_KEY: u32 = 0x54;
const FX_SRC_KEY: u32 = 0x5C;

/// What a blit does.
#[derive(Default)]
struct Op {
    dst: Rect,
    /// The source and its rectangle, or a value to fill with.
    src: Option<(u32, Rect)>,
    fill: u32,
    src_key: Option<(u32, u32)>,
    dst_key: Option<(u32, u32)>,
    mirror_x: bool,
    mirror_y: bool,
    /// Where it may draw, in the destination's pixels.
    clip: Option<Region>,
    /// What it does when the source or the destination is DXT.
    dxt: Option<Plan>,
}

/// A color key, with the test of a pixel against it worked out once a
/// blit rather than for each pixel.
#[derive(Clone, Copy)]
pub(super) enum Key {
    /// The pixels whose bits under `mask` are `value`: a key of one
    /// color, the usual one.
    Exact { mask: u32, value: u32 },
    /// The pixels each of whose channels (the masks that aren't 0) is
    /// between the key's low and high values of it.
    Ranges {
        masks: [u32; 3],
        low: u32,
        high: u32,
    },
}

impl Key {
    /// The key `key` of pixels of the surface `s`'s format.
    pub(super) fn new(key: (u32, u32), s: &Surface) -> Key {
        let f = &s.format;
        let masks = [f.r, f.g, f.b];
        let (low, high) = key;
        if f.flags & surface::DDPF_RGB == 0
            || f.flags & surface::DDPF_PALETTEINDEXED8 != 0
            || masks.iter().all(|&m| m == 0)
        {
            // The pixel's value as a whole.
            return if low == high {
                Key::Exact {
                    mask: u32::MAX,
                    value: low,
                }
            } else {
                Key::Ranges {
                    masks: [u32::MAX, 0, 0],
                    low,
                    high,
                }
            };
        }
        if masks.iter().all(|&m| low & m == high & m) {
            let mask = masks.iter().fold(0, |all, &m| all | m);
            return Key::Exact {
                mask,
                value: low & mask,
            };
        }
        Key::Ranges { masks, low, high }
    }

    /// Whether the pixel `value` is in the key.
    pub(super) fn holds(&self, value: u32) -> bool {
        match *self {
            Key::Exact { mask, value: key } => value & mask == key,
            Key::Ranges { masks, low, high } => masks
                .iter()
                .filter(|&&m| m != 0)
                .all(|&m| (low & m..=high & m).contains(&(value & m))),
        }
    }
}

fn get(data: &[u8], at: usize, bytes: usize) -> u32 {
    let mut v = [0u8; 4];
    v[..bytes].copy_from_slice(&data[at..at + bytes]);
    u32::from_le_bytes(v)
}

fn put(data: &mut [u8], at: usize, bytes: usize, value: u32) {
    data[at..at + bytes].copy_from_slice(&value.to_le_bytes()[..bytes]);
}

/// A pixel of `N` bytes as a number.
fn pixel<const N: usize>(p: &[u8; N]) -> u32 {
    let mut v = [0u8; 4];
    v[..N].copy_from_slice(p);
    u32::from_le_bytes(v)
}

/// The pixels of `N` bytes of the row `src` copied over those of `dst`,
/// a row as long, but for the ones in the source key and the ones over
/// pixels not in the destination key.
fn merge_row<const N: usize>(
    dst: &mut [u8],
    src: &[u8],
    src_key: Option<Key>,
    dst_key: Option<Key>,
) {
    let dst = dst.as_chunks_mut::<N>().0;
    let src = src.as_chunks::<N>().0;
    for (d, s) in dst.iter_mut().zip(src) {
        if src_key.is_some_and(|k| k.holds(pixel(s))) || dst_key.is_some_and(|k| !k.holds(pixel(d)))
        {
            continue;
        }
        *d = *s;
    }
}

/// `merge_row` for pixels of `bytes` bytes.
fn merge(bytes: usize, dst: &mut [u8], src: &[u8], src_key: Option<Key>, dst_key: Option<Key>) {
    match bytes {
        1 => merge_row::<1>(dst, src, src_key, dst_key),
        2 => merge_row::<2>(dst, src, src_key, dst_key),
        3 => merge_row::<3>(dst, src, src_key, dst_key),
        _ => merge_row::<4>(dst, src, src_key, dst_key),
    }
}

/// Where the pixel at (`x`, `y`) of a surface is in its memory.
fn offset(s: &Surface, x: i32, y: i32) -> u32 {
    y as u32 * s.pitch + x as u32 * s.bytes_per_pixel()
}

/// The bytes of a row of the rectangle `r` of a surface.
fn row_len(s: &Surface, r: &Rect) -> usize {
    r.width().max(0) as usize * s.bytes_per_pixel() as usize
}

/// The pixels of the rectangle `r` of a surface, `r.width()` a row: only
/// its columns are read, which on a card is what takes the time.
fn load(cpu: &mut Cpu, s: &Surface, r: &Rect) -> Vec<u8> {
    let len = row_len(s, r);
    let mut data = vec![0u8; len * r.height().max(0) as usize];
    surface::read_rows(cpu, s, offset(s, r.left, r.top), len, &mut data);
    data
}

/// Pixels as `load` gives them stored as the rectangle `r` of a
/// surface.
fn store(cpu: &mut Cpu, s: &Surface, r: &Rect, data: &[u8]) {
    let len = row_len(s, r);
    surface::write_rows(cpu, s, offset(s, r.left, r.top), len, data);
}

/// A fill of a 16-bit depth buffer in a Banshee's or Voodoo3's memory,
/// done as the 3D engine's fastfill of the depth, as the HAL's Clear
/// does it (`hal::card_fill`): true if it was. The fill is usually
/// DDBLT_DEPTHFILL's, but a color fill or a BLACKNESS or WHITENESS
/// raster operation of a depth buffer writes its value the same way, so
/// they come here too. The card fills the memory with its own engine,
/// and so does the emulated one, in the order of the 3D drawing queued
/// before it. Written by the CPU instead, the rows of a depth buffer the
/// OpenGL renderer draws are read back from the GPU first, and stay hot:
/// every triangle that tests depth there is then drawn in software until
/// they cool, which a game that fills its depth buffer this way every
/// frame (Omikron) never lets them do.
fn depth_fill(cpu: &mut Cpu, id: u32, d: &Surface, rects: &[Rect], op: &Op) -> bool {
    if d.memory != surface::MEM_VIDEO
        || d.format.flags & surface::DDPF_ZBUFFER == 0
        || d.bytes_per_pixel() != 2
        || op.src.is_some()
        || op.dst_key.is_some()
    {
        return false;
    }
    let rects: Vec<[i32; 4]> = rects.iter().map(super::gpu_blt::corners).collect();
    super::d3d::hal::card_fill(cpu, d, id, &rects, op.fill as u16)
}

/// Do the blit `op` on the surface `dst_id`.
fn run(cpu: &mut Cpu, dst_id: u32, op: &Op) -> Result<(), u32> {
    let Some(d) = state(cpu).surface(dst_id).cloned() else {
        return Err(DDERR_INVALIDOBJECT);
    };
    let bytes = d.bytes_per_pixel() as usize;
    let area = op.dst.intersect(&surface::bounds(&d));
    let rects: Vec<Rect> = match &op.clip {
        Some(clip) => clip.and_rect(&area).rects,
        None => vec![area],
    };
    let rects: Vec<Rect> = rects.into_iter().filter(|r| !r.is_empty()).collect();
    if rects.is_empty() {
        return Ok(());
    }
    match op.dxt {
        Some(Plan::Nothing) => return Ok(()),
        Some(Plan::Copy) => {
            let (src_id, sr) = op.src.ok_or(DDERR_INVALIDPARAMS)?;
            let s = state(cpu).surface(src_id).cloned();
            let s = s.ok_or(DDERR_INVALIDOBJECT)?;
            // Blocks are copied whole: a clipper that cuts the rectangle
            // would need them compressed again. The rectangles a clip
            // leaves don't overlap and lie within the destination's, so
            // they cover it when their areas add up to its area.
            let size = |r: &Rect| r.width() as i64 * r.height() as i64;
            let covered: i64 = rects.iter().map(size).sum();
            if area != op.dst || covered != size(&op.dst) || op.mirror_x || op.mirror_y {
                return Err(DDERR_UNSUPPORTED);
            }
            dxt_blt::copy(cpu, &s, &sr, &d, &op.dst)?;
            surface::changed(cpu, dst_id);
            return Ok(());
        }
        _ => {}
    }
    surface::shown(cpu, dst_id);
    if d.memory == surface::MEM_CARD && op.src.is_none() && op.dst_key.is_none() {
        super::voodoo::fill(cpu, &d, &rects, op.fill);
        surface::changed(cpu, dst_id);
        return Ok(());
    }
    if depth_fill(cpu, dst_id, &d, &rects, op) {
        surface::changed(cpu, dst_id);
        return Ok(());
    }
    // A ToadForce fills and copies in its own memory. The other adapters
    // don't come here, which would copy the source's description for
    // nothing on every blit.
    let plain = op.dxt.is_none() && op.dst_key.is_none() && !op.mirror_x && !op.mirror_y;
    if plain && crate::win32::screen::is_toadforce(cpu) {
        let blit = super::toadforce::Blit {
            dst: &d,
            rects: &rects,
            area: op.dst,
        };
        let done = match op.src {
            None => super::toadforce::fill(cpu, &blit, op.fill),
            Some((src_id, from)) => state(cpu)
                .surface(src_id)
                .cloned()
                .is_some_and(|s| super::toadforce::copy(cpu, &blit, &s, &from, op.src_key)),
        };
        if done {
            surface::changed(cpu, dst_id);
            return Ok(());
        }
    }
    let gpu = super::gpu_blt::Blit {
        dst_id,
        dst: &d,
        area: op.dst,
        rects: &rects,
        src: op.src,
        fill: op.fill,
        src_key: op.src_key,
        dst_key: op.dst_key,
        mirror: [op.mirror_x, op.mirror_y],
    };
    if op.dxt.is_none() && super::gpu_blt::run(cpu, &gpu) {
        // The renderer copied the pixels in its own copy of the card's
        // memory, not through `screen::write`, which counts them.
        let bytes = d.bytes_per_pixel();
        crate::win32::screen::drew_rects(cpu, d.bits, d.pitch, bytes, &rects);
        surface::changed(cpu, dst_id);
        return Ok(());
    }
    let (dw, dh) = (op.dst.width() as i64, op.dst.height() as i64);
    let decode = op.dxt == Some(Plan::Decode);
    // Where the source's pixels start, in the destination's.
    let mut origin = (op.dst.left, op.dst.top);
    // The source's pixels, its rectangle's only. A DXT source's are
    // decoded already stretched and mirrored, in the destination's
    // format, over the part of its rectangle on the surface: a rectangle
    // a clipper cuts may reach far outside it.
    let source = match op.src {
        Some((src_id, r)) => {
            let Some(s) = state(cpu).surface(src_id).cloned() else {
                return Err(DDERR_INVALIDOBJECT);
            };
            if decode {
                let mirror = [op.mirror_x, op.mirror_y];
                let data = dxt_blt::decode(cpu, &s, &r, &op.dst, &area, mirror, &d);
                origin = (area.left, area.top);
                let size = Rect::new(0, 0, area.width(), area.height());
                Some((data, d.clone(), size))
            } else {
                Some((load(cpu, &s, &r), s, r))
            }
        }
        None => None,
    };
    // A copy of the same size, not mirrored, takes the source's rows as
    // they are, and so does a decoded DXT source.
    let unscaled = decode
        || source.as_ref().is_some_and(|(_, _, sr)| {
            sr.width() as i64 == dw && sr.height() as i64 == dh && !op.mirror_x && !op.mirror_y
        });
    let src_key = match (&source, op.src_key) {
        (Some((_, s, _)), Some(k)) => Some(Key::new(k, s)),
        _ => None,
    };
    let dst_key = op.dst_key.map(|k| Key::new(k, &d));
    // Where the pixel under the destination's (`x`, `y`) is in the
    // source's, of an unscaled copy.
    let src_width = source.as_ref().map_or(0, |(_, _, sr)| sr.width() as usize);
    let src_at =
        |x: i32, y: i32| ((y - origin.1) as usize * src_width + (x - origin.0) as usize) * bytes;
    // A blit through no key puts its pixels over all that was there: the
    // destination isn't read. A fill or an unscaled copy stores its rows
    // as they are.
    let keyless = dst_key.is_none() && src_key.is_none();
    let overwrite = keyless && (source.is_none() || unscaled);
    let unscaled_source = source.as_ref().filter(|_| unscaled).map(|(data, ..)| data);
    for r in &rects {
        let len = r.width() as usize * bytes;
        let rows = r.height() as usize;
        if overwrite {
            let data: Cow<[u8]> = match &source {
                // The source's rows as they are, when they are as wide.
                Some((data, ..)) if r.width() as usize == src_width => {
                    Cow::Borrowed(&data[src_at(r.left, r.top)..][..len * rows])
                }
                Some((data, ..)) => (r.top..r.bottom)
                    .flat_map(|y| &data[src_at(r.left, y)..][..len])
                    .copied()
                    .collect(),
                None => {
                    let value = &op.fill.to_le_bytes()[..bytes];
                    Cow::Owned(value.repeat(r.width() as usize * rows))
                }
            };
            store(cpu, &d, r, &data);
            continue;
        }
        let mut out = if keyless {
            vec![0; len * rows]
        } else {
            load(cpu, &d, r)
        };
        for (dst_row, y) in out.chunks_exact_mut(len).zip(r.top..) {
            if let Some(data) = unscaled_source {
                let from = src_at(r.left, y);
                merge(bytes, dst_row, &data[from..from + len], src_key, dst_key);
                continue;
            }
            for (i, x) in (r.left..r.right).enumerate() {
                let at = i * bytes;
                let value = match &source {
                    Some((data, _, sr)) => {
                        let (sw, sh) = (sr.width() as i64, sr.height() as i64);
                        let mut u = (x - op.dst.left) as i64;
                        let mut v = (y - op.dst.top) as i64;
                        if op.mirror_x {
                            u = dw - 1 - u;
                        }
                        if op.mirror_y {
                            v = dh - 1 - v;
                        }
                        let (sx, sy) = (u * sw / dw, v * sh / dh);
                        let from = (sy * sw + sx) as usize * bytes;
                        let value = get(data, from, bytes);
                        if src_key.is_some_and(|k| k.holds(value)) {
                            continue;
                        }
                        value
                    }
                    None => op.fill,
                };
                if dst_key.is_some_and(|k| !k.holds(get(dst_row, at, bytes))) {
                    continue;
                }
                put(dst_row, at, bytes, value);
            }
        }
        store(cpu, &d, r, &out);
    }
    surface::changed(cpu, dst_id);
    Ok(())
}

/// The surface `this` is, if a blit can draw on it or read it.
fn ready(cpu: &mut Cpu, this: u32) -> Result<(u32, Surface), u32> {
    let (_, id) = surface_of(cpu, this)?;
    // What Direct3D drew in a 16-bit shadow of it, first.
    super::d3d::shadow::expose(cpu, id);
    let s = state(cpu).surface(id).cloned().ok_or(DDERR_INVALIDOBJECT)?;
    if s.lost {
        return Err(DDERR_SURFACELOST);
    }
    if s.locked || s.dc != 0 {
        return Err(DDERR_SURFACEBUSY);
    }
    Ok((id, s))
}

/// Read a COLORKEY at `at`.
fn read_key(cpu: &mut Cpu, at: u32) -> (u32, u32) {
    (cpu.bus.guest_read_32(at), cpu.bus.guest_read_32(at + 4))
}

/// Blt(this, dest rect, source, source rect, flags, fx).
pub fn blt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, dst_rect, src, src_rect, flags, fx] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (dst_id, d) = hr!(ready(cpu, this));
    if flags & DDBLT_KEYSRC != 0 && flags & DDBLT_KEYSRCOVERRIDE != 0
        || flags & DDBLT_KEYDEST != 0 && flags & DDBLT_KEYDESTOVERRIDE != 0
    {
        return Some(DDERR_INVALIDPARAMS);
    }
    let needs_fx = DDBLT_KEYSRCOVERRIDE
        | DDBLT_KEYDESTOVERRIDE
        | DDBLT_DDFX
        | DDBLT_ROP
        | DDBLT_COLORFILL
        | DDBLT_DEPTHFILL
        | DDBLT_ROTATIONANGLE;
    if flags & needs_fx != 0
        && (fx == 0
            || !cpu.bus.guest_probe(fx, FX_SIZE as usize, false)
            || cpu.bus.guest_read_32(fx) != FX_SIZE)
    {
        return Some(DDERR_INVALIDPARAMS);
    }
    if flags & DDBLT_ALPHA != 0 {
        return Some(DDERR_NOALPHAHW);
    }
    if flags & DDBLT_ZBUFFER != 0 {
        return Some(DDERR_NOZBUFFERHW);
    }
    if flags & (DDBLT_ROTATIONANGLE | DDBLT_DDROPS) != 0 {
        return Some(DDERR_NOROTATIONHW);
    }
    // The destination's rectangle: inside it, unless a clipper cuts it.
    let clip = if d.clipper != 0 {
        super::clipper::region(cpu, d.clipper)
    } else {
        None
    };
    let dst = if dst_rect != 0 {
        if !cpu.bus.guest_probe(dst_rect, 16, false) {
            return Some(DDERR_INVALIDPARAMS);
        }
        let r = Rect::read(cpu, dst_rect);
        let inside = surface::bounds(&d).intersect(&r) == r;
        // One a clipper cuts may be outside, but no wider than an i32.
        let huge = r.right as i64 - r.left as i64 > i32::MAX as i64
            || r.bottom as i64 - r.top as i64 > i32::MAX as i64;
        if r.left >= r.right || r.top >= r.bottom || huge || (!inside && clip.is_none()) {
            return Some(DDERR_INVALIDRECT);
        }
        r
    } else {
        surface::bounds(&d)
    };
    let mut op = Op {
        dst,
        clip,
        ..Op::default()
    };
    let rop = if flags & DDBLT_ROP != 0 {
        cpu.bus.guest_read_32(fx + FX_ROP)
    } else {
        SRCCOPY
    };
    let fill = if flags & (DDBLT_COLORFILL | DDBLT_DEPTHFILL) != 0 {
        Some(cpu.bus.guest_read_32(fx + FX_FILL))
    } else {
        match rop {
            SRCCOPY => None,
            BLACKNESS => Some(0),
            WHITENESS => Some(u32::MAX),
            _ => return Some(DDERR_NORASTEROPHW),
        }
    };
    if let Some(value) = fill {
        if d.compressed() {
            return Some(DDERR_UNSUPPORTED);
        }
        let bits = d.format.bits.min(32);
        op.fill = if bits >= 32 {
            value
        } else {
            value & ((1 << bits) - 1)
        };
    } else {
        if src == 0 {
            return Some(DDERR_INVALIDPARAMS);
        }
        let (src_id, s) = match ready(cpu, src) {
            Ok(v) => v,
            Err(DDERR_SURFACEBUSY) if surface_of(cpu, src).is_ok_and(|(_, i)| i == dst_id) => {
                return Some(DDERR_SURFACEBUSY);
            }
            Err(e) => return Some(e),
        };
        if s.compressed() || d.compressed() {
            op.dxt = Some(hr!(dxt_blt::plan(&s, &d)));
        } else if s.format.bits != d.format.bits {
            return Some(DDERR_INVALIDPIXELFORMAT);
        }
        let r = hr!(surface::rect_in(cpu, &s, src_rect));
        op.src = Some((src_id, r));
        if flags & DDBLT_KEYSRC != 0 {
            if s.key_set & 1 << KEY_SRC_BLT == 0 {
                return Some(DDERR_INVALIDPARAMS);
            }
            op.src_key = Some(s.keys[KEY_SRC_BLT]);
        }
        if flags & DDBLT_KEYSRCOVERRIDE != 0 {
            op.src_key = Some(read_key(cpu, fx + FX_SRC_KEY));
        }
        if flags & DDBLT_KEYDEST != 0 {
            // The destination key: the source's, as Windows takes it,
            // or the destination's own.
            let key = if s.key_set & 1 << KEY_DEST_BLT != 0 {
                s.keys[KEY_DEST_BLT]
            } else if d.key_set & 1 << KEY_DEST_BLT != 0 {
                d.keys[KEY_DEST_BLT]
            } else {
                s.keys[KEY_DEST_BLT]
            };
            op.dst_key = Some(key);
        }
    }
    if flags & DDBLT_KEYDESTOVERRIDE != 0 {
        op.dst_key = Some(read_key(cpu, fx + FX_DEST_KEY));
    }
    if op.dxt.is_some() {
        // A source key Blt asks for without the source having one was
        // DDERR_INVALIDPARAMS above.
        hr!(dxt_blt::keys(
            op.src_key.is_some(),
            true,
            op.dst_key.is_some()
        ));
    }
    if flags & DDBLT_DDFX != 0 {
        let ddfx = cpu.bus.guest_read_32(fx + FX_DDFX);
        if ddfx & DDBLTFX_ROTATE != 0 {
            return Some(DDERR_NOROTATIONHW);
        }
        op.mirror_x = ddfx & DDBLTFX_MIRRORLEFTRIGHT != 0;
        op.mirror_y = ddfx & DDBLTFX_MIRRORUPDOWN != 0;
    }
    Some(match run(cpu, dst_id, &op) {
        Ok(()) => DD_OK,
        Err(e) => e,
    })
}

/// BltFast(this, x, y, source, source rect, flags).
pub fn blt_fast(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, x, y, src, src_rect, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (dst_id, d) = hr!(ready(cpu, this));
    if src == 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    if flags & DDBLTFAST_SRCCOLORKEY != 0 && flags & DDBLTFAST_DESTCOLORKEY != 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    let (src_id, s) = hr!(ready(cpu, src));
    if d.clipper != 0 {
        return Some(DDERR_BLTFASTCANTCLIP);
    }
    let dxt = if s.compressed() || d.compressed() {
        Some(hr!(dxt_blt::plan(&s, &d)))
    } else if s.format.bits != d.format.bits {
        return Some(DDERR_INVALIDPIXELFORMAT);
    } else {
        None
    };
    let r = hr!(surface::rect_in(cpu, &s, src_rect));
    let (x, y) = (x as i32, y as i32);
    let (Some(right), Some(bottom)) = (x.checked_add(r.width()), y.checked_add(r.height())) else {
        return Some(DDERR_INVALIDRECT);
    };
    let dst = Rect::new(x, y, right, bottom);
    if x < 0 || y < 0 || surface::bounds(&d).intersect(&dst) != dst {
        return Some(DDERR_INVALIDRECT);
    }
    if dxt.is_some() {
        hr!(dxt_blt::keys(
            flags & DDBLTFAST_SRCCOLORKEY != 0,
            s.key_set & 1 << KEY_SRC_BLT != 0,
            flags & DDBLTFAST_DESTCOLORKEY != 0
        ));
    }
    let mut op = Op {
        dst,
        src: Some((src_id, r)),
        dxt,
        ..Op::default()
    };
    if flags & DDBLTFAST_SRCCOLORKEY != 0 && s.key_set & 1 << KEY_SRC_BLT != 0 {
        op.src_key = Some(s.keys[KEY_SRC_BLT]);
    }
    if flags & DDBLTFAST_DESTCOLORKEY != 0 && d.key_set & 1 << KEY_DEST_BLT != 0 {
        op.dst_key = Some(d.keys[KEY_DEST_BLT]);
    }
    Some(match run(cpu, dst_id, &op) {
        Ok(()) => DD_OK,
        Err(e) => e,
    })
}

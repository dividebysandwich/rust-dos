//! MSIMG32.DLL: the GDI extensions Windows 98 added, AlphaBlend,
//! GradientFill and TransparentBlt, on top of GDI32's DCs, bitmaps and
//! clipping. Windows 95 has no such DLL, so it is there only under
//! Windows 98 (`present`). The functions do what Windows 98 SE's
//! MSIMG32 5.00.1693.1 does, which differs from Windows 2000's in the
//! errors it sets and in how per-pixel alpha adds up (see each one).
//! Windows 98 also exports vSetDdrawflag and DllInitialize, its entry
//! point.

use super::gdi32::dc::{self, Dc};
use super::gdi32::draw::{self, Canvas, Translate};
use super::gdi32::surface::{self, Surface, Target};
use super::gdi32::{ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, Rect, TRUE, gdi};
use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "MSIMG32.DLL",
    base: 0xBF72_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
};

const fn stdcall(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: super::stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// The functions, with Windows 98 SE's ordinals. Append only (see
/// `stubs`).
static APIS: &[Api] = &[
    stdcall("vSetDdrawflag", 1, 0, v_set_ddrawflag),
    stdcall("AlphaBlend", 2, 11, alpha_blend),
    stdcall("DllInitialize", 3, 3, dll_initialize),
    stdcall("GradientFill", 4, 6, gradient_fill),
    stdcall("TransparentBlt", 5, 11, transparent_blt),
];

/// Whether the DLL is there: Windows 98 brought it.
pub fn present(version: super::version::WindowsVersion) -> bool {
    version.is_98()
}

/// vSetDdrawflag(): sets a flag of the DLL's own and returns nothing.
fn v_set_ddrawflag(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// DllInitialize(instance, reason, reserved): the DLL's entry point,
/// which a program has no reason to call. It succeeds.
fn dll_initialize(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TRUE)
}

fn set_error(cpu: &mut Cpu, code: u32) {
    super::gdi32::set_error(cpu, code);
}

/// A DC's device rectangle of the logical one at (`x`, `y`) of `w` by
/// `h`: left, top, right and bottom.
fn device_rect(dc: &Dc, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
    let (x0, y0) = dc.to_device(x, y);
    let (x1, y1) = dc.to_device(x.wrapping_add(w), y.wrapping_add(h));
    (x0, y0, x1, y1)
}

/// The source of AlphaBlend and TransparentBlt: the rows of the
/// source DC's surface its rectangle covers, the rectangle in that
/// surface's coordinates, and the DC's background color.
struct Source {
    surface: Surface,
    rect: (i32, i32, i32, i32),
    bk_color: u32,
}

/// Load the source rectangle (`x0`, `y0`)-(`x1`, `y1`) of the source DC
/// `sdc`, which draws on `place`, in its device coordinates. None when
/// it runs backwards or isn't all on the surface, which Windows 98 fails
/// without an error.
fn load_source(
    cpu: &mut Cpu,
    place: &dc::Place,
    sdc: &Dc,
    r: (i32, i32, i32, i32),
) -> Option<Source> {
    let (x0, y0, x1, y1) = r;
    let (width, height, _) = surface::dimensions(cpu, place.target)?;
    let (ox, oy) = place.origin;
    let (x0, y0, x1, y1) = (x0 + ox, y0 + oy, x1 + ox, y1 + oy);
    if x0 < 0 || y0 < 0 || x1 > width || y1 > height || x1 <= x0 || y1 <= y0 {
        return None;
    }
    let surface = Surface::load(cpu, place.target, y0, y1)?;
    let bk_color = dc::resolve_color(cpu, sdc, sdc.attrs.bk_color);
    Some(Source {
        surface,
        rect: (x0, y0, x1, y1),
        bk_color,
    })
}

/// The source pixel for the destination's pixel `d` of the run from `d0`
/// to `d1`, stretched from the source's run from `s0` to `s1`.
#[inline]
fn stretch(d: i32, (d0, d1): (i32, i32), (s0, s1): (i32, i32)) -> i32 {
    let (dw, sw) = ((d1 - d0) as i64, (s1 - s0) as i64);
    s0 + ((d - d0) as i64 * sw / dw) as i32
}

/// Whether a surface's pixels are 32-bit ones with alpha in their top
/// byte: a DIB section or bitmap of 32 bits with the usual masks.
fn has_alpha(s: &Surface) -> bool {
    s.bpp == 32 && s.target != Target::Screen && s.masks == surface::default_masks(32)
}

/// A pixel's value of `s` as ARGB, 8 bits each, for AlphaBlend. A
/// pixel without alpha has 255. A 16-bit pixel's channels are widened
/// by shifting them up, as Windows 98's AlphaBlend does, with the low
/// bits 0.
fn argb(s: &Surface, v: u32) -> u32 {
    if has_alpha(s) {
        return v;
    }
    if s.bpp == 16 && !s.device && !s.mono {
        let channel = |mask: u32| -> u32 {
            if mask == 0 {
                return 0;
            }
            let bits = (mask >> mask.trailing_zeros()).count_ones();
            let c = (v & mask) >> mask.trailing_zeros();
            if bits >= 8 {
                c >> (bits - 8)
            } else {
                c << (8 - bits)
            }
        };
        let [r, g, b] = s.masks.map(channel);
        return 0xFF00_0000 | r << 16 | g << 8 | b;
    }
    let c = s.color_of(v);
    0xFF00_0000 | (c & 0xFF) << 16 | (c & 0xFF00) | (c >> 16 & 0xFF)
}

/// ARGB as a pixel's value of `s`: a pixel without alpha leaves it out.
fn value(s: &Surface, argb: u32) -> u32 {
    if has_alpha(s) {
        return argb;
    }
    let c = (argb >> 16 & 0xFF) | (argb & 0xFF00) | (argb & 0xFF) << 16;
    s.value_of(c)
}

/// `x` / 255, rounded.
#[inline]
fn div255(x: u32) -> u32 {
    (x + 127) / 255
}

/// Each byte of `c` times `a` / 255.
#[inline]
fn scale(c: u32, a: u32) -> u32 {
    (0..4).fold(0, |out, i| {
        let v = c >> (8 * i) & 0xFF;
        out | div255(v * a) << (8 * i)
    })
}

/// Blend the source pixel `s` onto `d`, both ARGB. With a constant alpha
/// `sca` alone, each byte is the two's mix. With the source's alpha
/// (`per_pixel`), whose colors are premultiplied, the source scaled by
/// `sca` is added to the destination scaled by what the scaled source's
/// alpha leaves of it, as one 32-bit sum: Windows 98 lets a byte that
/// overflows carry into the next, where Windows 2000 saturates it. A
/// pixel whose scaled alpha is 0 stays as it is.
fn blend(s: u32, d: u32, sca: u32, per_pixel: bool) -> u32 {
    if !per_pixel {
        return (0..4).fold(0, |out, i| {
            let (sv, dv) = (s >> (8 * i) & 0xFF, d >> (8 * i) & 0xFF);
            out | div255(sv * sca + dv * (255 - sca)) << (8 * i)
        });
    }
    let s = if sca == 255 { s } else { scale(s, sca) };
    let a = s >> 24;
    if a == 0 {
        return d;
    }
    s.wrapping_add(scale(d, 255 - a))
}

// GDI's BLENDFUNCTION, as the program passes it in a doubleword.
const AC_SRC_OVER: u32 = 0;
const AC_SRC_ALPHA: u32 = 1;

/// AlphaBlend(dst, x, y, width, height, src, xsrc, ysrc, wsrc, hsrc,
/// blend): blend the source rectangle, stretched, onto the destination.
///
/// As Windows 98 SE's: a BLENDFUNCTION other than AC_SRC_OVER with no
/// flags and an AlphaFormat of 0 or AC_SRC_ALPHA, then a negative width
/// or height, are ERROR_INVALID_PARAMETER, before an invalid destination
/// DC is ERROR_INVALID_HANDLE. AC_SRC_ALPHA with a source that isn't 32
/// bits a pixel is ERROR_INVALID_PARAMETER. An empty rectangle, no
/// source DC and a source rectangle that isn't all on its surface fail
/// without an error.
/// Source and destination may be the same DC, and may overlap. A
/// SourceConstantAlpha of 0 changes nothing.
fn alpha_blend(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hdst, x, y, w, h, hsrc, xs, ys, ws, hs, func] = args else {
        unreachable!()
    };
    let [x, y, w, h, xs, ys, ws, hs] = [x, y, w, h, xs, ys, ws, hs].map(|v| v as i32);
    let [op, flags, sca, format] = func.to_le_bytes().map(u32::from);
    if op != AC_SRC_OVER || flags != 0 || format & !AC_SRC_ALPHA != 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    if w < 0 || h < 0 || ws < 0 || hs < 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let Some(ddc) = gdi(cpu).dc(hdst).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    if w == 0 || h == 0 || ws == 0 || hs == 0 {
        return Some(FALSE);
    }
    let Some(sdc) = gdi(cpu).dc(hsrc).cloned() else {
        return Some(FALSE);
    };
    let Some(place) = dc::place(cpu, hsrc) else {
        return Some(FALSE);
    };
    let Some(src) = load_source(cpu, &place, &sdc, device_rect(&sdc, xs, ys, ws, hs)) else {
        return Some(FALSE);
    };
    let per_pixel = format & AC_SRC_ALPHA != 0;
    if per_pixel && !has_alpha(&src.surface) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    if sca == 0 {
        return Some(TRUE);
    }
    let r = device_rect(&ddc, x, y, w, h);
    let Some(mut c) = Canvas::open(cpu, hdst, Rect::new(r.0, r.1, r.2, r.3)) else {
        return Some(TRUE);
    };
    let (ox, oy) = c.origin;
    let (sx0, sy0, sx1, sy1) = src.rect;
    let area = Rect::new(r.0, r.1, r.2, r.3).offset(ox, oy);
    // A palette's nearest colors, which are a search.
    let mut nearest = std::collections::HashMap::new();
    for clip in c.rects.clone() {
        let a = clip.intersect(&area);
        for py in a.top..a.bottom {
            let sy = stretch(py - oy, (r.1, r.3), (sy0, sy1));
            for px in a.left..a.right {
                let sx = stretch(px - ox, (r.0, r.2), (sx0, sx1));
                let s = argb(&src.surface, src.surface.get(sx, sy));
                let d = argb(&c.surface, c.surface.get(px, py));
                let v = blend(s, d, sca, per_pixel);
                let out = if c.surface.bpp <= 8 {
                    *nearest.entry(v).or_insert_with(|| value(&c.surface, v))
                } else {
                    value(&c.surface, v)
                };
                c.surface.put(px, py, out);
            }
        }
        c.drew(a);
    }
    c.close(cpu);
    Some(TRUE)
}

/// TransparentBlt(dst, x, y, width, height, src, xsrc, ysrc, wsrc,
/// hsrc, transparent): copy the source rectangle, stretched, to the
/// destination, but for the pixels whose color is `transparent`. A
/// color with its top byte set matches none; the alpha of a 32-bit
/// source's pixels doesn't count. Pixels go as they are between bitmaps
/// of the same format, and are converted between others.
///
/// As Windows 98 SE's: an invalid destination DC is ERROR_INVALID_HANDLE;
/// no source DC, and a source rectangle that starts left of or above
/// the surface or ends below it, are ERROR_INVALID_PARAMETER. A width or
/// height of 0 or less, and a source rectangle past the surface's right
/// edge, fail without an error.
fn transparent_blt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hdst, x, y, w, h, hsrc, xs, ys, ws, hs, transparent] = args else {
        unreachable!()
    };
    let [x, y, w, h, xs, ys, ws, hs] = [x, y, w, h, xs, ys, ws, hs].map(|v| v as i32);
    let Some(ddc) = gdi(cpu).dc(hdst).cloned() else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    if w <= 0 || h <= 0 || ws <= 0 || hs <= 0 {
        return Some(FALSE);
    }
    let Some(sdc) = gdi(cpu).dc(hsrc).cloned() else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    };
    let sr = device_rect(&sdc, xs, ys, ws, hs);
    let Some(place) = dc::place(cpu, hsrc) else {
        return Some(FALSE);
    };
    let height = surface::dimensions(cpu, place.target).map_or(0, |d| d.1);
    let (ox, oy) = place.origin;
    if sr.0 + ox < 0 || sr.1 + oy < 0 || sr.3 + oy > height {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let Some(src) = load_source(cpu, &place, &sdc, sr) else {
        return Some(FALSE);
    };
    let r = device_rect(&ddc, x, y, w, h);
    let Some(mut c) = Canvas::open(cpu, hdst, Rect::new(r.0, r.1, r.2, r.3)) else {
        return Some(TRUE);
    };
    // The DC's palette map, built once, and the translation table kept
    // from the last blit of the same colors, as BitBlt's.
    let map = c
        .surface
        .device
        .then(|| super::gdi32::palette::DeviceMap::new(cpu, ddc.attrs.palette));
    let text = draw::value_through(cpu, &ddc, &c.surface, ddc.attrs.text_color, map.as_ref());
    let bk = draw::value_through(cpu, &ddc, &c.surface, ddc.attrs.bk_color, map.as_ref());
    let mut xlat = Translate::kept(cpu, &src.surface, map, src.bk_color, text, bk);
    let (ox, oy) = c.origin;
    let (sx0, sy0, sx1, sy1) = src.rect;
    let area = Rect::new(r.0, r.1, r.2, r.3).offset(ox, oy);
    for clip in c.rects.clone() {
        let a = clip.intersect(&area);
        for py in a.top..a.bottom {
            let sy = stretch(py - oy, (r.1, r.3), (sy0, sy1));
            for px in a.left..a.right {
                let sx = stretch(px - ox, (r.0, r.2), (sx0, sx1));
                let v = src.surface.get(sx, sy);
                if src.surface.color_of(v) & 0xFF_FFFF == transparent {
                    continue;
                }
                let out = xlat.get(&src.surface, v, &c.surface);
                c.surface.put(px, py, out);
            }
        }
        c.drew(a);
    }
    xlat.keep(cpu);
    c.close(cpu);
    Some(TRUE)
}

// GradientFill's modes besides GRADIENT_FILL_RECT_H, 0.
const GRADIENT_FILL_RECT_V: u32 = 1;
const GRADIENT_FILL_TRIANGLE: u32 = 2;

/// A TRIVERTEX: a point in logical units and 16-bit red, green, blue
/// and alpha.
#[derive(Clone, Copy, Debug, Default)]
struct Vertex {
    x: i32,
    y: i32,
    c: [i64; 4],
}

/// The 4x4 ordered dither GradientFill draws 16-bit pixels with.
const BAYER: [[i64; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// The pixel value of the 16-bit color `c` (red, green, blue, alpha) on
/// `s` at surface pixel (`x`, `y`): 32-bit pixels take its top 8 bits
/// and alpha too, 16-bit ones are dithered, palette ones take the
/// nearest color, which `nearest` keeps.
fn gradient_value(
    s: &Surface,
    c: [i64; 4],
    x: i32,
    y: i32,
    nearest: &mut std::collections::HashMap<u32, u32>,
) -> u32 {
    let [r, g, b, a] = c.map(|v| (v >> 8).clamp(0, 255) as u32);
    if has_alpha(s) {
        return a << 24 | r << 16 | g << 8 | b;
    }
    if s.bpp == 16 && !s.device {
        let t = BAYER[y.rem_euclid(4) as usize][x.rem_euclid(4) as usize];
        let channel = |v: i64, mask: u32| -> u32 {
            if mask == 0 {
                return 0;
            }
            let bits = (mask >> mask.trailing_zeros()).count_ones();
            // 5 bits: (v / 128 + t) / 16; 6 bits, twice as fine.
            let level = if bits >= 6 {
                (v / 64 + t) / 16
            } else {
                (v / 128 + t) / 16
            };
            let max = (1i64 << bits) - 1;
            (level.clamp(0, max) as u32) << mask.trailing_zeros()
        };
        return channel(c[0], s.masks[0]) | channel(c[1], s.masks[1]) | channel(c[2], s.masks[2]);
    }
    let color = r | g << 8 | b << 16;
    if s.bpp > 8 {
        return s.value_of(color);
    }
    *nearest.entry(color).or_insert_with(|| s.value_of(color))
}

/// The vertices of the mesh's element: the indices, as many as it has.
fn mesh_indices(cpu: &mut Cpu, at: u32, i: u32, n: u32) -> Option<Vec<u32>> {
    let addr = at.checked_add(i.checked_mul(n * 4)?)?;
    if !cpu.bus.guest_probe(addr, n as usize * 4, false) {
        return None;
    }
    Some(
        (0..n)
            .map(|k| cpu.bus.guest_read_32(addr + 4 * k))
            .collect(),
    )
}

/// GradientFill(hdc, vertices, count, mesh, mesh count, mode): fill
/// rectangles with colors that run from one side to the other
/// (GRADIENT_FILL_RECT_H and _V), or triangles shaded between their
/// corners' colors (GRADIENT_FILL_TRIANGLE). A rectangle covers its
/// left and top edges and not its right and bottom ones; a triangle the
/// pixels of rows from its top down to its bottom that its edges take
/// in, with barycentric shading, as Windows draws them. 32-bit DIBs get
/// the vertices' alpha.
///
/// As Windows 98 SE's: an invalid DC or mode fails without an error, and
/// no mesh or no vertices succeed with nothing drawn. The elements are
/// drawn in turn, and one with an index past the vertices, or a
/// triangle without an area over more than one row, fails without an
/// error, the elements before it drawn. Windows 98 checks the indices
/// only in part and reads past the vertices, so that some such indices
/// succeed there.
fn gradient_fill(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hdc, vertices, count, mesh, mesh_count, mode] = args else {
        unreachable!()
    };
    let Some(dc) = gdi(cpu).dc(hdc).cloned() else {
        return Some(FALSE);
    };
    if mode > GRADIENT_FILL_TRIANGLE {
        return Some(FALSE);
    }
    if count == 0 || mesh_count == 0 {
        return Some(TRUE);
    }
    let per = if mode == GRADIENT_FILL_TRIANGLE { 3 } else { 2 };
    for i in 0..mesh_count.min(0x10_0000) {
        let Some(indices) = mesh_indices(cpu, mesh, i, per) else {
            break;
        };
        if indices.iter().any(|&k| k >= count) {
            return Some(FALSE);
        }
        let mut v = Vec::with_capacity(per as usize);
        for &k in &indices {
            let at = vertices.wrapping_add(k.wrapping_mul(16));
            if !cpu.bus.guest_probe(at, 16, false) {
                break;
            }
            let words = [8, 10, 12, 14].map(|o| cpu.bus.guest_read_16(at + o) as i64);
            let (lx, ly) = (cpu.bus.guest_read_32(at), cpu.bus.guest_read_32(at + 4));
            let (x, y) = dc.to_device(lx as i32, ly as i32);
            v.push(Vertex { x, y, c: words });
        }
        if v.len() < per as usize {
            continue;
        }
        if mode == GRADIENT_FILL_TRIANGLE {
            if !triangle(cpu, hdc, [v[0], v[1], v[2]]) {
                return Some(FALSE);
            }
        } else {
            rectangle(cpu, hdc, [v[0], v[1]], mode == GRADIENT_FILL_RECT_V);
        }
    }
    Some(TRUE)
}

/// Fill the rectangle between the corners `v`, its colors running from
/// left to right, or from top to bottom when `vertical`.
fn rectangle(cpu: &mut Cpu, hdc: u32, v: [Vertex; 2], vertical: bool) {
    let (mut a, mut b) = (v[0], v[1]);
    let r = Rect::new(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y));
    if r.is_empty() {
        return;
    }
    if vertical && a.y > b.y || !vertical && a.x > b.x {
        std::mem::swap(&mut a, &mut b);
    }
    let Some(mut c) = Canvas::open(cpu, hdc, r) else {
        return;
    };
    let (ox, oy) = c.origin;
    let area = r.offset(ox, oy);
    let len = if vertical {
        r.bottom - r.top
    } else {
        r.right - r.left
    } as i64;
    let mut nearest = std::collections::HashMap::new();
    for clip in c.rects.clone() {
        let ar = clip.intersect(&area);
        for py in ar.top..ar.bottom {
            for px in ar.left..ar.right {
                let pos = if vertical {
                    py - oy - r.top
                } else {
                    px - ox - r.left
                } as i64;
                let color: [i64; 4] =
                    std::array::from_fn(|i| (a.c[i] * (len - pos) + b.c[i] * pos) / len);
                let value = gradient_value(&c.surface, color, px, py, &mut nearest);
                c.surface.put(px, py, value);
            }
        }
        c.drew(ar);
    }
    c.close(cpu);
}

/// The x where the edge from (`x1`, `y1`) to (`x2`, `y2`) crosses row
/// `y`, followed from its right end for the rounding Windows has.
fn edge_x(y: i32, (x1, y1): (i32, i32), (x2, y2): (i32, i32)) -> i32 {
    let (y, x1, y1, x2, y2) = (y as i64, x1 as i64, y1 as i64, x2 as i64, y2 as i64);
    let x = if x2 > x1 {
        x2 + (y - y2) * (x2 - x1) / (y2 - y1)
    } else {
        x1 + (y - y1) * (x2 - x1) / (y2 - y1)
    };
    x as i32
}

/// Fill the triangle `v`, shaded between its corners' colors. A
/// triangle on one row has nothing to fill; one over more rows without
/// an area fails, as Windows 98 SE's does: false.
fn triangle(cpu: &mut Cpu, hdc: u32, mut v: [Vertex; 3]) -> bool {
    v.sort_by_key(|p| p.y);
    if v[0].y == v[2].y {
        return true;
    }
    let [(x0, y0), (x1, y1), (x2, y2)] = v.map(|p| (p.x as i64, p.y as i64));
    let det = (y2 - y1) * (x2 - x0) - (x2 - x1) * (y2 - y0);
    if det == 0 {
        return false;
    }
    let (left, right) = (
        v.iter().map(|p| p.x).min().unwrap_or(0),
        v.iter().map(|p| p.x).max().unwrap_or(0),
    );
    let r = Rect::new(left, v[0].y, right, v[2].y);
    let Some(mut c) = Canvas::open(cpu, hdc, r) else {
        return true;
    };
    let (ox, oy) = c.origin;
    // Only the rows and columns the clip region has, in device
    // coordinates: a triangle may run far past the surface.
    let clip = c
        .rects
        .iter()
        .fold(Rect::default(), |a, q| a.union(q))
        .offset(-ox, -oy)
        .intersect(&r);
    let p = |q: &Vertex| (q.x, q.y);
    let mut nearest = std::collections::HashMap::new();
    for y in clip.top..clip.bottom {
        let e1 = if y < v[1].y {
            edge_x(y, p(&v[0]), p(&v[1]))
        } else {
            edge_x(y, p(&v[1]), p(&v[2]))
        };
        let e2 = edge_x(y, p(&v[0]), p(&v[2]));
        let (from, to) = (e1.min(e2).max(clip.left), e1.max(e2).min(clip.right));
        for x in from..to {
            let (px, py) = (x + ox, y + oy);
            if !c.visible(px, py) {
                continue;
            }
            let (dx, dy) = (x as i64 - x2, y as i64 - y2);
            let l1 = (y1 - y2) * dx - (x1 - x2) * dy;
            let l2 = (y2 - y0) * dx - (x2 - x0) * dy;
            let color: [i64; 4] = std::array::from_fn(|i| {
                (v[0].c[i] * l1 + v[1].c[i] * l2 + v[2].c[i] * (det - l1 - l2)) / det
            });
            let value = gradient_value(&c.surface, color, px, py, &mut nearest);
            c.surface.put(px, py, value);
        }
        c.drew(Rect::new(from + ox, y + oy, to + ox, y + oy + 1));
    }
    c.close(cpu);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_alpha_rounds_the_mix() {
        // Windows 98 SE's results.
        assert_eq!(blend(0x4020_1008, 0x8080_8080, 128, false), 0x6050_4844);
        assert_eq!(blend(0x80FF_0000, 0x8080_8080, 1, false), 0x8080_7F7F);
        assert_eq!(blend(0x0012_3456, 0x8080_8080, 200, false), 0x1C2A_445F);
    }

    #[test]
    fn per_pixel_alpha_carries_between_bytes() {
        assert_eq!(blend(0x4020_1008, 0x8080_8080, 255, true), 0xA080_7068);
        assert_eq!(blend(0x80FF_0000, 0x8080_8080, 255, true), 0xC13F_4040);
        assert_eq!(blend(0x80FF_0000, 0x8080_8080, 200, true), 0xB316_4E4E);
        assert_eq!(blend(0x4020_1008, 0x8080_8080, 64, true), 0x8880_7C7A);
        assert_eq!(blend(0x0012_3456, 0x8080_8080, 255, true), 0x8080_8080);
        assert_eq!(blend(0x10FF_FFFF, 0x0020_4060, 255, true), 0x111E_3C59);
    }

    #[test]
    fn triangle_edges_round_as_windows() {
        assert_eq!(edge_x(2, (1, 1), (6, 3)), 4);
        assert_eq!(edge_x(2, (1, 1), (3, 7)), 2);
        assert_eq!(edge_x(5, (6, 3), (3, 7)), 5);
    }
}

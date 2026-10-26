//! Copying pixels: CopyRects between surfaces of one format,
//! UpdateTexture from a texture in system memory to one in video
//! memory, GetFrontBuffer, and a windowed device's Present, which puts
//! the back buffer in the window's client area on the desktop.

use super::super::ddraw::d3d::shadow;
use super::super::ddraw::surface::{self as dd_surface, Surface as DdSurface};
use super::super::ddraw::{self, Format};
use super::device::{dd_surface_of, this_device};
use super::resource::{self, FMT_A8R8G8B8};
use super::{D3D_OK, D3DERR_INVALIDCALL, state};
use crate::cpu::Cpu;
use crate::win32::gdi32::Rect;
use crate::win32::gdi32::dc::DCX_CLIPSIBLINGS;
use crate::win32::screen;

// Pixel formats.

/// A channel of `raw` under `mask`, as 8 bits: its bits repeated below
/// them.
fn channel(raw: u32, mask: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    let bits = mask.count_ones();
    let v = (raw & mask) >> mask.trailing_zeros();
    if bits >= 8 {
        return v >> (bits - 8);
    }
    let mut out = 0;
    let mut have = 0;
    while have < 8 {
        out = out << bits | v;
        have += bits;
    }
    out >> (have - 8)
}

/// A pixel of the format `f` as ARGB: opaque if it has no alpha.
pub fn to_argb(f: &Format, raw: u32) -> u32 {
    let a = if f.a != 0 { channel(raw, f.a) } else { 0xFF };
    a << 24 | channel(raw, f.r) << 16 | channel(raw, f.g) << 8 | channel(raw, f.b)
}

/// The pixels of the rectangle `r` of the surface `s`, as ARGB.
fn read_argb(cpu: &mut Cpu, s: &DdSurface, r: &Rect) -> Vec<u32> {
    let bpp = s.bytes_per_pixel();
    let w = (r.right - r.left).max(0) as u32;
    let mut out = Vec::with_capacity((w * (r.bottom - r.top).max(0) as u32) as usize);
    let mut row = vec![0; (w * bpp) as usize];
    for y in r.top..r.bottom {
        let offset = y as u32 * s.pitch + r.left as u32 * bpp;
        dd_surface::read_bytes(cpu, s, offset, &mut row);
        for p in row.chunks_exact(bpp as usize) {
            let mut b = [0; 4];
            b[..p.len()].copy_from_slice(p);
            out.push(to_argb(&s.format, u32::from_le_bytes(b)));
        }
    }
    out
}

/// The DirectDraw surface of the surface `id` of Direct3D 8's.
fn dd(cpu: &mut Cpu, id: u32) -> Option<(u32, DdSurface)> {
    let dd = dd_surface_of(cpu, id);
    let s = ddraw::state(cpu).surface(dd)?.clone();
    Some((dd, s))
}

/// The surface `id` of Direct3D 8's changed: its texture with it.
fn changed(cpu: &mut Cpu, id: u32) {
    let dd = dd_surface_of(cpu, id);
    dd_surface::changed(cpu, dd);
    let container = state(cpu).object(id).map_or(0, |o| o.container);
    let root = state(cpu)
        .object(container)
        .and_then(|o| o.texture.as_ref())
        .map_or(0, |t| t.dd);
    if root != 0 {
        dd_surface::changed(cpu, root);
    }
}

/// Copy the rectangle `r` of `src` to (`x`, `y`) of `dst`, byte for
/// byte.
fn copy_rect(cpu: &mut Cpu, src: &DdSurface, dst: &DdSurface, r: &Rect, x: u32, y: u32) {
    let bpp = src.bytes_per_pixel();
    let w = (r.right - r.left) as u32;
    let mut row = vec![0; (w * bpp) as usize];
    for k in 0..(r.bottom - r.top) as u32 {
        let from = (r.top as u32 + k) * src.pitch + r.left as u32 * bpp;
        dd_surface::read_bytes(cpu, src, from, &mut row);
        let to = (y + k) * dst.pitch + x * bpp;
        dd_surface::write_bytes(cpu, dst, to, &row);
    }
}

/// Copy all of the DirectDraw surface `src` to `dst`, of one size.
pub fn copy_all(cpu: &mut Cpu, src: u32, dst: u32) {
    let s = ddraw::state(cpu).surface(src).cloned();
    let d = ddraw::state(cpu).surface(dst).cloned();
    let (Some(s), Some(d)) = (s, d) else {
        return;
    };
    if (s.width, s.height, s.format.bits) != (d.width, d.height, d.format.bits) {
        return;
    }
    let r = Rect::new(0, 0, s.width as i32, s.height as i32);
    copy_rect(cpu, &s, &d, &r, 0, 0);
    dd_surface::changed(cpu, dst);
}

/// CopyRects(this, source, rects, count, dest, points): rectangles of a
/// surface to the places `points` says in another of its format (the
/// same places without them).
pub fn copy_rects(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, src, rects_at, count, dst, points_at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    hr!(this_device(cpu, this));
    let (src_id, s8) = hr!(resource::surface_of(cpu, src));
    let (dst_id, d8) = hr!(resource::surface_of(cpu, dst));
    if src_id == dst_id || s8.format != d8.format || s8.locked || d8.locked {
        return Some(D3DERR_INVALIDCALL);
    }
    let (Some((src_dd, _)), Some((_, d))) = (dd(cpu, src_id), dd(cpu, dst_id)) else {
        return Some(D3DERR_INVALIDCALL);
    };
    shadow::expose(cpu, src_dd);
    let Some(s) = ddraw::state(cpu).surface(src_dd).cloned() else {
        return Some(D3DERR_INVALIDCALL);
    };
    let mut list = Vec::new();
    if rects_at == 0 || count == 0 {
        let r = Rect::new(0, 0, s.width as i32, s.height as i32);
        let (x, y) = if points_at != 0 && cpu.bus.guest_probe(points_at, 8, false) {
            let x = cpu.bus.guest_read_32(points_at) as i32;
            (x, cpu.bus.guest_read_32(points_at + 4) as i32)
        } else {
            (0, 0)
        };
        list.push((r, x, y));
    } else {
        if count > 0x1_0000 || !cpu.bus.guest_probe(rects_at, count as usize * 16, false) {
            return Some(D3DERR_INVALIDCALL);
        }
        if points_at != 0 && !cpu.bus.guest_probe(points_at, count as usize * 8, false) {
            return Some(D3DERR_INVALIDCALL);
        }
        for i in 0..count {
            let r = Rect::read(cpu, rects_at + 16 * i);
            let (x, y) = if points_at != 0 {
                let x = cpu.bus.guest_read_32(points_at + 8 * i) as i32;
                (x, cpu.bus.guest_read_32(points_at + 8 * i + 4) as i32)
            } else {
                (r.left, r.top)
            };
            list.push((r, x, y));
        }
    }
    for (r, x, y) in &list {
        let inside = r.left >= 0
            && r.top >= 0
            && r.left < r.right
            && r.top < r.bottom
            && r.right as u32 <= s.width
            && r.bottom as u32 <= s.height;
        let fits = *x >= 0
            && *y >= 0
            && *x as i64 + r.right as i64 - r.left as i64 <= d.width as i64
            && *y as i64 + r.bottom as i64 - r.top as i64 <= d.height as i64;
        if !inside || !fits {
            return Some(D3DERR_INVALIDCALL);
        }
    }
    for (r, x, y) in list {
        copy_rect(cpu, &s, &d, &r, x as u32, y as u32);
    }
    changed(cpu, dst_id);
    Some(D3D_OK)
}

/// UpdateTexture(this, source, dest): each level of a texture in
/// system memory into the dest's level of its size.
pub fn update_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, src, dst] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    hr!(this_device(cpu, this));
    let (_, st) = hr!(resource::texture_of(cpu, src));
    let (_, dt) = hr!(resource::texture_of(cpu, dst));
    if st.pool != resource::POOL_SYSTEMMEM || dt.pool != resource::POOL_DEFAULT {
        return Some(D3DERR_INVALIDCALL);
    }
    if st.format != dt.format || st.levels.len() < dt.levels.len() {
        return Some(D3DERR_INVALIDCALL);
    }
    let size = |cpu: &mut Cpu, id: u32| dd(cpu, id).map(|(_, s)| (s.width, s.height));
    let mut pairs = Vec::new();
    for &level in &dt.levels {
        let want = size(cpu, level);
        let mut found = None;
        for &from in &st.levels {
            if size(cpu, from) == want {
                found = Some(from);
                break;
            }
        }
        let Some(from) = found else {
            return Some(D3DERR_INVALIDCALL);
        };
        pairs.push((from, level));
    }
    for (from, to) in pairs {
        let (a, b) = (dd_surface_of(cpu, from), dd_surface_of(cpu, to));
        copy_all(cpu, a, b);
    }
    dd_surface::changed(cpu, dt.dd);
    Some(D3D_OK)
}

/// GetFrontBuffer(this, dest): the screen's picture, into a surface of
/// A8R8G8B8 the display's size.
pub fn get_front_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, dst] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, dev) = hr!(this_device(cpu, this));
    let (dst_id, d8) = hr!(resource::surface_of(cpu, dst));
    if d8.format != FMT_A8R8G8B8 || d8.locked {
        return Some(D3DERR_INVALIDCALL);
    }
    let Some((_, d)) = dd(cpu, dst_id) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (w, h, _, _) = ddraw::display::current(cpu);
    if d.width < w || d.height < h {
        return Some(D3DERR_INVALIDCALL);
    }
    let pixels: Vec<u32> = if dev.front != 0
        && let Some(s) = ddraw::state(cpu).surface(dev.front).cloned()
    {
        let r = Rect::new(0, 0, s.width as i32, s.height as i32);
        read_argb(cpu, &s, &r)
    } else {
        let raw = screen::load_desktop(cpu, 0, h);
        if screen::desktop(cpu).bpp == 8 {
            // Indices into the system palette, whose colors are COLORREFs.
            let system = crate::win32::gdi32::palette::system_colors(cpu);
            raw.iter()
                .map(|&i| {
                    let c = system[i as usize];
                    0xFF00_0000 | (c & 0xFF) << 16 | c & 0xFF00 | (c >> 16) & 0xFF
                })
                .collect()
        } else {
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|p| 0xFF00_0000 | u32::from_le_bytes(*p))
                .collect()
        }
    };
    let width = (pixels.len() as u32).checked_div(h).unwrap_or(0);
    for y in 0..h {
        let row: Vec<u8> = pixels[(y * width) as usize..((y + 1) * width) as usize]
            .iter()
            .flat_map(|p| p.to_le_bytes())
            .collect();
        dd_surface::write_bytes(cpu, &d, y * d.pitch, &row);
    }
    changed(cpu, dst_id);
    Some(D3D_OK)
}

/// A windowed device's Present: the rectangle `src` of the back buffer
/// `back` (DirectDraw's surface; all of it with none), stretched onto
/// the rectangle `dst` of the window's client area (all of it), where
/// the window can be seen.
pub fn present_window(cpu: &mut Cpu, back: u32, src: Option<Rect>, dst: Option<Rect>, window: u32) {
    if ddraw::display::owns_display(cpu) || !cpu.bus.win32.user.is_window(window) {
        return;
    }
    let Some(s) = ddraw::state(cpu).surface(back).cloned() else {
        return;
    };
    let whole = Rect::new(0, 0, s.width as i32, s.height as i32);
    let src = src.map_or(whole, |r| r.intersect(&whole));
    if src.left >= src.right || src.top >= src.bottom {
        return;
    }
    let user = &cpu.bus.win32.user;
    let client = user.screen_client(window);
    let dst = match dst {
        Some(r) => Rect::new(
            client.left + r.left,
            client.top + r.top,
            client.left + r.right,
            client.top + r.bottom,
        ),
        None => client,
    };
    if dst.left >= dst.right || dst.top >= dst.bottom {
        return;
    }
    let visible = crate::win32::user32::paint::visible_region(user, window, DCX_CLIPSIBLINGS);
    let pixels = read_argb(cpu, &s, &src);
    let (sw, sh) = ((src.right - src.left) as i64, (src.bottom - src.top) as i64);
    let (dw, dh) = ((dst.right - dst.left) as i64, (dst.bottom - dst.top) as i64);
    let (width, height, bpp, pitch) = ddraw::display::current(cpu);
    let screen_rect = Rect::new(0, 0, width as i32, height as i32);
    let bytes = bpp.div_ceil(8);
    // On a desktop of 256 colours, the nearest color of the system
    // palette, remembered for each color.
    let system = (bytes == 1).then(|| crate::win32::gdi32::palette::system_colors(cpu));
    let mut indices = std::collections::HashMap::new();
    for r in &visible.rects {
        let r = r.intersect(&dst).intersect(&screen_rect);
        if r.left >= r.right || r.top >= r.bottom {
            continue;
        }
        for y in r.top..r.bottom {
            let sy = ((y - dst.top) as i64 * sh / dh) as usize;
            let mut row = Vec::with_capacity(((r.right - r.left) as u32 * bytes) as usize);
            for x in r.left..r.right {
                let sx = ((x - dst.left) as i64 * sw / dw) as usize;
                let p = pixels[sy * sw as usize + sx] & 0xFF_FFFF;
                match bytes {
                    1 => {
                        let table = system.as_deref().unwrap_or_default();
                        let i = *indices.entry(p).or_insert_with(|| {
                            let c = (p & 0xFF) << 16 | p & 0xFF00 | (p >> 16) & 0xFF;
                            crate::win32::gdi32::surface::nearest(table, c) as u8
                        });
                        row.push(i);
                    }
                    2 => row.extend_from_slice(&screen::to_565(p).to_le_bytes()),
                    3 => row.extend_from_slice(&p.to_le_bytes()[..3]),
                    _ => row.extend_from_slice(&p.to_le_bytes()),
                }
            }
            screen::write(cpu, y as u32 * pitch + r.left as u32 * bytes, &row);
        }
    }
}

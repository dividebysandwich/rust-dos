//! The video processor: the desktop from the card's memory in one of
//! four pixel formats, linear or tiled, through the colour lookup table;
//! the overlay window over it, which shows video and the 3D engine's
//! buffers; and the hardware cursor on top.
//!
//! From 86Box's vid_voodoo_banshee.c (`banshee_render_16bpp_tiled`,
//! `banshee_overlay_draw`, `banshee_hwcursor_draw`; Sarah Walker, GPLv2)
//! and section 8 of 3dfx's Voodoo3 specification. The picture is drawn
//! whole when the display asks, not a line at a time as 86Box does.

use super::super::mem::Vram;
use super::super::mirror::{BufferKey, Cursor, LayoutKey, Overlay, Scanout, Surface};
use super::*;

/// One tile: 128 bytes by 32 rows.
const TILE: u32 = 128 * 32;

/// A frame's worth of where things are, worked out from the registers.
#[derive(Clone, Copy, Debug, Default)]
struct Frame {
    cfg: u32,
    start: u32,
    width: usize,
    height: usize,
    /// Bytes from one desktop row to the next, or per row of tiles.
    stride: u32,
    tiled: bool,
    half: bool,
    /// Each pixel shown twice across (`Banshee::wide_pixels`).
    wide: bool,
}

impl Frame {
    /// The bytes of a surface of `rows` rows with `stride`: its stride
    /// times its rows, or its rows of tiles, each counted as the surface
    /// itself is laid out.
    fn span(stride: u32, tiled: bool, rows: usize) -> u64 {
        let rows = if tiled { rows.div_ceil(32) } else { rows };
        stride.max(1) as u64 * rows.max(1) as u64
    }

    /// The memory the desktop is shown from: its first byte, and the
    /// bytes of the rows `render_desktop` reads, half of the lines in
    /// half mode, which shows each row twice.
    fn desktop(&self) -> (u32, u64) {
        let rows = if self.half {
            self.height.div_ceil(2)
        } else {
            self.height
        };
        (self.start, Self::span(self.stride, self.tiled, rows))
    }
}

/// The byte address of byte `x` of row `y` of a surface at `start`.
#[inline]
fn surface_addr(start: u32, x: u32, y: u32, stride: u32, tiled: bool) -> u32 {
    if tiled {
        let row = (y & 31) * 128 + (y >> 5).wrapping_mul(stride);
        start
            .wrapping_add(row)
            .wrapping_add((x & 127) + (x >> 7).wrapping_mul(TILE))
    } else {
        start.wrapping_add(y.wrapping_mul(stride)).wrapping_add(x)
    }
}

#[inline]
fn byte(ram: &Vram, addr: u32, mask: u32) -> u8 {
    ram.byte((addr & mask) as usize)
}

#[inline]
fn word(ram: &Vram, addr: u32, mask: u32) -> u16 {
    ram.get(((addr & mask) >> 1) as usize)
}

#[inline]
fn rgb(v: u32) -> [u8; 3] {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// Each of the first `len` pixels of row `y` of a desktop of 16 bits a
/// pixel, with its x.
#[inline]
fn desktop_words(ram: &Vram, f: &Frame, y: usize, len: usize, each: impl FnMut(usize, u16)) {
    let line = if f.half { y as u32 / 2 } else { y as u32 };
    surface_words(ram, f.start, f.stride, f.tiled, line, len, each);
}

/// Each of the first `len` pixels of row `line` of a surface of 16 bits a
/// pixel at `start`, with its x: the words of a run (a row, or a tile's
/// 64 pixels) follow each other, so only its first needs the address.
#[inline]
fn surface_words(
    ram: &Vram,
    start: u32,
    stride: u32,
    tiled: bool,
    line: u32,
    len: usize,
    mut each: impl FnMut(usize, u16),
) {
    let mask = (ram.bytes() - 1) as u32;
    let words = mask >> 1;
    let mut x = 0;
    while x < len {
        let run = if tiled { 64 - (x & 63) } else { len - x };
        let end = (x + run).min(len);
        let mut w = (surface_addr(start, 2 * x as u32, line, stride, tiled) & mask) >> 1;
        for x in x..end {
            each(x, ram.get(w as usize));
            w = (w + 1) & words;
        }
        x = end;
    }
}

/// Spread each row of a picture drawn at the desktop's size over twice
/// the width (`Banshee::wide_pixels`), from its right end, so that no
/// pixel is overwritten before it is copied.
fn spread_wide(out: &mut [u8], pitch: usize, f: &Frame) {
    let width = f.width.min(pitch / 2);
    for y in 0..f.height {
        let Some(row) = out.get_mut(y * pitch * 4..(y * pitch + 2 * width) * 4) else {
            break;
        };
        let pixels = row.as_chunks_mut::<4>().0;
        for x in (0..width).rev() {
            let px = pixels[x];
            pixels[2 * x] = px;
            pixels[2 * x + 1] = px;
        }
    }
}

/// What the video processor shows, as the card's memory held it when
/// taken (`Banshee::take_shown`): the desktop's pixels at 16 bits, row by
/// row, unless the overlay covers all of it, those of the overlay over it
/// in its 16-bit formats, as many as it has on the screen, and what turns
/// them into colours then (the desktop's geometry, the overlay's
/// rectangle and the colour lookup table). Drawn later (`render`), they
/// make the picture `Banshee::render` drew when they were taken, for a
/// fraction of the work of drawing it.
#[derive(Clone, Debug, Default)]
pub struct ShownWords {
    frame: Frame,
    /// The desktop's half of the colour lookup table, or None with the
    /// table bypassed.
    clut: Option<[u32; 256]>,
    words: Vec<u16>,
    overlay: Option<ShownOverlay>,
}

/// The overlay's pixels on the screen (`ShownWords`): `cols` of them a
/// row from (`x`, `y`) on.
#[derive(Clone, Debug, Default)]
struct ShownOverlay {
    x: usize,
    y: usize,
    cols: usize,
    clut: Option<[u32; 256]>,
    words: Vec<u16>,
}

impl ShownWords {
    /// Draw the picture into `out` as `Banshee::render` does.
    pub fn render(&self, out: &mut [u8], pitch: usize) {
        let f = &self.frame;
        let clut = self.clut.as_ref().map(|c| &c[..]);
        for (y, words) in self.words.chunks(f.width.max(1)).enumerate().take(f.height) {
            let Some(row) = out.get_mut(y * pitch * 4..(y * pitch + f.width.min(pitch)) * 4) else {
                break;
            };
            for (px, &p) in row.as_chunks_mut::<4>().0.iter_mut().zip(words) {
                let [r, g, b] = rgb565(p, clut);
                *px = [r, g, b, 255];
            }
        }
        if let Some(o) = self.overlay.as_ref().filter(|o| o.cols > 0 && o.x < pitch) {
            let clut = o.clut.as_ref().map(|c| &c[..]);
            // The columns left of the output's right edge.
            let cols = o.cols.min(pitch - o.x);
            for (y, words) in (o.y..).zip(o.words.chunks(o.cols)) {
                let at = (y * pitch + o.x) * 4;
                // Over pixels of the desktop, which are opaque.
                let Some(row) = out.get_mut(at..(at + cols * 4).min(out.len())) else {
                    break;
                };
                for (px, &p) in row.as_chunks_mut::<4>().0.iter_mut().zip(words) {
                    let [r, g, b] = rgb565(p, clut);
                    *px = [r, g, b, 255];
                }
            }
        }
        if f.wide {
            spread_wide(out, pitch, f);
        }
    }
}

/// A 5-6-5 pixel's colour, through `clut` if there is one.
#[inline]
fn rgb565(p: u16, clut: Option<&[u32]>) -> [u8; 3] {
    let r = ((p >> 8) & 0xF8) as u8 | (p >> 13) as u8;
    let g = ((p >> 3) & 0xFC) as u8 | ((p >> 9) & 3) as u8;
    let b = ((p << 3) & 0xF8) as u8 | ((p >> 2) & 7) as u8;
    match clut {
        Some(c) => [
            (c[r as usize] >> 16) as u8,
            (c[g as usize] >> 8) as u8,
            c[b as usize] as u8,
        ],
        None => [r, g, b],
    }
}

/// YCbCr to RGB, ITU-R BT.601.
fn yuv(y: u8, u: u8, v: u8) -> [u8; 3] {
    let (y, u, v) = (y as i32 - 16, u as i32 - 128, v as i32 - 128);
    let c = |x: i32| (x >> 8).clamp(0, 255) as u8;
    [
        c(298 * y + 409 * v + 128),
        c(298 * y - 100 * u - 208 * v + 128),
        c(298 * y + 516 * u + 128),
    ]
}

impl Banshee {
    fn frame(&self) -> Frame {
        let cfg = self.reg(VID_PROC_CFG);
        let (w, h) = self.desktop_size();
        let tiled = cfg & VIDPROC_DESKTOP_TILED != 0;
        let stride = self.reg(VID_DESKTOP_OVERLAY_STRIDE) & 0x3FFF;
        Frame {
            cfg,
            start: self.reg(VID_DESKTOP_START_ADDR),
            width: w as usize,
            height: h as usize,
            stride: if tiled { stride * TILE } else { stride },
            tiled,
            half: cfg & VIDPROC_HALF_MODE != 0,
            wide: narrow(w),
        }
    }

    /// Whether a surface from byte `addr` on, as big as the desktop, is
    /// in memory the picture shows: the desktop's or the overlay's.
    pub fn shows(&self, addr: u32) -> bool {
        self.shows_bytes(addr, None)
    }

    /// The memory the desktop is shown from: its first byte, and the
    /// bytes its rows (or rows of tiles) take from there.
    pub fn desktop_memory(&self) -> (u32, u64) {
        self.frame().desktop()
    }

    /// Whether any of the `len` bytes from byte `addr` on (with None, a
    /// surface as big as the desktop) is in memory the picture shows.
    pub fn shows_bytes(&self, addr: u32, len: Option<u64>) -> bool {
        let f = self.frame();
        let desktop = f.desktop();
        let overlay = (f.cfg & VIDPROC_OVERLAY_ENABLE != 0).then(|| {
            let tiled = f.cfg & VIDPROC_OVERLAY_TILED != 0;
            let stride = (self.reg(VID_DESKTOP_OVERLAY_STRIDE) >> 16) & 0x7FFF;
            let stride = if tiled { stride * TILE } else { stride };
            (self.overlay_addr, Frame::span(stride, tiled, f.height))
        });
        // The surface drawn, as big as the bigger of the two.
        let drawn = len.unwrap_or_else(|| overlay.map_or(desktop.1, |o| o.1.max(desktop.1)));
        let a = addr as u64;
        [Some(desktop), overlay]
            .into_iter()
            .flatten()
            .any(|(start, span)| a < start as u64 + span && (start as u64) < a + drawn)
    }

    /// Draw the desktop, the overlay and the cursor into `out` (RGBA8,
    /// `pitch` pixels a row), as big as `picture_size` says: each pixel
    /// twice across with `wide_pixels`.
    pub fn render(&self, ram: &Vram, out: &mut [u8], pitch: usize) {
        let f = self.frame();
        self.render_desktop(ram, out, pitch, &f);
        if f.wide {
            spread_wide(out, pitch, &f);
        }
    }

    /// Take what `render` would draw now into `into`, as the memory's
    /// words, where the desktop has 16 bits a pixel, the overlay, if on,
    /// one of its 16-bit formats, and the cursor is off: false, with
    /// `into` as it was, otherwise.
    pub fn take_shown(&self, ram: &Vram, into: &mut ShownWords) -> bool {
        let f = self.frame();
        let overlay = f.cfg & VIDPROC_OVERLAY_ENABLE != 0;
        if (f.cfg >> 18) & 7 != 1
            || f.cfg & VIDPROC_CURSOR != 0
            || overlay && matches!((f.cfg >> 21) & 7, 0 | 5 | 6)
        {
            return false;
        }
        let clut = |bypass: u32, select: u32| {
            (f.cfg & bypass == 0).then(|| {
                let half = if f.cfg & select != 0 { 256 } else { 0 };
                let mut clut = [0; 256];
                clut.copy_from_slice(&self.clut[half..half + 256]);
                clut
            })
        };
        into.frame = f;
        into.clut = clut(VIDPROC_DESKTOP_CLUT_BYPASS, VIDPROC_DESKTOP_CLUT_SEL);
        let mut shown = into.overlay.take().unwrap_or_default();
        into.overlay = overlay.then(|| {
            let (stride, tiled) = self.overlay_surface(&f);
            shown.words.clear();
            shown.cols = 0;
            let mut first = None;
            self.overlay_rows(&f, |y, xs, sy, dudx| {
                let cols = xs.end().saturating_sub(*xs.start()) + 1;
                if xs.is_empty() {
                    return;
                }
                first.get_or_insert((*xs.start(), y));
                shown.cols = cols;
                let words = &mut shown.words;
                if dudx == 1 << 20 {
                    let start = self.overlay_addr;
                    surface_words(ram, start, stride, tiled, sy, cols, |_, p| words.push(p));
                } else {
                    let mask = (ram.bytes() - 1) as u32;
                    let mut u = 0u64;
                    for _ in xs {
                        let sx = (u >> 20) as u32;
                        u += dudx as u64;
                        let at = surface_addr(self.overlay_addr, sx * 2, sy, stride, tiled);
                        words.push(word(ram, at, mask));
                    }
                }
            });
            (shown.x, shown.y) = first.unwrap_or_default();
            shown.clut = clut(VIDPROC_OVERLAY_CLUT_BYPASS, VIDPROC_OVERLAY_CLUT_SEL);
            shown
        });
        into.words.clear();
        let covered = into.overlay.as_ref().is_some_and(|o| {
            (o.x, o.y, o.cols) == (0, 0, f.width) && o.words.len() == f.width * f.height
        });
        if !covered {
            // `desktop_words` gives each row's pixels from left to right.
            into.words.reserve(f.width * f.height);
            for y in 0..f.height {
                desktop_words(ram, &f, y, f.width, |_, p| into.words.push(p));
            }
        }
        true
    }

    /// Draw the desktop, the overlay and the cursor into `out` (RGBA8,
    /// `pitch` pixels a row), as big as `desktop_size` says.
    fn render_desktop(&self, ram: &Vram, out: &mut [u8], pitch: usize, f: &Frame) {
        let mask = (ram.bytes() - 1) as u32;
        let desktop_clut = &self.clut[if f.cfg & VIDPROC_DESKTOP_CLUT_SEL != 0 {
            256
        } else {
            0
        }..][..256];
        let bypass = f.cfg & VIDPROC_DESKTOP_CLUT_BYPASS != 0;
        let format = (f.cfg >> 18) & 7;
        for y in 0..f.height {
            let Some(row) = out.get_mut(y * pitch * 4..(y * pitch + f.width.min(pitch)) * 4) else {
                break;
            };
            let line = if f.half { y as u32 / 2 } else { y as u32 };
            let at = |x: u32| surface_addr(f.start, x, line, f.stride, f.tiled);
            if format == 1 {
                // 16 bits a pixel, the usual desktop and what the 3D engine
                // shows.
                let clut = (!bypass).then_some(desktop_clut);
                let pixels = row.as_chunks_mut::<4>().0;
                desktop_words(ram, f, y, pixels.len(), |x, p| {
                    let [r, g, b] = rgb565(p, clut);
                    pixels[x] = [r, g, b, 255];
                });
                continue;
            }
            for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let x = x as u32;
                let [r, g, b] = match format {
                    0 => {
                        let i = byte(ram, at(x), mask) as usize;
                        if bypass {
                            [i as u8; 3]
                        } else {
                            rgb(desktop_clut[i])
                        }
                    }
                    2 => {
                        let a = 3 * x;
                        [
                            byte(ram, at(a + 2), mask),
                            byte(ram, at(a + 1), mask),
                            byte(ram, at(a), mask),
                        ]
                    }
                    _ => {
                        let a = 4 * x;
                        [
                            byte(ram, at(a + 2), mask),
                            byte(ram, at(a + 1), mask),
                            byte(ram, at(a), mask),
                        ]
                    }
                };
                *px = [r, g, b, 255];
            }
        }
        if f.cfg & VIDPROC_OVERLAY_ENABLE != 0 {
            self.render_overlay(ram, out, pitch, f);
        }
        if f.cfg & VIDPROC_CURSOR != 0 {
            self.render_cursor(ram, out, pitch, f);
        }
    }

    /// The overlay's surface: bytes from one row (or row of tiles) to the
    /// next, and whether it is tiled.
    fn overlay_surface(&self, f: &Frame) -> (u32, bool) {
        let tiled = f.cfg & VIDPROC_OVERLAY_TILED != 0;
        let pitch_reg = (self.reg(VID_DESKTOP_OVERLAY_STRIDE) >> 16) & 0x7FFF;
        (if tiled { pitch_reg * TILE } else { pitch_reg }, tiled)
    }

    /// Each row of the screen the overlay covers, as far as the desktop
    /// goes: its y, the x of its pixels there, the row of the overlay
    /// surface shown there, and the step from one of its pixels to the
    /// next on the screen (in 1/2^20 pixels), as the scale bits say.
    fn overlay_rows(
        &self,
        f: &Frame,
        mut each: impl FnMut(usize, std::ops::RangeInclusive<usize>, u32, u32),
    ) {
        let start = self.reg(VID_OVERLAY_START_COORDS);
        let end = self.reg(VID_OVERLAY_END_SCREEN_COORDS);
        let (x0, y0) = ((start & 0xFFF) as usize, ((start >> 12) & 0xFFF) as usize);
        let (x1, y1) = ((end & 0xFFF) as usize, ((end >> 12) & 0xFFF) as usize);
        if x1 < x0 || y1 < y0 {
            return;
        }
        let dudx = if f.cfg & VIDPROC_H_SCALE != 0 {
            self.reg(VID_OVERLAY_DUDX) & 0xF_FFFF
        } else {
            1 << 20
        };
        let dvdy = if f.cfg & VIDPROC_V_SCALE != 0 {
            self.reg(VID_OVERLAY_DVDY) & 0xF_FFFF
        } else {
            1 << 20
        };
        let mut v = (self.reg(VID_OVERLAY_DVDY_OFFSET) & 0x7_FFFF) as u64;
        for y in y0..=y1.min(f.height.saturating_sub(1)) {
            let sy = (v >> 20) as u32;
            v += dvdy as u64;
            each(y, x0..=x1.min(f.width.saturating_sub(1)), sy, dudx);
        }
    }

    /// Each pixel of the screen the overlay covers (`overlay_rows`): its x
    /// and y, and those of the overlay surface's pixel shown there.
    fn overlay_walk(&self, f: &Frame, mut each: impl FnMut(usize, usize, u32, u32)) {
        self.overlay_rows(f, |y, xs, sy, dudx| {
            let mut u = 0u64;
            for x in xs {
                let sx = (u >> 20) as u32;
                u += dudx as u64;
                each(x, y, sx, sy);
            }
        });
    }

    /// The overlay: a rectangle of the screen from another surface, scaled
    /// if the scale bits say, in 5-6-5 or YUV 4:2:2.
    fn render_overlay(&self, ram: &Vram, out: &mut [u8], pitch: usize, f: &Frame) {
        let mask = (ram.bytes() - 1) as u32;
        let (stride, tiled) = self.overlay_surface(f);
        let clut = &self.clut[if f.cfg & VIDPROC_OVERLAY_CLUT_SEL != 0 {
            256
        } else {
            0
        }..][..256];
        let bypass = f.cfg & VIDPROC_OVERLAY_CLUT_BYPASS != 0;
        let format = (f.cfg >> 21) & 7;
        self.overlay_walk(f, |x, y, sx, sy| {
            if x >= pitch {
                return;
            }
            let at = |b: u32| surface_addr(self.overlay_addr, b, sy, stride, tiled);
            let px = match format {
                5 | 6 => {
                    // Two pixels in four bytes: YUYV or UYVY.
                    let pair = at((sx & !1) * 2);
                    let b = [0, 1, 2, 3].map(|i| byte(ram, pair + i, mask));
                    let (y_, u_, v_) = if format == 5 {
                        ([b[0], b[2]], b[1], b[3])
                    } else {
                        ([b[1], b[3]], b[0], b[2])
                    };
                    yuv(y_[(sx & 1) as usize], u_, v_)
                }
                0 => rgb(clut[byte(ram, at(sx), mask) as usize]),
                _ => rgb565(word(ram, at(sx * 2), mask), (!bypass).then_some(clut)),
            };
            if let Some(o) = out.get_mut((y * pitch + x) * 4..(y * pitch + x) * 4 + 3) {
                o.copy_from_slice(&px);
            }
        });
    }

    /// The 64x64 hardware cursor: two bit planes, 16 bytes a row, in
    /// Windows or X11 mode (86Box's `banshee_hwcursor_draw`).
    fn render_cursor(&self, ram: &Vram, out: &mut [u8], pitch: usize, f: &Frame) {
        let mask = (ram.bytes() - 1) as u32;
        let loc = self.reg(HW_CUR_LOC);
        let (cx, cy) = ((loc & 0x7FF) as i32 - 64, ((loc >> 16) & 0x7FF) as i32 - 64);
        let pattern = self.reg(HW_CUR_PAT_ADDR) & 0xFF_FFF0;
        let (c0, c1) = (rgb(self.reg(HW_CUR_C0)), rgb(self.reg(HW_CUR_C1)));
        let x11 = f.cfg & VIDPROC_CURSOR_X11 != 0;
        for row in 0..64 {
            let y = cy + row;
            if y < 0 || y as usize >= f.height {
                continue;
            }
            let base = pattern + row as u32 * 16;
            for col in 0..64 {
                let x = cx + col;
                if x < 0 || x as usize >= f.width || x as usize >= pitch {
                    continue;
                }
                let bit = 7 - (col & 7);
                let p0 = byte(ram, base + (col >> 3) as u32, mask) >> bit & 1;
                let p1 = byte(ram, base + 8 + (col >> 3) as u32, mask) >> bit & 1;
                let at = (y as usize * pitch + x as usize) * 4;
                let Some(o) = out.get_mut(at..at + 3) else {
                    continue;
                };
                if x11 {
                    if p0 != 0 {
                        o.copy_from_slice(if p1 != 0 { &c1 } else { &c0 });
                    }
                } else if p0 == 0 {
                    o.copy_from_slice(if p1 != 0 { &c1 } else { &c0 });
                } else if p1 != 0 {
                    for c in o.iter_mut() {
                        *c ^= 0xFF;
                    }
                }
            }
        }
    }
}

/// The bytes from a surface's first that hold the first `width_bytes` of
/// each of its first `rows` rows.
fn surface_len(width_bytes: u32, rows: u32, stride: u32, tiled: bool) -> u64 {
    let (width_bytes, rows, stride) =
        (width_bytes.max(1) as u64, rows.max(1) as u64, stride as u64);
    if tiled {
        (rows.div_ceil(32) - 1) * stride + width_bytes.div_ceil(128) * TILE as u64
    } else {
        (rows - 1) * stride + width_bytes
    }
}

impl Voodoo {
    /// What the video processor shows, for the OpenGL renderer to compose
    /// (`mirror::Scanout`), `key` the buffers it draws. A surface that is
    /// one of them, in 5-6-5 and with its stride, comes from its drawing.
    /// The other surfaces come from the renderer's copy of the card's
    /// memory, which gets the pages shown that were written since it last
    /// had them (`tracker`), first in the recording.
    pub(crate) fn banshee_scanout(&mut self, key: &LayoutKey) -> Option<Box<Scanout>> {
        let held = self.picture_held();
        let b = self.banshee.as_deref_mut()?;
        let dirty = std::mem::take(&mut b.dirty)
            | std::mem::take(&mut self.display_dirty)
            | std::mem::take(&mut self.scanout_dirty);
        let b = self.banshee.as_deref()?;
        let f = b.frame();
        let buffer = |start: u32, stride: u32, tiled: bool| {
            let same = |b: &&BufferKey| {
                b.stride.tiled == tiled && b.stride.words as u64 * 2 == stride as u64
            };
            key.color_at(start).filter(same).map(|_| start / 2)
        };
        let clut_half = |select: u32| if f.cfg & select != 0 { 256 } else { 0 };
        let format = (f.cfg >> 18) & 7;
        let desktop = Surface {
            start: f.start,
            stride: f.stride,
            tiled: f.tiled,
            format,
            clut: clut_half(VIDPROC_DESKTOP_CLUT_SEL),
            bypass: f.cfg & VIDPROC_DESKTOP_CLUT_BYPASS != 0,
            buffer: if format == 1 {
                buffer(f.start, f.stride, f.tiled)
            } else {
                None
            },
        };
        let (w, h) = (f.width as u32, f.height as u32);
        let overlay = (f.cfg & VIDPROC_OVERLAY_ENABLE != 0).then(|| {
            let start = b.reg(VID_OVERLAY_START_COORDS);
            let end = b.reg(VID_OVERLAY_END_SCREEN_COORDS);
            let tiled = f.cfg & VIDPROC_OVERLAY_TILED != 0;
            let stride = (b.reg(VID_DESKTOP_OVERLAY_STRIDE) >> 16) & 0x7FFF;
            let stride = if tiled { stride * TILE } else { stride };
            let format = (f.cfg >> 21) & 7;
            let step = |scaled: u32, reg: u32| {
                if f.cfg & scaled != 0 {
                    b.reg(reg) & 0xF_FFFF
                } else {
                    1 << 20
                }
            };
            let rgb = !matches!(format, 0 | 5 | 6);
            Overlay {
                surface: Surface {
                    start: b.overlay_addr,
                    stride,
                    tiled,
                    format,
                    clut: clut_half(VIDPROC_OVERLAY_CLUT_SEL),
                    bypass: f.cfg & VIDPROC_OVERLAY_CLUT_BYPASS != 0,
                    buffer: if rgb {
                        buffer(b.overlay_addr, stride, tiled)
                    } else {
                        None
                    },
                },
                rect: [
                    start & 0xFFF,
                    (start >> 12) & 0xFFF,
                    end & 0xFFF,
                    (end >> 12) & 0xFFF,
                ],
                dudx: step(VIDPROC_H_SCALE, VID_OVERLAY_DUDX),
                dvdy: step(VIDPROC_V_SCALE, VID_OVERLAY_DVDY),
                v_offset: b.reg(VID_OVERLAY_DVDY_OFFSET) & 0x7_FFFF,
            }
        });
        let (ram, mask) = (&self.fbi.ram, self.fbi.mask);
        let cursor = (f.cfg & VIDPROC_CURSOR != 0).then(|| {
            let loc = b.reg(HW_CUR_LOC);
            let pattern = b.reg(HW_CUR_PAT_ADDR) & 0xFF_FFF0;
            Cursor {
                x: (loc & 0x7FF) as i32 - 64,
                y: ((loc >> 16) & 0x7FF) as i32 - 64,
                x11: f.cfg & VIDPROC_CURSOR_X11 != 0,
                colors: [b.reg(HW_CUR_C0) & 0xFF_FFFF, b.reg(HW_CUR_C1) & 0xFF_FFFF],
                pattern: (0..1024).map(|i| byte(ram, pattern + i, mask)).collect(),
            }
        });
        // The memory the surfaces not drawn by the renderer show; none for
        // a desktop the overlay covers whole.
        let covered = overlay.as_ref().is_some_and(|o| {
            let [x0, y0, x1, y1] = o.rect;
            x0 == 0 && y0 == 0 && x1 + 1 >= w && y1 + 1 >= h
        });
        let mut spans = [None, None];
        if desktop.buffer.is_none() && !covered {
            let bpp = [1, 2, 3, 4][format.min(3) as usize];
            let rows = if f.half { h.div_ceil(2) } else { h };
            spans[0] = Some((f.start, surface_len(w * bpp, rows, f.stride, f.tiled)));
        }
        if let Some(o) = overlay.as_ref().filter(|o| o.surface.buffer.is_none()) {
            let [x0, y0, x1, y1] = o.rect;
            let (x1, y1) = (x1.min(w.saturating_sub(1)), y1.min(h.saturating_sub(1)));
            if x0 <= x1 && y0 <= y1 {
                // The source pixels it reaches, a YUV pair's second too.
                let across = (((x1 - x0) as u64 * o.dudx as u64) >> 20) as u32 + 2;
                let down = (o.v_offset as u64 + (y1 - y0) as u64 * o.dvdy as u64) >> 20;
                let bpp = if o.surface.format == 0 { 1 } else { 2 };
                let s = &o.surface;
                spans[1] = Some((
                    s.start,
                    surface_len(across * bpp, down as u32 + 1, s.stride, s.tiled),
                ));
            }
        }
        let bytes = ram.bytes() as u64;
        let spans = spans.map(|span| {
            span.map(|(start, len): (u32, u64)| {
                (start, start + len.min(bytes.saturating_sub(start as u64)) as u32)
            })
        });
        self.gpu_resend_dropped();
        // The 3D engine draws into its buffers without marking the pages
        // it changes: memory shown that shares pages with them goes again
        // whenever the picture may have changed, after the drawing queued.
        // The rows of the 3D buffers read back for it turn hot, as for a
        // program's read: the scanout reads them again every frame the
        // picture changes, which costs a read back from the GPU each
        // time, while the software rasterizer leaves them in the memory.
        // `gpu_takes` refuses the GPU while the software picture is
        // wanted for the same reason.
        let shared: Vec<(u32, u32)> = spans
            .iter()
            .flatten()
            .copied()
            .filter(|&(start, end)| key.overlaps(start, end))
            .collect();
        if dirty && !shared.is_empty() {
            if self.writes_queue() {
                self.flush_in(&shared);
            }
            for &(start, end) in &shared {
                self.gpu_sync(start, end);
                self.gpu.get_mut().tracker.mark(start, end);
            }
        }
        // The pages shown that the renderer lacks.
        for (start, end) in spans.into_iter().flatten() {
            self.send_pages(start, end);
        }
        let b = self.banshee.as_deref()?;
        Some(Box::new(Scanout {
            width: w,
            height: h,
            half: f.half,
            wide: f.wide,
            desktop,
            overlay,
            cursor,
            clut: b.clut.to_vec(),
            held,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card_1024x768(cfg: u32) -> Banshee {
        let mut b = Banshee::new(Kind::Voodoo3);
        b.crtc[0x01] = 127;
        b.crtc[0x12] = 0xFF;
        b.crtc[0x07] = 0x40;
        b.ext_write(VGA_INIT0, VGAINIT0_EXTENDED_SHIFT_OUT);
        b.ext_write(VID_PROC_CFG, VIDPROC_ENABLE | cfg);
        b
    }

    #[test]
    fn a_16_bit_linear_desktop() {
        let ram = Vram::new(MEMORY);
        let mut b = card_1024x768(1 << 18 | VIDPROC_DESKTOP_CLUT_BYPASS);
        b.ext_write(VID_DESKTOP_START_ADDR, 0x1000);
        b.ext_write(VID_DESKTOP_OVERLAY_STRIDE, 2048);
        // Pixel (3, 2): pure red.
        ram.set((0x1000 + 2 * 2048 + 3 * 2) / 2, 0xF800);
        let mut out = [0, 0, 0, 255].repeat(1024 * 768);
        b.render(&ram, &mut out, 1024);
        let at = (2 * 1024 + 3) * 4;
        assert_eq!(&out[at..at + 3], &[0xFF, 0, 0]);
        assert_eq!(&out[at + 4..at + 7], &[0, 0, 0]);
    }

    #[test]
    fn the_bytes_of_the_desktops_picture() {
        // 1024x768 at 16 bits a pixel: 1.5 MB, its rows one after another.
        let b = card_1024x768(1 << 18);
        assert_eq!(b.desktop_bytes_per_pixel(), 2);
        assert_eq!(b.desktop_bytes(), 1024 * 768 * 2);
        assert!(b.desktop_linear());
        // At 32 bits, 3 MB; half mode shows each of its 384 rows twice.
        let b = card_1024x768(3 << 18 | VIDPROC_HALF_MODE);
        assert_eq!(b.desktop_bytes(), 1024 * 384 * 4);
        // Tiled, its rows aren't one after another.
        assert!(!card_1024x768(1 << 18 | VIDPROC_DESKTOP_TILED).desktop_linear());
    }

    #[test]
    fn what_the_picture_shows() {
        // A 1024x768 desktop at 1 MB, 2048 bytes a row: 1.5 MB of it.
        let mut b = card_1024x768(1 << 18);
        b.ext_write(VID_DESKTOP_START_ADDR, 0x10_0000);
        b.ext_write(VID_DESKTOP_OVERLAY_STRIDE, 2048);
        assert!(b.shows(0x10_0000));
        assert!(b.shows(0x20_0000));
        assert!(!b.shows(0x28_0000));
        // The overlay's buffer, while the overlay is on.
        b.overlay_addr = 0x40_0000;
        assert!(!b.shows(0x40_0000));
        b.ext_write(
            VID_PROC_CFG,
            VIDPROC_ENABLE | 1 << 18 | VIDPROC_OVERLAY_ENABLE,
        );
        assert!(b.shows(0x40_0000));
    }

    #[test]
    fn a_half_mode_desktop_is_half_its_lines() {
        // 768 lines of 2048 bytes, each row shown twice: 384 rows, and the
        // memory after them (a back buffer) isn't the desktop's.
        let mut b = card_1024x768(1 << 18 | VIDPROC_HALF_MODE);
        b.ext_write(VID_DESKTOP_START_ADDR, 0x10_0000);
        b.ext_write(VID_DESKTOP_OVERLAY_STRIDE, 2048);
        assert_eq!(b.desktop_memory(), (0x10_0000, 384 * 2048));
        assert!(b.shows_bytes(0x10_0000 + 383 * 2048, Some(2048)));
        assert!(!b.shows_bytes(0x10_0000 + 384 * 2048, Some(2048)));
    }

    #[test]
    fn a_tiled_desktop_and_the_clut() {
        let ram = Vram::new(MEMORY);
        let mut b = card_1024x768(VIDPROC_DESKTOP_TILED);
        // 8 tiles of 128 bytes a row: 1024 pixels of 8 bits.
        b.ext_write(VID_DESKTOP_OVERLAY_STRIDE, 8);
        b.clut[7] = 0x11_2233;
        // Pixel (130, 33): tile 1 of the second row of tiles, row 1.
        ram.set_byte((8 * TILE + TILE + 128 + 2) as usize, 7);
        let mut out = [0, 0, 0, 255].repeat(1024 * 768);
        b.render(&ram, &mut out, 1024);
        let at = (33 * 1024 + 130) * 4;
        assert_eq!(&out[at..at + 3], &[0x11, 0x22, 0x33]);
    }

    /// Random words over the first 4 MB of `ram` and its last 128 KB, from
    /// `seed`.
    fn fill(ram: &Vram, seed: u32) {
        let mut x = seed | 1;
        let words = ram.bytes() / 2;
        for i in (0..2 << 20).chain(words - (64 << 10)..words) {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            ram.set(i, x as u16);
        }
    }

    #[test]
    fn the_words_taken_draw_the_picture_of_when_they_were_taken() {
        let narrow = |cfg: u32| {
            let mut b = card_1024x768(cfg | VIDPROC_HALF_MODE);
            b.crtc[0x01] = 39;
            b.crtc[0x12] = 0x8F;
            b.crtc[0x07] = 0x02;
            b
        };
        let overlay = 1 << 18 | 1 << 21 | VIDPROC_OVERLAY_ENABLE;
        // The overlay's rectangle: over all of the desktop and past its
        // edges, over part of it, a single pixel, off the desktop, and
        // with its ends before its starts.
        let (all, part) = ((0, 0xFFF | 0xFFF << 12), (100 | 50 << 12, 899 | 649 << 12));
        let dot = (10 | 10 << 12, 10 | 10 << 12);
        let off = (1100 | 800 << 12, 0xFFF | 0xFFF << 12);
        let inverted = (500 | 500 << 12, 100 | 100 << 12);
        // The desktop's and the overlay's start: low in the memory, or
        // close enough to its end that their rows run past it.
        let low = (0x1_0002, 0x20_0006);
        let end = (MEMORY as u32 - 0x1000 + 2, MEMORY as u32 - 0x800 + 6);
        let cards = [
            (card_1024x768(1 << 18), 2048, all, low),
            (
                card_1024x768(1 << 18 | VIDPROC_DESKTOP_CLUT_BYPASS),
                2048,
                all,
                low,
            ),
            (
                card_1024x768(1 << 18 | VIDPROC_DESKTOP_CLUT_SEL),
                2050,
                all,
                low,
            ),
            (card_1024x768(1 << 18 | VIDPROC_DESKTOP_TILED), 16, all, low),
            (card_1024x768(1 << 18 | VIDPROC_HALF_MODE), 2048, all, low),
            (narrow(1 << 18), 640, all, low),
            // The overlay over part of the desktop, and over all of it,
            // tiled and scaled, past its right and bottom edges.
            (card_1024x768(overlay), 2048, part, low),
            (
                card_1024x768(
                    overlay
                        | VIDPROC_OVERLAY_TILED
                        | VIDPROC_OVERLAY_CLUT_SEL
                        | VIDPROC_H_SCALE
                        | VIDPROC_V_SCALE,
                ),
                2048,
                all,
                low,
            ),
            (
                narrow(1 << 18 | 3 << 21 | VIDPROC_OVERLAY_ENABLE),
                640,
                all,
                low,
            ),
            // Test Drive 4's: the overlay tiled and unscaled over all of
            // the desktop, its rows copied in runs of a tile's pixels.
            (
                card_1024x768(
                    overlay
                        | VIDPROC_OVERLAY_TILED
                        | VIDPROC_DESKTOP_CLUT_BYPASS
                        | VIDPROC_OVERLAY_CLUT_SEL,
                ),
                2048,
                all,
                low,
            ),
            // One of the two through the table, the other bypassing it.
            (
                card_1024x768(overlay | VIDPROC_OVERLAY_CLUT_BYPASS),
                2048,
                part,
                low,
            ),
            (
                card_1024x768(overlay | VIDPROC_DESKTOP_CLUT_BYPASS),
                2048,
                part,
                low,
            ),
            // Rows that run past the end of the memory, linear and tiled,
            // the overlay unscaled and scaled across.
            (card_1024x768(overlay), 2048, part, end),
            (
                card_1024x768(overlay | VIDPROC_DESKTOP_TILED | VIDPROC_OVERLAY_TILED),
                16,
                part,
                end,
            ),
            (card_1024x768(overlay | VIDPROC_H_SCALE), 2048, part, end),
            // Overlays with no pixel, or one, on the desktop.
            (card_1024x768(overlay), 2048, dot, low),
            (card_1024x768(overlay), 2048, off, low),
            (card_1024x768(overlay), 2048, inverted, low),
        ];
        let ram = Vram::new(MEMORY);
        for (i, (mut b, stride, (from, to), (desktop, at))) in cards.into_iter().enumerate() {
            b.ext_write(VID_DESKTOP_START_ADDR, desktop);
            let tiled = b.reg(VID_PROC_CFG) & VIDPROC_OVERLAY_TILED != 0;
            let overlay_stride = if tiled { 16 } else { 1400 };
            b.ext_write(VID_DESKTOP_OVERLAY_STRIDE, overlay_stride << 16 | stride);
            b.overlay_addr = at;
            b.ext_write(VID_OVERLAY_START_COORDS, from);
            b.ext_write(VID_OVERLAY_END_SCREEN_COORDS, to);
            b.ext_write(VID_OVERLAY_DUDX, 0x8_0000);
            b.ext_write(VID_OVERLAY_DVDY, 0xA_0000);
            b.ext_write(VID_OVERLAY_DVDY_OFFSET, 0x4_0000);
            for (n, c) in b.clut.iter_mut().enumerate() {
                *c = (n as u32 * 0x01_0305) ^ 0x5A_A55A;
            }
            fill(&ram, i as u32);
            let (w, h) = b.picture_size();
            // As wide as the picture, and narrower than it.
            for pitch in [w as usize, w as usize - 100] {
                let mut then = [1, 2, 3, 4].repeat(pitch * h as usize);
                b.render(&ram, &mut then, pitch);
                let mut words = ShownWords::default();
                assert!(b.take_shown(&ram, &mut words), "card {i}");
                // What the memory, the table and the registers hold later
                // doesn't count.
                let saved = b.clone();
                fill(&ram, i as u32 + 100);
                b.clut[300] ^= 0xFF_FFFF;
                b.clut[30] ^= 0xFF_FFFF;
                b.overlay_addr += 0x1000;
                b.ext_write(VID_DESKTOP_START_ADDR, desktop + 0x2000);
                b.ext_write(VID_DESKTOP_OVERLAY_STRIDE, 300 << 16 | 1200);
                b.ext_write(VID_OVERLAY_START_COORDS, 7 | 9 << 12);
                b.ext_write(VID_OVERLAY_END_SCREEN_COORDS, 300 | 200 << 12);
                b.ext_write(VID_OVERLAY_DUDX, 0x3_0000);
                b.ext_write(VID_OVERLAY_DVDY, 0x5_0000);
                b.ext_write(VID_OVERLAY_DVDY_OFFSET, 0x7_0000);
                b.ext_write(VID_PROC_CFG, VIDPROC_ENABLE | 3 << 18);
                b.crtc[0x01] = 79;
                let mut later = [1, 2, 3, 4].repeat(pitch * h as usize);
                words.render(&mut later, pitch);
                assert!(then == later, "card {i}, pitch {pitch}");
                b = saved;
                fill(&ram, i as u32);
            }
        }
    }

    #[test]
    fn only_16_bit_pictures_with_no_cursor_are_taken_as_words() {
        let ram = Vram::new(MEMORY);
        let mut words = ShownWords::default();
        for cfg in [
            0,
            3 << 18,
            1 << 18 | VIDPROC_CURSOR,
            1 << 18 | VIDPROC_OVERLAY_ENABLE,
            1 << 18 | 5 << 21 | VIDPROC_OVERLAY_ENABLE,
        ] {
            let b = card_1024x768(cfg);
            assert!(!b.take_shown(&ram, &mut words), "{cfg:08X}");
        }
    }

    #[test]
    fn the_cursor_in_windows_mode() {
        let ram = Vram::new(MEMORY);
        let mut b = card_1024x768(VIDPROC_CURSOR | 1 << 18);
        b.ext_write(HW_CUR_PAT_ADDR, 0x10_0000);
        b.ext_write(HW_CUR_LOC, (64 + 10) | (64 + 20) << 16);
        b.ext_write(HW_CUR_C1, 0x00_FF00);
        // Row 0: the first pixel colour 1 (plane 0 clear, plane 1 set),
        // the second inverted (both set: black to white), the rest
        // transparent (plane 0 set, plane 1 clear).
        ram.set_byte(0x10_0000, 0x7F);
        ram.set_byte(0x10_0008, 0xC0);
        for i in 1..8 {
            ram.set_byte(0x10_0000 + i, 0xFF);
        }
        let mut out = [0, 0, 0, 255].repeat(1024 * 768);
        b.render(&ram, &mut out, 1024);
        let at = (20 * 1024 + 10) * 4;
        assert_eq!(&out[at..at + 3], &[0, 0xFF, 0]);
        assert_eq!(&out[at + 4..at + 7], &[0xFF, 0xFF, 0xFF]);
        // Plane 0 set, plane 1 clear: transparent.
        assert_eq!(&out[at + 8..at + 11], &[0, 0, 0]);
    }
}

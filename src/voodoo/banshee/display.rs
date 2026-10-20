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
use super::*;

/// One tile: 128 bytes by 32 rows.
const TILE: u32 = 128 * 32;

/// A frame's worth of where things are, worked out from the registers.
struct Frame {
    cfg: u32,
    start: u32,
    width: usize,
    height: usize,
    /// Bytes from one desktop row to the next, or per row of tiles.
    stride: u32,
    tiled: bool,
    half: bool,
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
        }
    }

    /// Whether a surface from byte `addr` on, as big as the desktop, is
    /// in memory the picture shows: the desktop's or the overlay's.
    pub fn shows(&self, addr: u32) -> bool {
        let f = self.frame();
        // A surface's bytes: its stride times its rows, or its rows of
        // tiles, each counted as the surface itself is laid out.
        let span = |stride: u32, tiled: bool| {
            let rows = if tiled {
                f.height.div_ceil(32)
            } else {
                f.height
            };
            stride.max(1) as u64 * rows.max(1) as u64
        };
        let desktop = (f.start, span(f.stride, f.tiled));
        let overlay = (f.cfg & VIDPROC_OVERLAY_ENABLE != 0).then(|| {
            let tiled = f.cfg & VIDPROC_OVERLAY_TILED != 0;
            let stride = (self.reg(VID_DESKTOP_OVERLAY_STRIDE) >> 16) & 0x7FFF;
            let stride = if tiled { stride * TILE } else { stride };
            (self.overlay_addr, span(stride, tiled))
        });
        // The surface drawn, as big as the bigger of the two.
        let drawn = overlay.map_or(desktop.1, |o| o.1.max(desktop.1));
        let a = addr as u64;
        [Some(desktop), overlay]
            .into_iter()
            .flatten()
            .any(|(start, span)| a < start as u64 + span && (start as u64) < a + drawn)
    }

    /// Draw the desktop, the overlay and the cursor into `out` (RGBA8,
    /// `pitch` pixels a row), as big as `desktop_size` says.
    pub fn render(&self, ram: &Vram, out: &mut [u8], pitch: usize) {
        let mask = (ram.bytes() - 1) as u32;
        let f = self.frame();
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
                // shows: the words of a run (a row, or a tile's 64 pixels)
                // follow each other, so only its first needs the address.
                let clut = (!bypass).then_some(desktop_clut);
                let pixels = row.as_chunks_mut::<4>().0;
                let words = mask >> 1;
                let mut x = 0;
                while x < pixels.len() {
                    let run = if f.tiled {
                        64 - (x & 63)
                    } else {
                        pixels.len() - x
                    };
                    let end = (x + run).min(pixels.len());
                    let mut w = (at(2 * x as u32) & mask) >> 1;
                    for px in &mut pixels[x..end] {
                        let [r, g, b] = rgb565(ram.get(w as usize), clut);
                        *px = [r, g, b, 255];
                        w = (w + 1) & words;
                    }
                    x = end;
                }
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
            self.render_overlay(ram, out, pitch, &f);
        }
        if f.cfg & VIDPROC_CURSOR != 0 {
            self.render_cursor(ram, out, pitch, &f);
        }
    }

    /// The overlay: a rectangle of the screen from another surface, scaled
    /// if the scale bits say, in 5-6-5 or YUV 4:2:2.
    fn render_overlay(&self, ram: &Vram, out: &mut [u8], pitch: usize, f: &Frame) {
        let mask = (ram.bytes() - 1) as u32;
        let start = self.reg(VID_OVERLAY_START_COORDS);
        let end = self.reg(VID_OVERLAY_END_SCREEN_COORDS);
        let (x0, y0) = ((start & 0xFFF) as usize, ((start >> 12) & 0xFFF) as usize);
        let (x1, y1) = ((end & 0xFFF) as usize, ((end >> 12) & 0xFFF) as usize);
        if x1 < x0 || y1 < y0 {
            return;
        }
        let tiled = f.cfg & VIDPROC_OVERLAY_TILED != 0;
        let pitch_reg = (self.reg(VID_DESKTOP_OVERLAY_STRIDE) >> 16) & 0x7FFF;
        let stride = if tiled { pitch_reg * TILE } else { pitch_reg };
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
        let clut = &self.clut[if f.cfg & VIDPROC_OVERLAY_CLUT_SEL != 0 {
            256
        } else {
            0
        }..][..256];
        let bypass = f.cfg & VIDPROC_OVERLAY_CLUT_BYPASS != 0;
        let format = (f.cfg >> 21) & 7;
        let mut v = (self.reg(VID_OVERLAY_DVDY_OFFSET) & 0x7_FFFF) as u64;
        for y in y0..=y1.min(f.height.saturating_sub(1)) {
            let sy = (v >> 20) as u32;
            v += dvdy as u64;
            let mut u = 0u64;
            for x in x0..=x1
                .min(f.width.saturating_sub(1))
                .min(pitch.saturating_sub(1))
            {
                let sx = (u >> 20) as u32;
                u += dudx as u64;
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
            }
        }
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

//! Drawing commands: a triangle from the parameters the host wrote (its
//! texture units, the subpixel adjustment, the buffer it goes into, and
//! its scan conversion into scanlines), and the fastfill that clears a
//! rectangle. From DOSBox-X's `triangle`, `triangle_create_work_item`,
//! `poly_render_triangle` and `fastfill` (voodoo_emu.cpp).

use super::raster::{self, RasterState, TmuParams, TmuRaster, TriParams};
use super::regs::*;
use super::{NONE, Voodoo};

/// X to whole pixels: pixel centres at .5 belong to the pixel on the left
/// (`round_coordinate`).
fn round_coordinate(value: f32) -> i32 {
    let result = (value as f64).floor() as i32;
    result + (value - result as f32 > 0.5) as i32
}

impl Voodoo {
    /// The word offset of a colour buffer, or None for a reserved choice.
    fn color_buffer(&self, select: u32) -> Option<usize> {
        let index = match select {
            0 => self.fbi.frontbuf,
            1 => self.fbi.backbuf,
            _ => return None,
        };
        let offs = self.fbi.rgboffs[index as usize];
        (offs != NONE).then_some(offs as usize / 2)
    }

    /// What drawing into colour buffer `dest` reads from the registers.
    pub(crate) fn raster_state(&self, dest: usize, texcount: usize) -> RasterState {
        let reg = &self.reg;
        let tmu = |unit: usize| -> Option<TmuRaster> {
            if unit >= texcount {
                return None;
            }
            let t = &self.tmu[unit];
            let mode = reg[t.base + TEXTURE_MODE];
            Some(TmuRaster {
                ram: t.ram.clone(),
                mask: t.mask,
                mode,
                lodmin: t.lodmin,
                lodmax: t.lodmax,
                lodbias: t.lodbias,
                lodmask: t.lodmask,
                lodoffset: t.lodoffset,
                detailmax: t.detailmax,
                detailbias: t.detailbias,
                detailscale: t.detailscale,
                wmask: t.wmask as i32,
                hmask: t.hmask as i32,
                lookup: t.lookup(mode),
            })
        };
        RasterState {
            fb: self.fbi.ram.clone(),
            fbz_color_path: reg[FBZ_COLOR_PATH],
            fbz_mode: reg[FBZ_MODE],
            alpha_mode: reg[ALPHA_MODE],
            fog_mode: reg[FOG_MODE],
            za_color: reg[ZA_COLOR],
            chroma_key: reg[CHROMA_KEY],
            color0: reg[COLOR0],
            color1: reg[COLOR1],
            fog_color: reg[FOG_COLOR],
            clip_left_right: reg[CLIP_LEFT_RIGHT],
            clip_low_y_high_y: reg[CLIP_LOW_Y_HIGH_Y],
            yorigin: self.fbi.yorigin,
            rowpixels: self.fbi.rowpixels as usize,
            dest,
            aux: (self.fbi.auxoffs != NONE).then_some(self.fbi.auxoffs as usize / 2),
            fogblend: self.fbi.fogblend,
            fogdelta: self.fbi.fogdelta,
            send_config: self.send_config.then_some(self.tmu_config),
            tmu: [tmu(0), tmu(1)],
        }
    }

    /// triangleCMD: draw the triangle the registers describe.
    pub(crate) fn triangle(&mut self) {
        let mut texcount = 0;
        if self.reg[FBI_INIT3] & (1 << 6) == 0 && self.reg[FBZ_COLOR_PATH] & (1 << 27) != 0 {
            texcount = self.tmu.len();
        }

        // Subpixel adjustment: the start values move from vertex A to the
        // centre of its pixel.
        if self.reg[FBZ_COLOR_PATH] & (1 << 26) != 0 {
            let f = &mut self.fbi;
            let dx = 8 - (f.ax & 15) as i32;
            let dy = 8 - (f.ay & 15) as i32;
            let adj = |d_dy: i32, d_dx: i32| dy.wrapping_mul(d_dy).wrapping_add(dx.wrapping_mul(d_dx)) >> 4;
            let adj64 =
                |d_dy: i64, d_dx: i64| (dy as i64).wrapping_mul(d_dy).wrapping_add((dx as i64).wrapping_mul(d_dx)) >> 4;
            f.startr = f.startr.wrapping_add(adj(f.drdy, f.drdx));
            f.startg = f.startg.wrapping_add(adj(f.dgdy, f.dgdx));
            f.startb = f.startb.wrapping_add(adj(f.dbdy, f.dbdx));
            f.starta = f.starta.wrapping_add(adj(f.dady, f.dadx));
            f.startw = f.startw.wrapping_add(adj64(f.dwdy, f.dwdx));
            let mul = |a: i32, b: i32| ((a as i64 * b as i64) >> 4) as i32;
            f.startz = f.startz.wrapping_add(mul(dy, f.dzdy)).wrapping_add(mul(dx, f.dzdx));
            for t in self.tmu.iter_mut().take(texcount) {
                t.startw = t.startw.wrapping_add(adj64(t.dwdy, t.dwdx));
                t.starts = t.starts.wrapping_add(adj64(t.dsdy, t.dsdx));
                t.startt = t.startt.wrapping_add(adj64(t.dtdy, t.dtdx));
            }
        }

        let Some(dest) = self.color_buffer((self.reg[FBZ_MODE] >> 14) & 3) else { return };

        let f = &self.fbi;
        let mut p = TriParams {
            ax: f.ax,
            ay: f.ay,
            startr: f.startr,
            startg: f.startg,
            startb: f.startb,
            starta: f.starta,
            startz: f.startz,
            startw: f.startw,
            drdx: f.drdx,
            dgdx: f.dgdx,
            dbdx: f.dbdx,
            dadx: f.dadx,
            dzdx: f.dzdx,
            dwdx: f.dwdx,
            drdy: f.drdy,
            dgdy: f.dgdy,
            dbdy: f.dbdy,
            dady: f.dady,
            dzdy: f.dzdy,
            dwdy: f.dwdy,
            tmu: Default::default(),
        };
        let verts = [(f.ax, f.ay), (f.bx, f.by), (f.cx, f.cy)].map(|(x, y)| (x as f32 / 16.0, y as f32 / 16.0));
        let reg = self.reg.clone();
        for unit in 0..texcount {
            let t = &mut self.tmu[unit];
            let lodbase = t.prepare(&reg[..]);
            p.tmu[unit] = TmuParams {
                starts: t.starts,
                startt: t.startt,
                startw: t.startw,
                dsdx: t.dsdx,
                dtdx: t.dtdx,
                dwdx: t.dwdx,
                dsdy: t.dsdy,
                dtdy: t.dtdy,
                dwdy: t.dwdy,
                lodbase,
            };
        }
        let st = self.raster_state(dest, texcount);
        let mut stipple = self.reg[STIPPLE];
        let mut stats = self.stats;
        match texcount {
            0 => render_triangle(verts, |y, x0, x1| raster::scanline::<0>(&st, &p, y, x0, x1, &mut stipple, &mut stats)),
            1 => render_triangle(verts, |y, x0, x1| raster::scanline::<1>(&st, &p, y, x0, x1, &mut stipple, &mut stats)),
            _ => render_triangle(verts, |y, x0, x1| raster::scanline::<2>(&st, &p, y, x0, x1, &mut stipple, &mut stats)),
        }
        self.reg[STIPPLE] = stipple;
        self.stats = stats;
        self.reg[FBI_TRIANGLES_OUT] = self.reg[FBI_TRIANGLES_OUT].wrapping_add(1);
        self.mark_drawn(dest);
    }

    /// Something was drawn into colour buffer `dest`: the display changes
    /// if it is the one shown.
    pub(crate) fn mark_drawn(&mut self, dest: usize) {
        if self.color_buffer(0) == Some(dest) {
            self.display_dirty = true;
        }
    }

    /// fastfillCMD: fill the clip rectangle with color1 and the depth or
    /// alpha of zaColor, as fbzMode's masks say.
    pub(crate) fn fastfill(&mut self) {
        let fbz = self.reg[FBZ_MODE];
        let (rgb, aux) = (fbz & (1 << 9) != 0, fbz & (1 << 10) != 0);
        if !rgb && !aux {
            return;
        }
        let (clip_x, clip_y) = (self.reg[CLIP_LEFT_RIGHT], self.reg[CLIP_LOW_Y_HIGH_Y]);
        let (sx, ex) = (((clip_x >> 16) & 0x3FF) as i32, (clip_x & 0x3FF) as i32);
        let (sy, ey) = (((clip_y >> 16) & 0x3FF) as i32, (clip_y & 0x3FF) as i32);

        let dest = if rgb { self.color_buffer((fbz >> 14) & 3) } else { Some(0) };
        let Some(dest) = dest else { return };
        // color1 through the dither matrix, 4x4 pixels.
        let mut dither = [0u16; 16];
        if rgb {
            let t = super::tables::tables();
            let c = self.reg[COLOR1];
            let (r, g, b) = ((c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF);
            for y in 0..4usize {
                for x in 0..4usize {
                    let (r5, g6, b5) = if fbz & (1 << 8) != 0 {
                        let lookup = if fbz & (1 << 11) == 0 { &t.dither4 } else { &t.dither2 };
                        let at = y << 11 | x << 1;
                        (
                            lookup[at | (r as usize) << 3] as u16,
                            lookup[at | (g as usize) << 3 | 1] as u16,
                            lookup[at | (b as usize) << 3] as u16,
                        )
                    } else {
                        ((r >> 3) as u16, (g >> 2) as u16, (b >> 3) as u16)
                    };
                    dither[y * 4 + x] = r5 << 11 | g6 << 5 | b5;
                }
            }
        }
        let st = self.raster_state(dest, 0);
        let mut stats = self.stats;
        for y in sy..ey {
            raster::fastfill_row(&st, &dither, y, sx, ex, &mut stats);
        }
        self.stats = stats;
        if rgb {
            self.mark_drawn(dest);
        }
    }
}

/// Scan-convert a triangle: for each scanline whose centre it covers, the
/// pixels from its left to its right edge, sampled at the pixel centres
/// (`poly_render_triangle`).
fn render_triangle(verts: [(f32, f32); 3], mut scanline: impl FnMut(i32, i32, i32)) {
    // Sorted by Y as the original sorts, keeping equal Ys in their order.
    let (mut v1, mut v2, mut v3) = (verts[0], verts[1], verts[2]);
    if v2.1 < v1.1 {
        std::mem::swap(&mut v1, &mut v2);
    }
    if v3.1 < v2.1 {
        std::mem::swap(&mut v2, &mut v3);
        if v2.1 < v1.1 {
            std::mem::swap(&mut v1, &mut v2);
        }
    }

    let v1y = round_coordinate(v1.1);
    let v3y = round_coordinate(v3.1);
    if v3y - v1y <= 0 {
        return;
    }
    let slope = |a: (f32, f32), b: (f32, f32)| if b.1 == a.1 { 0.0f32 } else { (b.0 - a.0) / (b.1 - a.1) };
    let dxdy_v1v2 = slope(v1, v2);
    let dxdy_v1v3 = slope(v1, v3);
    let dxdy_v2v3 = slope(v2, v3);
    // Scanlines beyond the 10-bit Y range can't be anywhere on screen.
    for y in v1y.max(-2048)..v3y.min(4096) {
        let fully = y as f32 + 0.5;
        let startx = v1.0 + (fully - v1.1) * dxdy_v1v3;
        let stopx = if fully < v2.1 { v1.0 + (fully - v1.1) * dxdy_v1v2 } else { v2.0 + (fully - v2.1) * dxdy_v2v3 };
        let (mut x0, mut x1) = (round_coordinate(startx), round_coordinate(stopx));
        if x0 > x1 {
            std::mem::swap(&mut x0, &mut x1);
        }
        if x0 >= x1 {
            (x0, x1) = (0, 0);
        }
        scanline(y, x0, x1.min(4096));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(verts: [(f32, f32); 3]) -> Vec<(i32, i32, i32)> {
        let mut out = Vec::new();
        render_triangle(verts, |y, x0, x1| out.push((y, x0, x1)));
        out
    }

    #[test]
    fn scan_conversion_samples_pixel_centres() {
        // A right triangle over a 4x4 square's lower left half.
        let s = spans([(0.0, 0.0), (0.0, 4.0), (4.0, 4.0)]);
        assert_eq!(s, vec![(0, 0, 0), (1, 0, 1), (2, 0, 2), (3, 0, 3)]);
        // Rounding: x.5 goes left.
        assert_eq!(round_coordinate(2.5), 2);
        assert_eq!(round_coordinate(2.51), 3);
        assert_eq!(round_coordinate(-0.5), -1);
    }

    #[test]
    fn flat_triangles_draw_nothing() {
        assert!(spans([(0.0, 1.0), (5.0, 1.0), (9.0, 1.2)]).is_empty());
    }
}

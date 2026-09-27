//! The pixel pipeline: what happens to each pixel of a triangle (and of
//! a frame buffer write through the pipeline) on its way into the frame
//! buffer. Stippling, the depth value and test, the texture units'
//! lookups and combines, the colour and alpha combine, chroma key, alpha
//! mask and test, fog, alpha blending, dithering, and the writes to the
//! colour and auxiliary (depth or alpha) buffers.
//!
//! A port of DOSBox-X's `raster_generic` and the pipeline macros in
//! voodoo_data.h (MAME's generic rasterizer), with the pixel counters MAME
//! keeps. Everything a triangle reads comes from a `RasterState` taken
//! when it was drawn, so render workers can draw it while the card goes
//! on changing its registers.

use super::mem::Vram;
use super::tables::{DITHER_MATRIX_2X2, DITHER_MATRIX_4X4, bilinear, tables};
use std::sync::Arc;

/// The pixel counters (`stats_block`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub pixels_in: i32,
    pub pixels_out: i32,
    pub chroma_fail: i32,
    pub zfunc_fail: i32,
    pub afunc_fail: i32,
    pub clip_fail: i32,
}

impl Stats {
    pub fn add(&mut self, other: &Stats) {
        self.pixels_in = self.pixels_in.wrapping_add(other.pixels_in);
        self.pixels_out = self.pixels_out.wrapping_add(other.pixels_out);
        self.chroma_fail = self.chroma_fail.wrapping_add(other.chroma_fail);
        self.zfunc_fail = self.zfunc_fail.wrapping_add(other.zfunc_fail);
        self.afunc_fail = self.afunc_fail.wrapping_add(other.afunc_fail);
        self.clip_fail = self.clip_fail.wrapping_add(other.clip_fail);
    }
}

crate::state_fields!(Stats { pixels_in, pixels_out, chroma_fail, zfunc_fail, afunc_fail, clip_fail });

/// A texture unit as a triangle draws with it.
#[derive(Clone)]
pub struct TmuRaster {
    pub ram: Vram,
    pub mask: u32,
    pub mode: u32,
    pub lodmin: i32,
    pub lodmax: i32,
    pub lodbias: i32,
    pub lodmask: u32,
    pub lodoffset: [u32; 9],
    pub detailmax: i32,
    pub detailbias: i32,
    pub detailscale: u32,
    pub wmask: i32,
    pub hmask: i32,
    pub lookup: Arc<[u32]>,
}

/// The registers and memory layout a triangle, a fastfill or a pipelined
/// frame buffer write draws with.
#[derive(Clone)]
pub struct RasterState {
    pub fb: Vram,
    pub fbz_color_path: u32,
    pub fbz_mode: u32,
    pub alpha_mode: u32,
    pub fog_mode: u32,
    pub za_color: u32,
    pub chroma_key: u32,
    pub color0: u32,
    pub color1: u32,
    pub fog_color: u32,
    pub clip_left_right: u32,
    pub clip_low_y_high_y: u32,
    pub yorigin: u32,
    pub rowpixels: usize,
    /// Word offsets of the colour buffer drawn into and of the auxiliary
    /// buffer, if there is one.
    pub dest: usize,
    pub aux: Option<usize>,
    pub fogblend: [u8; 64],
    pub fogdelta: [u8; 64],
    /// With trexInit1 bit 18, TMU 0 hands out its configuration (how
    /// Glide counts the texture units) rather than texels.
    pub send_config: Option<u32>,
    pub tmu: [Option<TmuRaster>; 2],
}

/// A triangle's start values at vertex A and their gradients: colours
/// 12.12, Z 20.12, W and the texture coordinates 16.32.
#[derive(Clone, Copy, Debug, Default)]
pub struct TriParams {
    pub ax: i16,
    pub ay: i16,
    pub startr: i32,
    pub startg: i32,
    pub startb: i32,
    pub starta: i32,
    pub startz: i32,
    pub startw: i64,
    pub drdx: i32,
    pub dgdx: i32,
    pub dbdx: i32,
    pub dadx: i32,
    pub dzdx: i32,
    pub dwdx: i64,
    pub drdy: i32,
    pub dgdy: i32,
    pub dbdy: i32,
    pub dady: i32,
    pub dzdy: i32,
    pub dwdy: i64,
    pub tmu: [TmuParams; 2],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TmuParams {
    pub starts: i64,
    pub startt: i64,
    pub startw: i64,
    pub dsdx: i64,
    pub dtdx: i64,
    pub dwdx: i64,
    pub dsdy: i64,
    pub dtdy: i64,
    pub dwdy: i64,
    pub lodbase: i32,
}

// The register fields the pipeline tests.
const fn bit(v: u32, n: u32) -> bool {
    (v >> n) & 1 != 0
}

fn a_of(c: u32) -> i32 {
    (c >> 24) as i32
}
fn r_of(c: u32) -> i32 {
    ((c >> 16) & 0xFF) as i32
}
fn g_of(c: u32) -> i32 {
    ((c >> 8) & 0xFF) as i32
}
fn b_of(c: u32) -> i32 {
    (c & 0xFF) as i32
}
fn pack(a: i32, r: i32, g: i32, b: i32) -> u32 {
    (a as u32 & 0xFF) << 24 | (r as u32 & 0xFF) << 16 | (g as u32 & 0xFF) << 8 | (b as u32 & 0xFF)
}
fn clamp8(v: i32) -> i32 {
    v.clamp(0, 0xFF)
}

/// The iterated colour as 8-bit components: without fbzColorPath bit 28
/// (always clear on a Voodoo Graphics) the integer part wraps, with the
/// hardware's special cases for -1 and 256 (`CLAMPED_ARGB`).
fn clamped_argb(iterr: i32, iterg: i32, iterb: i32, itera: i32, fbzcp: u32) -> u32 {
    let wrap = |v: i32| -> i32 {
        let v = (v >> 12) & 0xFFF;
        match v {
            0xFFF => 0,
            0x100 => 0xFF,
            _ => v & 0xFF,
        }
    };
    let clamp = |v: i32| (v >> 12).clamp(0, 0xFF);
    if !bit(fbzcp, 28) {
        pack(wrap(itera), wrap(iterr), wrap(iterg), wrap(iterb))
    } else {
        pack(clamp(itera), clamp(iterr), clamp(iterg), clamp(iterb))
    }
}

/// Z as 16 bits (`CLAMPED_Z`).
fn clamped_z(iterz: i32, fbzcp: u32) -> i32 {
    let v = iterz >> 12;
    if !bit(fbzcp, 28) {
        match v & 0xF_FFFF {
            0xF_FFFF => 0,
            0x1_0000 => 0xFFFF,
            v => v & 0xFFFF,
        }
    } else {
        v.clamp(0, 0xFFFF)
    }
}

/// W's integer part as 8 bits (`CLAMPED_W`).
fn clamped_w(iterw: i64, fbzcp: u32) -> i32 {
    let v = (iterw >> 32) as i16 as i32;
    if !bit(fbzcp, 28) {
        match v & 0xFFFF {
            0xFFFF => 0,
            0x100 => 0xFF,
            v => v & 0xFF,
        }
    } else {
        v.clamp(0, 0xFF)
    }
}

/// W as the 4.12 floating point value the depth buffer and fog use.
fn wfloat(iterw: i64) -> i32 {
    if iterw as u64 & 0xFFFF_0000_0000 != 0 {
        return 0;
    }
    let temp = iterw as u32;
    if temp & 0xFFFF_0000 == 0 {
        return 0xFFFF;
    }
    let exp = temp.leading_zeros() as i32;
    let w = (exp << 12) | ((!temp >> (19 - exp)) & 0xFFF) as i32;
    if w < 0xFFFF { w + 1 } else { w }
}

/// Where the pixels of one scanline go: the rows of the colour and
/// auxiliary buffers, and the dither rows for the scanline's Y.
struct Row<'a> {
    st: &'a RasterState,
    dest: usize,
    depth: Option<usize>,
    /// The matrix row for dither subtraction, the 4x4 row for LOD and fog
    /// dither, and the lookup for the colour output (all only with
    /// dithering on).
    dither: Option<&'static [u8]>,
    dither4: Option<&'static [u8]>,
    dither_lookup: Option<&'static [u8]>,
}

impl<'a> Row<'a> {
    fn new(st: &'a RasterState, y: i32, scry: i32) -> Self {
        let fbz = st.fbz_mode;
        let (mut dither, mut dither4, mut dither_lookup) = (None, None, None);
        if bit(fbz, 8) {
            let y3 = (y & 3) as usize;
            dither4 = Some(&DITHER_MATRIX_4X4[y3 * 4..y3 * 4 + 4]);
            if !bit(fbz, 11) {
                dither = dither4;
                dither_lookup = Some(&tables().dither4[y3 << 11..(y3 + 1) << 11]);
            } else {
                dither = Some(&DITHER_MATRIX_2X2[y3 * 4..y3 * 4 + 4]);
                dither_lookup = Some(&tables().dither2[y3 << 11..(y3 + 1) << 11]);
            }
        }
        let row = scry as usize * st.rowpixels;
        Self {
            st,
            dest: st.dest + row,
            depth: st.aux.map(|aux| aux + row),
            dither,
            dither4,
            dither_lookup,
        }
    }

    fn get(&self, at: usize) -> u16 {
        if at < self.st.fb.len() { self.st.fb.get(at) } else { 0 }
    }

    fn set(&self, at: usize, value: u16) {
        if at < self.st.fb.len() {
            self.st.fb.set(at, value);
        }
    }

    fn depth_at(&self, x: i32) -> Option<i32> {
        self.depth.map(|d| self.get(d + x as usize) as i32)
    }

    /// Stippling, the depth value and the depth test
    /// (`PIXEL_PIPELINE_BEGIN`). The depth value, or None to skip the
    /// pixel.
    #[inline(always)]
    fn begin(&self, x: i32, y: i32, iterz: i32, iterw: i64, stipple: &mut u32, stats: &mut Stats) -> Option<i32> {
        let st = self.st;
        let fbz = st.fbz_mode;
        stats.pixels_in += 1;
        if bit(fbz, 2) {
            if !bit(fbz, 12) {
                // Rotate mode: one bit a pixel, drawn where it is set.
                *stipple = stipple.rotate_left(1);
                if *stipple & 0x8000_0000 == 0 {
                    return None;
                }
            } else {
                let index = ((y & 3) << 3) | (!x & 7);
                if (*stipple >> index) & 1 == 0 {
                    return None;
                }
            }
        }

        let mut depthval = if !bit(fbz, 3) {
            clamped_z(iterz, st.fbz_color_path)
        } else if !bit(fbz, 21) {
            wfloat(iterw)
        } else if iterz as u32 & 0xF000_0000 != 0 {
            0
        } else {
            let temp = (iterz as u32) << 4;
            if temp & 0xFFFF_0000 == 0 {
                0xFFFF
            } else {
                let exp = temp.leading_zeros() as i32;
                let v = (exp << 12) | ((!temp >> (19 - exp)) & 0xFFF) as i32;
                if v < 0xFFFF { v + 1 } else { v }
            }
        };
        if bit(fbz, 16) {
            depthval = (depthval + st.za_color as i16 as i32).clamp(0, 0xFFFF);
        }

        if bit(fbz, 4) {
            let source = if !bit(fbz, 20) { depthval } else { st.za_color as u16 as i32 };
            let pass = match (fbz >> 5) & 7 {
                0 => false,
                7 => true,
                func => match self.depth_at(x) {
                    None => true,
                    Some(d) => match func {
                        1 => source < d,
                        2 => source == d,
                        3 => source <= d,
                        4 => source > d,
                        5 => source != d,
                        _ => source >= d,
                    },
                },
            };
            if !pass {
                stats.zfunc_fail += 1;
                return None;
            }
        }
        Some(depthval)
    }

    /// Chroma key, alpha mask and alpha test on the colour going on
    /// (`APPLY_CHROMAKEY`, `APPLY_ALPHAMASK`, `APPLY_ALPHATEST`); false to
    /// skip the pixel. The chroma test is on `color`, the others on its
    /// alpha.
    fn tests(&self, color: u32, alpha: i32, stats: &mut Stats) -> bool {
        let st = self.st;
        if bit(st.fbz_mode, 1) && ((color ^ st.chroma_key) & 0xFF_FFFF) == 0 {
            stats.chroma_fail += 1;
            return false;
        }
        if bit(st.fbz_mode, 13) && alpha & 1 == 0 {
            stats.afunc_fail += 1;
            return false;
        }
        if bit(st.alpha_mode, 0) {
            let reference = (st.alpha_mode >> 24) as i32;
            let pass = match (st.alpha_mode >> 1) & 7 {
                0 => false,
                1 => alpha < reference,
                2 => alpha == reference,
                3 => alpha <= reference,
                4 => alpha > reference,
                5 => alpha != reference,
                6 => alpha >= reference,
                _ => true,
            };
            if !pass {
                stats.afunc_fail += 1;
                return false;
            }
        }
        true
    }

    /// Fog, alpha blending and the writes (`PIXEL_PIPELINE_MODIFY`,
    /// `PIXEL_PIPELINE_FINISH`). `iter_a` is the iterated alpha fog can
    /// use.
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn finish(&self, x: i32, color: (i32, i32, i32, i32), depthval: i32, iterz: i32, iterw: i64, iter_a: i32, stats: &mut Stats) {
        let st = self.st;
        let fbz = st.fbz_mode;
        let (mut r, mut g, mut b, mut a) = color;
        let (prefogr, prefogg, prefogb) = (r, g, b);

        // Fog.
        let fog = st.fog_mode;
        if bit(fog, 0) {
            let fc = st.fog_color;
            let (fr, fg, fb);
            if bit(fog, 5) {
                (fr, fg, fb) = (r_of(fc), g_of(fc), b_of(fc));
            } else {
                let (mut tr, mut tg, mut tb) = if !bit(fog, 1) { (r_of(fc), g_of(fc), b_of(fc)) } else { (0, 0, 0) };
                if !bit(fog, 2) {
                    tr -= r;
                    tg -= g;
                    tb -= b;
                }
                let mut blend = match (fog >> 3) & 3 {
                    0 => {
                        let w = wfloat(iterw);
                        let delta = st.fogdelta[(w >> 10) as usize] as i32;
                        let mut deltaval = (delta & 0xFF) * ((w >> 2) & 0xFF);
                        if bit(fog, 7) && delta & 2 != 0 {
                            deltaval = -deltaval;
                        }
                        deltaval >>= 6;
                        if bit(fog, 6)
                            && let Some(d) = self.dither4
                        {
                            deltaval += d[(x & 3) as usize] as i32;
                        }
                        deltaval >>= 4;
                        st.fogblend[(w >> 10) as usize] as i32 + deltaval
                    }
                    1 => iter_a,
                    2 => clamped_z(iterz, st.fbz_color_path) >> 8,
                    _ => clamped_w(iterw, st.fbz_color_path),
                };
                blend += 1;
                (fr, fg, fb) = ((tr * blend) >> 8, (tg * blend) >> 8, (tb * blend) >> 8);
            }
            if !bit(fog, 2) {
                r += fr;
                g += fg;
                b += fb;
            } else {
                (r, g, b) = (fr, fg, fb);
            }
            (r, g, b) = (clamp8(r), clamp8(g), clamp8(b));
        }

        // Alpha blending with the colour and alpha already there.
        let am = st.alpha_mode;
        if bit(am, 4) {
            let dpix = self.get(self.dest + x as usize) as i32;
            let (mut dr, mut dg, mut db) = ((dpix >> 8) & 0xF8, (dpix >> 3) & 0xFC, (dpix << 3) & 0xF8);
            let da = match self.depth_at(x) {
                Some(d) if bit(fbz, 18) => d,
                _ => 0xFF,
            };
            let (sr, sg, sb, sa) = (r, g, b, a);
            if bit(fbz, 19)
                && let Some(d) = self.dither
            {
                let dith = d[(x & 3) as usize] as i32;
                dr = ((dr << 1) + 15 - dith) >> 1;
                dg = ((dg << 2) + 15 - dith) >> 2;
                db = ((db << 1) + 15 - dith) >> 1;
            }
            let scale = |c: i32, f: i32| (c * f) >> 8;
            (r, g, b) = match (am >> 8) & 15 {
                1 => (scale(sr, sa + 1), scale(sg, sa + 1), scale(sb, sa + 1)),
                2 => (scale(sr, dr + 1), scale(sg, dg + 1), scale(sb, db + 1)),
                3 => (scale(sr, da + 1), scale(sg, da + 1), scale(sb, da + 1)),
                4 => (sr, sg, sb),
                5 => (scale(sr, 0x100 - sa), scale(sg, 0x100 - sa), scale(sb, 0x100 - sa)),
                6 => (scale(sr, 0x100 - dr), scale(sg, 0x100 - dg), scale(sb, 0x100 - db)),
                7 => (scale(sr, 0x100 - da), scale(sg, 0x100 - da), scale(sb, 0x100 - da)),
                15 => {
                    let ta = sa.min(0x100 - da);
                    (scale(sr, ta + 1), scale(sg, ta + 1), scale(sb, ta + 1))
                }
                _ => (0, 0, 0),
            };
            let (ar, ag, ab) = match (am >> 12) & 15 {
                1 => (scale(dr, sa + 1), scale(dg, sa + 1), scale(db, sa + 1)),
                2 => (scale(dr, sr + 1), scale(dg, sg + 1), scale(db, sb + 1)),
                3 => (scale(dr, da + 1), scale(dg, da + 1), scale(db, da + 1)),
                4 => (dr, dg, db),
                5 => (scale(dr, 0x100 - sa), scale(dg, 0x100 - sa), scale(db, 0x100 - sa)),
                6 => (scale(dr, 0x100 - sr), scale(dg, 0x100 - sg), scale(db, 0x100 - sb)),
                7 => (scale(dr, 0x100 - da), scale(dg, 0x100 - da), scale(db, 0x100 - da)),
                15 => (scale(dr, prefogr + 1), scale(dg, prefogg + 1), scale(db, prefogb + 1)),
                _ => (0, 0, 0),
            };
            (r, g, b) = (r + ar, g + ag, b + ab);
            a = 0;
            if (am >> 16) & 15 == 4 {
                a = sa;
            }
            if (am >> 20) & 15 == 4 {
                a += da;
            }
            (r, g, b, a) = (clamp8(r), clamp8(g), clamp8(b), clamp8(a));
        }

        // The colour, dithered to 5-6-5.
        if bit(fbz, 9) {
            let (r5, g6, b5) = match self.dither_lookup {
                Some(lookup) => {
                    let col = ((x & 3) << 1) as usize;
                    (
                        lookup[col + ((r as usize) << 3)] as u16,
                        lookup[col + ((g as usize) << 3) + 1] as u16,
                        lookup[col + ((b as usize) << 3)] as u16,
                    )
                }
                None => ((r >> 3) as u16, (g >> 2) as u16, (b >> 3) as u16),
            };
            self.set(self.dest + x as usize, r5 << 11 | g6 << 5 | b5);
        }
        // Depth, or alpha with the alpha planes.
        if let Some(depth) = self.depth
            && bit(fbz, 10)
        {
            let value = if bit(fbz, 18) { a } else { depthval };
            self.set(depth + x as usize, value as u16);
        }
        stats.pixels_out += 1;
    }
}

/// A texture unit's colour for a pixel, combined with `cother`, the
/// colour of the unit before it (`TEXTURE_PIPELINE`).
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn texture(t: &TmuRaster, x: i32, dither4: Option<&[u8]>, cother: u32, lodbase: i32, iters: i64, itert: i64, iterw: i64) -> u32 {
    let mode = t.mode;
    let (mut s, mut tc, mut lod);
    if bit(mode, 0) {
        let (oow, log) = tables().reciplog(iterw);
        s = (oow.wrapping_mul(iters) >> 29) as i32;
        tc = (oow.wrapping_mul(itert) >> 29) as i32;
        lod = log + lodbase;
    } else {
        s = (iters >> 14) as i32;
        tc = (itert >> 14) as i32;
        lod = lodbase;
    }
    if bit(mode, 3) && iterw < 0 {
        s = 0;
        tc = 0;
    }

    lod += t.lodbias;
    if bit(mode, 4)
        && let Some(d) = dither4
    {
        lod += (d[(x & 3) as usize] as i32) << 4;
    }
    if lod < t.lodmin {
        lod = t.lodmin;
    }
    if lod > t.lodmax {
        lod = t.lodmax;
    }
    // A level the texture hasn't (with the LOD split) takes the next.
    let mut ilod = lod >> 8;
    if (t.lodmask >> ilod.clamp(0, 31)) & 1 == 0 {
        ilod += 1;
    }
    let ilod = ilod.clamp(0, 8);
    let texbase = t.lodoffset[ilod as usize];
    let smax = t.wmask >> ilod;
    let tmax = t.hmask >> ilod;
    let format = (mode >> 8) & 0xF;
    let wide = format >= 8;
    let full = (10..=12).contains(&format);
    let fetch = |s: i32, tc: i32| -> u32 {
        let index = (tc as u32).wrapping_mul(smax as u32 + 1).wrapping_add(s as u32);
        if !wide {
            t.lookup[t.ram.byte((texbase.wrapping_add(index) & t.mask) as usize) as usize]
        } else {
            let texel = t.ram.get(((texbase.wrapping_add(index.wrapping_mul(2)) & t.mask) >> 1) as usize) as u32;
            if full {
                t.lookup[texel as usize]
            } else {
                (t.lookup[(texel & 0xFF) as usize] & 0xFF_FFFF) | ((texel & 0xFF00) << 16)
            }
        }
    };

    let point = (lod == t.lodmin && !bit(mode, 2)) || (lod != t.lodmin && !bit(mode, 1));
    let c_local = if point {
        s >>= ilod + 18;
        tc >>= ilod + 18;
        if bit(mode, 6) {
            s = s.clamp(0, smax);
        }
        if bit(mode, 7) {
            tc = tc.clamp(0, tmax);
        }
        fetch(s & smax, tc & tmax)
    } else {
        s >>= ilod + 10;
        tc >>= ilod + 10;
        s -= 0x80;
        tc -= 0x80;
        // A Voodoo Graphics filters with 4 bits of fraction.
        let sfrac = (s & 0xF0) as u32;
        let tfrac = (tc & 0xF0) as u32;
        s >>= 8;
        tc >>= 8;
        let (mut s1, mut t1) = (s + 1, tc + 1);
        if bit(mode, 6) {
            s = s.clamp(0, smax);
            s1 = s1.clamp(0, smax);
        }
        if bit(mode, 7) {
            tc = tc.clamp(0, tmax);
            t1 = t1.clamp(0, tmax);
        }
        let (s, s1, tc, t1) = (s & smax, s1 & smax, tc & tmax, t1 & tmax);
        bilinear(fetch(s, tc), fetch(s1, tc), fetch(s, t1), fetch(s1, t1), sfrac, tfrac)
    };

    // The texture combine unit.
    let (mut tr, mut tg, mut tb) = if !bit(mode, 12) { (r_of(cother), g_of(cother), b_of(cother)) } else { (0, 0, 0) };
    let mut ta = if !bit(mode, 21) { a_of(cother) } else { 0 };
    if bit(mode, 13) {
        tr -= r_of(c_local);
        tg -= g_of(c_local);
        tb -= b_of(c_local);
    }
    if bit(mode, 22) {
        ta -= a_of(c_local);
    }
    let detail = || {
        if t.detailbias <= lod {
            0
        } else {
            (((t.detailbias - lod) << t.detailscale) >> 8).min(t.detailmax)
        }
    };
    let (mut br, mut bg, mut bb) = match (mode >> 14) & 7 {
        1 => (r_of(c_local), g_of(c_local), b_of(c_local)),
        2 => (a_of(cother), a_of(cother), a_of(cother)),
        3 => (a_of(c_local), a_of(c_local), a_of(c_local)),
        4 => {
            let d = detail();
            (d, d, d)
        }
        5 => (lod & 0xFF, lod & 0xFF, lod & 0xFF),
        _ => (0, 0, 0),
    };
    let mut ba = match (mode >> 23) & 7 {
        1 | 3 => a_of(c_local),
        2 => a_of(cother),
        4 => detail(),
        5 => lod & 0xFF,
        _ => 0,
    };
    if !bit(mode, 17) {
        br ^= 0xFF;
        bg ^= 0xFF;
        bb ^= 0xFF;
    }
    if !bit(mode, 26) {
        ba ^= 0xFF;
    }
    tr = (tr * (br + 1)) >> 8;
    tg = (tg * (bg + 1)) >> 8;
    tb = (tb * (bb + 1)) >> 8;
    ta = (ta * (ba + 1)) >> 8;
    match (mode >> 18) & 3 {
        1 => {
            tr += r_of(c_local);
            tg += g_of(c_local);
            tb += b_of(c_local);
        }
        2 => {
            tr += a_of(c_local);
            tg += a_of(c_local);
            tb += a_of(c_local);
        }
        _ => {}
    }
    if (mode >> 27) & 3 != 0 {
        ta += a_of(c_local);
    }
    let mut result = pack(clamp8(ta), clamp8(tr), clamp8(tg), clamp8(tb));
    if bit(mode, 20) {
        result ^= 0x00FF_FFFF;
    }
    if bit(mode, 29) {
        result ^= 0xFF00_0000;
    }
    result
}

/// The colour combine unit: c_other and c_local, chosen by fbzColorPath,
/// subtracted, blended, added and clamped. The ARGB result, or None if the
/// chroma key, alpha mask or alpha test rejects the pixel.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn color_combine(row: &Row, iterargb: u32, texel: u32, iterz: i32, iterw: i64, stats: &mut Stats) -> Option<(i32, i32, i32, i32)> {
    let st = row.st;
    let cp = st.fbz_color_path;
    let mut c_other = match cp & 3 {
        0 => iterargb,
        1 => texel,
        2 => st.color1,
        _ => 0,
    };
    // The chroma key tests c_other before its alpha is chosen.
    if bit(st.fbz_mode, 1) && ((c_other ^ st.chroma_key) & 0xFF_FFFF) == 0 {
        stats.chroma_fail += 1;
        return None;
    }
    let a_other = match (cp >> 2) & 3 {
        0 => a_of(iterargb),
        1 => a_of(texel),
        2 => a_of(st.color1),
        _ => 0,
    };
    c_other = (c_other & 0xFF_FFFF) | (a_other as u32) << 24;
    if bit(st.fbz_mode, 13) && a_other & 1 == 0 {
        stats.afunc_fail += 1;
        return None;
    }
    if !row.alpha_test(a_other, stats) {
        return None;
    }

    let local = if !bit(cp, 7) { if !bit(cp, 4) { iterargb } else { st.color0 } } else if a_of(texel) & 0x80 == 0 {
        iterargb
    } else {
        st.color0
    };
    let a_local = match (cp >> 5) & 3 {
        0 => a_of(iterargb),
        1 => a_of(st.color0),
        2 => clamped_z(iterz, cp) & 0xFF,
        _ => clamped_w(iterw, cp) & 0xFF,
    };
    Some(combine(cp, c_other, (local & 0xFF_FFFF) | (a_local as u32) << 24, texel))
}

/// The arithmetic of the colour combine unit on c_other and c_local.
#[inline(always)]
fn combine(cp: u32, c_other: u32, c_local: u32, texel: u32) -> (i32, i32, i32, i32) {
    let (mut r, mut g, mut b) = if !bit(cp, 8) { (r_of(c_other), g_of(c_other), b_of(c_other)) } else { (0, 0, 0) };
    let mut a = if !bit(cp, 17) { a_of(c_other) } else { 0 };
    if bit(cp, 9) {
        r -= r_of(c_local);
        g -= g_of(c_local);
        b -= b_of(c_local);
    }
    if bit(cp, 18) {
        a -= a_of(c_local);
    }
    let (mut br, mut bg, mut bb) = match (cp >> 10) & 7 {
        1 => (r_of(c_local), g_of(c_local), b_of(c_local)),
        2 => (a_of(c_other), a_of(c_other), a_of(c_other)),
        3 => (a_of(c_local), a_of(c_local), a_of(c_local)),
        4 => (a_of(texel), a_of(texel), a_of(texel)),
        5 => (r_of(texel), g_of(texel), b_of(texel)),
        _ => (0, 0, 0),
    };
    let mut ba = match (cp >> 19) & 7 {
        1 | 3 => a_of(c_local),
        2 => a_of(c_other),
        4 => a_of(texel),
        _ => 0,
    };
    if !bit(cp, 13) {
        br ^= 0xFF;
        bg ^= 0xFF;
        bb ^= 0xFF;
    }
    if !bit(cp, 22) {
        ba ^= 0xFF;
    }
    r = (r * (br + 1)) >> 8;
    g = (g * (bg + 1)) >> 8;
    b = (b * (bb + 1)) >> 8;
    a = (a * (ba + 1)) >> 8;
    match (cp >> 14) & 3 {
        1 => {
            r += r_of(c_local);
            g += g_of(c_local);
            b += b_of(c_local);
        }
        2 => {
            r += a_of(c_local);
            g += a_of(c_local);
            b += a_of(c_local);
        }
        _ => {}
    }
    if (cp >> 23) & 3 != 0 {
        a += a_of(c_local);
    }
    let (mut r, mut g, mut b, mut a) = (clamp8(r), clamp8(g), clamp8(b), clamp8(a));
    if bit(cp, 16) {
        r ^= 0xFF;
        g ^= 0xFF;
        b ^= 0xFF;
    }
    if bit(cp, 25) {
        a ^= 0xFF;
    }
    (r, g, b, a)
}

impl Row<'_> {
    fn alpha_test(&self, alpha: i32, stats: &mut Stats) -> bool {
        let am = self.st.alpha_mode;
        if !bit(am, 0) {
            return true;
        }
        let reference = (am >> 24) as i32;
        let pass = match (am >> 1) & 7 {
            0 => false,
            1 => alpha < reference,
            2 => alpha == reference,
            3 => alpha <= reference,
            4 => alpha > reference,
            5 => alpha != reference,
            6 => alpha >= reference,
            _ => true,
        };
        if !pass {
            stats.afunc_fail += 1;
        }
        pass
    }
}

/// The screen row scanline `y` draws into: flipped with the Y origin at
/// the bottom (fbzMode bit 17).
pub(crate) fn screen_y(st: &RasterState, y: i32, flip: bool) -> i32 {
    if flip { (st.yorigin as i32 - y) & 0x3FF } else { y }
}

/// Draw pixels `startx..stopx` of scanline `y` of a triangle with `TMUS`
/// texture units (`raster_generic`).
pub fn scanline<const TMUS: usize>(st: &RasterState, p: &TriParams, y: i32, startx: i32, stopx: i32, stipple: &mut u32, stats: &mut Stats) {
    let (mut startx, mut stopx) = (startx, stopx);
    let scry = screen_y(st, y, bit(st.fbz_mode, 17));
    if bit(st.fbz_mode, 0) {
        // Clipping: a row outside takes the whole scanline.
        if scry < ((st.clip_low_y_high_y >> 16) & 0x3FF) as i32 || scry >= (st.clip_low_y_high_y & 0x3FF) as i32 {
            stats.pixels_in += stopx - startx;
            stats.clip_fail += stopx - startx;
            return;
        }
        let left = ((st.clip_left_right >> 16) & 0x3FF) as i32;
        if startx < left {
            stats.pixels_in += left - startx;
            startx = left;
        }
        let right = (st.clip_left_right & 0x3FF) as i32;
        if stopx >= right {
            stats.pixels_in += stopx - right;
            stopx = right - 1;
        }
    }
    // Rows off the buffer, and pixels left of it, have nowhere to go.
    if scry < 0 || startx < 0 && stopx <= 0 {
        return;
    }
    let startx = startx.max(0);
    let row = Row::new(st, y, scry);

    let dx = startx - (p.ax >> 4) as i32;
    let dy = y - (p.ay >> 4) as i32;
    let at = |start: i32, ddy: i32, ddx: i32| start.wrapping_add(dy.wrapping_mul(ddy)).wrapping_add(dx.wrapping_mul(ddx));
    let at64 = |start: i64, ddy: i64, ddx: i64| {
        start.wrapping_add((dy as i64).wrapping_mul(ddy)).wrapping_add((dx as i64).wrapping_mul(ddx))
    };
    let mut iterr = at(p.startr, p.drdy, p.drdx);
    let mut iterg = at(p.startg, p.dgdy, p.dgdx);
    let mut iterb = at(p.startb, p.dbdy, p.dbdx);
    let mut itera = at(p.starta, p.dady, p.dadx);
    let mut iterz = at(p.startz, p.dzdy, p.dzdx);
    let mut iterw = at64(p.startw, p.dwdy, p.dwdx);
    let mut tex = [(0i64, 0i64, 0i64); 2];
    for (i, t) in tex.iter_mut().enumerate().take(TMUS) {
        let q = &p.tmu[i];
        *t = (at64(q.starts, q.dsdy, q.dsdx), at64(q.startt, q.dtdy, q.dtdx), at64(q.startw, q.dwdy, q.dwdx));
    }

    for x in startx..stopx {
        if let Some(depthval) = row.begin(x, y, iterz, iterw, stipple, stats) {
            // TMU 1 first, whose output TMU 0 combines with.
            let mut texel = 0u32;
            if TMUS >= 2
                && let Some(t) = &st.tmu[1]
                && t.lodmin < 8 << 8
            {
                let (s, tc, w) = tex[1];
                texel = texture(t, x, row.dither4, texel, p.tmu[1].lodbase, s, tc, w);
            }
            if TMUS >= 1
                && let Some(t) = &st.tmu[0]
                && t.lodmin < 8 << 8
            {
                texel = match st.send_config {
                    Some(config) => config,
                    None => {
                        let (s, tc, w) = tex[0];
                        texture(t, x, row.dither4, texel, p.tmu[0].lodbase, s, tc, w)
                    }
                };
            }
            let iterargb = clamped_argb(iterr, iterg, iterb, itera, st.fbz_color_path);
            if let Some(color) = color_combine(&row, iterargb, texel, iterz, iterw, stats) {
                row.finish(x, color, depthval, iterz, iterw, a_of(iterargb), stats);
            }
        }

        iterr = iterr.wrapping_add(p.drdx);
        iterg = iterg.wrapping_add(p.dgdx);
        iterb = iterb.wrapping_add(p.dbdx);
        itera = itera.wrapping_add(p.dadx);
        iterz = iterz.wrapping_add(p.dzdx);
        iterw = iterw.wrapping_add(p.dwdx);
        for (i, t) in tex.iter_mut().enumerate().take(TMUS) {
            let q = &p.tmu[i];
            *t = (t.0.wrapping_add(q.dsdx), t.1.wrapping_add(q.dtdx), t.2.wrapping_add(q.dwdx));
        }
    }
}

/// One pixel written through the frame buffer with the pixel pipeline
/// on (lfbMode bit 8): the frame buffer write's colour stands in for the
/// iterated colour and for c_other, as in DOSBox-X's `lfb_w`, which has no
/// texture in this path. `depth` is the write's depth (or zaColor's).
pub fn lfb_pixel(st: &RasterState, x: i32, y: i32, color: u32, depth: i32, stipple: &mut u32, stats: &mut Stats) {
    let scry = screen_y(st, y, bit(st.fbz_mode, 17));
    if bit(st.fbz_mode, 0)
        && (x < ((st.clip_left_right >> 16) & 0x3FF) as i32
            || x >= (st.clip_left_right & 0x3FF) as i32
            || scry < ((st.clip_low_y_high_y >> 16) & 0x3FF) as i32
            || scry >= (st.clip_low_y_high_y & 0x3FF) as i32)
    {
        stats.pixels_in += 1;
        stats.clip_fail += 1;
        return;
    }
    let row = Row::new(st, y, scry);
    let iterw = (depth as i64) << (30 - 16);
    let iterz = depth << 12;
    let Some(depthval) = row.begin(x, y, iterz, iterw, stipple, stats) else { return };
    if !row.tests(color, a_of(color), stats) {
        return;
    }
    let cp = st.fbz_color_path;
    // c_local: the write's colour (the "iterated" one) or color0.
    let local = if !bit(cp, 4) || bit(cp, 7) { color } else { st.color0 };
    let a_local = match (cp >> 5) & 3 {
        0 => a_of(color),
        1 => a_of(st.color0),
        2 => clamped_z(iterz, cp) & 0xFF,
        _ => clamped_w(iterw, cp) & 0xFF,
    };
    let rgba = combine(cp, color, (local & 0xFF_FFFF) | (a_local as u32) << 24, 0);
    row.finish(x, rgba, depthval, iterz, iterw, a_of(st.za_color), stats);
}

/// Fill pixels `startx..stopx` of row `y` for a fastfill: the colour
/// buffer with the dither pattern of color1, and the auxiliary buffer
/// with zaColor's depth (`raster_fastfill`).
pub fn fastfill_row(st: &RasterState, dither: &[u16; 16], y: i32, startx: i32, stopx: i32, stats: &mut Stats) {
    let scry = screen_y(st, y, bit(st.fbz_mode, 17));
    let (startx, stopx) = (startx.max(0) as usize, stopx.max(0) as usize);
    let row = scry as usize * st.rowpixels;
    let len = st.fb.len();
    if bit(st.fbz_mode, 9) {
        let pattern = &dither[(y as usize & 3) * 4..(y as usize & 3) * 4 + 4];
        for x in startx..stopx {
            let at = st.dest + row + x;
            if at < len {
                st.fb.set(at, pattern[x & 3]);
            }
        }
        stats.pixels_out += (stopx - startx) as i32;
    }
    if bit(st.fbz_mode, 10)
        && let Some(aux) = st.aux
    {
        let color = st.za_color as u16;
        for x in startx..stopx {
            let at = aux + row + x;
            if at < len {
                st.fb.set(at, color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterated_colours_wrap_as_the_hardware_does() {
        // 255.5 stays 255; 256 is 255 too; -1 (FFF) is 0; 257 wraps to 1.
        assert_eq!(clamped_argb(0xFF << 12 | 0x800, 0, 0, 0, 0) >> 16 & 0xFF, 0xFF);
        assert_eq!(clamped_argb(0x100 << 12, 0, 0, 0, 0) >> 16 & 0xFF, 0xFF);
        assert_eq!(clamped_argb(-(1 << 12), 0, 0, 0, 0) >> 16 & 0xFF, 0);
        assert_eq!(clamped_argb(0x101 << 12, 0, 0, 0, 0) >> 16 & 0xFF, 1);
        assert_eq!(clamped_z(0x1_0000 << 12, 0), 0xFFFF);
        assert_eq!(clamped_z(0x1234 << 12, 0), 0x1234);
    }

    #[test]
    fn w_as_floating_point() {
        // W = 1.0 (1 << 32 in 16.32) has bits 32+ set: nearest.
        assert_eq!(wfloat(1 << 32), 0);
        // Small W is far away.
        assert_eq!(wfloat(0x8000), 0xFFFF);
        // 0.5: one leading zero.
        assert_eq!(wfloat(0x8000_0000), 0x0000 | ((!0x8000_0000u32 >> 19) & 0xFFF) as i32 + 1);
    }

    #[test]
    fn combine_passes_c_other_through() {
        // Default fbzColorPath 0: c_other, times (0 ^ FF) + 1 = 256.
        assert_eq!(combine(0, 0x80_40_20_10, 0, 0), (0x40, 0x20, 0x10, 0x80));
        // Zero other, add c_local: c_local.
        assert_eq!(combine(1 << 8 | 1 << 17 | 1 << 14 | 1 << 23, 0xFFFF_FFFF, 0x0112_2334, 0), (0x12, 0x23, 0x34, 0x01));
    }
}

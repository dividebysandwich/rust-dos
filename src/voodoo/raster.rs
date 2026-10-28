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

#[path = "raster/dispatch.rs"]
mod dispatch;

#[cfg(all(test, feature = "sdl"))]
#[path = "raster/scene_profile.rs"]
pub(super) mod scene_profile;

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
    /// Let OpenGL distinguish forced point sampling from the game's choice.
    pub unfiltered: bool,
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
    /// The fraction bits bilinear filtering blends with: 4 on a Voodoo
    /// Graphics (F0h), all 8 on a Voodoo2.
    pub bilinear_mask: i32,
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
    /// The bits of the clip registers' fields: 10 on a Voodoo Graphics, 12
    /// on a Voodoo2.
    pub clip_mask: u32,
    /// chromaRange (Voodoo2; 0 has the range test off).
    pub chroma_range: u32,
    /// The bits of a fog table delta used: all on a Voodoo Graphics, the
    /// top 6 on a Voodoo2.
    pub fogdelta_mask: i32,
    pub yorigin: u32,
    /// The bits of a row counted from the Y origin at the bottom: 10 on a
    /// Voodoo Graphics and Voodoo2, where rows wrap around, all (-1) on a
    /// Banshee.
    pub ymask: i32,
    /// The pixels in a row of the colour buffer, and how its rows and
    /// the auxiliary buffer's lie in memory.
    pub rowpixels: usize,
    pub stride: Stride,
    pub aux_stride: Stride,
    /// Word offsets of the colour buffer drawn into and of the auxiliary
    /// buffer, if there is one.
    pub dest: usize,
    pub aux: Option<usize>,
    pub fogblend: [u8; 64],
    pub fogdelta: [u8; 64],
    /// With trexInit1 bit 18, TMU 0 hands out its configuration (how
    /// Glide counts the texture units) rather than texels.
    pub send_config: Option<u32>,
    /// The stipple register, which pattern-mode stippling reads.
    pub stipple: u32,
    pub tmu: [Option<TmuRaster>; 2],
}

/// How the rows of a buffer lie in memory: one after another, `words`
/// apart, or (a Banshee's tiled buffers) in tiles of 64 pixels by 32
/// rows, 2048 words each, with `words` from one row of tiles to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stride {
    pub words: usize,
    pub tiled: bool,
}

impl Stride {
    pub const fn linear(words: usize) -> Self {
        Self { words, tiled: false }
    }

    /// The word offset of row `y`'s first pixel.
    #[inline(always)]
    pub fn row(self, y: usize) -> usize {
        if self.tiled { (y >> 5) * self.words + (y & 31) * 64 } else { y * self.words }
    }

    /// The word offset of pixel `x` from its row's first.
    #[inline(always)]
    pub fn col(self, x: usize) -> usize {
        if self.tiled { (x & 63) | (x >> 6) << 11 } else { x }
    }

    /// The pixels a row holds.
    pub fn pixels(self) -> usize {
        if self.tiled { self.words / 2048 * 64 } else { self.words }
    }
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

impl RasterState {
    /// The clip rectangle: left, right, low Y and high Y.
    pub fn clip(&self) -> (i32, i32, i32, i32) {
        let (lr, ly, m) = (self.clip_left_right, self.clip_low_y_high_y, self.clip_mask);
        (((lr >> 16) & m) as i32, (lr & m) as i32, ((ly >> 16) & m) as i32, (ly & m) as i32)
    }

    /// Whether the chroma key rejects `color` (`chroma_key_test`): the key
    /// itself, or with chromaRange bit 28 (a Voodoo2's) its range test,
    /// each component between the key's and the range's, or outside with
    /// its exclusive bit, and all of them or (union mode) any.
    pub fn chroma_rejects(&self, color: u32) -> bool {
        let range = self.chroma_range;
        if range & (1 << 28) == 0 {
            return (color ^ self.chroma_key) & 0xFF_FFFF == 0;
        }
        let mut results = 0;
        for (i, shift) in [0u32, 8, 16].into_iter().enumerate() {
            let (low, high, test) = ((self.chroma_key >> shift) & 0xFF, (range >> shift) & 0xFF, (color >> shift) & 0xFF);
            let inside = (low <= test && test <= high) as u32;
            results |= (inside ^ ((range >> (24 + i)) & 1)) << i;
        }
        if range & (1 << 27) != 0 { results != 0 } else { results == 7 }
    }
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

/// The mode registers the pixel pipeline follows: fbzMode,
/// fbzColorPath, alphaMode, fogMode and the texture units' textureMode.
/// `scanline` may have some of their bits fixed when it is compiled (see
/// `Spec`), which leaves out the code for the others.
#[derive(Clone, Copy)]
pub(crate) struct Modes {
    fbz: u32,
    cp: u32,
    am: u32,
    fog: u32,
    tex: [u32; 2],
}

impl Modes {
    /// The registers as `st` has them, with the bits `S` fixes as it has
    /// them (the triangle's are those, see `Spec::matches`).
    #[inline(always)]
    fn of<S: Spec>(st: &RasterState) -> Self {
        let fix = |reg: u32, (known, value): (u32, u32)| (reg & !known) | value;
        let f = S::FIXED;
        let tex = |i: usize| st.tmu[i].as_ref().map_or(0, |t| t.mode);
        Modes {
            fbz: fix(st.fbz_mode, f[0]),
            cp: fix(st.fbz_color_path, f[1]),
            am: fix(st.alpha_mode, f[2]),
            fog: fix(st.fog_mode, f[3]),
            tex: [fix(tex(0), f[4]), fix(tex(1), f[5])],
        }
    }
}

/// Where the pixels of one scanline go: the rows of the colour and
/// auxiliary buffers, and the dither rows for the scanline's Y.
struct Row<'a> {
    st: &'a RasterState,
    m: Modes,
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
    fn new(st: &'a RasterState, m: Modes, y: i32, scry: i32) -> Self {
        let fbz = m.fbz;
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
        let scry = scry as usize;
        Self {
            st,
            m,
            dest: st.dest + st.stride.row(scry),
            depth: st.aux.map(|aux| aux + st.aux_stride.row(scry)),
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

    /// The word of pixel `x` in the colour buffer's row.
    #[inline(always)]
    fn color_word(&self, x: i32) -> usize {
        self.dest + self.st.stride.col(x as usize)
    }

    fn depth_at(&self, x: i32) -> Option<i32> {
        self.depth.map(|d| self.get(d + self.st.aux_stride.col(x as usize)) as i32)
    }

    /// Stippling, the depth value and the depth test
    /// (`PIXEL_PIPELINE_BEGIN`). The depth value, or None to skip the
    /// pixel.
    #[inline(always)]
    fn begin(&self, x: i32, y: i32, iterz: i32, iterw: i64, stipple: &mut u32, stats: &mut Stats) -> Option<i32> {
        let st = self.st;
        let fbz = self.m.fbz;
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
                if (st.stipple >> index) & 1 == 0 {
                    return None;
                }
            }
        }

        let mut depthval = if !bit(fbz, 3) {
            clamped_z(iterz, self.m.cp)
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
        let (st, m) = (self.st, self.m);
        if bit(m.fbz, 1) && st.chroma_rejects(color) {
            stats.chroma_fail += 1;
            return false;
        }
        if bit(m.fbz, 13) && alpha & 1 == 0 {
            stats.afunc_fail += 1;
            return false;
        }
        if bit(m.am, 0) {
            let reference = (m.am >> 24) as i32;
            let pass = match (m.am >> 1) & 7 {
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
        let fbz = self.m.fbz;
        let (mut r, mut g, mut b, mut a) = color;
        let (prefogr, prefogg, prefogb) = (r, g, b);

        // Fog.
        let fog = self.m.fog;
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
                        let mut deltaval = (delta & st.fogdelta_mask) * ((w >> 2) & 0xFF);
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
                    2 => clamped_z(iterz, self.m.cp) >> 8,
                    _ => clamped_w(iterw, self.m.cp),
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
        let am = self.m.am;
        if bit(am, 4) {
            let dpix = self.get(self.color_word(x)) as i32;
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
            self.set(self.color_word(x), r5 << 11 | g6 << 5 | b5);
        }
        // Depth, or alpha with the alpha planes.
        if let Some(depth) = self.depth
            && bit(fbz, 10)
        {
            let value = if bit(fbz, 18) { a } else { depthval };
            self.set(depth + self.st.aux_stride.col(x as usize), value as u16);
        }
        stats.pixels_out += 1;
    }
}

/// The ARGB colour of texel `s`, `tc` of the level at `texbase`, which is
/// `smax + 1` texels wide.
#[inline(always)]
pub(crate) fn fetch_texel(t: &TmuRaster, texbase: u32, smax: i32, s: i32, tc: i32) -> u32 {
    fetch_texel_in(t, t.mode, texbase, smax, s, tc)
}

/// `fetch_texel` with the textureMode `mode` (see `Modes`).
#[inline(always)]
fn fetch_texel_in(t: &TmuRaster, mode: u32, texbase: u32, smax: i32, s: i32, tc: i32) -> u32 {
    let format = (mode >> 8) & 0xF;
    let index = (tc as u32).wrapping_mul(smax as u32 + 1).wrapping_add(s as u32);
    // The addresses are masked to the memory's size (a power of two), and
    // the tables have an entry for every byte, or for every word of the
    // 16-bit formats that look a word up (`Tmu::lookup`).
    debug_assert!(t.mask as usize + 1 == t.ram.bytes() && t.lookup.len() >= 256);
    let lookup = |i: usize| {
        debug_assert!(i < t.lookup.len());
        // SAFETY: see above.
        unsafe { *t.lookup.get_unchecked(i) }
    };
    if format < 8 {
        let a = (texbase.wrapping_add(index) & t.mask) as usize;
        // SAFETY: a byte address within the memory.
        let word = unsafe { t.ram.get_unchecked(a >> 1) };
        lookup((word >> ((a & 1) * 8)) as u8 as usize)
    } else {
        // SAFETY: a word within the memory.
        let texel = unsafe { t.ram.get_unchecked(((texbase.wrapping_add(index.wrapping_mul(2)) & t.mask) >> 1) as usize) } as u32;
        if (10..=12).contains(&format) {
            debug_assert!(t.lookup.len() == 0x10000);
            lookup(texel as usize)
        } else {
            (lookup((texel & 0xFF) as usize) & 0xFF_FFFF) | ((texel & 0xFF00) << 16)
        }
    }
}

/// Whether the texture combine of textureMode `mode` reads the unit's own
/// texel (c_local). A unit that only passes on the colour of the one
/// before it, as TMU 0 does under 3dfx's OpenGL driver with one texture,
/// doesn't.
const fn uses_local(mode: u32) -> bool {
    bit(mode, 13)
        || matches!((mode >> 14) & 7, 1 | 3)
        || matches!((mode >> 18) & 3, 1 | 2)
        || bit(mode, 22)
        || matches!((mode >> 23) & 7, 1 | 3)
        || (mode >> 27) & 3 != 0
}

/// A texture unit's colour for a pixel, combined with `cother`, the
/// colour of the unit before it (`TEXTURE_PIPELINE`).
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn texture(t: &TmuRaster, mode: u32, x: i32, dither4: Option<&[u8]>, cother: u32, lodbase: i32, iters: i64, itert: i64, iterw: i64, rl: Option<(i64, i32)>) -> u32 {
    let (mut s, mut tc, mut lod);
    if bit(mode, 0) {
        // (`rl`: the reciprocal and log of `iterw`, where the other unit
        // worked them out for the same W.)
        let (oow, log) = rl.unwrap_or_else(|| tables().reciplog(iterw));
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
    let fetch = |s: i32, tc: i32| fetch_texel_in(t, mode, texbase, smax, s, tc);

    let point = (lod == t.lodmin && !bit(mode, 2)) || (lod != t.lodmin && !bit(mode, 1));
    let c_local = if !uses_local(mode) {
        // Nothing reads it: skip the lookups.
        0
    } else if point {
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
        // A Voodoo Graphics filters with 4 bits of fraction, a Voodoo2
        // with 8.
        let sfrac = (s & t.bilinear_mask) as u32;
        let tfrac = (tc & t.bilinear_mask) as u32;
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
    texture_combine(t, mode, lod, cother, c_local)
}

/// The texture combine unit of textureMode `mode`: the unit's texel
/// `c_local` with `cother`, at level of detail `lod`.
#[inline(always)]
fn texture_combine(t: &TmuRaster, mode: u32, lod: i32, cother: u32, c_local: u32) -> u32 {
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
    let cp = row.m.cp;
    let mut c_other = match cp & 3 {
        0 => iterargb,
        1 => texel,
        2 => st.color1,
        _ => 0,
    };
    // The chroma key tests c_other before its alpha is chosen.
    if bit(row.m.fbz, 1) && st.chroma_rejects(c_other) {
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
    if bit(row.m.fbz, 13) && a_other & 1 == 0 {
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
        let am = self.m.am;
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

/// Bits of the mode registers a `scanline` is compiled for (`Modes`): per
/// register (fbzMode, fbzColorPath, alphaMode, fogMode, and textureMode of
/// TMU 0 and 1), the bits fixed and their values. The compiler leaves out
/// the stages and choices those bits rule out, as MAME's rasterizer cache
/// does for the modes games use; any triangle whose registers have those
/// bits as fixed may be drawn with it (`variant`), the others by the
/// `Generic` one, which fixes none.
pub trait Spec {
    const FIXED: [(u32, u32); 6];
}

/// No bits fixed: every stage as the registers say.
pub struct Generic;

impl Spec for Generic {
    const FIXED: [(u32, u32); 6] = [(0, 0); 6];
}

/// The bits of fbzMode a variant fixes: those that choose the stages and
/// sources (stipple, depth from W and as floating point, the depth test
/// on, dithering and its kind, depth bias, the alpha planes, dither
/// subtraction, the depth source), not the depth function, writes,
/// clipping, chroma key or alpha mask, which stay as the triangle has
/// them. alphaMode's: all but the reference value of the alpha test.
const FBZ_FIXED: u32 = 1 << 2 | 1 << 3 | 1 << 4 | 1 << 8 | 1 << 11 | 1 << 12 | 1 << 16 | 1 << 18 | 1 << 19 | 1 << 20 | 1 << 21;
const AM_FIXED: u32 = 0x00FF_FFFF;
/// fbzColorPath's: all the raster reads, that is all but the subpixel
/// adjustment (26, applied at setup), the texture enable (27, which the
/// unit count stands for) and anti-aliasing (29).
const CP_FIXED: u32 = 0x13FF_FFFF;

/// A variant that fixes every bit that chooses a stage or a source:
/// fbzMode, alphaMode and fbzColorPath as `FBZ_FIXED`, `AM_FIXED` and
/// `CP_FIXED` say, and all of fogMode and the textureMode of the units
/// used.
const fn full(tmus: usize, [fbz, cp, am, fog, t0, t1]: [u32; 6]) -> [(u32, u32); 6] {
    [
        (FBZ_FIXED, fbz & FBZ_FIXED),
        (CP_FIXED, cp & CP_FIXED),
        (AM_FIXED, am & AM_FIXED),
        (u32::MAX, fog),
        (if tmus >= 1 { u32::MAX } else { 0 }, t0),
        (if tmus >= 2 { u32::MAX } else { 0 }, t1),
    ]
}

/// The bits of textureMode that choose the texture combine (12-29).
const TC_FIXED: u32 = 0x3FFF_F000;

/// A variant that fixes only fbzColorPath (as `CP_FIXED` says) and the
/// texture combine of the units used, and leaves fbzMode, alphaMode,
/// fogMode and the texture filtering as the triangle has them. Those cost
/// a variant little against the generic code, the combines most of the
/// difference, so one of these covers the depth, blending and fog settings
/// a game draws a colour path with.
const fn combine_only(tmus: usize, [cp, t0, t1]: [u32; 3]) -> [(u32, u32); 6] {
    [
        (0, 0),
        (CP_FIXED, cp & CP_FIXED),
        (0, 0),
        (0, 0),
        (if tmus >= 1 { TC_FIXED } else { 0 }, t0 & TC_FIXED),
        (if tmus >= 2 { TC_FIXED } else { 0 }, t1 & TC_FIXED),
    ]
}

/// A selected software scanline pipeline.
pub(crate) type Scanline = fn(&RasterState, &TriParams, i32, i32, i32, &mut u32, &mut Stats);

/// The variants: textures used, and the registers' fixed bits, from
/// `full` or `combine_only`. The first that fits a triangle draws it
/// (`variant`).
macro_rules! variants {
    ($($name:ident: $tmus:literal, $kind:ident [$($reg:expr),*];)*) => {
        $(
            struct $name;
            impl Spec for $name {
                const FIXED: [(u32, u32); 6] = $kind($tmus, [$($reg),*]);
            }
        )*
        $(
            impl $name {
                const KEY: dispatch::Key = dispatch::Key::$kind($tmus, [$($reg),*]);
            }
        )*
        #[repr(usize)]
        enum VariantId {
            $($name),*
        }

        #[cfg(test)]
        const VARIANTS: &[(usize, [(u32, u32); 6])] = &[$(($tmus, $name::FIXED)),*];
        const SCANLINES: &[Scanline] = &[$(scanline::<$tmus, $name>),*];

        const VARIANT_COUNTS: usize = 0 $(| (1 << $tmus))*;
        const FIRST_COMBINE: usize = {
            let keys = [$($name::KEY),*];
            let mut i = 0;
            while i < keys.len() {
                if matches!(keys[i], dispatch::Key::Combine(..)) {
                    break;
                }
                i += 1;
            }
            i
        };

        #[inline(always)]
        fn variant_index(key: dispatch::Key) -> Option<usize> {
            match key {
                $($name::KEY => Some(VariantId::$name as usize),)*
                _ => None,
            }
        }

        /// Draw a scanline with variant `variant` (from `variant`), or the
        /// generic code for `texcount` units.
        #[allow(clippy::too_many_arguments)]
        pub fn scanline_as(variant: Option<usize>, texcount: usize, st: &RasterState, p: &TriParams, y: i32, startx: i32, stopx: i32, stipple: &mut u32, stats: &mut Stats) {
            pipeline(variant, texcount)(st, p, y, startx, stopx, stipple, stats);
        }

        /// Select once for all the scanlines of a triangle.
        pub(crate) fn pipeline(variant: Option<usize>, texcount: usize) -> Scanline {
            if let Some(&draw) = variant.and_then(|i| SCANLINES.get(i)) {
                return draw;
            }
            match texcount {
                0 => scanline::<0, Generic>,
                1 => scanline::<1, Generic>,
                _ => scanline::<2, Generic>,
            }
        }
    };
}

// V0-V7 are the pipelines of Quake's 3dfx renderers (lightmapped textures
// in two units, blended or not, and the untextured ones), which other
// Glide games share. V8 and V9 fix the two states observed in a 60-batch
// Oni main-menu capture. C0 and C1 are 3dfx's OpenGL driver (`3DFXVGL.DLL`)
// with one texture, which it gives TMU 1 while TMU 0 passes its colour
// on: the texture times the iterated colour, with the iterated alpha or
// the texture's times it. Oni draws its menu and its levels with these.
variants! {
    V0: 2, full[0x90110, 0x0D42_0009, 0x0004_0400, 0, 0x8C26_1A0F, 0x8422_1C0F];
    V1: 2, full[0x90110, 0x0D42_0009, 0x0000_6010, 0, 0x8C26_130F, 0x8422_1C0F];
    V2: 2, full[0x90110, 0x0D42_0009, 0x0004_0409, 0, 0x8844_2A0F, 0x8C26_1A0F];
    V3: 0, full[0x90110, 0x0542_610A, 0x0004_5110, 0, 0, 0];
    V4: 2, full[0x90110, 0x0D42_0009, 0x0000_6010, 0, 0x8C26_130F, 0x8422_1A0F];
    V5: 2, full[0x90110, 0x0D42_0009, 0x0000_6010, 0, 0x8844_230F, 0x8C26_130F];
    V6: 2, full[0x90110, 0x0C00_0005, 0x0004_0409, 0, 0x8844_2A0F, 0x8C26_1C0F];
    V7: 2, full[0x90110, 0x0D42_0009, 0x0000_6010, 0, 0x8C26_130F, 0x8422_130F];
    V8: 2, full[0x80100, 0x0048_2405, 0x0000_5110, 0xC0, 0x8000_0006, 0x8C26_1C0F];
    V9: 2, full[0x80100, 0x0142_2409, 0x0000_5110, 0xC0, 0x8000_0006, 0x8C26_1ACF];
    C0: 2, combine_only[0x0D42_2409, 0x8000_0006, 0x8C26_1A0F];
    C1: 2, combine_only[0x0C48_2405, 0x8000_0006, 0x8C26_1C0F];
}

/// The variant a triangle with `texcount` units and `st`'s registers can be
/// drawn with: one whose fixed bits the registers have.
pub fn variant(st: &RasterState, texcount: usize) -> Option<usize> {
    let tmus = texcount.min(2);
    if VARIANT_COUNTS & (1 << tmus) == 0 {
        return None;
    }
    let tex = |i: usize| {
        if i < tmus {
            st.tmu[i].as_ref().map_or(0, |t| t.mode)
        } else {
            0
        }
    };
    let regs = [
        st.fbz_mode,
        st.fbz_color_path,
        st.alpha_mode,
        st.fog_mode,
        tex(0),
        tex(1),
    ];
    dispatch::variant(regs, tmus)
}

/// The screen row scanline `y` draws into: flipped with the Y origin at
/// the bottom (fbzMode bit 17).
pub(crate) fn screen_y(st: &RasterState, y: i32, flip: bool) -> i32 {
    if flip { (st.yorigin as i32 - y) & st.ymask } else { y }
}

/// Draw pixels `startx..stopx` of scanline `y` of a triangle with `TMUS`
/// texture units (`raster_generic`).
pub fn scanline<const TMUS: usize, S: Spec>(st: &RasterState, p: &TriParams, y: i32, startx: i32, stopx: i32, stipple: &mut u32, stats: &mut Stats) {
    let (mut startx, mut stopx) = (startx, stopx);
    let scry = screen_y(st, y, bit(st.fbz_mode, 17));
    if bit(st.fbz_mode, 0) {
        // Clipping: a row outside takes the whole scanline.
        let (left, right, low, high) = st.clip();
        if scry < low || scry >= high {
            stats.pixels_in += stopx - startx;
            stats.clip_fail += stopx - startx;
            return;
        }
        if startx < left {
            stats.pixels_in += left - startx;
            startx = left;
        }
        // Pixels left of clipRight are drawn: Glide's clip window of
        // the whole screen sets it to the screen's width.
        if stopx > right {
            stats.pixels_in += stopx - right;
            stopx = right;
        }
    }
    // Rows off the buffer, and pixels left of it, have nowhere to go.
    if scry < 0 || startx < 0 && stopx <= 0 {
        return;
    }
    let startx = startx.max(0);
    let m = Modes::of::<S>(st);
    let row = Row::new(st, m, y, scry);

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
    // Both units iterating the same W (as Glide sets them up) divide by it
    // once.
    let (w0, w1) = (&p.tmu[0], &p.tmu[1]);
    let same_w = TMUS >= 2
        && (w0.startw, w0.dwdx, w0.dwdy) == (w1.startw, w1.dwdx, w1.dwdy)
        && st.tmu[0].as_ref().is_some_and(|t| bit(m.tex[0], 0) && t.lodmin < 8 << 8)
        && st.tmu[1].as_ref().is_some_and(|t| bit(m.tex[1], 0) && t.lodmin < 8 << 8)
        && st.send_config.is_none();

    for x in startx..stopx {
        if let Some(depthval) = row.begin(x, y, iterz, iterw, stipple, stats) {
            // TMU 1 first, whose output TMU 0 combines with.
            let mut texel = 0u32;
            let rl = same_w.then(|| tables().reciplog(tex[1].2));
            if TMUS >= 2
                && let Some(t) = &st.tmu[1]
                && t.lodmin < 8 << 8
            {
                let (s, tc, w) = tex[1];
                texel = texture(t, m.tex[1], x, row.dither4, texel, p.tmu[1].lodbase, s, tc, w, rl);
            }
            if TMUS >= 1
                && let Some(t) = &st.tmu[0]
                && t.lodmin < 8 << 8
            {
                texel = match st.send_config {
                    Some(config) => config,
                    None => {
                        let (s, tc, w) = tex[0];
                        texture(t, m.tex[0], x, row.dither4, texel, p.tmu[0].lodbase, s, tc, w, rl)
                    }
                };
            }
            let iterargb = clamped_argb(iterr, iterg, iterb, itera, m.cp);
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
    let (left, right, low, high) = st.clip();
    if bit(st.fbz_mode, 0) && (x < left || x >= right || scry < low || scry >= high) {
        stats.pixels_in += 1;
        stats.clip_fail += 1;
        return;
    }
    if scry < 0 || x < 0 {
        return;
    }
    let row = Row::new(st, Modes::of::<Generic>(st), y, scry);
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
    if scry < 0 {
        return;
    }
    let (startx, stopx) = (startx.max(0) as usize, stopx.max(0) as usize);
    let len = st.fb.len();
    if bit(st.fbz_mode, 9) {
        let pattern = &dither[(y as usize & 3) * 4..(y as usize & 3) * 4 + 4];
        let row = st.dest + st.stride.row(scry as usize);
        for x in startx..stopx {
            let at = row + st.stride.col(x);
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
        let row = aux + st.aux_stride.row(scry as usize);
        for x in startx..stopx {
            let at = row + st.aux_stride.col(x);
            if at < len {
                st.fb.set(at, color);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
        assert_eq!(wfloat(0x8000_0000), ((!0x8000_0000u32 >> 19) & 0xFFF) as i32 + 1);
    }

    #[test]
    fn combine_passes_c_other_through() {
        // Default fbzColorPath 0: c_other, times (0 ^ FF) + 1 = 256.
        assert_eq!(combine(0, 0x80_40_20_10, 0, 0), (0x40, 0x20, 0x10, 0x80));
        // Zero other, add c_local: c_local.
        assert_eq!(combine(1 << 8 | 1 << 17 | 1 << 14 | 1 << 23, 0xFFFF_FFFF, 0x0112_2334, 0), (0x12, 0x23, 0x34, 0x01));
    }

    /// Two units with bilinear 256x256 textures of 16-bit texels, a
    /// 1024-pixel-wide colour buffer with a depth buffer after it, and
    /// fbzMode, fbzColorPath, alphaMode, fogMode and the units' modes from
    /// `regs`.
    pub(crate) fn two_unit_state(regs: [u32; 6]) -> RasterState {
        let lookup: Arc<[u32]> = (0..0x10000u32).map(|i| i.wrapping_mul(0x9E37_79B9)).collect();
        let tmu = |mode: u32| {
            let ram = Vram::new(1 << 20);
            for i in 0..ram.len() {
                ram.set(i, (i as u16).wrapping_mul(40503));
            }
            TmuRaster {
                ram,
                mask: (1 << 20) - 1,
                mode,
                unfiltered: false,
                lodmin: 0,
                lodmax: 8 << 8,
                lodbias: 0,
                lodmask: 0x1FF,
                lodoffset: [
                    0, 0x20000, 0x28000, 0x2A000, 0x2A800, 0x2AA00, 0x2AA80, 0x2AAA0, 0x2AAA8,
                ],
                detailmax: 0,
                detailbias: 0,
                detailscale: 0,
                wmask: 255,
                hmask: 255,
                lookup: lookup.clone(),
                bilinear_mask: 0xFF,
            }
        };
        let fb = Vram::new(4 << 20);
        for i in 0..fb.len() {
            fb.set(i, (i as u16).wrapping_mul(25173));
        }
        RasterState {
            fb,
            fbz_mode: regs[0],
            fbz_color_path: regs[1],
            alpha_mode: regs[2],
            fog_mode: regs[3],
            za_color: 0,
            chroma_key: 0,
            color0: 0x8040_2010,
            color1: 0x1020_4080,
            fog_color: 0x0030_3030,
            clip_left_right: 640,
            clip_low_y_high_y: 480,
            clip_mask: 0xFFF,
            chroma_range: 0,
            fogdelta_mask: 0xFC,
            yorigin: 479,
            ymask: 0x3FF,
            rowpixels: 1024,
            stride: Stride::linear(1024),
            aux_stride: Stride::linear(1024),
            dest: 0,
            aux: Some(1 << 20),
            fogblend: std::array::from_fn(|i| (i * 4) as u8),
            fogdelta: [4; 64],
            send_config: None,
            stipple: 0,
            tmu: [Some(tmu(regs[4])), Some(tmu(regs[5]))],
        }
    }

    #[test]
    fn texture_combines_without_local_ignore_the_texel() {
        let t = TmuRaster {
            ram: Vram::new(2),
            mask: 1,
            mode: 0,
            unfiltered: false,
            lodmin: 0,
            lodmax: 8 << 8,
            lodbias: 0,
            lodmask: 0x1FF,
            lodoffset: [0; 9],
            detailmax: 0xFF,
            detailbias: 0x400,
            detailscale: 2,
            wmask: 255,
            hmask: 255,
            lookup: Arc::new([0; 256]),
            bilinear_mask: 0xFF,
        };
        let (lod, cother) = (0x123, 0x9A3C_E571);
        for tc in 0..1u32 << 18 {
            let mode = tc << 12;
            if uses_local(mode) {
                continue;
            }
            let a = texture_combine(&t, mode, lod, cother, 0);
            for c_local in [0xFFFF_FFFF, 0x8040_C020] {
                let b = texture_combine(&t, mode, lod, cother, c_local);
                assert_eq!(a, b, "textureMode {mode:08X} reads c_local");
            }
        }
    }

    #[test]
    fn combine_only_variants_draw_as_the_generic_code() {
        // The states Oni draws with through 3dfx's OpenGL driver, which
        // differ in fbzMode, alphaMode and the texture formats as well.
        let states = [
            [0x0008_43E1, 0x0C48_2405, 0x0000_5110, 0xC0, 0x8000_0006, 0x8C26_1C0F],
            [0x0008_43E1, 0x0D42_2409, 0x0000_5110, 0xC0, 0x8000_0006, 0x8C26_1ACF],
            [0x0009_4771, 0x0D42_2409, 0x0000_5110, 0xC0, 0x8000_0006, 0x8C26_1A0F],
            [0x0009_4371, 0x0C48_2405, 0x0000_4110, 0xC0, 0x8000_0006, 0x8C26_1CCF],
            [0x0009_4771, 0x0D42_2409, 0x0000_0410, 0xC0, 0x8000_0006, 0x8C26_1A0F],
            [0x0009_4771, 0x0C48_2405, 0x0000_1410, 0xC0, 0x8000_0006, 0x8C26_1C0F],
            // And two with the colour path bits the raster doesn't read
            // changed: no subpixel adjustment, and anti-aliasing on.
            [0x0009_4771, 0x0942_2409, 0x0000_5110, 0xC0, 0x8000_0006, 0x8C26_1A0F],
            [0x0008_43E1, 0x2C48_2405, 0x0000_5110, 0xC0, 0x8000_0006, 0x8C26_1C0F],
        ];
        let tp = TmuParams {
            starts: 10 << 32,
            startt: 20 << 32,
            startw: 1 << 32,
            dsdx: 3 << 28,
            dtdx: 1 << 27,
            dwdx: 1 << 20,
            dsdy: 1 << 27,
            dtdy: 3 << 28,
            dwdy: 1 << 20,
            lodbase: 0,
        };
        let p = TriParams {
            startr: 100 << 12,
            startg: 50 << 12,
            startb: 20 << 12,
            starta: 200 << 12,
            startz: 0x4000 << 12,
            startw: 1 << 32,
            drdx: 1 << 10,
            dgdx: 1 << 9,
            dbdx: 1 << 8,
            dadx: -(1 << 10),
            dzdx: 1 << 14,
            dwdx: 1 << 20,
            drdy: 1 << 9,
            dzdy: -(1 << 13),
            dwdy: 1 << 20,
            tmu: [tp, tp],
            ..Default::default()
        };
        for regs in states {
            let st = two_unit_state(regs);
            let combine_only = [C0::FIXED, C1::FIXED];
            let v = combine_only
                .iter()
                .position(|fixed| {
                    regs.iter()
                        .zip(fixed)
                        .all(|(r, (mask, value))| r & mask == *value)
                })
                .map(|i| FIRST_COMBINE + i);
            assert!(
                v.is_some_and(|i| combine_only.contains(&VARIANTS[i].1)),
                "no combine-only variant for {regs:08X?}"
            );
            let generic = two_unit_state(regs);
            let (mut a, mut b) = (0, 0);
            let (mut sa, mut sb) = (Stats::default(), Stats::default());
            for y in 0..64 {
                scanline_as(v, 2, &st, &p, y, 0, 320, &mut a, &mut sa);
                scanline::<2, Generic>(&generic, &p, y, 0, 320, &mut b, &mut sb);
            }
            let differ = (0..st.fb.len()).find(|&i| st.fb.get(i) != generic.fb.get(i));
            assert_eq!(differ, None, "{regs:08X?} drawn with variant {v:?}");
            assert!(sa.pixels_out > 0 && sa.pixels_out == sb.pixels_out);
        }
    }
}

#[cfg(test)]
#[path = "raster/dispatch_output_tests.rs"]
mod dispatch_output_tests;

#[cfg(test)]
#[path = "raster/oni_state_tests.rs"]
mod oni_state_tests;

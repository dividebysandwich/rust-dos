//! The 3D RAGE's 3D engine and scaler ([RRG] chapter 6): the colour, Z
//! and texture coordinate interpolators that follow a trapezoid's leading
//! edge, the texture units (mipmaps, bilinear filtering, the texel
//! formats), lighting, fog, alpha blending and the Z buffer, and the
//! scaler that stretches an image into a rectangle.
//!
//! Interpolators are kept in the registers' own units: colours S.8.12
//! from bit 4 (the colour's integer part at bit 16), Z S.16.12, S and T
//! with their binary point at bit 16 and 1024.0 one wrap of the map.
//! There is no divide by W: the second-order increments approximate the
//! perspective.

use super::Mach64;
use super::engine::{Memory, Size};
use super::regs::*;

/// An 8-bit colour with alpha, as the pipe works on it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Argb {
    pub a: i32,
    pub r: i32,
    pub g: i32,
    pub b: i32,
}

impl Argb {
    const BLACK: Argb = Argb {
        a: 255,
        r: 0,
        g: 0,
        b: 0,
    };
    const NOTHING: Argb = Argb {
        a: 0,
        r: 0,
        g: 0,
        b: 0,
    };

    fn grey(v: i32) -> Argb {
        Argb {
            a: 255,
            r: v,
            g: v,
            b: v,
        }
    }

    /// Between `self` and `other`, `f` of 256 of the way.
    fn lerp(self, other: Argb, f: i32) -> Argb {
        let mix = |p: i32, q: i32| p + (((q - p) * f) >> 8);
        Argb {
            a: mix(self.a, other.a),
            r: mix(self.r, other.r),
            g: mix(self.g, other.g),
            b: mix(self.b, other.b),
        }
    }
}

/// The colours the pipe reads that aren't in its memory: the VGA DAC's
/// palette (6 or 8 bits a component), which 8-bit texels go through on
/// the 3D RAGE, and the 3D RAGE II's texture palette.
pub struct Palettes<'a> {
    pub dac: &'a [u8],
    pub dac_8bit: bool,
    pub texture: Option<&'a [u32]>,
}

impl Palettes<'_> {
    fn colour(&self, index: u8) -> Argb {
        if let Some(texture) = self.texture {
            let p = texture[index as usize];
            return Argb {
                a: 255,
                r: (p >> 16 & 0xFF) as i32,
                g: (p >> 8 & 0xFF) as i32,
                b: (p & 0xFF) as i32,
            };
        }
        let at = index as usize * 3;
        let c = |i: usize| {
            let v = self.dac.get(at + i).copied().unwrap_or(0) as i32;
            if self.dac_8bit {
                v
            } else {
                (v << 2 | v >> 4) & 0xFF
            }
        };
        Argb {
            a: 255,
            r: c(0),
            g: c(1),
            b: c(2),
        }
    }
}

fn five(v: u32) -> i32 {
    ((v << 3) | (v >> 2)) as i32
}

fn six(v: u32) -> i32 {
    ((v << 2) | (v >> 4)) as i32
}

fn four(v: u32) -> i32 {
    ((v << 4) | v) as i32
}

/// The bytes a texel of format `format` (a DP_PIX_WIDTH code) takes.
fn texel_bytes(format: u32) -> u32 {
    match format {
        2 | 7 | 8 => 1,
        6 | 14 => 4,
        _ => 2,
    }
}

/// A YUV colour as RGB (ITU-R BT.601), U and V around 128, or signed with
/// `signed_uv` (APPLE_YUV_MODE).
fn yuv(y: i32, u: i32, v: i32, signed_uv: bool) -> Argb {
    let (u, v) = if signed_uv {
        (u as u8 as i8 as i32, v as u8 as i8 as i32)
    } else {
        (u - 128, v - 128)
    };
    let clamp = |c: i32| c.clamp(0, 255);
    Argb {
        a: 255,
        r: clamp(y + ((359 * v) >> 8)),
        g: clamp(y - ((88 * u + 183 * v) >> 8)),
        b: clamp(y + ((453 * u) >> 8)),
    }
}

/// An image the pipe reads: texels of `format` from byte `base`, `pitch`
/// texels a row.
struct Image {
    base: u32,
    pitch: u32,
    format: u32,
}

/// What texels need beyond the memory: the palettes, the 3D RAGE II's
/// CI4 selection (DP_PIX_WIDTH bits 27-26 and the bank in 23-20), and the
/// YUV mode.
struct TexelSource<'a, 'm> {
    mem: &'a Memory<'m>,
    palettes: &'a Palettes<'a>,
    ci4: u32,
    signed_uv: bool,
}

impl TexelSource<'_, '_> {
    fn texel(&self, image: &Image, x: u32, y: u32) -> Argb {
        let byte_at = |at: u32| self.mem.read(at, Size::Byte);
        let at = image
            .base
            .wrapping_add((y * image.pitch + x) * texel_bytes(image.format));
        let word = || byte_at(at) | byte_at(at + 1) << 8;
        match image.format {
            2 => {
                let mut index = byte_at(at);
                // The 3D RAGE II's 4-bit texels: the low or high nibble,
                // with the bank above it.
                let bank = self.ci4 >> 20 & 0xF;
                match self.ci4 >> 26 & 3 {
                    1 => index = bank << 4 | index & 0xF,
                    2 => index = bank << 4 | index >> 4,
                    _ => {}
                }
                self.palettes.colour(index as u8)
            }
            7 => {
                let v = byte_at(at);
                Argb {
                    a: 255,
                    r: (v >> 5 & 7) as i32 * 255 / 7,
                    g: (v >> 2 & 7) as i32 * 255 / 7,
                    b: (v & 3) as i32 * 255 / 3,
                }
            }
            8 => Argb::grey(byte_at(at) as i32),
            3 => {
                let v = word();
                Argb {
                    a: if v & 0x8000 != 0 { 255 } else { 0 },
                    r: five(v >> 10 & 31),
                    g: five(v >> 5 & 31),
                    b: five(v & 31),
                }
            }
            4 => {
                let v = word();
                Argb {
                    a: 255,
                    r: five(v >> 11),
                    g: six(v >> 5 & 63),
                    b: five(v & 31),
                }
            }
            15 => {
                let v = word();
                Argb {
                    a: four(v >> 12),
                    r: four(v >> 8 & 15),
                    g: four(v >> 4 & 15),
                    b: four(v & 15),
                }
            }
            6 => Argb {
                a: byte_at(at + 3) as i32,
                r: byte_at(at + 2) as i32,
                g: byte_at(at + 1) as i32,
                b: byte_at(at) as i32,
            },
            // YUYV: a pair of pixels shares U and V.
            11 => {
                let pair = image.base.wrapping_add((y * image.pitch + (x & !1)) * 2);
                let luma = byte_at(pair + if x & 1 != 0 { 2 } else { 0 }) as i32;
                yuv(
                    luma,
                    byte_at(pair + 1) as i32,
                    byte_at(pair + 3) as i32,
                    self.signed_uv,
                )
            }
            14 => {
                let c = yuv(
                    byte_at(at + 2) as i32,
                    byte_at(at + 1) as i32,
                    byte_at(at) as i32,
                    self.signed_uv,
                );
                Argb {
                    a: byte_at(at + 3) as i32,
                    ..c
                }
            }
            _ => Argb::BLACK,
        }
    }
}

/// The destination's pixel as a colour, for blending.
fn unpack(v: u32, format: u32) -> Argb {
    match format {
        3 => Argb {
            a: if v & 0x8000 != 0 { 255 } else { 0 },
            r: five(v >> 10 & 31),
            g: five(v >> 5 & 31),
            b: five(v & 31),
        },
        4 => Argb {
            a: 255,
            r: five(v >> 11 & 31),
            g: six(v >> 5 & 63),
            b: five(v & 31),
        },
        15 => Argb {
            a: four(v >> 12 & 15),
            r: four(v >> 8 & 15),
            g: four(v >> 4 & 15),
            b: four(v & 15),
        },
        6 => Argb {
            a: (v >> 24) as i32,
            r: (v >> 16 & 0xFF) as i32,
            g: (v >> 8 & 0xFF) as i32,
            b: (v & 0xFF) as i32,
        },
        8 => Argb::grey((v & 0xFF) as i32),
        2 | 7 => Argb {
            a: 255,
            r: (v >> 5 & 7) as i32 * 255 / 7,
            g: (v >> 2 & 7) as i32 * 255 / 7,
            b: (v & 3) as i32 * 255 / 3,
        },
        _ => Argb::BLACK,
    }
}

/// The 4x4 ordered dither the pipe adds before it drops bits.
const DITHER: [[i32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// `c` as a pixel of `format` at `x`, `y`: its channels cut to the
/// format's bits after the dither (SCALE_3D_CNTL bit 2) or rounding
/// (bit 4). 8-bit destinations are RGB 3:3:2, red at most 6 with
/// RED_DITHER_MAX (bit 14).
fn pack(c: Argb, format: u32, x: i32, y: i32, cntl: u32) -> u32 {
    let cut = |v: i32, bits: i32| -> u32 {
        let drop = 8 - bits;
        let v = if cntl & 4 != 0 {
            v + ((DITHER[(y & 3) as usize][(x & 3) as usize] << drop) >> 4)
        } else if cntl & 0x10 != 0 {
            v + (1 << (drop - 1))
        } else {
            v
        };
        (v.clamp(0, 255) >> drop) as u32
    };
    let byte = |v: i32| v.clamp(0, 255) as u32;
    match format {
        3 => ((c.a >= 128) as u32 * 0x8000) | cut(c.r, 5) << 10 | cut(c.g, 5) << 5 | cut(c.b, 5),
        4 => cut(c.r, 5) << 11 | cut(c.g, 6) << 5 | cut(c.b, 5),
        15 => cut(c.a, 4) << 12 | cut(c.r, 4) << 8 | cut(c.g, 4) << 4 | cut(c.b, 4),
        6 => byte(c.a) << 24 | byte(c.r) << 16 | byte(c.g) << 8 | byte(c.b),
        8 => byte((c.r * 77 + c.g * 150 + c.b * 29) >> 8),
        _ => {
            let red = cut(c.r, 3);
            let red = if cntl & 1 << 14 != 0 { red.min(6) } else { red };
            red << 5 | cut(c.g, 3) << 2 | cut(c.b, 2)
        }
    }
}

/// A blend factor ([RRG] ALPHA_BLND_SRC/DST) on a scale of 256: zero,
/// one, the other operand's channel, one less it, the source alpha, one
/// less that.
fn factor(select: u32, alpha: i32, other: i32) -> i32 {
    let scale = |v: i32| v + (v >> 7);
    match select {
        0 => 0,
        2 => scale(other),
        3 => 256 - scale(other),
        4 => scale(alpha),
        5 => 256 - scale(alpha),
        _ => 256,
    }
}

/// `c` blended with the destination `d` by SCALE_3D_CNTL's factors.
fn blend(c: Argb, d: Argb, alpha: i32, cntl: u32) -> Argb {
    let (fs, fd) = (cntl >> 16 & 7, cntl >> 19 & 7);
    let channel =
        |s: i32, d: i32| ((s * factor(fs, alpha, d) + d * factor(fd, alpha, s)) >> 8).min(255);
    Argb {
        a: c.a,
        r: channel(c.r, d.r),
        g: channel(c.g, d.g),
        b: channel(c.b, d.b),
    }
}

/// A texture coordinate on the leading edge: its value, its steps across
/// (S_XINC_START) and down (S_Y_INC), and their second-order steps.
#[derive(Clone, Copy, Debug, Default)]
struct Coord {
    v: i64,
    dx: i64,
    dy: i64,
    dxx: i64,
    dyy: i64,
    dxy: i64,
}

impl Coord {
    fn load(card: &Mach64, start: usize, xinc: usize, yinc: usize, inc2: [usize; 3]) -> Coord {
        let s = |reg: usize, bits: u32| card.signed(reg, bits) as i64;
        Coord {
            v: s(start, 26),
            dx: s(xinc, 28),
            dy: s(yinc, 28),
            dxx: s(inc2[0], 27),
            dyy: s(inc2[1], 27),
            dxy: s(inc2[2], 27),
        }
    }

    /// The edge moved a pixel across: X_INC2 changes the step across, the
    /// cross term the step down.
    fn step_x(&mut self) {
        self.v += self.dx;
        self.dx += self.dxx;
        self.dy += self.dxy;
    }

    fn step_y(&mut self) {
        self.v += self.dy;
        self.dy += self.dyy;
        self.dx += self.dxy;
    }

    /// The value `n` steps across from the edge, and the step from there
    /// to the next pixel in direction `dir`.
    fn across(&self, n: i64, dir: i32) -> (i64, i64) {
        let value = self.v + n * self.dx + n * (n - 1) / 2 * self.dxx;
        let step = if dir > 0 {
            self.dx + n * self.dxx
        } else {
            -self.dx - (n - 1) * self.dxx
        };
        (value, step)
    }
}

/// A linear interpolator: its value on the edge and its steps.
#[derive(Clone, Copy, Debug, Default)]
struct Linear {
    v: i64,
    dx: i64,
    dy: i64,
}

impl Linear {
    /// From its START register, with X_INC and Y_INC the two before it.
    fn load(card: &Mach64, start: usize, bits: (u32, u32)) -> Linear {
        let s = |reg: usize, bits: u32| card.signed(reg, bits) as i64;
        Linear {
            v: s(start, bits.0),
            dx: s(start - 2, bits.1),
            dy: s(start - 1, bits.1),
        }
    }
}

/// The pipe's state for a trapezoid: what it reads of the registers when
/// it starts, the interpolators on the leading edge, and along the span.
pub struct Pipe {
    cntl: u32,
    pix_width: u32,
    tex_size: u32,
    tex_offsets: [u32; 11],
    z_cntl: u32,
    z_off_pitch: u32,
    compare: (u32, u32, u32),
    fog: u32,
    /// Red, green, blue, alpha; Z.
    colour: [Linear; 4],
    z: Linear,
    s: Coord,
    t: Coord,
    /// Along the span: the colours and Z and their steps, S and T and
    /// their steps, the steps down at the pixel (for the level of
    /// detail), the direction.
    span_colour: [(i64, i64); 4],
    span_z: (i64, i64),
    span_s: (i64, i64),
    span_t: (i64, i64),
    span_dy: (i64, i64),
    dir: i32,
}

const COLOURS: [usize; 4] = [RED_START, GREEN_START, BLUE_START, ALPHA_START];

impl Pipe {
    pub fn new(card: &Mach64) -> Pipe {
        let mut tex_offsets = [0; 11];
        for (i, offset) in tex_offsets.iter_mut().enumerate() {
            *offset = card.reg(TEX_0_OFF + i) & 0x7F_FFFF;
        }
        Pipe {
            cntl: card.reg(SCALE_3D_CNTL),
            pix_width: card.reg(DP_PIX_WIDTH),
            tex_size: card.reg(TEX_SIZE_PITCH),
            tex_offsets,
            z_cntl: card.reg(Z_CNTL),
            z_off_pitch: card.reg(Z_OFF_PITCH),
            compare: (
                card.reg(CLR_CMP_CLR),
                card.reg(CLR_CMP_MSK),
                card.reg(CLR_CMP_CNTL),
            ),
            fog: card.reg(DP_FRGD_CLR),
            // Colour starts read 26 bits: values past 1.0 (overbright
            // light) saturate rather than wrap.
            colour: COLOURS.map(|reg| Linear::load(card, reg, (26, 25))),
            z: Linear::load(card, Z_START, (29, 29)),
            s: Coord::load(
                card,
                S_START,
                S_XINC_START,
                S_Y_INC,
                [S_X_INC2, S_Y_INC2, S_XY_INC2],
            ),
            t: Coord::load(
                card,
                T_START,
                T_XINC_START,
                T_Y_INC,
                [T_X_INC2, T_Y_INC2, T_XY_INC2],
            ),
            span_colour: [(0, 0); 4],
            span_z: (0, 0),
            span_s: (0, 0),
            span_t: (0, 0),
            span_dy: (0, 0),
            dir: 1,
        }
    }

    /// The leading edge moved a pixel across.
    pub fn step_x(&mut self) {
        for c in &mut self.colour {
            c.v += c.dx;
        }
        self.z.v += self.z.dx;
        self.s.step_x();
        self.t.step_x();
    }

    /// The leading edge moved a scanline down.
    pub fn step_y(&mut self) {
        for c in &mut self.colour {
            c.v += c.dy;
        }
        self.z.v += self.z.dy;
        self.s.step_y();
        self.t.step_y();
    }

    /// A span starts `n` pixels from the leading edge in its direction
    /// `dir`, and goes left to right on the screen.
    pub fn span(&mut self, n: i32, dir: i32) {
        let n = n as i64;
        self.dir = dir;
        for (span, c) in self.span_colour.iter_mut().zip(self.colour) {
            *span = (c.v + c.dx * n, c.dx * dir as i64);
        }
        self.span_z = (self.z.v + self.z.dx * n, self.z.dx * dir as i64);
        self.span_s = self.s.across(n, dir);
        self.span_t = self.t.across(n, dir);
        self.span_dy = (self.s.dy + n * self.s.dxy, self.t.dy + n * self.t.dxy);
    }

    /// On to the span's next pixel.
    pub fn span_step(&mut self) {
        for span in &mut self.span_colour {
            span.0 += span.1;
        }
        self.span_z.0 += self.span_z.1;
        self.span_s.0 += self.span_s.1;
        self.span_s.1 += self.s.dxx;
        self.span_t.0 += self.span_t.1;
        self.span_t.1 += self.t.dxx;
        self.span_dy.0 += self.s.dxy * self.dir as i64;
        self.span_dy.1 += self.t.dxy * self.dir as i64;
    }

    /// Back into the registers, where the next trapezoid of the triangle
    /// goes on from.
    pub fn write_back(&self, card: &mut Mach64) {
        for (reg, c) in COLOURS.into_iter().zip(self.colour) {
            card.set_reg(reg, c.v as u32 & 0x03FF_FFFF);
        }
        card.set_reg(Z_START, self.z.v as u32 & 0x1FFF_FFFF);
        card.set_reg(S_START, self.s.v as u32 & 0x03FF_FFFF);
        card.set_reg(S_XINC_START, self.s.dx as u32 & 0x0FFF_FFFF);
        card.set_reg(S_Y_INC, self.s.dy as u32 & 0x0FFF_FFFF);
        card.set_reg(T_START, self.t.v as u32 & 0x03FF_FFFF);
        card.set_reg(T_XINC_START, self.t.dx as u32 & 0x0FFF_FFFF);
        card.set_reg(T_Y_INC, self.t.dy as u32 & 0x0FFF_FFFF);
    }

    /// The interpolated colour at the pixel, 8 bits a channel.
    fn interpolated(&self) -> Argb {
        let c = |i: usize| (self.span_colour[i].0 >> 16).clamp(0, 255) as i32;
        Argb {
            r: c(0),
            g: c(1),
            b: c(2),
            a: c(3),
        }
    }

    /// The texture sampled at the pixel: the level of detail from the
    /// largest step of S or T a pixel makes, in texels of the largest
    /// map; the largest map, filtered with BILINEAR_TEX_EN, when it
    /// magnifies, else by TEX_BLEND_FCN: the nearest map, a blend of the
    /// two nearest, filtered in the nearest, or filtered in the next
    /// larger.
    fn sample(&self, texels: &TexelSource) -> Argb {
        let largest = self.tex_size >> 4 & 0xF;
        let mut lod = 0.0;
        if self.cntl & 1 << 24 == 0 && largest > 0 {
            let scale = (1u64 << largest) as f64 / 1024.0 / 65536.0;
            let steps = [self.span_s.1, self.span_t.1, self.span_dy.0, self.span_dy.1];
            let rho = steps
                .iter()
                .map(|&d| (d as f64).abs() * scale)
                .fold(0.0, f64::max);
            if rho > 1.0 {
                lod = rho.log2();
            }
        }
        let (s, t) = (self.span_s.0, self.span_t.0);
        if lod <= 0.0 {
            return self.sample_level(texels, largest, s, t, self.cntl & 1 << 25 != 0);
        }
        let whole = (lod as u32).min(largest);
        let level = largest - whole;
        let larger = level.saturating_sub(1);
        match self.cntl >> 26 & 3 {
            1 => {
                let near = self.sample_level(texels, level, s, t, false);
                let next = self.sample_level(texels, larger, s, t, false);
                near.lerp(next, ((lod - whole as f64) * 256.0) as i32)
            }
            2 => self.sample_level(texels, level, s, t, true),
            3 => self.sample_level(texels, larger, s, t, true),
            _ => self.sample_level(texels, level, s, t, false),
        }
    }

    /// Map `level` (log2 of its size) sampled at `s`, `t`. A map's width
    /// and height shrink with its level, as does its pitch; S and T both
    /// cover the largest map's width (TEX_SIZE) in 1024.0. A map
    /// narrower than that (its pitch below its size) has nothing past
    /// its width: the sample is transparent black there, and the filter's
    /// taps stop at its edges.
    fn sample_level(&self, texels: &TexelSource, level: u32, s: i64, t: i64, filter: bool) -> Argb {
        let largest = self.tex_size >> 4 & 0xF;
        if largest > 10 {
            return Argb::BLACK;
        }
        let level = level.min(largest);
        let shrink = largest - level;
        let width_log = level;
        let height_log = (self.tex_size >> 8 & 0xF)
            .saturating_sub(shrink)
            .min(width_log);
        let pitch_log = (self.tex_size & 0xF).saturating_sub(shrink);
        let image = Image {
            base: self.tex_offsets[level as usize],
            pitch: 1 << pitch_log,
            format: self.pix_width >> 28,
        };
        // Texels with 8 bits of fraction.
        let shift = 16 + 10 - width_log as i32 - 8;
        let to_texels = |c: i64| if shift >= 0 { c >> shift } else { c << -shift };
        let (u, v) = (to_texels(s), to_texels(t));
        let (wrap_w, wrap_h) = ((1u32 << width_log) - 1, (1u32 << height_log) - 1);
        let narrow = pitch_log < width_log;
        let width = 1u32 << pitch_log.min(width_log);
        let at = |c: i64, wrap: u32| (c >> 8) as u32 & wrap;
        if narrow && at(u, wrap_w) >= width {
            return Argb::NOTHING;
        }
        if !filter {
            return texels.texel(&image, at(u, wrap_w), at(v, wrap_h));
        }
        let (cx, cy) = (at(u, wrap_w), at(v, wrap_h));
        let (u, v) = (u - 128, v - 128);
        let (mut x0, mut y0) = (at(u, wrap_w), at(v, wrap_h));
        let (mut fu, mut fv) = ((u & 0xFF) as i32, (v & 0xFF) as i32);
        let (x1, y1) = if narrow {
            // A tap that wrapped to the far side stays at the edge.
            if x0 > cx {
                (x0, fu) = (cx, 0);
            }
            if y0 > cy {
                (y0, fv) = (cy, 0);
            }
            ((x0 + 1).min(width - 1), (y0 + 1).min(wrap_h))
        } else {
            ((x0 + 1) & wrap_w, (y0 + 1) & wrap_h)
        };
        let texel = |x, y| texels.texel(&image, x, y);
        let top = texel(x0, y0).lerp(texel(x1, y0), fu);
        let bottom = texel(x0, y1).lerp(texel(x1, y1), fu);
        top.lerp(bottom, fv)
    }

    /// Z at the pixel against the Z buffer (Z_CNTL): 16 bits, the integer
    /// part of the interpolator. Returns whether it passes and where the
    /// pixel's Z is, with its new value.
    fn z_test(&self, mem: &Memory, x: i32, y: i32) -> (bool, u32, u32) {
        let z = (self.span_z.0 >> 12).clamp(0, 0xFFFF) as u32;
        let pitch = ((self.z_off_pitch >> 22 & 0x3FF) * 8) as i32;
        let at = ((self.z_off_pitch & 0xF_FFFF) << 2).wrapping_add((y * pitch + x) as u32);
        if self.z_cntl & 1 == 0 {
            return (true, at, z);
        }
        let old = mem.read(at, Size::Word);
        let pass = match self.z_cntl >> 4 & 7 {
            0 => false,
            1 => z < old,
            2 => z <= old,
            3 => z == old,
            4 => z >= old,
            5 => z > old,
            6 => z != old,
            _ => true,
        };
        (pass, at, z)
    }

    /// The pixel at `x`, `y` over `dst`, or None where the pipe leaves it:
    /// failing the Z test, an alpha mask's 0, a keyed texel.
    pub fn pixel(
        &self,
        mem: &mut Memory,
        palettes: &Palettes,
        x: i32,
        y: i32,
        dst: u32,
    ) -> Option<u32> {
        let (pass, z_at, z) = self.z_test(mem, x, y);
        if !pass {
            return None;
        }
        let cntl = self.cntl;
        let lit = self.interpolated();
        let mut c = lit;
        let mut alpha = lit.a;
        if cntl >> 6 & 3 == 2 {
            let texels = TexelSource {
                mem,
                palettes,
                ci4: self.pix_width,
                signed_uv: cntl & 1 << 10 != 0,
            };
            let mut t = self.sample(&texels);
            let alpha_in_texture = cntl & 1 << 30 != 0;
            if !alpha_in_texture {
                t.a = 255;
            } else if cntl & 1 << 28 != 0 {
                // TEX_AMASK_AEN: the texel's alpha is one bit. 3D DDI mode
                // drops the pixel for a 0; blended edge mode makes a 1 opaque.
                let bit = t.a & 0x80 != 0 || t.a & 1 != 0;
                if cntl & 1 << 29 == 0 {
                    if !bit {
                        return None;
                    }
                    t.a = lit.a;
                } else {
                    t.a = if bit { 255 } else { lit.a };
                }
            }
            // The colour key against the texel, as RGB 8:8:8.
            let (key, mask, compare) = self.compare;
            if compare >> 24 & 3 == 2 {
                let texel = ((t.r << 16) | (t.g << 8) | t.b) as u32 & mask;
                let equal = texel == key & mask;
                match compare & 7 {
                    1 => return None,
                    4 if !equal => return None,
                    5 if equal => return None,
                    _ => {}
                }
            }
            let tc = if cntl & 1 << 13 != 0 {
                Argb { a: t.a, ..lit }
            } else {
                t
            };
            let light = cntl >> 22 & 3;
            c = match light {
                // Modulate.
                1 => Argb {
                    a: t.a,
                    r: (tc.r * (lit.r + 1)) >> 8,
                    g: (tc.g * (lit.g + 1)) >> 8,
                    b: (tc.b * (lit.b + 1)) >> 8,
                },
                // Alpha decal.
                2 => {
                    let ta = t.a + (t.a >> 7);
                    let mix = |tex: i32, l: i32| (tex * ta + l * (256 - ta)) >> 8;
                    Argb {
                        a: t.a,
                        r: mix(tc.r, lit.r),
                        g: mix(tc.g, lit.g),
                        b: mix(tc.b, lit.b),
                    }
                }
                _ => tc,
            };
            alpha = if light == 2 || !alpha_in_texture {
                lit.a
            } else {
                t.a
            };
        }
        c.a = alpha;
        match cntl >> 11 & 3 {
            // Fog: the fog colour in place of the destination, the alpha
            // interpolator the fog factor.
            2 => {
                let f = lit.a + (lit.a >> 7);
                let fog = |shift: u32, v: i32| {
                    (v * f + (self.fog >> shift & 0xFF) as i32 * (256 - f)) >> 8
                };
                c = Argb {
                    a: c.a,
                    r: fog(16, c.r),
                    g: fog(8, c.g),
                    b: fog(0, c.b),
                };
            }
            1 => c = blend(c, unpack(dst, self.pix_width & 0xF), alpha, cntl),
            _ => {}
        }
        if self.z_cntl & 0x101 == 0x101 {
            mem.write(z_at, Size::Word, z);
        }
        Some(pack(c, self.pix_width & 0xF, x, y, cntl))
    }
}

/// The scaler (SCALE_3D_FCN 1): an image stretched over a rectangle, from
/// SCALE_Y_OFF (TEX_0_OFF), SCALE_WIDTH and SCALE_HEIGHT, SCALE_Y_PITCH,
/// accumulators that start at SCALE_HACC (RED_START) and SCALE_VACC and
/// step by SCALE_X_INC and SCALE_Y_INC, 4.12 fixed point.
pub struct Scaler {
    cntl: u32,
    dst_format: u32,
    image: Image,
    width: i32,
    height: i32,
    h0: i64,
    v0: i64,
    x_step: i64,
    y_step: i64,
}

impl Scaler {
    pub fn new(card: &Mach64) -> Option<Scaler> {
        let accumulator = |reg: usize| (((card.reg(reg) & 0x1F_FFF0) << 11) as i32 >> 11) as i64;
        let scaler = Scaler {
            cntl: card.reg(SCALE_3D_CNTL),
            dst_format: card.reg(DP_PIX_WIDTH) & 0xF,
            image: Image {
                base: card.reg(TEX_0_OFF) & 0x7F_FFF8,
                pitch: card.reg(S_Y_INC) & 0xFFF,
                format: card.reg(DP_PIX_WIDTH) >> 28,
            },
            width: (card.reg(SCALE_WIDTH) & 0xFFF) as i32,
            height: (card.reg(SCALE_HEIGHT) & 0xFFF) as i32,
            h0: accumulator(RED_START),
            v0: accumulator(SCALE_VACC),
            x_step: (card.reg(RED_X_INC) & 0xF_FFF0) as i64,
            y_step: (card.reg(GREEN_X_INC) & 0xF_FFF0) as i64,
        };
        (scaler.width > 0 && scaler.height > 0 && scaler.image.pitch > 0).then_some(scaler)
    }

    /// The pixel `col`, `row` of the rectangle over `dst`: point sampled
    /// with SCALE_PIX_REP, else blended between neighbours with 5-bit
    /// weights; blended with the destination with ALPHA_FOG_EN 1.
    pub fn pixel(
        &self,
        mem: &Memory,
        palettes: &Palettes,
        col: i32,
        row: i32,
        dst: u32,
    ) -> Option<u32> {
        let h = (self.h0 + col.unsigned_abs() as i64 * self.x_step).max(0);
        let v = (self.v0 + row.unsigned_abs() as i64 * self.y_step).max(0);
        let x0 = ((h >> 16) as i32).min(self.width - 1);
        let y0 = ((v >> 16) as i32).min(self.height - 1);
        let texels = TexelSource {
            mem,
            palettes,
            ci4: 0,
            signed_uv: self.cntl & 1 << 10 != 0,
        };
        let texel = |x: i32, y: i32| texels.texel(&self.image, x as u32, y as u32);
        let mut c = if self.cntl & 1 << 8 != 0 {
            texel(x0, y0)
        } else {
            let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
            let (fx, fy) = (((h >> 11) & 0x1F) as i32 * 8, ((v >> 11) & 0x1F) as i32 * 8);
            texel(x0, y0)
                .lerp(texel(x1, y0), fx)
                .lerp(texel(x0, y1).lerp(texel(x1, y1), fx), fy)
        };
        if self.cntl >> 11 & 3 == 1 {
            c = blend(c, unpack(dst, self.dst_format), c.a, self.cntl);
        }
        Some(pack(c, self.dst_format, col, row, self.cntl))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing_cuts_or_rounds() {
        let c = Argb {
            a: 255,
            r: 0xFF,
            g: 0x84,
            b: 0x07,
        };
        assert_eq!(pack(c, 4, 0, 0, 0), 0x1F << 11 | 0x21 << 5);
        assert_eq!(pack(c, 4, 0, 0, 0x10), 0x1F << 11 | 0x21 << 5 | 1);
        assert_eq!(
            unpack(0xF800, 4),
            Argb {
                a: 255,
                r: 255,
                g: 0,
                b: 0
            }
        );
    }

    #[test]
    fn blending_with_source_alpha() {
        // Source alpha and one less it: halfway.
        let cntl = 4 << 16 | 5 << 19;
        let out = blend(
            Argb {
                a: 128,
                r: 255,
                g: 0,
                b: 0,
            },
            Argb {
                a: 255,
                r: 0,
                g: 0,
                b: 255,
            },
            128,
            cntl,
        );
        assert_eq!((out.r, out.b), (128, 126));
    }

    #[test]
    fn quadratic_coordinates_step_as_they_add_up() {
        let c = Coord {
            v: 100,
            dx: 10,
            dxx: 2,
            ..Default::default()
        };
        let (at3, step) = c.across(3, 1);
        let mut edge = c;
        for _ in 0..3 {
            edge.step_x();
        }
        assert_eq!(at3, edge.v);
        assert_eq!(at3 + step, {
            edge.step_x();
            edge.v
        });
    }
}

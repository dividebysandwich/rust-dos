//! The TSP: a visible surface's colour at a pixel from its record in the
//! texture memory: flat or smooth shading, perspective-correct texturing
//! from twiddled, mip-mapped maps with bilinear filtering, a highlight,
//! the shadow light, fog by depth, and translucency over what the tile
//! holds.
//!
//! This follows Imagination's simulator of the PCX2's TSP (`simulat3/
//! texas.c`: `Texas`, `TexturePixel`, `AddressCalc`, `ColourConvert`),
//! its fixed-point arithmetic included.
//!
//! A record is at the TSP tag times two dwords from PREC_BASE. Its first
//! dword is the control word: texturing (bit 31), smooth shading (30), no
//! fog (29), the shadow light (28), a flat highlight (26), the texture's
//! exponent (21-18), global translucency (16-13), U and V reflection (12,
//! 11), translucent (10); flat shading's red in bits 7-0. The second has
//! flat green and blue and the shadow light's 555 colour, or smooth
//! shading's origin. A textured record has six more: the texture's
//! coefficients and map, then smooth shading's and the highlight's words.

use super::regs;

/// Mip-mapped maps alternate between the halves of the memory.
const BIG_BANK: u32 = 0x10_0000;

/// Where a mip-mapped map's levels start, by level (256x256 down).
const MIP_OFFSET: [u32; 9] = [0x5555, 0x1555, 0x0555, 0x0155, 0x0055, 0x0015, 0x0005, 0x0001, 0x0000];
/// Where U and V wrap, by map size.
const MAP_MASK: [i32; 4] = [0xFF, 0x7F, 0x3F, 0x1F];

/// Texture filtering, as the `powervr_filter` setting asks for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Filter {
    /// As the program sets the card.
    #[default]
    Auto,
    Point,
    Bilinear,
}

/// How the host shades.
#[derive(Clone, Copy, Debug, Default)]
pub struct Shader {
    pub filter: Filter,
}

#[derive(Clone, Copy, Default)]
struct Rgba {
    r: i32,
    g: i32,
    b: i32,
    a: i32,
}

/// A float with a 16-bit mantissa, the TSP's (`pfloat`).
#[derive(Clone, Copy)]
struct PFloat {
    m: i32,
    e: i32,
}

/// `ToPfloat`.
fn to_pfloat(x: i32) -> PFloat {
    let magnitude = (x as i64).unsigned_abs();
    let mut e = 0;
    while e < 32 && (1u64 << e) <= magnitude {
        e += 1;
    }
    let m = if e > 31 { x >> 16 } else { ((x as i64) << (31 - e)) as i32 >> 16 };
    PFloat { m, e }
}

/// `AShift`: a shift left, or right for a negative count.
fn ashift(x: i32, shift: i32) -> i32 {
    if shift < 0 {
        if shift < -31 { if x < 0 { -1 } else { 0 } } else { x >> -shift }
    } else if shift > 31 {
        0
    } else {
        x.wrapping_shl(shift as u32)
    }
}

/// The low 16 bits, signed.
fn int16(x: u32) -> i32 {
    x as u16 as i16 as i32
}

/// A 555 colour to 8 bits a channel, as the TSP widens them.
fn from_555(raw: u32) -> (i32, i32, i32) {
    let c = |shift: u32| ((raw >> shift & 0x1F) as i32) << 3 | 4;
    (c(10), c(5), c(0))
}

/// The interleaved bits of a texel's address within its map.
fn twiddle(x: i32, y: i32) -> u32 {
    fn spread(v: i32) -> u32 {
        let mut v = v as u32 & 0x3FF;
        v = (v | v << 8) & 0x00FF_00FF;
        v = (v | v << 4) & 0x0F0F_0F0F;
        v = (v | v << 2) & 0x3333_3333;
        (v | v << 1) & 0x5555_5555
    }
    spread(x) << 1 | spread(y)
}

/// A texture's map.
#[derive(Clone, Copy)]
struct Map {
    address: u32,
    /// 0 for 256x256 up to 3 for 32x32.
    size: usize,
    sixteen_bit: bool,
    mip_mapped: bool,
    /// 4444 rather than 555.
    alpha: bool,
    flip_uv: u32,
}

pub struct Tsp<'a> {
    textures: &'a [u8],
    prec_base: u32,
    cfr_scale: i32,
    fog_shift: u32,
    fog_colour: (i32, i32, i32),
    fog_table: [i32; 128],
    bilinear: bool,
}

impl<'a> Tsp<'a> {
    pub fn new(registers: &[u32], textures: &'a [u8], shader: &Shader) -> Self {
        let fogcol = registers[regs::FOGCOL];
        let mut fog_table = [0; 128];
        for (i, entry) in fog_table.iter_mut().enumerate() {
            *entry = (registers[regs::FOG_TABLE + i] & 0xFF) as i32;
        }
        let bilinear = match shader.filter {
            Filter::Auto => registers[regs::BILINEAR] & 3 != 3,
            Filter::Point => false,
            Filter::Bilinear => true,
        };
        Self {
            textures,
            prec_base: registers[regs::PREC_BASE],
            cfr_scale: (registers[regs::CAMERA] & 0xFFFF) as i32,
            fog_shift: registers[regs::FOGAMOUNT] & 31,
            fog_colour: ((fogcol >> 16 & 0xFF) as i32, (fogcol >> 8 & 0xFF) as i32, (fogcol & 0xFF) as i32),
            fog_table,
            bilinear,
        }
    }

    /// A dword of the texture memory.
    fn param(&self, dword: u32) -> u32 {
        let at = (dword as usize * 4) % self.textures.len();
        u32::from_le_bytes([self.textures[at], self.textures[at + 1], self.textures[at + 2], self.textures[at + 3]])
    }

    /// A 16-bit texel: pixel addresses count 16-bit pixels, two to a dword,
    /// the even one in the high half.
    fn texel(&self, address: u32) -> u32 {
        let dword = self.param(address >> 1);
        if address & 1 != 0 { dword & 0xFFFF } else { dword >> 16 }
    }

    /// `AddressCalc`: the texel at u, v of map level `level`.
    fn address(&self, map: &Map, u: i32, v: i32, level: usize) -> u32 {
        let (mut u, mut v) = (u, v);
        let flip_bit = if map.size == 0 { 128 } else { MAP_MASK[map.size] + 1 };
        if map.flip_uv != 0 {
            if u & flip_bit != 0 && map.flip_uv & 2 != 0 {
                u = !u;
            }
            if v & flip_bit != 0 && map.flip_uv & 1 != 0 {
                v = !v;
            }
        }
        u &= MAP_MASK[map.size];
        v &= MAP_MASK[map.size];
        if !map.mip_mapped {
            if map.sixteen_bit { map.address.wrapping_add(twiddle(u, v)) } else { map.address.wrapping_add(twiddle(u >> 1, v)) }
        } else {
            let mut address = map.address;
            if (map.size + level) & 1 != 0 {
                address ^= BIG_BANK;
            }
            address.wrapping_add(MIP_OFFSET[(map.size + level).min(8)]).wrapping_add(twiddle(u >> level, v >> level))
        }
    }

    /// `ColourConvert`: a texel's 5-bit channels and 4-bit alpha.
    fn convert(&self, raw: u32, map: &Map, which: i32) -> Rgba {
        if map.alpha {
            let widen = |c: u32| {
                let c = (c << 1) as i32;
                c | c >> 4
            };
            Rgba { r: widen(raw >> 8 & 15), g: widen(raw >> 4 & 15), b: widen(raw & 15), a: (raw >> 12 & 15) as i32 }
        } else if map.sixteen_bit {
            Rgba { r: (raw >> 10 & 0x1F) as i32, g: (raw >> 5 & 0x1F) as i32, b: (raw & 0x1F) as i32, a: 0 }
        } else {
            let raw = if which == 0 { raw >> 8 } else { raw };
            let (mut r, mut g, mut b) = ((raw & 0xE0) as i32, (raw << 3 & 0xE0) as i32, (raw << 6 & 0xC0) as i32);
            if b & 0x80 != 0 {
                b |= 0x3F;
            }
            if g & 0x80 != 0 {
                g |= 0x1F;
            }
            if r & 0x80 != 0 {
                r |= 0x1F;
            }
            Rgba { r: r >> 3, g: g >> 3, b: b >> 3, a: 0 }
        }
    }

    fn fetch(&self, map: &Map, u: i32, v: i32, level: usize, which: i32) -> Rgba {
        self.convert(self.texel(self.address(map, u, v, level)), map, which)
    }

    /// Four texels around u, v blended by the fractions (5 bits each), to
    /// 8 bits a channel.
    #[allow(clippy::too_many_arguments)]
    fn bilinear(&self, map: &Map, u: i32, v: i32, level: usize, step: i32, u_frac: i32, v_frac: i32, which: (i32, i32)) -> Rgba {
        let c = self.fetch(map, u, v, level, which.0);
        let u1 = self.fetch(map, (u + step) & 255, v, level, which.1);
        let v1 = self.fetch(map, u, (v + step) & 255, level, which.0);
        let u1v1 = self.fetch(map, (u + step) & 255, (v + step) & 255, level, which.1);
        let lerp = |a: i32, b: i32| (a << 3) + 4 + ((b - a) * u_frac >> 2);
        let lerp_a = |a: i32, b: i32| a + ((b - a) * u_frac >> 5);
        let ab = Rgba { r: lerp(c.r, u1.r), g: lerp(c.g, u1.g), b: lerp(c.b, u1.b), a: lerp_a(c.a, u1.a) };
        let cd = Rgba { r: lerp(v1.r, u1v1.r), g: lerp(v1.g, u1v1.g), b: lerp(v1.b, u1v1.b), a: lerp_a(v1.a, u1v1.a) };
        let mix = |a: i32, b: i32| ((b - a) * v_frac >> 5) + a;
        Rgba { r: mix(ab.r, cd.r), g: mix(ab.g, cd.g), b: mix(ab.b, cd.b), a: mix(ab.a, cd.a) }
    }

    /// `TexturePixel`: the texture's colour at x, y, 8 bits a channel and
    /// 4-bit alpha (15 clear).
    #[allow(clippy::too_many_arguments)]
    fn texture(&self, x: i32, y: i32, coeff: [i32; 9], exp: i32, pmip: PFloat, map: &Map, global_trans: i32) -> Rgba {
        let [a, b, c, d, e, f, p, q, r] = coeff;
        let abc = a.wrapping_mul(x).wrapping_add(b.wrapping_mul(y)).wrapping_add(c.wrapping_mul(self.cfr_scale));
        let def = d.wrapping_mul(x).wrapping_add(e.wrapping_mul(y)).wrapping_add(f.wrapping_mul(self.cfr_scale));
        let pqr = p.wrapping_mul(x).wrapping_add(q.wrapping_mul(y)).wrapping_add(r.wrapping_mul(self.cfr_scale));
        let mut bot = to_pfloat(pqr);
        bot.m >>= 1;
        bot.m = if bot.m > 0 { 0x800_0000 / bot.m } else { 0x4000 };
        let power_two = bot.m == 0x4000;
        if power_two {
            bot.m = 0x2000;
        }
        let quotient = |top: i32| -> (i32, i32) {
            let mut top = to_pfloat(top);
            top.e += exp;
            let value = ((top.m as i64 * bot.m as i64) >> 14) as i32;
            let shift = top.e - (bot.e + if power_two { 13 } else { 14 });
            (ashift(value, shift) & 255, ashift(value, shift + 5) & 8191)
        };
        let (u, mut u_frac) = quotient(abc);
        let (v, mut v_frac) = quotient(def);
        // The level of detail: pmip over the square of the bottom.
        bot.m >>= 6;
        bot.m *= bot.m;
        if power_two {
            bot.e -= 1;
        }
        if bot.m & 0x8000 != 0 {
            bot.e *= 2;
            bot.m >>= 8;
        } else {
            bot.e = bot.e * 2 + 1;
            bot.m >>= 7;
        }
        bot.m *= pmip.m;
        if bot.m & 0x8000 != 0 {
            bot.e = pmip.e - (bot.e - 2);
        } else {
            bot.e = pmip.e - (bot.e - 1);
        }
        let level = bot.e.clamp(0, 15);
        let mut colour = if level < 1 || !map.mip_mapped || !map.sixteen_bit {
            if self.bilinear {
                u_frac &= 31;
                v_frac &= 31;
                self.bilinear(map, u, v, 0, 1, u_frac, v_frac, (u & 1, (u + 1) & 1))
            } else {
                let t = self.fetch(map, u, v, 0, u & 1);
                Rgba { r: t.r << 3 | 4, g: t.g << 3 | 4, b: t.b << 3 | 4, a: t.a }
            }
        } else if map.size as i32 + level > 8 {
            // The 1x1 map.
            let t = self.convert(self.texel(self.address(map, u, v, 8 - map.size)), map, 0);
            Rgba { r: t.r << 3 | 4, g: t.g << 3 | 4, b: t.b << 3 | 4, a: t.a }
        } else {
            let compress = (level - 1) as usize;
            if self.bilinear {
                u_frac = (u_frac >> compress) & 31;
                v_frac = (v_frac >> compress) & 31;
                self.bilinear(map, u, v, compress, 1 << compress, u_frac, v_frac, (u & 1, u & 1))
            } else {
                let t = self.fetch(map, u, v, compress, u & 1);
                Rgba { r: t.r << 3 | 4, g: t.g << 3 | 4, b: t.b << 3 | 4, a: t.a }
            }
        };
        colour.a = (colour.a + global_trans).min(15);
        colour
    }

    /// The fog's share at a depth, out of 256.
    fn fog(&self, depth: f32) -> i32 {
        let fixed = (depth as f64 * 2_147_483_648.0).clamp(0.0, i32::MAX as f64) as i32;
        let index = fixed >> self.fog_shift;
        let fall = (index >> 7).min(9);
        self.fog_table[(index & 0x7F) as usize] >> fall
    }

    /// `Texas`: the colour of surface `tag` at x, y over `under` (0xRRGGBB).
    pub fn shade(&self, x: i32, y: i32, tag: u32, depth: f32, shadow: bool, under: u32) -> u32 {
        let base = self.prec_base.wrapping_add(tag << 1);
        let p = |i: u32| self.param(base.wrapping_add(i));
        let control = p(0);
        let textured = control & 0x8000_0000 != 0;
        let smooth = control & 0x4000_0000 != 0;
        let lit = control & 0x1000_0000 != 0 && !shadow;
        let mut next = base;
        let (mut x_offset, mut y_offset) = (0, 0);
        let mut flat = (0, 0, 0);
        if smooth {
            x_offset = int16(p(1) >> 16);
            y_offset = int16(p(1));
        } else {
            flat = ((control & 0xFF) as i32, (p(1) >> 24 & 0xFF) as i32, (p(1) >> 16 & 0xFF) as i32);
            if lit {
                let light = from_555(p(1) & 0xFFFF);
                flat = ((flat.0 + light.0).min(255), (flat.1 + light.1).min(255), (flat.2 + light.2).min(255));
            }
        }
        let mut colour = if textured {
            let coeff = [
                int16(p(5)),
                int16(p(5) >> 16),
                int16(p(4)),
                int16(p(7)),
                int16(p(7) >> 16),
                int16(p(6)),
                int16(p(3)),
                int16(p(3) >> 16),
                int16(p(2)),
            ];
            let map = Map {
                address: p(4) >> 16 | (p(6) & 0x00FF_0000),
                size: 3 - (p(6) >> 28 & 3) as usize,
                sixteen_bit: p(6) & 0x4000_0000 != 0,
                mip_mapped: p(6) & 0x8000_0000 != 0,
                alpha: p(6) & 0x0800_0000 != 0,
                flip_uv: control >> 11 & 3,
            };
            let pmip = PFloat { m: (p(2) >> 24) as i32, e: (p(2) >> 18 & 0x3F) as i32 };
            let exp = (control >> 18 & 15) as i32;
            let global_trans = (control >> 13 & 15) as i32;
            let mut t = self.texture(x, y, coeff, exp, pmip, &map, global_trans);
            if !smooth {
                t.r = t.r * flat.0 >> 8;
                t.g = t.g * flat.1 >> 8;
                t.b = t.b * flat.2 >> 8;
            }
            next = next.wrapping_add(8);
            t
        } else {
            next = next.wrapping_add(2);
            Rgba { r: flat.0, g: flat.1, b: flat.2, a: 0 }
        };
        if smooth {
            let shade = |at: u32| -> (i32, i32, i32) {
                let (w0, w1) = (self.param(at), self.param(at.wrapping_add(1)));
                let (t0, t1, t2) = (int16(w0), int16(w1 >> 16), int16(w1));
                let fraction = ((t0 << 2) + t1 * (y - y_offset) + t2 * (x - x_offset)).clamp(0, 0x10000) >> 8;
                let (r, g, b) = from_555(w0 >> 16);
                let scale = |c: i32| (c >> 3) * fraction >> 5;
                let bump = if fraction == 0x100 { 4 } else { 0 };
                (scale(r) + bump, scale(g) + bump, scale(b) + bump)
            };
            let mut hold = shade(next);
            next = next.wrapping_add(2);
            if lit {
                let light = shade(next);
                hold = ((hold.0 + light.0).min(255), (hold.1 + light.1).min(255), (hold.2 + light.2).min(255));
                next = next.wrapping_add(2);
            }
            if textured {
                colour.r = colour.r * hold.0 >> 8;
                colour.g = colour.g * hold.1 >> 8;
                colour.b = colour.b * hold.2 >> 8;
            } else {
                colour = Rgba { r: hold.0, g: hold.1, b: hold.2, a: 0 };
            }
        }
        if control & 0x0400_0000 != 0 {
            let word = self.param(next);
            let five = |raw: u32| {
                let (r, g, b) = from_555(raw);
                (r >> 3, g >> 3, b >> 3)
            };
            let mut highlight = five(word >> 16);
            if lit {
                let light = five(word & 0xFFFF);
                highlight = ((highlight.0 + light.0).min(31), (highlight.1 + light.1).min(31), (highlight.2 + light.2).min(31));
            }
            colour.r = (colour.r + (highlight.0 << 3)).min(255);
            colour.g = (colour.g + (highlight.1 << 3)).min(255);
            colour.b = (colour.b + (highlight.2 << 3)).min(255);
        }
        if control & 0x2000_0000 == 0 {
            let fog = self.fog(depth);
            colour.r += (self.fog_colour.0 - colour.r) * fog >> 8;
            colour.g += (self.fog_colour.1 - colour.g) * fog >> 8;
            colour.b += (self.fog_colour.2 - colour.b) * fog >> 8;
        }
        if control & 0x0400 != 0 {
            let alpha = if colour.a == 15 { 16 } else { colour.a };
            let (ur, ug, ub) = ((under >> 16 & 0xFF) as i32, (under >> 8 & 0xFF) as i32, (under & 0xFF) as i32);
            colour.r = (ur * alpha >> 4) + ((16 - alpha) * colour.r >> 4);
            colour.g = (ug * alpha >> 4) + ((16 - alpha) * colour.g >> 4);
            colour.b = (ub * alpha >> 4) + ((16 - alpha) * colour.b >> 4);
        }
        let c = |v: i32| v.clamp(0, 255) as u32;
        c(colour.r) << 16 | c(colour.g) << 8 | c(colour.b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twiddling_interleaves_u_above_v() {
        assert_eq!(twiddle(0, 0), 0);
        assert_eq!(twiddle(0, 1), 1);
        assert_eq!(twiddle(1, 0), 2);
        assert_eq!(twiddle(3, 3), 15);
        assert_eq!(twiddle(255, 0), 0xAAAA);
    }

    #[test]
    fn pfloats_keep_15_bits_and_the_sign() {
        let p = to_pfloat(1);
        assert_eq!((p.m, p.e), (0x4000, 1));
        let p = to_pfloat(-3);
        assert_eq!(p.e, 2);
        assert!(p.m < 0);
        assert_eq!(to_pfloat(0).m, 0);
    }
}

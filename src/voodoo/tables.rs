//! The tables and arithmetic the rasterizer shares: the reciprocal and
//! log2 table behind `fast_reciplog`, the dither matrices and their lookups,
//! the texel formats' colour tables, and the conversions of the float
//! registers. From DOSBox-X's voodoo_data.h, voodoo_types.h and voodoo_emu.cpp
//! (`voodoo_init`, `init_tmu_shared`), which have them from MAME.

use std::sync::{Arc, OnceLock};

/// log2 of the reciprocal table's size, and the precisions of its input,
/// its entries and its results.
const RECIPLOG_LOOKUP_BITS: u32 = 9;
const RECIPLOG_INPUT_PREC: i32 = 32;
const RECIPLOG_LOOKUP_PREC: i32 = 22;
const RECIP_OUTPUT_PREC: i32 = 15;
const LOG_OUTPUT_PREC: i32 = 8;

pub const DITHER_MATRIX_4X4: [u8; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
pub const DITHER_MATRIX_2X2: [u8; 16] = [2, 10, 2, 10, 14, 6, 14, 6, 2, 10, 2, 10, 14, 6, 14, 6];

/// Everything built once and shared by every card.
pub struct Tables {
    /// Pairs of 1/n and log2(n) for n from 1.0 to 2.0.
    reciplog: Vec<u32>,
    /// A colour component through the 4x4 and 2x2 dither matrices: index
    /// `y << 11 | value << 3 | x << 1 | green`.
    pub dither4: Vec<u8>,
    pub dither2: Vec<u8>,
    /// The texel formats' fixed colour tables, as ARGB.
    pub rgb332: Arc<[u32]>,
    pub alpha8: Arc<[u32]>,
    pub int8: Arc<[u32]>,
    pub ai44: Arc<[u32]>,
    pub rgb565: Arc<[u32]>,
    pub argb1555: Arc<[u32]>,
    pub argb4444: Arc<[u32]>,
    /// For the formats a Voodoo Graphics hasn't (6, 7 and 15): black.
    pub none: Arc<[u32]>,
}

pub fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(Tables::build)
}

pub fn argb(a: u32, r: u32, g: u32, b: u32) -> u32 {
    (a & 0xFF) << 24 | (r & 0xFF) << 16 | (g & 0xFF) << 8 | (b & 0xFF)
}

pub fn extract_565(val: u32) -> (u32, u32, u32) {
    (
        ((val >> 8) & 0xF8) | ((val >> 13) & 0x07),
        ((val >> 3) & 0xFC) | ((val >> 9) & 0x03),
        ((val << 3) & 0xF8) | ((val >> 2) & 0x07),
    )
}

pub fn extract_x555(val: u32) -> (u32, u32, u32) {
    (
        ((val >> 7) & 0xF8) | ((val >> 12) & 0x07),
        ((val >> 2) & 0xF8) | ((val >> 7) & 0x07),
        ((val << 3) & 0xF8) | ((val >> 2) & 0x07),
    )
}

pub fn extract_555x(val: u32) -> (u32, u32, u32) {
    (
        ((val >> 8) & 0xF8) | ((val >> 13) & 0x07),
        ((val >> 3) & 0xF8) | ((val >> 8) & 0x07),
        ((val << 2) & 0xF8) | ((val >> 3) & 0x07),
    )
}

/// ARGB 1-5-5-5: the alpha bit made all ones or zeros.
pub fn extract_1555(val: u32) -> (u32, u32, u32, u32) {
    let (r, g, b) = extract_x555(val);
    (((val as i16 as i32) >> 15) as u32 & 0xFF, r, g, b)
}

/// RGBA 5-5-5-1.
pub fn extract_5551(val: u32) -> (u32, u32, u32, u32) {
    let (r, g, b) = extract_555x(val);
    (r, g, b, if val & 1 != 0 { 0xFF } else { 0 })
}

pub fn extract_4444(val: u32) -> (u32, u32, u32, u32) {
    (
        ((val >> 8) & 0xF0) | ((val >> 12) & 0x0F),
        ((val >> 4) & 0xF0) | ((val >> 8) & 0x0F),
        (val & 0xF0) | ((val >> 4) & 0x0F),
        ((val << 4) & 0xF0) | (val & 0x0F),
    )
}

fn extract_332(val: u32) -> (u32, u32, u32) {
    (
        (val & 0xE0) | ((val >> 3) & 0x1C) | ((val >> 6) & 0x03),
        ((val << 3) & 0xE0) | (val & 0x1C) | ((val >> 3) & 0x03),
        // DOSBox-X masks the third term with C0h, a typo MAME fixed.
        ((val << 6) & 0xC0) | ((val << 4) & 0x30) | ((val << 2) & 0x0C) | (val & 0x03),
    )
}

/// The dithered 5 or 6-bit value of an 8-bit component.
fn dither_rb(value: i32, dither: i32) -> i32 {
    ((value << 1) - (value >> 4) + (value >> 7) + dither) >> 1
}

fn dither_g(value: i32, dither: i32) -> i32 {
    ((value << 2) - (value >> 4) + (value >> 6) + dither) >> 2
}

impl Tables {
    fn build() -> Self {
        let mut reciplog = vec![0u32; (2 << RECIPLOG_LOOKUP_BITS) + 2];
        for val in 0..=(1u32 << RECIPLOG_LOOKUP_BITS) {
            let value = (1 << RECIPLOG_LOOKUP_BITS) + val;
            reciplog[val as usize * 2] = (1u32 << (RECIPLOG_LOOKUP_PREC as u32 + RECIPLOG_LOOKUP_BITS)) / value;
            reciplog[val as usize * 2 + 1] = ((value as f64 / (1u32 << RECIPLOG_LOOKUP_BITS) as f64).ln()
                / 2f64.ln()
                * (1u32 << RECIPLOG_LOOKUP_PREC) as f64) as u32;
        }

        let mut dither4 = vec![0u8; 256 * 16 * 2];
        let mut dither2 = vec![0u8; 256 * 16 * 2];
        for val in 0..256 * 16 * 2 {
            let g = val & 1;
            let x = (val >> 1) & 3;
            let color = ((val >> 3) & 0xFF) as i32;
            let y = (val >> 11) & 3;
            let (m4, m2) = (DITHER_MATRIX_4X4[y * 4 + x] as i32, DITHER_MATRIX_2X2[y * 4 + x] as i32);
            if g == 0 {
                dither4[val] = (dither_rb(color, m4) >> 3) as u8;
                dither2[val] = (dither_rb(color, m2) >> 3) as u8;
            } else {
                dither4[val] = (dither_g(color, m4) >> 2) as u8;
                dither2[val] = (dither_g(color, m2) >> 2) as u8;
            }
        }

        let table8 = |f: &dyn Fn(u32) -> u32| -> Arc<[u32]> { (0..256).map(f).collect() };
        let table16 = |f: &dyn Fn(u32) -> u32| -> Arc<[u32]> { (0..65536).map(f).collect() };
        Self {
            reciplog,
            dither4,
            dither2,
            rgb332: table8(&|v| {
                let (r, g, b) = extract_332(v);
                argb(0xFF, r, g, b)
            }),
            alpha8: table8(&|v| argb(v, v, v, v)),
            int8: table8(&|v| argb(0xFF, v, v, v)),
            ai44: table8(&|v| {
                let a = (v & 0xF0) | ((v >> 4) & 0x0F);
                let i = ((v << 4) & 0xF0) | (v & 0x0F);
                argb(a, i, i, i)
            }),
            rgb565: table16(&|v| {
                let (r, g, b) = extract_565(v);
                argb(0xFF, r, g, b)
            }),
            argb1555: table16(&|v| {
                let (a, r, g, b) = extract_1555(v);
                argb(a, r, g, b)
            }),
            argb4444: table16(&|v| {
                let (a, r, g, b) = extract_4444(v);
                argb(a, r, g, b)
            }),
            none: table16(&|_| 0),
        }
    }

    /// A fast 16.16 reciprocal of a 16.32 value (1/w in the rasterizer),
    /// and log2 of it in 16.8 as the second result (`fast_reciplog`).
    #[inline]
    pub fn reciplog(&self, value: i64) -> (i64, i32) {
        let (value, neg) = if value < 0 { (value.wrapping_neg(), true) } else { (value, false) };
        let mut exp: i32 = 0;
        // Push a value that spilled out of 32 bits back under them.
        let mut temp = if value as u64 & 0xFFFF_0000_0000 != 0 {
            exp -= 16;
            (value >> 16) as u32
        } else {
            value as u32
        };
        if temp == 0 {
            return (if neg { 0x8000_0000 } else { 0x7FFF_FFFF }, 1000 << LOG_OUTPUT_PREC);
        }
        let lz = temp.leading_zeros() as i32;
        temp <<= lz;
        exp += lz;
        let index = ((temp >> (31 - RECIPLOG_LOOKUP_BITS - 1)) & ((2 << RECIPLOG_LOOKUP_BITS) - 2)) as usize;
        let table = &self.reciplog[index..index + 4];
        let interp = (temp >> (31 - RECIPLOG_LOOKUP_BITS - 8)) & 0xFF;
        let rlog = table[1].wrapping_mul(0x100 - interp).wrapping_add(table[3].wrapping_mul(interp)) >> 8;
        let mut recip =
            (table[0].wrapping_mul(0x100 - interp).wrapping_add(table[2].wrapping_mul(interp)) >> 8) as u64;
        let rlog = (rlog + (1 << (RECIPLOG_LOOKUP_PREC - LOG_OUTPUT_PREC - 1))) >> (RECIPLOG_LOOKUP_PREC - LOG_OUTPUT_PREC);
        let log2 = ((exp - (31 - RECIPLOG_INPUT_PREC)) << LOG_OUTPUT_PREC) - rlog as i32;
        exp += (RECIP_OUTPUT_PREC - RECIPLOG_LOOKUP_PREC) - (31 - RECIPLOG_INPUT_PREC);
        if exp < 0 {
            recip >>= -exp;
        } else {
            recip <<= exp;
        }
        (if neg { (recip as i64).wrapping_neg() } else { recip as i64 }, log2)
    }
}

/// An IEEE single in a float register as a fixed-point value with
/// `fixedbits` of fraction.
pub fn float_to_int32(data: u32, fixedbits: i32) -> i32 {
    let exponent = ((data >> 23) & 0xFF) as i32 - 127 - 23 + fixedbits;
    let mut result = ((data & 0x7F_FFFF) | 0x80_0000) as i32;
    if exponent < 0 {
        result = if exponent > -32 { result >> -exponent } else { 0 };
    } else {
        result = if exponent < 32 { result.wrapping_shl(exponent as u32) } else { 0x7FFF_FFFF };
    }
    if data & 0x8000_0000 != 0 { result.wrapping_neg() } else { result }
}

pub fn float_to_int64(data: u32, fixedbits: i32) -> i64 {
    let exponent = ((data >> 23) & 0xFF) as i32 - 127 - 23 + fixedbits;
    let mut result = ((data & 0x7F_FFFF) | 0x80_0000) as i64;
    if exponent < 0 {
        result = if exponent > -64 { result >> -exponent } else { 0 };
    } else {
        result = if exponent < 64 { result.wrapping_shl(exponent as u32) } else { 0x7FFF_FFFF_FFFF_FFFF };
    }
    if data & 0x8000_0000 != 0 { result.wrapping_neg() } else { result }
}

/// Four ARGB texels weighed by the 8-bit fractions `u` and `v`, two
/// components at a time as the original does it in 32-bit arithmetic.
#[inline]
pub fn bilinear(rgb00: u32, rgb01: u32, rgb10: u32, rgb11: u32, u: u32, v: u32) -> u32 {
    let lerp = |a: u32, b: u32, f: u32| {
        (a & 0x00FF_00FF).wrapping_add(((b & 0x00FF_00FF).wrapping_sub(a & 0x00FF_00FF)).wrapping_mul(f) >> 8)
    };
    let rb0 = lerp(rgb00, rgb01, u);
    let rb1 = lerp(rgb10, rgb11, u);
    let ag0 = lerp(rgb00 >> 8, rgb01 >> 8, u);
    let ag1 = lerp(rgb10 >> 8, rgb11 >> 8, u);
    let rb = lerp(rb0, rb1, v);
    let ag = lerp(ag0, ag1, v);
    ((ag << 8) & 0xFF00_FF00) | (rb & 0x00FF_00FF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_registers_convert_to_fixed_point() {
        assert_eq!(float_to_int32(1.0f32.to_bits(), 4), 16);
        assert_eq!(float_to_int32((-2.5f32).to_bits(), 12), -10240);
        assert_eq!(float_to_int32(0.0f32.to_bits(), 12), 0);
        assert_eq!(float_to_int64(0.5f32.to_bits(), 32), 1 << 31);
        assert_eq!(float_to_int64(3.0f32.to_bits(), 32), 3 << 32);
    }

    #[test]
    fn reciplog_gives_reciprocal_and_log() {
        let t = tables();
        // 1.0 in 16.32 is 1 << 32: its reciprocal in 16.16... the table's
        // output precision, and log2(1/1) = 0.
        let (recip, log) = t.reciplog(1 << 32);
        assert_eq!(log, 0);
        assert_eq!(recip, 1 << RECIP_OUTPUT_PREC);
        let (recip, log) = t.reciplog(2 << 32);
        assert_eq!(log, -256);
        assert_eq!(recip, 1 << (RECIP_OUTPUT_PREC - 1));
        assert_eq!(t.reciplog(0).1, 1000 << 8);
    }

    #[test]
    fn dither_tables_reduce_to_5_and_6_bits() {
        let t = tables();
        // White stays white, black black, whatever the position.
        for x in 0..4 {
            assert_eq!(t.dither4[0xFF << 3 | x << 1], 31);
            assert_eq!(t.dither4[0xFF << 3 | x << 1 | 1], 63);
            assert_eq!(t.dither2[x << 1], 0);
        }
    }

    #[test]
    fn bilinear_blends_components() {
        // Halfway between black and white in both directions.
        let c = bilinear(0xFF00_0000, 0xFFFF_FFFF, 0xFF00_0000, 0xFFFF_FFFF, 0x80, 0x80);
        assert_eq!(c & 0xFF, 0x7F);
        assert_eq!(c >> 24, 0xFF);
        assert_eq!(bilinear(1, 2, 3, 4, 0, 0), 1);
    }

    #[test]
    fn texel_tables_expand_formats() {
        let t = tables();
        assert_eq!(t.rgb565[0xFFFF], 0xFFFF_FFFF);
        assert_eq!(t.rgb565[0xF800], 0xFFFF_0000);
        assert_eq!(t.argb1555[0x8000], 0xFF00_0000);
        assert_eq!(t.argb1555[0x7C00] >> 24, 0);
        assert_eq!(t.argb4444[0xF00F], 0xFF00_00FF);
        assert_eq!(t.ai44[0xF0], 0xFF00_0000);
        assert_eq!(t.rgb332[0xE0], 0xFFFF_0000);
    }
}

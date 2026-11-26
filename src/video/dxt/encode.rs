//! Compressing pixels into DXT blocks, as DirectDraw does when a blit
//! copies pixels into a DXT surface.
//!
//! Microsoft's encoder isn't known block for block, so this one aims at
//! what any good S3TC encoder gives: for each block it tries a few
//! pairs of end colours (the ends of the pixels' spread along their main
//! axis, those ends refined by least squares, and the corners of their
//! bounding box) and keeps the pair whose decoded block is nearest the
//! pixels. Each pair is judged by `decode_block`'s own palette, so the
//! codes picked are the best ones for the decoder.
//!
//! The blocks are made for the specification's decoder
//! (`Rules::S3tc`), which the cards that read DXT textures follow. A
//! block of DXT2 to DXT5 always has its first colour greater than its
//! second, unless the two are equal: DirectDraw's decoder reads any
//! other block as three colours and black, so such a block decodes the
//! same under both sets of rules. The alpha values between a DXT4 or
//! DXT5 block's two ends are the one place where the rules differ on a
//! block made here.
//!
//! DXT1 keeps a pixel whose alpha is under 128 as transparent black,
//! the third code of a three-colour block. The arithmetic is in `f32`
//! with no functions that round differently from one host to another,
//! so a block comes out the same everywhere.

use super::{Format, Rgba, Rules, alpha_values, palette};

/// The alpha below which DXT1 keeps a pixel transparent.
const OPAQUE: u8 = 128;

/// The block of `format` that holds the 16 pixels `pixels` (row by
/// row).
pub fn encode_block(format: Format, pixels: &[Rgba; 16]) -> Vec<u8> {
    let mut block = vec![0u8; format.block_bytes()];
    let colour = match format {
        Format::Dxt1 => &mut block[..],
        _ => &mut block[8..],
    };
    colour.copy_from_slice(&encode_colour(format, pixels));
    match format {
        Format::Dxt1 => {}
        Format::Dxt2 | Format::Dxt3 => block[..8].copy_from_slice(&explicit_alpha(pixels)),
        Format::Dxt4 | Format::Dxt5 => block[..8].copy_from_slice(&interpolated_alpha(pixels)),
    }
    block
}

/// DXT2's and DXT3's alpha: each pixel's 4 bits nearest its alpha, the
/// low ones first.
fn explicit_alpha(pixels: &[Rgba; 16]) -> [u8; 8] {
    let nibble = |p: &Rgba| (p[3] as u32 + 8) / 17;
    std::array::from_fn(|i| (nibble(&pixels[2 * i]) | nibble(&pixels[2 * i + 1]) << 4) as u8)
}

/// The block of DXT4's or DXT5's alpha with the ends `a0` and `a1`, and
/// its error: each pixel takes the code of the value nearest its alpha.
fn alpha_block(pixels: &[Rgba; 16], a0: u8, a1: u8) -> ([u8; 8], u32) {
    let mut block = [a0, a1, 0, 0, 0, 0, 0, 0];
    let values = alpha_values(&block, Rules::S3tc);
    let mut codes = 0u64;
    let mut error = 0;
    for (i, p) in pixels.iter().enumerate() {
        let a = p[3] as i32;
        let (code, e) = (0..8)
            .map(|c| (c, (values[c] as i32 - a).unsigned_abs()))
            .min_by_key(|&(_, e)| e)
            .unwrap();
        codes |= (code as u64) << (3 * i);
        error += e * e;
    }
    block[2..].copy_from_slice(&codes.to_le_bytes()[..6]);
    (block, error)
}

/// DXT4's and DXT5's alpha: of the eight values between the lowest and
/// the highest alpha, or the six between those other than 0 and 255
/// (with 0 and 255 as codes of their own), the one nearer the pixels.
fn interpolated_alpha(pixels: &[Rgba; 16]) -> [u8; 8] {
    let alphas = pixels.iter().map(|p| p[3]);
    let (lo, hi) = (alphas.clone().min().unwrap(), alphas.clone().max().unwrap());
    let inner = alphas.filter(|&a| a != 0 && a != 255);
    let (inner_lo, inner_hi) = (inner.clone().min().unwrap_or(lo), inner.max().unwrap_or(hi));
    let mut best = alpha_block(pixels, inner_lo, inner_hi);
    if hi > lo {
        let eight = alpha_block(pixels, hi, lo);
        if eight.1 < best.1 {
            best = eight;
        }
    }
    best.0
}

/// A colour of three channels from 0 to 255.
type Colour = [f32; 3];

fn colour(p: &Rgba) -> Colour {
    [p[0] as f32, p[1] as f32, p[2] as f32]
}

/// The 5-6-5 value nearest `c`.
fn narrow(c: Colour) -> u16 {
    let q = |v: f32, max: f32| (v.clamp(0.0, 255.0) * max / 255.0 + 0.5) as u16;
    q(c[0], 31.0) << 11 | q(c[1], 63.0) << 5 | q(c[2], 31.0)
}

/// The colour half of a block with the colours `c0` and `c1`, and its
/// error: each pixel takes the code of the colour nearest it that the
/// decoder gives, and a pixel DXT1 keeps transparent the transparent
/// code. None if a pixel has no code: a transparent one in a
/// four-colour block, or an opaque one with only transparent black.
fn colour_block(format: Format, pixels: &[Rgba; 16], c0: u16, c1: u16) -> Option<([u8; 8], u64)> {
    let mut block = [0u8; 8];
    block[..2].copy_from_slice(&c0.to_le_bytes());
    block[2..4].copy_from_slice(&c1.to_le_bytes());
    let colours = palette(&block, format, Rules::S3tc);
    let mut codes = 0u32;
    let mut error = 0u64;
    for (i, p) in pixels.iter().enumerate() {
        let clear = format == Format::Dxt1 && p[3] < OPAQUE;
        let (code, e) = (0..4)
            .filter(|&c| (colours[c][3] == 0) == clear)
            .map(|c| {
                let d = |k: usize| (colours[c][k] as i32 - p[k] as i32).pow(2) as u64;
                (c, if clear { 0 } else { d(0) + d(1) + d(2) })
            })
            .min_by_key(|&(_, e)| e)?;
        codes |= (code as u32) << (2 * i);
        error += e;
    }
    block[4..].copy_from_slice(&codes.to_le_bytes());
    Some((block, error))
}

/// The main axis of the colours `cs` around their mean `mean`: the
/// eigenvector of their covariance with the largest eigenvalue, found
/// by repeated multiplication. Zero when they are all one colour.
///
/// The multiplication starts from the covariance's column of the
/// channel that varies most. A fixed start such as grey (1, 1, 1) is
/// at right angles to the axis of colours whose channels add up to the
/// same sum, red and green for one, and would give no axis for them.
fn main_axis(cs: &[Colour], mean: Colour) -> Colour {
    let mut cov = [[0f32; 3]; 3];
    for c in cs {
        let d = [c[0] - mean[0], c[1] - mean[1], c[2] - mean[2]];
        for (i, row) in cov.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v += d[i] * d[j];
            }
        }
    }
    let widest = (0..3).fold(0, |w, k| if cov[k][k] > cov[w][w] { k } else { w });
    let mut axis = cov[widest];
    for _ in 0..8 {
        let next: Colour = std::array::from_fn(|i| {
            cov[i][0] * axis[0] + cov[i][1] * axis[1] + cov[i][2] * axis[2]
        });
        let largest = next.iter().fold(0f32, |m, v| m.max(v.abs()));
        if largest == 0.0 {
            return [0.0; 3];
        }
        axis = next.map(|v| v / largest);
    }
    axis
}

/// The two colours at the ends of `cs` along `axis`.
fn ends(cs: &[Colour], axis: Colour) -> (Colour, Colour) {
    let dot = |c: &Colour| c[0] * axis[0] + c[1] * axis[1] + c[2] * axis[2];
    let mut lo = (f32::MAX, cs[0]);
    let mut hi = (f32::MIN, cs[0]);
    for c in cs {
        let t = dot(c);
        if t < lo.0 {
            lo = (t, *c);
        }
        if t > hi.0 {
            hi = (t, *c);
        }
    }
    (lo.1, hi.1)
}

/// The end colours that put the pixels `cs` nearest the points their
/// codes give, each a weight `w` of the first end and `1 - w` of the
/// second: the least squares solution. None when the weights don't
/// pin both ends down.
fn least_squares(cs: &[Colour], weights: &[f32]) -> Option<(Colour, Colour)> {
    let (mut aa, mut bb, mut ab) = (0f32, 0f32, 0f32);
    let mut ax = [0f32; 3];
    let mut bx = [0f32; 3];
    for (c, &w) in cs.iter().zip(weights) {
        let v = 1.0 - w;
        aa += w * w;
        bb += v * v;
        ab += w * v;
        for k in 0..3 {
            ax[k] += w * c[k];
            bx[k] += v * c[k];
        }
    }
    let det = aa * bb - ab * ab;
    if det.abs() < 1e-6 {
        return None;
    }
    let a = std::array::from_fn(|k| (ax[k] * bb - bx[k] * ab) / det);
    let b = std::array::from_fn(|k| (bx[k] * aa - ax[k] * ab) / det);
    Some((a, b))
}

/// The pair of 5-6-5 colours, in the order the block's kind needs: the
/// first greater for four colours, not greater for three.
fn ordered(a: u16, b: u16, four: bool) -> (u16, u16) {
    if (a > b) == four || a == b {
        (a, b)
    } else {
        (b, a)
    }
}

/// The colour half of the block for `pixels`.
fn encode_colour(format: Format, pixels: &[Rgba; 16]) -> [u8; 8] {
    let clear = |p: &Rgba| format == Format::Dxt1 && p[3] < OPAQUE;
    let opaque: Vec<Colour> = pixels.iter().filter(|p| !clear(p)).map(colour).collect();
    // A DXT1 block with transparent pixels is a three-colour one.
    let four = opaque.len() == 16;
    let mut best: Option<([u8; 8], u64)> = None;
    let consider = |best: &mut Option<([u8; 8], u64)>, a: Colour, b: Colour| {
        let (c0, c1) = ordered(narrow(a), narrow(b), four);
        if let Some(found) = colour_block(format, pixels, c0, c1)
            && best.is_none_or(|(_, e)| found.1 < e)
        {
            *best = Some(found);
        }
    };
    if opaque.is_empty() {
        consider(&mut best, [0.0; 3], [0.0; 3]);
        return best.map_or([0; 8], |(b, _)| b);
    }
    let n = opaque.len() as f32;
    let mean: Colour = std::array::from_fn(|k| opaque.iter().map(|c| c[k]).sum::<f32>() / n);
    let (lo, hi) = ends(&opaque, main_axis(&opaque, mean));
    consider(&mut best, hi, lo);
    // The bounding box's corners.
    let min = std::array::from_fn(|k| opaque.iter().fold(255f32, |m, c| m.min(c[k])));
    let max = std::array::from_fn(|k| opaque.iter().fold(0f32, |m, c| m.max(c[k])));
    consider(&mut best, max, min);
    // Refine the best so far twice by least squares over its codes.
    for _ in 0..2 {
        let Some((block, _)) = best else { break };
        let codes = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        let c0 = u16::from_le_bytes([block[0], block[1]]);
        let c1 = u16::from_le_bytes([block[2], block[3]]);
        let three = !(c0 > c1);
        let weight = |code: u32| match (code, three) {
            (0, _) => 1.0,
            (1, _) => 0.0,
            (2, false) => 2.0 / 3.0,
            (3, false) => 1.0 / 3.0,
            _ => 0.5,
        };
        let weights: Vec<f32> = pixels
            .iter()
            .enumerate()
            .filter(|(_, p)| !clear(p))
            .map(|(i, _)| weight(codes >> (2 * i) & 3))
            .collect();
        match least_squares(&opaque, &weights) {
            Some((a, b)) => consider(&mut best, a, b),
            None => break,
        }
    }
    best.map_or([0; 8], |(b, _)| b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::dxt::decode_block;

    /// The pixels the block `block` of `format` decodes to under the
    /// specification: what a card shows of a block made here.
    fn decoded(format: Format, block: &[u8]) -> [Rgba; 16] {
        decode_block(format, Rules::S3tc, block)
    }

    const FORMATS: [Format; 5] = [
        Format::Dxt1,
        Format::Dxt2,
        Format::Dxt3,
        Format::Dxt4,
        Format::Dxt5,
    ];

    /// A block of one colour comes back as it was, to 5-6-5's precision,
    /// in every format and under both decoders.
    #[test]
    fn one_colour_comes_back() {
        let c = [255, 0, 0, 255];
        for format in FORMATS {
            let block = encode_block(format, &[c; 16]);
            for rules in [Rules::S3tc, Rules::DirectDraw] {
                assert_eq!(decode_block(format, rules, &block), [c; 16], "{format:?}");
            }
        }
    }

    /// Two 5-6-5 colours, half the pixels each, come back exactly: they
    /// are the block's two ends.
    #[test]
    fn two_colours_come_back_exactly() {
        let (a, b) = ([255, 255, 0, 255], [8, 8, 24, 255]);
        let pixels: [Rgba; 16] = std::array::from_fn(|i| if i % 3 == 0 { a } else { b });
        for format in FORMATS {
            let block = encode_block(format, &pixels);
            assert_eq!(decoded(format, &block), pixels, "{format:?}");
            assert_eq!(decode_block(format, Rules::DirectDraw, &block), pixels);
        }
    }

    /// Two colours whose channels add up to the same sum, red and green
    /// half the pixels each, come back exactly too.
    #[test]
    fn red_and_green_come_back_exactly() {
        let (a, b) = ([255, 0, 0, 255], [0, 255, 0, 255]);
        let pixels: [Rgba; 16] = std::array::from_fn(|i| if i % 2 == 0 { a } else { b });
        for format in FORMATS {
            let block = encode_block(format, &pixels);
            assert_eq!(decoded(format, &block), pixels, "{format:?}");
        }
    }

    /// A ramp of 16 greys on four codes comes back within half the step
    /// between two codes (255 / 3 / 2) of each pixel.
    #[test]
    fn a_ramp_comes_back_close() {
        let pixels: [Rgba; 16] = std::array::from_fn(|i| {
            let v = (i * 17) as u8;
            [v, v, v, 255]
        });
        for format in [Format::Dxt1, Format::Dxt3, Format::Dxt5] {
            let out = decoded(format, &encode_block(format, &pixels));
            for (p, q) in pixels.iter().zip(&out) {
                for k in 0..3 {
                    assert!((p[k] as i32 - q[k] as i32).abs() <= 43, "{p:?} {q:?}");
                }
            }
        }
    }

    /// DXT1 keeps a pixel of alpha under 128 transparent black, and the
    /// others opaque.
    #[test]
    fn dxt1_keeps_low_alpha_transparent() {
        let pixels: [Rgba; 16] = std::array::from_fn(|i| match i % 4 {
            0 => [10, 20, 30, 0],
            1 => [200, 100, 50, 127],
            _ => [0, 255, 0, 200],
        });
        let out = decoded(Format::Dxt1, &encode_block(Format::Dxt1, &pixels));
        for (p, q) in pixels.iter().zip(&out) {
            if p[3] < 128 {
                assert_eq!(*q, [0, 0, 0, 0]);
            } else {
                assert_eq!(*q, [0, 255, 0, 255]);
            }
        }
        let clear = [[9, 9, 9, 3]; 16];
        let out = decoded(Format::Dxt1, &encode_block(Format::Dxt1, &clear));
        assert_eq!(out, [[0, 0, 0, 0]; 16]);
    }

    /// DXT3 keeps 4 bits of each pixel's alpha, the nearest.
    #[test]
    fn dxt3_alpha_is_the_nearest_of_sixteen() {
        let pixels: [Rgba; 16] = std::array::from_fn(|i| [255, 255, 255, (i * 16) as u8]);
        let out = decoded(Format::Dxt3, &encode_block(Format::Dxt3, &pixels));
        for (p, q) in pixels.iter().zip(&out) {
            let nearest = ((p[3] as u32 + 8) / 17 * 17) as u8;
            assert_eq!(q[3], nearest);
        }
    }

    /// DXT5's alpha comes back within half a step of its eight values,
    /// and exactly where the pixels have only the two ends, or 0 and
    /// 255 besides two others.
    #[test]
    fn dxt5_alpha_comes_back_close() {
        let ramp: [Rgba; 16] = std::array::from_fn(|i| [0, 0, 0, 40 + (i * 9) as u8]);
        let out = decoded(Format::Dxt5, &encode_block(Format::Dxt5, &ramp));
        for (p, q) in ramp.iter().zip(&out) {
            assert!((p[3] as i32 - q[3] as i32).abs() <= 10, "{p:?} {q:?}");
        }
        let ends: [Rgba; 16] = std::array::from_fn(|i| {
            let a = [0, 255, 60, 90][i % 4];
            [0, 0, 0, a]
        });
        let out = decoded(Format::Dxt5, &encode_block(Format::Dxt5, &ends));
        assert_eq!(out.map(|p| p[3]), ends.map(|p| p[3]));
    }

    /// A DXT1 or DXT3 block made here decodes the same under
    /// DirectDraw's rules as under the specification's: DXT3's colour
    /// half is the one DXT2 to DXT5 share, and its alpha has one rule.
    #[test]
    fn colour_halves_decode_alike_under_both_rules() {
        let mut seed = 0x9E37_79B9u32;
        for _ in 0..200 {
            let pixels: [Rgba; 16] = std::array::from_fn(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let v = seed.to_le_bytes();
                [v[0], v[1], v[2], 255]
            });
            for format in [Format::Dxt1, Format::Dxt3] {
                let block = encode_block(format, &pixels);
                assert_eq!(
                    decode_block(format, Rules::S3tc, &block),
                    decode_block(format, Rules::DirectDraw, &block)
                );
            }
        }
    }
}

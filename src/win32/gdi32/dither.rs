//! DrawDib's dither: how MSVFW32's DrawDib functions, and MCIAVI, which
//! draws through them, show a picture of 16, 24 or 32 bits a pixel on a
//! display of 256 colours. Each pixel's red, green and blue take one of
//! the six levels (0, 51, ... 255) of the halftone palette's colour
//! cube, chosen by a 4x4 ordered dither, so an area between two of the
//! cube's colours becomes a pattern of both. The picture is then drawn
//! as an 8-bit DIB of the halftone palette's colours. GDI's own
//! StretchDIBits and SetDIBitsToDevice don't dither: on Windows 95 and
//! 98 they map each pixel to one colour of the DC's palette through an
//! inverse colour table (`DeviceMap::deep_color`), whatever the stretch
//! mode (those have no HALFTONE mode).
//!
//! This is what Windows 98 SE's DrawDib draws, read back pixel by pixel
//! from pictures drawn on an 8-bit desktop (a 24-bit and a 16-bit DIB,
//! at 1:1, moved, doubled and stretched to a size that isn't a multiple
//! of the picture's):
//!
//! - DrawDib stretches first and dithers the pixels it draws: the
//!   matrix repeats every 4 pixels of the destination whatever the
//!   scale, so a source pixel drawn twice takes two cells.
//! - The matrix starts at the destination rectangle's bottom left pixel
//!   and runs up, as a bottom-up DIB's rows do; it doesn't follow the
//!   screen. The pixel `x` from the left and `y` from the bottom has the
//!   rank `BAYER[x % 4][y % 4]`.
//! - Stretching takes the source pixel `x * source width / width` from
//!   the left and `y * source height / height` from the bottom.
//! - An 8-bit channel value `v` takes the level `v / 51`, and the next
//!   one when the rest, `v % 51`, is above `51 * rank / 16`.
//! - A 16-bit (5-5-5) picture goes by its 5-bit channels: the level is
//!   `v / 6`, and the next one when `v % 6` reaches the rank's threshold
//!   in `THRESHOLDS_16`.
//!
//! The levels of each rank and channel value are tables built at
//! compile time, so a pixel costs three lookups.
//!
//! Windows 98 doesn't draw Cinepak movies this way: Cinepak's codec
//! decodes them to 8 bits itself, in a palette and with a dither of its
//! own, which MCIAVI realizes. Until rust-dos has that (#1712), they are
//! dithered here as the other movies are.

use super::palette::{HALFTONE, halftone_colors};
use super::surface::{Surface, default_masks};

/// The 4x4 ordered dither matrix: the rank of the pixel `x` from the
/// destination's left and `y` from its bottom is `BAYER[x % 4][y % 4]`.
const BAYER: [[usize; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// For a 16-bit picture, the rest (`v % 6` of a 5-bit channel `v`) from
/// which a pixel of each rank takes the next level; 6 never does.
const THRESHOLDS_16: [usize; 16] = [2, 6, 4, 4, 6, 3, 5, 3, 5, 3, 5, 2, 4, 4, 2, 6];

/// The halftone palette's index of the cube's colour of levels (r, g,
/// b), at `r + 6 g + 36 b`. The palette lacks four of the cube's colours,
/// those of one channel 0, one 51 and one 255: Windows 98's DrawDib
/// draws (0, 51, 255) and (51, 0, 255) as (0, 0, 255), and (0, 255, 51)
/// and (51, 255, 0) as (0, 255, 0), the last of the palette's colours
/// nearest to them, as here.
static CUBE: [u8; 216] = cube();

const fn cube() -> [u8; 216] {
    let mut table = [0u8; 216];
    let mut at = 0;
    while at < 216 {
        let want = [at % 6 * 51, at / 6 % 6 * 51, at / 36 * 51];
        let mut best = (u32::MAX, 0);
        let mut i = 0;
        while i < 256 {
            let c = HALFTONE[i];
            let mut d = 0;
            let mut s = 0;
            while s < 3 {
                let v = (c >> (8 * s)) & 0xFF;
                let w = want[s] as u32;
                let e = if v > w { v - w } else { w - v };
                d += e * e;
                s += 1;
            }
            if d <= best.0 {
                best = (d, i);
            }
            i += 1;
        }
        table[at] = best.1 as u8;
        at += 1;
    }
    table
}

/// For each rank and 8-bit channel value, the cube's level it takes.
static LEVELS: [[u8; 256]; 16] = levels();

/// For each rank and 5-bit channel value, the cube's level it takes.
static LEVELS_16: [[u8; 32]; 16] = levels_16();

const fn levels() -> [[u8; 256]; 16] {
    let mut table = [[0u8; 256]; 16];
    let mut rank = 0;
    while rank < 16 {
        let threshold = 51 * rank / 16;
        let mut v = 0;
        while v < 256 {
            table[rank][v] = (v / 51 + (v % 51 > threshold) as usize) as u8;
            v += 1;
        }
        rank += 1;
    }
    table
}

const fn levels_16() -> [[u8; 32]; 16] {
    let mut table = [[0u8; 32]; 16];
    let mut rank = 0;
    while rank < 16 {
        let mut v = 0;
        while v < 32 {
            table[rank][v] = (v / 6 + (v % 6 >= THRESHOLDS_16[rank]) as usize) as u8;
            v += 1;
        }
        rank += 1;
    }
    table
}

/// The rank of the pixel `x` from the destination's left and `y` from
/// its bottom.
#[inline]
fn rank(x: i32, y: i32) -> usize {
    BAYER[(x & 3) as usize][(y & 3) as usize]
}

/// The halftone palette's index of the cube's colour of levels (r, g, b).
#[inline]
fn cube_index(r: u8, g: u8, b: u8) -> u32 {
    CUBE[r as usize + 6 * g as usize + 36 * b as usize] as u32
}

/// The halftone palette's index of the colour (a COLORREF) of the pixel
/// `x` from the destination's left and `y` from its bottom.
#[inline]
pub fn halftone_index(x: i32, y: i32, color: u32) -> u8 {
    let levels = &LEVELS[rank(x, y)];
    let r = levels[(color & 0xFF) as usize];
    let g = levels[((color >> 8) & 0xFF) as usize];
    let b = levels[((color >> 16) & 0xFF) as usize];
    cube_index(r, g, b) as u8
}

/// The part `from` (left, top, right, bottom, with rows counted from the
/// top) of the picture `src`, of more than 8 bits a pixel, stretched to
/// `width` by `height` and dithered to an 8-bit picture of the halftone
/// palette's colours, as DrawDib draws it in a destination of that size.
/// A source pixel outside `src` takes the nearest one of its edge.
pub fn to_halftone(src: &Surface, from: (i32, i32, i32, i32), width: i32, height: i32) -> Surface {
    let mut out = Surface::memory(
        width.max(0),
        height.max(0),
        8,
        halftone_colors(),
        default_masks(8),
    );
    let (left, top, right, bottom) = from;
    let (sw, sh) = ((right - left) as i64, (bottom - top) as i64);
    if width <= 0 || height <= 0 || sw <= 0 || sh <= 0 || src.width <= 0 || src.height <= 0 {
        return out;
    }
    // 24 and 32 bits of the usual layout give their channels straight,
    // and 16 bits of 5-5-5 their 5-bit ones.
    let plain = matches!(src.bpp, 24 | 32) && src.masks == default_masks(32);
    let five = src.bpp == 16 && src.masks == default_masks(16);
    let columns: Vec<usize> = (0..width as i64)
        .map(|x| (left + (x * sw / width as i64) as i32).clamp(0, src.width - 1) as usize)
        .collect();
    let mut line = vec![0; src.width as usize];
    let mut values = vec![0; width as usize];
    let mut loaded = None;
    for y in 0..height {
        let up = height - 1 - y;
        let sy = bottom - 1 - (up as i64 * sh / height as i64) as i32;
        let sy = sy.clamp(0, src.height - 1);
        if loaded != Some(sy) {
            src.get_row(0, sy, &mut line);
            loaded = Some(sy);
        }
        for ((x, &sx), v) in (0..).zip(&columns).zip(values.iter_mut()) {
            let p = line[sx];
            *v = if five {
                let levels = &LEVELS_16[rank(x, up)];
                let channel = |shift: u32| levels[((p >> shift) & 0x1F) as usize];
                cube_index(channel(10), channel(5), channel(0))
            } else {
                let color = if plain {
                    (p >> 16) & 0xFF | p & 0xFF00 | (p & 0xFF) << 16
                } else {
                    src.color_of(p)
                };
                halftone_index(x, up, color) as u32
            };
        }
        out.put_row(0, y, &values);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A colour of the cube stays that colour in every cell.
    #[test]
    fn the_cube_colours_stay_flat() {
        let colors = halftone_colors();
        for level in 0..6u32 {
            let v = level * 51;
            let color = v | v << 8 | v << 16;
            for y in 0..4 {
                for x in 0..4 {
                    let i = halftone_index(x, y, color);
                    assert_eq!(colors[i as usize], color);
                }
            }
        }
    }

    /// Windows 98's DrawDib draws grey 102 + 51 k / 16 with k pixels of
    /// each 4x4 cell at the next level, taking them in this order (x
    /// from the left, y from the bottom).
    #[test]
    fn the_pixels_rise_in_windows_order() {
        let order = [
            (0, 0),
            (2, 2),
            (0, 2),
            (2, 0),
            (1, 1),
            (3, 3),
            (1, 3),
            (3, 1),
            (0, 1),
            (2, 3),
            (0, 3),
            (2, 1),
            (1, 0),
            (3, 2),
            (1, 2),
            (3, 0),
        ];
        for k in 0..16u32 {
            let v = 102 + 51 * k / 16;
            let grey = v | v << 8 | v << 16;
            for (n, &(x, y)) in order.iter().enumerate() {
                let raised = halftone_index(x, y, grey) as u32 != cube_index(2, 2, 2);
                assert_eq!(raised, n < k as usize, "grey {v}, pixel ({x}, {y})");
            }
        }
    }

    /// Every colour of the cube the palette has is drawn as itself, and
    /// the four it lacks as Windows 98's DrawDib draws them.
    #[test]
    fn the_cube_maps_to_the_palette() {
        for at in 0..216u32 {
            let (r, g, b) = (at % 6 * 51, at / 6 % 6 * 51, at / 36 * 51);
            let want = match (r, g, b) {
                (0, 51, 255) | (51, 0, 255) => 0xFF0000,
                (0, 255, 51) | (51, 255, 0) => 0x00FF00,
                _ => r | g << 8 | b << 16,
            };
            let i = cube_index((r / 51) as u8, (g / 51) as u8, (b / 51) as u8);
            assert_eq!(HALFTONE[i as usize], want, "({r}, {g}, {b})");
        }
    }

    /// A value between levels averages to itself over the matrix.
    #[test]
    fn a_value_between_levels_is_a_pattern_of_both() {
        for v in [25u32, 100, 128, 200] {
            let mut sum = 0;
            for y in 0..4 {
                for x in 0..4 {
                    sum += HALFTONE[halftone_index(x, y, v) as usize] & 0xFF;
                }
            }
            let average = sum / 16;
            assert!(average.abs_diff(v) <= 8, "{v}: average {average}");
        }
    }

    /// A doubled picture is dithered at its new size, from its bottom
    /// row: each source pixel takes four pixels of the matrix.
    #[test]
    fn the_matrix_follows_the_destination() {
        let mut src = Surface::memory(2, 2, 24, Vec::new(), default_masks(24));
        src.put_row(0, 0, &[0x80_8080, 0x80_8080]);
        src.put_row(0, 1, &[0x80_8080, 0x80_8080]);
        let out = to_halftone(&src, (0, 0, 2, 2), 4, 4);
        let mut row = [0; 4];
        // Grey 128 is 26 above level 2 (102): ranks 0 to 8 go up.
        for (top, up) in (0..4).zip((0..4).rev()) {
            out.get_row(0, top, &mut row);
            for x in 0..4 {
                let level = if rank(x, up) <= 8 { 3 } else { 2 };
                let expected = cube_index(level, level, level);
                assert_eq!(row[x as usize], expected, "({x}, {up} from the bottom)");
            }
        }
    }

    /// A 16-bit picture goes by its 5-bit channels: grey 16 of 31 (level
    /// 2, rest 4) raises the ten ranks whose threshold is 4 or less.
    #[test]
    fn a_16_bit_picture_uses_its_own_table() {
        let mut src = Surface::memory(4, 4, 16, Vec::new(), default_masks(16));
        let grey = 16 << 10 | 16 << 5 | 16;
        for y in 0..4 {
            src.put_row(0, y, &[grey; 4]);
        }
        let out = to_halftone(&src, (0, 0, 4, 4), 4, 4);
        let mut row = [0; 4];
        let mut raised = 0;
        for y in 0..4 {
            out.get_row(0, y, &mut row);
            raised += row.iter().filter(|&&v| v == cube_index(3, 3, 3)).count();
        }
        assert_eq!(raised, 10);
    }
}

//! DrawDib's dither: how MSVFW32's DrawDib functions, and MCIAVI, which
//! draws through them, show a picture of 16, 24 or 32 bits a pixel on a
//! display of 256 colours. Each pixel's red, green and blue take one of
//! the six levels (0, 51, ... 255) of the halftone palette's colour
//! cube, chosen by a 4x4 ordered (Bayer) dither, so an area between two
//! of the cube's colours becomes a pattern of both. The picture is then
//! drawn as an 8-bit DIB of the halftone palette's colours. GDI's own
//! StretchDIBits and SetDIBitsToDevice don't dither: on Windows 95 and
//! 98 they map each pixel to the nearest colour of the DC's palette,
//! whatever the stretch mode (those have no HALFTONE mode).
//!
//! The levels of each matrix cell and channel value are a table built
//! at compile time, so a pixel costs three lookups.

use super::palette::halftone_colors;
use super::surface::{Surface, default_masks};

/// The 4x4 ordered dither matrix.
const BAYER: [[u32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// The halftone palette's index of the cube's colour (0, 0, 0); the
/// colour of levels (r, g, b) is at `CUBE + r + 6 g + 36 b`.
pub const CUBE: u32 = 10;

/// For each cell of the matrix (`y % 4 * 4 + x % 4`) and each channel
/// value, the cube's level the channel takes: the value's place between
/// two levels, against the cell's threshold.
static LEVELS: [[u8; 256]; 16] = levels();

const fn levels() -> [[u8; 256]; 16] {
    let mut table = [[0u8; 256]; 16];
    let mut cell = 0;
    while cell < 16 {
        // Thresholds spread evenly over a step, from 1/32 to 31/32.
        let threshold = (2 * BAYER[cell / 4][cell % 4] + 1) * 255 / 32;
        let mut v = 0;
        while v < 256 {
            table[cell][v] = ((v as u32 * 5 + threshold) / 255) as u8;
            v += 1;
        }
        cell += 1;
    }
    table
}

/// The halftone palette's index of the colour (a COLORREF) at the pixel
/// (`x`, `y`) of the picture.
#[inline]
pub fn halftone_index(x: i32, y: i32, color: u32) -> u8 {
    let levels = &LEVELS[((y & 3) * 4 + (x & 3)) as usize];
    let r = levels[(color & 0xFF) as usize] as u32;
    let g = levels[((color >> 8) & 0xFF) as usize] as u32;
    let b = levels[((color >> 16) & 0xFF) as usize] as u32;
    (CUBE + r + 6 * g + 36 * b) as u8
}

/// The picture `src`, of more than 8 bits a pixel, dithered to an 8-bit
/// one of the halftone palette's colours, of the same size. The matrix
/// starts at the picture's top left pixel.
pub fn to_halftone(src: &Surface) -> Surface {
    let mut out = Surface::memory(src.width, src.height, 8, halftone_colors(), default_masks(8));
    let area = src.loaded().intersect(&src.rect());
    if area.is_empty() {
        return out;
    }
    // 24 and 32 bits of the usual layout give their channels straight.
    let plain = matches!(src.bpp, 24 | 32) && src.masks == default_masks(32);
    let mut values = vec![0; (area.right - area.left) as usize];
    for y in area.top..area.bottom {
        src.get_row(area.left, y, &mut values);
        for (x, v) in (area.left..).zip(values.iter_mut()) {
            let color = if plain {
                (*v >> 16) & 0xFF | *v & 0xFF00 | (*v & 0xFF) << 16
            } else {
                src.color_of(*v)
            };
            *v = halftone_index(x, y, color) as u32;
        }
        out.put_row(area.left, y, &values);
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

    /// A value halfway between two levels takes each in half the cells,
    /// and their average over the matrix is the value.
    #[test]
    fn a_value_between_levels_is_a_pattern_of_both() {
        for v in [25u32, 100, 128, 200] {
            let mut sum = 0;
            for y in 0..4 {
                for x in 0..4 {
                    let i = halftone_index(x, y, v) as u32 - CUBE;
                    sum += (i % 6) * 51;
                }
            }
            let average = sum / 16;
            assert!(average.abs_diff(v) <= 8, "{v}: average {average}");
        }
    }
}

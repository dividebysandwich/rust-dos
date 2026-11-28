//! The 8-bit output of Intel's Indeo 4 codec: the palette it gives
//! for ICM_DECOMPRESS_GET_PALETTE, and the ordered dither that puts a
//! YVU9 picture in it. This is what Video for Windows 1.1e's IR41.DLL
//! (4.1, 1995, which Civilization II installs) gives on Windows 95, read
//! back from a probe that drives the driver through its DriverProc and
//! decodes Civilization II's OPENING.AVI: the palette as the driver
//! wrote it, and every pixel of 55 frames as it wrote them, against the
//! planes FFmpeg decodes from the same frames. The driver writes the
//! same indexes whether the program asked for its palette or not, gave
//! it back with ICM_DECOMPRESS_SET_PALETTE or not, and whatever colours
//! the output DIB has.
//!
//! - The palette is the 20 static colours (the first ten at 0 to 9, the
//!   last ten at 246 to 255), then 234 colours of the codec's own at 10
//!   to 243 and two blacks at 244 and 245.
//! - The 234 are 18 chroma points of 13 luma levels each: the colour at
//!   `10 + 13 * point + level`. The points lie on a lattice of 32 steps
//!   of U and V (`POINTS`); the levels are 15 steps of Y apart, the
//!   first for a Y of about 38, so black (Y 16) comes out as the
//!   darkest grey, (25, 25, 25).
//! - A pixel's level is `(Y - t + 15) / 15`, rounded down and kept to
//!   0..12, where the threshold `t` is one of four by the pixel's place
//!   in a 4 by 4 matrix (`LUMA`).
//! - Its chroma point is the lattice point `(U + du - 126) / 32` and
//!   `(V + dv - 126) / 32`, each rounded down and kept to -2..2, where
//!   `du` and `dv` are one of eight steps of 4 by the pixel's place
//!   (`CHROMA_U`, `CHROMA_V`). Seven of the 25 lattice points have no
//!   colour, and take one of a neighbour's (`GRID`).
//!
//! The matrices start at the picture's top left pixel. That gives all
//! but 10 of the 2,112,000 pixels of the 55 frames. The 10 are at the
//! two lattice points without a colour that the frames reach: of the
//! pixels at (-2, 2), 86 take (-2, 1) and 4 (-1, 2); of those at
//! (2, 1), 294 take (1, 1) and 6 (2, 0). The codec decides those by
//! values of U and V finer than the lattice, in a way not worked out;
//! here each takes the neighbour most of its pixels took. The other
//! five points without a colour, which the frames don't reach, take a
//! neighbour of theirs the same way.
//!
//! Windows 98 SE's 32-bit ir41_32.ax (4.31), run the same way, gives the
//! same palette and the same indexes for the frames it decodes as
//! IR41.DLL does (all of the key frame and frame 30 at 320 by 120). Its
//! later frames differ from IR41.DLL's in the decoded picture, which
//! isn't the dither's doing.

use super::Picture;

/// The index of the codec's first colour.
pub const FIRST: usize = 10;

/// The codec's colours, as COLORREFs, at `FIRST`: 13 luma levels of
/// each chroma point in turn.
pub static COLOURS: [u32; 234] = [
    0x191919, 0x2B2B2B, 0x3C3C3C, 0x4D4D4D, 0x5F5F5F, 0x707070, 0x828282, 0x939393, 0xA5A5A5,
    0xB6B6B6, 0xC8C8C8, 0xD9D9D9, 0xEBEBEB, 0x193300, 0x2B4500, 0x3C5609, 0x4D681A, 0x5F792C,
    0x708A3D, 0x829C4F, 0x93AD60, 0xA5BF72, 0xB6D083, 0xC8E295, 0xD9F3A6, 0xEBFFB8, 0x002619,
    0x00372B, 0x00493C, 0x0D5A4E, 0x1E6B5F, 0x307D70, 0x418E82, 0x53A093, 0x64B1A5, 0x76C3B6,
    0x87D4C8, 0x99E6D9, 0xAAF7EB, 0x19004C, 0x2B115E, 0x3C226F, 0x4E3381, 0x5F4592, 0x7056A3,
    0x8268B5, 0x9379C6, 0xA58BD8, 0xB69CE9, 0xC8AEFB, 0xD9BFFF, 0xEBD1FF, 0x5A0D19, 0x6B1E2B,
    0x7D303C, 0x8E414D, 0x9F525F, 0xB16470, 0xC27582, 0xD48793, 0xE598A5, 0xF7AAB6, 0xFFBBC8,
    0xFFCDD9, 0xFFDEEB, 0x004000, 0x005100, 0x006309, 0x0D741A, 0x1E852C, 0x30973D, 0x41A84F,
    0x53BA60, 0x64CB72, 0x76DD83, 0x87EE95, 0x99FFA6, 0xAAFFB8, 0x5A2700, 0x6B3800, 0x7D4A09,
    0x8E5B1A, 0x9F6C2C, 0xB17E3D, 0xC28F4F, 0xD4A160, 0xE5B272, 0xF7C483, 0xFFD595, 0xFFE7A6,
    0xFFF8B8, 0x5A004C, 0x6B045E, 0x7D166F, 0x8E2781, 0xA03892, 0xB14AA3, 0xC25BB5, 0xD46DC6,
    0xE57ED8, 0xF790E9, 0xFFA1FB, 0xFFB3FF, 0xFFC4FF, 0x000C4C, 0x001D5E, 0x002F6F, 0x0D4081,
    0x1E5192, 0x3063A4, 0x4174B5, 0x5386C6, 0x6497D8, 0x76A9E9, 0x87BAFB, 0x99CCFF, 0xAADDFF,
    0x003219, 0x00442B, 0x00553C, 0x00674E, 0x00785F, 0x008970, 0x019B82, 0x12AC93, 0x24BEA5,
    0x35CFB6, 0x47E1C8, 0x58F2D9, 0x6AFFEB, 0x19007F, 0x2B0091, 0x3C08A2, 0x4E19B4, 0x5F2BC5,
    0x703CD7, 0x824EE8, 0x935FF9, 0xA571FF, 0xB682FF, 0xC894FF, 0xD9A5FF, 0xEBB7FF, 0x194D00,
    0x2B5F00, 0x3C7000, 0x4D8200, 0x5F9300, 0x70A40A, 0x82B61C, 0x93C72D, 0xA5D93F, 0xB6EA50,
    0xC8FC62, 0xD9FF73, 0xEBFF84, 0x9A0019, 0xAC122B, 0xBD233C, 0xCF344D, 0xE0465F, 0xF15770,
    0xFF6982, 0xFF7A93, 0xFF8CA5, 0xFF9DB6, 0xFFAFC8, 0xFFC0D9, 0xFFD2EB, 0x00184C, 0x002A5E,
    0x003B6F, 0x004C81, 0x005E92, 0x006FA4, 0x0181B5, 0x1292C6, 0x24A4D8, 0x35B5E9, 0x47C7FB,
    0x58D8FF, 0x6AEAFF, 0x00007F, 0x000391, 0x0015A2, 0x0D26B4, 0x1E37C5, 0x3049D7, 0x415AE8,
    0x536CFA, 0x647DFF, 0x768FFF, 0x87A0FF, 0x99B2FF, 0xAAC3FF, 0x004C00, 0x005E00, 0x006F09,
    0x00811A, 0x00922C, 0x00A33D, 0x01B54F, 0x12C660, 0x24D872, 0x35E983, 0x47FB95, 0x58FFA6,
    0x6AFFB8, 0x5A4100, 0x6B5200, 0x7D6400, 0x8E7500, 0x9F8600, 0xB1980A, 0xC2A91C, 0xD4BB2D,
    0xE5CC3F, 0xF7DE50, 0xFFEF62, 0xFFFF73, 0xFFFF84, 0x9A1A00, 0xAC2C00, 0xBD3D09, 0xCF4E1A,
    0xE0602C, 0xF1713D, 0xFF834F, 0xFF9460, 0xFFA672, 0xFFB783, 0xFFC995, 0xFFDAA6, 0xFFECB7,
];

/// The chroma points' lattice coordinates, U and V in steps of 32 from
/// 128, by point.
const POINTS: [(i8, i8); 18] = [
    (0, 0),
    (0, -1),
    (-1, 0),
    (0, 1),
    (1, 0),
    (-1, -1),
    (1, -1),
    (1, 1),
    (-1, 1),
    (-2, 0),
    (0, 2),
    (0, -2),
    (2, 0),
    (-2, 1),
    (-1, 2),
    (-2, -1),
    (1, -2),
    (2, -1),
];

/// The chroma point of each lattice point, at `GRID[u + 2][v + 2]`.
/// Of the seven with no colour of their own, (-2, 2) takes (-2, 1) and
/// (2, 1) takes (1, 1), as most of the codec's pixels there do;
/// (-2, -2) takes (-2, -1), (2, -2) (2, -1), (-1, -2) (-1, -1), and
/// (1, 2) and (2, 2) (1, 1).
const GRID: [[u8; 5]; 5] = grid();

const fn grid() -> [[u8; 5]; 5] {
    let mut grid = [[0u8; 5]; 5];
    let mut point = 0;
    while point < POINTS.len() {
        let (u, v) = POINTS[point];
        grid[(u + 2) as usize][(v + 2) as usize] = point as u8;
        point += 1;
    }
    // The points with no colour.
    grid[0][4] = 13;
    grid[4][3] = 7;
    grid[0][0] = 15;
    grid[4][0] = 17;
    grid[1][0] = 5;
    grid[3][4] = 7;
    grid[4][4] = 7;
    grid
}

/// The luma thresholds by the pixel's row and column in the 4 by 4
/// matrix: Y from it up takes level 1, from 15 more level 2, and so on.
const LUMA: [[i16; 4]; 4] = [
    [45, 49, 41, 52],
    [41, 52, 45, 49],
    [49, 45, 52, 41],
    [52, 41, 49, 45],
];

/// What each pixel adds to U and to V before they go to the lattice, by
/// its row and column.
const CHROMA_U: [[i16; 4]; 4] = [
    [20, 4, 12, 0],
    [24, 28, 8, 16],
    [12, 0, 20, 4],
    [8, 16, 24, 28],
];
const CHROMA_V: [[i16; 4]; 4] = [
    [12, 4, 20, 28],
    [24, 0, 16, 8],
    [20, 28, 12, 4],
    [16, 8, 24, 0],
];

/// The palette the codec gives, as COLORREFs, with the static colours
/// `statics` (the first ten, then the last ten).
pub fn palette_8(statics: &[u32; 20]) -> [u32; 256] {
    let mut colours = [0u32; 256];
    colours[..10].copy_from_slice(&statics[..10]);
    colours[246..].copy_from_slice(&statics[10..]);
    colours[FIRST..FIRST + COLOURS.len()].copy_from_slice(&COLOURS);
    colours
}

/// The index of the pixel of luma `y` and chroma `u` and `v` at `column`
/// and `row` from the picture's top left.
pub fn index(y: u8, u: u8, v: u8, column: usize, row: usize) -> u8 {
    let (r, c) = (row & 3, column & 3);
    let level = ((i16::from(y) - LUMA[r][c] + 15).div_euclid(15)).clamp(0, 12);
    let lattice = |value: u8, add: i16| (i16::from(value) + add - 126).div_euclid(32).clamp(-2, 2);
    let u = lattice(u, CHROMA_U[r][c]);
    let v = lattice(v, CHROMA_V[r][c]);
    let point = GRID[(u + 2) as usize][(v + 2) as usize];
    (FIRST as i16 + 13 * i16::from(point) + level) as u8
}

/// The picture as the codec gives it in 8 bits with its own palette,
/// `zoom` times its size: rows of indexes from the top. At twice the
/// size (ICM_DECOMPRESSEX to an output twice as large) each pixel's Y,
/// U and V make four pixels of the output, and each of those takes the
/// dither of its own place in the output, as IR41.DLL does: of the
/// 2,918,400 pixels of 19 frames at 640 by 240 all but 24 come out so,
/// the 24 at the lattice points without a colour.
pub fn indexes(picture: &Picture, zoom: usize) -> Vec<u8> {
    let (w, h, z) = (picture.width, picture.height, zoom.max(1));
    let cw = picture.chroma_width();
    let mut out = vec![0; w * z * h * z];
    for (row, line) in out.chunks_exact_mut((w * z).max(1)).enumerate() {
        let r = row / z;
        let ys = &picture.y[r * w..][..w];
        let us = &picture.u[r / 4 * cw..][..cw];
        let vs = &picture.v[r / 4 * cw..][..cw];
        for (column, o) in line.iter_mut().enumerate() {
            let c = column / z;
            *o = index(ys[c], us[c / 4], vs[c / 4], column, row);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every chroma point's colours are 13 levels of luma, darkest first.
    #[test]
    fn each_points_colours_get_brighter() {
        for point in COLOURS.chunks_exact(13) {
            let luma = |c: u32| 299 * (c & 0xFF) + 587 * (c >> 8 & 0xFF) + 114 * (c >> 16);
            assert!(point.windows(2).all(|w| luma(w[0]) < luma(w[1])));
        }
    }

    /// The grid gives each point with a colour its own.
    #[test]
    fn the_grid_keeps_the_points() {
        for (point, &(u, v)) in POINTS.iter().enumerate() {
            assert_eq!(GRID[(u + 2) as usize][(v + 2) as usize] as usize, point);
        }
    }

    /// Pixels the codec gave on Windows 95, from Civilization II's
    /// OPENING.AVI: black takes the darkest grey, a dark grey the two
    /// darkest by the pixel's place, and a blue (U far above 128) a blue
    /// point.
    #[test]
    fn pixels_take_the_codecs_indexes() {
        // Black (Y 16, no chroma): level 0 of grey everywhere.
        for row in 0..4 {
            for column in 0..4 {
                assert_eq!(index(16, 128, 128, column, row), 10);
            }
        }
        // Y 47 is level 1 where the threshold is 41 or 45, else level 0.
        let levels: Vec<u8> = (0..4).map(|c| index(47, 128, 128, c, 0)).collect();
        assert_eq!(levels, [11, 10, 11, 10]);
        // U far above 128, V at it: the point (2, 0), the 13th, blue.
        assert_eq!(index(130, 200, 128, 3, 0), 10 + 13 * 12 + 6);
        // The palette has the codec's colours between the statics.
        let palette = palette_8(&[0x0012_3456; 20]);
        assert_eq!(palette[9], 0x0012_3456);
        assert_eq!(palette[10], 0x0019_1919);
        assert_eq!(palette[243], 0x00FF_ECB7);
        assert_eq!(palette[10 + 13 * 12 + 6], 0x00FF_6982);
        assert_eq!(palette[244], 0);
        assert_eq!(palette[246], 0x0012_3456);
    }

    /// A picture's indexes go by each pixel's place from the top left:
    /// a picture of Y 47 all over has the dither's pattern of levels 0
    /// and 1, the same in each block of 4 by 4, and its chroma comes from
    /// the planes at a quarter of the size.
    #[test]
    fn a_pictures_indexes_follow_the_matrix() {
        let picture = Picture {
            width: 8,
            height: 4,
            y: vec![47; 32],
            u: vec![128, 200],
            v: vec![128, 128],
        };
        let out = indexes(&picture, 1);
        for row in 0..4 {
            for column in 0..8 {
                let u = if column < 4 { 128 } else { 200 };
                assert_eq!(out[row * 8 + column], index(47, u, 128, column, row));
            }
        }
        assert_eq!(&out[..4], [11, 10, 11, 10]);
        assert_eq!(out[4], 10 + 13 * 12 + 1);
    }

    /// At twice the size each of a pixel's four takes the dither of its
    /// own place in the output: a picture of Y 47 all over has the same
    /// pattern of levels as at its own size, over twice the area.
    #[test]
    fn a_zoomed_pictures_indexes_follow_the_matrix_of_the_output() {
        let picture = Picture {
            width: 4,
            height: 4,
            y: vec![47; 16],
            u: vec![128],
            v: vec![128],
        };
        let out = indexes(&picture, 2);
        assert_eq!(out.len(), 64);
        for row in 0..8 {
            for column in 0..8 {
                assert_eq!(out[row * 8 + column], index(47, 128, 128, column, row));
            }
        }
        assert_eq!(&out[..8], [11, 10, 11, 10, 11, 10, 11, 10]);
    }
}

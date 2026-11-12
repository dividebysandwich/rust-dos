//! Indeo 5 per-band tile geometry.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/02-gop-and-band-layer.md`
//! §4.1 (tile-count derivation) and §4.2 (per-tile record layout).
//!
//! After the band header, each coded band is partitioned into a grid of
//! rectangular tiles; the inverse Slant transform (`spec/06 §2`)
//! processes one tile at a time. The band-header exit path populates a
//! per-tile record array (`spec/02 §4`) before any per-tile coefficient
//! data is decoded. This module materialises the **structural** tile
//! grid — the per-tile `(x, y, width, height)` rectangles, including the
//! `spec/02 §4.2` last-column / bottom-row remainder special case — from
//! the band dimensions and the per-axis tile counts. The per-tile
//! coefficient data-size header (`spec/02 §4.3`) and the coefficient
//! stream itself are the gated `spec/03+`/`spec/05+` scope and are not
//! parsed here.
//!
//! ## Tile-count derivation (`spec/02 §4.1`)
//!
//! Tile counts flow from the GOP slice-size (`spec/02 §1.4`/`§1.6`) and
//! the band's downsampling: with the default `slice_size_id = 0`
//! (64×64 slices) the per-axis tile count is `ceil(picture_dim /
//! slice_dim)` — e.g. a 352×288 picture gives `ceil(352/64) = 6`
//! columns and `ceil(288/64) = 5` rows of tiles, applied uniformly to
//! every band of the plane (the band's own width/height set the tile
//! *pixel* size, the picture/slice ratio sets the *count*).
//!
//! ## Per-tile dimensions (`spec/02 §4.2`)
//!
//! The regular tile width is `band_width / tile_count_x` (truncating);
//! the **last** column tile gets the remainder `band_width -
//! regular_width · (tile_count_x - 1)` (`spec/02 §4.2`,
//! `IR50_32.DLL!0x1001e7b2`). The same applies on the vertical axis.

/// Ceiling-divide `a / b` for non-zero `b` (`spec/02 §4.1` `ceil`).
#[inline]
fn ceil_div(a: u32, b: u32) -> u32 {
    debug_assert!(b != 0);
    a.div_ceil(b)
}

/// Spec/02 §4.1 — derive the per-axis tile count from a picture
/// dimension and the GOP slice dimension (`ceil(picture / slice)`).
///
/// Returns at least `1` (a degenerate `picture_dim == 0` still yields a
/// single empty tile row/column so the grid is non-empty). `slice_dim`
/// must be non-zero (the GOP `slice_size_id` always maps to a positive
/// slice size, `spec/02 §1.4`).
pub fn tile_count(picture_dim: u32, slice_dim: u32) -> u32 {
    if slice_dim == 0 {
        return 1;
    }
    ceil_div(picture_dim, slice_dim).max(1)
}

/// A single tile's structural rectangle within a band (`spec/02 §4.2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile {
    /// Column index in the tile grid (`0..tile_count_x`).
    pub col: u32,
    /// Row index in the tile grid (`0..tile_count_y`).
    pub row: u32,
    /// Left edge (pixels) of the tile within the band.
    pub x: u32,
    /// Top edge (pixels) of the tile within the band.
    pub y: u32,
    /// Tile width in pixels (`[tile+0x00]`).
    pub width: u32,
    /// Tile height in pixels (`[tile+0x04]`).
    pub height: u32,
}

/// The full per-band tile grid (`spec/02 §4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileGrid {
    /// Number of tile columns (`[band+0xd8]`).
    pub count_x: u32,
    /// Number of tile rows (`[band+0xdc]`).
    pub count_y: u32,
    /// The tiles in raster order `(row, col)` (`spec/02 §4.4`).
    pub tiles: Vec<Tile>,
}

impl TileGrid {
    /// Build the tile grid for a band of `band_width × band_height`
    /// partitioned into `count_x × count_y` tiles (`spec/02 §4.2`).
    ///
    /// Regular tiles get `band_width / count_x` × `band_height /
    /// count_y`; the last column / bottom row carry the remainder so the
    /// tiles tile the band exactly (`spec/02 §4.2`). `count_x` /
    /// `count_y` are clamped to `>= 1`.
    pub fn build(band_width: u32, band_height: u32, count_x: u32, count_y: u32) -> Self {
        let count_x = count_x.max(1);
        let count_y = count_y.max(1);
        let reg_w = band_width / count_x;
        let reg_h = band_height / count_y;

        let mut tiles = Vec::with_capacity((count_x * count_y) as usize);
        // Raster order: rows outer, columns inner (spec/02 §4.4).
        let mut y = 0u32;
        for row in 0..count_y {
            let height = if row == count_y - 1 {
                // Bottom-row remainder (spec/02 §4.2).
                band_height - reg_h * (count_y - 1)
            } else {
                reg_h
            };
            let mut x = 0u32;
            for col in 0..count_x {
                let width = if col == count_x - 1 {
                    band_width - reg_w * (count_x - 1)
                } else {
                    reg_w
                };
                tiles.push(Tile {
                    col,
                    row,
                    x,
                    y,
                    width,
                    height,
                });
                x += width;
            }
            y += height;
        }

        TileGrid {
            count_x,
            count_y,
            tiles,
        }
    }

    /// The tile at grid position `(col, row)`, if in range.
    pub fn tile(&self, col: u32, row: u32) -> Option<&Tile> {
        if col >= self.count_x || row >= self.count_y {
            return None;
        }
        self.tiles.get((row * self.count_x + col) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_count_spec_example() {
        // spec/02 §4.1: 352x288 picture, 64x64 slices -> 6x5 tiles.
        assert_eq!(tile_count(352, 64), 6);
        assert_eq!(tile_count(288, 64), 5);
    }

    #[test]
    fn tile_count_exact_division() {
        assert_eq!(tile_count(128, 64), 2);
        assert_eq!(tile_count(64, 64), 1);
    }

    #[test]
    fn tile_count_floors_at_one() {
        assert_eq!(tile_count(0, 64), 1);
        assert_eq!(tile_count(10, 0), 1);
    }

    #[test]
    fn grid_tiles_cover_band_exactly() {
        // 88x72 band into 6x5 tiles. Regular 14x14, last col 88-14*5=18,
        // last row 72-14*4=16. Verify total coverage = band area and no
        // gaps/overlaps along each axis.
        let g = TileGrid::build(88, 72, 6, 5);
        assert_eq!(g.tiles.len(), 30);
        // Row 0 widths sum to band width.
        let row0_w: u32 = g.tiles.iter().filter(|t| t.row == 0).map(|t| t.width).sum();
        assert_eq!(row0_w, 88);
        // Column 0 heights sum to band height.
        let col0_h: u32 = g
            .tiles
            .iter()
            .filter(|t| t.col == 0)
            .map(|t| t.height)
            .sum();
        assert_eq!(col0_h, 72);
        // Last column width is the remainder.
        let last = g.tile(5, 0).unwrap();
        assert_eq!(last.width, 88 - (88 / 6) * 5);
        // Last row height is the remainder.
        let bottom = g.tile(0, 4).unwrap();
        assert_eq!(bottom.height, 72 - (72 / 5) * 4);
    }

    #[test]
    fn grid_tile_origins_are_cumulative() {
        let g = TileGrid::build(100, 100, 4, 4);
        // x of each tile equals sum of preceding widths in its row.
        for row in 0..4 {
            let mut acc = 0;
            for col in 0..4 {
                let t = g.tile(col, row).unwrap();
                assert_eq!(t.x, acc, "row {row} col {col}");
                acc += t.width;
            }
            assert_eq!(acc, 100);
        }
    }

    #[test]
    fn grid_exact_division_uniform_tiles() {
        // 64x64 into 4x4 -> all tiles 16x16.
        let g = TileGrid::build(64, 64, 4, 4);
        assert!(g.tiles.iter().all(|t| t.width == 16 && t.height == 16));
    }

    #[test]
    fn grid_single_tile() {
        let g = TileGrid::build(40, 30, 1, 1);
        assert_eq!(g.tiles.len(), 1);
        let t = g.tile(0, 0).unwrap();
        assert_eq!((t.x, t.y, t.width, t.height), (0, 0, 40, 30));
    }

    #[test]
    fn grid_raster_order() {
        let g = TileGrid::build(60, 40, 3, 2);
        // tiles[i] should be in (row, col) raster order.
        let order: Vec<(u32, u32)> = g.tiles.iter().map(|t| (t.row, t.col)).collect();
        assert_eq!(order, vec![(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2)]);
    }

    #[test]
    fn grid_clamps_zero_counts() {
        let g = TileGrid::build(40, 40, 0, 0);
        assert_eq!((g.count_x, g.count_y), (1, 1));
        assert_eq!(g.tiles.len(), 1);
    }

    #[test]
    fn tile_out_of_range_is_none() {
        let g = TileGrid::build(40, 40, 2, 2);
        assert!(g.tile(2, 0).is_none());
        assert!(g.tile(0, 2).is_none());
        assert!(g.tile(1, 1).is_some());
    }
}

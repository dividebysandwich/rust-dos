//! Indeo 5 per-tile macroblock grid (`spec/03 §3`).
//!
//! Spec source: `docs/video/indeo/indeo5/spec/03-tile-and-macroblock-layer.md`
//! §3 (block-kernel entry `IR50_32.DLL!0x1001f257`-`0x1001f288`).
//!
//! Each coded tile is decoded as a regular grid of macroblocks in
//! raster order. The per-band `(mb_size, blk_size)` pair comes from
//! the GOP `band_info` array (`spec/02 §1.7`, [`super::MB_SIZE_TABLE`]
//! / [`super::BLK_SIZE_TABLE`]); a macroblock contains either one
//! block (`mb_size == blk_size`) or four blocks in raster order
//! (`spec/03 §3.3`: block 0 = top-left, 1 = top-right, 2 =
//! bottom-left, 3 = bottom-right). Last-column / bottom-row
//! macroblocks are clamped to the tile boundary; the clamped-off
//! region is zero-padded for transform purposes (`spec/03 §3.2`).

/// Spec/03 §3.3 — the three 16-element four-block coordinate tables at
/// `IR50_32.DLL!.rdata 0x10088c38` (block-x-within-mb).
pub const FOUR_BLOCK_X: [u8; 16] = [0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1];

/// Spec/03 §3.3 — `.rdata 0x10088c48` (block-y-within-mb).
pub const FOUR_BLOCK_Y: [u8; 16] = [0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1];

/// Spec/03 §3.3 — `.rdata 0x10088c58` (mb-z / band-index component).
pub const FOUR_BLOCK_Z: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1];

/// Spec/03 §3.4 — per-frame-type block-stride table A at
/// `.rdata 0x10088c08` (indexed by `frame_type`, `[frame+0xb8]`).
pub const BLK_STRIDE_TABLE_A: [u32; 4] = [0x0000_0000, 0x0000_0002, 0x0000_0002, 0x0000_0002];

/// Spec/03 §3.4 — per-frame-type block-stride table B at
/// `.rdata 0x10088c18`.
pub const BLK_STRIDE_TABLE_B: [u32; 4] = [0x0000_0000, 0x0000_0101, 0x0000_0101, 0x0000_0101];

/// Spec/03 §3.4 — the per-band-index packed-flags table at
/// `.rdata 0x10088bf0` (8 DWORDs, consumed at `0x1001f227`).
pub const BAND_INDEX_FLAGS: [u32; 8] = [
    0x0100_0002,
    0x0100_0000,
    0x0100_0000,
    0x0100_0000,
    0,
    0,
    0,
    0x0000_0002,
];

/// Spec/03 §3.5 — the partial-MB inner-loop row counts selected by the
/// `mb_size == blk_size` test at `IR50_32.DLL!0x1001f264`-`0x1001f26f`:
/// the "MB matches block" case runs a 2-row inner loop, the four-block
/// case a 5-row loop (the difference is the zero-padding rows the
/// inverse transform requires for partial MBs).
pub fn partial_mb_pad_rows(mb_size: u32, blk_size: u32) -> u32 {
    if mb_size == blk_size { 2 } else { 5 }
}

/// Spec/03 §3.1 — blocks per macroblock: `(mb_size / blk_size)^2`
/// (1 or 4 for the in-spec size pairs).
pub fn blocks_per_mb(mb_size: u32, blk_size: u32) -> u32 {
    let per_row = mb_size / blk_size;
    per_row * per_row
}

/// Spec/03 §3.5 — the MB stride (`[esp+0x90]`):
/// `blk_size * blocks_per_mb_row`.
pub fn mb_stride(mb_size: u32, blk_size: u32) -> u32 {
    blk_size * (mb_size / blk_size)
}

/// One macroblock of the per-tile grid, in `spec/03 §3.3` raster
/// order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Macroblock {
    /// Grid column (0-based).
    pub mb_x: u32,
    /// Grid row (0-based).
    pub mb_y: u32,
    /// Pixel x of the MB's top-left corner within the tile.
    pub x: u32,
    /// Pixel y of the MB's top-left corner within the tile.
    pub y: u32,
    /// Coded width — clamped to the tile boundary for the last
    /// column (`spec/03 §3.2`).
    pub width: u32,
    /// Coded height — clamped for the bottom row.
    pub height: u32,
}

impl Macroblock {
    /// `true` when the MB is clamped by the tile boundary (a partial
    /// MB, zero-padded to `mb_size` for transform purposes,
    /// `spec/03 §3.2`).
    pub fn is_partial(&self, mb_size: u32) -> bool {
        self.width < mb_size || self.height < mb_size
    }
}

/// One block within a macroblock (`spec/03 §3.3` raster order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MbBlock {
    /// Block index within the MB (`0..blocks_per_mb`).
    pub block_idx: u32,
    /// Pixel x of the block's top-left corner within the tile.
    pub x: u32,
    /// Pixel y of the block's top-left corner within the tile.
    pub y: u32,
}

/// Spec/03 §3.2 — the per-tile macroblock grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MbGrid {
    /// Per-band MB side in pixels (8 or 16; 4 for the unusual
    /// single-4×4 case).
    pub mb_size: u32,
    /// Per-band block side in pixels (4 or 8).
    pub blk_size: u32,
    /// `ceil(tile_width / mb_size)` (`[esp+0x88]`).
    pub mb_count_x: u32,
    /// `ceil(tile_height / mb_size)` (`[esp+0x8c]`).
    pub mb_count_y: u32,
    /// Tile width in pixels.
    pub tile_width: u32,
    /// Tile height in pixels.
    pub tile_height: u32,
}

impl MbGrid {
    /// Derive the grid for one tile (`spec/03 §3.2`).
    pub fn build(tile_width: u32, tile_height: u32, mb_size: u32, blk_size: u32) -> Self {
        MbGrid {
            mb_size,
            blk_size,
            mb_count_x: tile_width.div_ceil(mb_size),
            mb_count_y: tile_height.div_ceil(mb_size),
            tile_width,
            tile_height,
        }
    }

    /// Total macroblocks in the tile (the inner-tile loop trip count).
    pub fn mb_count(&self) -> u32 {
        self.mb_count_x * self.mb_count_y
    }

    /// Blocks per macroblock (`spec/03 §3.1`, `[band+0x24]`).
    pub fn blocks_per_mb(&self) -> u32 {
        blocks_per_mb(self.mb_size, self.blk_size)
    }

    /// The macroblock at grid position `(mb_x, mb_y)`, with the
    /// last-column / bottom-row clamp (`spec/03 §3.2`).
    pub fn macroblock(&self, mb_x: u32, mb_y: u32) -> Option<Macroblock> {
        if mb_x >= self.mb_count_x || mb_y >= self.mb_count_y {
            return None;
        }
        let x = mb_x * self.mb_size;
        let y = mb_y * self.mb_size;
        Some(Macroblock {
            mb_x,
            mb_y,
            x,
            y,
            width: self.mb_size.min(self.tile_width - x),
            height: self.mb_size.min(self.tile_height - y),
        })
    }

    /// Iterate the tile's macroblocks in `spec/03 §3.3` raster order
    /// (row-major, top-to-bottom, left-to-right).
    pub fn iter(&self) -> impl Iterator<Item = Macroblock> + '_ {
        (0..self.mb_count_y).flat_map(move |mb_y| {
            (0..self.mb_count_x).map(move |mb_x| {
                self.macroblock(mb_x, mb_y)
                    .expect("in-range grid coordinates")
            })
        })
    }

    /// The blocks of one macroblock in `spec/03 §3.3` raster order
    /// (block 0 = top-left, 1 = top-right, 2 = bottom-left, 3 =
    /// bottom-right; a single block for `mb_size == blk_size`).
    pub fn blocks(&self, mb: &Macroblock) -> Vec<MbBlock> {
        let per_row = self.mb_size / self.blk_size;
        (0..self.blocks_per_mb())
            .map(|block_idx| {
                let bx = block_idx % per_row;
                let by = block_idx / per_row;
                MbBlock {
                    block_idx,
                    x: mb.x + bx * self.blk_size,
                    y: mb.y + by * self.blk_size,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_block_tables_encode_quadrants() {
        // spec/03 §3.3 — index pairs (2k, 2k+1) share coordinates; the
        // x/y tables walk TL, TR, BL, BR per z-half.
        for k in 0..8 {
            assert_eq!(FOUR_BLOCK_X[2 * k], FOUR_BLOCK_X[2 * k + 1]);
            assert_eq!(FOUR_BLOCK_Y[2 * k], FOUR_BLOCK_Y[2 * k + 1]);
            assert_eq!(FOUR_BLOCK_Z[2 * k], FOUR_BLOCK_Z[2 * k + 1]);
        }
        // First z-half quadrant walk: (0,0) (1,0) (0,1) (1,1).
        assert_eq!(
            (FOUR_BLOCK_X[0], FOUR_BLOCK_Y[0], FOUR_BLOCK_Z[0]),
            (0, 0, 0)
        );
        assert_eq!(
            (FOUR_BLOCK_X[2], FOUR_BLOCK_Y[2], FOUR_BLOCK_Z[2]),
            (1, 0, 0)
        );
        assert_eq!(
            (FOUR_BLOCK_X[4], FOUR_BLOCK_Y[4], FOUR_BLOCK_Z[4]),
            (0, 1, 0)
        );
        assert_eq!(
            (FOUR_BLOCK_X[6], FOUR_BLOCK_Y[6], FOUR_BLOCK_Z[6]),
            (1, 1, 0)
        );
        // Second half repeats with z = 1.
        assert_eq!(
            (FOUR_BLOCK_X[8], FOUR_BLOCK_Y[8], FOUR_BLOCK_Z[8]),
            (0, 0, 1)
        );
        assert_eq!(
            (FOUR_BLOCK_X[14], FOUR_BLOCK_Y[14], FOUR_BLOCK_Z[14]),
            (1, 1, 1)
        );
    }

    #[test]
    fn stride_tables_zero_for_intra() {
        // spec/03 §3.4 — INTRA has no inter prediction; INTER and the
        // droppable types share the non-trivial strides.
        assert_eq!(BLK_STRIDE_TABLE_A[0], 0);
        assert_eq!(BLK_STRIDE_TABLE_B[0], 0);
        for ft in 1..4 {
            assert_eq!(BLK_STRIDE_TABLE_A[ft], 2);
            assert_eq!(BLK_STRIDE_TABLE_B[ft], 0x101);
        }
        assert_eq!(BAND_INDEX_FLAGS[0], 0x0100_0002);
        assert_eq!(BAND_INDEX_FLAGS[7], 0x0000_0002);
    }

    #[test]
    fn blocks_per_mb_size_pairs() {
        // spec/03 §3.1 table.
        assert_eq!(blocks_per_mb(16, 8), 4);
        assert_eq!(blocks_per_mb(8, 8), 1);
        assert_eq!(blocks_per_mb(8, 4), 4);
        assert_eq!(blocks_per_mb(4, 4), 1);
    }

    #[test]
    fn mb_stride_values() {
        // spec/03 §3.5.
        assert_eq!(mb_stride(16, 8), 16);
        assert_eq!(mb_stride(8, 4), 8);
        assert_eq!(mb_stride(8, 8), 8);
    }

    #[test]
    fn pad_row_selector() {
        // spec/03 §3.5 — 2-row loop when MB matches block, else 5.
        assert_eq!(partial_mb_pad_rows(8, 8), 2);
        assert_eq!(partial_mb_pad_rows(16, 8), 5);
        assert_eq!(partial_mb_pad_rows(8, 4), 5);
    }

    #[test]
    fn grid_counts_ceil() {
        // 64x64 tile of 16-px MBs -> 4x4; 66x64 -> 5x4 (ceil).
        let g = MbGrid::build(64, 64, 16, 8);
        assert_eq!((g.mb_count_x, g.mb_count_y), (4, 4));
        assert_eq!(g.mb_count(), 16);
        let g = MbGrid::build(66, 64, 16, 8);
        assert_eq!((g.mb_count_x, g.mb_count_y), (5, 4));
    }

    #[test]
    fn raster_iteration_and_clamp() {
        // 40x24 tile of 16-px MBs -> 3x2 grid; last column is 8 wide,
        // bottom row 8 tall (spec/03 §3.2 clamp).
        let g = MbGrid::build(40, 24, 16, 8);
        let mbs: Vec<Macroblock> = g.iter().collect();
        assert_eq!(mbs.len(), 6);
        // Raster order: row 0 first.
        assert_eq!((mbs[0].mb_x, mbs[0].mb_y), (0, 0));
        assert_eq!((mbs[1].mb_x, mbs[1].mb_y), (1, 0));
        assert_eq!((mbs[2].mb_x, mbs[2].mb_y), (2, 0));
        assert_eq!((mbs[3].mb_x, mbs[3].mb_y), (0, 1));
        // Last-column clamp.
        assert_eq!(mbs[2].width, 8);
        assert!(mbs[2].is_partial(16));
        // Bottom-row clamp.
        assert_eq!(mbs[3].height, 8);
        // Interior MB is full.
        assert_eq!((mbs[0].width, mbs[0].height), (16, 16));
        assert!(!mbs[0].is_partial(16));
    }

    #[test]
    fn four_blocks_raster_within_mb() {
        let g = MbGrid::build(64, 64, 16, 8);
        let mb = g.macroblock(1, 1).unwrap();
        let blocks = g.blocks(&mb);
        assert_eq!(blocks.len(), 4);
        // Block 0 TL, 1 TR, 2 BL, 3 BR (spec/03 §3.3).
        assert_eq!((blocks[0].x, blocks[0].y), (16, 16));
        assert_eq!((blocks[1].x, blocks[1].y), (24, 16));
        assert_eq!((blocks[2].x, blocks[2].y), (16, 24));
        assert_eq!((blocks[3].x, blocks[3].y), (24, 24));
    }

    #[test]
    fn single_block_mb() {
        let g = MbGrid::build(32, 32, 8, 8);
        assert_eq!(g.blocks_per_mb(), 1);
        let mb = g.macroblock(2, 3).unwrap();
        let blocks = g.blocks(&mb);
        assert_eq!(blocks.len(), 1);
        assert_eq!((blocks[0].x, blocks[0].y), (16, 24));
    }

    #[test]
    fn out_of_range_mb_rejected() {
        let g = MbGrid::build(64, 64, 16, 8);
        assert!(g.macroblock(4, 0).is_none());
        assert!(g.macroblock(0, 4).is_none());
    }
}

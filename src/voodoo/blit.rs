//! The Voodoo2's 2D blitter: screen-to-screen copies, rectangle fills,
//! SGRAM page fills (how Glide 3 clears whole buffers) and CPU-to-screen
//! copies fed through bltData, with the source and destination chroma
//! range tests choosing among four raster operations. From 86Box's
//! `voodoo_v2_blit_start` and `voodoo_v2_blit_data`
//! (vid_voodoo_blitter.c), with the Voodoo2 specification's sizes (a
//! fill is |bltSizeX| + 1 pixels wide, as a copy is) and its byte
//! swizzling and transparent monochrome bits.

use super::regs::*;
use super::workers::{Job, Word};
use super::{NONE, Voodoo};

/// A CPU-to-screen copy under way: where its next pixel goes, how many
/// rows are left and the destination's row stride in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Blit {
    pub active: bool,
    pub x: i32,
    pub y: i32,
    pub column: i32,
    pub rows: i32,
}

crate::state_fields!(Blit { active, x, y, column, rows });

/// bltCommand's fields.
const COMMAND: u32 = 7;
const SCREEN_TO_SCREEN: u32 = 0;
const CPU_TO_SCREEN: u32 = 1;
const RECT_FILL: u32 = 2;
const SGRAM_FILL: u32 = 3;
const SRC_CHROMA: u32 = 1 << 10;
const DST_CHROMA: u32 = 1 << 12;
const SRC_TILED: u32 = 1 << 14;
const DST_TILED: u32 = 1 << 15;
const CLIPPING: u32 = 1 << 16;
const TRANSPARENT: u32 = 1 << 17;

/// Where a Voodoo2's SGRAM fill writes. The card keeps its memory in
/// tiles of 32 by 32 pixels, two side by side in each 4 KB page, so that
/// a page holds 32 rows of 64 pixels, each row 16 of the fill's 64-bit
/// columns (Glide's `_grBufferClear` for the CVG; 3dfx's Direct3D driver
/// clears the last 24 rows of 3DMark 2000's 800x600 depth buffer with
/// columns 0 to 383 of each page in a row of tiles). Each buffer starts
/// on a page and its pages go along its rows of tiles. The memory here
/// keeps each buffer in rows of `rowpixels`, which hold the same pixels
/// as the card's rows of tiles, in another order.
struct SgramTiles {
    /// Pages to a row of tiles: `rowpixels` / 64.
    page_row: u32,
    rowpixels: u32,
    /// Pages to a buffer (fbiInit2 bits 19:11).
    pages: u32,
    /// The buffers' pictures' height.
    height: u32,
    /// The colour buffers' and the auxiliary buffer's first pages.
    buffers: [Option<u32>; 4],
}

impl SgramTiles {
    /// The buffer that page `p` is in, as its first page, and which of
    /// its pages `p` is.
    fn find(&self, p: u32) -> Option<(u32, u32)> {
        let mut buffers = self.buffers.iter().flatten();
        let base = *buffers.find(|&&b| b <= p && p < b + self.pages)?;
        Some((base, p - base))
    }

    /// The first of the four words of column `c` of page `p`. Pages
    /// outside the buffers are taken as rows of 2,048 words.
    fn word(&self, p: u32, c: u32) -> usize {
        let Some((base, rel)) = self.find(p) else {
            return p as usize * 2048 + 4 * c as usize;
        };
        let y = rel / self.page_row * 32 + c / 16;
        let x = rel % self.page_row * 64 + c % 16 * 4;
        base as usize * 2048 + (y * self.rowpixels + x) as usize
    }
}

/// Rows `top..bottom` of the buffer at word offset `base`: (base, top,
/// bottom).
type BufferRows = (usize, u32, u32);

/// A 5-6-5 colour as 8-8-8.
fn rgb888(color: u16) -> u32 {
    let (r, g, b) = (
        (color >> 11) as u32,
        ((color >> 5) & 0x3F) as u32,
        (color & 0x1F) as u32,
    );
    ((r << 3 | r >> 2) << 16) | ((g << 2 | g >> 4) << 8) | (b << 3 | b >> 2)
}

/// A 12-bit signed field.
fn s12(v: u32) -> i32 {
    ((v << 20) as i32) >> 20
}

/// Raster operation `rop` of source `s` on destination `d`.
fn mix(rop: u32, s: u16, d: u16) -> u16 {
    match rop & 15 {
        0x0 => 0,
        0x1 => !(s | d),
        0x2 => !s & d,
        0x3 => !s,
        0x4 => s & !d,
        0x5 => !d,
        0x6 => s ^ d,
        0x7 => !(s & d),
        0x8 => s & d,
        0x9 => !(s ^ d),
        0xA => d,
        0xB => !s | d,
        0xC => s,
        0xD => s | !d,
        0xE => s | d,
        _ => 0xFFFF,
    }
}

/// Whether every 5-6-5 pixel is in a chroma range register's range
/// (true) or none is (false): None if it depends on the pixel.
fn chroma_constant(range: u32) -> Option<bool> {
    let parts = |v: u32| [v >> 11 & 0x1F, v >> 5 & 0x3F, v & 0x1F];
    let (low, high) = (parts(range & 0xFFFF), parts(range >> 16));
    if low.iter().zip(&high).any(|(l, h)| l > h) {
        Some(false)
    } else if low == [0; 3] && high == [0x1F, 0x3F, 0x1F] {
        Some(true)
    } else {
        None
    }
}

/// The bytes of rows `y0` to `y1` (either way round) of a blit's side at
/// byte `base` with `stride` bytes a row, to the last column the blitter
/// reaches in the last, whatever the stride.
fn blt_rows(base: u32, stride: u32, y0: i32, y1: i32) -> (u32, u32) {
    let (top, last) = (y0.min(y1).max(0) as u32, y0.max(y1).max(0) as u32);
    let end = base
        .saturating_add(last.saturating_mul(stride))
        .saturating_add(2 * 2048);
    (base.saturating_add(top.saturating_mul(stride)), end)
}

/// Whether a 5-6-5 pixel is within a chroma range register's minimum
/// (bits 15:0) and maximum (bits 31:16).
fn in_range(range: u32, pixel: u16) -> bool {
    let p = pixel as u32;
    let parts = |v: u32| (v >> 11 & 0x1F, v >> 5 & 0x3F, v & 0x1F);
    let ((r, g, b), (r0, g0, b0), (r1, g1, b1)) = (parts(p), parts(range & 0xFFFF), parts(range >> 16));
    (r0..=r1).contains(&r) && (g0..=g1).contains(&g) && (b0..=b1).contains(&b)
}

impl Voodoo {
    fn blt_command(&self) -> u32 {
        self.reg[BLT_COMMAND]
    }

    /// A side's base byte address and row stride: linear, or 32x32 tiles
    /// (a tile row of `stride & 3Fh` tiles, at 4 KB pages).
    fn blt_layout(&self, base: u32, stride: u32, tiled: bool) -> (u32, u32) {
        if tiled { ((base & 0x3FF) << 12, (stride & 0x3F) * 32 * 2) } else { (base & 0x3F_FFF8, stride & 0xFF8) }
    }

    fn blt_src(&self) -> (u32, u32) {
        let tiled = self.blt_command() & SRC_TILED != 0;
        self.blt_layout(self.reg[BLT_SRC_BASE_ADDR], self.reg[BLT_XY_STRIDES] & 0xFFF, tiled)
    }

    fn blt_dst(&self) -> (u32, u32) {
        let tiled = self.blt_command() & DST_TILED != 0;
        self.blt_layout(self.reg[BLT_DST_BASE_ADDR], (self.reg[BLT_XY_STRIDES] >> 16) & 0xFFF, tiled)
    }

    fn blt_clipped(&self, x: i32, y: i32) -> bool {
        if self.blt_command() & CLIPPING == 0 {
            return false;
        }
        let (cx, cy) = (self.reg[BLT_CLIP_X], self.reg[BLT_CLIP_Y]);
        let (left, right, low, high) = (((cx >> 16) & 0xFFF) as i32, (cx & 0xFFF) as i32, ((cy >> 16) & 0xFFF) as i32, (cy & 0xFFF) as i32);
        x < left || x >= right || y < low || y >= high
    }

    /// The pixel at byte address `addr`, if it is in the memory.
    fn blt_get(&self, addr: u32) -> u16 {
        let at = (addr >> 1) as usize;
        if at < self.fbi.ram.len() { self.fbi.ram.get(at) } else { 0 }
    }

    /// Write a pixel through the raster operations: the ROP the source
    /// and destination chroma range tests select.
    fn blt_put(&mut self, addr: u32, src: u16) {
        let at = (addr >> 1) as usize;
        if at >= self.fbi.ram.len() {
            return;
        }
        let dst = self.fbi.ram.get(at);
        let value = mix(self.blt_rop(src, dst), src, dst);
        self.fbi.ram.set(at, value);
        self.blt_mirror(at, value);
    }

    /// The ROP the source and destination chroma range tests select for
    /// `src` over `dst`.
    fn blt_rop(&self, src: u16, dst: u16) -> u32 {
        let command = self.blt_command();
        let mut rop = 0;
        if command & DST_CHROMA != 0 && in_range(self.reg[BLT_DST_CHROMA_RANGE], dst) {
            rop |= 1;
        }
        if command & SRC_CHROMA != 0 && in_range(self.reg[BLT_SRC_CHROMA_RANGE], src) {
            rop |= 2;
        }
        (self.reg[BLT_ROP] >> (4 * rop)) & 15
    }

    /// Tell the OpenGL renderer's recording about a pixel the blitter
    /// wrote, if it is in a buffer's picture.
    fn blt_mirror(&mut self, at: usize, value: u16) {
        if !self.recording() {
            return;
        }
        let fbi = &self.fbi;
        let row = fbi.rowpixels.max(1) as usize;
        let colour = fbi.rgboffs.iter().filter(|&&o| o != NONE).map(|&o| (Some(o as usize / 2), o as usize / 2));
        let aux = (fbi.auxoffs != NONE).then_some((None, fbi.auxoffs as usize / 2));
        let found = colour.chain(aux).filter(|&(_, base)| at >= base).min_by_key(|&(_, base)| at - base);
        let Some((dest, base)) = found else { return };
        let (x, y) = (((at - base) % row) as u32, ((at - base) / row) as u32);
        if x < fbi.width && y < fbi.height {
            let width = fbi.width;
            if let Some(mirror) = self.mirror.get_mut() {
                mirror.pixel(dest.map(|d| d as u32), x, y, value, width);
            }
        }
    }

    /// bit 31 of bltCommand, bltDstXY or bltSize: run the blit, or for a
    /// CPU-to-screen one get ready for its data.
    pub(crate) fn blit_start(&mut self) {
        let command = self.blt_command();
        if command & COMMAND == SGRAM_FILL && self.sgram_fill_rows() {
            self.display_dirty = true;
            return;
        }
        match command & COMMAND {
            CPU_TO_SCREEN => {
                // Its pixels may wait behind what is drawn
                // (`blt_queue_dest`); blit_data waits for the drawing
                // where they can't.
                let xy = self.reg[BLT_DST_XY];
                let (x, y) = ((xy & 0x7FF) as i32, ((xy >> 16) & 0x7FF) as i32);
                let rows = s12(self.reg[BLT_SIZE] >> 16).max(0);
                self.blt = super::blit::Blit {
                    active: true,
                    x,
                    y,
                    column: 0,
                    rows,
                };
                return;
            }
            // A fill that keeps every pixel as it is (Moto Racer fills
            // with ROP AAAAh each frame) changes nothing.
            RECT_FILL
                if self
                    .blt_rops(Some(self.reg[BLT_COLOR] as u16))
                    .0
                    .iter()
                    .flatten()
                    .all(|&r| r == 0xA) =>
            {
                return;
            }
            _ => {}
        }
        let (size_x, size_y) = (s12(self.reg[BLT_SIZE]), s12(self.reg[BLT_SIZE] >> 16));
        let (dx, dy) = (size_x.signum().max(0) * 2 - 1, size_y.signum().max(0) * 2 - 1);
        let (dst_x, dst_y) = ((self.reg[BLT_DST_XY] & 0x7FF) as i32, ((self.reg[BLT_DST_XY] >> 16) & 0x7FF) as i32);
        let (dst_base, dst_stride) = self.blt_dst();
        // After what is drawn, which may read or write the same memory:
        // the rows the blit reads and writes. An SGRAM fill's are pages.
        let src_y = ((self.reg[BLT_SRC_XY] >> 16) & 0x7FF) as i32;
        let (src_base, src_stride) = self.blt_src();
        let rows = [
            blt_rows(dst_base, dst_stride, dst_y, dst_y + size_y),
            blt_rows(src_base, src_stride, src_y, src_y + size_y),
        ];
        match command & COMMAND {
            SCREEN_TO_SCREEN => self.flush_in(&rows),
            RECT_FILL => self.flush_in(&rows[..1]),
            _ => self.flush(),
        }
        self.mirror_sync();
        let at = |x: i32, y: i32, base: u32, stride: u32| base.wrapping_add((y as u32).wrapping_mul(stride)).wrapping_add(2 * x as u32);
        match command & COMMAND {
            SCREEN_TO_SCREEN => {
                let (src_base, src_stride) = self.blt_src();
                let (src_x, src_y) = ((self.reg[BLT_SRC_XY] & 0x7FF) as i32, ((self.reg[BLT_SRC_XY] >> 16) & 0x7FF) as i32);
                for row in 0..=size_y.abs() {
                    for col in 0..=size_x.abs() {
                        let (x, y) = (dst_x + col * dx, dst_y + row * dy);
                        if self.blt_clipped(x, y) {
                            continue;
                        }
                        let src = self.blt_get(at(src_x + col * dx, src_y + row * dy, src_base, src_stride));
                        self.blt_put(at(x, y, dst_base, dst_stride), src);
                    }
                }
            }
            RECT_FILL => {
                let color = self.reg[BLT_COLOR] as u16;
                for row in 0..=size_y.abs() {
                    for col in 0..=size_x.abs() {
                        let (x, y) = (dst_x + col * dx, dst_y + row * dy);
                        if !self.blt_clipped(x, y) {
                            self.blt_put(at(x, y, dst_base, dst_stride), color);
                        }
                    }
                }
            }
            SGRAM_FILL => self.sgram_fill(),
            _ => {}
        }
        self.display_dirty = true;
    }

    /// The words an SGRAM fill covers (`sgram_fill`), if they are one run:
    /// from the first column of the first page to the last column of the
    /// last.
    fn sgram_run(&self) -> Option<(u32, u32)> {
        let (dst, size) = (self.reg[BLT_DST_XY], self.reg[BLT_SIZE]);
        let (page, column) = ((dst >> 16) & 0x3FF, dst & 0x1FF);
        let (pages, columns) = ((size >> 16) & 0x3FF, size & 0x1FF);
        let last = (column + columns).min(511);
        let run = (
            page * 2048 + 4 * column,
            (page + pages) * 2048 + 4 * (last + 1),
        );
        (pages == 0 || (column == 0 && last == 511)).then_some(run)
    }

    /// An SGRAM fill of whole rows of the buffers, as Glide 3 clears a
    /// buffer: queued as jobs behind what is drawn, rather than drawn at
    /// once after waiting for it, so the OpenGL renderer's held-back jobs
    /// under it can be dropped (`backlog`). False if it fills anything
    /// else, which `sgram_fill` does, or the buffers overlap or reach past
    /// the memory, where the workers' rows of one buffer aren't those of
    /// another.
    fn sgram_fill_rows(&mut self) -> bool {
        if self.banshee.is_some() || !self.backlog.borrow().layout.prunable {
            return false;
        }
        let rows = match self.sgram_tiles() {
            Some(tiles) => match self.sgram_rows(&tiles) {
                Some((rows, false)) => rows,
                _ => return false,
            },
            None => {
                let Some((first, end)) = self.sgram_run() else {
                    return false;
                };
                let Some(rows) = self.whole_rows(first, end) else {
                    return false;
                };
                rows
            }
        };
        let color = self.reg[BLT_COLOR] as u16;
        self.mirror_sync();
        self.mirror_fill_rows(&rows, color);
        let (width, rowpixels) = (self.fbi.width as usize, self.fbi.rowpixels as usize);
        for (base, top, bottom) in rows {
            let access = self.access(|layout| super::backlog::rows(layout, base, (top, bottom)));
            let job = |columns| Job::Rows {
                fb: self.fbi.ram.clone(),
                base,
                rowpixels,
                rows: (top, bottom),
                columns,
                value: color,
            };
            // Rows wider than the picture (800 pixels in 832) are kept
            // for the words right of it, which the renderer doesn't have:
            // it takes the picture, and those words are filled here.
            let picture = access
                .clone()
                .map(|a| super::backlog::Access { kept: false, ..a });
            let split = width < rowpixels && access.as_ref().is_some_and(|a| a.kept && !a.pinned);
            let taken = picture
                .as_ref()
                .filter(|_| split)
                .and_then(|a| self.gpu_takes(&job((0, width)), a));
            if let Some(bytes) = taken {
                self.gpu_left(&job((0, width)), bytes);
                let outside = super::backlog::Access {
                    kept: true,
                    ..Default::default()
                };
                self.submit_outside(job((width, rowpixels)), Some(outside));
            } else {
                self.submit(job((0, rowpixels)), access);
            }
            self.mark_drawn(base);
        }
        true
    }

    /// The words `first..end` as rows of the buffers (the colour buffers'
    /// and the auxiliary buffer's, up to their heights, where the
    /// backlog's tiles have them): each buffer's word offset and its rows.
    /// None if they aren't whole rows of them.
    fn whole_rows(&self, first: u32, end: u32) -> Option<Vec<(usize, u32, u32)>> {
        let layout = self.backlog.borrow().layout;
        // Rows one after another, the same for every buffer.
        if layout.stride.tiled || layout.aux_stride != layout.stride {
            return None;
        }
        let row = layout.stride.words;
        let size = row * layout.height as usize;
        let (mut at, end) = (first as usize, end as usize);
        let mut rows = Vec::new();
        while at < end {
            let mut bases = layout.bases.into_iter().flatten();
            let base = bases.find(|&b| b <= at && at < b + size)?;
            let stop = end.min(base + size);
            if (at - base) % row != 0 || (stop - base) % row != 0 {
                return None;
            }
            rows.push((
                base,
                ((at - base) / row) as u32,
                ((stop - base) / row) as u32,
            ));
            at = stop;
        }
        Some(rows)
    }

    /// How the SGRAM's pages lie in the buffers, if they lie in tiles
    /// (`SgramTiles`): rows of whole pages, each buffer on a page.
    fn sgram_tiles(&self) -> Option<SgramTiles> {
        let fbi = &self.fbi;
        let rowpixels = fbi.rowpixels;
        let pages = (self.reg[FBI_INIT2] >> 11) & 0x1FF;
        if self.banshee.is_some() || rowpixels == 0 || !rowpixels.is_multiple_of(64) || pages == 0 {
            return None;
        }
        let mut buffers = [None; 4];
        let offsets = fbi.rgboffs.iter().chain([&fbi.auxoffs]);
        for (slot, &offs) in buffers.iter_mut().zip(offsets) {
            if offs == NONE {
                continue;
            }
            if !offs.is_multiple_of(0x1000) {
                return None;
            }
            *slot = Some(offs / 0x1000);
        }
        Some(SgramTiles {
            page_row: rowpixels / 64,
            rowpixels,
            pages,
            height: fbi.height,
            buffers,
        })
    }

    /// The rows of the buffers an SGRAM fill covers in `tiles`: each
    /// buffer's word offset and its rows, in the order the fill reaches
    /// them, and whether it also fills memory outside the buffers'
    /// pictures. None if it covers part of a row of a picture.
    fn sgram_rows(&self, tiles: &SgramTiles) -> Option<(Vec<BufferRows>, bool)> {
        let (dst, size) = (self.reg[BLT_DST_XY], self.reg[BLT_SIZE]);
        let (page, column) = ((dst >> 16) & 0x3FF, dst & 0x1FF);
        let (pages, columns) = ((size >> 16) & 0x3FF, size & 0x1FF);
        let last = (column + columns).min(511);
        // The rows of a page each column range covers, and whether it
        // covers them whole.
        let (top, bottom) = (column / 16, last / 16 + 1);
        let whole = column % 16 == 0 && last % 16 == 15;
        let mut rows: Vec<BufferRows> = Vec::new();
        let mut outside = false;
        let mut p = page;
        while p <= page + pages {
            let Some((base, rel)) = tiles.find(p) else {
                outside = true;
                p += 1;
                continue;
            };
            // The fill's pages in this row of tiles.
            let row = rel / tiles.page_row;
            let end = (p + tiles.page_row - rel % tiles.page_row)
                .min(page + pages + 1)
                .min(base + tiles.pages);
            let first = row * 32;
            let (y0, y1) = (first + top, (first + bottom).min(tiles.height));
            if y0 < y1 {
                if !whole || rel % tiles.page_row != 0 || end - p != tiles.page_row {
                    return None;
                }
                let base = base as usize * 2048;
                match rows.last_mut() {
                    Some((b, _, last)) if *b == base && *last == y0 => *last = y1,
                    _ => rows.push((base, y0, y1)),
                }
            }
            outside |= first + bottom > tiles.height;
            p = end;
        }
        Some((rows, outside))
    }

    /// An SGRAM fill: whole 4 KB pages from bltDstXY's row (bits 24:16),
    /// columns of 64 bits (four pixels) from its column (bits 8:0), as
    /// many pages as bltSize bits 24:16 say and columns as bits 8:0, with
    /// bltColor and no clipping or raster operations. The pages are the
    /// memory's own, in tiles (`SgramTiles`), where the memory here is in
    /// rows.
    fn sgram_fill(&mut self) {
        let (dst, size) = (self.reg[BLT_DST_XY], self.reg[BLT_SIZE]);
        let (page, column) = ((dst >> 16) & 0x3FF, dst & 0x1FF);
        let (pages, columns) = ((size >> 16) & 0x3FF, size & 0x1FF);
        let color = self.reg[BLT_COLOR] as u16;
        let ram = &self.fbi.ram;
        let last = (column + columns).min(511);
        if let Some(tiles) = self.sgram_tiles() {
            for p in page..=page + pages {
                for c in column..=last {
                    let at = tiles.word(p, c);
                    for at in at..at + 4 {
                        if at < ram.len() {
                            ram.set(at, color);
                        }
                    }
                }
            }
            match self.sgram_rows(&tiles) {
                Some((rows, _)) => self.mirror_fill_rows(&rows, color),
                None => {
                    // Part of some rows: a snapshot takes the fill.
                    if let Some(mirror) = self.mirror.get_mut() {
                        mirror.invalidate();
                    }
                }
            }
            return;
        }
        for p in page..=page + pages {
            let base = p as usize * 2048;
            for at in base + 4 * column as usize..base + 4 * (last as usize + 1) {
                if at < ram.len() {
                    ram.set(at, color);
                }
            }
        }
        if let Some((first, end)) = self.sgram_run() {
            self.mirror_fill(first, end, color);
        } else if let Some(mirror) = self.mirror.get_mut() {
            // The same columns of several pages: a snapshot takes them.
            mirror.invalidate();
        }
    }

    /// Tell the OpenGL renderer's recording about an SGRAM fill of whole
    /// `rows` of the buffers (`sgram_rows`): each buffer's word offset and
    /// its rows.
    fn mirror_fill_rows(&mut self, rows: &[(usize, u32, u32)], color: u16) {
        if !self.recording() {
            return;
        }
        let (width, aux) = (self.fbi.width, self.fbi.auxoffs);
        let rgb = rgb888(color);
        if let Some(mirror) = self.mirror.get_mut() {
            for &(base, top, bottom) in rows {
                let colour = aux == NONE || base != aux as usize / 2;
                mirror.fill(super::mirror::Fill {
                    dest: base as u32,
                    rect: [0, width, top, bottom],
                    color: colour.then_some(rgb),
                    aux: (!colour).then_some(color),
                    alpha_planes: false,
                    pattern: None,
                });
            }
        }
    }

    /// Tell the OpenGL renderer's recording about an SGRAM fill of the
    /// words `first..end`: the rows of each buffer it covers, as fills, or
    /// a snapshot where it covers part of a row.
    fn mirror_fill(&mut self, first: u32, end: u32, color: u16) {
        if !self.recording() {
            return;
        }
        let fbi = &self.fbi;
        let (width, height, row) = (fbi.width, fbi.height, fbi.rowpixels.max(1));
        let buffers = fbi.rgboffs.iter().map(|&o| (o, true)).chain([(fbi.auxoffs, false)]);
        let mut fills = Vec::new();
        for (offs, colour) in buffers {
            if offs == NONE {
                continue;
            }
            let base = offs / 2;
            let start = first.max(base);
            let stop = end.min(base + row * height);
            if start >= stop {
                continue;
            }
            if (start - base) % row != 0 || (stop - base) % row != 0 {
                // A row partly covered: a snapshot takes the fill.
                if let Some(mirror) = self.mirror.get_mut() {
                    mirror.invalidate();
                }
                return;
            }
            let (top, bottom) = ((start - base) / row, (stop - base) / row);
            if top < bottom {
                fills.push((base, colour, top, bottom));
            }
        }
        let rgb = rgb888(color);
        if let Some(mirror) = self.mirror.get_mut() {
            for (base, colour, top, bottom) in fills {
                mirror.fill(super::mirror::Fill {
                    dest: base,
                    rect: [0, width, top, bottom],
                    color: colour.then_some(rgb),
                    aux: (!colour).then_some(color),
                    alpha_planes: false,
                    pattern: None,
                });
            }
        }
    }

    /// bltData: source data for a CPU-to-screen copy, one to 32 pixels a
    /// word as bltCommand's format says, row after row.
    pub(crate) fn blit_data(&mut self, data: u32) {
        let command = self.blt_command();
        if command & COMMAND != CPU_TO_SCREEN || !self.blt.active {
            return;
        }
        // After what is drawn, unless the copy goes into the queue behind
        // it (`blt_queue_dest`). Mostly nothing is queued, and the words
        // skip the wait: Moto Racer sends 40,000 a frame for its sky.
        // The recording's layout is checked once a copy, like the queue's
        // answer: what it is worked out from changes with the same writes,
        // which drop that answer. A recording that lost its layout since
        // (too many commands) gets it back.
        let synced = self.blt_queue.get().is_some()
            && (self.mirror.get_mut().as_ref()).is_none_or(|m| m.layout().is_some());
        let queue = self.blt_queue_cached();
        if queue.is_none() && self.writes_queue() {
            // The rows it has left to write, whose pixels it may read.
            let (base, stride) = self.blt_dst();
            self.flush_in(&[blt_rows(
                base,
                stride,
                self.blt.y,
                self.blt.y + self.blt.rows,
            )]);
        }
        if synced {
            debug_assert!(
                (self.mirror.borrow().as_ref())
                    .is_none_or(|m| !m.needs_snapshot(&self.mirror_key(m.layout()))),
                "a layout change blt_queue_stale missed"
            );
        } else {
            self.mirror_sync();
        }
        let mut data = data;
        if command & (1 << 8) != 0 {
            data = data.swap_bytes();
        }
        if command & (1 << 9) != 0 {
            data = data.rotate_left(16);
        }
        let size_x = s12(self.reg[BLT_SIZE]).max(0);
        let (dst_base, dst_stride) = self.blt_dst();
        let (fg, bg) = (self.reg[BLT_COLOR] as u16, (self.reg[BLT_COLOR] >> 16) as u16);
        let format = (command >> 3) & 7;
        if format <= 1 {
            // Monochrome data has its leftmost pixel in each byte's top
            // bit (the spec's table of bit positions): reverse each byte so
            // the pixels come out from bit 0 up.
            data = data.reverse_bits().swap_bytes();
        }
        let lanes = (command >> 6) & 3;
        let mut bits = 32;
        let mut words = [Word {
            row: 0,
            at: 0,
            value: 0,
        }; 32];
        let mut queued = 0;
        while bits > 0 && self.blt.column <= size_x && self.blt.active {
            let x = self.blt.x + self.blt.column;
            let y = self.blt.y;
            let pixel = match format {
                0 | 1 => {
                    let set = data & 1 != 0;
                    data >>= 1;
                    bits -= 1;
                    (set || command & TRANSPARENT == 0).then_some(if set { fg } else { bg })
                }
                2 => {
                    let p = data as u16;
                    data >>= 16;
                    bits -= 16;
                    // BGR lanes swap red and blue.
                    Some(if lanes & 1 != 0 { (p >> 11) | (p & 0x07E0) | (p << 11) } else { p })
                }
                3..=5 => {
                    let (r, g, b) = match lanes {
                        0 => (data >> 16, data >> 8, data),
                        1 => (data, data >> 8, data >> 16),
                        2 => (data >> 24, data >> 16, data >> 8),
                        _ => (data >> 8, data >> 16, data >> 24),
                    };
                    let (mut r, mut g, mut b) = ((r & 0xFF) as usize, (g & 0xFF) as usize, (b & 0xFF) as usize);
                    bits = 0;
                    if format != 3 {
                        // Ordered dithering, 2x2 or 4x4.
                        let t = super::tables::tables();
                        let lookup = if format == 4 { &t.dither2 } else { &t.dither4 };
                        let at = ((y & 3) as usize) << 11 | ((x & 3) as usize) << 1;
                        r = lookup[at | r << 3] as usize;
                        g = lookup[at | g << 3 | 1] as usize;
                        b = lookup[at | b << 3] as usize;
                        Some((r << 11 | g << 5 | b) as u16)
                    } else {
                        Some(((r >> 3) << 11 | (g >> 2) << 5 | b >> 3) as u16)
                    }
                }
                _ => {
                    bits = 0;
                    None
                }
            };
            if let Some(p) = pixel
                && !self.blt_clipped(x, y)
            {
                let addr = dst_base.wrapping_add((y as u32).wrapping_mul(dst_stride)).wrapping_add(2 * x as u32);
                if queue.is_some() {
                    // The destination's test picks the same ROP for every
                    // pixel there (`blt_queue_dest`), so any value will
                    // do for it. ROP AAAAh keeps the pixel as it is: a
                    // source colour key's transparent pixels.
                    let rop = self.blt_rop(p, 0);
                    if rop != 0xA {
                        let at = addr >> 1;
                        let value = mix(rop, p, 0);
                        words[queued] = Word {
                            row: y as u16,
                            at,
                            value,
                        };
                        queued += 1;
                    }
                } else {
                    self.blt_put(addr, p);
                }
            }
            self.blt.column += 1;
            if self.blt.column > size_x {
                // The next row, or done.
                self.blt.column = 0;
                self.blt.y += 1;
                self.blt.rows -= 1;
                if self.blt.rows < 0 {
                    self.blt.active = false;
                }
                // A row starts on a new word, or with byte-packed
                // monochrome data (format 1) on a new byte.
                if format != 1 {
                    break;
                }
                let skip = bits % 8;
                data >>= skip;
                bits -= skip;
            }
        }
        if let Some(dest) = queue
            && queued > 0
        {
            let words = &words[..queued];
            // The copy's rows are the buffer's (`blt_queue_dest`), so the
            // recording gets the pixels where they are, without working
            // out which buffer each is in.
            let (width, row) = (self.fbi.width, self.fbi.stride.words);
            if let Some(mirror) = self.mirror.get_mut() {
                for w in words {
                    let x = (w.at as usize - dest - w.row as usize * row) as u32;
                    if x < width {
                        mirror.pixel(Some(dest as u32), x, w.row as u32, w.value, width);
                    }
                }
            }
            if self.writes_queue() {
                self.queue_words(words);
                // The display waits for them if it shows the buffer.
                self.mark_drawn(dest);
            } else {
                for w in words {
                    self.fbi.ram.set(w.at as usize, w.value);
                }
            }
        }
        self.display_dirty = true;
    }

    /// The ROPs the chroma range tests can pick for the blit's pixels,
    /// from a source pixel `src` or any (None where they pick fewer), and
    /// whether the destination's test picks between them by the pixel
    /// there. Without a test its bit of the ROP number is clear, and with
    /// a range that holds every colour or none (Moto Racer's Glide sets
    /// 0 to FFFFh) it is the same for every pixel.
    fn blt_rops(&self, src: Option<u16>) -> ([Option<u32>; 4], bool) {
        let command = self.blt_command();
        let (dst, dst_tested) = if command & DST_CHROMA == 0 {
            ([Some(0), None], false)
        } else {
            match chroma_constant(self.reg[BLT_DST_CHROMA_RANGE]) {
                Some(true) => ([Some(1), None], false),
                Some(false) => ([Some(0), None], false),
                None => ([Some(0), Some(1)], true),
            }
        };
        let source = match src {
            _ if command & SRC_CHROMA == 0 => [Some(0), None],
            Some(p) if in_range(self.reg[BLT_SRC_CHROMA_RANGE], p) => [Some(2), None],
            Some(_) => [Some(0), None],
            None => [Some(0), Some(2)],
        };
        let rop =
            |d: Option<u32>, s: Option<u32>| Some((self.reg[BLT_ROP] >> (4 * (d? | s?))) & 15);
        let rops = [
            rop(dst[0], source[0]),
            rop(dst[0], source[1]),
            rop(dst[1], source[0]),
            rop(dst[1], source[1]),
        ];
        (rops, dst_tested)
    }

    /// The colour buffer a CPU-to-screen copy writes into (its word
    /// offset), when its pixels can wait behind the drawing queued before
    /// them as frame buffer writes do (`lfb_write`): they don't depend on
    /// the destination's, and what is left of its rectangle lies in a
    /// colour buffer, whose rows the triangles draw. Otherwise the drawing
    /// is done first.
    ///
    /// It is worked out once a copy. What it depends on changes only with
    /// a register write other than bltData or a new layout of the
    /// buffers, and those drop it (`blt_queue_stale`). Debug builds check
    /// that nothing else changed it.
    fn blt_queue_cached(&self) -> Option<usize> {
        if let Some((key, dest)) = self.blt_queue.get() {
            debug_assert_eq!(key, self.blt_queue_key(), "a change blt_queue_stale missed");
            return dest;
        }
        let dest = self.blt_queue_dest();
        self.blt_queue.set(Some((self.blt_queue_key(), dest)));
        dest
    }

    /// Drop `blt_queue_cached`'s answer: something it was worked out from
    /// may have changed.
    pub(crate) fn blt_queue_stale(&self) {
        self.blt_queue.set(None);
    }

    /// What `blt_queue_dest` works its answer out from, which debug builds
    /// check the cached answer against. The source's chroma range isn't:
    /// it counts every ROP the source's test can pick.
    fn blt_queue_key(&self) -> [u32; 15] {
        let (r, f) = (&self.reg, &self.fbi);
        [
            r[BLT_DST_BASE_ADDR],
            r[BLT_XY_STRIDES],
            r[BLT_DST_CHROMA_RANGE],
            r[BLT_ROP],
            r[BLT_COMMAND],
            r[BLT_SIZE],
            f.rgboffs[0],
            f.rgboffs[1],
            f.rgboffs[2],
            f.stride.words as u32,
            f.stride.tiled as u32,
            f.height,
            f.ram.len() as u32,
            self.blt.x as u32,
            (self.blt.y + self.blt.rows) as u32,
        ]
    }

    /// `blt_queue_cached`'s answer, worked out.
    fn blt_queue_dest(&self) -> Option<usize> {
        // The ROPs the chroma tests can pick must give the same pixel
        // whatever the destination holds (0, !s, s or 1s) or keep the
        // destination's (d, which leaves the pixel out: Moto Racer's
        // sprites with a source colour key), and the destination's test
        // must pick the same for every pixel.
        let (rops, dst_tested) = self.blt_rops(None);
        if dst_tested
            || rops
                .iter()
                .flatten()
                .any(|&r| !matches!(r, 0x0 | 0x3 | 0xA | 0xC | 0xF))
        {
            return None;
        }
        // A tiled destination is addressed as rows of its tiles' width
        // (`blt_layout`), which the checks below hold to a buffer's rows.
        let (base, stride) = self.blt_dst();
        let f = &self.fbi;
        if f.stride.tiled || stride as usize != 2 * f.stride.words {
            return None;
        }
        let right = self.blt.x as usize + s12(self.reg[BLT_SIZE]).max(0) as usize;
        let bottom = self.blt.y as usize + self.blt.rows as usize;
        let start = base as usize / 2;
        if right >= f.stride.words
            || bottom >= f.height as usize
            || start + (bottom + 1) * f.stride.words > f.ram.len()
        {
            return None;
        }
        f.rgboffs
            .iter()
            .any(|&o| o != NONE && o == base)
            .then_some(start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voodoo::{Board, Now, Voodoo};

    fn card() -> Voodoo {
        let mut v = Voodoo::with_workers(Board::Voodoo2, 0);
        v.reg[BLT_ROP] = 0xCCCC; // source copy
        v
    }

    fn write(v: &mut Voodoo, reg: usize, data: u32) {
        v.write((reg as u32) << 2, data, 0xFFFF_FFFF, Now::default());
    }

    #[test]
    fn rectangle_fill_is_size_plus_one_pixels() {
        let mut v = card();
        write(&mut v, BLT_ROP, 0xCCCC);
        write(&mut v, BLT_XY_STRIDES, 2048 << 16);
        write(&mut v, BLT_COLOR, 0x1234);
        write(&mut v, BLT_DST_XY, 2 << 16 | 3);
        write(&mut v, BLT_SIZE, 1 << 16 | 1);
        write(&mut v, BLT_COMMAND, 1 << 31 | RECT_FILL);
        let px = |x: usize, y: usize| v.fbi.ram.get(y * 1024 + x);
        assert_eq!([px(3, 2), px(4, 2), px(3, 3), px(4, 3)], [0x1234; 4]);
        assert_eq!([px(2, 2), px(5, 2), px(3, 4)], [0; 3]);
    }

    #[test]
    fn sgram_fill_clears_whole_pages() {
        let mut v = card();
        write(&mut v, BLT_COLOR, 0xBEEF);
        write(&mut v, BLT_DST_XY, 1 << 16);
        write(&mut v, BLT_SIZE, 1 << 16 | 511);
        write(&mut v, BLT_COMMAND, 1 << 31 | SGRAM_FILL);
        assert_eq!(v.fbi.ram.get(2047), 0);
        assert_eq!(v.fbi.ram.get(2048), 0xBEEF);
        assert_eq!(v.fbi.ram.get(3 * 2048 - 1), 0xBEEF);
        assert_eq!(v.fbi.ram.get(3 * 2048), 0);
    }

    /// A Voodoo2 with `workers` threads set up as Glide sets it for
    /// 640x480: two colour buffers 150 pages apart, the auxiliary buffer
    /// after them, 640-pixel rows.
    fn glide_card(workers: usize) -> Voodoo {
        let mut v = Voodoo::with_workers(Board::Voodoo2, workers);
        v.reg[FBI_INIT1] = 10 << 4;
        v.reg[FBI_INIT2] = 150 << 11;
        v.reg[FBI_INIT5] &= !(3 << 9);
        v.reg[FBI_INIT6] &= !(1 << 30);
        v.recompute_video_memory();
        v
    }

    /// Frames as Glide 3 draws them: the back buffer and the auxiliary
    /// buffer cleared with SGRAM fills, a fastfill of part of the back
    /// buffer, and a swap.
    fn glide_frames(v: &mut Voodoo, count: u32) {
        for frame in 0..count {
            let page = 150 * v.fbi.backbuf as u32;
            for (page, color) in [(page, 0x0841 * frame), (300, 0xFFFF)] {
                write(v, BLT_COLOR, color);
                write(v, BLT_DST_XY, page << 16);
                write(v, BLT_SIZE, 149 << 16 | 511);
                write(v, BLT_COMMAND, 1 << 31 | SGRAM_FILL);
            }
            write(v, FBZ_MODE, 1 | 1 << 9 | 1 << 14);
            write(v, CLIP_LEFT_RIGHT, (10 + frame) << 16 | 300);
            write(v, CLIP_LOW_Y_HIGH_Y, 20 << 16 | (200 + frame));
            write(v, COLOR1, 0x0040_80C0 + frame);
            write(v, FASTFILL_CMD, 0);
            write(v, SWAPBUFFER_CMD, 0);
        }
    }

    #[test]
    fn sgram_fills_of_whole_buffers_drop_what_they_cover() {
        // With the OpenGL renderer, a buffer cleared with an SGRAM fill
        // makes what was drawn into it before invisible: those jobs go
        // (Oni's menu, #204), and nothing waits for the workers.
        let mut gl = glide_card(2);
        gl.set_mirror(true);
        gl.set_software_picture(false);
        let mut sw = glide_card(0);
        glide_frames(&mut gl, 8);
        glide_frames(&mut sw, 8);
        let status = gl.describe(Now::default());
        assert!(status["pruned_jobs"].as_u64().unwrap() >= 10, "{}", status);
        assert_eq!(status["catch_ups"], 0, "{}", status);
        // Once something reads the memory, it is what was drawn.
        assert!(gl.frame_buffer().to_bytes() == sw.frame_buffer().to_bytes());
        // Fills of anything but whole rows of the buffers wait for what is
        // drawn, as before.
        write(&mut gl, FASTFILL_CMD, 0);
        assert_eq!(gl.describe(Now::default())["held_jobs"], 1);
        write(&mut gl, BLT_DST_XY, 600 << 16);
        write(&mut gl, BLT_SIZE, 0);
        write(&mut gl, BLT_COMMAND, 1 << 31 | SGRAM_FILL);
        assert_eq!(gl.describe(Now::default())["held_jobs"], 0);
    }

    /// A Voodoo2 with `workers` threads set up as 3DMark 2000 sets it for
    /// 800x600: three colour buffers and the auxiliary buffer 247 pages
    /// apart (19 rows of 13 pages), 832-pixel rows.
    fn card_800(workers: usize) -> Voodoo {
        let mut v = Voodoo::with_workers(Board::Voodoo2, workers);
        v.reg[FBI_INIT1] = 13 << 4;
        v.reg[FBI_INIT2] = 247 << 11;
        v.reg[FBI_INIT5] = v.reg[FBI_INIT5] & !(3 << 9) | 2 << 9;
        v.reg[FBI_INIT6] &= !(1 << 30);
        (v.fbi.width, v.fbi.height) = (800, 600);
        v.recompute_video_memory();
        v
    }

    /// Frames as 3dfx's Direct3D driver clears 3DMark 2000's depth
    /// buffer: an SGRAM fill of its first 18 rows of tiles (576 rows),
    /// and one of the first 384 columns of the 13 pages of the next,
    /// which are its last 24 rows. Then a fastfill of part of the back
    /// buffer and a swap.
    fn depth_clears_800(v: &mut Voodoo, count: u32) {
        for frame in 0..count {
            depth_fills_800(v);
            write(v, FBZ_MODE, 1 | 1 << 9 | 1 << 14);
            write(v, CLIP_LEFT_RIGHT, (10 + frame) << 16 | 700);
            write(v, CLIP_LOW_Y_HIGH_Y, 20 << 16 | (590 + frame));
            write(v, COLOR1, 0x0040_80C0 + frame);
            write(v, FASTFILL_CMD, 0);
            write(v, SWAPBUFFER_CMD, 0);
        }
    }

    /// The two SGRAM fills of `depth_clears_800`.
    fn depth_fills_800(v: &mut Voodoo) {
        let aux = v.fbi.auxoffs / 0x1000;
        write(v, BLT_COLOR, 0xFFFF);
        write(v, BLT_DST_XY, aux << 16);
        write(v, BLT_SIZE, 233 << 16 | 511);
        write(v, BLT_COMMAND, 1 << 31 | SGRAM_FILL);
        write(v, BLT_DST_XY, (aux + 234) << 16);
        write(v, BLT_SIZE, 12 << 16 | 383);
        write(v, BLT_COMMAND, 1 << 31 | SGRAM_FILL);
    }

    #[test]
    fn sgram_fills_write_pages_of_tiles() {
        // A page is 32 rows of 64 pixels, its columns four pixels of one
        // row, and a buffer's pages go along its rows of tiles.
        let mut v = card_800(0);
        write(&mut v, BLT_COLOR, 0xBEEF);
        write(&mut v, BLT_DST_XY, 14 << 16 | 16);
        write(&mut v, BLT_SIZE, 1);
        write(&mut v, BLT_COMMAND, 1 << 31 | SGRAM_FILL);
        // Page 14 is the second of the second row of tiles; columns 16
        // and 17 are the first eight pixels of its second row.
        let px = |v: &Voodoo, x: usize, y: usize| v.fbi.ram.get(y * 832 + x);
        assert_eq!([px(&v, 64, 33), px(&v, 71, 33)], [0xBEEF; 2]);
        assert_eq!(
            [
                px(&v, 63, 33),
                px(&v, 72, 33),
                px(&v, 64, 32),
                px(&v, 64, 34)
            ],
            [0; 4]
        );
        // The depth buffer's last 24 rows, as 3DMark 2000 clears them.
        depth_fills_800(&mut v);
        let aux = v.fbi.auxoffs as usize / 2;
        let depth = |y: usize, x: usize| v.fbi.ram.get(aux + y * 832 + x);
        for y in [0, 300, 575, 576, 590, 599] {
            for x in [0, 63, 64, 400, 799, 831] {
                assert_eq!(depth(y, x), 0xFFFF, "{x}, {y}");
            }
        }
        // Not the rows of the last row of tiles past the picture.
        assert_eq!([depth(600, 0), depth(607, 831)], [0; 2]);
    }

    #[test]
    fn sgram_fills_of_the_last_rows_of_tiles_stay_on_the_gpu() {
        // 3DMark 2000's clear of its depth buffer's last 24 rows goes to
        // the renderer as a fill of those rows, the same as the rows
        // before it: nothing is read back, nothing waits for the
        // workers, and the recording needs no snapshot.
        let mut v = card_800(2);
        v.set_mirror(true);
        v.set_software_picture(false);
        let log = std::rc::Rc::default();
        v.set_gpu_link(Some(Box::new(Renderer(std::rc::Rc::clone(&log)))));
        v.set_gpu_reads(true);
        depth_clears_800(&mut v, 3);
        let status = v.describe(Now::default());
        assert_eq!(status["catch_ups"], 0, "{status}");
        assert_eq!(status["gpu_read_backs"], 0, "{status}");
        assert!(log.borrow().is_empty(), "{:?}", log.borrow());
        assert!(v.mirror_layout().is_some(), "the recording was invalidated");
        let aux = v.fbi.auxoffs / 2;
        // The words right of the picture, which the renderer doesn't
        // have, are filled in memory.
        v.catch_up_workers();
        let depth = |y: u32, x: u32| v.fbi.ram.get((aux + y * 832 + x) as usize);
        assert_eq!([depth(10, 800), depth(599, 831)], [0xFFFF; 2]);
        let fills: Vec<_> = v
            .take_mirror()
            .unwrap()
            .commands
            .iter()
            .filter_map(|c| match c {
                super::super::mirror::Command::Fill(f) if f.dest == aux => Some(f.rect),
                _ => None,
            })
            .collect();
        assert_eq!(fills.len(), 6, "{fills:?}");
        assert!(fills.contains(&[0, 800, 576, 600]), "{fills:?}");
        // Held back in software instead, the memory is as the software
        // renderer draws it.
        let mut gl = card_800(2);
        gl.set_mirror(true);
        gl.set_software_picture(false);
        let mut sw = card_800(0);
        depth_clears_800(&mut gl, 3);
        depth_clears_800(&mut sw, 3);
        let status = gl.describe(Now::default());
        assert_eq!(status["catch_ups"], 0, "{status}");
        assert!(
            gl.mirror_layout().is_some(),
            "the recording was invalidated"
        );
        assert!(gl.frame_buffer().to_bytes() == sw.frame_buffer().to_bytes());
    }

    #[test]
    fn cpu_to_screen_with_16_bit_pixels() {
        let mut v = card();
        write(&mut v, BLT_ROP, 0xCCCC);
        write(&mut v, BLT_XY_STRIDES, 2048 << 16);
        write(&mut v, BLT_DST_XY, 1 << 16 | 10);
        // Three pixels wide, two rows: a word and a half each.
        write(&mut v, BLT_SIZE, 1 << 16 | 2);
        write(&mut v, BLT_COMMAND, 1 << 31 | 2 << 3 | CPU_TO_SCREEN);
        for word in [0x2222_1111, 0x0000_3333, 0x5555_4444, 0x0000_6666] {
            write(&mut v, BLT_DATA, word);
        }
        let px = |x: usize, y: usize| v.fbi.ram.get(y * 1024 + x);
        assert_eq!([px(10, 1), px(11, 1), px(12, 1), px(13, 1)], [0x1111, 0x2222, 0x3333, 0]);
        assert_eq!([px(10, 2), px(11, 2), px(12, 2)], [0x4444, 0x5555, 0x6666]);
    }

    /// A copy from the CPU into the back buffer, 16-bit pixels with the
    /// source copied: 4 pixels by 3 rows at (12, `y`), over what the
    /// frames before drew there. `flags` go into bltCommand.
    fn copy_into_back_buffer(v: &mut Voodoo, y: u32, flags: u32) {
        let back = v.fbi.rgboffs[v.fbi.backbuf as usize];
        write(v, BLT_ROP, 0xCCCC);
        write(v, BLT_DST_BASE_ADDR, back);
        write(v, BLT_XY_STRIDES, 1280 << 16);
        if flags & DST_TILED != 0 {
            // As Moto Racer's Glide sets it up: 4 KB pages and rows of 20
            // tiles, and a destination chroma range that holds every
            // colour, so that ROP 1 (source) is the one used.
            write(v, BLT_DST_BASE_ADDR, back >> 12);
            write(v, BLT_XY_STRIDES, 20 << 16);
            write(v, BLT_DST_CHROMA_RANGE, 0xFFFF_0000);
            write(v, BLT_ROP, 0xAACA);
        }
        if flags & SRC_CHROMA != 0 {
            // Its sprites' colour key: the source's pixels in the range
            // (the third data word's) pick ROP 2 or 3, which keep the
            // destination's.
            write(v, BLT_SRC_CHROMA_RANGE, 0x0303_0303);
        }
        write(v, BLT_DST_XY, y << 16 | 12);
        write(v, BLT_SIZE, 2 << 16 | 3);
        write(v, BLT_COMMAND, 1 << 31 | flags | 2 << 3 | CPU_TO_SCREEN);
        for i in 1..=6 {
            write(v, BLT_DATA, 0x0101_0101 * i);
        }
    }

    #[test]
    fn cpu_to_screen_copies_wait_behind_the_drawing() {
        // Moto Racer copies its sky into the back buffer from the CPU at
        // the start of each frame. A copy whose pixels don't depend on the
        // destination's waits behind the drawing queued before it, as
        // frame buffer writes do, instead of waiting for it to be drawn:
        // with the OpenGL renderer the jobs held back stay held. Either
        // way the memory ends up as it would have.
        for flags in [
            0,
            DST_TILED | DST_CHROMA,
            DST_TILED | DST_CHROMA | SRC_CHROMA,
        ] {
            let mut gl = glide_card(2);
            gl.set_mirror(true);
            gl.set_software_picture(false);
            let mut threads = glide_card(2);
            let mut sw = glide_card(0);
            for v in [&mut gl, &mut threads, &mut sw] {
                glide_frames(v, 2);
                copy_into_back_buffer(v, 30, flags);
            }
            let status = gl.describe(Now::default());
            assert_eq!(status["catch_ups"], 0, "{flags:X} {status}");
            assert!(
                status["held_jobs"].as_u64().unwrap() > 0,
                "{flags:X} {status}"
            );
            let memory = sw.frame_buffer().to_bytes();
            let back = sw.fbi.rgboffs[sw.fbi.backbuf as usize] as usize / 2;
            let px = |x: usize, y: usize| sw.fbi.ram.get(back + y * 640 + x);
            assert_eq!(
                [px(12, 30), px(15, 30), px(12, 32), px(15, 32)],
                [0x0101, 0x0202, 0x0505, 0x0606],
                "{flags:X}"
            );
            assert!(gl.frame_buffer().to_bytes() == memory, "{flags:X}");
            assert!(threads.frame_buffer().to_bytes() == memory, "{flags:X}");
            // The recording has the copy's pixels where they are in the
            // back buffer, but for those the colour key leaves out.
            let commands = gl.take_mirror().unwrap().commands;
            let rows: Vec<_> = commands
                .iter()
                .filter_map(|c| match c {
                    super::super::mirror::Command::Pixels(p)
                        if p.dest == Some(back as u32) && (30..33).contains(&p.y) =>
                    {
                        Some((p.x, p.y, p.values.clone()))
                    }
                    _ => None,
                })
                .collect();
            let middle = match flags & SRC_CHROMA {
                0 => (12, 31, vec![0x0303, 0x0303, 0x0404, 0x0404]),
                _ => (14, 31, vec![0x0404, 0x0404]),
            };
            assert_eq!(
                rows,
                [
                    (12, 30, vec![0x0101, 0x0101, 0x0202, 0x0202]),
                    middle,
                    (12, 32, vec![0x0505, 0x0505, 0x0606, 0x0606])
                ],
                "{flags:X}"
            );
        }
    }

    #[test]
    fn fills_that_keep_every_pixel_dont_wait_for_the_drawing() {
        // ROP AAAAh (destination) whatever the chroma tests say: the fill
        // changes nothing, so the jobs held back stay held.
        let mut gl = glide_card(2);
        gl.set_mirror(true);
        gl.set_software_picture(false);
        let mut sw = glide_card(0);
        for v in [&mut gl, &mut sw] {
            glide_frames(v, 2);
            write(v, BLT_ROP, 0xAAAA);
            write(v, BLT_XY_STRIDES, 1280 << 16);
            write(v, BLT_COLOR, 0x1234);
            write(v, BLT_DST_XY, 30 << 16 | 12);
            write(v, BLT_SIZE, 9 << 16 | 9);
            write(v, BLT_COMMAND, 1 << 31 | SRC_CHROMA | RECT_FILL);
        }
        let status = gl.describe(Now::default());
        assert_eq!(status["catch_ups"], 0, "{status}");
        assert!(gl.frame_buffer().to_bytes() == sw.frame_buffer().to_bytes());
    }

    #[test]
    fn other_cpu_to_screen_copies_wait_for_the_drawing() {
        // Destination chroma keying reads what the drawing left there, and
        // a copy past the buffer's last row writes memory the triangles
        // don't draw by rows.
        for (y, flags) in [(30, DST_CHROMA), (478, 0)] {
            let mut gl = glide_card(2);
            gl.set_mirror(true);
            gl.set_software_picture(false);
            let mut sw = glide_card(0);
            for v in [&mut gl, &mut sw] {
                glide_frames(v, 2);
                copy_into_back_buffer(v, y, flags);
            }
            assert_eq!(gl.describe(Now::default())["held_jobs"], 0, "y {y}");
            assert!(
                gl.frame_buffer().to_bytes() == sw.frame_buffer().to_bytes(),
                "y {y}"
            );
        }
    }

    /// An OpenGL renderer that drew each colour buffer in one colour and
    /// the depth buffer at one value, and notes what it is asked.
    struct Renderer(std::rc::Rc<std::cell::RefCell<Vec<String>>>);

    impl crate::voodoo::gpu::GpuLink for Renderer {
        fn ready(&self) -> bool {
            false
        }

        fn run(&mut self, commands: Vec<crate::voodoo::mirror::Command>) {
            self.0.borrow_mut().push(format!("run {}", commands.len()));
        }

        fn read(&mut self, _at: u32, _out: &mut [u8]) -> bool {
            false
        }

        fn read_buffer(
            &mut self,
            dest: Option<u32>,
            (top, bottom): (u32, u32),
            out: &mut [u16],
        ) -> bool {
            assert_eq!(out.len(), 640 * (bottom - top) as usize);
            out.fill(dest.map_or(0x7777, |d| 0x1234 ^ d as u16));
            self.0
                .borrow_mut()
                .push(format!("read {dest:?} {top}..{bottom}"));
            true
        }
    }

    #[test]
    fn gpu_reads_leave_the_drawing_to_the_renderer() {
        // With voodoo_readback=gpu nothing is drawn in software or held
        // back: the first read of the memory gets the renderer's pictures
        // of the buffers, after it was given what was recorded. A copy
        // from the CPU goes into the memory as well, and the renderer's
        // pictures have it too.
        let mut v = glide_card(2);
        v.set_mirror(true);
        v.set_software_picture(false);
        let log = std::rc::Rc::default();
        v.set_gpu_link(Some(Box::new(Renderer(std::rc::Rc::clone(&log)))));
        v.set_gpu_reads(true);
        glide_frames(&mut v, 2);
        copy_into_back_buffer(&mut v, 30, DST_TILED | DST_CHROMA);
        let status = v.describe(Now::default());
        assert!(status["gpu_only_jobs"].as_u64().unwrap() >= 4, "{status}");
        assert_eq!(status["held_jobs"], 0, "{status}");
        assert_eq!(status["catch_ups"], 0, "{status}");
        assert!(log.borrow().is_empty(), "{:?}", log.borrow());
        let back = v.fbi.rgboffs[v.fbi.backbuf as usize] as usize / 2;
        assert_eq!(
            v.fbi.ram.get(back + 30 * 640 + 12),
            0x0101,
            "the copy is in memory"
        );
        let aux = v.fbi.auxoffs as usize / 2;
        let memory = v.frame_buffer();
        assert_eq!(memory.get(back + 30 * 640 + 12), 0x1234 ^ back as u16);
        assert_eq!(memory.get(aux + 5), 0x7777);
        let log = log.borrow();
        assert!(log[0].starts_with("run "), "{log:?}");
        assert!(log.iter().any(|l| l == "read None 0..480"), "{log:?}");
        assert_eq!(v.describe(Now::default())["gpu_read_backs"], 1);
        // Read once, the memory stays as it is until the renderer draws
        // again.
        v.frame_buffer();
        assert_eq!(v.describe(Now::default())["gpu_read_backs"], 1);
    }

    #[test]
    fn gpu_reads_go_over_the_jobs_held_from_before() {
        // Jobs held back before reads went to the renderer came before
        // those left to it, which drew them too: a read draws them first
        // and the renderer's pictures go over them, not the other way.
        let mut v = glide_card(2);
        v.set_mirror(true);
        v.set_software_picture(false);
        glide_frames(&mut v, 1);
        let held = v.describe(Now::default())["held_jobs"].as_u64().unwrap();
        assert!(held > 0, "{held}");
        let log: std::rc::Rc<std::cell::RefCell<Vec<String>>> = std::rc::Rc::default();
        v.set_gpu_link(Some(Box::new(Renderer(std::rc::Rc::clone(&log)))));
        v.set_gpu_reads(true);
        glide_frames(&mut v, 1);
        let aux = v.fbi.auxoffs as usize / 2;
        let memory = v.frame_buffer();
        assert_eq!(memory.get(aux + 5), 0x7777);
    }

    #[test]
    fn gpu_reads_read_back_only_the_rows_asked_for() {
        // A screen-to-screen copy reads back the rows of the buffer it
        // reads and writes, and nothing else; a read of the whole memory
        // afterwards reads the rest.
        let mut v = glide_card(2);
        v.set_mirror(true);
        v.set_software_picture(false);
        let log: std::rc::Rc<std::cell::RefCell<Vec<String>>> = std::rc::Rc::default();
        v.set_gpu_link(Some(Box::new(Renderer(std::rc::Rc::clone(&log)))));
        v.set_gpu_reads(true);
        glide_frames(&mut v, 2);
        let back = v.fbi.rgboffs[v.fbi.backbuf as usize];
        write(&mut v, BLT_ROP, 0xCCCC);
        write(&mut v, BLT_SRC_BASE_ADDR, back);
        write(&mut v, BLT_DST_BASE_ADDR, back);
        write(&mut v, BLT_XY_STRIDES, 1280 << 16 | 1280);
        write(&mut v, BLT_SRC_XY, 100 << 16 | 12);
        write(&mut v, BLT_DST_XY, 300 << 16 | 12);
        write(&mut v, BLT_SIZE, 2 << 16 | 3);
        write(&mut v, BLT_COMMAND, 1 << 31 | SCREEN_TO_SCREEN);
        let reads: Vec<String> = log
            .borrow()
            .iter()
            .filter(|l| l.starts_with("read"))
            .cloned()
            .collect();
        let key = Some(back / 2);
        assert_eq!(
            reads,
            [
                format!("read {key:?} 300..306"),
                format!("read {key:?} 100..106")
            ]
        );
        assert_eq!(
            v.fbi.ram.get(back as usize / 2 + 301 * 640 + 13),
            0x1234 ^ (back / 2) as u16
        );
        log.borrow_mut().clear();
        v.frame_buffer();
        let log = log.borrow();
        for rows in ["0..100", "106..300", "306..480"] {
            assert!(log.contains(&format!("read {key:?} {rows}")), "{log:?}");
        }
        assert!(log.contains(&"read None 0..480".to_string()), "{log:?}");
    }

    #[test]
    fn gpu_reads_of_the_lfb_read_back_the_rows_around_the_pixel() {
        // A game testing the depth of one pixel (3DMark 2000's lens
        // flares) gets two bands of 32 rows, not the whole buffer, and a
        // second read in them reads nothing back.
        let mut v = glide_card(2);
        v.set_mirror(true);
        v.set_software_picture(false);
        let log: std::rc::Rc<std::cell::RefCell<Vec<String>>> = std::rc::Rc::default();
        v.set_gpu_link(Some(Box::new(Renderer(std::rc::Rc::clone(&log)))));
        v.set_gpu_reads(true);
        glide_frames(&mut v, 2);
        v.reg[crate::voodoo::regs::LFB_MODE] = 2 << 6;
        assert_eq!(v.lfb_read(100 << 9 | 5) & 0xFFFF, 0x7777);
        assert_eq!(v.lfb_read(120 << 9 | 5) & 0xFFFF, 0x7777);
        let reads: Vec<String> = log
            .borrow()
            .iter()
            .filter(|l| l.starts_with("read"))
            .cloned()
            .collect();
        assert_eq!(reads, ["read None 96..160"]);
    }

    #[test]
    fn gpu_reads_count_the_rows_not_drawn_since_the_last_read() {
        // After a read of everything, a fastfill draws rows 20..40. A read
        // of depth rows 96..160 then reads only rows nothing drew since:
        // a cache would have had them. One of rows 0..64 reads the 20
        // drawn among them.
        let mut v = glide_card(2);
        v.set_mirror(true);
        v.set_software_picture(false);
        let log: std::rc::Rc<std::cell::RefCell<Vec<String>>> = std::rc::Rc::default();
        v.set_gpu_link(Some(Box::new(Renderer(std::rc::Rc::clone(&log)))));
        v.set_gpu_reads(true);
        glide_frames(&mut v, 2);
        v.frame_buffer();
        let count = |v: &Voodoo| {
            let status = v.describe(Now::default());
            [
                "gpu_read_rows",
                "gpu_partial_reads",
                "gpu_read_rows_drawn",
                "gpu_read_rows_undrawn",
            ]
            .map(|k| status[k].as_u64().unwrap())
        };
        let [rows, partial, drawn, undrawn] = count(&v);
        assert!(rows >= 3 * 480 && rows == drawn + undrawn, "{rows} {drawn}");
        assert_eq!((partial, undrawn), (0, 0), "the first reads read what was drawn");
        write(&mut v, CLIP_LEFT_RIGHT, 10 << 16 | 300);
        write(&mut v, CLIP_LOW_Y_HIGH_Y, 20 << 16 | 40);
        write(&mut v, FASTFILL_CMD, 0);
        v.reg[crate::voodoo::regs::LFB_MODE] = 2 << 6;
        v.lfb_read(100 << 9 | 5);
        let now = count(&v);
        assert_eq!(
            [now[0] - rows, now[1] - partial, now[2] - drawn, now[3] - undrawn],
            [64, 1, 0, 64]
        );
        v.lfb_read(30 << 9 | 5);
        let last = count(&v);
        assert_eq!([last[1] - now[1], last[2] - now[2]], [1, 20]);
        assert_eq!(last[0] - now[0], last[2] - now[2] + last[3] - now[3]);
    }

    #[test]
    fn cpu_to_screen_copies_land_after_the_drawing() {
        // A fastfill in the middle of a copy, over the rows it wrote: the
        // data word after it waits for it, the others find nothing queued.
        // With worker threads, and with the OpenGL renderer holding jobs
        // back, the memory ends up as without them.
        let mut gl = glide_card(2);
        gl.set_mirror(true);
        gl.set_software_picture(false);
        let mut threads = glide_card(2);
        let mut sw = glide_card(0);
        for v in [&mut gl, &mut threads, &mut sw] {
            glide_frames(v, 2);
            let back = v.fbi.rgboffs[v.fbi.backbuf as usize];
            write(v, BLT_ROP, 0xCCCC);
            write(v, BLT_DST_BASE_ADDR, back);
            write(v, BLT_XY_STRIDES, 1280 << 16);
            write(v, BLT_DST_XY, 30 << 16 | 12);
            write(v, BLT_SIZE, 2 << 16 | 3);
            write(v, BLT_COMMAND, 1 << 31 | 2 << 3 | CPU_TO_SCREEN);
            for i in 1..=6 {
                if i == 4 {
                    write(v, FASTFILL_CMD, 0);
                }
                write(v, BLT_DATA, 0x0101_0101 * i);
            }
        }
        let memory = sw.frame_buffer().to_bytes();
        let back = sw.fbi.rgboffs[sw.fbi.backbuf as usize] as usize / 2;
        let px = |x: usize, y: usize| sw.fbi.ram.get(back + y * 640 + x);
        // The fill covers the first rows, the rest of the copy lands on it.
        assert_eq!([px(12, 30), px(13, 31)], [px(20, 30), px(20, 31)]);
        assert_ne!(px(12, 30), 0x0101);
        assert_eq!([px(14, 31), px(15, 32)], [0x0404, 0x0606]);
        assert!(gl.frame_buffer().to_bytes() == memory);
        assert!(threads.frame_buffer().to_bytes() == memory);
    }

    #[test]
    fn a_register_written_during_a_copy_is_seen_by_its_next_word() {
        // The copy's first row may wait behind the drawing. Then bltRop
        // changes to one that reads the destination (XOR): the rows after
        // need what the drawing left there, so they wait for it, as
        // without the OpenGL renderer.
        let mut gl = glide_card(2);
        gl.set_mirror(true);
        gl.set_software_picture(false);
        let mut sw = glide_card(0);
        for v in [&mut gl, &mut sw] {
            glide_frames(v, 2);
            let back = v.fbi.rgboffs[v.fbi.backbuf as usize];
            write(v, BLT_ROP, 0xCCCC);
            write(v, BLT_DST_BASE_ADDR, back);
            write(v, BLT_XY_STRIDES, 1280 << 16);
            write(v, BLT_DST_XY, 30 << 16 | 12);
            write(v, BLT_SIZE, 2 << 16 | 3);
            write(v, BLT_COMMAND, 1 << 31 | 2 << 3 | CPU_TO_SCREEN);
            for i in 1..=6 {
                if i == 3 {
                    write(v, BLT_ROP, 0x6666);
                }
                write(v, BLT_DATA, 0x0101_0101 * i);
            }
        }
        assert!(gl.describe(Now::default())["catch_ups"].as_u64().unwrap() > 0);
        let memory = sw.frame_buffer().to_bytes();
        let back = sw.fbi.rgboffs[sw.fbi.backbuf as usize] as usize / 2;
        let px = |x: usize, y: usize| sw.fbi.ram.get(back + y * 640 + x);
        // The fastfill's colour, XORed with the copy's from the second row.
        let fill = px(20, 31);
        assert_eq!(
            [px(12, 30), px(12, 31), px(15, 32)],
            [0x0101, fill ^ 0x0303, fill ^ 0x0606]
        );
        assert!(gl.frame_buffer().to_bytes() == memory);
    }

    #[test]
    fn copies_keep_their_place_among_the_jobs_held_back() {
        // Copies into the back buffer with fastfills between them that
        // cover part of their rows, and the last copy longer than a batch
        // of the words held back (`WORDS_HELD`): the words and the jobs
        // land in the order they came, as without the OpenGL renderer and
        // without worker threads.
        let mut gl = glide_card(2);
        gl.set_mirror(true);
        gl.set_software_picture(false);
        let mut threads = glide_card(2);
        let mut sw = glide_card(0);
        let copies = [(0, 110), (20, 110), (150, 210)];
        for v in [&mut gl, &mut threads, &mut sw] {
            glide_frames(v, 2);
            let back = v.fbi.rgboffs[v.fbi.backbuf as usize];
            for (k, &(y, rows)) in copies.iter().enumerate() {
                write(v, BLT_ROP, 0xCCCC);
                write(v, BLT_DST_BASE_ADDR, back);
                write(v, BLT_XY_STRIDES, 1280 << 16);
                write(v, BLT_DST_XY, y << 16);
                write(v, BLT_SIZE, (rows - 1) << 16 | 639);
                write(v, BLT_COMMAND, 1 << 31 | 2 << 3 | CPU_TO_SCREEN);
                for i in 0..320 * rows {
                    let value = (2 * i + 7919 * k as u32) & 0xFFFF;
                    write(v, BLT_DATA, value * 0x0001_0001);
                }
                if k + 1 < copies.len() {
                    let top = y + 50;
                    write(v, CLIP_LEFT_RIGHT, 100 << 16 | 400);
                    write(v, CLIP_LOW_Y_HIGH_Y, top << 16 | (top + 100));
                    write(v, COLOR1, 0x0010_2030 * (k as u32 + 1));
                    write(v, FASTFILL_CMD, 0);
                }
            }
        }
        let status = gl.describe(Now::default());
        assert_eq!(status["catch_ups"], 0, "{status}");
        assert!(status["held_jobs"].as_u64().unwrap() > 0, "{status}");
        assert!(640 * copies[2].1 as usize > super::super::WORDS_HELD);
        let memory = sw.frame_buffer().to_bytes();
        let back = sw.fbi.rgboffs[sw.fbi.backbuf as usize] as usize / 2;
        let px = |x: usize, y: usize| sw.fbi.ram.get(back + y * 640 + x);
        // Columns 100 to 399: the second copy (rows 20 to 129) over the
        // first fill (rows 50 to 149), the second fill (rows 70 to 169)
        // over that copy, and the last copy (from row 150) over the fill.
        let value = |k: usize, x: usize, row: usize| (2 * (row * 320 + x / 2) + 7919 * k) as u16;
        assert_eq!(px(500, 10), value(0, 500, 10));
        assert_eq!(px(200, 60), value(1, 200, 40));
        let fill = px(200, 100);
        assert_ne!(fill, value(1, 200, 80));
        assert_eq!([px(100, 140), px(398, 70)], [fill, fill]);
        assert_eq!(px(200, 160), value(2, 200, 10));
        assert!(gl.frame_buffer().to_bytes() == memory);
        assert!(threads.frame_buffer().to_bytes() == memory);
    }

    #[test]
    fn monochrome_rows_start_on_a_new_word_or_byte() {
        // The leftmost pixel is each byte's top bit.
        for (format, second_row) in [(0u32, 0x0000_0080), (1, 0x0000_8000)] {
            let mut v = card();
            write(&mut v, BLT_XY_STRIDES, 2048 << 16);
            write(&mut v, BLT_COLOR, 0x0000_FFFF);
            // Two pixels wide, two rows: the first row's set, then the
            // second's first pixel set where its row starts.
            write(&mut v, BLT_SIZE, 1 << 16 | 1);
            write(&mut v, BLT_COMMAND, 1 << 31 | format << 3 | CPU_TO_SCREEN);
            if format == 0 {
                write(&mut v, BLT_DATA, 0xC0);
            }
            write(&mut v, BLT_DATA, if format == 0 { second_row } else { second_row | 0xC0 });
            let px = |x: usize, y: usize| v.fbi.ram.get(y * 1024 + x);
            assert_eq!([px(0, 0), px(1, 0), px(0, 1), px(1, 1)], [0xFFFF, 0xFFFF, 0xFFFF, 0], "format {format}");
        }
    }

    #[test]
    fn chroma_ranges_choose_the_raster_operation() {
        let mut v = card();
        // ROP 0 (neither in range): source; ROP 2 (source in range): keep.
        write(&mut v, BLT_ROP, 0xA << 8 | 0xC);
        write(&mut v, BLT_SRC_CHROMA_RANGE, 0x1234 << 16 | 0x1234);
        write(&mut v, BLT_XY_STRIDES, 2048 << 16);
        write(&mut v, BLT_SIZE, 0);
        for (x, color) in [(0, 0x1234), (1, 0x4321)] {
            write(&mut v, BLT_COLOR, color);
            write(&mut v, BLT_DST_XY, x);
            write(&mut v, BLT_COMMAND, 1 << 31 | SRC_CHROMA | RECT_FILL);
        }
        assert_eq!([v.fbi.ram.get(0), v.fbi.ram.get(1)], [0, 0x4321]);
    }
}

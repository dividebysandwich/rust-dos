//! The Voodoo2's 2D blitter: screen-to-screen copies, rectangle fills,
//! SGRAM page fills (how Glide 3 clears whole buffers) and CPU-to-screen
//! copies fed through bltData, with the source and destination chroma
//! range tests choosing among four raster operations. From 86Box's
//! `voodoo_v2_blit_start` and `voodoo_v2_blit_data`
//! (vid_voodoo_blitter.c), with the Voodoo2 specification's sizes (a
//! fill is |bltSizeX| + 1 pixels wide, as a copy is) and its byte
//! swizzling and transparent monochrome bits.

use super::regs::*;
use super::workers::Job;
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
        let command = self.blt_command();
        let mut rop = 0;
        if command & DST_CHROMA != 0 && in_range(self.reg[BLT_DST_CHROMA_RANGE], dst) {
            rop |= 1;
        }
        if command & SRC_CHROMA != 0 && in_range(self.reg[BLT_SRC_CHROMA_RANGE], src) {
            rop |= 2;
        }
        let value = mix(self.reg[BLT_ROP] >> (4 * rop), src, dst);
        self.fbi.ram.set(at, value);
        self.blt_mirror(at, value);
    }

    /// Tell the OpenGL renderer's recording about a pixel the blitter
    /// wrote, if it is in a buffer's picture.
    fn blt_mirror(&mut self, at: usize, value: u16) {
        if self.mirror.is_none() {
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
            if let Some(mirror) = &mut self.mirror {
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
        // After what is drawn, which may read or write the same memory.
        self.flush();
        self.mirror_sync();
        let (size_x, size_y) = (s12(self.reg[BLT_SIZE]), s12(self.reg[BLT_SIZE] >> 16));
        let (dx, dy) = (size_x.signum().max(0) * 2 - 1, size_y.signum().max(0) * 2 - 1);
        let (dst_x, dst_y) = ((self.reg[BLT_DST_XY] & 0x7FF) as i32, ((self.reg[BLT_DST_XY] >> 16) & 0x7FF) as i32);
        let (dst_base, dst_stride) = self.blt_dst();
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
            CPU_TO_SCREEN => {
                self.blt = super::blit::Blit { active: true, x: dst_x, y: dst_y, column: 0, rows: size_y.max(0) };
                return;
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
        let Some((first, end)) = self.sgram_run() else {
            return false;
        };
        let Some(rows) = self.whole_rows(first, end) else {
            return false;
        };
        let color = self.reg[BLT_COLOR] as u16;
        self.mirror_sync();
        self.mirror_fill(first, end, color);
        let rowpixels = self.fbi.rowpixels as usize;
        for (base, top, bottom) in rows {
            let access = self.access(|layout| super::backlog::rows(layout, base, (top, bottom)));
            let fb = self.fbi.ram.clone();
            let job = Job::Rows {
                fb,
                base,
                rowpixels,
                rows: (top, bottom),
                value: color,
            };
            self.submit(job, access);
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

    /// An SGRAM fill: whole 4 KB pages from bltDstXY's row (bits 24:16),
    /// columns of 64 bits (four pixels) from its column (bits 8:0), as
    /// many pages as bltSize bits 24:16 say and columns as bits 8:0, with
    /// bltColor and no clipping or raster operations.
    fn sgram_fill(&mut self) {
        let (dst, size) = (self.reg[BLT_DST_XY], self.reg[BLT_SIZE]);
        let (page, column) = ((dst >> 16) & 0x3FF, dst & 0x1FF);
        let (pages, columns) = ((size >> 16) & 0x3FF, size & 0x1FF);
        let color = self.reg[BLT_COLOR] as u16;
        let ram = &self.fbi.ram;
        let last = (column + columns).min(511);
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
        } else if let Some(mirror) = &mut self.mirror {
            // The same columns of several pages: a snapshot takes them.
            mirror.invalidate();
        }
    }

    /// Tell the OpenGL renderer's recording about an SGRAM fill of the
    /// words `first..end`: the rows of each buffer it covers, as fills, or
    /// a snapshot where it covers part of a row.
    fn mirror_fill(&mut self, first: u32, end: u32, color: u16) {
        if self.mirror.is_none() {
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
                if let Some(mirror) = &mut self.mirror {
                    mirror.invalidate();
                }
                return;
            }
            let (top, bottom) = ((start - base) / row, (stop - base) / row);
            if top < bottom {
                fills.push((base, colour, top, bottom));
            }
        }
        let (r, g, b) = ((color >> 11) as u32, ((color >> 5) & 0x3F) as u32, (color & 0x1F) as u32);
        let rgb = ((r << 3 | r >> 2) << 16) | ((g << 2 | g >> 4) << 8) | (b << 3 | b >> 2);
        if let Some(mirror) = &mut self.mirror {
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
        // After what is drawn: the first word of a copy waits for it, and
        // the words after find nothing queued (Moto Racer sends 40,000 a
        // frame for its sky).
        if self.writes_queue() {
            self.flush();
        }
        self.mirror_sync();
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
                self.blt_put(addr, p);
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
        self.display_dirty = true;
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

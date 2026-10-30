//! The linear frame buffer: the host writing pixels (colour, alpha,
//! depth) straight into the frame buffer in the formats lfbMode selects,
//! with or without the pixel pipeline, and reading them back. From
//! DOSBox-X's `lfb_w` and `lfb_r` (voodoo_emu.cpp).

use super::raster;
use super::regs::*;
use super::tables::{extract_1555, extract_555x, extract_565, extract_5551, extract_x555};
use super::workers::Word;
use super::{LOG_RESERVED, NONE, Voodoo};

/// What a write carries for each of its (up to) two pixels, four bits a
/// pixel.
const RGB: u32 = 1;
const ALPHA: u32 = 2;
const DEPTH: u32 = 4;
/// The depth in the upper half of a 32-bit write, for its one pixel.
const DEPTH_MSW: u32 = 8;

/// Where a run of dwords written to the card's window goes, when all of
/// them go to the frame buffer or the card's memory (`memory_run`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryRun {
    /// A Voodoo Graphics' or Voodoo2's linear frame buffer, whose writes
    /// go through its FIFO one by one.
    Fifo,
    /// A Banshee's 3D linear frame buffer, from this dword of the window
    /// (`lfb_run`).
    Lfb(u32),
    /// A Banshee's memory through BAR1, from this address (`linear_run`).
    Linear(u32),
}

/// The rows of a buffer `height` rows high that a read of row `row`
/// brings up to date: the band of 32 the row is in and the next, 32
/// being the height of a row of tiles.
fn lfb_band(row: u32, height: u32) -> (u32, u32) {
    let top = (row / 32 * 32).min(height);
    (top, (top + 64).min(height))
}

impl Voodoo {
    /// Where the dwords written to the `len` bytes of the window from
    /// `offset` on go, if all of them go to the frame buffer or the card's
    /// memory, where they change no register or the window's layout. A
    /// string move's writes there can go in one call, in order
    /// (`Bus::move_to_voodoo`).
    pub fn memory_run(&self, offset: u32, len: u32) -> Option<MemoryRun> {
        let end = offset.checked_add(len)?;
        if self.banshee.is_none() {
            return (offset >= 0x40_0000 && end <= 0x80_0000).then_some(MemoryRun::Fifo);
        }
        let lfb = self.lfb_run(offset, len).map(MemoryRun::Lfb);
        lfb.or_else(|| self.linear_run(offset, len).map(MemoryRun::Linear))
    }

    /// Where the `len` bytes of a Banshee's window from `offset` on are in
    /// its 3D engine's linear frame buffer, all of them: the dword
    /// `lfb_write` takes for the first.
    pub fn lfb_run(&self, offset: u32, len: u32) -> Option<u32> {
        let end = offset.checked_add(len)?;
        let lfb = 0x100_0000..=0x200_0000;
        (self.banshee.is_some() && lfb.contains(&offset) && lfb.contains(&end))
            .then_some((offset & 0xFF_FFFC) >> 2)
    }

    /// Where the `len` bytes of a Banshee's window from `offset` on are in
    /// its memory, all of them through BAR1 below the tile base, one
    /// after the other, and outside the command FIFOs' rings: the first
    /// byte's address, for `banshee_memory_write`.
    pub fn linear_run(&self, offset: u32, len: u32) -> Option<u32> {
        use super::banshee::LINEAR;
        let b = self.banshee.as_ref()?;
        let end = offset.checked_add(len)?;
        if offset < LINEAR || end > 2 * LINEAR {
            return None;
        }
        let addr = b.linear_run(offset - LINEAR, len)?;
        let ring = b
            .fifo
            .iter()
            .any(|f| f.enabled && f.base < addr + len && addr < f.end);
        (!ring).then_some(addr)
    }

    /// A write to the frame buffer at dword `offset` of the window: `data`,
    /// of which `mem_mask` selects the halves written (`lfb_w`).
    pub(crate) fn lfb_write(&mut self, offset: u32, data: u32, mem_mask: u32) {
        self.lfb_write_with(offset, data, mem_mask, &mut None);
    }

    /// Whole dwords written to the frame buffer from dword `offset` of
    /// the window on, one after the other, as `lfb_write` writes them:
    /// a string move into the linear frame buffer. Through the pixel
    /// pipeline, the drawing queued before is done and the pipeline's
    /// state read once for all of them, as nothing between the writes
    /// changes either.
    pub(crate) fn lfb_write_run(&mut self, offset: u32, data: impl IntoIterator<Item = u32>) {
        let mut pipeline = None;
        for (i, value) in (0..).zip(data) {
            self.lfb_write_with(offset.wrapping_add(i), value, u32::MAX, &mut pipeline);
        }
    }

    /// `lfb_write`, with the pipeline's state from an earlier write of
    /// the same run in `pipeline`, or put there.
    fn lfb_write_with(
        &mut self,
        offset: u32,
        mut data: u32,
        mut mem_mask: u32,
        pipeline: &mut Option<raster::RasterState>,
    ) {
        self.mirror_sync();
        let lfb_mode = self.reg[LFB_MODE];
        if lfb_mode & (1 << 12) != 0 {
            data = data.swap_bytes();
            mem_mask = mem_mask.swap_bytes();
        }
        if lfb_mode & (1 << 11) != 0 {
            data = data.rotate_left(16);
            mem_mask = mem_mask.rotate_left(16);
        }

        let za = self.reg[ZA_COLOR];
        let mut sw = [(za & 0xFFFF) as i32; 2];
        let mut sa = [(za >> 24); 2];
        let (mut sr, mut sg, mut sb) = ([0u32; 2], [0u32; 2], [0u32; 2]);
        let halves = [data & 0xFFFF, data >> 16];
        let mut offset = offset;
        let lanes = (lfb_mode >> 9) & 3;
        let swap = lanes & 1 != 0;
        let mut set_rgb = |pix: usize, (r, g, b): (u32, u32, u32)| {
            (sr[pix], sg[pix], sb[pix]) = if swap { (b, g, r) } else { (r, g, b) };
        };
        let mask = match lfb_mode & 0xF {
            // 16-bit colours, two pixels a dword.
            0 => {
                for (pix, &half) in halves.iter().enumerate() {
                    set_rgb(pix, extract_565(half));
                }
                offset <<= 1;
                RGB | RGB << 4
            }
            1 => {
                for (pix, &half) in halves.iter().enumerate() {
                    set_rgb(pix, if lanes < 2 { extract_x555(half) } else { extract_555x(half) });
                }
                offset <<= 1;
                RGB | RGB << 4
            }
            2 => {
                for pix in 0..2 {
                    let (a, rgb) = if lanes < 2 {
                        let (a, r, g, b) = extract_1555(halves[pix]);
                        (a, (r, g, b))
                    } else {
                        let (r, g, b, a) = extract_5551(halves[pix]);
                        (a, (r, g, b))
                    };
                    sa[pix] = a;
                    set_rgb(pix, rgb);
                }
                offset <<= 1;
                (RGB | ALPHA) | (RGB | ALPHA) << 4
            }
            // 32-bit colours, one pixel.
            4 => {
                set_rgb(0, if lanes < 2 { (data >> 16 & 0xFF, data >> 8 & 0xFF, data & 0xFF) } else { (data >> 24, data >> 16 & 0xFF, data >> 8 & 0xFF) });
                RGB
            }
            5 => {
                let bytes = [data >> 24, data >> 16 & 0xFF, data >> 8 & 0xFF, data & 0xFF];
                if lanes < 2 {
                    sa[0] = bytes[0];
                    set_rgb(0, (bytes[1], bytes[2], bytes[3]));
                } else {
                    set_rgb(0, (bytes[0], bytes[1], bytes[2]));
                    sa[0] = bytes[3];
                }
                RGB | ALPHA
            }
            // Depth and a 16-bit colour, one pixel.
            12 => {
                sw[0] = (data >> 16) as i32;
                set_rgb(0, extract_565(halves[0]));
                RGB | DEPTH_MSW
            }
            13 => {
                sw[0] = (data >> 16) as i32;
                set_rgb(0, if lanes < 2 { extract_x555(halves[0]) } else { extract_555x(halves[0]) });
                RGB | DEPTH_MSW
            }
            14 => {
                sw[0] = (data >> 16) as i32;
                if lanes < 2 {
                    let (a, r, g, b) = extract_1555(halves[0]);
                    sa[0] = a;
                    set_rgb(0, (r, g, b));
                } else {
                    let (r, g, b, a) = extract_5551(halves[0]);
                    sa[0] = a;
                    set_rgb(0, (r, g, b));
                }
                RGB | ALPHA | DEPTH_MSW
            }
            // Two 16-bit depths.
            15 => {
                sw = [halves[0] as i32, halves[1] as i32];
                offset <<= 1;
                DEPTH | DEPTH << 4
            }
            _ => return,
        };

        // A Banshee's rows are 2048 pixels apart, a Voodoo2's 1024.
        let (mut x, y) = if self.banshee.is_some() {
            ((offset & 0x7FF) as i32, ((offset >> 11) & 0x7FF) as i32)
        } else {
            ((offset & 0x3FF) as i32, ((offset >> 10) & 0x3FF) as i32)
        };
        let mut mask = mask;
        if mem_mask & 0xFFFF == 0 {
            mask &= !(0x0F - DEPTH_MSW);
        }
        if mem_mask & 0xFFFF_0000 == 0 {
            mask &= !(0xF0 + DEPTH_MSW);
        }

        let buffer = match (lfb_mode >> 4) & 3 {
            // A Banshee writes where colBufferAddr says.
            _ if self.banshee.is_some() => 0,
            0 => self.fbi.frontbuf,
            1 => self.fbi.backbuf,
            _ => {
                self.log_once(LOG_RESERVED, "[3DFX] A frame buffer write to a reserved buffer is ignored");
                return;
            }
        };
        let offs = self.fbi.rgboffs[buffer as usize];
        if offs == NONE {
            return;
        }
        let dest = offs as usize / 2;
        let fbz = self.reg[FBZ_MODE];

        if lfb_mode & (1 << 8) == 0 {
            // Straight into the buffers, or behind the jobs queued.
            let scry = if lfb_mode & (1 << 13) != 0 {
                (self.fbi.yorigin as i32 - y) & self.fbi.ymask
            } else {
                y
            };
            if scry < 0 {
                return;
            }
            // Right of the row or below the buffer, in other rows' memory,
            // which other workers draw: after them.
            if (x + 2 > self.fbi.rowpixels as i32 || scry >= self.fbi.height as i32) && self.writes_queue() {
                self.flush();
            }
            let queue = self.writes_queue();
            let mut words = [Word { row: scry as u16, at: 0, value: 0 }; 4];
            let mut queued = 0;
            // The pixels written, for the OpenGL renderer: X, and the
            // colour and auxiliary values.
            let mut written = [(0i32, None, None); 2];
            let mut count = 0;
            let ram = &self.fbi.ram;
            let (stride, aux_stride) = (self.fbi.stride, self.fbi.aux_stride);
            let color_row = dest + stride.row(scry as usize);
            let aux = self.fbi.auxoffs;
            let aux_row = aux as usize / 2 + aux_stride.row(scry as usize);
            let dither = fbz & (1 << 8) != 0;
            let tables = super::tables::tables();
            let lookup = if fbz & (1 << 11) == 0 { &tables.dither4 } else { &tables.dither2 };
            let alpha_planes = fbz & (1 << 18) != 0;
            let mut pix = 0;
            while mask != 0 {
                if mask & 0x0F != 0 {
                    let has_rgb = mask & RGB != 0;
                    let has_alpha = mask & ALPHA != 0 && alpha_planes;
                    let has_depth = mask & (DEPTH | DEPTH_MSW) != 0 && !alpha_planes;
                    let mut put = |at: usize, value: u16| {
                        if queue {
                            words[queued].at = at as u32;
                            words[queued].value = value;
                            queued += 1;
                        } else {
                            ram.set(at, value);
                        }
                    };
                    let mut color = None;
                    let at = color_row + stride.col(x as usize);
                    if has_rgb && at < ram.len() {
                        let (r, g, b) = (sr[pix], sg[pix], sb[pix]);
                        let value = if dither {
                            let at = ((y & 3) as usize) << 11 | ((x & 3) as usize) << 1;
                            (lookup[at | (r as usize) << 3] as u16) << 11
                                | (lookup[at | (g as usize) << 3 | 1] as u16) << 5
                                | lookup[at | (b as usize) << 3] as u16
                        } else {
                            ((r >> 3) << 11 | (g >> 2) << 5 | b >> 3) as u16
                        };
                        put(at, value);
                        color = Some(value);
                    }
                    let mut auxval = None;
                    let at = aux_row + aux_stride.col(x as usize);
                    if aux != NONE && at < ram.len() && (has_alpha || has_depth) {
                        let value = if has_alpha { sa[pix] as u16 } else { sw[pix] as u16 };
                        put(at, value);
                        auxval = Some(value);
                    }
                    self.stats.pixels_out += 1;
                    written[count] = (x, color, auxval);
                    count += 1;
                }
                x += 1;
                mask >>= 4;
                pix += 1;
            }
            if queued > 0 {
                self.queue_words(&words[..queued]);
            }
            for &(x, color, auxval) in &written[..count] {
                self.mirror_pixel(dest, x, scry, color, auxval);
            }
        } else {
            // Through the pixel pipeline, which reads the buffers.
            let st = match pipeline {
                Some(st) => st,
                None => {
                    self.flush();
                    pipeline.insert(self.raster_state(dest, 0))
                }
            };
            let mut stipple = self.reg[STIPPLE];
            let mut stats = self.stats;
            let mut pix = 0;
            let scry = raster::screen_y(st, y, fbz & (1 << 17) != 0);
            let mut written = [0i32; 2];
            let mut count = 0;
            while mask != 0 {
                if mask & 0x0F != 0 {
                    let color = sa[pix] << 24 | sr[pix] << 16 | sg[pix] << 8 | sb[pix];
                    raster::lfb_pixel(st, x, y, color, sw[pix], &mut stipple, &mut stats);
                    written[count] = x;
                    count += 1;
                }
                x += 1;
                mask >>= 4;
                pix += 1;
            }
            self.reg[STIPPLE] = stipple;
            self.stats = stats;
            // What the pipeline left, from the memory.
            for &x in &written[..count] {
                self.mirror_written(dest, x, scry, fbz & (1 << 10) != 0);
            }
        }
        self.mark_drawn(dest);
    }

    /// A read of the frame buffer at dword `offset` of the window: two
    /// 16-bit pixels of the buffer lfbMode selects (`lfb_r`).
    pub(crate) fn lfb_read(&self, offset: u32) -> u32 {
        let lfb_mode = self.reg[LFB_MODE];
        let (x, y) = if self.banshee.is_some() {
            (((offset << 1) & 0x7FE) as usize, ((offset >> 10) & 0x7FF) as i32)
        } else {
            (((offset << 1) & 0x3FE) as usize, ((offset >> 9) & 0x3FF) as i32)
        };
        let (offs, stride) = match (lfb_mode >> 6) & 3 {
            0 => (self.fbi.rgboffs[self.fbi.frontbuf as usize], self.fbi.stride),
            1 => (self.fbi.rgboffs[self.fbi.backbuf as usize], self.fbi.stride),
            2 => (self.fbi.auxoffs, self.fbi.aux_stride),
            _ => (NONE, self.fbi.stride),
        };
        if offs == NONE {
            return 0xFFFF_FFFF;
        }
        let scry = if lfb_mode & (1 << 13) != 0 {
            (self.fbi.yorigin as i32 - y) & self.fbi.ymask
        } else {
            y
        };
        if scry < 0 {
            return 0xFFFF_FFFF;
        }
        // The rows around the pixel read: a program reading one pixel
        // mostly reads its neighbours after it, and each read back from
        // the OpenGL renderer waits for the GPU. A game that tests a few
        // depths a frame (3DMark 2000's lens flares) then doesn't wait
        // for the whole buffer, and one that reads all of it reads it in
        // a few bands, the GPU idle after the first.
        let (top, bottom) = lfb_band(scry as u32, self.fbi.height);
        let (top, bottom) = (
            super::mirror::span(stride, top) as u32 * 2,
            super::mirror::span(stride, bottom) as u32 * 2,
        );
        self.catch_up_in(Some((
            offs.saturating_add(top),
            offs.saturating_add(bottom),
        )));
        let row = offs as usize / 2 + stride.row(scry as usize);
        let ram = &self.fbi.ram;
        let word = |x: usize| {
            let at = row + stride.col(x);
            if at < ram.len() { ram.get(at) as u32 } else { 0xFFFF }
        };
        if row + stride.col(x) >= ram.len() {
            return 0xFFFF_FFFF;
        }
        let mut data = word(x) | word(x + 1) << 16;
        if lfb_mode & (1 << 15) != 0 {
            data = data.rotate_left(16);
        }
        if lfb_mode & (1 << 16) != 0 {
            data = data.swap_bytes();
        }
        data
    }
}

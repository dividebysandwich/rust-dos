//! The linear frame buffer: the host writing pixels (colour, alpha,
//! depth) straight into the frame buffer in the formats lfbMode selects,
//! with or without the pixel pipeline, and reading them back. From
//! DOSBox-X's `lfb_w` and `lfb_r` (voodoo_emu.cpp).

use super::raster;
use super::regs::*;
use super::tables::{extract_1555, extract_555x, extract_565, extract_5551, extract_x555};
use super::{LOG_RESERVED, NONE, Voodoo};

/// What a write carries for each of its (up to) two pixels, four bits a
/// pixel.
const RGB: u32 = 1;
const ALPHA: u32 = 2;
const DEPTH: u32 = 4;
/// The depth in the upper half of a 32-bit write, for its one pixel.
const DEPTH_MSW: u32 = 8;

impl Voodoo {
    /// A write to the frame buffer at dword `offset` of the window: `data`,
    /// of which `mem_mask` selects the halves written (`lfb_w`).
    pub(crate) fn lfb_write(&mut self, offset: u32, mut data: u32, mut mem_mask: u32) {
        self.flush();
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
        let mut sa = [(za >> 24) as u32; 2];
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
                for pix in 0..2 {
                    set_rgb(pix, extract_565(halves[pix]));
                }
                offset <<= 1;
                RGB | RGB << 4
            }
            1 => {
                for pix in 0..2 {
                    set_rgb(pix, if lanes < 2 { extract_x555(halves[pix]) } else { extract_555x(halves[pix]) });
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

        let mut x = (offset & 0x3FF) as i32;
        let y = ((offset >> 10) & 0x3FF) as i32;
        let mut mask = mask;
        if mem_mask & 0xFFFF == 0 {
            mask &= !(0x0F - DEPTH_MSW);
        }
        if mem_mask & 0xFFFF_0000 == 0 {
            mask &= !(0xF0 + DEPTH_MSW);
        }

        let buffer = match (lfb_mode >> 4) & 3 {
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
        // The pixels written, for the OpenGL renderer: X, the buffer row,
        // and whether the auxiliary buffer's changed too.
        let mut written = [(0i32, 0i32, false); 2];
        let mut count = 0;

        if lfb_mode & (1 << 8) == 0 {
            // Straight into the buffers.
            let scry = if lfb_mode & (1 << 13) != 0 { (self.fbi.yorigin as i32 - y) & 0x3FF } else { y };
            let ram = &self.fbi.ram;
            let destmax = (self.fbi.mask as usize + 1 - offs as usize) / 2;
            let aux = self.fbi.auxoffs;
            let depthmax = if aux == NONE { 0 } else { (self.fbi.mask as usize + 1 - aux as usize) / 2 };
            let dither = fbz & (1 << 8) != 0;
            let tables = super::tables::tables();
            let lookup = if fbz & (1 << 11) == 0 { &tables.dither4 } else { &tables.dither2 };
            let mut bufoffs = scry as usize * self.fbi.rowpixels as usize + x as usize;
            let alpha_planes = fbz & (1 << 18) != 0;
            let mut pix = 0;
            while mask != 0 {
                if mask & 0x0F != 0 {
                    let has_rgb = mask & RGB != 0;
                    let has_alpha = mask & ALPHA != 0 && alpha_planes;
                    let has_depth = mask & (DEPTH | DEPTH_MSW) != 0 && !alpha_planes;
                    if has_rgb && bufoffs < destmax {
                        let (r, g, b) = (sr[pix], sg[pix], sb[pix]);
                        let color = if dither {
                            let at = ((y & 3) as usize) << 11 | ((x & 3) as usize) << 1;
                            (lookup[at | (r as usize) << 3] as u16) << 11
                                | (lookup[at | (g as usize) << 3 | 1] as u16) << 5
                                | lookup[at | (b as usize) << 3] as u16
                        } else {
                            ((r >> 3) << 11 | (g >> 2) << 5 | b >> 3) as u16
                        };
                        ram.set(dest + bufoffs, color);
                    }
                    if aux != NONE && bufoffs < depthmax {
                        let depth = aux as usize / 2 + bufoffs;
                        if has_alpha {
                            ram.set(depth, sa[pix] as u16);
                        }
                        if has_depth {
                            ram.set(depth, sw[pix] as u16);
                        }
                    }
                    self.stats.pixels_out += 1;
                    written[count] = (x, scry, has_alpha || has_depth);
                    count += 1;
                }
                bufoffs += 1;
                x += 1;
                mask >>= 4;
                pix += 1;
            }
        } else {
            // Through the pixel pipeline.
            let st = self.raster_state(dest, 0);
            let mut stipple = self.reg[STIPPLE];
            let mut stats = self.stats;
            let mut pix = 0;
            let scry = raster::screen_y(&st, y, fbz & (1 << 17) != 0);
            while mask != 0 {
                if mask & 0x0F != 0 {
                    let color = sa[pix] << 24 | sr[pix] << 16 | sg[pix] << 8 | sb[pix];
                    raster::lfb_pixel(&st, x, y, color, sw[pix], &mut stipple, &mut stats);
                    written[count] = (x, scry, fbz & (1 << 10) != 0);
                    count += 1;
                }
                x += 1;
                mask >>= 4;
                pix += 1;
            }
            self.reg[STIPPLE] = stipple;
            self.stats = stats;
        }
        for &(x, y, aux) in &written[..count] {
            self.mirror_pixel(dest, x, y, aux);
        }
        self.mark_drawn(dest);
    }

    /// A read of the frame buffer at dword `offset` of the window: two
    /// 16-bit pixels of the buffer lfbMode selects (`lfb_r`).
    pub(crate) fn lfb_read(&self, offset: u32) -> u32 {
        self.pool.flush();
        let lfb_mode = self.reg[LFB_MODE];
        let x = ((offset << 1) & 0x3FE) as usize;
        let y = ((offset >> 9) & 0x3FF) as i32;
        let offs = match (lfb_mode >> 6) & 3 {
            0 => self.fbi.rgboffs[self.fbi.frontbuf as usize],
            1 => self.fbi.rgboffs[self.fbi.backbuf as usize],
            2 => self.fbi.auxoffs,
            _ => NONE,
        };
        if offs == NONE {
            return 0xFFFF_FFFF;
        }
        let scry = if lfb_mode & (1 << 13) != 0 { (self.fbi.yorigin as i32 - y) & 0x3FF } else { y };
        let bufmax = (self.fbi.mask as usize + 1 - offs as usize) / 2;
        let bufoffs = scry as usize * self.fbi.rowpixels as usize + x;
        if bufoffs >= bufmax {
            return 0xFFFF_FFFF;
        }
        let base = offs as usize / 2 + bufoffs;
        let ram = &self.fbi.ram;
        let word = |at: usize| if at < ram.len() { ram.get(at) as u32 } else { 0xFFFF };
        let mut data = word(base) | word(base + 1) << 16;
        if lfb_mode & (1 << 15) != 0 {
            data = data.rotate_left(16);
        }
        if lfb_mode & (1 << 16) != 0 {
            data = data.swap_bytes();
        }
        data
    }
}

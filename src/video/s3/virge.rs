//! The S3 ViRGE's own engines behind its memory-mapped registers (the
//! "new MMIO" 64 KB at the linear frame buffer + 16 MB, and at A0000h with
//! CR53 bit 4): the 2D engine's BitBLT (A400h), 2D line (A800h) and 2D
//! polygon (AC00h) registers, the image transfer data port, the colour
//! pattern (A100h), the subsystem status and control register (8504h),
//! the advanced function control (850Ch), command DMA (8590h-859Ch) and
//! the streams processor's registers (8180h-81FFh), with the 3D engine
//! (`s3d`) at B000h-B5FFh. The Trio64's 8514-style engine (`engine`)
//! stays at its ports and packed registers.
//!
//! Every command runs completely when it is started, so the engines are
//! never busy and their FIFOs always empty.
//!
//! Ported from DOSBox-X's ViRGE emulation in vga_xga.cpp (the
//! `XGA_ViRGE_*` functions and their register decode), which the Windows
//! 3.1 and 98 ViRGE drivers ran against.

use super::s3d::{S3d, Target};
use super::streams::Streams;

/// One of the three 2D register groups. The registers with the same
/// mnemonic are one register at three addresses (databook 19.1), so all
/// three groups take them; each keeps its own CMD_SET, whose command
/// decides which register autoexecutes it.
#[derive(Clone, Debug, Default)]
pub struct Group {
    pub src_base: u32,
    pub dst_base: u32,
    pub right_clip: u32,
    pub left_clip: u32,
    pub bottom_clip: u32,
    pub top_clip: u32,
    pub src_stride: u32,
    pub dst_stride: u32,
    pub mono_pat: u64,
    pub mono_pat_bg: u32,
    pub mono_pat_fg: u32,
    pub src_bg: u32,
    pub src_fg: u32,
    pub command_set: u32,
    /// Stored plus one, as the register holds the width less one.
    pub rect_width: u32,
    pub rect_height: u32,
    pub rect_src_x: u32,
    pub rect_src_y: u32,
    pub rect_dst_x: u32,
    pub rect_dst_y: u32,
    // 2D line.
    pub lindrawend0: i32,
    pub lindrawend1: i32,
    pub lindrawxdelta: i32,
    pub lindrawstartx: i32,
    pub lindrawstarty: u32,
    pub lindrawcounty: u32,
    // 2D polygon: the edges' deltas and starts (S11.20), and the
    // accumulators.
    pub polyrdx: i32,
    pub polyrxstart: i32,
    pub polyldx: i32,
    pub polylxstart: i32,
    pub polyystart: u32,
    pub polyycount: u32,
    pub polyledge: i32,
    pub polyredge: i32,
    /// With autoexecute, the register (offset & 3FFh) whose write runs the
    /// command again, or 0.
    pub execute_on: u32,
}

crate::state_fields!(Group {
    src_base, dst_base, right_clip, left_clip, bottom_clip, top_clip, src_stride, dst_stride, mono_pat,
    mono_pat_bg, mono_pat_fg, src_bg, src_fg, command_set, rect_width, rect_height, rect_src_x, rect_src_y,
    rect_dst_x, rect_dst_y, lindrawend0, lindrawend1, lindrawxdelta, lindrawstartx, lindrawstarty,
    lindrawcounty, polyrdx, polyrxstart, polyldx, polylxstart, polyystart, polyycount, polyledge, polyredge,
    execute_on
});

impl Group {
    fn rop(&self) -> u8 {
        (self.command_set >> 17) as u8
    }

    /// Bytes per pixel of the command's format (bits 4-2): 8, 16 or 24
    /// bits, or 0 for the reserved ones.
    fn bypp(&self) -> u32 {
        match self.command_set >> 2 & 7 {
            0 => 1,
            1 => 2,
            2 => TRUECOLOR_BYPP,
            _ => 0,
        }
    }
}

/// The ViRGE's drivers work in 24 bit truecolour, three bytes a pixel.
const TRUECOLOR_BYPP: u32 = 3;

/// A BitBLT's image transfer from the processor in progress.
#[derive(Clone, Debug, Default)]
pub struct Transfer {
    pub active: bool,
    startx: u32,
    stopy: u32,
    src_stride: u32,
    /// Bytes left of the current source row, and pixels left to draw of it.
    src_xrem: u32,
    src_drem: u32,
    /// Up to eight bytes shifted in, and how many.
    buffer: u64,
    count: u8,
    /// Bytes of the next doubleword to skip.
    initskip: u8,
}

crate::state_fields!(Transfer { active, startx, stopy, src_stride, src_xrem, src_drem, buffer, count, initskip });

/// Command DMA (8590h-859Ch): a circular buffer in system memory the
/// engine reads register writes from.
#[derive(Clone, Debug, Default)]
pub struct CommandDma {
    pub base: u32,
    pub wp: u32,
    pub rp: u32,
    pub enable: u32,
    /// Doublewords left of the block being read, and the register the
    /// next one goes to (bit 31: the image transfer port).
    pub remain: u32,
    pub reg: u32,
}

crate::state_fields!(CommandDma { base, wp, rp, enable, remain, reg });

/// The ViRGE's engines and their status.
#[derive(Clone, Debug)]
pub struct Virge {
    /// BitBLT, 2D line and 2D polygon.
    pub groups: [Group; 3],
    /// The colour pattern, A100h-A1BFh: 64 pixels of 8, 16 or 24 bits.
    pub colorpat: [u32; 48],
    pub transfer: Transfer,
    /// Subsystem status (8504h read, interrupt bits 6-0) and control (the
    /// last written, enables in bits 13-7).
    pub subsys_stat: u32,
    pub subsys_ctl: u32,
    /// Advanced function control (850Ch): bit 0 enhanced functions, bit 1
    /// reset, bit 4 linear addressing (ORed with CR58 bit 4).
    pub advfunc: u32,
    pub dma: CommandDma,
    pub s3d: S3d,
    pub streams: Streams,
    /// Commands run, for the debugger: BitBLTs, rectangles, lines,
    /// polygons.
    pub counts: [u64; 4],
}

impl Default for Virge {
    fn default() -> Self {
        Self {
            groups: Default::default(),
            colorpat: [0; 48],
            transfer: Transfer::default(),
            subsys_stat: 0,
            subsys_ctl: 0,
            advfunc: 0,
            dma: CommandDma::default(),
            s3d: S3d::default(),
            streams: Streams::default(),
            counts: [0; 4],
        }
    }
}

crate::state_fields!(Virge { groups, colorpat, transfer, subsys_stat, subsys_ctl, advfunc, dma, s3d, streams } skip { counts });

const BITBLT: usize = 0;
const LINE2D: usize = 1;
const POLY2D: usize = 2;

/// Subsystem status bits.
pub const STAT_VSY: u32 = 0x01;
pub const STAT_S3D_DONE: u32 = 0x02;
pub const STAT_CMD_DMA_DONE: u32 = 0x20;

/// What a write asks of the bus beyond the engines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Drew into video memory.
    Drew,
    /// Read command DMA's buffer up to the write pointer.
    CommandDma,
    /// Linear addressing (850Ch bit 4) changed.
    Lfb,
}

/// ROP3: bit (P*4 + S*2 + D) of `rop` is the result for those pattern,
/// source and destination bits.
fn mix(src: u32, pat: u32, dst: u32, rop: u8) -> u32 {
    match rop {
        0x00 => 0,
        0x55 => !dst,
        0x5A => dst ^ pat,
        0x66 => dst ^ src,
        0x88 => dst & src,
        0xAA => dst,
        0xCC => src,
        0xEE => dst | src,
        0xF0 => pat,
        0xFF => 0xFFFF_FFFF,
        _ => {
            let mut r = 0;
            for bit in 0..8 {
                if rop & (1 << bit) != 0 {
                    let p = if bit & 4 != 0 { pat } else { !pat };
                    let s = if bit & 2 != 0 { src } else { !src };
                    let d = if bit & 1 != 0 { dst } else { !dst };
                    r |= p & s & d;
                }
            }
            r
        }
    }
}

impl Virge {
    /// Set status bits (VSY at each retrace, S3D DONE, command DMA done).
    /// The interrupt line isn't raised: software polls these.
    pub fn set_status(&mut self, bits: u32) {
        self.subsys_stat |= bits & 0x7F;
    }

    /// Subsystem status (8504h): 16 FIFO slots free, the engine idle and
    /// its FIFO empty, and the interrupt bits.
    pub fn status(&self) -> u32 {
        16 << 8 | 0x2000 | 0x40 | self.subsys_stat
    }

    /// The pattern pixel of the colour pattern at (`x`, `y`), in the
    /// BitBLT command's format.
    fn pat_pixel(&self, x: u32, y: u32) -> u32 {
        let i = (((y & 7) << 3) + (x & 7)) as usize;
        let byte = |n: usize| (self.colorpat.get(n / 4).copied().unwrap_or(0) >> (8 * (n % 4))) & 0xFF;
        match self.groups[BITBLT].command_set >> 2 & 7 {
            0 => byte(i),
            1 => byte(i * 2) | byte(i * 2 + 1) << 8,
            2 => byte(i * 3) | byte(i * 3 + 1) << 8 | byte(i * 3 + 2) << 16,
            _ => 0,
        }
    }

    fn pat_pixel_mono(&self, x: u32, y: u32) -> u32 {
        let g = &self.groups[BITBLT];
        let row = (g.mono_pat >> (8 * (y & 7))) as u8;
        if row & (0x80 >> (x & 7)) != 0 { g.mono_pat_fg } else { g.mono_pat_bg }
    }

    /// Write the register at offset `port` (`len` bytes of `val`) of the
    /// 64 KB window.
    pub fn write(&mut self, port: u16, val: u32, len: u8, t: &mut Target<'_>) -> Effect {
        let mut effect = Effect::None;
        let mut drew = false;
        if (0x8180..0x8200).contains(&port) {
            self.streams.write(port, val, len);
            return Effect::Drew;
        }
        match port {
            0x850C => {
                let nv = val & 0x13;
                if (nv ^ self.advfunc) & 0x10 != 0 {
                    effect = Effect::Lfb;
                }
                self.advfunc = nv;
            }
            0x8504 => {
                self.subsys_stat &= !(val & 0x7F);
                self.subsys_ctl = val & 0xFFFF;
                // Bits 15-14 = 10b reset the engines.
                if (val >> 14) & 3 == 2 {
                    self.transfer.active = false;
                    self.dma.remain = 0;
                    self.s3d.reset();
                }
            }
            0x8590 => self.dma.base = val & 0xFFFF_F002,
            0x8594 => {
                self.dma.wp = val & 0xFFFC;
                if val & 0x1_0000 != 0 {
                    effect = Effect::CommandDma;
                }
            }
            0x8598 => {
                self.dma.rp = val & 0xFFFC;
                self.dma.remain = 0;
            }
            0x859C => {
                self.dma.enable = val & 1;
                if self.dma.enable != 0 {
                    effect = Effect::CommandDma;
                }
            }
            0xA4D4 | 0xA8D4 | 0xACD4 => self.all(|g| g.src_base = val & 0x3F_FFF8),
            0xA4D8 | 0xA8D8 | 0xACD8 => self.all(|g| g.dst_base = val & 0x3F_FFF8),
            0xA4DC | 0xA8DC | 0xACDC => self.all(|g| {
                g.right_clip = val & 0x7FF;
                g.left_clip = val >> 16 & 0x7FF;
            }),
            0xA4E0 | 0xA8E0 | 0xACE0 => self.all(|g| {
                g.bottom_clip = val & 0x7FF;
                g.top_clip = val >> 16 & 0x7FF;
            }),
            0xA4E4 | 0xA8E4 | 0xACE4 => self.all(|g| {
                g.src_stride = val & 0xFF8;
                g.dst_stride = val >> 16 & 0xFF8;
            }),
            0xA4E8 | 0xA4EC | 0xACE8 | 0xACEC => {
                let shift = if port & 4 != 0 { 32 } else { 0 };
                self.all(|g| g.mono_pat = (g.mono_pat & !(0xFFFF_FFFFu64 << shift)) | (val as u64) << shift)
            }
            0xA4F0 | 0xACF0 => self.all(|g| g.mono_pat_bg = val & 0xFF_FFFF),
            0xA4F4 | 0xA8F4 | 0xACF4 => self.all(|g| g.mono_pat_fg = val & 0xFF_FFFF),
            0xA4F8 => self.all(|g| g.src_bg = val & 0xFF_FFFF),
            0xA4FC => self.all(|g| g.src_fg = val & 0xFF_FFFF),
            0xA500 | 0xA900 | 0xAD00 => {
                let n = ((port >> 10) & 3) as usize - 1;
                self.groups[n].command_set = val;
                self.transfer.active = false;
                if val & 1 != 0 {
                    self.defer(n);
                } else {
                    self.groups[n].execute_on = 0;
                    drew |= self.execute(n, t.vram);
                }
            }
            0xA504 | 0xA904 | 0xAD04 => self.all(|g| {
                g.rect_height = val & 0x7FF;
                g.rect_width = (val >> 16 & 0x7FF) + 1;
            }),
            0xA508 | 0xA908 | 0xAD08 => self.all(|g| {
                g.rect_src_y = val & 0x7FF;
                g.rect_src_x = val >> 16 & 0x7FF;
            }),
            0xA50C | 0xA90C | 0xAD0C => self.all(|g| {
                g.rect_dst_y = val & 0x7FF;
                g.rect_dst_x = val >> 16 & 0x7FF;
            }),
            0xA96C => {
                let g = &mut self.groups[LINE2D];
                g.lindrawend0 = (val >> 16) as u16 as i16 as i32;
                g.lindrawend1 = val as u16 as i16 as i32;
            }
            0xA970 => self.groups[LINE2D].lindrawxdelta = val as i32,
            0xA974 => self.groups[LINE2D].lindrawstartx = val as i32,
            0xA978 => self.groups[LINE2D].lindrawstarty = val & 0x3FFF,
            0xA97C => self.groups[LINE2D].lindrawcounty = val & 0x8000_3FFF,
            0xAD68 => self.groups[POLY2D].polyrdx = val as i32,
            0xAD6C => self.groups[POLY2D].polyrxstart = val as i32,
            0xAD70 => self.groups[POLY2D].polyldx = val as i32,
            0xAD74 => self.groups[POLY2D].polylxstart = val as i32,
            0xAD78 => self.groups[POLY2D].polyystart = val & 0x7FF,
            0xAD7C => self.groups[POLY2D].polyycount = val & 0x3000_07FF,
            p if p < 0x8000 || (0xD000..0xF000).contains(&p) => drew |= self.transfer_data(val, t.vram),
            p if (0xA100..0xA1C0).contains(&p) => self.colorpat[((p - 0xA100) >> 2) as usize] = val,
            p => {
                let mut s3d_drew = false;
                if self.s3d.write(p, val, len, t, &mut s3d_drew) && s3d_drew {
                    self.set_status(STAT_S3D_DONE);
                    drew = true;
                }
            }
        }
        // Autoexecute: the register that runs the armed command again.
        let n = match port & 0xFC00 {
            0xA400 => Some(BITBLT),
            0xA800 => Some(LINE2D),
            0xAC00 => Some(POLY2D),
            _ => None,
        };
        if let Some(n) = n {
            let on = self.groups[n].execute_on;
            if on != 0 && on == (port & 0x3FF) as u32 {
                self.transfer.active = false;
                drew |= self.execute(n, t.vram);
            }
        }
        if drew && effect == Effect::None { Effect::Drew } else { effect }
    }

    fn all(&mut self, f: impl Fn(&mut Group)) {
        self.groups.iter_mut().for_each(f);
    }

    /// Arm group `n`'s command to run at each write of its last register:
    /// DEST_XY for BitBLTs and rectangles, the Y count for lines and
    /// polygons.
    fn defer(&mut self, n: usize) {
        let g = &mut self.groups[n];
        let command = g.command_set >> 27 & 0x1F;
        g.execute_on = match (n, command) {
            (BITBLT, 0x00 | 0x02) => 0x10C,
            (LINE2D, 0x03) | (POLY2D, 0x05) => 0x17C,
            _ => 0,
        };
    }

    /// Run group `n`'s command; true if it drew.
    fn execute(&mut self, n: usize, vram: &mut [u8]) -> bool {
        let command = self.groups[n].command_set >> 27 & 0x1F;
        match (n, command) {
            (BITBLT, 0x00) => {
                self.counts[0] += 1;
                self.bitblt(vram)
            }
            (BITBLT, 0x02) => {
                self.counts[1] += 1;
                draw_rect(&mut self.groups[BITBLT], vram);
                true
            }
            (LINE2D, 0x03) => {
                self.counts[2] += 1;
                draw_line(&self.groups[LINE2D], vram);
                true
            }
            (POLY2D, 0x05) => {
                self.counts[3] += 1;
                draw_poly(&mut self.groups[POLY2D], vram);
                true
            }
            _ => false,
        }
    }

    /// A BitBLT: from video memory now, or from the processor through the
    /// image transfer port (bit 7).
    fn bitblt(&mut self, vram: &mut [u8]) -> bool {
        let g = &self.groups[BITBLT];
        let cmd = g.command_set;
        if cmd & 0x80 != 0 {
            let mut src_stride = if cmd & 0x40 != 0 {
                // Mono: a bit a pixel.
                g.rect_width.div_ceil(8)
            } else {
                g.rect_width * g.bypp().max(1)
            };
            src_stride = match cmd >> 10 & 3 {
                1 => (src_stride + 1) & !1,
                2 => (src_stride + 3) & !3,
                _ => src_stride,
            };
            self.transfer = Transfer {
                active: src_stride != 0,
                startx: g.rect_dst_x,
                stopy: g.rect_dst_y.wrapping_add(g.rect_height).wrapping_sub(1),
                src_stride,
                src_xrem: src_stride,
                src_drem: g.rect_width,
                buffer: 0,
                count: 0,
                initskip: (cmd >> 12 & 3) as u8,
            };
            return false;
        }
        // Video memory to video memory, always opaque: the TP bit is only
        // for image transfers.
        let g = &self.groups[BITBLT];
        if g.rect_width == 0 || g.rect_height == 0 {
            return false;
        }
        let (rx, dx, ex) = if cmd & (1 << 25) == 0 {
            let ex = g.rect_dst_x as i32 - (g.rect_width as i32 - 1);
            (u32::MAX, g.rect_dst_x, ex.max(0) as u32)
        } else {
            (1, g.rect_dst_x, g.rect_dst_x + g.rect_width - 1)
        };
        let (ry, dy, ey) = if cmd & (1 << 26) == 0 {
            let ey = g.rect_dst_y as i32 - (g.rect_height as i32 - 1);
            (u32::MAX, g.rect_dst_y, ey.max(0) as u32)
        } else {
            (1, g.rect_dst_y, g.rect_dst_y + g.rect_height - 1)
        };
        let sxa = g.rect_src_x.wrapping_sub(g.rect_dst_x);
        let sya = g.rect_src_y.wrapping_sub(g.rect_dst_y);
        let mono = cmd & 0x100 != 0;
        let (mut y, mut sy) = (dy, dy.wrapping_add(sya));
        let mut guard = 0u32;
        loop {
            let (mut x, mut sx) = (dx, dx.wrapping_add(sxa));
            loop {
                let src = read_src(g, vram, sx, sy);
                let dst = read_dst(g, vram, x, y);
                let pat = if mono { self.pat_pixel_mono(x, y) } else { self.pat_pixel(x, y) };
                draw_clipped(g, vram, x, y, mix(src, pat, dst, g.rop()));
                guard += 1;
                if x == ex || guard > 1 << 22 {
                    break;
                }
                sx = sx.wrapping_add(rx);
                x = x.wrapping_add(rx);
            }
            if y == ey || guard > 1 << 22 {
                break;
            }
            sy = sy.wrapping_add(ry);
            y = y.wrapping_add(ry);
        }
        true
    }

    /// A doubleword to the image transfer port: pixels of the BitBLT from
    /// the processor.
    fn transfer_data(&mut self, val: u32, vram: &mut [u8]) -> bool {
        if !self.transfer.active {
            return false;
        }
        let mut val = val as u64;
        let mut valbytes = 4u8;
        let tr = &mut self.transfer;
        if tr.initskip > 0 {
            let skip = tr.initskip.min(4);
            valbytes -= skip;
            val >>= 8 * skip as u64;
            tr.initskip = 0;
        }
        if tr.count > 0 {
            tr.buffer |= val << (8 * tr.count as u64);
            tr.count += valbytes;
        } else {
            tr.buffer = val;
            tr.count = valbytes;
        }
        let g = self.groups[BITBLT].clone();
        let cmd = g.command_set;
        let (mut x, mut y) = (g.rect_dst_x, g.rect_dst_y);
        let rop = g.rop();
        let mono_pattern = cmd & 0x100 != 0;
        let transparent = cmd & 0x200 != 0;
        if cmd & 0x40 != 0 {
            // A mono bitmap: a bit a pixel, foreground or background (or
            // nothing, transparent).
            while self.transfer.count > 0 {
                let byte = self.transfer.buffer as u8;
                let mut msk = 0x80u8;
                while msk != 0 {
                    if self.transfer.src_drem > 0 {
                        let set = byte & msk != 0;
                        if set || !transparent {
                            let src = if set { g.src_fg } else { g.src_bg };
                            let dst = read_dst(&g, vram, x, y);
                            let pat = if mono_pattern { self.pat_pixel_mono(x, y) } else { self.pat_pixel(x, y) };
                            draw_clipped(&g, vram, x, y, mix(src, pat, dst, rop));
                        }
                        self.transfer.src_drem -= 1;
                    }
                    msk >>= 1;
                    x = x.wrapping_add(1);
                }
                let tr = &mut self.transfer;
                tr.buffer >>= 8;
                tr.count -= 1;
                tr.src_xrem = tr.src_xrem.saturating_sub(1);
                if tr.src_xrem == 0 {
                    if y == tr.stopy {
                        *tr = Transfer::default();
                        break;
                    }
                    tr.src_drem = g.rect_width;
                    tr.src_xrem = tr.src_stride;
                    x = tr.startx;
                    y = y.wrapping_add(1);
                }
            }
        } else {
            let bypp = g.bypp().max(1) as u8;
            let bmask = match bypp {
                1 => 0xFF,
                2 => 0xFFFF,
                _ => 0xFF_FFFF,
            };
            // Transparent colour transfers leave the destination where the
            // pixel is the source foreground colour; not at 24 bits.
            let transparent = transparent && bypp <= 2;
            while self.transfer.count >= bypp {
                if self.transfer.src_drem > 0 {
                    let src = self.transfer.buffer as u32 & bmask;
                    if !transparent || src != g.src_fg & bmask {
                        let dst = read_dst(&g, vram, x, y);
                        let pat = if mono_pattern { self.pat_pixel_mono(x, y) } else { self.pat_pixel(x, y) };
                        draw_clipped(&g, vram, x, y, mix(src, pat, dst, rop));
                    }
                    self.transfer.src_drem -= 1;
                }
                x = x.wrapping_add(1);
                let tr = &mut self.transfer;
                tr.buffer >>= 8 * bypp as u64;
                tr.count -= bypp;
                tr.src_xrem = tr.src_xrem.saturating_sub(bypp as u32);
                if tr.src_xrem < bypp as u32 {
                    if y == tr.stopy {
                        *tr = Transfer::default();
                        break;
                    }
                    if tr.src_xrem > 0 {
                        // The row's padding: skip it, here or in the next
                        // doubleword.
                        if tr.src_xrem > tr.count as u32 {
                            // The rest of the padding is in the next
                            // doubleword: skip it there, and start the
                            // next row after it.
                            tr.initskip = (tr.src_xrem - tr.count as u32) as u8;
                            tr.count = 0;
                            tr.buffer = 0;
                            tr.src_drem = g.rect_width;
                            tr.src_xrem = tr.src_stride;
                            x = tr.startx;
                            y = y.wrapping_add(1);
                            break;
                        }
                        tr.buffer >>= 8 * tr.src_xrem as u64;
                        tr.count -= tr.src_xrem as u8;
                    }
                    tr.src_drem = g.rect_width;
                    tr.src_xrem = tr.src_stride;
                    x = tr.startx;
                    y = y.wrapping_add(1);
                }
            }
        }
        let g = &mut self.groups[BITBLT];
        g.rect_dst_x = x;
        g.rect_dst_y = y;
        true
    }

    /// Read back a 2D register (the doubleword holding `port`), the colour
    /// pattern, a 3D register, a streams register, or the status.
    pub fn read(&self, port: u16, len: u8) -> Option<u32> {
        let narrow = |v: u32| {
            let v = v >> ((port & 3) * 8);
            if len < 4 { v & ((1u32 << (len as u32 * 8)) - 1) } else { v }
        };
        if (0x8180..0x8200).contains(&port) {
            return Some(self.streams.read(port, len));
        }
        match port {
            0x8504 | 0x8505 => return Some(narrow(self.status())),
            0x850C..=0x850F => {
                // Always "not busy": 8 command FIFO slots.
                return Some(narrow(8 << 6 | 1 | 0x10 | (self.advfunc & 2)));
            }
            0x8590..=0x8593 => return Some(narrow(self.dma.base)),
            0x8594..=0x8597 => return Some(narrow(self.dma.wp)),
            0x8598..=0x859B => return Some(narrow(self.dma.rp)),
            0x859C..=0x859F => return Some(narrow(self.dma.enable)),
            _ => {}
        }
        if let Some(v) = self.s3d.read(port, len) {
            return Some(v);
        }
        let off = (port & 0x3FC) as u32;
        let blk = port & 0xFC00;
        if blk == 0xA000 {
            return (0x100..0x1C0).contains(&off).then(|| narrow(self.colorpat[((off - 0x100) >> 2) as usize]));
        }
        let n = match blk {
            0xA400 => BITBLT,
            0xA800 => LINE2D,
            0xAC00 => POLY2D,
            _ => return None,
        };
        let r = &self.groups[n];
        let v = match (n, off) {
            (_, 0x0D4) => r.src_base,
            (_, 0x0D8) => r.dst_base,
            (_, 0x0DC) => r.left_clip << 16 | r.right_clip,
            (_, 0x0E0) => r.top_clip << 16 | r.bottom_clip,
            (_, 0x0E4) => r.dst_stride << 16 | r.src_stride,
            (_, 0x0E8) => r.mono_pat as u32,
            (_, 0x0EC) => (r.mono_pat >> 32) as u32,
            (_, 0x0F0) => r.mono_pat_bg,
            (_, 0x0F4) => r.mono_pat_fg,
            (_, 0x0F8) => r.src_bg,
            (_, 0x0FC) => r.src_fg,
            (_, 0x100) => r.command_set,
            (_, 0x104) => (r.rect_width.wrapping_sub(1) & 0x7FF) << 16 | r.rect_height,
            (_, 0x108) => r.rect_src_x << 16 | r.rect_src_y,
            (_, 0x10C) => r.rect_dst_x << 16 | r.rect_dst_y,
            (LINE2D, 0x16C) => (r.lindrawend0 as u32 & 0xFFFF) << 16 | (r.lindrawend1 as u32 & 0xFFFF),
            (LINE2D, 0x170) => r.lindrawxdelta as u32,
            (LINE2D, 0x174) => r.lindrawstartx as u32,
            (LINE2D, 0x178) => r.lindrawstarty,
            (LINE2D, 0x17C) => r.lindrawcounty,
            (POLY2D, 0x168) => r.polyrdx as u32,
            (POLY2D, 0x16C) => r.polyrxstart as u32,
            (POLY2D, 0x170) => r.polyldx as u32,
            (POLY2D, 0x174) => r.polylxstart as u32,
            (POLY2D, 0x178) => r.polyystart,
            (POLY2D, 0x17C) => r.polyycount,
            _ => return None,
        };
        Some(narrow(v))
    }

    pub fn reset(&mut self) {
        let streams = std::mem::take(&mut self.streams);
        *self = Virge { streams, ..Virge::default() };
    }

    /// Command DMA's buffer: its base and the offset mask (4 or 64 KB).
    pub fn dma_buffer(&self) -> (u32, u32) {
        let big = self.dma.base & 2 != 0;
        let mask = if big { 0xFFFC } else { 0x0FFC };
        (self.dma.base & if big { 0xFFFF_0000 } else { 0xFFFF_F000 }, mask)
    }

    /// The next doubleword of command DMA taken from its buffer: where it
    /// goes (a register offset, or None for a header or the end).
    pub fn dma_step(&mut self, data: u32) -> Option<u16> {
        if self.dma.remain == 0 {
            // A header: the count (bits 15-0), the register (bits 29-16,
            // in doublewords) or image data (bit 31).
            self.dma.remain = data & 0xFFFF;
            self.dma.reg = if data & 0x8000_0000 != 0 { 0x8000_0000 } else { (data >> 16 & 0x3FFF) << 2 };
            return None;
        }
        self.dma.remain -= 1;
        if self.dma.reg & 0x8000_0000 != 0 {
            return Some(0);
        }
        let reg = self.dma.reg as u16 & 0xFFFC;
        self.dma.reg = (self.dma.reg + 4) & 0xFFFC;
        Some(reg)
    }
}

fn pixel_addr(base: u32, stride: u32, x: u32, y: u32, bypp: u32) -> u32 {
    y.wrapping_mul(stride).wrapping_add(x.wrapping_mul(bypp)).wrapping_add(base)
}

fn read_px(vram: &[u8], addr: u32, bypp: u32) -> u32 {
    let at = addr as usize;
    if bypp == 0 || at >= vram.len() {
        return 0;
    }
    (0..bypp as usize).map(|i| (vram.get(at + i).copied().unwrap_or(0) as u32) << (8 * i)).sum()
}

fn read_src(g: &Group, vram: &[u8], x: u32, y: u32) -> u32 {
    let bypp = g.bypp();
    read_px(vram, pixel_addr(g.src_base, g.src_stride, x, y, bypp), bypp)
}

fn read_dst(g: &Group, vram: &[u8], x: u32, y: u32) -> u32 {
    let bypp = g.bypp();
    read_px(vram, pixel_addr(g.dst_base, g.dst_stride, x, y, bypp), bypp)
}

/// Draw a pixel, if drawing is on (bit 5).
fn draw(g: &Group, vram: &mut [u8], x: u32, y: u32, c: u32) {
    if g.command_set & 0x20 == 0 {
        return;
    }
    let bypp = g.bypp();
    let at = pixel_addr(g.dst_base, g.dst_stride, x, y, bypp) as usize;
    if bypp == 0 || at >= vram.len() {
        return;
    }
    for i in 0..bypp as usize {
        if let Some(b) = vram.get_mut(at + i) {
            *b = (c >> (8 * i)) as u8;
        }
    }
}

/// Draw a pixel inside the clip rectangle, with hardware clipping on
/// (bit 1).
fn draw_clipped(g: &Group, vram: &mut [u8], x: u32, y: u32, c: u32) {
    if g.command_set & 2 == 0 || (x >= g.left_clip && x <= g.right_clip && y >= g.top_clip && y <= g.bottom_clip) {
        draw(g, vram, x, y, c);
    }
}

/// A rectangle fill with the mono pattern. The other operations than
/// BitBLT force a pattern in the ROP to the pattern foreground colour
/// (databook, Command Set register); Windows 3.1's drivers fill solid
/// rectangles with ROP F0h.
fn draw_rect(g: &mut Group, vram: &mut [u8]) {
    if g.rect_width == 0 || g.rect_height == 0 {
        return;
    }
    let (mut bex, mut bey) = (g.rect_dst_x, g.rect_dst_y);
    let (mut enx, mut eny) = (bex + g.rect_width - 1, bey + g.rect_height - 1);
    if g.command_set & 2 != 0 {
        bex = bex.max(g.left_clip);
        bey = bey.max(g.top_clip);
        enx = enx.min(g.right_clip);
        eny = eny.min(g.bottom_clip);
    }
    let rop = g.rop();
    let transparent = g.command_set & 0x200 != 0;
    for y in bey..=eny {
        let mut rb = (g.mono_pat >> (8 * ((y.wrapping_sub(g.rect_dst_y)) & 7))) as u8;
        if bex != g.rect_dst_x {
            rb = rb.rotate_left(bex.wrapping_sub(g.rect_dst_x) & 7);
        }
        for x in bex..=enx {
            let set = rb & 0x80 != 0;
            if set || !transparent {
                let src = if set { g.mono_pat_fg } else { g.mono_pat_bg };
                let dst = read_dst(g, vram, x, y);
                draw(g, vram, x, y, mix(src, g.mono_pat_fg, dst, rop));
            }
            rb = rb.rotate_left(1);
        }
    }
    g.rect_dst_x = enx.wrapping_add(1);
    g.rect_dst_y = eny.wrapping_add(1);
}

/// A 2D line, drawn bottom up: X start and delta S11.20, the first and
/// last pixel's X (to leave out the ends of a polyline's segments).
fn draw_line(g: &Group, vram: &mut [u8]) {
    let xdir: i32 = if g.lindrawcounty & 0x8000_0000 != 0 { 1 } else { -1 };
    let mut ycount = (g.lindrawcounty & 0x1FFF) as i32;
    let mut y = (g.lindrawstarty & 0x1FFF) as i32;
    let mut xf = g.lindrawstartx;
    let xdelta = g.lindrawxdelta;
    let (mut xend, mut xstart) = (g.lindrawend1, g.lindrawend0);
    let rop = g.rop();
    let mut plot = |x: i32, y: i32| {
        let (x, y) = (x as u32, y as u32);
        let dst = read_dst(g, vram, x, y);
        draw_clipped(g, vram, x, y, mix(0, g.mono_pat_fg, dst, rop));
    };
    let mut x = xstart;
    if ycount <= 1 {
        // A horizontal line, xstart to xend (Windows 3.1's drivers).
        for _ in 0..4096 {
            if (xdir > 0 && x > xend) || (xdir < 0 && x < xend) {
                break;
            }
            plot(x, y);
            x += xdir;
        }
    } else if (-(1 << 20)..=(1 << 20)).contains(&xdelta) {
        // Y major. Leave out the first pixel if it is before xstart, the
        // last if it is past xend: XOR polylines need their joints drawn
        // once.
        x = xf >> 20;
        if (xdir > 0 && x < xstart) || (xdir < 0 && x > xstart) {
            xf = xf.wrapping_add(xdelta);
            ycount -= 1;
            y -= 1;
        }
        while ycount > 0 {
            x = xf >> 20;
            if ycount == 1 && ((xdir > 0 && x > xend) || (xdir < 0 && x < xend)) {
                break;
            }
            plot(x, y);
            xf = xf.wrapping_add(xdelta);
            y -= 1;
            ycount -= 1;
        }
    } else if xdelta >= 0 {
        // X major, leftwards (bottom up).
        while ycount > 0 {
            let xto = xf >> 20;
            while x <= xto && x - xstart < 8192 {
                if x >= xstart && x <= xend {
                    plot(x, y);
                }
                x += 1;
            }
            xf = xf.wrapping_add(xdelta);
            y -= 1;
            ycount -= 1;
        }
    } else {
        // X major, rightwards.
        std::mem::swap(&mut xstart, &mut xend);
        while ycount > 0 {
            let xto = xf >> 20;
            while x >= xto && xend - x < 8192 {
                if x >= xstart && x <= xend {
                    plot(x, y);
                }
                x -= 1;
            }
            xf = xf.wrapping_add(xdelta);
            y -= 1;
            ycount -= 1;
        }
    }
}

/// A 2D polygon's trapezoid: every scanline between the left and right
/// edge accumulators, bottom up; PYCNT bits 29 and 28 reload them.
fn draw_poly(g: &mut Group, vram: &mut [u8]) {
    if g.polyycount & (1 << 28) != 0 {
        g.polyredge = g.polyrxstart;
    }
    if g.polyycount & (1 << 29) != 0 {
        g.polyledge = g.polylxstart;
    }
    let mut ycount = (g.polyycount & 0x7FF) as i32;
    let mut y = g.polyystart & 0x7FF;
    let rop = g.rop();
    while ycount > 0 {
        let (mut x, mut xend) = (g.polyledge >> 20, g.polyredge >> 20);
        if x > xend {
            std::mem::swap(&mut x, &mut xend);
        }
        xend = xend.min(x + 4095);
        for x in x.max(0)..=xend {
            let dst = read_dst(g, vram, x as u32, y);
            draw_clipped(g, vram, x as u32, y, mix(0, g.mono_pat_fg, dst, rop));
        }
        g.polyledge = g.polyledge.wrapping_add(g.polyldx);
        g.polyredge = g.polyredge.wrapping_add(g.polyrdx);
        y = y.wrapping_sub(1) & 0x7FF;
        ycount -= 1;
    }
    g.polyystart = y;
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRIDE: u32 = 1024;

    fn palette(_: u8) -> (u8, u8, u8) {
        (0, 0, 0)
    }

    fn write(v: &mut Virge, vram: &mut [u8], port: u16, value: u32) -> Effect {
        let mut t = Target { vram, palette: &palette };
        v.write(port, value, 4, &mut t)
    }

    /// 8 bits per pixel, drawing on, the ROP.
    fn cmd(command: u32, rop: u32) -> u32 {
        command << 27 | rop << 17 | 0x20
    }

    fn setup(v: &mut Virge, vram: &mut [u8]) {
        write(v, vram, 0xA4D4, 0);
        write(v, vram, 0xA4D8, 0);
        write(v, vram, 0xA4E4, STRIDE << 16 | STRIDE);
    }

    #[test]
    fn a_rectangle_fills_with_the_pattern_foreground() {
        let mut vram = vec![0u8; 1 << 20];
        let mut v = Virge::default();
        setup(&mut v, &mut vram);
        write(&mut v, &mut vram, 0xA4F4, 0x5A);
        // 4 wide (less one), 3 high, at (2, 1).
        write(&mut v, &mut vram, 0xA504, 3 << 16 | 3);
        write(&mut v, &mut vram, 0xA50C, 2 << 16 | 1);
        assert_eq!(write(&mut v, &mut vram, 0xA500, cmd(2, 0xF0)), Effect::Drew);
        let at = |x: u32, y: u32| vram[(y * STRIDE + x) as usize];
        assert_eq!((at(2, 1), at(5, 3)), (0x5A, 0x5A));
        assert_eq!((at(1, 1), at(6, 1), at(2, 0), at(2, 4)), (0, 0, 0, 0));
        assert_eq!(v.counts[1], 1);
    }

    #[test]
    fn autoexecute_fills_again_at_each_destination() {
        let mut vram = vec![0u8; 1 << 20];
        let mut v = Virge::default();
        setup(&mut v, &mut vram);
        write(&mut v, &mut vram, 0xA4F4, 0x11);
        write(&mut v, &mut vram, 0xA504, 1);
        // Armed: nothing yet.
        write(&mut v, &mut vram, 0xA500, cmd(2, 0xF0) | 1);
        assert!(vram.iter().all(|&b| b == 0));
        write(&mut v, &mut vram, 0xA50C, 7 << 16 | 2);
        write(&mut v, &mut vram, 0xA50C, 9 << 16 | 4);
        assert_eq!(vram[(2 * STRIDE + 7) as usize], 0x11);
        assert_eq!(vram[(4 * STRIDE + 9) as usize], 0x11);
        assert_eq!(v.counts[1], 2);
    }

    #[test]
    fn a_bitblt_copies_video_memory_with_the_rop() {
        let mut vram = vec![0u8; 1 << 20];
        vram[..4].copy_from_slice(&[0x10, 0x11, 0x12, 0x13]);
        vram[STRIDE as usize * 5 + 20] = 0xFF;
        let mut v = Virge::default();
        setup(&mut v, &mut vram);
        write(&mut v, &mut vram, 0xA504, 3 << 16 | 1);
        write(&mut v, &mut vram, 0xA508, 0);
        write(&mut v, &mut vram, 0xA50C, 20 << 16 | 5);
        // Left to right, top down, source XOR destination.
        write(&mut v, &mut vram, 0xA500, cmd(0, 0x66) | 1 << 25 | 1 << 26);
        let row = &vram[STRIDE as usize * 5 + 20..][..4];
        assert_eq!(row, [0xEF, 0x11, 0x12, 0x13]);
    }

    #[test]
    fn a_mono_image_transfer_expands_bits_to_colours() {
        let mut vram = vec![0u8; 1 << 20];
        let mut v = Virge::default();
        setup(&mut v, &mut vram);
        write(&mut v, &mut vram, 0xA4F8, 0x22);
        write(&mut v, &mut vram, 0xA4FC, 0x77);
        // 8 wide, 2 high: a byte a row, rows doubleword aligned.
        write(&mut v, &mut vram, 0xA504, 7 << 16 | 2);
        write(&mut v, &mut vram, 0xA50C, 0);
        write(&mut v, &mut vram, 0xA500, cmd(0, 0xCC) | 0x80 | 0x40 | 2 << 10);
        assert!(v.transfer.active);
        write(&mut v, &mut vram, 0x0000, 0xA5);
        write(&mut v, &mut vram, 0x0000, 0x0F);
        assert!(!v.transfer.active);
        let row = |y: usize| vram[y * STRIDE as usize..][..8].to_vec();
        assert_eq!(row(0), [0x77, 0x22, 0x77, 0x22, 0x22, 0x77, 0x22, 0x77]);
        assert_eq!(row(1), [0x22, 0x22, 0x22, 0x22, 0x77, 0x77, 0x77, 0x77]);
    }

    #[test]
    fn status_and_registers_read_back() {
        let mut vram = vec![0u8; 1 << 16];
        let mut v = Virge::default();
        write(&mut v, &mut vram, 0xA8E4, 0x0280_0140);
        // Shared: the BitBLT group sees the stride written for lines.
        assert_eq!(v.read(0xA4E4, 4), Some(0x0280_0140));
        assert_eq!(v.read(0xA4E6, 2), Some(0x0280));
        // Idle, 16 FIFO slots; VSY set and cleared by writing it.
        v.set_status(STAT_VSY);
        assert_eq!(v.read(0x8504, 2), Some(0x3041));
        write(&mut v, &mut vram, 0x8504, STAT_VSY);
        assert_eq!(v.read(0x8504, 1), Some(0x40));
        assert_eq!(v.read(0x8505, 1), Some(0x30));
        // Linear addressing on asks the bus to move the frame buffer.
        assert_eq!(write(&mut v, &mut vram, 0x850C, 0x10), Effect::Lfb);
    }

    #[test]
    fn rop3_combines_pattern_source_and_destination() {
        // DPSxx: all three XORed (96h), and PSa (C0h).
        assert_eq!(mix(0b1100, 0b1010, 0b0110, 0x96), 0b1100 ^ 0b1010 ^ 0b0110);
        assert_eq!(mix(0b1100, 0b1010, 0b0110, 0xC0), 0b1000);
        assert_eq!(mix(1, 2, 3, 0x88), 1);
    }
}

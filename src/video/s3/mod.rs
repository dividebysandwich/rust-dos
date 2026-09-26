//! The S3 Trio64 (86C764) of `machine=svga_s3`: the chip's extended CRTC
//! and sequencer registers as S3's drivers program them, the modes its
//! registers describe, the linear frame buffer and memory-mapped I/O they
//! place, its hardware cursor, and its graphics engine (`engine`).
//!
//! The registers follow DOSBox-X's S3 Trio64 (vga_s3.cpp), whose values
//! Windows 95's S3 driver was installed against: chip ID E1h and device
//! 8811h, 4 MB of video memory. The chip's 4 MB are `Vbe::vram`; the
//! standard VGA modes keep the VGA's own planes, as on the plain SVGA.

pub mod engine;

use super::crt::CrtTiming;
use super::vga::VgaCard;

/// What the extended registers describe for the display: the picture in
/// linear video memory, and its bits per pixel (8 through the DAC, 15, 16,
/// 24 or 32 direct colour), or None for the VGA's own modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub width: u16,
    pub height: u16,
    pub bpp: u8,
}

/// The chip's own registers.
#[derive(Clone, Debug)]
pub struct S3 {
    /// CRTC registers 19h-7Fh, by index.
    crtc: [u8; 0x80],
    /// Sequencer registers 08h-1Fh, by index.
    seq: [u8; 0x20],
    /// The next byte of SR17's reading sequence.
    sr17: u8,
    /// Bits 8-9 of the logical screen width (the CRTC Offset register's
    /// high bits), from CR43 bit 2 or CR51 bits 4-5, whichever came last.
    pub offset_high: u8,
    /// The hardware cursor's colour stacks (CR4A, CR4B), each written byte
    /// by byte from a position reading CR45 resets.
    pub cursor_fg: [u8; 4],
    pub cursor_bg: [u8; 4],
    fg_at: u8,
    bg_at: u8,
}

impl Default for S3 {
    fn default() -> Self {
        Self::new()
    }
}

/// CR36: 4 MB of fast page mode memory.
const MEMORY_CONFIG: u8 = 0x1A;

impl S3 {
    pub fn new() -> Self {
        let mut s3 = Self {
            crtc: [0; 0x80],
            seq: [0; 0x20],
            sr17: 0,
            offset_high: 0,
            cursor_fg: [0; 4],
            cursor_bg: [0; 4],
            fg_at: 0,
            bg_at: 0,
        };
        // The linear frame buffer's window where DOSBox-X's BIOS puts it,
        // E0000000h, 4 MB, not enabled.
        s3.crtc[0x58] = 0x03;
        s3.crtc[0x59] = 0xE0;
        s3.crtc[0x5A] = 0x00;
        s3.crtc[0x6B] = 0xE0;
        // Clock synthesizer defaults (the Trio64's reset values).
        s3.seq[0x10] = 0x42;
        s3.seq[0x11] = 0x3E;
        s3.seq[0x12] = 0x40;
        s3.seq[0x13] = 0x3E;
        s3
    }

    /// Unlock the registers, as the BIOS does before it sets a mode: CR38
    /// and CR39 for the CRTC's, SR08 for the sequencer's.
    pub fn unlock(&mut self) {
        self.crtc[0x38] = 0x48;
        self.crtc[0x39] = 0xA5;
        self.seq[0x08] = 0x06;
    }

    pub fn crtc(&self, index: u8) -> u8 {
        self.crtc.get(index as usize).copied().unwrap_or(0)
    }

    /// Read a CRTC register from 19h up. `bank` is the window's bank and
    /// `start_high` bits 16-20 of the display start.
    pub fn read_crtc(&mut self, index: u8, vga: &VgaCard, bank: u32, start_high: u8) -> u8 {
        match index {
            // The attribute controller's index and whether its palette
            // address source bit is set.
            0x24 | 0x26 => 0x20 | (vga.attribute_index & 0x1F),
            // Device ID (8811h), revision, chip ID.
            0x2D => 0x88,
            0x2E => 0x11,
            0x2F => 0x00,
            0x30 => 0xE1,
            0x35 => self.crtc[0x35] & 0xF0 | (bank & 0x0F) as u8,
            0x36 => MEMORY_CONFIG,
            0x37 => 0x2B,
            // Not interlaced.
            0x42 => 0x0D,
            0x43 => self.crtc[0x43] | (self.offset_high & 1) << 2,
            // Reading the cursor mode starts both colour stacks over.
            0x45 => {
                self.fg_at = 0;
                self.bg_at = 0;
                self.crtc[0x45] | 0xA0
            }
            0x4A => self.cursor_fg[self.fg_at as usize & 3],
            0x4B => self.cursor_bg[self.bg_at as usize & 3],
            0x51 => {
                (start_high >> 2 & 0x03) | ((bank >> 4) as u8 & 0x03) << 2 | (self.offset_high & 3) << 4 | self.crtc[0x51] & 0xC0
            }
            0x69 => start_high & 0x1F,
            0x6A => (bank & 0x7F) as u8,
            0x19..=0x7F => self.crtc[index as usize],
            _ => 0,
        }
    }

    /// Write a CRTC register from 19h up; what else it changes, the bank or
    /// the display start, comes back for the caller to apply.
    pub fn write_crtc(&mut self, index: u8, value: u8, bank: &mut u32, start_high: &mut u8) {
        let i = index as usize;
        match index {
            // Bank bits 0-3, with CR38 unlocked.
            0x35 => {
                if self.crtc[0x38] == 0x48 {
                    self.crtc[0x35] = value & 0xF0;
                    *bank = (*bank & !0x0F) | (value & 0x0F) as u32;
                }
            }
            // Display start bits 16-17 (and the enhanced memory mapping,
            // bit 3).
            0x31 => {
                self.crtc[0x31] = value;
                *start_high = (*start_high & !0x03) | (value >> 4 & 0x03);
            }
            0x43 => {
                self.crtc[0x43] = value & !0x04;
                self.offset_high = (self.offset_high & !1) | (value >> 2 & 1);
            }
            0x4A => {
                if self.fg_at > 2 {
                    self.fg_at = 0;
                }
                self.cursor_fg[self.fg_at as usize] = value;
                self.fg_at += 1;
            }
            0x4B => {
                if self.bg_at > 2 {
                    self.bg_at = 0;
                }
                self.cursor_bg[self.bg_at as usize] = value;
                self.bg_at += 1;
            }
            // Display start bits 18-19, bank bits 4-5, logical width bits
            // 8-9.
            0x51 => {
                self.crtc[0x51] = value & 0xC0;
                *start_high = (*start_high & !0x0C) | (value & 0x03) << 2;
                *bank = (*bank & !0x30) | ((value & 0x0C) << 2) as u32;
                self.offset_high = value >> 4 & 0x03;
            }
            0x69 => *start_high = value & 0x1F,
            0x6A => *bank = (value & 0x7F) as u32,
            // Read only: the IDs and the configuration straps.
            0x2D..=0x30 | 0x36 | 0x37 => {}
            0x19..=0x7F => self.crtc[i] = value,
            _ => {}
        }
    }

    /// Read a sequencer register from 08h up: 09h and on only with SR08
    /// unlocked (06h).
    pub fn read_seq(&mut self, index: u8) -> u8 {
        if index > 0x08 && self.seq[0x08] != 0x06 {
            return if index < 0x1B { 0 } else { index };
        }
        match index {
            // A signature read four times in turn.
            0x17 => {
                let value = [0x7B, 0xC0, 0x00, 0xDA][self.sr17 as usize];
                self.sr17 = (self.sr17 + 1) % 4;
                value
            }
            0x08..=0x1F => self.seq[index as usize],
            _ => 0,
        }
    }

    pub fn write_seq(&mut self, index: u8, value: u8) {
        if index > 0x08 && self.seq[0x08] != 0x06 {
            return;
        }
        if (0x08..=0x1F).contains(&index) {
            self.seq[index as usize] = value;
        }
    }

    /// The CRTC Offset register's value in the enhanced modes: bits 0-7
    /// from CR13, 8-9 from the S3's.
    pub fn offset(&self, vga: &VgaCard) -> u32 {
        vga.crtc_regs[0x13] as u32 | (self.offset_high as u32) << 8
    }

    /// The picture the registers describe, if it is one of the enhanced
    /// modes (DOSBox-X's `VGA_DetermineMode_S3`): the pixel format of CR67,
    /// or with it 0 a 256-colour mode with enhanced memory mapping (CR31
    /// bit 3). Its size comes from the CRTC's display end registers and
    /// their S3 overflow bits, eight pixels to a character clock (four at
    /// 15 and 16 bits and at 8 bits without CR3A bit 4).
    pub fn format(&self, vga: &VgaCard) -> Option<Format> {
        let graphics = vga.attribute_regs[0x10] & 0x01 != 0;
        let chained = vga.graphics_regs[0x05] & 0x40 != 0 || self.crtc[0x3A] & 0x10 != 0;
        let bpp = match self.crtc[0x67] >> 4 {
            0 if graphics && chained && self.crtc[0x31] & 0x08 != 0 => 8,
            1 => 8,
            3 => 15,
            5 => 16,
            7 => 24,
            0x0D => 32,
            _ => return None,
        };
        let per_clock = match bpp {
            8 if self.crtc[0x3A] & 0x10 == 0 => 4,
            15 | 16 => 4,
            _ => 8,
        };
        let crtc = &vga.crtc_regs;
        let hdisplay = crtc[0x01] as u32 + 1 + ((self.crtc[0x5D] as u32 & 0x02) << 7);
        let vdisplay = crtc[0x12] as u32
            | (crtc[0x07] as u32 & 0x02) << 7
            | (crtc[0x07] as u32 & 0x40) << 3
            | (self.crtc[0x5E] as u32 & 0x02) << 9;
        let scanlines = ((crtc[0x09] & 0x1F) as u32 + 1) << (crtc[0x09] >> 7);
        let width = (hdisplay * per_clock).min(2048) as u16;
        let height = ((vdisplay + 1) / scanlines).min(2048) as u16;
        (width >= 8 && height >= 8).then_some(Format { width, height, bpp })
    }

    /// Whether the window at A0000h shows linear video memory rather than
    /// the VGA's planes: enhanced memory mapping (CR31 bit 3) in an
    /// enhanced mode.
    pub fn linear_window(&self, vga: &VgaCard) -> bool {
        self.format(vga).is_some()
    }

    /// The linear frame buffer's address, if it is enabled (CR58 bit 4):
    /// CR59 and CR5A's, aligned to the window's size (CR58 bits 0-1).
    pub fn lfb_base(&self) -> Option<u32> {
        if self.crtc[0x58] & 0x10 == 0 {
            return None;
        }
        let size = match self.crtc[0x58] & 0x03 {
            0 => 0x1_0000,
            1 => 0x10_0000,
            2 => 0x20_0000,
            _ => 0x40_0000,
        };
        let window = (self.crtc[0x59] as u32) << 24 | (self.crtc[0x5A] as u32) << 16;
        Some(window & !(size - 1))
    }

    /// Whether the graphics engine's registers and image transfer are at
    /// A0000h-AFFFFh (CR53 bit 4), and at the linear frame buffer + 16 MB
    /// (bit 3).
    pub fn mmio(&self) -> bool {
        self.crtc[0x53] & 0x10 != 0
    }

    pub fn mmio_high(&self) -> bool {
        self.crtc[0x53] & 0x08 != 0
    }

    /// The graphics engine's screen width and bytes per pixel (CR50).
    pub fn engine_layout(&self) -> (u32, u32) {
        let width = match self.crtc[0x50] & 0xC1 {
            0x01 => 1152,
            0x40 => 640,
            0x80 => 800,
            0xC0 => 1280,
            0x81 => 1600,
            _ => 1024,
        };
        let bytes = match self.crtc[0x50] & 0x30 {
            0x10 => 2,
            0x30 => 4,
            _ => 1,
        };
        (width, bytes)
    }

    /// The hardware cursor, if it is on (CR45 bit 0).
    pub fn cursor(&self) -> Option<Cursor> {
        if self.crtc[0x45] & 0x01 == 0 {
            return None;
        }
        Some(Cursor {
            x: ((self.crtc[0x46] as u32) << 8 | self.crtc[0x47] as u32) & 0x7FF,
            y: ((self.crtc[0x48] as u32) << 8 | self.crtc[0x49] as u32) & 0x7FF,
            skip_x: (self.crtc[0x4E] & 0x3F) as u32,
            skip_y: (self.crtc[0x4F] & 0x3F) as u32,
            address: (((self.crtc[0x4C] as u32 & 0x0F) << 8) | self.crtc[0x4D] as u32) << 10,
            x11: self.crtc[0x55] & 0x10 != 0,
        })
    }

    /// The display timing for `format`: the standard one for its height.
    pub fn timing(&self, format: Format) -> CrtTiming {
        super::vbe::timing_for(format.height)
    }
}

/// The hardware cursor's position and pattern: 64x64 pixels of two bits
/// each at `address` in video memory, from pixel (`skip_x`, `skip_y`) of it
/// on, at (`x`, `y`) on the screen; in X11 mode (`x11`) or Windows'.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub x: u32,
    pub y: u32,
    pub skip_x: u32,
    pub skip_y: u32,
    pub address: u32,
    pub x11: bool,
}

/// What the cursor does to a pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorPixel {
    Transparent,
    Foreground,
    Background,
    Invert,
}

impl Cursor {
    /// The cursor's pixel at (`cx`, `cy`) of its 64x64 pattern in `vram`:
    /// 16 pixels to four bytes, two of the AND plane then two of the XOR
    /// plane.
    pub fn pixel(&self, vram: &[u8], cx: u32, cy: u32) -> CursorPixel {
        let bit = cy * 64 + cx;
        let group = self.address as usize + (bit / 16) as usize * 4;
        let byte = (bit % 16 / 8) as usize;
        let mask = 0x80 >> (bit % 8);
        let at = |i: usize| vram.get((group + i) % vram.len()).copied().unwrap_or(0);
        let (a, b) = (at(byte) & mask != 0, at(byte + 2) & mask != 0);
        match (self.x11, a, b) {
            (true, false, _) => CursorPixel::Transparent,
            (true, true, true) => CursorPixel::Foreground,
            (true, true, false) => CursorPixel::Background,
            (false, true, true) => CursorPixel::Invert,
            (false, true, false) => CursorPixel::Transparent,
            (false, false, true) => CursorPixel::Foreground,
            (false, false, false) => CursorPixel::Background,
        }
    }
}

crate::state_fields!(S3 { crtc, seq, sr17, offset_high, cursor_fg, cursor_bg, fg_at, bg_at });

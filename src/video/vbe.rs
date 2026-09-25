//! The Super VGA behind the VESA BIOS Extensions: 4 MB of linear video
//! memory, seen through a 64 KB window at A0000h that banks switch, or
//! whole through a linear frame buffer high in the physical address space,
//! and the modes VBE programs pick from, up to 1024x768 with 32-bit color.
//!
//! The standard VGA modes keep the VGA's own planar memory; only VBE modes
//! use this memory.

use super::crt::CrtTiming;

/// Physical address of the linear frame buffer.
pub const LFB_BASE: usize = 0xE000_0000;
/// Video memory: 4 MB.
pub const VRAM_SIZE: usize = 4 << 20;
/// Size and granularity of the window at A0000h.
pub const WINDOW_SIZE: usize = 0x10000;

/// A VBE mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VbeMode {
    pub number: u16,
    pub width: u16,
    pub height: u16,
    /// 8 (palette), 15, 16 or 32 (direct color).
    pub bpp: u8,
    pub timing: CrtTiming,
}

impl VbeMode {
    pub fn bytes_per_pixel(&self) -> usize {
        (self.bpp as usize).div_ceil(8)
    }
}

const fn mode(number: u16, width: u16, height: u16, bpp: u8, timing: CrtTiming) -> VbeMode {
    VbeMode { number, width, height, bpp, timing }
}

/// The modes, in the order the mode list gives them. "24-bit" color is
/// 32 bits per pixel, as on S3 cards.
pub static MODES: [VbeMode; 16] = [
    mode(0x100, 640, 400, 8, CrtTiming::VGA_400),
    mode(0x101, 640, 480, 8, CrtTiming::VESA_480),
    mode(0x103, 800, 600, 8, CrtTiming::VESA_600),
    mode(0x105, 1024, 768, 8, CrtTiming::VESA_768),
    mode(0x10D, 320, 200, 15, CrtTiming::VGA_400),
    mode(0x10E, 320, 200, 16, CrtTiming::VGA_400),
    mode(0x10F, 320, 200, 32, CrtTiming::VGA_400),
    mode(0x110, 640, 480, 15, CrtTiming::VESA_480),
    mode(0x111, 640, 480, 16, CrtTiming::VESA_480),
    mode(0x112, 640, 480, 32, CrtTiming::VESA_480),
    mode(0x113, 800, 600, 15, CrtTiming::VESA_600),
    mode(0x114, 800, 600, 16, CrtTiming::VESA_600),
    mode(0x115, 800, 600, 32, CrtTiming::VESA_600),
    mode(0x116, 1024, 768, 15, CrtTiming::VESA_768),
    mode(0x117, 1024, 768, 16, CrtTiming::VESA_768),
    mode(0x118, 1024, 768, 32, CrtTiming::VESA_768),
];

pub fn find_mode(number: u16) -> Option<&'static VbeMode> {
    MODES.iter().find(|m| m.number == number & 0x1FF)
}

pub struct Vbe {
    pub vram: Vec<u8>,
    /// The VBE mode set, if one is.
    pub mode: Option<&'static VbeMode>,
    /// The mode was set with the linear frame buffer (bit 14).
    pub lfb: bool,
    /// Window A's position in video memory, in 64 KB units.
    pub bank: u32,
    /// Bytes per scan line.
    pub pitch: u32,
    /// Byte offset of the displayed picture, as set, and as the display
    /// uses it since the last vertical retrace.
    pub start: u32,
    pub latched_start: u32,
    /// CRTC register 69h: bits 16-23 of the display start in doublewords,
    /// above the VGA's Start Address registers (as on S3 cards).
    pub start_high: u8,
}

impl Default for Vbe {
    fn default() -> Self {
        Self::new()
    }
}

impl Vbe {
    pub fn new() -> Self {
        Self {
            vram: vec![0; VRAM_SIZE],
            mode: None,
            lfb: false,
            bank: 0,
            pitch: 0,
            start: 0,
            latched_start: 0,
            start_high: 0,
        }
    }

    /// Switch to `mode`, clearing video memory unless `keep` is set.
    pub fn set_mode(&mut self, mode: &'static VbeMode, lfb: bool, keep: bool) {
        self.mode = Some(mode);
        self.lfb = lfb;
        self.bank = 0;
        self.pitch = mode.width as u32 * mode.bytes_per_pixel() as u32;
        self.start = 0;
        self.latched_start = 0;
        self.start_high = 0;
        if !keep {
            self.vram.fill(0);
        }
    }

    /// Back to the standard VGA modes.
    pub fn reset(&mut self) {
        self.mode = None;
        self.lfb = false;
    }

    /// Where a byte of the window at A0000h is in video memory.
    #[inline]
    pub fn window_offset(&self, offset: usize) -> usize {
        (self.bank as usize * WINDOW_SIZE + offset) % VRAM_SIZE
    }

    /// Where `len` bytes at physical address `addr` are in the linear
    /// frame buffer, if they are all there.
    #[inline]
    pub fn lfb_offset(addr: usize, len: usize) -> Option<usize> {
        let offset = addr.checked_sub(LFB_BASE)?;
        (offset + len <= VRAM_SIZE).then_some(offset)
    }

    /// The rows of the picture (in frame rows) that `len` bytes of video
    /// memory at `offset` are in, as (first, after last), if they show.
    pub fn frame_rows(&self, offset: usize, len: usize) -> Option<(u32, u32)> {
        let mode = self.mode?;
        let pitch = self.pitch.max(1) as usize;
        let start = self.latched_start as usize;
        let first = offset.checked_sub(start)? / pitch;
        let last = (offset + len - 1 - start) / pitch;
        if first >= mode.height as usize {
            return None;
        }
        let scale = self.scale() as usize;
        Some(((first * scale) as u32, ((last + 1).min(mode.height as usize) * scale) as u32))
    }

    /// Small modes are shown doubled.
    pub fn scale(&self) -> u32 {
        match self.mode {
            Some(mode) if mode.width < 640 => 2,
            _ => 1,
        }
    }

    /// The size of the picture: the mode's, doubled if it is small.
    pub fn frame_size(&self) -> Option<(u32, u32)> {
        let mode = self.mode?;
        let scale = self.scale();
        Some((mode.width as u32 * scale, mode.height as u32 * scale))
    }

    /// Scan lines of video memory at the current pitch.
    pub fn max_lines(&self) -> u32 {
        (VRAM_SIZE as u32) / self.pitch.max(1)
    }
}

/// The mode is saved by its number, and found again among the modes.
impl crate::savestate::State for Vbe {
    fn save(&self, w: &mut crate::savestate::Writer) {
        let Vbe { vram, mode, lfb, bank, pitch, start, latched_start, start_high } = self;
        vram.save(w);
        mode.map_or(0, |m| m.number).save(w);
        crate::savestate::State::save(&(*lfb, *bank, *pitch), w);
        crate::savestate::State::save(&(*start, *latched_start, *start_high), w);
    }
    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        let Vbe { vram, mode, lfb, bank, pitch, start, latched_start, start_high } = self;
        vram.load(r)?;
        let mut number = 0u16;
        number.load(r)?;
        *mode = match number {
            0 => None,
            n => Some(find_mode(n).ok_or_else(|| crate::savestate::StateError::Invalid(format!("VESA mode {:X}h", n)))?),
        };
        lfb.load(r)?;
        bank.load(r)?;
        pitch.load(r)?;
        start.load(r)?;
        latched_start.load(r)?;
        start_high.load(r)?;
        Ok(())
    }
}

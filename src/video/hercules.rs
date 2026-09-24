//! The Hercules Graphics Card (`machine=hercules`): an MDA, with a Motorola
//! 6845 CRTC at 3B4h/3B5h (and its mirrors at 3B0h-3B7h), the Mode Control
//! register at 3B8h, the status at 3BAh, and the Configuration Switch at
//! 3BFh, over 64 KB at B0000h: the monochrome text mode 7, and 720x348
//! graphics that programs set up themselves in two pages. It is the same
//! `VgaCard`, answering as a Hercules card does.

use super::VideoMode;
use super::crt::CrtTiming;
use super::vga::VgaCard;

/// The ports of the Hercules card.
pub const PORTS: &[u16] = &[0x3B0, 0x3B1, 0x3B2, 0x3B3, 0x3B4, 0x3B5, 0x3B6, 0x3B7, 0x3B8, 0x3BA, 0x3BF];

/// The card's crystal: 16.257 MHz, a character every 9 pixels in text and
/// every 16 in graphics.
const CLOCK: u64 = 16_257_000;

/// The 6845 registers R0-R15 of the MDA's text mode (the IBM BIOS's) and of
/// the Hercules graphics mode (Hercules' own).
#[rustfmt::skip]
const CRTC_TEXT: [u8; 16] = [0x61, 0x50, 0x52, 0x0F, 0x19, 0x06, 0x19, 0x19, 0x02, 0x0D, 0x0B, 0x0C, 0, 0, 0, 0];
#[rustfmt::skip]
pub const CRTC_GRAPHICS: [u8; 16] = [0x35, 0x2D, 0x2E, 0x07, 0x5B, 0x02, 0x57, 0x57, 0x02, 0x03, 0x00, 0x00, 0, 0, 0, 0];

/// The width and height of the graphics as Hercules' own software sets
/// them up (programs may set up others).
pub const GRAPHICS_SIZE: (usize, usize) = (720, 348);

/// The MDA's shades: black, the normal video, and bright (intensified).
pub const BLACK: (u8, u8, u8) = (0, 0, 0);
pub const NORMAL: (u8, u8, u8) = (0xC0, 0xC0, 0xC0);
pub const BRIGHT: (u8, u8, u8) = (0xFF, 0xFF, 0xFF);

impl VgaCard {
    /// The text mode, as the BIOS sets mode 7.
    pub(super) fn herc_set_mode(&mut self) {
        self.crtc_regs = [0; 25];
        self.crtc_regs[..16].copy_from_slice(&CRTC_TEXT);
        self.herc_mode = 0x29;
        self.herc_config = 0;
        self.herc_changed();
    }

    fn herc_changed(&mut self) {
        self.invalidate_timing();
        self.mark_dirty_full();
    }

    pub(super) fn herc_io_read(&mut self, port: u16) -> u8 {
        match port {
            // The cursor and light pen addresses can be read; the other
            // registers read 0.
            0x3B1 | 0x3B3 | 0x3B5 | 0x3B7 => match self.crtc_index {
                14..=17 => self.crtc_regs[self.crtc_index as usize],
                _ => 0,
            },
            _ => 0xFF,
        }
    }

    pub(super) fn herc_io_write(&mut self, port: u16, value: u8) {
        match port {
            0x3B0 | 0x3B2 | 0x3B4 | 0x3B6 => self.crtc_index = value & 0x1F,
            0x3B1 | 0x3B3 | 0x3B5 | 0x3B7 => {
                let index = self.crtc_index as usize;
                if index < 18 {
                    self.crtc_regs[index] = value;
                    match index {
                        12 | 13 => {}
                        0..=9 => self.herc_changed(),
                        _ => self.mark_dirty_full(),
                    }
                }
            }
            0x3B8 => {
                // Graphics (bit 1) and page 1 (bit 7) only where the
                // Configuration Switch allows them.
                let mut value = value;
                if self.herc_config & 0x01 == 0 {
                    value &= !0x02;
                }
                if self.herc_config & 0x02 == 0 {
                    value &= !0x80;
                }
                self.herc_mode = value;
                self.herc_changed();
            }
            0x3BF => self.herc_config = value & 0x03,
            _ => {}
        }
    }

    /// The mode the Mode Control register (3B8h) sets.
    pub fn herc_video_mode(&self) -> VideoMode {
        if self.herc_mode & 0x02 != 0 { VideoMode::HercGraphics } else { VideoMode::Mono80x25 }
    }

    /// Whether the picture is on (Mode Control bit 3).
    pub fn herc_video_enabled(&self) -> bool {
        self.herc_mode & 0x08 != 0
    }

    /// Where the Hercules card's memory is: B0000h, and B8000h too for page
    /// 1 when the Configuration Switch enables it.
    pub fn herc_window(&self) -> (usize, usize, usize) {
        let size = if self.herc_config & 0x02 != 0 { 0x10000 } else { 0x8000 };
        (0xB0000, size, 0xFFFF)
    }

    /// The shape of the graphics the 6845 shows: R1 characters of 16 pixels
    /// (2 bytes) a row; R9 + 1 scanlines to a character row, each from its
    /// own 8 KB bank; R6 character rows. The width and height in pixels,
    /// the bytes a row of a bank and the banks.
    pub fn herc_graphics_shape(&self) -> (usize, usize, usize, usize) {
        let regs = &self.crtc_regs;
        let chars = (regs[1] as usize).clamp(1, 64);
        let banks = ((regs[9] & 0x1F) as usize + 1).min(4);
        let rows = ((regs[6] & 0x7F) as usize).clamp(1, 127);
        (chars * 16, (rows * banks).min(512), chars * 2, banks)
    }

    pub(super) fn herc_timing(&self) -> Option<CrtTiming> {
        let dots = if self.herc_mode & 0x02 != 0 { 16 } else { 9 };
        CrtTiming::from_6845(&self.crtc_regs, CLOCK / dots)
    }
}

/// The status port (3BAh) at time `t_ns`: bit 0 in the horizontal retrace,
/// bit 3 while the beam draws, and on a Hercules card bit 7 low in the
/// vertical retrace, which is how programs tell it from an MDA. Bits 4-6
/// are 000, a plain HGC.
pub fn status(timing: &CrtTiming, t_ns: u64) -> u8 {
    let vga = timing.status(t_ns);
    let retrace = vga & 0x08 != 0;
    let blank = vga & 0x01 != 0;
    (if retrace { 0 } else { 0x80 }) | (if blank { 0x01 } else { 0x08 })
}

/// Draw the graphics: a bit a pixel, scanline y from bank y % banks (8 KB
/// each) at row y / banks, as `herc_graphics_shape` has them (720x348 in
/// four banks, 90 bytes a row, as Hercules' software sets them up), from
/// the Start Address (in words) of page 0 at B0000h or page 1 at B8000h
/// (Mode Control bit 7).
pub fn render_graphics(canvas: &mut [u8], canvas_w: usize, vga: &VgaCard) {
    let (width, height, row_bytes, banks) = vga.herc_graphics_shape();
    if !vga.herc_video_enabled() {
        return;
    }
    let canvas_h = canvas.len() / (canvas_w * 3);
    let page = if vga.herc_mode & 0x80 != 0 { 0x8000 } else { 0 };
    let start = vga.latched_start_addr * 2;
    let vram = &vga.vram_text;
    for y in 0..height.min(canvas_h) {
        let row = page + (y % banks) * 0x2000 + ((start + (y / banks) * row_bytes) & 0x1FFF);
        for x in 0..width.min(canvas_w) {
            if vram[(row + x / 8) & 0xFFFF] & (0x80 >> (x % 8)) != 0 {
                let i = (y * canvas_w + x) * 3;
                canvas[i..i + 3].copy_from_slice(&[NORMAL.0, NORMAL.1, NORMAL.2]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_graphics_are_50_hz() {
        let mut vga = VgaCard::new();
        vga.herc_set_mode();
        let timing = vga.herc_timing().unwrap();
        assert_eq!((timing.total, timing.display), (370, 350));
        assert!((timing.hz() - 49.8).abs() < 0.5, "{}", timing.hz());
        vga.crtc_regs[..16].copy_from_slice(&CRTC_GRAPHICS);
        vga.herc_mode = 0x0A;
        let timing = vga.herc_timing().unwrap();
        assert_eq!((timing.total, timing.display), (370, 348));
        assert!((timing.hz() - 50.8).abs() < 1.0, "{}", timing.hz());
    }

    #[test]
    fn graphics_need_the_configuration_switch() {
        let mut vga = VgaCard::new();
        vga.herc_set_mode();
        vga.herc_io_write(0x3B8, 0x8A);
        assert_eq!(vga.herc_video_mode(), VideoMode::Mono80x25);
        vga.herc_io_write(0x3BF, 0x01);
        vga.herc_io_write(0x3B8, 0x8A);
        assert_eq!((vga.herc_video_mode(), vga.herc_mode & 0x80), (VideoMode::HercGraphics, 0));
        vga.herc_io_write(0x3BF, 0x03);
        vga.herc_io_write(0x3B8, 0x8A);
        assert_eq!(vga.herc_mode & 0x80, 0x80);
        assert_eq!(vga.herc_window(), (0xB0000, 0x10000, 0xFFFF));
    }
}

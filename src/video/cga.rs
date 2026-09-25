//! IBM's Color Graphics Adapter (`machine=cga`): a Motorola 6845 CRTC at
//! 3D4h/3D5h (and its mirrors at 3D0h-3D7h), the Mode Control register at
//! 3D8h, the Color Select register at 3D9h and the status at 3DAh, over 16
//! KB of memory at B8000h. Its colours are the fixed 16 of an RGBI monitor.
//! It is the same `VgaCard`, answering as a CGA does.

use super::VideoMode;
use super::composite::{CompositeMode, CompositeSettings, Decoder};
use super::crt::CrtTiming;
use super::palette::rgbi;
use super::vga::VgaCard;

/// The ports of the CGA.
pub const PORTS: &[u16] = &[0x3D0, 0x3D1, 0x3D2, 0x3D3, 0x3D4, 0x3D5, 0x3D6, 0x3D7, 0x3D8, 0x3D9, 0x3DA, 0x3DB, 0x3DC];

/// The CGA's crystal: 14.318 MHz, a character every 8 pixels in the 80
/// column mode and every 16 in the others.
const CLOCK: u64 = 14_318_180;

/// The 6845 registers R0-R15 the IBM BIOS loads for the 40 and 80-column
/// text modes and the graphics modes.
#[rustfmt::skip]
pub(super) const CRTC_40: [u8; 16] = [0x38, 0x28, 0x2D, 0x0A, 0x1F, 0x06, 0x19, 0x1C, 0x02, 0x07, 0x06, 0x07, 0, 0, 0, 0];
#[rustfmt::skip]
pub(super) const CRTC_80: [u8; 16] = [0x71, 0x50, 0x5A, 0x0A, 0x1F, 0x06, 0x19, 0x1C, 0x02, 0x07, 0x06, 0x07, 0, 0, 0, 0];
#[rustfmt::skip]
pub(super) const CRTC_GRAPHICS: [u8; 16] = [0x38, 0x28, 0x2D, 0x0A, 0x7F, 0x06, 0x64, 0x70, 0x02, 0x01, 0x06, 0x07, 0, 0, 0, 0];

/// The Mode Control register's values for modes 0-6 (as BDA 0465h keeps
/// them): bit 0 80 columns, bit 1 graphics, bit 2 no colour burst, bit 3
/// video on, bit 4 640 pixels, bit 5 blinking.
const MODE_CONTROL: [u8; 7] = [0x2C, 0x28, 0x2D, 0x29, 0x2A, 0x2E, 0x1E];

/// An RGBI colour as 8-bit RGB.
fn rgb(color: u8) -> (u8, u8, u8) {
    let [r, g, b] = rgbi(color);
    (r << 2 | r >> 4, g << 2 | g >> 4, b << 2 | b >> 4)
}

impl VgaCard {
    /// Set the registers as the BIOS does for `mode` (0-6).
    pub(super) fn cga_set_mode(&mut self, mode: VideoMode) {
        let number = mode as usize;
        let crtc = match mode {
            VideoMode::Text40x25 | VideoMode::Text40x25Color => &CRTC_40,
            VideoMode::Text80x25 | VideoMode::Text80x25Color => &CRTC_80,
            _ => &CRTC_GRAPHICS,
        };
        self.crtc_regs = [0; 25];
        self.crtc_regs[..16].copy_from_slice(crtc);
        self.cga_mode = MODE_CONTROL.get(number).copied().unwrap_or(0x29);
        self.cga_color = if mode == VideoMode::Cga640x200 { 0x3F } else { 0x30 };
        self.cga_changed();
    }

    /// A register that changes the picture or its timing changed.
    pub(super) fn cga_changed(&mut self) {
        self.invalidate_timing();
        self.refresh_composite();
        self.mark_dirty_full();
    }

    /// Change how the monitor shows the graphics (`composite`).
    pub fn set_composite(&mut self, settings: CompositeSettings) {
        if settings != self.composite {
            self.composite = settings;
            self.refresh_composite();
            self.mark_dirty_full();
        }
    }

    /// Whether the graphics go through the composite decoder: a CGA in a
    /// graphics mode with `composite=on`, or with `auto` in 640x200 with
    /// the colour burst, the mode composite programs draw artifact colours
    /// in (the BIOS's mode 6 turns the burst off).
    pub fn composite_active(&self) -> bool {
        let mode = self.cga_mode;
        self.adapter == super::adapter::Adapter::Cga
            && mode & 0x02 != 0
            && match self.composite.mode {
                CompositeMode::On => true,
                CompositeMode::Auto => mode & 0x10 != 0 && mode & 0x04 == 0,
                CompositeMode::Off => false,
            }
    }

    /// Have the decoder for the colour burst and revision in place while
    /// composite is on.
    fn refresh_composite(&mut self) {
        if !self.composite_active() {
            return;
        }
        let bw = self.cga_mode & 0x04 != 0;
        if !self.composite_decoder.as_ref().is_some_and(|d| d.is_for(self.composite.era, bw)) {
            self.composite_decoder = Some(Box::new(Decoder::new(self.composite.era, bw)));
        }
    }

    /// The composite decoder, while the graphics go through it.
    pub fn composite_decoder(&self) -> Option<&Decoder> {
        if self.composite_active() { self.composite_decoder.as_deref() } else { None }
    }

    pub(super) fn cga_io_read(&mut self, port: u16) -> u8 {
        match port {
            // The 6845 lets the cursor address (R14, R15) and the light pen
            // address (R16, R17) be read; the other registers read 0.
            0x3D1 | 0x3D3 | 0x3D5 | 0x3D7 => match self.crtc_index {
                14..=17 => self.crtc_regs[self.crtc_index as usize],
                _ => 0,
            },
            // The index and the Mode Control and Color Select registers
            // are write-only.
            _ => 0xFF,
        }
    }

    pub(super) fn cga_io_write(&mut self, port: u16, value: u8) {
        match port {
            0x3D0 | 0x3D2 | 0x3D4 | 0x3D6 => self.crtc_index = value & 0x1F,
            0x3D1 | 0x3D3 | 0x3D5 | 0x3D7 => {
                let index = self.crtc_index as usize;
                if index < 18 {
                    self.crtc_regs[index] = value;
                    match index {
                        // The Start Address is latched at the vertical retrace.
                        12 | 13 => {}
                        0..=9 => self.cga_changed(),
                        _ => self.mark_dirty_full(),
                    }
                }
            }
            0x3D8 => {
                self.cga_mode = value;
                self.cga_changed();
            }
            0x3D9 => {
                self.cga_color = value;
                self.mark_dirty_full();
            }
            _ => {}
        }
    }

    /// The mode the Mode Control register (3D8h) sets, for programs that
    /// program the card without the BIOS.
    pub fn cga_video_mode(&self) -> VideoMode {
        let mode = self.cga_mode;
        let bw = mode & 0x04 != 0;
        match (mode & 0x02 != 0, mode & 0x10 != 0, mode & 0x01 != 0) {
            (true, true, _) => VideoMode::Cga640x200,
            (true, false, _) if bw => VideoMode::Cga320x200,
            (true, false, _) => VideoMode::Cga320x200Color,
            (false, _, true) if bw => VideoMode::Text80x25,
            (false, _, true) => VideoMode::Text80x25Color,
            (false, _, false) if bw => VideoMode::Text40x25,
            (false, _, false) => VideoMode::Text40x25Color,
        }
    }

    /// Whether the picture is on (Mode Control bit 3).
    pub fn cga_video_enabled(&self) -> bool {
        self.cga_mode & 0x08 != 0
    }

    /// The display timing the 6845's registers give: characters of the
    /// 80-column clock or of the slower one, and scanlines in character
    /// rows (R9 + 1 each).
    pub(super) fn cga_timing(&self) -> Option<CrtTiming> {
        let char_clock = if self.cga_mode & 0x01 != 0 { CLOCK / 8 } else { CLOCK / 16 };
        CrtTiming::from_6845(&self.crtc_regs, char_clock)
    }

    /// The 16 colours of the text modes.
    pub fn cga_text_colors(&self) -> [(u8, u8, u8); 16] {
        std::array::from_fn(|color| rgb(color as u8))
    }

    /// The four colours of the 320x200 mode: the background from the Color
    /// Select register's bits 0-3, and palette 0 (green, red, brown) or 1
    /// (cyan, magenta, white) by its bit 5, bright with its bit 4. Without
    /// the colour burst (Mode Control bit 2) an RGB monitor shows the third
    /// palette: cyan, red and white.
    pub fn cga_colors_4(&self) -> [(u8, u8, u8); 4] {
        self.cga_indices_4().map(rgb)
    }

    /// The RGBI colours (0-15) behind `cga_colors_4`.
    pub fn cga_indices_4(&self) -> [u8; 4] {
        let select = self.cga_color;
        let bright = if select & 0x10 != 0 { 0x08 } else { 0 };
        let colors = if self.cga_mode & 0x04 != 0 {
            [3, 4, 7]
        } else if select & 0x20 != 0 {
            [3, 5, 7]
        } else {
            [2, 4, 6]
        };
        [select & 0x0F, colors[0] | bright, colors[1] | bright, colors[2] | bright]
    }

    /// The two colours of the 640x200 mode: black, and the Color Select
    /// register's bits 0-3.
    pub fn cga_colors_2(&self) -> [(u8, u8, u8); 2] {
        [rgb(0), rgb(self.cga_color & 0x0F)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bios_modes_are_60_hz_with_262_lines() {
        let mut vga = VgaCard::new();
        vga.adapter = super::super::adapter::Adapter::Cga;
        for mode in [VideoMode::Text40x25Color, VideoMode::Text80x25Color, VideoMode::Cga320x200Color] {
            vga.cga_set_mode(mode);
            let timing = vga.cga_timing().unwrap();
            assert_eq!((timing.total, timing.display), (262, 200), "{:?}", mode);
            assert!((timing.hz() - 59.92).abs() < 0.05, "{:?}: {}", mode, timing.hz());
        }
    }

    #[test]
    fn the_mode_control_register_picks_the_mode() {
        let mut vga = VgaCard::new();
        for (mode, value) in
            [(VideoMode::Cga320x200Color, 0x0A), (VideoMode::Cga640x200, 0x1E), (VideoMode::Text80x25Color, 0x29)]
        {
            vga.cga_mode = value;
            assert_eq!(vga.cga_video_mode(), mode);
        }
    }

    #[test]
    fn mode_4_palettes() {
        let mut vga = VgaCard::new();
        vga.cga_mode = 0x0A;
        vga.cga_color = 0x01;
        assert_eq!(vga.cga_colors_4(), [rgb(1), rgb(2), rgb(4), rgb(6)]);
        vga.cga_color = 0x30;
        assert_eq!(vga.cga_colors_4(), [rgb(0), rgb(11), rgb(13), rgb(15)]);
        vga.cga_mode = 0x0E;
        vga.cga_color = 0x00;
        assert_eq!(vga.cga_colors_4(), [rgb(0), rgb(3), rgb(4), rgb(7)]);
        assert_eq!(rgb(6), (0xAA, 0x55, 0x00), "brown");
    }
}

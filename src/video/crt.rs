//! The timing of the picture the CRT controller sends to the monitor:
//! scanlines, display enable and vertical retrace, in emulated time. Programs
//! see it through Input Status 1 (port 3DAh) and time their page flips,
//! palette changes and even their timer calibration against it.

/// One frame of the display, as the CRTC counts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CrtTiming {
    /// Length of a scanline, horizontal blanking and retrace included.
    pub line_ns: u32,
    /// The part of a scanline with display enable.
    pub hdisplay_ns: u32,
    /// Scanlines per frame.
    pub total: u32,
    /// Scanlines with display enable (Vertical Display End).
    pub display: u32,
    /// First scanline of the vertical retrace.
    pub retrace_start: u32,
    /// First scanline after it.
    pub retrace_end: u32,
}

/// The two pixel clocks of the VGA.
const CLOCK_25MHZ: u64 = 25_175_000;
const CLOCK_28MHZ: u64 = 28_322_000;

impl CrtTiming {
    /// 400 displayed scanlines at 70 Hz: the text modes, mode 13h and the
    /// 200 and 350-line modes (31.469 kHz, 449 lines).
    pub const VGA_400: Self = Self {
        line_ns: 31_778,
        hdisplay_ns: 25_422,
        total: 449,
        display: 400,
        retrace_start: 412,
        retrace_end: 414,
    };
    /// 640x480 at 60 Hz (31.469 kHz, 525 lines).
    pub const VESA_480: Self = Self {
        line_ns: 31_778,
        hdisplay_ns: 25_422,
        total: 525,
        display: 480,
        retrace_start: 490,
        retrace_end: 492,
    };
    /// 800x600 at 60 Hz (40 MHz pixel clock, 1056 dots, 37.879 kHz).
    pub const VESA_600: Self = Self {
        line_ns: 26_400,
        hdisplay_ns: 20_000,
        total: 628,
        display: 600,
        retrace_start: 601,
        retrace_end: 605,
    };
    /// 1024x768 at 60 Hz (65 MHz pixel clock, 1344 dots, 48.363 kHz).
    pub const VESA_768: Self = Self {
        line_ns: 20_677,
        hdisplay_ns: 15_754,
        total: 806,
        display: 768,
        retrace_start: 771,
        retrace_end: 777,
    };

    /// The timing a VGA produces from its registers: the pixel clock from
    /// the Miscellaneous Output register (bits 2-3), 8 or 9-dot characters
    /// and the halved clock from the sequencer's Clocking Mode (`seq01`),
    /// and the horizontal and vertical counts from the CRTC. None if the
    /// registers describe no picture a monitor could show, as happens
    /// halfway through reprogramming them.
    pub fn from_registers(misc: u8, seq01: u8, crtc: &[u8]) -> Option<Self> {
        let mut clock = match (misc >> 2) & 3 {
            1 => CLOCK_28MHZ,
            _ => CLOCK_25MHZ,
        };
        if seq01 & 0x08 != 0 {
            clock /= 2;
        }
        let dots: u64 = if seq01 & 0x01 != 0 { 8 } else { 9 };
        let htotal = crtc[0x00] as u64 + 5;
        let hdisplay = crtc[0x01] as u64 + 1;

        // Bits 8 and 9 of the vertical counts are in the Overflow register.
        let overflow = crtc[0x07] as u32;
        let high = |bit8: u32, bit9: u32| ((overflow >> bit8) & 1) << 8 | ((overflow >> bit9) & 1) << 9;
        let mut total = (crtc[0x06] as u32 | high(0, 5)) + 2;
        let mut display = (crtc[0x12] as u32 | high(1, 6)) + 1;
        let mut retrace_start = crtc[0x10] as u32 | high(2, 7);
        // The retrace ends when the low four bits of the line counter
        // match Vertical Retrace End.
        let length = match (crtc[0x11] as u32).wrapping_sub(retrace_start) & 0x0F {
            0 => 16,
            n => n,
        };
        let mut retrace_end = retrace_start + length;
        // CRTC Mode Control bit 2: the vertical counter advances every
        // second scanline.
        if crtc[0x17] & 0x04 != 0 {
            total *= 2;
            display *= 2;
            retrace_start *= 2;
            retrace_end *= 2;
        }

        let ns = |chars: u64| (chars * dots * 1_000_000_000 + clock / 2) / clock;
        let line_ns = ns(htotal);
        let timing = Self {
            line_ns: line_ns as u32,
            hdisplay_ns: ns(hdisplay) as u32,
            total,
            display,
            retrace_start,
            retrace_end,
        };
        let line_hz = 1_000_000_000 / line_ns.max(1);
        let frame_hz = 1_000_000_000 / timing.frame_ns().max(1);
        let sane = hdisplay <= htotal
            && display <= total
            && retrace_start < total
            && (15_000..=70_000).contains(&line_hz)
            && (40..=120).contains(&frame_hz);
        sane.then_some(timing)
    }

    /// The timing an EGA produces from its registers: the 14.318 or 16.257
    /// MHz clock (Miscellaneous Output bits 2-3), halved by the sequencer's
    /// Clocking Mode bit 3, 8 or 9-dot characters, and the CRTC's counts,
    /// which on the EGA are the totals less 2, with only bit 8 of the
    /// vertical ones in the Overflow register.
    pub fn from_ega_registers(misc: u8, seq01: u8, crtc: &[u8]) -> Option<Self> {
        let mut clock: u64 = if (misc >> 2) & 3 == 1 { 16_257_000 } else { 14_318_180 };
        if seq01 & 0x08 != 0 {
            clock /= 2;
        }
        let dots: u64 = if seq01 & 0x01 != 0 { 8 } else { 9 };
        let htotal = crtc[0x00] as u64 + 2;
        let hdisplay = (crtc[0x01] as u64 + 1).min(htotal);
        let overflow = crtc[0x07] as u32;
        let high = |bit: u32| ((overflow >> bit) & 1) << 8;
        let mut total = (crtc[0x06] as u32 | high(0)) + 2;
        let mut display = (crtc[0x12] as u32 | high(1)) + 1;
        let mut retrace_start = crtc[0x10] as u32 | high(2);
        let length = match (crtc[0x11] as u32).wrapping_sub(retrace_start) & 0x0F {
            0 => 16,
            n => n,
        };
        let mut retrace_end = retrace_start + length;
        if crtc[0x17] & 0x04 != 0 {
            total *= 2;
            display *= 2;
            retrace_start *= 2;
            retrace_end *= 2;
        }
        let ns = |chars: u64| (chars * dots * 1_000_000_000 + clock / 2) / clock;
        let timing = Self {
            line_ns: ns(htotal) as u32,
            hdisplay_ns: ns(hdisplay) as u32,
            total,
            display: display.min(total),
            retrace_start,
            retrace_end,
        };
        let line_hz = 1_000_000_000 / timing.line_ns.max(1) as u64;
        let frame_hz = 1_000_000_000 / timing.frame_ns().max(1);
        let sane = retrace_start < total && (15_000..=30_000).contains(&line_hz) && (40..=90).contains(&frame_hz);
        sane.then_some(timing)
    }

    /// The timing a Motorola 6845 (the CGA's and MDA's CRTC) produces from
    /// its registers R0-R9 at `char_clock` characters a second: R0 + 1
    /// characters a scanline, R1 of them shown; R4 + 1 character rows of R9
    /// + 1 scanlines, and R5 more scanlines, a frame, R6 rows shown; the
    /// vertical sync from row R7 on, 16 scanlines long.
    pub fn from_6845(regs: &[u8], char_clock: u64) -> Option<Self> {
        let htotal = regs[0] as u64 + 1;
        let hdisplay = (regs[1] as u64).min(htotal);
        let row = (regs[9] & 0x1F) as u32 + 1;
        let total = (regs[4] & 0x7F) as u32 * row + row + (regs[5] & 0x1F) as u32;
        let display = ((regs[6] & 0x7F) as u32 * row).min(total);
        let retrace_start = (regs[7] & 0x7F) as u32 * row;
        let ns = |chars: u64| (chars * 1_000_000_000 + char_clock / 2) / char_clock;
        let timing = Self {
            line_ns: ns(htotal) as u32,
            hdisplay_ns: ns(hdisplay) as u32,
            total,
            display,
            retrace_start,
            retrace_end: retrace_start + 16,
        };
        let frame_hz = 1_000_000_000 / timing.frame_ns().max(1);
        let sane = retrace_start < total && (40..=120).contains(&frame_hz) && timing.line_ns > 0;
        sane.then_some(timing)
    }

    pub fn frame_ns(&self) -> u64 {
        self.line_ns as u64 * self.total as u64
    }

    pub fn hz(&self) -> f64 {
        1e9 / self.frame_ns() as f64
    }

    /// Input Status 1 at time `t_ns`: bit 3 during the vertical retrace,
    /// bit 0 whenever display enable is off (horizontal or vertical
    /// blanking). Lines below the display keep bit 0 set, so a program
    /// counting its 1-to-0 edges sees `display` of them per frame.
    pub fn status(&self, t_ns: u64) -> u8 {
        let pos = t_ns % self.frame_ns();
        let line = (pos / self.line_ns as u64) as u32;
        let column = pos % self.line_ns as u64;
        let mut status = 0;
        if self.in_retrace(line) {
            status |= 0x08;
        }
        if line >= self.display || column >= self.hdisplay_ns as u64 {
            status |= 0x01;
        }
        status
    }

    fn in_retrace(&self, line: u32) -> bool {
        // A retrace may run past the last line into the next frame.
        (line + self.total - self.retrace_start) % self.total
            < self.retrace_end - self.retrace_start
    }

    fn retrace_ns(&self) -> u64 {
        self.retrace_start as u64 * self.line_ns as u64
    }

    /// The number of vertical retraces that began up to time `t_ns`.
    pub fn retraces(&self, t_ns: u64) -> u64 {
        (t_ns + self.frame_ns() - self.retrace_ns()) / self.frame_ns()
    }

    /// When the first vertical retrace after `t_ns` begins.
    pub fn next_retrace(&self, t_ns: u64) -> u64 {
        self.retraces(t_ns) * self.frame_ns() + self.retrace_ns()
    }
}

crate::state_fields!(CrtTiming { line_ns, hdisplay_ns, total, display, retrace_start, retrace_end });


#[cfg(test)]
mod tests {
    use super::*;

    /// CRTC registers 00h-18h of the IBM BIOS mode 3.
    const MODE_3: [u8; 25] = [
        0x5F, 0x4F, 0x50, 0x82, 0x55, 0x81, 0xBF, 0x1F, 0x00, 0x4F, 0x0D, 0x0E, 0x00, 0x00, 0x00,
        0x00, 0x9C, 0x8E, 0x8F, 0x28, 0x1F, 0x96, 0xB9, 0xA3, 0xFF,
    ];

    #[test]
    fn mode_3_is_the_70_hz_timing() {
        // 100 characters of 9 dots at 28.322 MHz: 31.777 µs, 449 lines.
        let t = CrtTiming::from_registers(0x67, 0x00, &MODE_3).unwrap();
        assert_eq!((t.line_ns, t.hdisplay_ns), (31_777, 25_422));
        assert_eq!((t.total, t.display, t.retrace_start, t.retrace_end), (449, 400, 412, 414));
        assert!((t.hz() - 70.09).abs() < 0.01);
    }

    #[test]
    fn nonsense_registers_give_no_timing() {
        let mut crtc = MODE_3;
        crtc[0x06] = 0;
        crtc[0x07] = 0;
        assert_eq!(CrtTiming::from_registers(0x67, 0x00, &crtc), None);
    }

    #[test]
    fn retrace_and_display_enable() {
        let t = CrtTiming::VGA_400;
        let line = t.line_ns as u64;
        assert_eq!(t.status(0), 0);
        assert_eq!(t.status(t.hdisplay_ns as u64), 0x01);
        assert_eq!(t.status(400 * line), 0x01);
        assert_eq!(t.status(412 * line), 0x09);
        assert_eq!(t.status(414 * line), 0x01);
        assert_eq!(t.retraces(412 * line - 1), 0);
        assert_eq!(t.retraces(412 * line), 1);
        assert_eq!(t.next_retrace(0), 412 * line);
        assert_eq!(t.next_retrace(412 * line), 412 * line + t.frame_ns());
    }
}

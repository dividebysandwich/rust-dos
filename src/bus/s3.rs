//! The bus's side of the S3 Trio64 (`video::s3`): its extended registers
//! at the VGA's ports, what they decide about the display and the memory
//! map, and the graphics engine's ports and memory-mapped registers.

use super::Bus;
use crate::video::VideoMode;
use crate::video::adapter::Adapter;
use crate::video::s3::engine::Surface;
use crate::video::vbe::VbeMode;

/// The graphics engine's ports: 42E8h, 46E8h, 4AE8h, 82E8h-BEE8h every
/// 400h with their odd bytes, and the pixel data at E2E8h-E2EBh.
pub fn is_engine_port(port: u16) -> bool {
    match port & 0x03FE {
        0x02E8 => matches!(port >> 10, 0x10..=0x12 | 0x20..=0x2F | 0x38),
        0x02EA => port >> 10 == 0x38,
        _ => false,
    }
}

impl Bus {
    /// Whether the card is the S3.
    #[inline]
    pub(crate) fn s3(&self) -> bool {
        self.vga.adapter == Adapter::S3
    }

    pub(crate) fn s3_crtc_write(&mut self, index: u8, value: u8) {
        let (mut bank, mut start_high) = (self.vbe.bank, self.vbe.start_high);
        self.vga.s3.write_crtc(index, value, &mut bank, &mut start_high);
        self.vbe.bank = bank;
        if start_high != self.vbe.start_high {
            self.vbe.start_high = start_high;
            self.update_vbe_start();
        }
        // The hardware cursor moves or changes: show it again.
        if (0x45..=0x4F).contains(&index) || index == 0x55 {
            self.vga.mark_dirty_full();
        }
        self.s3_settle();
    }

    pub(crate) fn s3_crtc_read(&mut self, index: u8) -> u8 {
        let (bank, start_high) = (self.vbe.bank, self.vbe.start_high);
        let mut s3 = std::mem::take(&mut self.vga.s3);
        let value = s3.read_crtc(index, &self.vga, bank, start_high);
        self.vga.s3 = s3;
        value
    }

    /// Show what the registers describe: an enhanced mode in linear video
    /// memory, or the VGA's own, with the linear frame buffer where they
    /// put it. Runs after every write to a register it depends on.
    pub(crate) fn s3_settle(&mut self) {
        // The frame buffer, unless it would cover RAM.
        self.vbe.lfb_base = self.vga.s3.lfb_base().filter(|&base| base as usize >= self.ram.len());
        match self.vga.s3.format(&self.vga) {
            Some(format) => {
                let pitch = self.vga.s3.offset(&self.vga) * 8;
                let same = self.video_mode == VideoMode::Vesa
                    && self.vbe.mode.is_some_and(|m| (m.width, m.height, m.bpp) == (format.width, format.height, format.bpp))
                    && self.vbe.pitch == pitch;
                if same {
                    return;
                }
                let number = self.vbe.mode.map_or(0, |m| m.number);
                let timing = self.vga.s3.timing(format);
                if self.video_mode != VideoMode::Vesa || self.vbe.mode.map(|m| (m.width, m.height, m.bpp)) != Some((format.width, format.height, format.bpp)) {
                    self.log_string(&format!(
                        "[S3] Enhanced mode {}x{}, {} bits per pixel, {} bytes a line",
                        format.width, format.height, format.bpp, pitch
                    ));
                }
                self.vbe.mode = Some(VbeMode { number, width: format.width, height: format.height, bpp: format.bpp, timing });
                self.vbe.pitch = pitch;
                self.video_mode = VideoMode::Vesa;
                self.vga.set_fixed_timing(Some(timing));
                self.update_vbe_start();
                self.vga.mark_dirty_full();
            }
            None if self.video_mode == VideoMode::Vesa => {
                // Back to the VGA's modes, as the registers have them.
                self.vbe.reset();
                self.vga.set_fixed_timing(None);
                let mode = self.vga.register_mode().or_else(|| self.vga.check_video_mode()).unwrap_or(VideoMode::Text80x25Color);
                self.log_string(&format!("[S3] Back to the VGA's {:?}", mode));
                self.video_mode = mode;
                self.vga.mark_dirty_full();
            }
            None => {}
        }
    }

    /// The graphics engine's view of video memory.
    fn engine_surface(&mut self) -> Surface<'_> {
        let (width, bytes) = self.vga.s3.engine_layout();
        Surface { vram: &mut self.vbe.vram, width, bytes }
    }

    /// Write the graphics engine's port or memory-mapped register `port`.
    pub(crate) fn engine_write(&mut self, port: u16, value: u32, len: u8) {
        let mut engine = std::mem::take(&mut self.s3_engine);
        let drew = engine.write(port, value, len, &mut self.engine_surface());
        self.s3_engine = engine;
        if drew && self.video_mode == VideoMode::Vesa {
            self.vga.mark_dirty_full();
        }
    }

    pub(crate) fn engine_read(&mut self, port: u16, len: u8) -> u32 {
        let mut engine = std::mem::take(&mut self.s3_engine);
        let value = engine.read(port, len, &self.engine_surface());
        self.s3_engine = engine;
        value
    }

    /// A memory-mapped register of the graphics engine read.
    pub(crate) fn s3_peek(&self, port: u16, len: u8) -> u32 {
        self.s3_engine.peek(port, len, self.vga.s3.engine_layout().1)
    }

    /// The memory-mapped register (or image transfer offset) at physical
    /// address `addr`, if the S3 maps them there: at A0000h-AFFFFh with
    /// CR53 bit 4, 16 MB above the linear frame buffer with bit 3.
    #[inline]
    pub(crate) fn s3_mmio(&self, addr: usize) -> Option<u16> {
        if !self.s3() {
            return None;
        }
        let s3 = &self.vga.s3;
        if s3.mmio() && (0xA0000..0xB0000).contains(&addr) {
            return Some((addr - 0xA0000) as u16);
        }
        let base = s3.lfb_base()? as usize + 0x100_0000;
        (s3.mmio_high() && (base..base + 0x10000).contains(&addr)).then(|| (addr - base) as u16)
    }

    /// A port access 2 or 4 bytes wide: to the graphics engine whole, to
    /// anything else a byte at a time, low byte first.
    pub fn io_write_wide(&mut self, port: u16, value: u32, len: u8) {
        if self.s3() && is_engine_port(port) {
            self.engine_write(port, value, len);
            return;
        }
        // The PCI configuration address, a doubleword.
        if port == 0xCF8 && len == 4 && self.pci_present() {
            self.pci.address = value;
            return;
        }
        for i in 0..len {
            self.io_write(port.wrapping_add(i as u16), (value >> (8 * i)) as u8);
        }
    }

    pub fn io_read_wide(&mut self, port: u16, len: u8) -> u32 {
        if self.s3() && is_engine_port(port) {
            return self.engine_read(port, len);
        }
        if port == 0xCF8 && len == 4 && self.pci_present() {
            return self.pci.address;
        }
        let mut value = 0;
        for i in 0..len {
            value |= (self.io_read(port.wrapping_add(i as u16)) as u32) << (8 * i);
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_s_ports_are_the_xxe8_family() {
        for port in [0x42E8, 0x46E8, 0x4AE8, 0x82E8, 0x9AE8, 0x9AE9, 0xBEE8, 0xE2E8, 0xE2EA] {
            assert!(is_engine_port(port), "{:04X}", port);
        }
        for port in [0x02E8, 0x03E8, 0x3D4, 0x1E8, 0xC2E8, 0x82EC] {
            assert!(!is_engine_port(port), "{:04X}", port);
        }
    }
}

impl Bus {
    /// Set the S3's registers for the VESA mode `mode` as its BIOS does
    /// (DOSBox-X's `FinishSetMode`): the display end and line offset for
    /// the mode's size, enhanced memory mapping, the pixel format, the
    /// graphics engine's screen, and the linear frame buffer at E0000000h.
    /// What the display shows follows from them (`s3_settle`).
    pub(crate) fn s3_program_mode(&mut self, mode: &VbeMode) {
        let s3 = &mut self.vga.s3;
        s3.unlock();
        let (per_clock, format) = match mode.bpp {
            8 => (8, 0x00),
            15 => (4, 0x30),
            16 => (4, 0x50),
            24 => (8, 0x70),
            _ => (8, 0xD0),
        };
        let hde = mode.width as u32 / per_clock - 1;
        let vde = mode.height as u32 - 1;
        let offset = mode.width as u32 * mode.bytes_per_pixel() as u32 / 8;
        let crtc = &mut self.vga.crtc_regs;
        crtc[0x01] = hde as u8;
        crtc[0x12] = vde as u8;
        crtc[0x07] = (crtc[0x07] & !0x42) | ((vde >> 8 & 1) << 1) as u8 | ((vde >> 9 & 1) << 6) as u8;
        crtc[0x09] &= 0x60;
        crtc[0x13] = offset as u8;
        let width_bits = match mode.width {
            640 => 0x40,
            800 => 0x80,
            1152 => 0x01,
            1280 => 0xC0,
            1600 => 0x81,
            _ => 0x00,
        };
        let depth_bits = match mode.bytes_per_pixel() {
            2 => 0x10,
            4 => 0x30,
            _ => 0x00,
        };
        let (mut bank, mut start_high) = (0, 0);
        for (index, value) in [
            (0x5D, ((hde >> 8 & 1) << 1) as u8),
            (0x5E, ((vde >> 10 & 1) << 1) as u8),
            (0x51, ((offset >> 8 & 3) << 4) as u8),
            (0x69, 0),
            (0x6A, 0),
            (0x3A, 0x15),
            (0x31, 0x09),
            (0x67, format),
            (0x50, width_bits | depth_bits),
            (0x53, 0x00),
            (0x58, 0x13),
            (0x59, 0xE0),
            (0x5A, 0x00),
            (0x6B, 0xE0),
            (0x41, 0x88),
            (0x52, 0x80),
            (0x45, 0x00),
        ] {
            self.vga.s3.write_crtc(index, value, &mut bank, &mut start_high);
        }
        self.vga.s3.write_seq(0x15, 0x03);
        self.vbe.bank = bank;
        self.vbe.start_high = start_high;
        self.s3_settle();
    }

    /// Set the S3's registers back for a standard VGA mode (INT 10h
    /// AH=00h): no enhanced mapping, pixel format or linear frame buffer.
    pub(crate) fn s3_program_standard(&mut self) {
        self.vga.s3.unlock();
        let (mut bank, mut start_high) = (0, 0);
        for (index, value) in [
            (0x31, 0x05),
            (0x3A, 0x05),
            (0x67, 0x00),
            (0x50, 0x00),
            (0x51, 0x00),
            (0x43, 0x00),
            (0x53, 0x00),
            (0x58, 0x03),
            (0x5D, 0x00),
            (0x5E, 0x00),
            (0x45, 0x00),
            (0x69, 0x00),
            (0x6A, 0x00),
        ] {
            self.vga.s3.write_crtc(index, value, &mut bank, &mut start_high);
        }
        self.vbe.bank = bank;
        self.vbe.start_high = start_high;
        self.s3_settle();
    }
}

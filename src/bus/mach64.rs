//! The ATI mach64 on the bus: its ports, its registers in its aperture
//! and in the VGA's window, the window's banks, and the accelerator CRTC
//! taking the screen over from the VGA.

use super::Bus;
use crate::mach64::{Live, MEMORY, REGISTERS, Target, regs};
use crate::video::VideoMode;
use crate::video::vbe::VbeMode;

pub use crate::mach64::bios::extended as bios;

/// Where an access to memory the card decodes goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mmio {
    /// The register area: offset in its 2 KB (block 1, then block 0).
    Registers(u32),
    /// The memory through the aperture's upper (big-endian) half.
    Memory(usize),
}

/// The ports of the DAC_REGS lanes.
const DAC_PORTS: [u16; 4] = [0x3C8, 0x3C9, 0x3C6, 0x3C7];

impl Bus {
    /// Whether the display adapter is a mach64.
    pub fn mach64(&self) -> bool {
        self.vga.adapter.is_mach64()
    }

    /// The register and byte port `port` reaches, if it is the card's.
    #[inline]
    pub(crate) fn mach64_port(&self, port: u16) -> Option<(usize, u8)> {
        if self.mach64() {
            self.mach64.port(port)
        } else {
            None
        }
    }

    /// Where an access at physical address `addr` goes, if to the card's
    /// registers or its memory past the linear frame buffer: the top 2 KB
    /// of each half of the aperture, its upper half, and B000:F800h while
    /// CONFIG_CNTL puts the registers there and the VGA's text window
    /// isn't.
    #[inline]
    pub(crate) fn mach64_mmio(&self, addr: usize) -> Option<Mmio> {
        if !self.mach64() {
            return None;
        }
        if (0xB_F800..0xC_0000).contains(&addr) {
            let text_window = self.vga.graphics_regs[0x06] & 0x0C == 0x0C;
            return (self.mach64.vga_window() && !text_window)
                .then(|| Mmio::Registers((addr - 0xB_F800) as u32));
        }
        let base = self.mach64.aperture()? as usize;
        let offset = addr
            .checked_sub(base)
            .filter(|&o| o < crate::mach64::APERTURE as usize)?;
        let half = offset & 0x7F_FFFF;
        if half >= REGISTERS as usize {
            return Some(Mmio::Registers((half - REGISTERS as usize) as u32));
        }
        (offset >= 0x80_0000 && half < MEMORY as usize).then_some(Mmio::Memory(half))
    }

    /// The CRTC's state for the registers that show it.
    fn mach64_live(&self) -> Live {
        let timing = self.vga.peek_timing();
        let now = self.clock.now_ns();
        let (line, _) = timing.position(now);
        Live {
            line,
            vblank: line >= timing.display,
            retraces: timing.retraces(now),
        }
    }

    /// A register read through a port: reads of the DAC's data go on to
    /// the next colour.
    pub(crate) fn mach64_read(&mut self, reg: usize, mask: u32) -> u32 {
        if reg == regs::DAC_REGS {
            let mut value = 0;
            for (lane, port) in DAC_PORTS.into_iter().enumerate() {
                if mask >> (8 * lane) & 0xFF != 0 {
                    value |= (self.io_read(port) as u32) << (8 * lane);
                }
            }
            return value;
        }
        self.mach64_peek(reg, mask)
    }

    /// A register as it reads, without what reading it does.
    pub(crate) fn mach64_peek(&self, reg: usize, mask: u32) -> u32 {
        if reg == regs::DAC_REGS {
            let vga = &self.vga;
            let data = vga
                .palette
                .get(vga.dac_read_index as usize * 3 + vga.dac_step as usize)
                .copied()
                .unwrap_or(0);
            let lanes = [vga.dac_write_index, data, vga.dac_mask, vga.dac_state];
            return u32::from_le_bytes(lanes) & mask;
        }
        self.mach64.read(reg, self.mach64_live()) & mask
    }

    /// The bytes `mask` selects of `value` written to register `reg`.
    pub(crate) fn mach64_write(&mut self, reg: usize, value: u32, mask: u32) {
        match reg {
            regs::DAC_REGS => {
                self.mach64
                    .trace(|| format!("w DAC_REGS = {:08X} & {:08X}", value, mask));
                for (lane, port) in DAC_PORTS.into_iter().enumerate() {
                    if mask >> (8 * lane) & 0xFF != 0 {
                        self.io_write(port, (value >> (8 * lane)) as u8);
                    }
                }
            }
            regs::DAC_CNTL => {
                let kept = mask & regs::write_mask(reg);
                let new = self.mach64.reg(reg) & !kept | value & kept;
                self.mach64.set_reg(reg, new);
                let wide = new & 0x100 != 0;
                if self.vga.dac_8bit != wide {
                    self.vga.dac_8bit = wide;
                    self.vga.mark_dirty_full();
                }
            }
            _ => {
                let live = self.mach64_live();
                let mut target = Target {
                    vram: &mut self.vbe.vram,
                    dac: &self.vga.palette,
                    dac_8bit: self.vga.dac_8bit,
                };
                let fx = self.mach64.write(reg, value, mask, live, &mut target);
                if fx.display {
                    self.mach64_settle();
                }
                if (fx.drawn || fx.cursor) && self.video_mode == VideoMode::Vesa {
                    self.vga.mark_dirty_full();
                }
            }
        }
        self.mach64_log();
    }

    /// A port access of `len` bytes at `port`, one of the card's.
    pub(crate) fn mach64_port_write(&mut self, port: u16, value: u32, len: u8) {
        for (reg, lane, bytes, shift) in Self::mach64_lanes(port, len, |p| self.mach64.port(p)) {
            self.mach64_write(reg, value >> shift << (8 * lane), lanes_mask(lane, bytes));
        }
    }

    pub(crate) fn mach64_port_read(&mut self, port: u16, len: u8) -> u32 {
        let mut value = 0;
        for (reg, lane, bytes, shift) in Self::mach64_lanes(port, len, |p| self.mach64.port(p)) {
            let mask = lanes_mask(lane, bytes);
            value |= (self.mach64_read(reg, mask) & mask) >> (8 * lane) << shift;
        }
        value
    }

    /// The registers an access of `len` bytes from `port` reaches: for
    /// each, the register, its first byte, how many bytes and where they
    /// are in the access.
    fn mach64_lanes(
        port: u16,
        len: u8,
        decode: impl Fn(u16) -> Option<(usize, u8)>,
    ) -> Vec<(usize, u8, u8, u32)> {
        let mut parts = Vec::new();
        let mut i = 0;
        while i < len {
            let Some((reg, lane)) = decode(port.wrapping_add(i as u16)) else {
                i += 1;
                continue;
            };
            let bytes = (4 - lane).min(len - i);
            parts.push((reg, lane, bytes, 8 * i as u32));
            i += bytes;
        }
        parts
    }

    /// A write of `len` bytes of `value` at `at`.
    pub(crate) fn mach64_mmio_write(&mut self, at: Mmio, value: u32, len: u8) {
        match at {
            Mmio::Memory(offset) => {
                let bytes = value.to_le_bytes();
                for i in 0..len as usize {
                    self.write_vram((offset + i) % MEMORY as usize, &bytes[i..i + 1]);
                }
            }
            Mmio::Registers(offset) => {
                let card = &self.mach64;
                for (reg, lane, bytes, shift) in Self::mach64_lanes(0, len, |i| {
                    card.register_at(offset + i as u32)
                        .map(|reg| (reg, ((offset + i as u32) & 3) as u8))
                }) {
                    self.mach64_write(reg, value >> shift << (8 * lane), lanes_mask(lane, bytes));
                }
            }
        }
    }

    /// A read of `len` bytes at `at`, without what reading does.
    pub(crate) fn mach64_mmio_read(&self, at: Mmio, len: u8) -> u32 {
        match at {
            Mmio::Memory(offset) => (0..len as usize)
                .map(|i| (self.vbe.vram[(offset + i) % MEMORY as usize] as u32) << (8 * i))
                .sum(),
            Mmio::Registers(offset) => {
                let mut value = 0;
                for i in 0..len as u32 {
                    let byte = match self.mach64.register_at(offset + i) {
                        Some(reg) => {
                            let lane = (offset + i) & 3;
                            self.mach64_peek(reg, 0xFF << (8 * lane)) >> (8 * lane) & 0xFF
                        }
                        None => 0xFF,
                    };
                    value |= byte << (8 * i);
                }
                value
            }
        }
    }

    /// Where a byte at `offset` of the window at A0000h is in the video
    /// memory in a VESA mode: on a mach64 through its 32 KB page
    /// registers, elsewhere the VBE's bank.
    #[inline]
    pub(crate) fn vesa_window(&self, offset: usize, write: bool) -> usize {
        if self.mach64() {
            self.mach64.window_page(offset, write)
        } else {
            self.vbe.window_offset(offset)
        }
    }

    /// Set both of the window's pages for 64 KB bank `bank`, as the VBE
    /// does.
    pub(crate) fn mach64_set_bank(&mut self, bank: u32) {
        let pages = ((2 * bank) & 0xFF) | (((2 * bank + 1) & 0xFF) << 16);
        self.mach64.set_reg(regs::MEM_VGA_WP_SEL, pages);
        self.mach64.set_reg(regs::MEM_VGA_RP_SEL, pages);
    }

    /// The aperture moved, or the card stopped decoding memory: the
    /// linear frame buffer is where it is.
    pub(crate) fn mach64_moved(&mut self) {
        self.vbe.lfb_base = self
            .mach64
            .aperture()
            .filter(|&base| base as usize >= self.ram.len());
    }

    /// Show what the accelerator CRTC describes, as a VESA mode, or go
    /// back to the VGA's when it lets go of the screen.
    pub(crate) fn mach64_settle(&mut self) {
        match self.mach64.screen() {
            Some(screen) => {
                let size = (screen.width, screen.height, screen.bpp);
                let current = self
                    .vbe
                    .mode
                    .filter(|_| self.mach64.native && self.video_mode == VideoMode::Vesa);
                // A BIOS mode keeps its number.
                let number = self
                    .vbe
                    .mode
                    .filter(|m| (m.width, m.height, m.bpp) == size)
                    .map_or(0, |m| m.number);
                let mode = VbeMode {
                    number,
                    width: screen.width,
                    height: screen.height,
                    bpp: screen.bpp,
                    timing: screen.timing,
                };
                if current != Some(mode) || self.vbe.pitch != screen.pitch {
                    if current.map(|m| (m.width, m.height, m.bpp)) != Some(size) {
                        let line = format!(
                            "[MACH64] Display {}x{}, {} bits per pixel, {} bytes a line, {:.1} Hz",
                            screen.width,
                            screen.height,
                            screen.bpp,
                            screen.pitch,
                            screen.timing.hz()
                        );
                        self.log_string(&line);
                    }
                    self.mach64.native = true;
                    self.vbe.mode = Some(mode);
                    self.vbe.pitch = screen.pitch;
                    self.video_mode = VideoMode::Vesa;
                    self.vga.set_fixed_timing(Some(screen.timing));
                    self.vga.mark_dirty_full();
                }
                if self.vbe.start != screen.base {
                    self.vbe.start = screen.base;
                    self.note_display_start();
                }
            }
            None if self.mach64.native => {
                self.mach64.native = false;
                self.vbe.reset();
                self.vga.set_fixed_timing(None);
                let mode = self
                    .vga
                    .register_mode()
                    .or_else(|| self.vga.check_video_mode())
                    .unwrap_or(VideoMode::Text80x25Color);
                self.log_string(&format!("[MACH64] Back to the VGA's {:?}", mode));
                self.video_mode = mode;
                self.vga.mark_dirty_full();
            }
            None => {}
        }
    }

    /// A standard mode set through the BIOS (INT 10h AH=00h): the VGA
    /// shows the screen, with a 6-bit DAC and the window's pages at 0.
    pub(crate) fn mach64_mode_set(&mut self) {
        if !self.mach64() {
            return;
        }
        let gen_cntl = self.mach64.reg(regs::CRTC_GEN_CNTL);
        self.mach64_write(regs::CRTC_GEN_CNTL, gen_cntl & !0x0300_0000, u32::MAX);
        self.mach64_write(
            regs::DAC_CNTL,
            self.mach64.reg(regs::DAC_CNTL) & !0x100,
            u32::MAX,
        );
        self.mach64_set_bank(0);
    }

    pub(crate) fn mach64_log(&mut self) {
        if self.mach64.log.is_empty() {
            return;
        }
        for line in std::mem::take(&mut self.mach64.log) {
            self.log_string(&line);
        }
    }
}

/// The mask of `bytes` bytes from byte `lane` of a register.
fn lanes_mask(lane: u8, bytes: u8) -> u32 {
    let low = if bytes >= 4 {
        u32::MAX
    } else {
        (1u32 << (8 * bytes)) - 1
    };
    low << (8 * lane)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_of_lanes() {
        assert_eq!(lanes_mask(0, 4), u32::MAX);
        assert_eq!(lanes_mask(2, 2), 0xFFFF_0000);
        assert_eq!(lanes_mask(3, 1), 0xFF00_0000);
    }
}

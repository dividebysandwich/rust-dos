//! The bus's side of the Voodoo Banshee and Voodoo3 (`voodoo::banshee`):
//! accesses of any size to its BARs, its I/O BAR with the VGA's ports in
//! it, and the VGA registers it shares with `VgaCard`, which decide its
//! extended modes' size and timing.

use super::Bus;
use crate::voodoo::banshee::{self, Banshee};

impl Bus {
    pub(crate) fn banshee(&self) -> Option<&Banshee> {
        self.voodoo.as_ref()?.banshee()
    }

    fn banshee_mut(&mut self) -> Option<&mut Banshee> {
        self.voodoo.as_mut()?.banshee.as_deref_mut()
    }

    /// `len` bytes of the card's window from `offset`, any alignment.
    pub(crate) fn banshee_read_bytes(&self, offset: u32, len: u32) -> u32 {
        let Some(v) = &self.voodoo else {
            return u32::MAX;
        };
        if vga_in_bar0(offset, len) {
            return (0..len)
                .map(|i| (self.banshee_vga_peek(offset + i) as u32) << (8 * i))
                .sum();
        }
        let now = self.voodoo_now();
        let shift = 8 * (offset & 3);
        let mut value = (v.read(offset & !3, now) as u64) >> shift;
        if (offset & 3) + len > 4 {
            value |= (v.read((offset & !3).wrapping_add(4), now) as u64) << (32 - shift);
        }
        let mask = if len >= 4 {
            u32::MAX
        } else {
            (1 << (8 * len)) - 1
        };
        value as u32 & mask
    }

    /// `len` bytes of `value` written to the card's window at `offset`,
    /// as the dword writes with byte enables the card takes.
    pub(crate) fn banshee_write_bytes(&mut self, offset: u32, value: u32, len: u32) {
        if vga_in_bar0(offset, len) {
            for i in 0..len {
                let at = (offset + i) & 0xFF;
                let byte = (value >> (8 * i)) as u8;
                if (0xB0..0xE0).contains(&at) {
                    self.io_write(0x300 + at as u16, byte);
                } else {
                    self.banshee_port_write(at, byte);
                }
            }
            return;
        }
        let shift = 8 * (offset & 3);
        let bytes = if len >= 4 {
            u32::MAX as u64
        } else {
            (1u64 << (8 * len)) - 1
        };
        let (data, mask) = ((value as u64) << shift, bytes << shift);
        self.voodoo_write(offset & !3, data as u32, mask as u32);
        if mask >> 32 != 0 {
            self.voodoo_write(
                (offset & !3).wrapping_add(4),
                (data >> 32) as u32,
                (mask >> 32) as u32,
            );
        }
    }

    /// A byte of the VGA's ports in BAR0 as a read of the port gives it,
    /// without what reading it does: reads of memory can't change the
    /// machine (`mmio_peek` does the same for the ViRGE).
    fn banshee_vga_peek(&self, offset: u32) -> u8 {
        let at = offset & 0xFF;
        if !(0xB0..0xE0).contains(&at) {
            let Some(v) = &self.voodoo else { return 0xFF };
            return (v.banshee_ext_read(at & !3, self.voodoo_now()) >> (8 * (at & 3))) as u8;
        }
        let port = 0x300 + at as u16;
        match port {
            0x3B5 | 0x3D5 if self.vga.crtc_index >= 0x19 => {
                self.banshee_crtc_read(self.vga.crtc_index)
            }
            _ => self.vga_peek(port),
        }
    }

    /// The register of the card's I/O BAR at `port`, if it is one.
    #[inline]
    pub(crate) fn banshee_io(&self, port: u16) -> Option<u32> {
        self.banshee()?.io_offset(port)
    }

    /// A byte written to the I/O BAR: its VGA ports (B0h-DFh) are the
    /// VGA's at 3B0h-3DFh, the rest the init and video registers.
    pub(crate) fn banshee_port_write(&mut self, offset: u32, value: u8) {
        if (0xB0..0xE0).contains(&offset) {
            self.write_port(0x300 + offset as u16, value);
            return;
        }
        self.banshee_ext_write(
            offset & !3,
            (value as u32) << (8 * (offset & 3)),
            0xFF << (8 * (offset & 3)),
        );
    }

    pub(crate) fn banshee_port_read(&mut self, offset: u32, now_ns: u64) -> u8 {
        if (0xB0..0xE0).contains(&offset) {
            return self.read_port(0x300 + offset as u16, now_ns);
        }
        let Some(v) = &self.voodoo else { return 0xFF };
        (v.banshee_ext_read(offset & !3, self.voodoo_now()) >> (8 * (offset & 3))) as u8
    }

    /// A word or dword of the I/O BAR written at once, which goes to one
    /// init register; the VGA's ports a byte at a time.
    pub(crate) fn banshee_port_write_wide(&mut self, offset: u32, value: u32, len: u8) {
        if (0xB0..0xE0).contains(&offset) || (offset & 3) + len as u32 > 4 {
            for i in 0..len as u32 {
                self.banshee_port_write(offset + i, (value >> (8 * i)) as u8);
            }
            return;
        }
        let shift = 8 * (offset & 3);
        let mask = if len == 4 { u32::MAX } else { 0xFFFF << shift };
        self.banshee_ext_write(offset & !3, value << shift, mask);
    }

    pub(crate) fn banshee_port_read_wide(&mut self, offset: u32, len: u8, now_ns: u64) -> u32 {
        if (0xB0..0xE0).contains(&offset) || (offset & 3) + len as u32 > 4 {
            return (0..len as u32)
                .map(|i| (self.banshee_port_read(offset + i, now_ns) as u32) << (8 * i))
                .sum();
        }
        let Some(v) = &self.voodoo else {
            return u32::MAX;
        };
        let value = v.banshee_ext_read(offset & !3, self.voodoo_now()) >> (8 * (offset & 3));
        if len == 4 { value } else { value & 0xFFFF }
    }

    /// The bytes `mask` selects of init register `offset` written: as
    /// through BAR0.
    fn banshee_ext_write(&mut self, offset: u32, value: u32, mask: u32) {
        self.voodoo_write(offset & 0xFC, value, mask);
    }

    /// A VGA register the card shares was written (`port` with what the
    /// VGA now holds): mirror what the video processor needs and work the
    /// display out again.
    pub(crate) fn banshee_vga_written(&mut self, port: u16) {
        if self.banshee().is_none() {
            return;
        }
        match port {
            0x3C9 => {
                // An entry completes with its blue.
                if self.vga.dac_step != 0 {
                    return;
                }
                let index = self.vga.dac_write_index.wrapping_sub(1);
                let at = index as usize * 3;
                let wide = self.vga.dac_8bit;
                let scale = |c: u8| if wide { c } else { (c << 2) | (c >> 4) };
                let p = &self.vga.palette;
                let rgb = (scale(p[at]), scale(p[at + 1]), scale(p[at + 2]));
                if let Some(b) = self.banshee_mut() {
                    b.vga_palette_written(index, rgb);
                }
            }
            // The cursor's shape and place and the start address (0Ah-0Fh)
            // change neither the timing nor the desktop's size.
            0x3B5 | 0x3D5 if (0x0A..=0x0F).contains(&self.vga.crtc_index) => {}
            0x3C2 | 0x3C5 | 0x3B5 | 0x3D5 => self.banshee_settle(),
            _ => {}
        }
    }

    /// A Super VGA CRTC register (19h-3Fh) of the card's.
    pub(crate) fn banshee_crtc_write(&mut self, index: u8, value: u8) {
        if let Some(b) = self.banshee_mut()
            && (index as usize) < b.crtc.len()
        {
            b.crtc[index as usize] = value;
        }
        self.banshee_settle();
    }

    pub(crate) fn banshee_crtc_read(&self, index: u8) -> u8 {
        self.banshee()
            .and_then(|b| b.crtc.get(index as usize).copied())
            .unwrap_or(0)
    }

    /// Work out the display again after the card's mode registers
    /// changed: the VGA's CRTC with the card's extra bits and PLL make the
    /// timing, vgaInit0 the DAC's width.
    pub(crate) fn banshee_settle(&mut self) {
        let crtc = self.vga.crtc_regs;
        let (misc, seq01) = (self.vga.misc_output_reg, self.vga.sequencer_regs[1]);
        let now = self.voodoo_now();
        if let Some(v) = &mut self.voodoo
            && v.banshee().is_some_and(|b| b.crtc[0x11] != crtc[0x11])
        {
            v.banshee_crtc11(crtc[0x11], now);
        }
        let Some(b) = self.banshee_mut() else { return };
        b.crtc[..crtc.len()].copy_from_slice(&crtc);
        b.misc_output = misc;
        b.seq01 = seq01;
        let ext = b.timing_extension();
        let extended = b.extended();
        if self.vga.svga_ext != Some(ext) {
            self.vga.svga_ext = Some(ext);
            self.vga.invalidate_timing();
        }
        let timing = self.vga.timing();
        if let Some(v) = &mut self.voodoo {
            v.set_timing(timing);
        }
        if extended {
            self.vga.mark_dirty_full();
        }
        self.sync_banshee_irq();
    }

    /// Drive the card's interrupt line, the vertical retrace interrupt's:
    /// raise the request when it goes up, withdraw it when it drops.
    pub(crate) fn sync_banshee_irq(&mut self) {
        let now = self.voodoo_now();
        let Some(v) = &mut self.voodoo else { return };
        let line = v.banshee_irq(now);
        let Some(b) = v.banshee.as_deref_mut() else { return };
        let old = std::mem::replace(&mut b.irq_up, line);
        // An interrupt armed is among the timer events.
        self.clock.schedule(self.next_event());
        if old == line {
            return;
        }
        if let Some(old) = old {
            self.pic.lower(old);
        }
        if let Some(irq) = line {
            self.pic.raise(irq);
        }
        self.refresh_irq();
    }

    /// Withdraw the request the card has up, before a reset or the card
    /// going away forgets it.
    pub(crate) fn lower_banshee_irq(&mut self) {
        let up = self.banshee_mut().and_then(|b| b.irq_up.take());
        if let Some(irq) = up {
            self.pic.lower(irq);
            self.refresh_irq();
        }
    }

    /// When the card's vertical retrace interrupt comes up, if it is
    /// armed.
    pub(crate) fn banshee_next_event(&self) -> Option<u64> {
        self.voodoo.as_ref()?.banshee_vint_due()
    }

    /// Input Status 0 with the card's: bit 7, a vertical retrace
    /// interrupt pending.
    pub(crate) fn banshee_input_status(&mut self, value: u8) -> u8 {
        let now = self.voodoo_now();
        match &self.voodoo {
            Some(v) if v.banshee_vint_pending(now) => value | 0x80,
            _ => value,
        }
    }

    /// The card's init or video registers were written: vgaInit0 sets the
    /// DAC's width, and the rest may change the display.
    pub(crate) fn banshee_settle_ext(&mut self) {
        if let Some(b) = self.banshee() {
            self.vga.dac_8bit = b.reg(banshee::VGA_INIT0) & banshee::VGAINIT0_DAC_8BIT != 0;
        }
        self.banshee_settle();
    }

    /// The BIOS set a VGA or VESA mode: the card's own modes end.
    pub(crate) fn banshee_mode_set(&mut self) {
        if let Some(b) = self.banshee_mut() {
            b.bios_mode_set();
            self.banshee_settle();
        }
    }
}

/// Whether an access of `len` bytes at `offset` of BAR0 touches the
/// VGA's ports in the init registers' space (B0h-DFh of every 256 bytes
/// below 80000h), which the card passes to 3B0h-3DFh as its I/O BAR does.
fn vga_in_bar0(offset: u32, len: u32) -> bool {
    offset < 0x8_0000 && (0..len).any(|i| (0xB0..0xE0).contains(&((offset + i) & 0xFF)))
}

#[cfg(test)]
mod tests {
    use crate::cpu::Cpu;
    use crate::voodoo::Board;

    #[test]
    fn bar0_reaches_the_vga_ports() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.configure_voodoo(Some(Board::Voodoo3));
        let bus = &mut cpu.bus;
        // CRTC index and data in one word, as at 3D4h.
        bus.voodoo_write_16(0xD4, 0x2A13);
        assert_eq!(bus.vga.crtc_regs[0x13], 0x2A);
        assert_eq!(bus.voodoo_read_8(0xD5), 0x2A);
        // The same 256 bytes again higher up, and the card's own CRTC
        // registers past 18h.
        bus.voodoo_write_8(0x1_00D4, 0x1A);
        bus.voodoo_write_8(0x1_00D5, 0x04);
        assert_eq!(bus.banshee_crtc_read(0x1A), 0x04);
        assert_eq!(bus.voodoo_read_16(0xD4), 0x041A);
    }
}

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
        // The memory through BAR1 where it is in order: only the bytes
        // and what the card does for them.
        if let Some(addr) = v.linear_run(offset, len) {
            return v.banshee_linear_load(addr, len);
        }
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
        if let Some(addr) = self.voodoo.as_ref().and_then(|v| v.linear_run(offset, len)) {
            self.banshee_linear_store(addr, value, len);
            return;
        }
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
        if offset >= banshee::LINEAR
            && let Some(b) = self.banshee()
        {
            let addr = b.linear_addr(offset - banshee::LINEAR);
            self.note_banshee_memory_write(addr, len);
        }
    }

    /// `len` bytes (1 to 4) of `value` written into the card's memory
    /// from byte `addr` on, which is below the tile base and outside the
    /// command FIFOs' rings (`Voodoo::linear_run`): what the writes of
    /// their dwords through the window do, a swap held back waited for
    /// and `cycles=auto` told, without the way through its registers.
    fn banshee_linear_store(&mut self, addr: u32, value: u32, len: u32) {
        let _zone = crate::profile::zone(crate::profile::Zone::Voodoo);
        self.voodoo_swap_wait();
        if let Some(v) = &mut self.voodoo {
            v.banshee_linear_store(addr, value, len);
        }
        self.note_banshee_memory_write(addr, len);
    }

    /// `len` bytes (1, 2 or 4) at physical address `addr`, if they are
    /// all a Banshee's or Voodoo3's memory through BAR1 in order
    /// (`banshee_linear_at`), as `read_8`, `read_16` and `read_32` read
    /// them there: for translated code's reads, which come with the
    /// address of something that isn't plain RAM and would go through
    /// every window before the card's. None, having read nothing, where
    /// they are something else.
    #[cfg_attr(not(dynrec), allow(dead_code))]
    #[inline]
    pub(crate) fn read_banshee_linear(&self, addr: usize, len: usize) -> Option<u32> {
        let at = self.banshee_linear_at(addr, len)?;
        let _profile = self.profile.as_deref().map(|p| p.mem(addr, false));
        if self.observe.on {
            self.observe.mapped(addr, self.ram.len());
        }
        Some(self.voodoo.as_ref()?.banshee_linear_load(at, len as u32))
    }

    /// The same for a write, as `write_8`, `write_16` and `write_32` make
    /// one there. False, having written nothing, where the bytes are
    /// something else.
    #[cfg_attr(not(dynrec), allow(dead_code))]
    #[inline]
    pub(crate) fn write_banshee_linear(&mut self, addr: usize, value: u32, len: usize) -> bool {
        let Some(at) = self.banshee_linear_at(addr, len) else {
            return false;
        };
        let _profile = self.profile.as_deref().map(|p| p.mem(addr, true));
        if self.observe.on {
            self.observe.mapped_write(addr, self.ram.len());
        }
        self.banshee_linear_store(at, value, len as u32);
        true
    }

    /// Where translated code may read and write a Banshee's or Voodoo3's
    /// memory by itself (`Voodoo::banshee_direct`): BAR1's physical
    /// address and the memory there. None while an access does more than
    /// move the bytes: the card's own reasons, a proof of an idle loop
    /// that follows every access (`idle`), a VESA frame buffer over the
    /// BAR, or another card whose windows the bus looks at before this
    /// one's (`banshee_linear_at`).
    #[cfg_attr(not(dynrec), allow(dead_code))]
    pub(crate) fn banshee_direct(&self) -> Option<(u32, banshee::direct::Direct)> {
        let v = self.voodoo.as_ref()?;
        let b = v.banshee()?;
        if self.observe.on
            || self.s3()
            || self.mach64()
            || self.powervr.is_some()
            || self.toadforce.is_some()
        {
            return None;
        }
        let direct = v.banshee_direct()?;
        let bar1 = b.bar1()?;
        let end = bar1 as u64 + direct.len as u64;
        if (bar1 as usize) < self.ram.len() || end > 0xFFFE_0000 {
            return None;
        }
        if let Some(lfb) = self.vbe.lfb_base {
            let lfb = lfb as u64;
            if lfb < end && (bar1 as u64) < lfb + self.vbe.vram.len() as u64 {
                return None;
            }
        }
        Some((bar1, direct))
    }

    /// Translated code wrote the card's memory by itself.
    #[cfg_attr(not(dynrec), allow(dead_code))]
    pub(crate) fn banshee_wrote_directly(&mut self) {
        if let Some(v) = &mut self.voodoo {
            v.banshee_wrote_directly();
        }
    }

    /// The CPU wrote `len` bytes into the card's memory from byte `addr`
    /// on. What lands in the desktop shown counts for `cycles=auto` as a
    /// write into a VESA frame buffer does: a game under Windows that
    /// copies its frames into the primary surface itself draws them there.
    #[inline]
    pub(crate) fn note_banshee_memory_write(&mut self, addr: u32, len: u32) {
        let Some(b) = self.banshee().filter(|b| b.extended()) else {
            return;
        };
        let (start, span) = b.desktop_memory();
        let start = start as u64;
        let from = (addr as u64).max(start);
        let to = (addr as u64 + len as u64).min(start + span);
        if from < to {
            self.note_video_write((from - start) as usize, to - from);
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

    /// The register of the card's I/O BAR at `port`, if it is one. With
    /// the BAR moved to 300h its VGA ports are the VGA's own, which the
    /// port paths reach without going round through the BAR again.
    #[inline]
    pub(crate) fn banshee_io(&self, port: u16) -> Option<u32> {
        let offset = self.banshee()?.io_offset(port)?;
        (!(0x3B0..0x3E0).contains(&port)).then_some(offset)
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
            v.set_timing(timing, now);
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
        let Some(b) = v.banshee.as_deref_mut() else {
            return;
        };
        let old = std::mem::replace(&mut b.irq_up, line);
        // An interrupt armed is among the timer events.
        self.clock.schedule(self.next_event());
        if old == line {
            return;
        }
        if let Some(old) = old {
            self.withdraw_irq(old);
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
            self.withdraw_irq(irq);
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

    /// Put back the registers a video BIOS service changed (`mode_regs`),
    /// and the display they make with the VGA's.
    pub(crate) fn banshee_restore_mode_regs(&mut self, regs: &banshee::ModeRegs) {
        if let Some(b) = self.banshee_mut() {
            b.restore_mode_regs(regs);
            self.banshee_settle();
        }
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

    #[test]
    fn a_write_of_the_desktop_start_is_a_flip() {
        use crate::voodoo::banshee::{VGA_INIT0, VID_DESKTOP_START_ADDR, VID_PROC_CFG};
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.configure_voodoo(Some(Board::Voodoo3));
        let bus = &mut cpu.bus;
        bus.voodoo_write_32(VGA_INIT0, 0x0000_1140);
        bus.voodoo_write_32(VID_PROC_CFG, 0x0904_0081);
        assert!(bus.banshee().unwrap().extended());
        // A guest driver's page flips: through BAR0, then the I/O BAR.
        let flips = bus.activity.flips;
        bus.clock.icount += 10_000;
        bus.voodoo_write_32(VID_DESKTOP_START_ADDR, 0x10_0000);
        bus.clock.icount += 10_000;
        bus.banshee_port_write_wide(VID_DESKTOP_START_ADDR, 0x20_0000, 4);
        assert_eq!(bus.activity.flips, flips + 2);
        // With the video processor off, as a mode set has it, the start
        // shows nothing: no flip.
        bus.voodoo_write_32(VID_PROC_CFG, 0x0904_0080);
        bus.clock.icount += 10_000;
        bus.voodoo_write_32(VID_DESKTOP_START_ADDR, 0);
        assert_eq!(bus.activity.flips, flips + 2);
    }

    #[test]
    fn a_service_for_a_dos_box_in_a_window_keeps_the_desktop() {
        use crate::video::echo::Display;
        use crate::voodoo::banshee::{VGA_INIT0, VID_PROC_CFG};
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.configure_voodoo(Some(Board::Voodoo3));
        let bus = &mut cpu.bus;
        bus.voodoo_write_32(VGA_INIT0, 0x0000_1140);
        bus.voodoo_write_32(VID_PROC_CFG, 0x0904_0081);
        bus.banshee_crtc_write(0x1A, 0x40);
        assert!(bus.banshee().unwrap().extended());
        // A mode set for a DOS box whose ports Windows traps works on the
        // card, which is then put back as it was.
        let display = Display::of(bus);
        bus.banshee_mode_set();
        assert!(!bus.banshee().unwrap().extended());
        display.restore(bus);
        let b = bus.banshee().unwrap();
        assert!(b.extended());
        assert_eq!(b.reg(VID_PROC_CFG), 0x0904_0081);
        assert_eq!(bus.banshee_crtc_read(0x1A), 0x40);
    }

    /// A Voodoo3 showing a desktop from 8000h of its memory, its rows (as
    /// many as the VGA's mode has lines) 16 bytes apart, which ends in
    /// the page after: BAR1 is tiled from 4 MB on.
    fn card_with_a_desktop() -> Box<Cpu> {
        use crate::voodoo::banshee::{
            LFB_MEMORY_CONFIG, VGA_INIT0, VID_DESKTOP_OVERLAY_STRIDE, VID_DESKTOP_START_ADDR,
            VID_PROC_CFG,
        };
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.configure_voodoo(Some(Board::Voodoo3));
        let bus = &mut cpu.bus;
        bus.voodoo_write_32(LFB_MEMORY_CONFIG, 0x400);
        bus.voodoo_write_32(VGA_INIT0, 0x0000_1140);
        bus.voodoo_write_32(VID_PROC_CFG, 0x0004_0081);
        bus.voodoo_write_32(VID_DESKTOP_OVERLAY_STRIDE, 0x10);
        bus.voodoo_write_32(VID_DESKTOP_START_ADDR, 0x8000);
        let (start, span) = bus.banshee().unwrap().desktop_memory();
        assert_eq!(start, 0x8000);
        assert!((0x1010..0x2000).contains(&span), "{span:#x} bytes");
        cpu
    }

    #[test]
    fn the_linear_memory_is_read_and_written_as_through_the_bus() {
        use crate::voodoo::banshee::INITIAL_BAR0;
        const BAR1: usize = crate::voodoo::banshee::INITIAL_BAR1 as usize;
        let (mut a, mut b) = (card_with_a_desktop(), card_with_a_desktop());
        let (a, b) = (&mut a.bus, &mut b.bus);
        let mut value = 0x0103_0507u32;
        let (_, span) = a.banshee().unwrap().desktop_memory();
        // Out of sight, across the desktop's first byte, in it and
        // across its last.
        for at in [0x7001usize, 0x7FFD, 0x8003, 0x8000 + span as usize - 0x22] {
            for len in [1usize, 2, 4] {
                value = value.wrapping_mul(0x0101_0101).wrapping_add(len as u32);
                let addr = BAR1 + at + 8 * len;
                match len {
                    1 => _ = a.write_8(addr, value as u8),
                    2 => _ = a.write_16(addr, value as u16),
                    _ => a.write_32(addr, value),
                }
                assert!(b.write_banshee_linear(addr, value, len));
                let read = match len {
                    1 => a.read_8(addr) as u32,
                    2 => a.read_16(addr) as u32,
                    _ => a.read_32(addr),
                };
                assert_eq!(b.read_banshee_linear(addr, len), Some(read), "{len} at {at:#x}");
                a.clock.icount += 1;
                b.clock.icount += 1;
            }
        }
        let memory = |bus: &crate::bus::Bus| {
            let mut bytes = vec![0; 0x4000];
            bus.voodoo.as_ref().unwrap().banshee_memory_read(0x7000, &mut bytes);
            bytes
        };
        assert!(memory(a) == memory(b), "the card's memory differs");
        // What landed in the desktop counts for `cycles=auto` the same.
        assert_ne!(a.activity.video_bytes, 0);
        assert_eq!(a.activity.video_bytes, b.activity.video_bytes);
        // The registers, the tiles and what is past the BAR stay the
        // window's.
        for addr in [INITIAL_BAR0 as usize + 0x28, BAR1 + 0x40_0000, BAR1 + 0x3F_FFFE, BAR1 - 2] {
            assert_eq!(b.read_banshee_linear(addr, 4), None, "{addr:#x}");
            assert!(!b.write_banshee_linear(addr, 0, 4), "{addr:#x}");
        }
        assert!(b.read_banshee_linear(BAR1 + 0x3F_FFFC, 4).is_some());
    }

    #[test]
    fn direct_memory_leaves_the_desktop_and_observed_accesses_to_the_bus() {
        const BAR1: u32 = crate::voodoo::banshee::INITIAL_BAR1;
        let mut cpu = card_with_a_desktop();
        let bus = &mut cpu.bus;
        let (at, direct) = bus.banshee_direct().expect("a quiet card");
        assert_eq!((at, direct.len), (BAR1, 0x40_0000));
        let (_, span) = bus.banshee().unwrap().desktop_memory();
        assert_eq!(direct.kept, [(0x8000, 0x8000 + span as u32), (0, 0), (0, 0)]);
        // A proof of an idle loop follows every access (`idle`).
        bus.observe.on = true;
        assert!(bus.banshee_direct().is_none());
        bus.observe.on = false;
        // The VESA frame buffer comes before the card on the bus.
        let lfb = bus.vbe.lfb_base.replace(BAR1 + 0x1000);
        assert!(bus.banshee_direct().is_none());
        bus.vbe.lfb_base = lfb;
        assert!(bus.banshee_direct().is_some());
        // The card's memory BARs switched off.
        bus.banshee_mut().unwrap().pci.command = 0x01;
        assert!(bus.banshee_direct().is_none());
    }
}

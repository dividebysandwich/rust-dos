//! The 3dfx card on the bus: its 16 MB window where BAR0 puts it, with
//! the access sizes the card takes (DOSBox-X's `Voodoo_PageHandler`), the
//! time its writes take when its FIFO is full, its swaps' retraces among
//! the timer events, and putting it in or taking it out.

use super::Bus;
use crate::voodoo::{Board, Effect, Now, Voodoo, WINDOW};

impl Bus {
    /// Put in the 3dfx card `board` (None: take it out). A card already
    /// there of the same kind stays as it is.
    pub fn configure_voodoo(&mut self, board: Option<Board>) {
        if self.voodoo.as_ref().map(|v| v.board) == board {
            return;
        }
        self.voodoo = board.map(Voodoo::new);
        self.vga.mark_dirty_full();
        self.clock.schedule(self.next_event());
    }

    /// A PCI reset of the 3dfx card, as at power-on.
    pub fn reset_voodoo(&mut self) {
        if let Some(v) = &mut self.voodoo {
            v.reset();
            self.vga.mark_dirty_full();
        }
    }

    /// The card's BAR0 or video clock changed.
    pub(crate) fn voodoo_moved(&mut self) {
        self.vga.mark_dirty_full();
    }

    /// The time, as the card needs it.
    fn voodoo_now(&self) -> Now {
        Now { ns: self.clock.now_ns(), ticks: self.clock.now_ticks() }
    }

    /// The offset of `addr` in the card's window, if it is in it. The
    /// window counts where it is above the RAM and below the BIOS ROM's
    /// mirror at the top.
    #[inline]
    pub fn voodoo_at(&self, addr: usize) -> Option<u32> {
        let base = self.voodoo.as_ref()?.base()? as usize;
        (addr >= base && addr < base + WINDOW as usize && base >= self.ram.len() && addr < 0xFFFE_0000)
            .then(|| (addr - base) as u32)
    }

    pub(crate) fn voodoo_read_16(&self, offset: u32) -> u16 {
        let Some(v) = &self.voodoo else { return 0xFFFF };
        if offset & 1 != 0 {
            return 0xFFFF;
        }
        let dword = v.read(offset & !3, self.voodoo_now());
        if offset & 2 != 0 { (dword >> 16) as u16 } else { dword as u16 }
    }

    pub(crate) fn voodoo_read_32(&self, offset: u32) -> u32 {
        let Some(v) = &self.voodoo else { return 0xFFFF_FFFF };
        let now = self.voodoo_now();
        match offset & 3 {
            0 => v.read(offset, now),
            2 => (v.read(offset & !3, now) >> 16) | v.read((offset & !3).wrapping_add(4), now) << 16,
            _ => 0xFFFF_FFFF,
        }
    }

    pub(crate) fn voodoo_write_16(&mut self, offset: u32, value: u16) {
        if offset & 1 != 0 {
            return;
        }
        if offset & 2 != 0 {
            self.voodoo_write(offset & !3, (value as u32) << 16, 0xFFFF_0000);
        } else {
            self.voodoo_write(offset, value as u32, 0x0000_FFFF);
        }
    }

    pub(crate) fn voodoo_write_32(&mut self, offset: u32, value: u32) {
        match offset & 3 {
            0 => self.voodoo_write(offset, value, 0xFFFF_FFFF),
            2 => {
                self.voodoo_write(offset & !3, value << 16, 0xFFFF_0000);
                self.voodoo_write((offset & !3).wrapping_add(4), value >> 16, 0x0000_FFFF);
            }
            // Odd: the two dwords it touches, read, merged and written.
            odd => {
                let (first, second) = (offset & !3, (offset & !3).wrapping_add(4));
                let (a, b) = (self.voodoo_read_32(first), self.voodoo_read_32(second));
                let (a, b) = if odd == 1 {
                    ((a & 0xFF_FFFF) | (value & 0xFF) << 24, (b & 0xFF00_0000) | value >> 8)
                } else {
                    ((a & 0xFF) | (value & 0xFF_FFFF) << 8, (b & 0xFFFF_FF00) | value >> 24)
                };
                self.voodoo_write(first, a, 0xFFFF_FFFF);
                self.voodoo_write(second, b, 0xFFFF_FFFF);
            }
        }
    }

    /// A byte written to the window, which the card ignores.
    pub(crate) fn voodoo_write_8(&mut self) {
        if let Some(v) = &mut self.voodoo {
            v.byte_write();
        }
        self.voodoo_log();
    }

    fn voodoo_write(&mut self, offset: u32, value: u32, mask: u32) {
        let now = self.voodoo_now();
        let Some(v) = &mut self.voodoo else { return };
        let effect = v.write(offset, value, mask, now);
        self.voodoo_effect(effect);
    }

    /// Carry out what a write asked for: the CPU waiting for a full FIFO,
    /// a swap's retrace among the timer events.
    fn voodoo_effect(&mut self, effect: Effect) {
        if let Some(to) = effect.stall_to {
            self.clock.stall_to(to);
            if let Some(v) = &mut self.voodoo {
                v.service(self.clock.now_ticks());
            }
        }
        if effect.reschedule || effect.stall_to.is_some() {
            self.clock.schedule(self.next_event());
        }
        self.voodoo_log();
    }

    fn voodoo_log(&mut self) {
        let lines = self.voodoo.as_mut().map(|v| std::mem::take(&mut v.log)).unwrap_or_default();
        for line in lines {
            self.log_string(&line);
        }
    }

    pub(crate) fn voodoo_next_event(&self) -> Option<u64> {
        self.voodoo.as_ref().and_then(|v| v.next_event())
    }

    /// Retire the swaps whose retraces came.
    pub(crate) fn voodoo_service(&mut self) {
        let now = self.clock.now_ticks();
        if let Some(v) = &mut self.voodoo {
            v.service(now);
        }
    }

    /// For the debugger's status: what the 3dfx card is doing.
    pub fn voodoo_status(&self) -> Option<serde_json::Value> {
        self.voodoo.as_ref().map(|v| v.describe(self.voodoo_now()))
    }

    /// Whether the 3dfx card shows its picture instead of the VGA's.
    pub fn voodoo_output(&self) -> bool {
        self.voodoo.as_ref().is_some_and(|v| v.output())
    }

    /// For the debugger: a dword of the card's window without the effects
    /// of a read by the CPU (which has none but the status's timing).
    pub fn voodoo_peek_32(&self, addr: usize) -> Option<u32> {
        let offset = self.voodoo_at(addr)?;
        Some(self.voodoo_read_32(offset & !3) >> (8 * (offset & 3)))
    }
}

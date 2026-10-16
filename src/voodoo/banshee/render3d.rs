//! The 3D engine's registers in the Banshee's and Voodoo3's map: the
//! Voodoo2's triangle and mode registers. On the card the buffers are
//! placed by address and stride (colBufferAddr, auxBufferAddr) instead
//! of the fbiInit registers, and swaps show leftOverlayBuf through the
//! video processor's overlay.
//!
//! Not done yet: the writes go to the Voodoo2's registers unchanged, so
//! those Banshee registers, tiled buffers and the overlay swap are
//! missing, and 3D doesn't draw where the driver expects.
//!
//! From 86Box's vid_voodoo_reg.c (Sarah Walker, GPLv2) and section 5 of
//! 3dfx's Voodoo3 specification.

use super::super::regs::*;
use super::super::{Effect, Now, Voodoo};

impl Voodoo {
    /// A 3D register read through BAR0 (200000h-5FFFFFh).
    pub(crate) fn banshee_3d_read(&self, offset: u32, now: Now) -> u32 {
        match ((offset >> 2) & 0xFF) as usize {
            STATUS => self.banshee_status(now),
            regnum => self.register_read(regnum as u32, now),
        }
    }

    /// A 3D register written through BAR0.
    pub(crate) fn banshee_3d_write(
        &mut self,
        offset: u32,
        data: u32,
        now: Now,
        effect: &mut Effect,
    ) {
        self.banshee_3d_register(offset & 0x3FFC, data, now, effect);
    }

    /// A 3D register written, from BAR0 or a command FIFO: byte address
    /// `addr`, with the chips it goes to in bits 13:10.
    pub(crate) fn banshee_3d_register(
        &mut self,
        addr: u32,
        data: u32,
        now: Now,
        effect: &mut Effect,
    ) {
        let mut chips = (addr >> 10) & 0xF;
        if chips == 0 {
            chips = 0xF;
        }
        chips &= self.chipmask;
        let regnum = ((addr >> 2) & 0xFF) as usize;
        if self.chip.access[regnum] & WRITE == 0 {
            return;
        }
        self.register_to(regnum, chips, data, now, effect);
    }
}

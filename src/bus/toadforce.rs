//! The bus's side of the ToadForce card (`toadforce`): its windows in
//! the physical address space, the time it is told with every access,
//! the screen following its mode registers, and its interrupt line.
//!
//! Card memory is the Super VGA's video memory (`vbe.vram`), with BAR 1
//! as its linear frame buffer, so reads and writes of it are the VESA
//! frame buffer's and need nothing here. While the card shows a mode of
//! its own, the Super VGA's scan-out state describes that mode, as it
//! does an S3's or a mach64's own modes, and what it held for the VGA
//! waits in the card (`Legacy`) for `MODE_ENABLE` to go back to 0.

use super::Bus;
use crate::toadforce::display::{Legacy, Shown};
use crate::toadforce::pci::Bar;
use crate::toadforce::{Change, Effects, Live, ToadForce, caps, regs};
use crate::video::VideoMode;
use crate::video::vbe::{self, VbeMode};

/// Where an access to memory the card decodes goes, but for card
/// memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Window {
    /// A byte offset in BAR 0.
    Registers(u32),
    /// A byte offset in BAR 2.
    Ring(u32),
}

/// The card's own mode as a mode of the Super VGA's, whose scan-out
/// shows it. It has no VESA number.
fn scanout(shown: &Shown) -> VbeMode {
    VbeMode {
        number: 0,
        width: shown.width as u16,
        height: shown.height as u16,
        bpp: (shown.format.bytes() * 8) as u8,
        timing: shown.timing(),
    }
}

impl Bus {
    /// Whether the display adapter is a ToadForce.
    pub fn toadforce(&self) -> bool {
        self.toadforce.is_some()
    }

    /// Whether the card shows a mode of its own, which the Super VGA's
    /// scan-out state then describes.
    pub(crate) fn toadforce_shows(&self) -> bool {
        self.toadforce
            .as_deref()
            .is_some_and(|c| c.display.shown().is_some())
    }

    /// Have the card the display adapter calls for: a ToadForce with
    /// `toadforce_memory` bytes on `Adapter::ToadForce`, none on the
    /// others. A card already there with that memory stays as it is.
    pub fn fit_toadforce(&mut self) {
        let wanted = self
            .vga
            .adapter
            .is_toadforce()
            .then_some(self.toadforce_memory);
        if self.toadforce.as_ref().map(|c| c.memory()) == wanted {
            // The card that stays keeps its configuration space, so the
            // frame buffer is where its BAR 1 is and not where the
            // adapter's BIOS data was just written to say.
            self.toadforce_moved();
            return;
        }
        self.lower_toadforce_irq();
        let live = self.toadforce_live();
        if let Some(card) = &mut self.toadforce {
            // The screen goes back to the VGA before the card goes.
            let mut fx = Effects::default();
            card.reset(live, &mut fx);
            self.toadforce_settle();
        }
        self.toadforce = wanted.map(|memory| Box::new(ToadForce::new(memory, caps::NEWEST_LEVEL)));
        self.vbe
            .resize(wanted.map_or(vbe::VRAM_SIZE, |memory| memory as usize));
        if self.toadforce() {
            self.toadforce_moved();
        }
        self.vga.mark_dirty_full();
        self.clock.schedule(self.next_event());
    }

    /// The moment, as the card needs it.
    pub(crate) fn toadforce_live(&self) -> Live {
        Live {
            ns: self.clock.now_ns(),
            vga: self.vga.peek_timing(),
        }
    }

    /// Where physical address `addr` is in the card's registers or its
    /// ring, if it is. The windows count where they are above the RAM
    /// and below the BIOS ROM's mirror at the top.
    #[inline]
    pub(crate) fn toadforce_at(&self, addr: usize) -> Option<Window> {
        let card = self.toadforce.as_deref()?;
        if addr < self.ram.len() || addr >= 0xFFFE_0000 {
            return None;
        }
        let addr = addr as u32;
        if let Some(offset) = card.pci.offset_in(Bar::Registers, addr) {
            return Some(Window::Registers(offset));
        }
        card.pci.offset_in(Bar::Ring, addr).map(Window::Ring)
    }

    /// `len` bytes (1, 2 or 4) read at `at`. A read of part of a
    /// register returns those bytes of it.
    pub(crate) fn toadforce_read(&self, at: Window, len: u32) -> u32 {
        let Some(card) = self.toadforce.as_deref() else {
            return u32::MAX;
        };
        match at {
            Window::Ring(offset) => card.ring_read(offset, len),
            Window::Registers(offset) => {
                let live = self.toadforce_live();
                // Each register the bytes are in is read once: a whole
                // word is one register, and an unaligned read two.
                let mut register = (offset & !3, card.read(offset & !3, live));
                (0..len)
                    .map(|i| {
                        let at = offset + i;
                        if register.0 != at & !3 {
                            register = (at & !3, card.read(at & !3, live));
                        }
                        (register.1 >> (8 * (at & 3)) & 0xFF) << (8 * i)
                    })
                    .sum()
            }
        }
    }

    /// `len` bytes (1, 2 or 4) of `value` written at `at`. A register
    /// takes whole, aligned words: other writes to one are ignored.
    pub(crate) fn toadforce_write(&mut self, at: Window, value: u32, len: u32) {
        match at {
            Window::Ring(offset) => {
                if let Some(card) = &mut self.toadforce {
                    card.ring_write(offset, value, len);
                }
            }
            Window::Registers(offset) if len == 4 && offset & 3 == 0 => {
                self.toadforce_register_write(offset, value);
            }
            Window::Registers(_) => {}
        }
    }

    /// A word written to the register at `offset` of BAR 0: a guest's,
    /// through the window, or one of the emulator's own drivers'.
    pub fn toadforce_register_write(&mut self, offset: u32, value: u32) {
        let live = self.toadforce_live();
        let Some(card) = &mut self.toadforce else {
            return;
        };
        let (old_error, old_level) = (card.read(regs::ERROR, live), card.level());
        let fx = card.write(offset, value, &mut self.vbe.vram, live);
        let (error, at) = (
            card.read(regs::ERROR, live),
            card.read(regs::ERROR_AT, live),
        );
        let level = card.level();
        if old_error == 0 && error != 0 {
            let line = format!(
                "[TOADFORCE] Rejected with error {} at {:08X} (register {:03X}h = {:08X}h)",
                error, at, offset, value
            );
            self.log_string(&line);
        }
        if level != old_level {
            self.log_string(&format!("[TOADFORCE] Feature level {}", level));
        }
        self.toadforce_effects(fx);
    }

    /// The register at `offset` of BAR 0 as it reads now.
    pub fn toadforce_register(&self, offset: u32) -> u32 {
        self.toadforce_read(Window::Registers(offset & !3), 4)
    }

    /// Hand the card the commands in `words`, as the emulator's own
    /// drivers (the built-in Windows') do in place of writing them into
    /// the ring: the card runs the same words the same way. A command it
    /// rejects is a bug in the driver, which a debug build stops at.
    pub fn toadforce_submit(&mut self, words: &[u32]) {
        let live = self.toadforce_live();
        let Some(card) = &mut self.toadforce else {
            return;
        };
        let clean = card.read(regs::ERROR, live) == 0;
        let fx = card.submit(words, &mut self.vbe.vram, live);
        let error = card.read(regs::ERROR, live);
        if clean && error != 0 {
            self.log_string(&format!(
                "[TOADFORCE] A built-in driver's command was rejected with error {}",
                error
            ));
            debug_assert!(
                false,
                "a built-in driver's command was rejected: error {error}"
            );
        }
        self.toadforce_effects(fx);
    }

    /// Carry out what an access to the card changed outside it.
    fn toadforce_effects(&mut self, fx: Effects) {
        for (first, end) in fx.drawn {
            if self.video_mode == VideoMode::Vesa
                && let Some((top, bottom)) =
                    self.vbe.frame_rows(first as usize, (end - first) as usize)
            {
                self.vga.mark_dirty_rows(top, bottom);
            }
        }
        match fx.display {
            Change::Mode => self.toadforce_settle(),
            Change::Picture => self.toadforce_picture(),
            Change::None => {}
        }
        self.sync_toadforce_irq();
    }

    /// The surface the card shows moved, or a colour of its palette
    /// changed: the screen shows it from the next frame.
    fn toadforce_picture(&mut self) {
        let Some(card) = self.toadforce.as_deref() else {
            return;
        };
        if card.display.shown().is_none() {
            return;
        }
        let start = card.display.start();
        if self.vbe.start != start {
            self.vbe.start = start;
            self.note_display_start();
        }
        self.vga.mark_dirty_full();
    }

    /// Make the screen what the card's display says: its own mode, as a
    /// mode of the Super VGA's with the card's timing, or the VGA's
    /// picture as it was before the card's mode took the screen.
    pub(crate) fn toadforce_settle(&mut self) {
        let Some(card) = self.toadforce.as_deref_mut() else {
            return;
        };
        let shown = card.display.shown().copied();
        let start = card.display.start();
        match shown {
            Some(shown) => {
                if card.display.legacy.is_none() {
                    let vbe = &self.vbe;
                    let vesa = (self.video_mode == VideoMode::Vesa)
                        .then_some(vbe.mode)
                        .flatten();
                    card.display.legacy = Some(Legacy {
                        video_mode: self.video_mode,
                        vesa: vesa.map(|m| (m.number, m.width, m.height)),
                        vesa_bpp: vesa.map_or(0, |m| m.bpp),
                        timing: self.vga.fixed_timing(),
                        lfb: vbe.lfb,
                        pitch: vbe.pitch,
                        start: vbe.start,
                    });
                }
                let timing = shown.timing();
                let mode = scanout(&shown);
                if self.vbe.mode != Some(mode) || self.vbe.pitch != shown.pitch {
                    let line = format!(
                        "[TOADFORCE] Display {}x{}, {} bits a pixel, {} bytes a line, {} Hz",
                        shown.width, shown.height, mode.bpp, shown.pitch, shown.refresh
                    );
                    self.log_string(&line);
                }
                self.vbe.mode = Some(mode);
                self.vbe.lfb = true;
                self.vbe.pitch = shown.pitch;
                self.vbe.start = start;
                self.vbe.latched_start = start;
                self.video_mode = VideoMode::Vesa;
                self.vga.set_fixed_timing(Some(timing));
            }
            None => {
                let Some(legacy) = card.display.legacy.take() else {
                    return;
                };
                self.vbe.mode = legacy.vesa.map(|(number, width, height)| VbeMode {
                    number,
                    width,
                    height,
                    bpp: legacy.vesa_bpp,
                    timing: legacy.timing.unwrap_or(vbe::timing_for(height)),
                });
                self.vbe.lfb = legacy.lfb;
                self.vbe.pitch = legacy.pitch;
                self.vbe.start = legacy.start;
                self.vbe.latched_start = legacy.start;
                self.video_mode = legacy.video_mode;
                self.vga.set_fixed_timing(legacy.timing);
                self.log_string(&format!(
                    "[TOADFORCE] Back to the VGA's {:?}",
                    legacy.video_mode
                ));
            }
        }
        self.note_display_start();
        self.vga.mark_dirty_full();
    }

    /// The colours of the card's own mode, while it shows one.
    pub fn toadforce_palette(&self) -> Option<&[u32; 256]> {
        let display = &self.toadforce.as_deref()?.display;
        display.shown().map(|_| display.palette())
    }

    /// A state was loaded: the linear frame buffer is where its card has
    /// BAR 1, and the card's own mode, which a state doesn't keep the
    /// Super VGA's description of, is described again.
    pub(crate) fn toadforce_after_load(&mut self) {
        self.toadforce_moved();
        let shown = self
            .toadforce
            .as_deref()
            .and_then(|c| c.display.shown().copied());
        if let Some(shown) = shown {
            self.vbe.mode = Some(scanout(&shown));
            self.vga.set_fixed_timing(Some(shown.timing()));
        }
    }

    /// The video BIOS sets a mode: it writes 0 to `MODE_ENABLE` first,
    /// so that the VGA has the screen for the mode it sets.
    pub(crate) fn toadforce_mode_set(&mut self) {
        if self.toadforce() {
            self.toadforce_register_write(regs::MODE_ENABLE, 0);
        }
    }

    /// A reset of the machine: the card as a write of 1 to `RESET`
    /// leaves it.
    pub fn reset_toadforce(&mut self) {
        if self.toadforce() {
            self.toadforce_register_write(regs::RESET, 1);
        }
    }

    /// A BAR, the card's decoding or its interrupt line changed: the
    /// linear frame buffer is where BAR 1 says, while that isn't over
    /// the RAM, and a request the card has up moves to the new line.
    pub(crate) fn toadforce_moved(&mut self) {
        let Some(card) = self.toadforce.as_deref() else {
            return;
        };
        self.vbe.lfb_base = card
            .pci
            .window(Bar::Memory)
            .map(|(base, _)| base)
            .filter(|&base| base as usize >= self.ram.len());
        self.sync_toadforce_irq();
    }

    /// Drive the card's interrupt line: raise the request when INTA#
    /// goes up, withdraw it when it drops, and have the timer wake the
    /// card for what it waits for.
    pub(crate) fn sync_toadforce_irq(&mut self) {
        let Some(card) = self.toadforce.as_deref_mut() else {
            return;
        };
        let line = card.irq();
        let old = std::mem::replace(&mut card.irq_up, line);
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

    /// Withdraw the request the card has up, before the card goes away.
    fn lower_toadforce_irq(&mut self) {
        let up = self.toadforce.as_deref_mut().and_then(|c| c.irq_up.take());
        if let Some(irq) = up {
            self.withdraw_irq(irq);
            self.refresh_irq();
        }
    }

    /// The interrupt request the card has up.
    pub(crate) fn toadforce_irq_up(&self) -> Option<u8> {
        self.toadforce.as_deref().and_then(|c| c.irq_up)
    }

    /// When the card next changes by itself, in PIT ticks: the vertical
    /// blank it raises an interrupt at or flips at.
    pub(crate) fn toadforce_next_event(&self) -> Option<u64> {
        let ns = self.toadforce.as_deref()?.next_event()?;
        // By way of the instruction the blank begins at. The clock's
        // ticks and its nanoseconds are each rounded down from the
        // instruction count at every change of speed, so they drift
        // apart, and a blank's nanoseconds counted as ticks from the
        // clock's zero would come later with every change. The tick
        // after that instruction's: by then the blank has begun. The
        // answer depends on the blank and the clock alone, not on when
        // it is asked for, which the host's timing decides.
        let at = self.clock.icount_at_ns(ns);
        Some(self.clock.ticks_at(at) + 1)
    }

    /// Bring the card up to now, at the time `toadforce_next_event`
    /// gave.
    pub(crate) fn toadforce_service(&mut self) {
        let live = self.toadforce_live();
        if let Some(card) = &mut self.toadforce {
            let fx = card.service(live);
            self.toadforce_effects(fx);
        }
    }
}

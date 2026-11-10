//! ToadForce, a PCI display adapter that never existed: rust-dos defines
//! its registers, its memory, its command ring and its capabilities, and
//! this module is the card. `docs/toadforce-spec.md` is its interface as
//! a guest sees it, which the code and its tests follow chapter by
//! chapter, and `docs/toadforce.md` says why it is shaped this way.
//!
//! The card is three windows and an interrupt line:
//!
//! - BAR 0, the registers (`regs`): identity, the feature level, the
//!   error latch, the display's mode (`display`) and the capability
//!   table (`caps`).
//! - BAR 1, card memory, which holds every surface. It is the Super
//!   VGA's video memory (`Bus::vbe`), which the VESA BIOS's modes use
//!   the start of, so the card here is handed it with each access and
//!   never keeps it.
//! - BAR 2, the command ring. A write of `RING_PUT` runs the commands
//!   the guest put there (`command`), all of them, before it returns.
//!
//! Nothing here knows the bus. `Bus`'s side (`bus/toadforce.rs`) decodes
//! the windows, tells the card the time with every access (`Live`),
//! and carries out what an access changed outside the card (`Effects`):
//! the picture on the screen and the interrupt line.
//!
//! Feature level 0 is the card as a display adapter: modes, `FILL`,
//! `BLT` and `FLIP`. The 3D levels add commands and capability entries
//! and leave this level's as they are.

pub mod blit;
pub mod caps;
pub mod command;
pub mod display;
pub mod format;
pub mod pci;
pub mod regs;

use command::Command;
pub use display::{Change, Live};
use regs::Error;

/// What an access changed outside the card, for the bus to carry out.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// What the screen has to follow.
    pub display: Change,
    /// The bytes of card memory commands wrote: each run from its first
    /// byte to after its last.
    pub drawn: Vec<(u32, u32)>,
}

/// Where the commands being run came from, for `ERROR_AT`.
#[derive(Clone, Copy)]
enum Origin {
    /// The ring, from this byte offset on.
    Ring(u32),
    /// `ToadForce::submit`.
    Direct,
}

impl Origin {
    /// What `ERROR_AT` holds for the command `word` words in.
    fn at(self, word: usize) -> u32 {
        match self {
            Origin::Ring(base) => base + 4 * word as u32,
            Origin::Direct => regs::AT_DIRECT,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToadForce {
    /// The ring's contents, as words.
    ring: Vec<u32>,
    pub pci: pci::Config,
    /// The bytes of card memory.
    memory: u32,
    level_max: u32,
    level: u32,
    irq_status: u32,
    irq_enable: u32,
    ring_put: u32,
    ring_get: u32,
    error: u32,
    error_at: u32,
    pub display: display::Display,
    /// The interrupt request the card has up, which the bus keeps.
    pub irq_up: Option<u8>,
    /// The `FILL` and `BLT` commands carried out since the card was
    /// made, for a look at what drives it. No part of the card's state.
    drawn: u64,
}

// The ring first: rewind compares states block by block from a section's
// start, so what is large and seldom changes goes before what moves.
crate::state_fields!(ToadForce {
    ring,
    pci,
    memory,
    level_max,
    level,
    irq_status,
    irq_enable,
    ring_put,
    ring_get,
    error,
    error_at,
    display,
    irq_up,
} skip { drawn });

impl ToadForce {
    /// A card with `memory` bytes of card memory (one of
    /// `regs::MEMORY_SIZES`) and the feature levels up to `level_max`, as
    /// at power-on.
    // While level 0 is the newest, the `min` below has nothing to do.
    #[allow(clippy::unnecessary_min_or_max)]
    pub fn new(memory: u32, level_max: u32) -> Self {
        debug_assert!(regs::MEMORY_SIZES.contains(&memory));
        ToadForce {
            ring: vec![0; (regs::RING_SIZE / 4) as usize],
            pci: pci::Config::new(memory),
            memory,
            level_max: level_max.min(caps::NEWEST_LEVEL),
            level: 0,
            irq_status: 0,
            irq_enable: 0,
            ring_put: 0,
            ring_get: 0,
            error: 0,
            error_at: 0,
            display: display::Display::default(),
            irq_up: None,
            drawn: 0,
        }
    }

    /// The bytes of card memory.
    pub fn memory(&self) -> u32 {
        self.memory
    }

    /// How many `FILL` and `BLT` commands the card carried out.
    pub fn drawn(&self) -> u64 {
        self.drawn
    }

    /// The highest feature level the card has.
    pub fn level_max(&self) -> u32 {
        self.level_max
    }

    /// The feature level in use.
    pub fn level(&self) -> u32 {
        self.level
    }

    /// The interrupt line the card asserts, if it asserts INTA# and the
    /// system gave it a line.
    pub fn irq(&self) -> Option<u8> {
        let line = self.pci.interrupt_line;
        (self.irq_status & self.irq_enable != 0 && line < 16).then_some(line)
    }

    /// When the card next changes by itself, in nanoseconds of the
    /// emulated clock: the vertical blank that raises an interrupt or
    /// makes a waiting flip. `service` then brings the card there.
    pub fn next_event(&self) -> Option<u64> {
        let blank_irq =
            self.irq_enable & regs::IRQ_VBLANK != 0 && self.irq_status & regs::IRQ_VBLANK == 0;
        (blank_irq || self.display.flip_pending())
            .then(|| self.display.next_blank())
            .flatten()
    }

    /// Bring the card up to `live`: the vertical blanks that began, and
    /// what they do.
    pub fn service(&mut self, live: Live) -> Effects {
        let mut fx = Effects::default();
        self.sync(live, &mut fx);
        fx
    }

    fn sync(&mut self, live: Live, fx: &mut Effects) {
        let synced = self.display.sync(live);
        self.irq_status |= irq_bits(synced);
        if synced.flipped {
            fx.display = fx.display.max(Change::Picture);
        }
    }

    /// The register at `offset` of BAR 0, a multiple of 4, as it reads at
    /// `live`. Reading changes nothing.
    pub fn read(&self, offset: u32, live: Live) -> u32 {
        match offset {
            regs::ID => regs::ID_VALUE,
            regs::LEVEL_MAX => self.level_max,
            regs::LEVEL => self.level,
            regs::STATUS => {
                let error = if self.error != 0 {
                    regs::STATUS_ERROR
                } else {
                    0
                };
                self.display.status(live) | error
            }
            regs::IRQ_STATUS => self.irq_status | irq_bits(self.display.due(live)),
            regs::IRQ_ENABLE => self.irq_enable,
            regs::RING_PUT => self.ring_put,
            regs::RING_GET => self.ring_get,
            regs::ERROR => self.error,
            regs::ERROR_AT => self.error_at,
            regs::MEMORY => self.memory,
            regs::MODE_ENABLE..=regs::SCANLINE | regs::PALETTE..regs::PALETTE_END => {
                self.display.read(offset, live)
            }
            regs::CAPS..regs::CAPS_END => {
                caps::read(self.level, ((offset - regs::CAPS) / 4) as usize)
            }
            _ => 0,
        }
    }

    /// A word written to the register at `offset` of BAR 0, a multiple of
    /// 4, at `live`. `memory` is card memory, which a write of
    /// `RING_PUT` draws into.
    pub fn write(&mut self, offset: u32, value: u32, memory: &mut [u8], live: Live) -> Effects {
        let mut fx = Effects::default();
        self.sync(live, &mut fx);
        match offset {
            // A level the card doesn't have is ignored: a driver reads
            // the register back.
            regs::LEVEL if value <= self.level_max => self.level = value,
            regs::IRQ_STATUS => self.irq_status &= !value,
            regs::IRQ_ENABLE => self.irq_enable = value & regs::IRQ_ALL,
            regs::RING_PUT => self.put(value, memory, &mut fx),
            regs::ERROR => {
                self.error = 0;
                self.error_at = 0;
            }
            regs::RESET if value & 1 != 0 => self.reset(live, &mut fx),
            regs::MODE_ENABLE..=regs::MODE_REFRESH | regs::PALETTE..regs::PALETTE_END => {
                match self.display.write(offset, value, self.memory, live) {
                    Ok(change) => fx.display = fx.display.max(change),
                    Err(error) => self.latch(error, regs::AT_REGISTER),
                }
            }
            _ => {}
        }
        fx
    }

    /// Every register at its reset value, no mode shown, no flip pending
    /// and every command state at its default. Card memory, the ring's
    /// contents and PCI configuration space keep what they hold.
    pub fn reset(&mut self, live: Live, fx: &mut Effects) {
        self.level = 0;
        self.irq_status = 0;
        self.irq_enable = 0;
        self.ring_put = 0;
        self.ring_get = 0;
        self.error = 0;
        self.error_at = 0;
        self.display.reset(live);
        fx.display = Change::Mode;
    }

    /// Latch the first rejection since `ERROR` was last written.
    fn latch(&mut self, error: Error, at: u32) {
        if self.error == 0 {
            self.error = error as u32;
            self.error_at = at;
            self.irq_status |= regs::IRQ_ERROR;
        }
    }

    /// `RING_PUT` written: run the commands from `RING_GET` up to
    /// `value`, round the ring's end if `value` is below it.
    fn put(&mut self, value: u32, memory: &mut [u8], fx: &mut Effects) {
        if !value.is_multiple_of(4) || value > regs::RING_PUT_MAX {
            self.latch(Error::BadPut, regs::AT_REGISTER);
            return;
        }
        // The commands don't change the ring, which is taken out of the
        // card while they change the rest of it.
        let ring = std::mem::take(&mut self.ring);
        let (get, put) = (self.ring_get as usize / 4, value as usize / 4);
        if put >= get {
            self.run(&ring[get..put], Origin::Ring(self.ring_get), memory, fx);
        } else if self.run(&ring[get..], Origin::Ring(self.ring_get), memory, fx) {
            self.run(&ring[..put], Origin::Ring(0), memory, fx);
        }
        self.ring = ring;
        self.ring_put = value;
        self.ring_get = value;
    }

    /// Run the commands `words` hold, as a write of `RING_PUT` runs the
    /// ring's: the way in for the emulator's own drivers (the built-in
    /// Windows'), which hand the card the words a guest's driver would
    /// have written into the ring, without the bus in between.
    pub fn submit(&mut self, words: &[u32], memory: &mut [u8], live: Live) -> Effects {
        let mut fx = Effects::default();
        self.sync(live, &mut fx);
        self.run(words, Origin::Direct, memory, &mut fx);
        fx
    }

    /// Run the commands in `words`, one after the other. False if a
    /// header's length left no next command to find, which drops the
    /// rest of what was submitted.
    fn run(&mut self, words: &[u32], origin: Origin, memory: &mut [u8], fx: &mut Effects) -> bool {
        debug_assert_eq!(memory.len(), self.memory as usize);
        let mut at = 0;
        while at < words.len() {
            let len = command::length(words[at]);
            if len == 0 || len > words.len() - at {
                self.latch(Error::BadLength, origin.at(at));
                return false;
            }
            let done = command::decode(&words[at..at + len], self.level, self.memory)
                .and_then(|c| self.execute(c, memory, fx));
            if let Err(error) = done {
                self.latch(error, origin.at(at));
            }
            at += len;
        }
        true
    }

    /// Carry out a decoded command.
    fn execute(
        &mut self,
        command: Command,
        memory: &mut [u8],
        fx: &mut Effects,
    ) -> Result<(), Error> {
        match command {
            Command::Nop => {}
            Command::Fill { dst, rect, value } => {
                blit::fill(memory, &dst, &rect, value);
                self.drawn += 1;
                fx.drawn.push(dst.span(&rect));
            }
            Command::Blt {
                dst,
                dst_rect,
                src,
                src_rect,
                key,
            } => {
                blit::blt(memory, &dst, &dst_rect, &src, &src_rect, key);
                self.drawn += 1;
                fx.drawn.push(dst.span(&dst_rect));
            }
            Command::Flip { offset, wait } => {
                if self.display.flip(offset, wait, self.memory)? {
                    self.irq_status |= regs::IRQ_FLIP;
                    fx.display = fx.display.max(Change::Picture);
                }
            }
        }
        Ok(())
    }

    /// `len` bytes (1 to 4) of the ring at byte `offset`, as the guest
    /// reads back what it wrote. Bytes past the ring's end read as 0.
    pub fn ring_read(&self, offset: u32, len: u32) -> u32 {
        (0..len)
            .map(|i| {
                let at = offset + i;
                let word = self.ring.get((at / 4) as usize).copied().unwrap_or(0);
                (word >> (8 * (at & 3)) & 0xFF) << (8 * i)
            })
            .sum()
    }

    /// The low `len` bytes (1 to 4) of `value` written to the ring at
    /// byte `offset`.
    pub fn ring_write(&mut self, offset: u32, value: u32, len: u32) {
        for i in 0..len {
            let at = offset + i;
            if let Some(word) = self.ring.get_mut((at / 4) as usize) {
                let shift = 8 * (at & 3);
                *word = *word & !(0xFF << shift) | (value >> (8 * i) & 0xFF) << shift;
            }
        }
    }
}

/// The bits of `IRQ_STATUS` what a `sync` found sets.
fn irq_bits(synced: display::Synced) -> u32 {
    let blank = if synced.blank { regs::IRQ_VBLANK } else { 0 };
    let flip = if synced.flipped { regs::IRQ_FLIP } else { 0 };
    blank | flip
}

#[cfg(test)]
mod tests;

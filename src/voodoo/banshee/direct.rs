//! A program's own reads and writes of a Banshee's or Voodoo3's memory
//! through BAR1 below the tile base, where the BAR is the memory in
//! order: the pixels of a locked DirectDraw surface, which a software
//! renderer reads and writes a byte, word or dword at a time, millions
//! of them a second.
//!
//! Such an access is the bytes and what the card does around them
//! (`Voodoo::banshee_read`, `banshee_linear_write`): the 3D drawing
//! queued before is done first, what only the OpenGL renderer has there
//! is read back, a write marks the picture changed and tells the
//! recording. Two shorter ways do the same without the way through the
//! window's registers:
//!
//! * `banshee_linear_load` and `banshee_linear_store` do all of it for
//!   an access of 1 to 4 bytes, for the bus (`Bus::read_banshee_linear`).
//! * `banshee_direct` says where nothing is left to do but the bytes, so
//!   that translated code reads and writes them itself (`dynrec`'s
//!   `card_state`), and what it must leave to the bus. While the OpenGL
//!   renderer records, that is page by page: a page whose access through
//!   the card left nothing behind it lets the next ones skip the card
//!   (`Tracker::grant`) until its state changes.

use super::super::Voodoo;
use super::super::tracker::{DIRECT_WRITE, PAGE};
use super::{BAR_SIZE, blit2d};

/// How many ranges of the memory translated code leaves to the bus when
/// it writes: the desktop shown and the two command FIFOs' rings.
pub const KEPT: usize = 3;

/// The part of the card's memory that a program's reads and writes need
/// nothing done for but the bytes (`Voodoo::banshee_direct`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Direct {
    /// The memory's first byte on the host.
    pub base: *mut u8,
    /// The bytes from BAR1's first that are the memory in order: those
    /// below the tile base that the memory has.
    pub len: u32,
    /// Byte ranges of them (start, end) whose writes do more than store
    /// the bytes: the desktop shown, whose writes count as the program's
    /// output for `cycles=auto` (`Bus::note_banshee_memory_write`), and
    /// the command FIFOs' rings, whose dwords are commands. Reads of
    /// them are only the bytes.
    pub kept: [(u32, u32); KEPT],
    /// None if every page is only its bytes. Otherwise only those that
    /// the card says are (`Voodoo::banshee_grant`), with the count that
    /// changes when a page stops being so (`Tracker::direct_generation`):
    /// a page that isn't may become so after an access through the card.
    pub grants: Option<u64>,
}

impl Voodoo {
    /// Whether nothing is behind the card's memory for a program's read
    /// or write of it to wait for or read back: no drawing queued for
    /// the workers, no rows or pages newer on the GPU, and none hot,
    /// which a read would keep hot (`Tracker::touch`).
    fn banshee_memory_quiet(&self) -> bool {
        if self.gpu_behind.get() {
            return false;
        }
        let tracker = &self.gpu.borrow().tracker;
        !tracker.any_gpu() && !tracker.any_in_buffers() && !tracker.any_hot()
    }

    /// `len` bytes (1 to 4) of the card's memory from byte `addr` on, as
    /// reads through BAR1 below the tile base give them (`banshee_read`
    /// of the dwords they are in): the drawing queued before is done
    /// first, and the dwords read are brought back from the GPU.
    pub(crate) fn banshee_linear_load(&self, addr: u32, len: u32) -> u32 {
        debug_assert!((1..=4).contains(&len));
        if self.workers_queue() {
            self.catch_up_workers();
        }
        let mask = self.fbi.mask;
        let (first, last) = (addr & !3, addr.wrapping_add(len - 1) & !3);
        if !self.banshee_memory_quiet() {
            self.gpu_sync(first & mask, (first & mask) + 4);
            if last != first {
                self.gpu_sync(last & mask, (last & mask) + 4);
            }
        }
        let value = blit2d::read(&self.fbi.ram, addr, len, mask);
        // The next reads of the page may need nothing but the bytes.
        let tracker = &mut self.gpu.borrow_mut().tracker;
        for at in [first & mask, last & mask] {
            if tracker.direct_at(at) == 0 {
                tracker.grant(at, false);
            }
        }
        value
    }

    /// `len` bytes (1 to 4) of `value` written into the card's memory
    /// from byte `addr` on, as writes through BAR1 below the tile base
    /// and outside the command FIFOs' rings put them
    /// (`banshee_linear_write` of the dwords they are in, with their
    /// byte enables): after the drawing queued before, with the picture
    /// marked changed and the recording told of each dword's bytes.
    pub(crate) fn banshee_linear_store(&mut self, addr: u32, value: u32, len: u32) {
        debug_assert!((1..=4).contains(&len));
        if self.workers_queue() {
            self.catch_up_workers();
        }
        let mask = self.fbi.mask;
        let quiet = self.banshee_memory_quiet();
        let (mut at, mut left, mut value) = (addr, len, value);
        // A dword's bytes at a time, as the card takes them.
        while left > 0 {
            let n = (4 - (at & 3)).min(left);
            let a = at & mask;
            if !quiet {
                self.gpu_sync(a & !3, (a & !3) + 4);
            }
            blit2d::write(&self.fbi.ram, at, n, value, mask);
            self.display_dirty = true;
            self.mirror_memory(a, a + n);
            self.banshee_grant_write(a);
            at = at.wrapping_add(n);
            left -= n;
            value = value.checked_shr(8 * n).unwrap_or(0);
        }
    }

    /// A program wrote byte `at` of the memory through BAR1, with all
    /// that goes with it done: its next writes of the page may be only
    /// the bytes if the recording has no pixel of its 3D buffers there
    /// (`mirror_memory`), as far as the tracker goes (`Tracker::grant`).
    fn banshee_grant_write(&mut self, at: u32) {
        let tracker = &mut self.gpu.get_mut().tracker;
        if tracker.direct_at(at) & DIRECT_WRITE != 0 {
            return;
        }
        let page = at & !(PAGE - 1);
        let layout = self.mirror.get_mut().as_ref().and_then(|m| m.layout());
        let free = layout.is_none_or(|key| !key.overlaps(page, page + PAGE));
        tracker.grant(at, free);
    }

    /// Where a program's reads and writes of the card's memory through
    /// BAR1 are only the bytes, while nothing else changes the card:
    /// nothing is queued for the workers, and no swap waits for its turn
    /// under the frame rate cap (a write behind it waits for it). Every
    /// page is if the OpenGL renderer has nothing newer and isn't
    /// recording (a write tells the recording), and no bytes are hot;
    /// otherwise those the tracker says are (`Direct::grants`). A write
    /// still marks the picture changed: `banshee_wrote_directly`, once
    /// the writes are done and before anything looks.
    pub fn banshee_direct(&self) -> Option<Direct> {
        let b = self.banshee.as_deref()?;
        if self.workers_queue() || self.deferred.is_some() {
            return None;
        }
        let grants = (self.recording() || !self.banshee_memory_quiet())
            .then(|| self.gpu.borrow().tracker.direct_generation());
        let bytes = self.fbi.ram.bytes().min(self.fbi.mask as usize + 1) as u64;
        let len = bytes.min(b.tile_base() as u64).min(BAR_SIZE as u64) as u32;
        let mut kept = [(0, 0); KEPT];
        if b.extended() {
            let (start, span) = b.desktop_memory();
            let end = (start as u64 + span).min(len as u64) as u32;
            if start < end {
                kept[0] = (start, end);
            }
        }
        for (keep, f) in kept[1..].iter_mut().zip(&b.fifo) {
            if f.enabled && f.base < f.end.min(len) {
                *keep = (f.base, f.end.min(len));
            }
        }
        Some(Direct {
            base: self.fbi.ram.as_mut_ptr(),
            len,
            kept,
            grants,
        })
    }

    /// For `Direct::grants`: what a program's reads and writes of the page
    /// with byte `at` of the memory may do by themselves
    /// (`tracker::DIRECT_READ`, `DIRECT_WRITE`), and the count of changes.
    pub fn banshee_grant(&self, at: u32) -> (u8, u64) {
        let tracker = &self.gpu.borrow().tracker;
        (tracker.direct_at(at), tracker.direct_generation())
    }

    /// `banshee_grant` for every page from the first, which `f` gets.
    pub fn banshee_grants<R>(&self, f: impl FnOnce(&mut dyn Iterator<Item = u8>) -> R) -> R {
        f(&mut self.gpu.borrow().tracker.direct())
    }

    /// The texture units hear of the pages the program may have written
    /// by itself (`Tracker::take_unstamped`): before the recording decodes
    /// textures from the memory.
    pub(crate) fn banshee_stamp_direct_writes(&mut self) {
        for (start, end) in self.gpu.get_mut().tracker.take_unstamped() {
            for tmu in &mut self.tmu {
                tmu.note_written(start, end);
            }
        }
    }

    /// Translated code wrote the card's memory by itself
    /// (`banshee_direct`): the picture changed.
    pub fn banshee_wrote_directly(&mut self) {
        self.display_dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::super::{LFB_MEMORY_CONFIG, LINEAR, MEMORY};
    use super::*;
    use crate::voodoo::{Board, Now};

    const NOW: Now = Now { ns: 0, ticks: 0 };

    fn card() -> Voodoo {
        let mut v = Voodoo::new(Board::Voodoo3);
        // The tile base as high as it goes: BAR1 below it is the memory in
        // order, and wraps at the memory's end.
        v.write(LFB_MEMORY_CONFIG, 0x1FFF, u32::MAX, NOW);
        v
    }

    /// `len` bytes at BAR1 offset `at` through the window's dwords, as
    /// `Bus::banshee_read_bytes` reads them.
    fn window_read(v: &Voodoo, at: u32, len: u32) -> u32 {
        let offset = LINEAR + at;
        let shift = 8 * (offset & 3);
        let mut value = (v.read(offset & !3, NOW) as u64) >> shift;
        if (offset & 3) + len > 4 {
            value |= (v.read((offset & !3) + 4, NOW) as u64) << (32 - shift);
        }
        (value & ((1u64 << (8 * len)) - 1)) as u32
    }

    /// The same written, as `Bus::banshee_write_bytes` writes them.
    fn window_write(v: &mut Voodoo, at: u32, value: u32, len: u32) {
        let offset = LINEAR + at;
        let shift = 8 * (offset & 3);
        let (data, mask) = ((value as u64) << shift, ((1u64 << (8 * len)) - 1) << shift);
        v.write(offset & !3, data as u32, mask as u32, NOW);
        if mask >> 32 != 0 {
            v.write(
                (offset & !3) + 4,
                (data >> 32) as u32,
                (mask >> 32) as u32,
                NOW,
            );
        }
    }

    #[test]
    fn loads_and_stores_of_any_size_and_alignment_match_the_window() {
        let (mut a, mut b) = (card(), card());
        let mut seed = 0x1234_5678u32;
        let mut next = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            seed
        };
        // Across the memory's end too, where the addresses wrap.
        for base in [0x1000u32, 0x2_3FF8, MEMORY as u32 - 8] {
            for at in base..base + 8 {
                for len in 1..=4 {
                    let value = next();
                    window_write(&mut a, at, value, len);
                    b.banshee_linear_store(at, value, len);
                    for (r, rl) in (base.saturating_sub(4)..base + 12).zip((1..=4).cycle()) {
                        assert_eq!(
                            b.banshee_linear_load(r, rl),
                            window_read(&a, r, rl),
                            "{rl} bytes at {r:#x} after {len} at {at:#x}"
                        );
                    }
                }
            }
        }
        assert_eq!(a.frame_buffer().to_bytes(), b.frame_buffer().to_bytes());
    }

    #[test]
    fn a_store_marks_the_picture_changed() {
        let mut v = card();
        v.display_dirty = false;
        v.banshee_linear_store(0x4001, 0xAB, 1);
        assert!(v.display_dirty);
        v.display_dirty = false;
        v.banshee_wrote_directly();
        assert!(v.display_dirty);
    }

    #[test]
    fn direct_memory_is_what_is_below_the_tile_base() {
        let mut v = card();
        let d = v.banshee_direct().expect("a quiet card");
        assert_eq!(d.len, MEMORY as u32);
        assert_eq!(d.base, v.fbi.ram.as_mut_ptr());
        v.write(LFB_MEMORY_CONFIG, 0x100, u32::MAX, NOW);
        assert_eq!(v.banshee_direct().map(|d| d.len), Some(0x10_0000));
        // The bytes the host has there are the memory's.
        v.banshee_linear_store(0x1234, 0xCAFE_F00D, 4);
        let host = unsafe { std::slice::from_raw_parts(d.base.add(0x1234), 4) };
        assert_eq!(host, 0xCAFE_F00Du32.to_le_bytes());
    }

    #[test]
    fn a_held_swap_keeps_the_memory_from_being_direct() {
        let mut v = card();
        v.gpu.get_mut().tracker.mark(0, 4096);
        let grants = |v: &Voodoo| v.banshee_direct().map(|d| d.grants.is_some());
        assert_eq!(grants(&v), Some(false), "dirty pages are the card's own");
        // A write tells the recording, and rows newer in the renderer are
        // read back first: the card says which pages need neither.
        v.set_mirror(true);
        assert_eq!(grants(&v), Some(true), "a write tells the recording");
        v.set_mirror(false);
        assert_eq!(grants(&v), Some(false));
        v.gpu_behind.set(true);
        assert_eq!(grants(&v), Some(true), "rows are newer in the renderer");
        v.gpu_behind.set(false);
        v.deferred = Some((0, 1));
        assert!(v.banshee_direct().is_none(), "a write waits for the swap");
    }

    #[test]
    fn while_recording_a_page_goes_direct_after_an_access_through_the_card() {
        use crate::voodoo::tracker::DIRECT_READ;
        let mut v = card();
        v.set_mirror(true);
        let grants = v
            .banshee_direct()
            .and_then(|d| d.grants)
            .expect("page by page");
        assert_eq!(v.banshee_grant(0x5000), (0, grants));
        // A read: the page's reads need nothing else from now on.
        v.banshee_linear_load(0x5004, 4);
        assert_eq!(v.banshee_grant(0x5000), (DIRECT_READ, grants));
        // A write marks the page, after which its writes need nothing
        // else either.
        v.banshee_linear_store(0x5FFE, 0x1234, 2);
        assert_eq!(v.banshee_grant(0x5000).0, DIRECT_READ | DIRECT_WRITE);
        assert_eq!(v.banshee_grant(0x6000).0, 0);
        // Before a texture is decoded the texture units hear of the page,
        // whose writes go through the card again: the next one is heard
        // of too.
        let heard = v.tmu[0].page_writes[5];
        v.banshee_stamp_direct_writes();
        assert_ne!(v.tmu[0].page_writes[5], heard);
        assert_eq!(v.banshee_grant(0x5000).0, DIRECT_READ);
        assert_ne!(v.banshee_direct().and_then(|d| d.grants), Some(grants));
        let mut pages = Vec::new();
        v.banshee_grants(|g| pages.extend(g.take(7)));
        assert_eq!(pages, [0, 0, 0, 0, 0, DIRECT_READ, 0]);
    }
}

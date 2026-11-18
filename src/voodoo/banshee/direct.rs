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
//!   `card_state`), and what it must leave to the bus.

use super::super::Voodoo;
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
        if !self.banshee_memory_quiet() {
            let (first, last) = (addr & !3, addr.wrapping_add(len - 1) & !3);
            self.gpu_sync(first & mask, (first & mask) + 4);
            if last != first {
                self.gpu_sync(last & mask, (last & mask) + 4);
            }
        }
        blit2d::read(&self.fbi.ram, addr, len, mask)
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
            at = at.wrapping_add(n);
            left -= n;
            value = value.checked_shr(8 * n).unwrap_or(0);
        }
    }

    /// Where a program's reads and writes of the card's memory through
    /// BAR1 are only the bytes, while nothing else changes the card:
    /// nothing is queued for the workers, no swap waits for its turn
    /// under the frame rate cap (a write behind it waits for it), the
    /// OpenGL renderer has nothing newer and isn't recording (a write
    /// tells the recording), and no bytes are hot. A write still marks
    /// the picture changed: `banshee_wrote_directly`, once the writes
    /// are done and before anything looks.
    pub fn banshee_direct(&self) -> Option<Direct> {
        let b = self.banshee.as_deref()?;
        if self.workers_queue()
            || self.deferred.is_some()
            || self.recording()
            || !self.banshee_memory_quiet()
        {
            return None;
        }
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
        })
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
    fn a_recording_rows_behind_and_a_held_swap_keep_the_memory_from_being_direct() {
        let mut v = card();
        v.gpu.get_mut().tracker.mark(0, 4096);
        assert!(
            v.banshee_direct().is_some(),
            "dirty pages are the card's own"
        );
        v.set_mirror(true);
        assert!(v.banshee_direct().is_none(), "a write tells the recording");
        v.set_mirror(false);
        assert!(v.banshee_direct().is_some());
        v.gpu_behind.set(true);
        assert!(
            v.banshee_direct().is_none(),
            "rows are newer in the renderer"
        );
        v.gpu_behind.set(false);
        v.deferred = Some((0, 1));
        assert!(v.banshee_direct().is_none(), "a write waits for the swap");
    }
}

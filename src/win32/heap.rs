//! Heaps (HeapCreate, HeapAlloc and their family): blocks of ranges of
//! the process's address space, given out first fit, the pages behind
//! them committed as the heap grows, in the page file until they are
//! touched (`Memory::commit_paged`). A heap made with a maximum size has
//! one range of that size; a growable one, as the process heap is, takes
//! another range when its ranges are full. The blocks' sizes are kept
//! here, not in the heap's memory, so a program that writes past a
//! block's end spoils its neighbour's contents only, as with Windows 95's
//! heaps.

use super::memory::{
    GRANULARITY, MEM_PRIVATE, Memory, PAGE, PAGE_READWRITE, PRIVATE, SHARED, SYSTEM,
};
use crate::bus::Bus;

/// Blocks start at multiples of this, as on Windows.
const ALIGN: u32 = 8;
/// The least a growable heap reserves at a time.
const GROWTH: u32 = 0x10_0000;
/// How many block lengths a heap keeps a first-fit hint for.
const HINTS: usize = 16;

/// The W32 section's version that added the processes' system heaps: a
/// state saved by an older rust-dos has none.
pub const SYSTEM_HEAP_SAVED: u16 = 79;

/// A process's system heap, once it has one (`Heap::create_shared`).
pub type SystemHeap = crate::savestate::Since<SYSTEM_HEAP_SAVED, Option<Heap>>;

/// A range of address space a heap has its blocks in.
#[derive(Clone, Debug, Default)]
pub struct Segment {
    pub base: u32,
    pub reserved: u32,
    /// How much of the range, from its start, has pages behind it.
    committed: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Heap {
    /// Its handle: the address of its first range.
    pub base: u32,
    segments: Vec<Segment>,
    /// The blocks given out, as (address, size), in address order.
    blocks: Vec<(u32, u32)>,
    /// Whether it takes more ranges when it is full.
    pub growable: bool,
    /// The flags it was made with that its calls go by
    /// (HEAP_NO_SERIALIZE, HEAP_GENERATE_EXCEPTIONS).
    pub flags: u32,
    /// Where first fit may start looking, for the lengths a block was
    /// lately asked for: (len, i) says no block before `blocks[i]` has a
    /// free gap of `len` bytes or more in front of it, the gap from the
    /// end of the block before it in its range, or from the range's
    /// start. Gaps at the ends of ranges aren't covered: first fit looks
    /// at each range's last gap whatever the hint. Without these, a
    /// program that keeps thousands of small blocks and allocates and
    /// frees a few more every frame has every allocation walk past all
    /// of them. Not saved: a heap loaded from a state starts without.
    hints: Vec<(u32, usize)>,
}

/// The bytes a block of `size` takes, if that is a size at all.
fn extent(size: u32) -> Option<u32> {
    Some(size.max(1).checked_add(ALIGN - 1)? & !(ALIGN - 1))
}

impl Heap {
    /// A heap in the reserved range of `reserved` bytes at `base`.
    pub fn new(base: u32, reserved: u32, growable: bool) -> Heap {
        Heap {
            base,
            segments: vec![Segment {
                base,
                reserved,
                committed: 0,
            }],
            growable,
            ..Heap::default()
        }
    }

    /// Make a heap: its first range reserved for `max` bytes, or for a
    /// growable one (`max` 0) as much as `initial` and at least 1 MB, and
    /// `initial` bytes of it committed. None when there is no room.
    pub fn create(memory: &mut Memory, bus: &mut Bus, initial: u32, max: u32) -> Option<Heap> {
        let growable = max == 0;
        let len = if growable {
            initial.max(GROWTH)
        } else {
            max.max(initial)
        };
        let len = len.checked_add(PAGE - 1)? & !(PAGE - 1);
        let base =
            memory.reserve_region(None, len, PRIVATE, SHARED, PAGE_READWRITE, MEM_PRIVATE)?;
        let mut heap = Heap::new(base, len, growable);
        let initial = initial.min(len).next_multiple_of(PAGE);
        if initial > 0 {
            if memory
                .commit_paged(bus, base, initial, PAGE_READWRITE)
                .is_none()
            {
                heap.destroy(memory, bus);
                return None;
            }
            heap.segments[0].committed = initial;
        }
        Some(heap)
    }

    /// A heap of `max` bytes in the shared arena that doesn't grow, its
    /// pages committed as its blocks need them: the system's own, for what
    /// Windows 98 keeps in KERNEL32's heap there, such as the objects of
    /// critical sections (`Process::system_heap`). None when there is no
    /// room.
    pub fn create_shared(memory: &mut Memory, max: u32) -> Option<Heap> {
        let len = max.checked_add(PAGE - 1)? & !(PAGE - 1);
        let base = memory.reserve_region(None, len, SHARED, SYSTEM, PAGE_READWRITE, MEM_PRIVATE)?;
        Some(Heap::new(base, len, false))
    }

    /// Give its ranges and their pages back.
    pub fn destroy(&mut self, memory: &mut Memory, bus: &mut Bus) {
        for segment in self.segments.drain(..) {
            for page in (segment.base..segment.base + segment.committed).step_by(PAGE as usize) {
                memory.unmap(bus, page);
            }
            memory.unreserve(segment.base);
        }
        self.blocks.clear();
        self.hints.clear();
    }

    /// The index in `blocks` before which no block of a range has a gap
    /// of `len` bytes or more in front of it.
    fn hint(&self, len: u32) -> usize {
        self.hints
            .iter()
            .find(|&&(l, _)| l == len)
            .map_or(0, |&(_, i)| i)
    }

    /// Where in the segment `s` a block of `len` bytes fits first, and the
    /// place in `blocks` it goes in.
    fn fit(&self, s: usize, len: u32) -> Option<(u32, usize)> {
        let segment = &self.segments[s];
        let end = segment.base as u64 + segment.reserved as u64;
        let first = self.blocks.partition_point(|&(b, _)| b < segment.base);
        let last = first + self.blocks[first..].partition_point(|&(b, _)| (b as u64) < end);
        // Blocks before the hint have smaller gaps in front of them, so
        // the search starts after the block before it (the gaps to its
        // range's end are still looked at).
        let mut index = self.hint(len).clamp(first, last);
        let mut at = match index {
            i if i > first => {
                let (start, size) = self.blocks[i - 1];
                start + extent(size)?
            }
            _ => segment.base,
        };
        while let Some(&(start, size)) = self.blocks.get(index) {
            if start as u64 >= end || start - at >= len {
                break;
            }
            at = start + extent(size)?;
            index += 1;
        }
        (at as u64 + len as u64 <= end).then_some((at, index))
    }

    /// Commit the segment `s` up to `end`. False when there is no memory.
    fn commit_to(&mut self, s: usize, memory: &mut Memory, bus: &mut Bus, end: u32) -> bool {
        let segment = &mut self.segments[s];
        let needed = (end - segment.base).next_multiple_of(PAGE);
        if needed > segment.committed {
            let from = segment.base + segment.committed;
            if memory
                .commit_paged(bus, from, needed - segment.committed, PAGE_READWRITE)
                .is_none()
            {
                return false;
            }
            segment.committed = needed;
        }
        true
    }

    /// A block of `size` bytes, or None when the heap is full or there is
    /// no memory for its pages. Pages committed for it are zeroed; a block
    /// in the place of one freed has what that one had.
    pub fn alloc(&mut self, memory: &mut Memory, bus: &mut Bus, size: u32) -> Option<u32> {
        let len = extent(size)?;
        let found = (0..self.segments.len()).find_map(|s| Some((s, self.fit(s, len)?)));
        let (s, (at, index)) = match found {
            Some(found) => found,
            None if self.growable => {
                let reserve = len.checked_add(GRANULARITY - 1)? & !(GRANULARITY - 1);
                let reserve = reserve.max(GROWTH);
                let base = memory.reserve_region(
                    None,
                    reserve,
                    PRIVATE,
                    SHARED,
                    PAGE_READWRITE,
                    MEM_PRIVATE,
                )?;
                self.segments.push(Segment {
                    base,
                    reserved: reserve,
                    committed: 0,
                });
                let s = self.segments.len() - 1;
                (s, self.fit(s, len)?)
            }
            None => return None,
        };
        if !self.commit_to(s, memory, bus, at + len) {
            return None;
        }
        self.blocks.insert(index, (at, size));
        // The new block splits the gap it went into, so the gaps before
        // every hint after it are still too small, one block further on.
        for hint in &mut self.hints {
            if hint.1 > index {
                hint.1 += 1;
            }
        }
        // It starts where that gap did, with nothing in front of it, and
        // the search found every gap before it smaller than `len`, unless
        // a range it didn't get to (one after `s`) lies lower.
        let base = self.segments[s].base;
        if self.segments[s + 1..].iter().all(|g| g.base > base) {
            self.set_hint(len, index + 1);
        }
        Some(at)
    }

    /// Remember that no gap before `blocks[index]` holds `len` bytes.
    fn set_hint(&mut self, len: u32, index: usize) {
        match self.hints.iter_mut().find(|h| h.0 == len) {
            Some(hint) => hint.1 = index,
            None => {
                if self.hints.len() == HINTS {
                    self.hints.remove(0);
                }
                self.hints.push((len, index));
            }
        }
    }

    /// The gap in front of `blocks[index]` may have grown: the hints
    /// after it can't vouch for it any more.
    fn widened(&mut self, index: usize) {
        for hint in &mut self.hints {
            hint.1 = hint.1.min(index);
        }
    }

    fn find(&self, at: u32) -> Option<usize> {
        self.blocks
            .binary_search_by_key(&at, |&(start, _)| start)
            .ok()
    }

    /// Whether `at` is the start of one of its blocks.
    pub fn has(&self, at: u32) -> bool {
        self.find(at).is_some()
    }

    /// Give back the block at `at`. False if there is none there.
    pub fn free(&mut self, at: u32) -> bool {
        match self.find(at) {
            Some(i) => {
                // The block after it now has the freed bytes in front of it.
                self.blocks.remove(i);
                self.widened(i);
                true
            }
            None => false,
        }
    }

    /// The size of the block at `at`.
    pub fn size(&self, at: u32) -> Option<u32> {
        self.find(at).map(|i| self.blocks[i].1)
    }

    /// Change the size of the block at `at` to `size` where it is, if the
    /// next block leaves room. False if it doesn't, or there is no block.
    pub fn resize_in_place(
        &mut self,
        memory: &mut Memory,
        bus: &mut Bus,
        at: u32,
        size: u32,
    ) -> bool {
        let Some(i) = self.find(at) else {
            return false;
        };
        let Some(s) = self
            .segments
            .iter()
            .position(|s| (s.base..s.base + s.reserved).contains(&at))
        else {
            return false;
        };
        let segment_end = self.segments[s].base + self.segments[s].reserved;
        let limit = self
            .blocks
            .get(i + 1)
            .map_or(segment_end, |&(next, _)| next.min(segment_end));
        let Some(end) = extent(size).and_then(|len| at.checked_add(len)) else {
            return false;
        };
        if end > limit || !self.commit_to(s, memory, bus, end) {
            return false;
        }
        self.blocks[i].1 = size;
        // A block that shrank leaves a larger gap before the next one.
        self.widened(i + 1);
        true
    }

    /// The size of the largest free piece of its ranges (HeapCompact).
    pub fn largest_free(&self) -> u32 {
        (0..self.segments.len())
            .map(|s| {
                let segment = &self.segments[s];
                let end = segment.base + segment.reserved;
                let mut at = segment.base;
                let mut largest = 0;
                let first = self.blocks.partition_point(|&(b, _)| b < segment.base);
                for &(start, size) in self.blocks[first..].iter().take_while(|&&(b, _)| b < end) {
                    largest = largest.max(start - at);
                    at = start + extent(size).unwrap_or(0);
                }
                largest.max(end.saturating_sub(at))
            })
            .max()
            .unwrap_or(0)
    }
}

/// A moveable block of GlobalAlloc's or LocalAlloc's: its handle, the
/// address of a pointer to the block (in a block of the process heap of
/// its own), as Windows' are, which programs read the block's address
/// from.
#[derive(Clone, Debug, Default)]
pub struct Moveable {
    pub handle: u32,
    /// The block in the process heap, 0 when discarded (or of size 0).
    pub block: u32,
    /// GMEM_DISCARDABLE, if it was made with it.
    pub flags: u32,
    /// How many times it is locked.
    pub locks: u32,
}

crate::state_fields!(Moveable {
    handle,
    block,
    flags,
    locks
});
crate::state_fields!(Segment {
    base,
    reserved,
    committed
});
crate::state_fields!(Heap {
    base,
    segments,
    blocks,
    growable,
    flags
} skip {
    hints
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::Cpu;

    /// A heap of ranges at `bases`, in that order, each of `len` bytes and
    /// all committed, so its calls never need pages.
    fn heap(bases: &[u32], len: u32) -> Heap {
        Heap {
            base: bases[0],
            segments: bases
                .iter()
                .map(|&base| Segment {
                    base,
                    reserved: len,
                    committed: len,
                })
                .collect(),
            ..Heap::default()
        }
    }

    /// What first fit gives for `size` without the hints.
    fn plain_alloc(heap: &Heap, cpu: &mut Cpu, size: u32) -> Option<u32> {
        let mut plain = heap.clone();
        plain.hints.clear();
        plain.alloc(&mut Memory::default(), &mut cpu.bus, size)
    }

    /// Allocations, frees and resizes picked by a fixed generator give
    /// the blocks first fit without the hints gives, in ranges that lie
    /// in and out of the order they were made in: with a few lengths, and
    /// with more lengths than the heap keeps hints for.
    #[test]
    fn hints_keep_first_fit() {
        for seed in [0x1234_5678, 0x9E37_79B9, 0x0BAD_F00D] {
            first_fit_matches(seed, |r| [1, 8, 13, 24, 48, 48, 100, 300][r as usize % 8]);
            first_fit_matches(seed, |r| 1 + r % 400);
        }
    }

    /// 20,000 operations from the generator seeded with `seed`, the sizes
    /// allocated picked by `size` from a random number.
    fn first_fit_matches(mut seed: u32, size: fn(u32) -> u32) {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let mut memory = Memory::default();
        let mut heap = heap(&[0x20_0000, 0x10_0000, 0x30_0000], 0x2000);
        let mut next = move || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            seed >> 8
        };
        let mut live = Vec::new();
        for _ in 0..20_000 {
            let roll = next();
            match roll % 8 {
                0..=3 => {
                    let size = size(next());
                    let expected = plain_alloc(&heap, &mut cpu, size);
                    let got = heap.alloc(&mut memory, &mut cpu.bus, size);
                    assert_eq!(got, expected, "alloc of {size}");
                    live.extend(got);
                }
                4..=6 if !live.is_empty() => {
                    let at = live.swap_remove(next() as usize % live.len());
                    assert!(heap.free(at));
                }
                7 if !live.is_empty() => {
                    let at = live[next() as usize % live.len()];
                    let size = 1 + next() % 120;
                    heap.resize_in_place(&mut memory, &mut cpu.bus, at, size);
                }
                _ => {}
            }
        }
    }

    /// Once blocks fill the start of a heap, first fit for a length it
    /// was just asked for starts after them.
    #[test]
    fn search_starts_after_full_blocks() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let mut memory = Memory::default();
        let mut heap = heap(&[0x10_0000], 0x10_0000);
        for _ in 0..1000 {
            heap.alloc(&mut memory, &mut cpu.bus, 16).unwrap();
        }
        let at = heap.alloc(&mut memory, &mut cpu.bus, 48).unwrap();
        assert_eq!(heap.hint(48), 1001);
        assert!(heap.free(at));
        assert_eq!(heap.hint(48), 1000);
        assert_eq!(heap.fit(0, 48), Some((0x10_0000 + 16_000, 1000)));
    }
}

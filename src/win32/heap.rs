//! Heaps (HeapCreate, HeapAlloc and their family): blocks of ranges of
//! the process's address space, given out first fit, the pages behind
//! them committed as the heap grows. A heap made with a maximum size has
//! one range of that size; a growable one, as the process heap is, takes
//! another range when its ranges are full. The blocks' sizes are kept
//! here, not in the heap's memory, so a program that writes past a
//! block's end spoils its neighbour's contents only, as with Windows 95's
//! heaps.

use super::memory::{GRANULARITY, MEM_PRIVATE, Memory, PAGE, PAGE_READWRITE, PRIVATE, SHARED};
use super::memory::{USER, WRITABLE};
use crate::bus::Bus;

/// Blocks start at multiples of this, as on Windows.
const ALIGN: u32 = 8;
/// The least a growable heap reserves at a time.
const GROWTH: u32 = 0x10_0000;

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
            if memory.commit(bus, base, initial, USER | WRITABLE).is_none() {
                heap.destroy(memory, bus);
                return None;
            }
            heap.segments[0].committed = initial;
        }
        Some(heap)
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
    }

    /// Where in the segment `s` a block of `len` bytes fits first, and the
    /// place in `blocks` it goes in.
    fn fit(&self, s: usize, len: u32) -> Option<(u32, usize)> {
        let segment = &self.segments[s];
        let end = segment.base as u64 + segment.reserved as u64;
        let mut index = self.blocks.partition_point(|&(b, _)| b < segment.base);
        let mut at = segment.base;
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
                .commit(bus, from, needed - segment.committed, USER | WRITABLE)
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
        Some(at)
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
                self.blocks.remove(i);
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
});

//! Heaps (HeapAlloc and its family): blocks of a range of the process's
//! address space, given out first fit, the pages behind them committed as
//! the heap grows. The blocks' sizes are kept here, not in the heap's
//! memory, so a program that writes past a block's end spoils its
//! neighbour's contents only, as with Windows 95's heaps.

use super::memory::{Memory, PAGE, USER, WRITABLE};
use crate::bus::Bus;

/// Blocks start at multiples of this, as on Windows.
const ALIGN: u32 = 8;

#[derive(Clone, Debug, Default)]
pub struct Heap {
    /// Its address range, which is also its handle.
    pub base: u32,
    pub reserved: u32,
    /// How much of the range, from its start, has pages behind it.
    committed: u32,
    /// The blocks given out, as (address, size), in address order.
    blocks: Vec<(u32, u32)>,
}

impl Heap {
    pub fn new(base: u32, reserved: u32) -> Heap {
        Heap {
            base,
            reserved,
            ..Heap::default()
        }
    }

    /// A block of `size` bytes, or None when the range is full or there is
    /// no memory for its pages. Pages committed for it are zeroed; a block
    /// in the place of one freed has what that one had.
    pub fn alloc(&mut self, memory: &mut Memory, bus: &mut Bus, size: u32) -> Option<u32> {
        let len = size.max(1).checked_add(ALIGN - 1)? & !(ALIGN - 1);
        let mut at = self.base;
        let mut index = 0;
        for &(start, size) in &self.blocks {
            if start - at >= len {
                break;
            }
            at = (start + size).next_multiple_of(ALIGN);
            index += 1;
        }
        let end = at.checked_add(len)?;
        if end > self.base + self.reserved {
            return None;
        }
        let needed = (end - self.base).next_multiple_of(PAGE);
        if needed > self.committed {
            let from = self.base + self.committed;
            memory.commit(bus, from, needed - self.committed, USER | WRITABLE)?;
            self.committed = needed;
        }
        self.blocks.insert(index, (at, size));
        Some(at)
    }

    fn find(&self, at: u32) -> Option<usize> {
        self.blocks
            .binary_search_by_key(&at, |&(start, _)| start)
            .ok()
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
        let limit = self
            .blocks
            .get(i + 1)
            .map_or(self.base + self.reserved, |&(next, _)| next);
        let Some(end) = at.checked_add(size.max(1)) else {
            return false;
        };
        if end > limit {
            return false;
        }
        let needed = (end - self.base).next_multiple_of(PAGE);
        if needed > self.committed {
            let from = self.base + self.committed;
            if memory
                .commit(bus, from, needed - self.committed, USER | WRITABLE)
                .is_none()
            {
                return false;
            }
            self.committed = needed;
        }
        self.blocks[i].1 = size;
        true
    }
}

crate::state_fields!(Heap {
    base,
    reserved,
    committed,
    blocks
});

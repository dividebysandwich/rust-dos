//! A log of the machine's port accesses, for debuggers (`/api/ports`):
//! the last `capacity` reads and writes, with the instruction count they
//! were made at to line them up with a trace.

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug)]
pub struct PortAccess {
    pub icount: u64,
    pub port: u16,
    pub value: u32,
    /// Bytes: 1, 2 or 4 (the graphics engine's ports take words whole).
    pub len: u8,
    pub write: bool,
}

pub struct PortLog {
    entries: VecDeque<PortAccess>,
    capacity: usize,
    /// The ports kept, from and to.
    pub ports: (u16, u16),
}

impl PortLog {
    pub fn new(capacity: usize) -> Self {
        Self { entries: VecDeque::with_capacity(capacity.min(1 << 16)), capacity: capacity.max(1), ports: (0, 0xFFFF) }
    }

    pub fn push(&mut self, access: PortAccess) {
        if !(self.ports.0..=self.ports.1).contains(&access.port) {
            return;
        }
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(access);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The accesses, oldest first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &PortAccess> {
        self.entries.iter()
    }
}

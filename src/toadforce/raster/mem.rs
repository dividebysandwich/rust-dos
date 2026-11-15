//! Card memory as the renderer's threads use it.
//!
//! A batch of draws runs on several threads at once. Each writes the
//! rows of the render target it owns and reads textures anywhere, so no
//! thread can hold the memory as a slice. `Mem` is the memory's address
//! and length, with reads and writes of a few bytes that check their
//! range. The renderer keeps the threads' bytes apart (`DrawState`'s
//! `serial`): a draw whose textures or buffers overlap what it writes
//! runs on one thread.

use std::marker::PhantomData;

/// Card memory for as long as the borrow it was made from.
#[derive(Clone, Copy)]
pub struct Mem<'a> {
    ptr: *mut u8,
    len: usize,
    borrow: PhantomData<&'a mut [u8]>,
}

// The bytes one thread writes no other reads or writes (see the module
// comment), and the borrow keeps everything else from the memory.
unsafe impl Send for Mem<'_> {}
unsafe impl Sync for Mem<'_> {}

impl<'a> Mem<'a> {
    pub fn new(memory: &'a mut [u8]) -> Self {
        Mem {
            ptr: memory.as_mut_ptr(),
            len: memory.len(),
            borrow: PhantomData,
        }
    }

    /// The `N` bytes at `at`, or zeroes when they aren't all in the
    /// memory.
    #[inline]
    pub fn read<const N: usize>(&self, at: usize) -> [u8; N] {
        match at.checked_add(N) {
            // In range, and u8 has no alignment.
            Some(end) if end <= self.len => unsafe { self.ptr.add(at).cast::<[u8; N]>().read() },
            _ => [0; N],
        }
    }

    /// The `N` bytes at each of `at`, or None when any of them aren't
    /// all in the memory: one check for the lot.
    #[inline]
    pub fn read_all<const N: usize, const K: usize>(&self, at: [usize; K]) -> Option<[[u8; N]; K]> {
        let last = at.into_iter().max()?;
        if last.checked_add(N)? > self.len {
            return None;
        }
        // Each is at most `last`, which is in range, and u8 has no
        // alignment.
        Some(at.map(|at| unsafe { self.ptr.add(at).cast::<[u8; N]>().read() }))
    }

    /// Put `bytes` at `at`. Nothing is written when they don't all fit.
    #[inline]
    pub fn write<const N: usize>(&self, at: usize, bytes: [u8; N]) {
        if at.checked_add(N).is_some_and(|end| end <= self.len) {
            // In range, and u8 has no alignment.
            unsafe { self.ptr.add(at).cast::<[u8; N]>().write(bytes) }
        }
    }

    #[inline]
    pub fn u8(&self, at: usize) -> u32 {
        self.read::<1>(at)[0] as u32
    }

    #[inline]
    pub fn u16(&self, at: usize) -> u32 {
        u16::from_le_bytes(self.read(at)) as u32
    }

    #[inline]
    pub fn u32(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.read(at))
    }

    /// The pixel of `bytes` bytes (1, 2 or 4) at `at`.
    #[inline]
    pub fn pixel(&self, at: usize, bytes: u32) -> u32 {
        match bytes {
            1 => self.u8(at),
            2 => self.u16(at),
            _ => self.u32(at),
        }
    }

    /// Write the low `bytes` bytes (2 or 4) of `value` at `at`.
    #[inline]
    pub fn set_pixel(&self, at: usize, bytes: u32, value: u32) {
        match bytes {
            2 => self.write(at, (value as u16).to_le_bytes()),
            _ => self.write(at, value.to_le_bytes()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_stay_inside_the_memory() {
        let mut bytes = vec![1u8, 2, 3, 4, 5, 6];
        let mem = Mem::new(&mut bytes);
        assert_eq!(mem.u32(0), 0x0403_0201);
        assert_eq!(mem.u16(4), 0x0605);
        assert_eq!(mem.u8(5), 6);
        // Past the end, in part or whole, and at an offset that wraps.
        assert_eq!(mem.u32(3), 0);
        assert_eq!(mem.u8(6), 0);
        assert_eq!(mem.u32(usize::MAX - 1), 0);
        assert_eq!(mem.read_all::<2, 2>([0, 4]), Some([[1, 2], [5, 6]]));
        assert_eq!(mem.read_all::<2, 2>([0, 5]), None);
        assert_eq!(mem.read_all::<2, 2>([usize::MAX, 0]), None);
        mem.set_pixel(4, 4, 0xFFFF_FFFF);
        mem.write(usize::MAX, [9u8; 2]);
        mem.set_pixel(2, 2, 0xAABB);
        assert_eq!(mem.pixel(2, 2), 0xAABB);
        assert_eq!(bytes, [1, 2, 0xBB, 0xAA, 5, 6]);
    }
}

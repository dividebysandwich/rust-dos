//! The card's memory: the frame buffer and the texture units' RAM, as
//! 16-bit words that the render workers and the emulator share. Relaxed
//! atomic loads and stores are plain moves on x86 and ARM; being atomic,
//! the memory stays sound to read (the display, a save state, the
//! debugger) while workers draw into parts of it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering::Relaxed};

#[derive(Clone)]
pub struct Vram {
    words: Arc<[AtomicU16]>,
}

impl Vram {
    /// `bytes` of memory, cleared.
    pub fn new(bytes: usize) -> Self {
        Self { words: (0..bytes / 2).map(|_| AtomicU16::new(0)).collect() }
    }

    /// Size in bytes.
    pub fn bytes(&self) -> usize {
        self.words.len() * 2
    }

    /// Size in words.
    #[inline]
    pub fn len(&self) -> usize {
        self.words.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// The word at index `i`.
    #[inline]
    pub fn get(&self, i: usize) -> u16 {
        self.words[i].load(Relaxed)
    }

    /// The word at index `i`, which is within the memory.
    ///
    /// # Safety
    ///
    /// `i < self.len()`.
    #[inline]
    pub unsafe fn get_unchecked(&self, i: usize) -> u16 {
        debug_assert!(i < self.words.len());
        // SAFETY: the caller keeps `i` within the words.
        unsafe { self.words.get_unchecked(i).load(Relaxed) }
    }

    #[inline]
    pub fn set(&self, i: usize, value: u16) {
        self.words[i].store(value, Relaxed)
    }

    /// The byte at byte address `a` (little-endian words).
    #[inline]
    pub fn byte(&self, a: usize) -> u8 {
        (self.get(a >> 1) >> ((a & 1) * 8)) as u8
    }

    pub fn set_byte(&self, a: usize, value: u8) {
        let word = &self.words[a >> 1];
        let shift = (a & 1) * 8;
        let old = word.load(Relaxed);
        word.store((old & !(0xFF << shift)) | (value as u16) << shift, Relaxed);
    }

    /// The bytes from byte address `a` on, into `out`: a word at a time
    /// where they cover one whole. They must be within the memory.
    pub fn read_bytes(&self, a: usize, out: &mut [u8]) {
        let (a, out) = match out.split_first_mut() {
            Some((first, rest)) if a & 1 != 0 => {
                *first = self.byte(a);
                (a + 1, rest)
            }
            _ => (a, out),
        };
        let (pairs, last) = out.as_chunks_mut::<2>();
        let words = &self.words[a >> 1..][..pairs.len()];
        for (pair, w) in pairs.iter_mut().zip(words) {
            *pair = w.load(Relaxed).to_le_bytes();
        }
        if let [b] = last {
            *b = self.byte(a + 2 * pairs.len());
        }
    }

    /// `data` written from byte address `a` on, a word at a time where
    /// it covers one whole. It must be within the memory.
    pub fn write_bytes(&self, a: usize, data: &[u8]) {
        let (a, data) = match data.split_first() {
            Some((&first, rest)) if a & 1 != 0 => {
                self.set_byte(a, first);
                (a + 1, rest)
            }
            _ => (a, data),
        };
        let (pairs, last) = data.as_chunks::<2>();
        let words = &self.words[a >> 1..][..pairs.len()];
        for (pair, w) in pairs.iter().zip(words) {
            w.store(u16::from_le_bytes(*pair), Relaxed);
        }
        if let [b] = last {
            self.set_byte(a + 2 * pairs.len(), *b);
        }
    }

    /// Fill words `range` with `value`.
    pub fn fill(&self, range: std::ops::Range<usize>, value: u16) {
        for w in &self.words[range] {
            w.store(value, Relaxed);
        }
    }

    /// A copy of it, other memory with the same words.
    ///
    /// # Safety
    ///
    /// No other thread writes this memory while it is copied (other
    /// threads may read it): the words are copied as plain memory.
    pub unsafe fn copy(&self) -> Self {
        let mut words = Arc::<[AtomicU16]>::new_uninit_slice(self.words.len());
        let to = Arc::get_mut(&mut words).expect("a new Arc is unique");
        // SAFETY: AtomicU16 has u16's layout, the two slices are the same
        // length and different memory, and the caller keeps writers out,
        // so the plain reads race with no write.
        let words = unsafe {
            std::ptr::copy_nonoverlapping(
                self.words.as_ptr(),
                to.as_mut_ptr().cast::<AtomicU16>(),
                self.words.len(),
            );
            words.assume_init()
        };
        Self { words }
    }

    /// Words `range` of `from` copied into this.
    ///
    /// # Safety
    ///
    /// Nothing else has this memory, and no other thread writes `from`
    /// while it is copied: the words are copied as plain memory.
    pub unsafe fn copy_words(&self, from: &Vram, range: std::ops::Range<usize>) {
        let (to, from) = (&self.words[range.clone()], &from.words[range]);
        // SAFETY: the two slices are the same length and different memory
        // (this one is the caller's alone), and the caller keeps writers
        // out of `from`.
        unsafe { std::ptr::copy_nonoverlapping(from.as_ptr(), to.as_ptr().cast_mut(), to.len()) };
    }

    /// Whether this is the only handle on the memory: whatever the others
    /// did with it happened before.
    pub fn unique(&mut self) -> bool {
        Arc::get_mut(&mut self.words).is_some()
    }

    /// Whether this and `other` are the same memory.
    pub fn same(&self, other: &Vram) -> bool {
        Arc::ptr_eq(&self.words, &other.words)
    }

    /// All of it as little-endian bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.bytes());
        for w in self.words.iter() {
            bytes.extend_from_slice(&w.load(Relaxed).to_le_bytes());
        }
        bytes
    }

    /// Put `bytes` (little-endian words, as long as the memory) in it.
    pub fn load_bytes(&self, bytes: &[u8]) {
        for (w, b) in self.words.iter().zip(bytes.as_chunks::<2>().0) {
            w.store(u16::from_le_bytes(*b), Relaxed);
        }
    }
}

impl std::fmt::Debug for Vram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Vram({} KB)", self.bytes() / 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_and_words_share_the_memory() {
        let m = Vram::new(8);
        m.set(1, 0x1234);
        assert_eq!((m.byte(2), m.byte(3)), (0x34, 0x12));
        m.set_byte(3, 0xAB);
        assert_eq!(m.get(1), 0xAB34);
        let copy = m.to_bytes();
        let other = Vram::new(8);
        other.load_bytes(&copy);
        assert_eq!(other.get(1), 0xAB34);
        assert!(!m.same(&other) && m.same(&m.clone()));
        // SAFETY: nothing else has the memory.
        let copied = unsafe { m.copy() };
        assert!(!copied.same(&m) && copied.to_bytes() == m.to_bytes());
        copied.set(1, 0);
        assert_eq!(m.get(1), 0xAB34);
        let mut copied = copied;
        assert!(copied.unique());
        // SAFETY: nothing else has either memory.
        unsafe { copied.copy_words(&m, 1..2) };
        assert_eq!(copied.get(1), 0xAB34);
        let other = copied.clone();
        assert!(!copied.unique());
        drop(other);
        assert!(copied.unique());
    }

    /// Runs of bytes read and written a word at a time are the bytes
    /// one at a time, from odd and even addresses, of odd and even
    /// lengths.
    #[test]
    fn runs_of_bytes_are_the_bytes() {
        let m = Vram::new(16);
        for at in 0..4 {
            for len in 0..8 {
                let data: Vec<u8> = (0..len as u8).map(|i| 0x40 + i).collect();
                m.fill(0..8, 0xA5A5);
                m.write_bytes(at, &data);
                let mut want = vec![0xA5u8; 16];
                want[at..at + len].copy_from_slice(&data);
                assert_eq!(m.to_bytes(), want, "at {at}, {len} bytes");
                let mut out = vec![0; len];
                m.read_bytes(at, &mut out);
                assert_eq!(out, data, "at {at}, {len} bytes");
            }
        }
    }
}

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

    /// `data` written from byte address `at` on, which with it is within
    /// the memory: a byte at each end that is half a word, the words
    /// between whole.
    pub fn store_bytes(&self, at: usize, data: &[u8]) {
        let (mut at, mut data) = (at, data);
        if at & 1 != 0
            && let Some((&first, rest)) = data.split_first()
        {
            self.set_byte(at, first);
            (at, data) = (at + 1, rest);
        }
        let (pairs, tail) = data.as_chunks::<2>();
        let words = &self.words[at / 2..at / 2 + pairs.len()];
        for (w, b) in words.iter().zip(pairs) {
            w.store(u16::from_le_bytes(*b), Relaxed);
        }
        if let [last] = tail {
            self.set_byte(at + 2 * pairs.len(), *last);
        }
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

    #[test]
    fn stored_bytes_leave_the_halves_of_words_around_them() {
        let m = Vram::new(8);
        m.load_bytes(&[0xEE; 8]);
        m.store_bytes(1, &[1, 2, 3, 4, 5]);
        assert_eq!(m.to_bytes(), [0xEE, 1, 2, 3, 4, 5, 0xEE, 0xEE]);
        m.store_bytes(2, &[6, 7]);
        assert_eq!(m.to_bytes(), [0xEE, 1, 6, 7, 4, 5, 0xEE, 0xEE]);
        m.store_bytes(7, &[8]);
        m.store_bytes(0, &[]);
        assert_eq!(m.to_bytes(), [0xEE, 1, 6, 7, 4, 5, 0xEE, 8]);
    }
}

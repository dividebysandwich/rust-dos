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
        for (w, b) in self.words.iter().zip(bytes.chunks_exact(2)) {
            w.store(u16::from_le_bytes([b[0], b[1]]), Relaxed);
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
    }
}

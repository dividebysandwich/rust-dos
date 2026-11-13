//! LSB-first bit reader for the IV50 bitstream (spec 00, "Byte and bit
//! order").
//!
//! Bytes are taken in file order and bit 0 of a byte comes first. A
//! field of n bits has its first bit in bit 0 of the value. The reader
//! keeps an absolute bit position over a copy of the frame that is
//! padded with zero bytes, so a peek near the end never runs off the
//! buffer; reading past the real end is reported by [`Bits::overrun`].

/// Zero bytes appended to the frame so that an 8-byte load at any byte
/// position inside the frame stays in the buffer.
const PAD: usize = 16;

/// The bit reader.
pub(crate) struct Bits<'a> {
    data: &'a [u8],
    /// Length of the frame without the padding, in bits.
    len_bits: usize,
    /// Absolute bit position of the next bit.
    pos: usize,
}

/// A frame copied into a zero-padded buffer for [`Bits`].
pub(crate) struct PaddedFrame {
    buf: Vec<u8>,
    len: usize,
}

impl PaddedFrame {
    /// Copy `data` and append the zero padding.
    pub fn new(data: &[u8]) -> Self {
        let mut buf = Vec::with_capacity(data.len() + PAD);
        buf.extend_from_slice(data);
        buf.resize(data.len() + PAD, 0);
        PaddedFrame {
            buf,
            len: data.len(),
        }
    }

    /// A reader positioned at byte `byte`.
    pub fn reader_at(&self, byte: usize) -> Bits<'_> {
        Bits {
            data: &self.buf,
            len_bits: self.len * 8,
            pos: byte.min(self.len) * 8,
        }
    }

    /// Frame length in bytes, padding excluded.
    pub fn len(&self) -> usize {
        self.len
    }
}

impl Bits<'_> {
    /// The next 32 bits without consuming them (bits past the end of the
    /// frame read as zero).
    #[inline(always)]
    pub fn peek32(&self) -> u32 {
        let byte = self.pos >> 3;
        if byte + 8 > self.data.len() {
            return 0;
        }
        let w = u64::from_le_bytes(self.data[byte..byte + 8].try_into().unwrap());
        (w >> (self.pos & 7)) as u32
    }

    /// Skip `n` bits.
    #[inline(always)]
    pub fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    /// Read an `n`-bit field, n from 0 to 32.
    #[inline(always)]
    pub fn read(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let v = self.peek32();
        self.pos += n as usize;
        if n == 32 { v } else { v & ((1u32 << n) - 1) }
    }

    /// Read one bit.
    #[inline(always)]
    pub fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    /// Skip to the next byte boundary (no change on a boundary).
    #[inline]
    pub fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    /// Absolute bit position.
    #[cfg(test)]
    pub fn bit_pos(&self) -> usize {
        self.pos
    }

    /// Byte that holds the next bit.
    #[inline]
    pub fn byte_pos(&self) -> usize {
        self.pos >> 3
    }

    /// Whether the reader has gone past the end of the frame.
    #[inline]
    pub fn overrun(&self) -> bool {
        self.pos > self.len_bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_lsb_first() {
        let f = PaddedFrame::new(&[0b1010_0101, 0x34, 0x12]);
        let mut r = f.reader_at(0);
        assert_eq!(r.read(1), 1);
        assert_eq!(r.read(2), 0b10);
        assert_eq!(r.read(5), 0b10100);
        assert_eq!(r.read(16), 0x1234);
        assert!(!r.overrun());
        r.read(1);
        assert!(r.overrun());
    }

    #[test]
    fn align_skips_to_byte_boundary() {
        let f = PaddedFrame::new(&[0xff, 0x0f]);
        let mut r = f.reader_at(0);
        r.align();
        assert_eq!(r.bit_pos(), 0);
        r.read(3);
        r.align();
        assert_eq!(r.read(8), 0x0f);
    }
}

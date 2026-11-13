//! The row-width prefix codes of the macroblock and block layers
//! (spec 03, "Huffman codes"; spec 04, 1.2).
//!
//! A descriptor is a list of row widths. Row i is a prefix of i one-bits
//! and a zero-bit (the last row has no zero-bit), followed by w[i] bits
//! that form a number most significant bit first. The symbol is the
//! number of symbols in the rows before plus that number.
//!
//! The decoder peeks 13 bits (the longest code allowed) and looks the
//! symbol and code length up in a table of 8192 entries.

use super::bits::Bits;

/// Longest code the builder accepts (spec 03, "Huffman codes").
const MAX_LEN: u32 = 13;

/// The MB-layer presets (spec 02, 2.3).
pub(crate) const MB_PRESETS: [&[u8]; 8] = [
    &[0, 4, 5, 4, 4, 4, 6, 6],
    &[0, 2, 2, 3, 3, 3, 3, 5, 3, 2, 2, 2],
    &[0, 2, 3, 4, 3, 3, 3, 3, 4, 3, 2, 2],
    &[0, 3, 4, 4, 3, 3, 3, 3, 3, 2, 2, 2],
    &[0, 4, 4, 3, 3, 3, 3, 2, 3, 3, 2, 1, 1],
    &[0, 4, 4, 4, 4, 3, 3, 3, 2],
    &[0, 4, 4, 4, 4, 3, 3, 2, 2, 2],
    &[0, 4, 4, 4, 3, 3, 2, 3, 2, 2, 2, 2],
];

/// The block-layer presets (spec 02, 3.2; spec 04, 1.3).
pub(crate) const BLOCK_PRESETS: [&[u8]; 8] = [
    &[1, 2, 3, 4, 4, 7, 5, 5, 4, 1],
    &[2, 3, 4, 4, 4, 7, 5, 4, 3, 3, 2],
    &[2, 4, 5, 5, 5, 5, 6, 4, 4, 3, 1, 1],
    &[3, 3, 4, 4, 5, 6, 6, 4, 4, 3, 2, 1, 1],
    &[3, 4, 4, 5, 5, 5, 6, 5, 4, 2, 2],
    &[3, 4, 5, 5, 5, 5, 6, 4, 3, 3, 2, 1, 1],
    &[3, 4, 5, 5, 5, 6, 5, 4, 3, 3, 2, 1, 1],
    &[3, 4, 4, 5, 5, 5, 6, 5, 5],
];

/// A decoding table. Each entry holds the symbol in bits 0..7 and the
/// code length in bits 8..12; length 0 marks a bit pattern that no
/// symbol below 256 owns.
#[derive(Clone)]
pub(crate) struct Huff {
    table: Box<[u16; 1 << MAX_LEN]>,
    rows: Vec<u8>,
}

impl Huff {
    /// Build the table for a descriptor. Returns `None` when a row is
    /// wider than 8 bits or a code is longer than 13 bits, which the DLL
    /// rejects (spec 03, "Huffman codes").
    pub fn new(rows: &[u8]) -> Option<Huff> {
        let n = rows.len();
        if n == 0 || n > 16 {
            return None;
        }
        let mut table = Box::new([0u16; 1 << MAX_LEN]);
        let mut base: u32 = 0;
        for (i, &w) in rows.iter().enumerate() {
            let w = w as u32;
            let last = i == n - 1;
            let plen = if last { i as u32 } else { i as u32 + 1 };
            let len = plen + w;
            if w > 8 || len > MAX_LEN {
                return None;
            }
            // Prefix bits in read order: i ones, then a zero unless last.
            let prefix: u32 = (1u32 << i) - 1;
            for x in 0..(1u32 << w) {
                let sym = base + x;
                if sym >= 256 {
                    break;
                }
                // The extra bits are read most significant first, so the
                // first bit read (bit plen of the peek) is bit w-1 of x.
                let mut pat = prefix;
                for j in 0..w {
                    pat |= ((x >> (w - 1 - j)) & 1) << (plen + j);
                }
                let entry = (sym as u16) | ((len as u16) << 8);
                let step = 1u32 << len;
                let mut idx = pat;
                while idx < (1 << MAX_LEN) {
                    table[idx as usize] = entry;
                    idx += step;
                }
            }
            base += 1 << w;
        }
        Some(Huff {
            table,
            rows: rows.to_vec(),
        })
    }

    /// The descriptor this table was built from.
    pub fn rows(&self) -> &[u8] {
        &self.rows
    }

    /// Decode one symbol, or `None` for a pattern that maps to no
    /// symbol below 256.
    #[inline(always)]
    pub fn decode(&self, r: &mut Bits) -> Option<u32> {
        let e = self.table[(r.peek32() & ((1 << MAX_LEN) - 1)) as usize];
        let len = (e >> 8) as u32;
        if len == 0 {
            return None;
        }
        r.skip(len);
        Some((e & 0xff) as u32)
    }
}

/// Read a descriptor carried in the stream: a 4-bit row count and that
/// many 4-bit row widths (spec 02, 2.3).
pub(crate) fn read_descriptor(r: &mut Bits) -> Vec<u8> {
    let n = r.read(4);
    (0..n).map(|_| r.read(4) as u8).collect()
}

/// The signed value of a macroblock-layer symbol (spec 03, section 3):
/// odd symbols are positive, even ones negative, and 255 is -128.
#[inline]
pub(crate) fn signed(s: u32) -> i32 {
    if s & 1 == 1 {
        let v = (s as i32 + 1) / 2;
        if v == 128 { -128 } else { v }
    } else {
        -((s / 2) as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::super::bits::PaddedFrame;
    use super::*;

    /// Write bits LSB first, the way the decoder reads them.
    fn pack(bits: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; bits.len().div_ceil(8) + 1];
        for (i, &b) in bits.iter().enumerate() {
            out[i / 8] |= b << (i % 8);
        }
        out
    }

    #[test]
    fn rows_give_prefix_then_msb_first_extra_bits() {
        let h = Huff::new(&[1, 2, 3]).unwrap();
        // Row 0: "0" + 1 bit: symbols 0, 1.
        // Row 1: "10" + 2 bits: symbols 2..5.
        // Row 2 (last): "11" + 3 bits: symbols 6..13.
        let stream = pack(&[0, 1, 1, 0, 1, 0, 1, 1, 1, 1, 0]);
        let f = PaddedFrame::new(&stream);
        let mut r = f.reader_at(0);
        assert_eq!(h.decode(&mut r), Some(1));
        assert_eq!(h.decode(&mut r), Some(2 + 2));
        assert_eq!(h.decode(&mut r), Some(6 + 6));
    }

    #[test]
    fn presets_cover_256_symbols() {
        // Preset 7 has room for 264 symbols; the codes of symbols 256 and
        // above are not entered (see impl-questions.md).
        for rows in BLOCK_PRESETS {
            let total: u32 = rows.iter().map(|&w| 1u32 << w).sum();
            assert!(total >= 256);
            assert!(Huff::new(rows).is_some());
        }
        for rows in MB_PRESETS {
            assert!(Huff::new(rows).is_some());
        }
    }

    #[test]
    fn invalid_descriptors_are_refused() {
        assert!(Huff::new(&[9]).is_none());
        assert!(Huff::new(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]).is_none());
    }

    #[test]
    fn signed_map() {
        assert_eq!(signed(0), 0);
        assert_eq!(signed(1), 1);
        assert_eq!(signed(2), -1);
        assert_eq!(signed(253), 127);
        assert_eq!(signed(254), -127);
        assert_eq!(signed(255), -128);
    }
}

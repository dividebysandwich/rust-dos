//! Indeo 5 LSB-first bit reader.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/00-scope.md` §3 and
//! `spec/01-file-header.md` §3.1.
//!
//! Indeo 5's picture / GOP / frame / band headers are a bit-packed
//! stream read **LSB-first within each byte**, with subsequent bytes
//! contributing the next-higher bit positions (`spec/00 §3`). The
//! binary implements this with a 32-bit accumulator window seeded by a
//! whole-DWORD prefetch (`spec/01 §3.1`):
//!
//! ```text
//! eax = [bitstream_ptr]   ; read first DWORD (4 bytes, little-endian)
//! bitstream_ptr += 4      ; advance past the initial DWORD
//! bit_offset = 0x20       ; 32 bits prefetched into the accumulator
//! accumulator = eax       ; 32-bit window over the bitstream
//! ```
//!
//! Each `getbits(n)` extracts the low `n` bits of the accumulator (via
//! the `(1 << n) - 1` mask from the `.rdata` mask table at
//! `IR50_32.DLL!.rdata 0x1008d680`, indexed by bit-width — `spec/02
//! §0`), then shifts the accumulator right by `n`, refilling the top
//! end one byte at a time as the running bit position crosses byte
//! boundaries (`spec/01 §3.1` refill at
//! `IR50_32.DLL!0x10023393`-`0x100233a3`):
//!
//! ```text
//! dl = [bitstream_ptr]        ; load next byte
//! accumulator >>= 8           ; make room at the top
//! accumulator |= (dl << 24)   ; place new byte at the top
//! bitstream_ptr++
//! bit_offset += 8
//! ```
//!
//! This module models that accumulator precisely so the header parsers
//! consume the same bits, in the same order, the binary does. It is a
//! pure reader over a byte slice — no codec state, no allocation.

/// Spec/02 §0 — the largest bit-width a single `read` extracts in the
/// header parsers. `pic_hdr_size` / `band_data_size` / `transp_color`
/// are 24-bit reads; the custom-dimension pair is a 26-bit read split
/// into two 13-bit halves (`spec/02 §1.6`). 26 is therefore the widest
/// single read; we keep the accumulator a full 32 bits so any
/// in-spec read fits without a second refill mid-extract.
pub const MAX_READ_BITS: u32 = 26;

/// Errors the LSB-first reader can raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BitReaderError {
    /// The stream did not even carry the initial 4-byte DWORD prefetch
    /// (`spec/01 §3.1`); an Indeo 5 frame is always at least the
    /// 16-bit picture-start triplet, so fewer than 4 bytes of payload
    /// is malformed.
    TooShortForPrefetch {
        /// Number of payload bytes actually available.
        available: usize,
    },
    /// A `read(n)` requested more than [`MAX_READ_BITS`] bits in one
    /// call. The header grammar never does this; a request past the
    /// bound is a caller bug, surfaced rather than silently truncated.
    ReadTooWide {
        /// Requested bit-width.
        requested: u32,
    },
    /// The reader ran past the end of the input slice while refilling
    /// the accumulator. The byte stream ended before the parser
    /// finished consuming the fields it expected.
    UnexpectedEof {
        /// Total bits consumed so far at the point of the fault.
        bits_consumed: u64,
    },
}

impl core::fmt::Display for BitReaderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BitReaderError::TooShortForPrefetch { available } => write!(
                f,
                "indeo5 bitreader: stream of {available} bytes too short for the 4-byte DWORD prefetch (spec/01 §3.1)"
            ),
            BitReaderError::ReadTooWide { requested } => write!(
                f,
                "indeo5 bitreader: read({requested}) exceeds the {MAX_READ_BITS}-bit single-read bound (spec/02 §0)"
            ),
            BitReaderError::UnexpectedEof { bits_consumed } => write!(
                f,
                "indeo5 bitreader: ran past end of input after {bits_consumed} bits (spec/01 §3.1 refill)"
            ),
        }
    }
}

impl std::error::Error for BitReaderError {}

/// An LSB-first bit reader over an Indeo 5 header byte slice.
///
/// Construct with [`BitReader::new`], which performs the `spec/01
/// §3.1` initial DWORD prefetch, then pull fields with [`read`],
/// [`read_bit`], [`align`], and [`skip`].
///
/// [`read`]: BitReader::read
/// [`read_bit`]: BitReader::read_bit
/// [`align`]: BitReader::align
/// [`skip`]: BitReader::skip
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitReader<'a> {
    /// The full payload slice.
    data: &'a [u8],
    /// Index of the next byte the refill will pull from `data`. After
    /// the initial DWORD prefetch this is `4` (or `data.len()` if the
    /// stream was exactly 4 bytes).
    next_byte: usize,
    /// The accumulator window; valid bits occupy the low
    /// [`Self::live_bits`] positions and are extracted from the low
    /// end. Refills place new bytes just above the live region.
    accumulator: u64,
    /// Number of valid (refilled, not-yet-consumed) bits currently in
    /// [`Self::accumulator`], occupying its low end. Starts at 32 (the
    /// DWORD prefetch) and is kept `>= n` before each `read(n)` by
    /// refilling one byte at a time.
    live_bits: u32,
    /// Total bits consumed from the front of the stream so far (the
    /// bit position of the next unread bit). Mirrors the parser's
    /// running consumption used for byte-alignment and the
    /// "bytes consumed" out-parameter (`spec/02 §2.8`).
    consumed: u64,
}

impl<'a> BitReader<'a> {
    /// Construct a reader and perform the `spec/01 §3.1` DWORD
    /// prefetch.
    ///
    /// The first four bytes are loaded as a little-endian DWORD into
    /// the accumulator and `bit_offset` is set to `0x20`. If the slice
    /// is shorter than 4 bytes the prefetch is impossible and
    /// [`BitReaderError::TooShortForPrefetch`] is returned.
    pub fn new(data: &'a [u8]) -> Result<Self, BitReaderError> {
        if data.len() < 4 {
            return Err(BitReaderError::TooShortForPrefetch {
                available: data.len(),
            });
        }
        let accumulator = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as u64;
        Ok(BitReader {
            data,
            next_byte: 4,
            accumulator,
            live_bits: 32,
            consumed: 0,
        })
    }

    /// Total bits consumed from the *front* of the stream so far
    /// (i.e. the bit position of the next unread bit). This is the
    /// quantity the byte-alignment helpers and the picture-header
    /// "bytes consumed" out-parameter (`spec/02 §2.8`) are computed
    /// from.
    pub fn bits_read(&self) -> u64 {
        self.consumed
    }

    /// Byte position of the next unread bit, rounded down. Equal to
    /// `bits_read() / 8`.
    pub fn byte_pos(&self) -> u64 {
        self.bits_read() / 8
    }

    /// Refill one byte into the accumulator just above the live
    /// region, mirroring the binary's one-byte refill primitive
    /// (`spec/01 §3.1`). Grows `live_bits` by 8.
    fn refill_one(&mut self) {
        let byte = if self.next_byte < self.data.len() {
            let b = self.data[self.next_byte];
            self.next_byte += 1;
            b
        } else {
            // Past the physical end of input. A clean-room decoder
            // treats reads past the slice as zero-fill so a truncated
            // stream surfaces as a structural error upstream rather
            // than a panic; the EOF guard in `read` stops runaway
            // refills.
            self.next_byte += 1;
            0
        };
        self.accumulator |= (byte as u64) << self.live_bits;
        self.live_bits += 8;
    }

    /// Read `n` bits LSB-first, returning them right-aligned in a
    /// `u32`. `n` must be in `0..=MAX_READ_BITS`.
    ///
    /// `read(0)` returns `0` without touching the stream.
    pub fn read(&mut self, n: u32) -> Result<u32, BitReaderError> {
        if n == 0 {
            return Ok(0);
        }
        if n > MAX_READ_BITS {
            return Err(BitReaderError::ReadTooWide { requested: n });
        }
        // Ensure at least n live bits are present at the low end,
        // refilling one byte at a time.
        while self.live_bits < n {
            // Guard against reading indefinitely past EOF.
            if self.next_byte > self.data.len() {
                return Err(BitReaderError::UnexpectedEof {
                    bits_consumed: self.consumed,
                });
            }
            self.refill_one();
        }
        let mask: u64 = (1u64 << n) - 1;
        let value = (self.accumulator & mask) as u32;
        self.accumulator >>= n;
        self.live_bits -= n;
        self.consumed += n as u64;
        Ok(value)
    }

    /// Read a single bit LSB-first.
    pub fn read_bit(&mut self) -> Result<u8, BitReaderError> {
        Ok(self.read(1)? as u8)
    }

    /// Consume `0..=7` bits to reach the next whole-byte boundary
    /// (`spec/02 §0` `align(8)`). A no-op when already byte-aligned.
    pub fn align(&mut self) -> Result<(), BitReaderError> {
        let partial = (self.bits_read() & 7) as u32;
        if partial != 0 {
            self.read(8 - partial)?;
        }
        Ok(())
    }

    /// Skip `n` bits (used by the opaque-extension loops in
    /// `spec/02 §1.9` / §2.5 / §3.9, which read-and-discard).
    pub fn skip(&mut self, n: u32) -> Result<(), BitReaderError> {
        // The widest discard is one byte at a time in the extension
        // loops, so route through `read` in 26-bit chunks to respect
        // the single-read bound.
        let mut remaining = n;
        while remaining > 0 {
            let take = remaining.min(MAX_READ_BITS);
            self.read(take)?;
            remaining -= take;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefetch_requires_four_bytes() {
        assert_eq!(
            BitReader::new(&[0x1f]),
            Err(BitReaderError::TooShortForPrefetch { available: 1 })
        );
        assert_eq!(
            BitReader::new(&[0, 0, 0]),
            Err(BitReaderError::TooShortForPrefetch { available: 3 })
        );
        assert!(BitReader::new(&[0, 0, 0, 0]).is_ok());
    }

    #[test]
    fn lsb_first_wiki_example() {
        // spec/00 §3: byte 0b01110000, reading 3 bits then 5 bits
        // returns 000b then 01110b.
        let data = [0b0111_0000u8, 0, 0, 0];
        let mut r = BitReader::new(&data).unwrap();
        assert_eq!(r.read(3).unwrap(), 0b000);
        assert_eq!(r.read(5).unwrap(), 0b01110);
    }

    #[test]
    fn picture_start_triplet_bits() {
        // PSC=0x1f (5 bits, all ones), frame_type=0 (3 bits),
        // frame_number=0 (8 bits). LSB-first: byte 0 low 5 bits =
        // 11111, next 3 bits (byte0 high) = frame_type, byte1 =
        // frame_number.
        // byte0 = frame_type<<5 | 0x1f. frame_type=0 -> 0x1f.
        let data = [0x1f, 0x00, 0x00, 0x00, 0x00];
        let mut r = BitReader::new(&data).unwrap();
        assert_eq!(r.read(5).unwrap(), 0x1f);
        assert_eq!(r.read(3).unwrap(), 0);
        assert_eq!(r.read(8).unwrap(), 0);
        assert_eq!(r.bits_read(), 16);
    }

    #[test]
    fn frame_type_in_high_bits_of_first_byte() {
        // PSC=0x1f, frame_type=4 (NULL). byte0 = (4<<5)|0x1f = 0x9f.
        let data = [0x9f, 0x07, 0x00, 0x00, 0x00];
        let mut r = BitReader::new(&data).unwrap();
        assert_eq!(r.read(5).unwrap(), 0x1f);
        assert_eq!(r.read(3).unwrap(), 4);
        assert_eq!(r.read(8).unwrap(), 0x07);
    }

    #[test]
    fn refill_crosses_dword_boundary() {
        // Read 32 bits across the prefetch, then one more byte must
        // refill cleanly.
        let data = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x00, 0x00];
        let mut r = BitReader::new(&data).unwrap();
        assert_eq!(r.read(8).unwrap(), 0x11);
        assert_eq!(r.read(8).unwrap(), 0x22);
        assert_eq!(r.read(8).unwrap(), 0x33);
        assert_eq!(r.read(8).unwrap(), 0x44);
        // Accumulator now exhausted of prefetch; next read refills.
        assert_eq!(r.read(8).unwrap(), 0x55);
        assert_eq!(r.read(8).unwrap(), 0x66);
        assert_eq!(r.bits_read(), 48);
    }

    #[test]
    fn align_to_byte_boundary() {
        let data = [0xff, 0xff, 0xff, 0xff, 0xff];
        let mut r = BitReader::new(&data).unwrap();
        r.read(5).unwrap();
        assert_eq!(r.bits_read(), 5);
        r.align().unwrap();
        assert_eq!(r.bits_read(), 8);
        // Already aligned -> no-op.
        r.align().unwrap();
        assert_eq!(r.bits_read(), 8);
    }

    #[test]
    fn read_zero_is_noop() {
        let data = [0xab, 0xcd, 0xef, 0x12];
        let mut r = BitReader::new(&data).unwrap();
        assert_eq!(r.read(0).unwrap(), 0);
        assert_eq!(r.bits_read(), 0);
    }

    #[test]
    fn read_too_wide_rejected() {
        let data = [0, 0, 0, 0, 0];
        let mut r = BitReader::new(&data).unwrap();
        assert_eq!(
            r.read(27),
            Err(BitReaderError::ReadTooWide { requested: 27 })
        );
    }

    #[test]
    fn twenty_six_bit_split_pair() {
        // spec/02 §1.6 custom dimensions: 26-bit read, high 13 = height,
        // low 13 = width. Build height=200, width=100.
        // value = (height << 13) | width, emitted LSB-first.
        let height = 200u32;
        let width = 100u32;
        let value = (height << 13) | width;
        let mut bytes = value.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        let mut r = BitReader::new(&bytes).unwrap();
        let combined = r.read(26).unwrap();
        assert_eq!(combined & 0x1fff, width);
        assert_eq!(combined >> 13, height);
    }
}

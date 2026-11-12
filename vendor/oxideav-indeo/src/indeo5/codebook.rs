//! Indeo 5 Huffman codebooks (the IVI prefix-code form).
//!
//! Spec sources: `docs/video/indeo/indeo5/spec/04-entropy.md` §1
//! (descriptor records, preset numeric data), the Indeo 4 wiki annex A
//! ("Huffman coding", `docs/video/indeo/indeo4/wiki/Indeo_4.wiki`) that
//! the Indeo 5 wiki page's descriptor sections defer to, and the r338
//! Kraft-anomaly note
//! (`provenance/11-extractor-univdreams-dispatch-and-arbitration.md`
//! Ask 3).
//!
//! ## Descriptor semantics — resolved Kraft anomaly
//!
//! A descriptor is a row list `xbits[0..num_rows]`. Per the Indeo 4
//! wiki annex A (shared by Indeo 5), row `k`'s codewords are
//!
//! ```text
//! [k one-bits] [0] [xbits[k] extra bits]      (k < num_rows - 1)
//! [k one-bits]     [xbits[k] extra bits]      (k == num_rows - 1)
//! ```
//!
//! — the terminating `0` of the last row is "replaced with an x bit"
//! (the wiki's asterisk convention), making the code exactly complete:
//! the Kraft sum is `Σ_{k<n-1} 2^-(k+1) + 2^-(n-1) = 1` for **any**
//! `xbits` values. This resolves the reported spec/04 Kraft anomaly:
//! the preset records are *not* per-symbol code lengths (under that
//! misreading six of eight block presets over-subscribe, exactly the
//! anomalous set the r338 note reproduces) but per-row extra-bit
//! counts of the prefix form above.
//!
//! Row `k` carries `2^xbits[k]` consecutive symbols; symbol indices
//! accumulate across rows in descriptor order (`base[k] = Σ_{j<k}
//! 2^xbits[j]`).
//!
//! **Extra-bit order (fixture-arbitrated).** The stream is LSB-first
//! per byte (`spec/00 §3`); within one codeword the extra bits are
//! consumed most-significant-first (each successive stream bit shifts
//! into the low end of the value). This reading was arbitrated against
//! the two staged `IV50` INTRA fixtures
//! (`docs/video/indeo/indeo5/fixtures/`): it is the only variant under
//! which all six band payloads decode to byte-exact exhaustion.

use super::bitreader::{BitReader, BitReaderError};

/// Spec/04 §1.3 — the maximum number of rows a descriptor can declare.
/// The inline form's `num_rows` is a 4-bit field; the presets observe
/// `8..=13` (`spec/04 §1.4`/`§1.5`).
pub const MAX_ROWS: usize = 16;

/// Spec/04 §1.5 — the eight **mb-Huffman** preset descriptors
/// (`IR50_32.DLL!.rdata 0x1008d710`, Table A; also the Indeo 4 wiki
/// annex D "Macroblock huffman tables"). Each record is the per-row
/// `xbits` array. Record 7 is the implicit default (`spec/04 §1.1`).
pub const MB_HUFF_PRESETS: [&[u8]; 8] = [
    &[0, 4, 5, 4, 4, 4, 6, 6],
    &[0, 2, 2, 3, 3, 3, 3, 5, 3, 2, 2, 2],
    &[0, 2, 3, 4, 3, 3, 3, 3, 4, 3, 2, 2],
    &[0, 3, 4, 4, 3, 3, 3, 3, 3, 2, 2, 2],
    &[0, 4, 4, 3, 3, 3, 3, 2, 3, 3, 2, 1, 1],
    &[0, 4, 4, 4, 4, 3, 3, 3, 2],
    &[0, 4, 4, 4, 4, 3, 3, 2, 2, 2],
    &[0, 4, 4, 4, 3, 3, 2, 3, 2, 2, 2, 2],
];

/// Spec/04 §1.4 — the eight **block-Huffman** preset descriptors
/// (`IR50_32.DLL!.rdata 0x1008d798`, Table B; also the Indeo 4 wiki
/// annex D "Block huffman tables"). Record 7 is the implicit default
/// (`spec/04 §1.2`).
pub const BLOCK_HUFF_PRESETS: [&[u8]; 8] = [
    &[1, 2, 3, 4, 4, 7, 5, 5, 4, 1],
    &[2, 3, 4, 4, 4, 7, 5, 4, 3, 3, 2],
    &[2, 4, 5, 5, 5, 5, 6, 4, 4, 3, 1, 1],
    &[3, 3, 4, 4, 5, 6, 6, 4, 4, 3, 2, 1, 1],
    &[3, 4, 4, 5, 5, 5, 6, 5, 4, 2, 2],
    &[3, 4, 5, 5, 5, 5, 6, 4, 3, 3, 2, 1, 1],
    &[3, 4, 5, 5, 5, 6, 5, 4, 3, 3, 2, 1, 1],
    &[3, 4, 4, 5, 5, 5, 6, 5, 5],
];

/// Spec/04 §1.1 / §1.2 — the implicit-default preset index (record 7).
pub const DEFAULT_PRESET_ID: usize = 7;

/// The Huffman context a descriptor belongs to (`spec/04 §0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HuffContext {
    /// Per-MB header VLCs (`mb_huff_desc`, Table A presets).
    Mb,
    /// Per-block coefficient VLCs (`blk_huff_desc`, Table B presets).
    Block,
}

impl HuffContext {
    /// The eight preset descriptors for this context.
    pub fn presets(self) -> &'static [&'static [u8]; 8] {
        match self {
            HuffContext::Mb => &MB_HUFF_PRESETS,
            HuffContext::Block => &BLOCK_HUFF_PRESETS,
        }
    }
}

/// Errors raised while building or decoding a codebook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CodebookError {
    /// A descriptor declared no rows at all.
    NoRows,
    /// A descriptor declared more than [`MAX_ROWS`] rows.
    TooManyRows {
        /// The row count found.
        found: usize,
    },
    /// A row carried an `xbits` wider than the inline 4-bit field
    /// permits (`> 15`, `spec/04 §1.6`).
    XbitsTooWide {
        /// The offending row index.
        row: usize,
        /// The `xbits` found.
        xbits: u8,
    },
    /// Underlying bit-reader fault during decode.
    BitReader(BitReaderError),
}

impl From<BitReaderError> for CodebookError {
    fn from(e: BitReaderError) -> Self {
        CodebookError::BitReader(e)
    }
}

impl core::fmt::Display for CodebookError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CodebookError::NoRows => write!(f, "indeo5 codebook: descriptor has no rows"),
            CodebookError::TooManyRows { found } => write!(
                f,
                "indeo5 codebook: {found} rows exceeds the maximum {MAX_ROWS} (spec/04 §1.3)"
            ),
            CodebookError::XbitsTooWide { row, xbits } => write!(
                f,
                "indeo5 codebook: row {row} xbits {xbits} exceeds 15 (spec/04 §1.6)"
            ),
            CodebookError::BitReader(e) => write!(f, "indeo5 codebook: {e}"),
        }
    }
}

impl std::error::Error for CodebookError {}

/// One codeword of the prefix-form codebook, in stream order: the
/// `length` bits of `code` are consumed most-significant-bit first
/// (prefix ones, the terminating zero when present, then the extra
/// bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Codeword {
    /// The symbol index.
    pub symbol: u16,
    /// Total codeword bit-length.
    pub length: u8,
    /// The codeword bits, MSB-first over `length` bits.
    pub code: u32,
}

/// A built Indeo prefix-form codebook.
///
/// Construct from a descriptor with [`Codebook::build`] (or
/// [`Codebook::from_preset`] / [`Codebook::from_huff_desc`]), then
/// decode symbols off an LSB-first [`BitReader`] with
/// [`Codebook::decode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Codebook {
    xbits: Vec<u8>,
    /// `base[k]` = first symbol index of row `k`.
    base: Vec<u32>,
    num_symbols: u32,
}

impl Codebook {
    /// Build a codebook from a per-row `xbits` descriptor.
    pub fn build(xbits: &[u8]) -> Result<Self, CodebookError> {
        if xbits.is_empty() {
            return Err(CodebookError::NoRows);
        }
        if xbits.len() > MAX_ROWS {
            return Err(CodebookError::TooManyRows { found: xbits.len() });
        }
        let mut base = Vec::with_capacity(xbits.len());
        let mut sum: u32 = 0;
        for (row, &x) in xbits.iter().enumerate() {
            if x > 15 {
                return Err(CodebookError::XbitsTooWide { row, xbits: x });
            }
            base.push(sum);
            sum += 1u32 << x;
        }
        Ok(Codebook {
            xbits: xbits.to_vec(),
            base,
            num_symbols: sum,
        })
    }

    /// Build the codebook for a context's preset record
    /// (`huff_table_id 0..=7`, `spec/04 §1.4`/`§1.5`). All sixteen
    /// preset records build valid (exactly complete) codebooks under
    /// the prefix form.
    pub fn from_preset(context: HuffContext, id: usize) -> Result<Self, CodebookError> {
        Codebook::build(context.presets()[id & 7])
    }

    /// Build the codebook a [`super::HuffDesc`] selects, defaulting to
    /// the context's preset 7 when `desc` is `None` (`spec/04
    /// §1.1`/`§1.2`).
    pub fn from_huff_desc(
        context: HuffContext,
        desc: Option<&super::HuffDesc>,
    ) -> Result<Self, CodebookError> {
        match desc {
            None => Codebook::from_preset(context, DEFAULT_PRESET_ID),
            Some(super::HuffDesc::Preset { id }) => Codebook::from_preset(context, *id as usize),
            Some(super::HuffDesc::Custom { row_lengths }) => Codebook::build(row_lengths),
        }
    }

    /// The raw per-row `xbits` record for a context's preset
    /// (`spec/04 §1.4`/`§1.5`).
    pub fn raw_preset(context: HuffContext, id: usize) -> &'static [u8] {
        context.presets()[id & 7]
    }

    /// Number of rows in the descriptor.
    pub fn num_rows(&self) -> usize {
        self.xbits.len()
    }

    /// Total number of symbols (`Σ 2^xbits[k]`).
    pub fn num_symbols(&self) -> u32 {
        self.num_symbols
    }

    /// The scaled Kraft sum over the codeword set, as
    /// `(sum, scale_log2)` with the code complete iff
    /// `sum == 1 << scale_log2`. The prefix form is exactly complete
    /// for every descriptor: `Σ_{k<n-1} 2^-(k+1) + 2^-(n-1) = 1`.
    pub fn kraft_scaled(&self) -> (u64, u8) {
        let n = self.xbits.len() as u32;
        if n == 1 {
            return (1, 0);
        }
        let mut sum: u64 = 1; // last row: 2^-(n-1), scaled by 2^(n-1)
        for k in 0..(n - 1) {
            sum += 1u64 << (n - 2 - k);
        }
        (sum, (n - 1) as u8)
    }

    /// The codeword for `symbol`, in stream order (MSB-first bits:
    /// prefix ones, terminating zero when not the last row, then the
    /// extra bits). `None` when the symbol is out of range.
    pub fn codeword(&self, symbol: u32) -> Option<Codeword> {
        if symbol >= self.num_symbols {
            return None;
        }
        let n = self.xbits.len();
        let mut row = n - 1;
        for k in 0..n {
            if self.base[k] > symbol {
                row = k - 1;
                break;
            }
        }
        let extra = symbol - self.base[row];
        let x = self.xbits[row] as u32;
        let last = row == n - 1;
        let prefix_len = row as u32 + u32::from(!last);
        // `row` ones followed by a zero (unless last row).
        let prefix: u32 = if row == 0 { 0 } else { (1 << row) - 1 };
        let prefix = prefix << (prefix_len - row as u32);
        let length = prefix_len + x;
        Some(Codeword {
            symbol: symbol as u16,
            length: length as u8,
            code: (prefix << x) | extra,
        })
    }

    /// Decode one symbol from an LSB-first bit reader.
    ///
    /// Counts the run of one-bits (the row prefix, capped at
    /// `num_rows - 1` where the terminating zero is absent), then reads
    /// the row's extra bits most-significant-first.
    pub fn decode(&self, r: &mut BitReader<'_>) -> Result<u32, CodebookError> {
        let n = self.xbits.len();
        let mut row = 0usize;
        while row < n - 1 && r.read_bit()? == 1 {
            row += 1;
        }
        let x = self.xbits[row];
        let mut extra: u32 = 0;
        for _ in 0..x {
            extra = (extra << 1) | u32::from(r.read_bit()?);
        }
        Ok(self.base[row] + extra)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LSB-first bit packer mirroring the [`BitReader`] bit order.
    struct BitWriter {
        bits: Vec<u8>,
    }
    impl BitWriter {
        fn new() -> Self {
            BitWriter { bits: Vec::new() }
        }
        /// Append `n` bits LSB-first (low bit of `value` first).
        fn put(&mut self, value: u32, n: u32) {
            for i in 0..n {
                self.bits.push(((value >> i) & 1) as u8);
            }
        }
        /// Append a codeword: stream order is MSB-first over `length`.
        fn put_codeword(&mut self, cw: Codeword) {
            for i in (0..cw.length).rev() {
                self.bits.push(((cw.code >> i) & 1) as u8);
            }
        }
        fn finish(mut self) -> Vec<u8> {
            while !self.bits.len().is_multiple_of(8) {
                self.bits.push(0);
            }
            let mut out = Vec::new();
            for chunk in self.bits.chunks(8) {
                let mut byte = 0u8;
                for (i, &b) in chunk.iter().enumerate() {
                    byte |= b << i;
                }
                out.push(byte);
            }
            while out.len() < 8 {
                out.push(0);
            }
            out
        }
    }

    #[test]
    fn wiki_annex_a_example_codebook() {
        // Indeo 4 wiki annex A: numRows = 7, xbits = 1,2,3,4,5,6,7*.
        // Row 0: "0x" (2 symbols), ..., row 6: "111111" + 7 extra bits
        // (128 symbols, no terminating zero).
        let cb = Codebook::build(&[1, 2, 3, 4, 5, 6, 7]).unwrap();
        assert_eq!(cb.num_symbols(), 2 + 4 + 8 + 16 + 32 + 64 + 128);
        // Row-0 symbol 1: "0" + extra 1 -> bits 01 (MSB-first), len 2.
        assert_eq!(
            cb.codeword(1),
            Some(Codeword {
                symbol: 1,
                length: 2,
                code: 0b01
            })
        );
        // Row-1 symbol 2 (base 2, extra 0): "10" + "00" -> 1000, len 4.
        assert_eq!(
            cb.codeword(2),
            Some(Codeword {
                symbol: 2,
                length: 4,
                code: 0b1000
            })
        );
        // Last-row base symbol: 6 ones then 7 extra zeros, no
        // terminating zero: len 13, code = 1111110000000.
        let base6 = 2 + 4 + 8 + 16 + 32 + 64;
        assert_eq!(
            cb.codeword(base6),
            Some(Codeword {
                symbol: base6 as u16,
                length: 13,
                code: 0b111111 << 7
            })
        );
        assert_eq!(cb.codeword(cb.num_symbols()), None);
    }

    #[test]
    fn prefix_form_is_exactly_complete() {
        // Kraft sum == 1 for every preset of both contexts — the r338
        // Kraft-anomaly resolution.
        for id in 0..8 {
            for ctx in [HuffContext::Mb, HuffContext::Block] {
                let cb = Codebook::from_preset(ctx, id).unwrap();
                let (sum, scale) = cb.kraft_scaled();
                assert_eq!(sum, 1u64 << scale, "{ctx:?} preset {id}");
            }
        }
    }

    #[test]
    fn preset_alphabet_sizes() {
        // Block presets 0..=6 span exactly 256 symbols — the size of
        // the per-band rv-table composite space; the default preset 7
        // spans 264.
        for id in 0..7 {
            let cb = Codebook::from_preset(HuffContext::Block, id).unwrap();
            assert_eq!(cb.num_symbols(), 256, "block preset {id}");
        }
        let cb = Codebook::from_preset(HuffContext::Block, 7).unwrap();
        assert_eq!(cb.num_symbols(), 264);
    }

    #[test]
    fn decode_round_trips_every_row() {
        let cb = Codebook::build(&[0, 2, 3, 1]).unwrap();
        // Symbols: row0 {0}, row1 {1..=4}, row2 {5..=12}, row3 {13,14}.
        assert_eq!(cb.num_symbols(), 15);
        let mut w = BitWriter::new();
        let symbols = [0u32, 3, 5, 12, 13, 14, 0, 4];
        for &s in &symbols {
            w.put_codeword(cb.codeword(s).unwrap());
        }
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        for &s in &symbols {
            assert_eq!(cb.decode(&mut r).unwrap(), s);
        }
    }

    #[test]
    fn extra_bits_are_msb_first() {
        // Fixture-arbitrated bit order: for row-0 xbits=2, the first
        // stream bit after the (empty) prefix is the extra value's MSB.
        let cb = Codebook::build(&[2]).unwrap();
        // Stream bits: 1,0 -> extra = 0b10 = 2.
        let mut w = BitWriter::new();
        w.put(1, 1);
        w.put(0, 1);
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        assert_eq!(cb.decode(&mut r).unwrap(), 2);
    }

    #[test]
    fn last_row_has_no_terminator() {
        // Two rows, xbits [1, 1]: row 0 = "0x", row 1 = "1x" (the
        // terminating zero replaced by the extra bit).
        let cb = Codebook::build(&[1, 1]).unwrap();
        assert_eq!(cb.codeword(0).unwrap().length, 2); // 0 + x
        assert_eq!(cb.codeword(2).unwrap().length, 2); // 1 + x
        let mut w = BitWriter::new();
        for s in [0, 1, 2, 3] {
            w.put_codeword(cb.codeword(s).unwrap());
        }
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        for s in [0, 1, 2, 3] {
            assert_eq!(cb.decode(&mut r).unwrap(), s);
        }
        assert_eq!(r.bits_read(), 8);
    }

    #[test]
    fn build_rejects_bad_descriptors() {
        assert_eq!(Codebook::build(&[]), Err(CodebookError::NoRows));
        assert_eq!(
            Codebook::build(&[1; 17]),
            Err(CodebookError::TooManyRows { found: 17 })
        );
        assert_eq!(
            Codebook::build(&[16]),
            Err(CodebookError::XbitsTooWide { row: 0, xbits: 16 })
        );
    }

    #[test]
    fn from_huff_desc_routes() {
        let def = Codebook::from_huff_desc(HuffContext::Block, None).unwrap();
        assert_eq!(def, Codebook::from_preset(HuffContext::Block, 7).unwrap());

        let desc = super::super::HuffDesc::Preset { id: 3 };
        assert_eq!(
            Codebook::from_huff_desc(HuffContext::Block, Some(&desc)).unwrap(),
            Codebook::from_preset(HuffContext::Block, 3).unwrap()
        );

        let custom = super::super::HuffDesc::Custom {
            row_lengths: vec![0, 2, 3, 5, 5, 6, 7],
        };
        let cb = Codebook::from_huff_desc(HuffContext::Block, Some(&custom)).unwrap();
        // The 320x240 fixture's Y-band custom descriptor: 269 symbols.
        assert_eq!(cb.num_symbols(), 269);
    }

    #[test]
    fn raw_preset_exposes_numeric_data() {
        assert_eq!(
            Codebook::raw_preset(HuffContext::Block, 0),
            &[1, 2, 3, 4, 4, 7, 5, 5, 4, 1]
        );
        assert_eq!(
            Codebook::raw_preset(HuffContext::Mb, 0),
            &[0, 4, 5, 4, 4, 4, 6, 6]
        );
        assert_eq!(
            Codebook::raw_preset(HuffContext::Block, 7),
            Codebook::raw_preset(HuffContext::Block, 15)
        );
    }
}

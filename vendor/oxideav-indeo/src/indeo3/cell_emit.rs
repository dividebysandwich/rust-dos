//! Indeo 3 in-cell predictor chain: the per-row outer-loop driver that
//! turns the round-6/7 single-position dyad-pair emission
//! ([`super::emit_variant`]) into a complete cell decode over a real
//! strip pixel buffer.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/07-output-reconstruction.md`
//! §1.2 (in-cell predictor chain) + §2.1 / §2.4 (dyad-pair add +
//! per-sample iteration order), cross-referenced from
//! `docs/video/indeo/indeo3/spec/06-entropy.md` §6.3 / §6.4.
//!
//! Rounds 6 and 7 (`reconstruct`) landed the *single-position*
//! arithmetic: given one predictor DWORD and one primary / secondary
//! delta entry, [`super::apply_dyad_pair`] / [`super::emit_variant`]
//! form the output pixel-pair DWORD and decide the per-variant store
//! shape. What they explicitly deferred is the **chain**: a cell is N
//! rows tall (§1.2: `N ∈ {4, 8}`), and the predictor for row `k` is the
//! row the decoder *just emitted* at row `k-1` of the same cell, read
//! back via `[edi - 0xb0]`. The binary realises this with the per-row
//! outer-loop tail at `IR32_32.DLL!0x10006fc0..0x10006fdb`:
//!
//! ```text
//! add [esp + 0x34], 0xb0    ; cell-position offset += row stride
//! mov eax, [esp + 0x20]      ; horizontal stride
//! mov cl, ch                 ; reset row column counter
//! add edi, eax               ; advance pixel-buffer pointer
//! sub ecx, 0x1000000         ; row-band counter
//! jae 0x10006cb2             ; next row
//! ```
//!
//! This module owns that outer loop. It walks a cell's rows top to
//! bottom over a caller-supplied `&mut [u8]` strip pixel buffer; for
//! each row it reads the row-above predictor DWORD(s) out of the buffer
//! (`[edi - 0xb0]`, or the §1.3 top-of-strip constant
//! [`super::TOP_OF_STRIP_PREDICTOR`] when the row-above slot falls in
//! the strip's pre-allocated padding), applies the §2.4 left-to-right
//! dyad-pair iteration via [`super::emit_variant`], and writes the
//! emitted row(s) back into the buffer so the next row's predictor
//! re-read picks them up. The §6.4 sign disposition propagates straight
//! through: a [`super::DyadOutcome::Fault`] at any position aborts the
//! chain with [`CellEmitError::DyadFault`] (the binary's error-code-2
//! fault at `0x1000855f`).
//!
//! What this module **deliberately does not do** (the §1 chapter
//! boundary):
//!
//! * It does not read the bitstream. The per-position primary /
//!   secondary delta DWORDs are supplied by the caller (the entropy
//!   dispatcher in `spec/06 §3` resolves the per-frame-arena lookup;
//!   the codebook-bank values themselves are `spec/07 §3.4` / `§7.1`
//!   Extractor territory).
//! * It does not perform the §1.3 cross-cell predictor continuity /
//!   inter-cell edge fix-up (owned by [`super::StripEdgeFixupDims`] /
//!   `spec/03 §5.5`). It reads whatever the buffer holds one row above
//!   the cell's top, which is the caller's responsibility to have set
//!   up (the fix-up, or the zero-fill seed at top-of-strip).
//! * It does not perform the §1.4 VQ_NULL copy-upper path (a pure
//!   predictor copy with no delta) — that is a distinct unpacker arm.
//! * It does not perform the §4.3 7-bit→8-bit output upshift (owned by
//!   [`super::upshift_7bit_to_8bit`]) or the §5.7 strip-to-frame
//!   assembly (owned by [`super::assemble_plane_if09`]).

use super::reconstruct::{
    DyadOutcome, PREDICTOR_ROW_STRIDE, TOP_OF_STRIP_PREDICTOR, VariantEmission, emit_variant,
    pack_predictor,
};
use super::vq::CellVariant;

/// Spec/07 §2.4 — the number of pixels one dyad-pair DWORD covers in a
/// row (a 4-byte softSIMD DWORD = four horizontally adjacent pixels).
pub const PIXELS_PER_DYAD_DWORD: usize = 4;

/// Spec/07 §1.2 / §2.2 — the destination-pointer advance, in row-stride
/// units, the outer loop applies after emitting one *source* row of a
/// cell.
///
/// Variant A (plain) and variants C / D (row-doubling) emit two output
/// rows per source row and advance two row strides (`2 * 0xb0`);
/// variant B (with-edge averaging) emits one output row and advances
/// one row stride (`0xb0`). This mirrors the `mov eax, [esp + 0x20]`
/// horizontal-stride pick at `IR32_32.DLL!0x100066c7` that is set per
/// cell-shape variant.
pub const fn rows_per_source_row(variant: CellVariant) -> usize {
    match variant {
        CellVariant::WithEdge => 1,
        CellVariant::Plain | CellVariant::DoubledRow | CellVariant::FullyDoubled => 2,
    }
}

/// The geometry of one cell to drive through the in-cell predictor
/// chain.
///
/// `width_dwords` is the cell's width measured in dyad-pair DWORDs
/// (= width-in-pixels / 4, per §2.4: a row of an 8-pixel-wide cell is
/// two dyad-pair DWORDs). `source_rows` is the number of *source* rows
/// the unpacker reads from the bitstream (the §1.2 `N` before the
/// per-variant vertical doubling); the number of *emitted* output rows
/// is `source_rows * rows_per_source_row(variant)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellEmitGeometry {
    /// Cell width in dyad-pair DWORDs (pixels / 4).
    pub width_dwords: usize,
    /// Number of source rows the unpacker reads (§1.2 `N`).
    pub source_rows: usize,
    /// Byte offset of the cell's top-left pixel within the strip buffer.
    pub top_left_offset: usize,
    /// The cell-shape variant (controls the per-row store shape and the
    /// destination-pointer advance).
    pub variant: CellVariant,
}

/// The per-position delta entries the caller feeds the chain, in §2.4
/// row-major left-to-right order (row 0 dyad 0, row 0 dyad 1, …, then
/// row 1, …).
///
/// Each entry pairs the per-frame-arena primary-table DWORD with the
/// secondary-table 16-bit word the continuation path consults
/// ([`super::apply_dyad_pair`]). The caller supplies exactly
/// `width_dwords * source_rows` entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DyadDelta {
    /// The per-frame-arena primary-table DWORD (`[esi + 4*edx + 0x400]`).
    pub primary: u32,
    /// The secondary-table 16-bit word (`[esi + 4*edx + 0x402]`),
    /// consulted only on a continuation.
    pub secondary: u16,
}

/// The error modes the in-cell predictor chain surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CellEmitError {
    /// `width_dwords` or `source_rows` is zero (a degenerate cell the
    /// §2.4 iteration order cannot describe). Carries which dimension.
    ZeroDimension {
        /// `true` if `width_dwords == 0`, `false` if `source_rows == 0`.
        is_width: bool,
    },
    /// The number of supplied [`DyadDelta`] entries does not equal
    /// `width_dwords * source_rows`.
    DeltaCountMismatch {
        /// The expected entry count (`width_dwords * source_rows`).
        expected: usize,
        /// The supplied entry count.
        supplied: usize,
    },
    /// A row's write region would land outside the strip buffer (the
    /// `[edi]` store at `top_left_offset + emitted_row * 0xb0 +
    /// dword * 4 .. +4` exceeds `buffer.len()`). Carries the offending
    /// end offset and the buffer length.
    WriteOutOfBounds {
        /// The exclusive end byte offset the store would require.
        write_end: usize,
        /// The supplied buffer length.
        buffer_len: usize,
    },
    /// The §2.1 dyad-pair add left the low half's sign bit set after the
    /// secondary-table add: the binary's error-code-2 fault at
    /// `IR32_32.DLL!0x1000855f` (§6.4 negative-overflow). Carries the
    /// source row and dyad-DWORD column at which the fault occurred.
    DyadFault {
        /// The source-row index (0-based) of the faulting position.
        row: usize,
        /// The dyad-DWORD column index (0-based) of the faulting
        /// position.
        dword: usize,
    },
}

impl core::fmt::Display for CellEmitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CellEmitError::ZeroDimension { is_width } => write!(
                f,
                "spec/07 §2.4: cell {} is zero",
                if *is_width {
                    "width"
                } else {
                    "source-row count"
                }
            ),
            CellEmitError::DeltaCountMismatch { expected, supplied } => write!(
                f,
                "spec/07 §2.4: dyad-delta count {supplied} != expected {expected}"
            ),
            CellEmitError::WriteOutOfBounds {
                write_end,
                buffer_len,
            } => write!(
                f,
                "spec/07 §1.2: row store end {write_end} exceeds strip buffer length {buffer_len}"
            ),
            CellEmitError::DyadFault { row, dword } => write!(
                f,
                "spec/07 §6.4: dyad-pair underflow fault (error code 2) at row {row}, dword {dword}"
            ),
        }
    }
}

/// The result of driving a cell through the in-cell predictor chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct CellEmitStats {
    /// The number of source rows processed (= `geometry.source_rows`).
    pub source_rows: usize,
    /// The number of output rows written
    /// (`source_rows * rows_per_source_row(variant)`).
    pub emitted_rows: usize,
    /// The number of continuation bytes the chain consumed (the count
    /// of positions that took the [`super::DyadOutcome::Continuation`]
    /// path — the `inc ebp` advances the caller must account for in the
    /// bitstream cursor).
    pub continuation_bytes: usize,
}

/// Read the row-above predictor DWORD for a write at byte
/// `write_offset` in the strip buffer (§1.1 `[edi - 0xb0]`).
///
/// When the row-above slot falls before the start of the buffer (the
/// §1.3 top-of-strip case), every predictor byte is the constant
/// [`TOP_OF_STRIP_PREDICTOR`] (`0x40`). When the slot is in-buffer the
/// four predictor bytes are read little-endian, matching the binary's
/// `mov eax, [edi - 0xb0]` DWORD load.
fn predictor_dword(buffer: &[u8], write_offset: usize) -> u32 {
    match write_offset.checked_sub(PREDICTOR_ROW_STRIDE) {
        // §1.3 top-of-strip: row-above is the zero-fill padding seed.
        None => pack_predictor([TOP_OF_STRIP_PREDICTOR; 4]),
        Some(pred_off) => {
            let mut bytes = [TOP_OF_STRIP_PREDICTOR; 4];
            for (i, b) in bytes.iter_mut().enumerate() {
                // A partial read past the buffer end keeps the zero seed
                // for the missing bytes; callers size the buffer to the
                // cell, so this is a defensive clamp rather than the hot
                // path.
                if let Some(v) = buffer.get(pred_off + i) {
                    *b = *v;
                }
            }
            pack_predictor(bytes)
        }
    }
}

/// Spec/07 §1.2 / §2.4 — drive one cell through the in-cell predictor
/// chain over a caller-supplied strip pixel buffer.
///
/// Walks the cell's `source_rows` rows top to bottom. For each source
/// row, walks the row's `width_dwords` dyad-pair positions left to
/// right (§2.4 row-major order). Each position:
///
/// 1. reads the row-above predictor DWORD from the buffer
///    ([`predictor_dword`], `[edi - 0xb0]`),
/// 2. applies the per-variant emission ([`super::emit_variant`]) — the
///    §2.1 dyad-pair add plus the variant's store shape,
/// 3. writes the emitted DWORD(s) into the buffer at the destination
///    row(s), so a subsequent row's predictor re-read sees them.
///
/// `deltas` are the per-position primary / secondary entries in §2.4
/// row-major order (`deltas[row * width_dwords + dword]`). On any
/// position's [`super::DyadOutcome::Fault`] the walk aborts with
/// [`CellEmitError::DyadFault`] (the binary's error-code-2 fault),
/// leaving the buffer mutated up to but not including the faulting
/// position. On success returns a [`CellEmitStats`] carrying the row
/// counts and the consumed-continuation-byte count.
pub fn emit_cell_chain(
    buffer: &mut [u8],
    geometry: CellEmitGeometry,
    deltas: &[DyadDelta],
) -> Result<CellEmitStats, CellEmitError> {
    if geometry.width_dwords == 0 {
        return Err(CellEmitError::ZeroDimension { is_width: true });
    }
    if geometry.source_rows == 0 {
        return Err(CellEmitError::ZeroDimension { is_width: false });
    }
    let expected = geometry.width_dwords * geometry.source_rows;
    if deltas.len() != expected {
        return Err(CellEmitError::DeltaCountMismatch {
            expected,
            supplied: deltas.len(),
        });
    }

    let rows_per = rows_per_source_row(geometry.variant);
    let mut continuation_bytes = 0usize;
    // The destination pointer `edi` for the current source row's first
    // dyad position. The §1.2 outer-loop tail advances it by
    // `rows_per * 0xb0` after each source row (the per-variant
    // horizontal-stride pick at 0x100066c7).
    let mut row_dst_offset = geometry.top_left_offset;

    for src_row in 0..geometry.source_rows {
        for dword in 0..geometry.width_dwords {
            let write_offset = row_dst_offset + dword * PIXELS_PER_DYAD_DWORD;
            let predictor = predictor_dword(buffer, write_offset);
            let delta = deltas[src_row * geometry.width_dwords + dword];
            let VariantEmission { outcome, rows } =
                emit_variant(geometry.variant, predictor, delta.primary, delta.secondary);

            match outcome {
                DyadOutcome::Fault => {
                    return Err(CellEmitError::DyadFault {
                        row: src_row,
                        dword,
                    });
                }
                DyadOutcome::Continuation { .. } => continuation_bytes += 1,
                DyadOutcome::Primary { .. } => {}
            }

            // Store the emitted row(s). `rows[i]` lands at
            // `write_offset + i * 0xb0` (the `[edi]`, `[edi + 0xb0]`
            // stores of §2.2). Bounds-check against the buffer first so
            // a malformed geometry surfaces a typed error rather than a
            // panic.
            for (i, &dest_dword) in rows.as_slice().iter().enumerate() {
                let store_off = write_offset + i * PREDICTOR_ROW_STRIDE;
                let write_end = store_off + PIXELS_PER_DYAD_DWORD;
                if write_end > buffer.len() {
                    return Err(CellEmitError::WriteOutOfBounds {
                        write_end,
                        buffer_len: buffer.len(),
                    });
                }
                buffer[store_off..write_end].copy_from_slice(&dest_dword.to_le_bytes());
            }
        }
        // §1.2 outer-loop tail: advance `edi` by the per-variant row
        // stride. The next source row's predictor re-read at
        // `[edi - 0xb0]` then reads this row's just-emitted output.
        row_dst_offset += rows_per * PREDICTOR_ROW_STRIDE;
    }

    Ok(CellEmitStats {
        source_rows: geometry.source_rows,
        emitted_rows: geometry.source_rows * rows_per,
        continuation_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::reconstruct::{apply_dyad_pair, pack_predictor, unpack_pixels};

    const STRIDE: usize = PREDICTOR_ROW_STRIDE;

    fn d(primary: u32, secondary: u16) -> DyadDelta {
        DyadDelta { primary, secondary }
    }

    #[test]
    fn pixels_per_dword_and_rows_per_source_row() {
        assert_eq!(PIXELS_PER_DYAD_DWORD, 4);
        assert_eq!(rows_per_source_row(CellVariant::WithEdge), 1);
        assert_eq!(rows_per_source_row(CellVariant::Plain), 2);
        assert_eq!(rows_per_source_row(CellVariant::DoubledRow), 2);
        assert_eq!(rows_per_source_row(CellVariant::FullyDoubled), 2);
    }

    #[test]
    fn zero_width_and_zero_rows_rejected() {
        let mut buf = vec![0u8; STRIDE * 8];
        let g = CellEmitGeometry {
            width_dwords: 0,
            source_rows: 4,
            top_left_offset: STRIDE,
            variant: CellVariant::WithEdge,
        };
        assert_eq!(
            emit_cell_chain(&mut buf, g, &[]),
            Err(CellEmitError::ZeroDimension { is_width: true })
        );
        let g = CellEmitGeometry {
            width_dwords: 1,
            source_rows: 0,
            top_left_offset: STRIDE,
            variant: CellVariant::WithEdge,
        };
        assert_eq!(
            emit_cell_chain(&mut buf, g, &[]),
            Err(CellEmitError::ZeroDimension { is_width: false })
        );
    }

    #[test]
    fn delta_count_mismatch_rejected() {
        let mut buf = vec![0u8; STRIDE * 8];
        let g = CellEmitGeometry {
            width_dwords: 1,
            source_rows: 4,
            top_left_offset: STRIDE,
            variant: CellVariant::WithEdge,
        };
        // expects 4 deltas, supply 3.
        let deltas = [d(0, 0), d(0, 0), d(0, 0)];
        assert_eq!(
            emit_cell_chain(&mut buf, g, &deltas),
            Err(CellEmitError::DeltaCountMismatch {
                expected: 4,
                supplied: 3
            })
        );
    }

    #[test]
    fn withedge_single_column_chain_predictor_rolls() {
        // Variant B (with-edge): one row emitted per source row, one
        // row-stride advance. The cell starts one row below the strip
        // top so the row-0 predictor is in-buffer (we seed it).
        let mut buf = vec![0u8; STRIDE * 6];
        let top = STRIDE; // cell top-left at row 1
        // Seed the row-above-the-cell predictor (row 0) with a known
        // pattern.
        let seed = [0x10u8, 0x12, 0x14, 0x16];
        buf[0..4].copy_from_slice(&seed);

        let g = CellEmitGeometry {
            width_dwords: 1,
            source_rows: 4,
            top_left_offset: top,
            variant: CellVariant::WithEdge,
        };
        // Use primary deltas that never overflow (small positive) so we
        // can predict the average chain by hand.
        let deltas = [
            d(pack_predictor([0x02, 0x02, 0x02, 0x02]), 0),
            d(pack_predictor([0x04, 0x04, 0x04, 0x04]), 0),
            d(pack_predictor([0x06, 0x06, 0x06, 0x06]), 0),
            d(pack_predictor([0x08, 0x08, 0x08, 0x08]), 0),
        ];
        let stats = emit_cell_chain(&mut buf, g, &deltas).unwrap();
        assert_eq!(stats.source_rows, 4);
        assert_eq!(stats.emitted_rows, 4);
        assert_eq!(stats.continuation_bytes, 0);

        // Reproduce the chain with the scalar single-position API and
        // confirm the buffer matches row by row.
        let mut pred = pack_predictor(seed);
        for (i, delta) in deltas.iter().enumerate() {
            let em = emit_variant(CellVariant::WithEdge, pred, delta.primary, delta.secondary);
            let expected_row = em.rows.as_slice()[0];
            let off = top + i * STRIDE;
            let got = pack_predictor([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]);
            assert_eq!(got, expected_row, "row {i} mismatch");
            // Next row's predictor is this just-emitted row.
            pred = expected_row;
        }
    }

    #[test]
    fn top_of_strip_predictor_is_mid_range_seed() {
        // A cell at the very top of the strip (top_left_offset 0) reads
        // the fixture-arbitrated 0x40 boundary predictor for row 0
        // (r451; spec/07 §7.4 resolved).
        let mut buf = vec![0u8; STRIDE * 4];
        let g = CellEmitGeometry {
            width_dwords: 1,
            source_rows: 1,
            top_left_offset: 0,
            variant: CellVariant::WithEdge,
        };
        let delta = d(pack_predictor([0x20, 0x22, 0x24, 0x26]), 0);
        emit_cell_chain(&mut buf, g, &[delta]).unwrap();
        // Predictor was the 0x40 boundary seed:
        let seed = pack_predictor([TOP_OF_STRIP_PREDICTOR; 4]);
        let expected = emit_variant(CellVariant::WithEdge, seed, delta.primary, delta.secondary);
        let row0 = pack_predictor([buf[0], buf[1], buf[2], buf[3]]);
        assert_eq!(row0, expected.rows.as_slice()[0]);
    }

    #[test]
    fn plain_variant_writes_two_rows_per_source_row() {
        // Variant A (plain): each source row emits two identical output
        // rows, and the destination advances two row strides.
        let mut buf = vec![0u8; STRIDE * 8];
        let top = STRIDE;
        let g = CellEmitGeometry {
            width_dwords: 1,
            source_rows: 2,
            top_left_offset: top,
            variant: CellVariant::Plain,
        };
        let deltas = [
            d(pack_predictor([0x05, 0x05, 0x05, 0x05]), 0),
            d(pack_predictor([0x03, 0x03, 0x03, 0x03]), 0),
        ];
        let stats = emit_cell_chain(&mut buf, g, &deltas).unwrap();
        assert_eq!(stats.emitted_rows, 4);

        // Source row 0: predictor is top-of-something (row 0 above cell,
        // which is the zero region at `top - STRIDE = 0`). Plain stores
        // the same DWORD to row `top` and row `top + STRIDE`.
        let pred0 = predictor_dword(&buf, top);
        let em0 = apply_dyad_pair(pred0, deltas[0].primary, deltas[0].secondary);
        let DyadOutcome::Primary { pixels: p0 } = em0 else {
            panic!("expected Primary");
        };
        assert_eq!(
            unpack_pixels(p0),
            [buf[top], buf[top + 1], buf[top + 2], buf[top + 3]]
        );
        // Both stored rows are identical (vertical doubling).
        for k in 0..4 {
            assert_eq!(buf[top + k], buf[top + STRIDE + k]);
        }

        // Source row 1's destination is `top + 2 * STRIDE`; its
        // predictor `[edi - 0xb0]` is `top + STRIDE` — the *second*
        // emitted row of source row 0. Confirm the chain advanced two
        // strides.
        let pred1 = predictor_dword(&buf, top + 2 * STRIDE);
        let expected_pred1 = pack_predictor([
            buf[top + STRIDE],
            buf[top + STRIDE + 1],
            buf[top + STRIDE + 2],
            buf[top + STRIDE + 3],
        ]);
        assert_eq!(pred1, expected_pred1);
    }

    #[test]
    fn width_two_dwords_iterates_left_to_right() {
        // An 8-pixel-wide cell is two dyad-pair DWORDs per row (§2.4);
        // the deltas are consumed in row-major left-to-right order.
        let mut buf = vec![0u8; STRIDE * 4];
        let g = CellEmitGeometry {
            width_dwords: 2,
            source_rows: 1,
            top_left_offset: 0,
            variant: CellVariant::WithEdge,
        };
        let left = d(pack_predictor([0x10, 0x10, 0x10, 0x10]), 0);
        let right = d(pack_predictor([0x40, 0x40, 0x40, 0x40]), 0);
        emit_cell_chain(&mut buf, g, &[left, right]).unwrap();

        // Left dyad lands at bytes 0..4, right at bytes 4..8 (same row).
        let seed = pack_predictor([TOP_OF_STRIP_PREDICTOR; 4]);
        let exp_left = emit_variant(CellVariant::WithEdge, seed, left.primary, left.secondary);
        let exp_right = emit_variant(CellVariant::WithEdge, seed, right.primary, right.secondary);
        assert_eq!(
            pack_predictor([buf[0], buf[1], buf[2], buf[3]]),
            exp_left.rows.as_slice()[0]
        );
        assert_eq!(
            pack_predictor([buf[4], buf[5], buf[6], buf[7]]),
            exp_right.rows.as_slice()[0]
        );
    }

    #[test]
    fn continuation_byte_count_tracked() {
        // Force the continuation path: a predictor + primary that
        // overflows the high half, with a secondary that recovers.
        // predictor 0x7fff_0000 + primary 0x0001_0000 = 0x8000_0000
        // (bit 31 set → continuation); secondary 0x8000 clears the low
        // half's sign bit (per the reconstruct continuation test).
        let mut buf = vec![0u8; STRIDE * 4];
        // Seed the predictor row above the cell so the high half is set.
        let top = STRIDE;
        buf[0..4].copy_from_slice(&0x7fff_0000u32.to_le_bytes());
        let g = CellEmitGeometry {
            width_dwords: 1,
            source_rows: 1,
            top_left_offset: top,
            variant: CellVariant::Plain,
        };
        let delta = d(0x0001_0000, 0x8000);
        let stats = emit_cell_chain(&mut buf, g, &[delta]).unwrap();
        assert_eq!(stats.continuation_bytes, 1);
    }

    #[test]
    fn dyad_fault_aborts_chain() {
        // A continuation that still leaves the low half's sign bit set
        // faults (§6.4 error code 2). Position it at row 1, dword 0 of a
        // 1-wide, 2-row cell so the reported coordinates are exercised.
        let mut buf = vec![0u8; STRIDE * 6];
        let top = STRIDE;
        let g = CellEmitGeometry {
            width_dwords: 1,
            source_rows: 2,
            top_left_offset: top,
            variant: CellVariant::Plain,
        };
        // Row 0: a benign in-range delta (predictor is the zero region
        // above the cell at offset 0).
        let benign = d(pack_predictor([0x01, 0x01, 0x01, 0x01]), 0);
        // Row 1: force the fault. Its predictor is row 0's emitted
        // output; we make the primary overflow and the secondary fail.
        // predictor + primary high-half overflow, secondary leaves bit
        // 15 set → Fault.
        let faulting = d(0x8000_0000, 0x0001);
        let r = emit_cell_chain(&mut buf, g, &[benign, faulting]);
        assert!(matches!(
            r,
            Err(CellEmitError::DyadFault { row: 1, dword: 0 })
        ));
    }

    #[test]
    fn write_out_of_bounds_surfaced() {
        // A cell whose store region exceeds the buffer surfaces a typed
        // error rather than panicking.
        let mut buf = vec![0u8; 4]; // far too small
        let g = CellEmitGeometry {
            width_dwords: 1,
            source_rows: 1,
            top_left_offset: 0,
            variant: CellVariant::Plain, // two-row store: needs >= STRIDE+4
        };
        let delta = d(0, 0);
        let r = emit_cell_chain(&mut buf, g, &[delta]);
        assert!(matches!(
            r,
            Err(CellEmitError::WriteOutOfBounds { buffer_len: 4, .. })
        ));
    }

    #[test]
    fn four_by_four_intra_cell_end_to_end() {
        // A full 4×4 INTRA cell (variant B, one row per source row,
        // width 1 dyad-DWORD, 4 source rows) decoded end to end. Verify
        // the rolling predictor by replaying the scalar single-position
        // API and confirming byte-for-byte agreement across all 4 rows.
        let mut buf = vec![0u8; STRIDE * 6];
        let top = 2 * STRIDE;
        // Seed row above the cell.
        let above = [0x30u8, 0x31, 0x32, 0x33];
        buf[(top - STRIDE)..(top - STRIDE + 4)].copy_from_slice(&above);

        let deltas = [
            d(pack_predictor([0x02, 0x00, 0x04, 0x00]), 0),
            d(pack_predictor([0x01, 0x03, 0x05, 0x07]), 0),
            d(pack_predictor([0x00, 0x02, 0x00, 0x06]), 0),
            d(pack_predictor([0x08, 0x00, 0x02, 0x00]), 0),
        ];
        let g = CellEmitGeometry {
            width_dwords: 1,
            source_rows: 4,
            top_left_offset: top,
            variant: CellVariant::WithEdge,
        };
        emit_cell_chain(&mut buf, g, &deltas).unwrap();

        let mut pred = pack_predictor(above);
        for (i, delta) in deltas.iter().enumerate() {
            let em = emit_variant(CellVariant::WithEdge, pred, delta.primary, delta.secondary);
            let row = em.rows.as_slice()[0];
            let off = top + i * STRIDE;
            assert_eq!(
                pack_predictor([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]),
                row,
                "4x4 row {i}"
            );
            pred = row;
        }
    }
}

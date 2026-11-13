//! Indeo 3 fixture-arbitrated row-stream cell executor (r451).
//!
//! This module is the crate's first **real-bitstream-validated** cell
//! decoder: the per-cell mode-byte semantics recovered by arbitrating
//! the staged spec chapters against the real `IV32` fixture corpus
//! (`tests/data/iv32-160x120-all-intra/`, black-box reference decode
//! `expected.yuv`). Driving it over the first frame's luma stream
//! reproduces the reference decode **byte-exactly everywhere outside
//! the picture's re-coded detail region** across the first five base
//! columns — 10 404 of 12 480 pixels exact, every divergence inside
//! the rows the unstaged subdivision/overlay mechanism re-codes (see
//! `tests/indeo3_fixtures.rs`).
//!
//! ## The arbitrated semantics
//!
//! A cell decodes as a sequence of **row emissions** over its coded
//! rows (`spec/07 §2.1`'s per-row walk, settled at the value level by
//! the fixture):
//!
//! * A literal byte `0x00..=0xF7` is an **entry index** into the
//!   operative codebook block (`spec/04 §5.2` staging block, entry =
//!   the byte value): the one/two-byte softSIMD add of
//!   [`super::StagingImage::row_delta`] produces one output DWORD,
//!   and the delta **applies across the whole row** (each of the
//!   row's DWORDs takes `pred + delta` against its own predictor
//!   DWORD — a uniform per-position delta, `spec/04 §2.2`'s inner
//!   loop "advances to the next cell column").
//! * `0xFD` — null delta for **all remaining** coded rows: each
//!   repeats its predictor row (`spec/06 §4.2`'s "skip remaining
//!   rows", fixture-settled as predictor propagation, not
//!   zero-fill).
//! * `0xFF` / `0xFE` — null delta for one / two rows.
//! * `0xFB n` — null delta (`n & 0x1F` rows; bit 5 = edge-mark
//!   instead), the `spec/06 §4.4` counter runs at row granularity.
//! * `0xFC` — null delta for the rest of the cell **and** the next
//!   cell (the `spec/06 §4.6` carry).
//!
//! The predictor for a cell's first coded row is the pixel row above
//! the cell — the strip boundary constant `0x40` when the cell abuts
//! the top ([`super::TOP_OF_STRIP_PREDICTOR`], fixture-arbitrated).
//!
//! ## The doubled store
//!
//! A cell is either **plain** (each coded row = one output row) or
//! **doubled** (each coded row = two output rows: the second carries
//! the decoded row, the first the byte-wise average of the previous
//! and current decoded rows — the fixture's odd rows are byte-exact
//! averages, e.g. `18 = avg(20, 16)`; the first pair has no previous
//! row and repeats the decoded row). This is `spec/04 §2.2`'s
//! doubled-row variant family, settled at the value level.
//!
//! ## What stays open
//!
//! The **cell sequencing** — which cells exist and in what order —
//! rides the undocumented cell-geometry banks
//! (`IR32_32.DLL!0x100038f0`, `spec/04 §5.3`/`§7.9`): the fixture
//! shows the 160×120 luma plane decomposing into eight full-height
//! columns (24 px plain / 16 px doubled, alternating) consumed in
//! raster order with **no interleaved tree codes**, with the
//! right-hand columns further subdivided by a mechanism the staged
//! docs do not yet pin. The layout is therefore supplied by the
//! caller.

use super::staging::{RowDeltaOutcome, StagingImage};

/// One decoded cell row in pixel bytes (7-bit internal range).
pub type Row = Vec<u8>;

/// How a [`decode_cell_rows`] run finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellRowsRun {
    /// The decoded coded rows (each `width` bytes).
    pub rows: Vec<Row>,
    /// Bytes consumed from the stream.
    pub bytes_consumed: usize,
    /// `true` when the cell ended with the `0xFC` carry (the next
    /// cell is consumed as all-null with no byte reads).
    pub next_cell_skip: bool,
}

/// Errors the row-stream executor surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RowStreamError {
    /// The stream ran out mid-cell.
    StreamExhausted {
        /// Bytes consumed before exhaustion.
        consumed: usize,
    },
    /// An `0xFB` counter the binary rejects (`spec/06 §4.4`).
    FbCounterInvalid {
        /// The counter byte.
        counter: u8,
    },
    /// An escape with no arbitrated row-stream semantics yet
    /// (`0xF8`..`0xFA`).
    UnsupportedEscape {
        /// The escape byte.
        escape: u8,
    },
    /// A literal's continuation byte was an escape (`spec/06 §3.3`
    /// reads a plain entry index).
    ContinuationEscape {
        /// The offending byte.
        byte: u8,
    },
    /// The two-byte form still had bit 31 set (`spec/07 §2.3` step 3,
    /// error code 2) or a mid-row DWORD needed a continuation the
    /// first DWORD did not.
    RangeFault,
    /// `width` is zero or not a DWORD multiple, or `coded_rows` is
    /// zero.
    BadGeometry,
    /// The staging block index is out of range.
    BadBlock,
}

impl core::fmt::Display for RowStreamError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RowStreamError::StreamExhausted { consumed } => {
                write!(f, "row stream exhausted after {consumed} bytes")
            }
            RowStreamError::FbCounterInvalid { counter } => {
                write!(f, "spec/06 §4.4: invalid 0xFB counter {counter:#04x}")
            }
            RowStreamError::UnsupportedEscape { escape } => {
                write!(f, "row stream: escape {escape:#04x} not arbitrated yet")
            }
            RowStreamError::ContinuationEscape { byte } => {
                write!(
                    f,
                    "spec/06 §3.3: continuation byte {byte:#04x} is an escape"
                )
            }
            RowStreamError::RangeFault => f.write_str("spec/07 §2.3: dyad range fault"),
            RowStreamError::BadGeometry => f.write_str("row stream: bad cell geometry"),
            RowStreamError::BadBlock => f.write_str("row stream: staging block out of range"),
        }
    }
}

impl std::error::Error for RowStreamError {}

fn pack(row: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([row[off], row[off + 1], row[off + 2], row[off + 3]])
}

fn unpack_into(row: &mut [u8], off: usize, v: u32) {
    row[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// Decode one cell's coded rows from a mode-byte stream
/// (fixture-arbitrated; see the module docs).
///
/// * `staging` / `block` — the operative codebook block (the
///   `cb_offset`-biased staging window, `spec/04 §6`).
/// * `boundary` — the pixel row directly above the cell (`width`
///   bytes; the strip boundary constant for top cells).
/// * `stream` — the mode-byte stream at the cell's first byte.
/// * `width` — cell width in pixels (a DWORD multiple).
/// * `coded_rows` — the number of coded rows (the output height for
///   a plain cell, half of it for a doubled cell).
pub fn decode_cell_rows(
    staging: &StagingImage,
    block: usize,
    boundary: &[u8],
    stream: &[u8],
    width: usize,
    coded_rows: usize,
) -> Result<CellRowsRun, RowStreamError> {
    if width == 0 || !width.is_multiple_of(4) || boundary.len() != width || coded_rows == 0 {
        return Err(RowStreamError::BadGeometry);
    }
    let mut rows: Vec<Row> = Vec::with_capacity(coded_rows);
    let mut cursor = 0usize;
    let mut next_cell_skip = false;

    let read = |cursor: &mut usize| -> Result<u8, RowStreamError> {
        let b = *stream
            .get(*cursor)
            .ok_or(RowStreamError::StreamExhausted { consumed: *cursor })?;
        *cursor += 1;
        Ok(b)
    };

    while rows.len() < coded_rows {
        let pred_row: &[u8] = rows.last().map_or(boundary, |r| &r[..]);
        let b = read(&mut cursor)?;
        match b {
            0xFD => {
                while rows.len() < coded_rows {
                    let r = rows.last().map_or(boundary, |r| &r[..]).to_vec();
                    rows.push(r);
                }
                break;
            }
            0xFC => {
                while rows.len() < coded_rows {
                    let r = rows.last().map_or(boundary, |r| &r[..]).to_vec();
                    rows.push(r);
                }
                next_cell_skip = true;
                break;
            }
            0xFF | 0xFE => {
                let n = if b == 0xFF { 1 } else { 2 };
                for _ in 0..n {
                    if rows.len() >= coded_rows {
                        break;
                    }
                    let r = rows.last().map_or(boundary, |r| &r[..]).to_vec();
                    rows.push(r);
                }
            }
            0xFB => {
                let counter = read(&mut cursor)?;
                let n = counter & 0x1F;
                if n == 0 || counter & 0xC0 != 0 {
                    return Err(RowStreamError::FbCounterInvalid { counter });
                }
                let mark = counter & 0x20 != 0;
                for _ in 0..n {
                    if rows.len() >= coded_rows {
                        break;
                    }
                    let mut r = rows.last().map_or(boundary, |r| &r[..]).to_vec();
                    if mark {
                        for px in &mut r {
                            *px |= 0x80;
                        }
                    }
                    rows.push(r);
                }
            }
            0xF8..=0xFA => return Err(RowStreamError::UnsupportedEscape { escape: b }),
            entry => {
                // Literal: the one/two-byte row delta, replicated
                // across the row's DWORD positions.
                let first = staging
                    .row_delta(block, entry, None, pack(pred_row, 0))
                    .ok_or(RowStreamError::BadBlock)?;
                let continuation = match first {
                    RowDeltaOutcome::NeedsContinuation => {
                        let c = read(&mut cursor)?;
                        if c >= 0xF8 {
                            return Err(RowStreamError::ContinuationEscape { byte: c });
                        }
                        Some(c)
                    }
                    RowDeltaOutcome::RangeFault => return Err(RowStreamError::RangeFault),
                    RowDeltaOutcome::Complete { .. } => None,
                };
                let mut row = vec![0u8; width];
                for off in (0..width).step_by(4) {
                    let pred = pack(pred_row, off);
                    match staging.row_delta(block, entry, continuation, pred) {
                        Some(RowDeltaOutcome::Complete { value, .. }) => {
                            unpack_into(&mut row, off, value)
                        }
                        Some(RowDeltaOutcome::NeedsContinuation)
                        | Some(RowDeltaOutcome::RangeFault) => {
                            return Err(RowStreamError::RangeFault);
                        }
                        None => return Err(RowStreamError::BadBlock),
                    }
                }
                rows.push(row);
            }
        }
    }

    Ok(CellRowsRun {
        rows,
        bytes_consumed: cursor,
        next_cell_skip,
    })
}

/// Expand a doubled cell's coded rows into output rows (the
/// fixture-arbitrated doubled store; see the module docs): coded row
/// `k` produces output rows `(avg(prev, cur), cur)`, with the first
/// pair `(cur, cur)`.
pub fn expand_doubled_rows(coded: &[Row]) -> Vec<Row> {
    let mut out = Vec::with_capacity(coded.len() * 2);
    let mut prev: Option<&Row> = None;
    for cur in coded {
        match prev {
            None => out.push(cur.clone()),
            Some(p) => out.push(
                p.iter()
                    .zip(cur.iter())
                    .map(|(&a, &b)| ((u16::from(a) + u16::from(b)) / 2) as u8)
                    .collect(),
            ),
        }
        out.push(cur.clone());
        prev = Some(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::CodebookSeedArea;

    fn staging() -> StagingImage {
        StagingImage::build(&CodebookSeedArea::load())
    }

    #[test]
    fn fixture_first_cell_rows() {
        // The real fixture's first cell stream: [6c 6c] [d3] [FD] over
        // a 24-px row, 16 coded rows, boundary 0x40.
        let img = staging();
        let boundary = vec![0x40u8; 24];
        let run = decode_cell_rows(&img, 0, &boundary, &[0x6c, 0x6c, 0xd3, 0xfd], 24, 16)
            .expect("cell decodes");
        assert_eq!(run.bytes_consumed, 4);
        assert!(!run.next_cell_skip);
        assert_eq!(run.rows.len(), 16);
        assert!(run.rows[0].iter().all(|&b| b == 0x0a));
        for r in &run.rows[1..] {
            assert!(r.iter().all(|&b| b == 0x08));
        }
    }

    #[test]
    fn doubled_expansion_averages_odd_rows() {
        // The fixture's 16-px doubled columns: coded [10s, 8s, 8s...]
        // expands to [20, 20, 18, 16, 16, 16, ...] output (per the
        // (avg, cur) pair store, upshifted).
        let coded = vec![vec![10u8; 4], vec![8u8; 4], vec![8u8; 4]];
        let out = expand_doubled_rows(&coded);
        assert_eq!(out.len(), 6);
        assert!(out[0].iter().all(|&b| b == 10));
        assert!(out[1].iter().all(|&b| b == 10));
        assert!(out[2].iter().all(|&b| b == 9)); // avg(10, 8)
        assert!(out[3].iter().all(|&b| b == 8));
        assert!(out[4].iter().all(|&b| b == 8));
        assert!(out[5].iter().all(|&b| b == 8));
    }

    #[test]
    fn fb_and_fc_row_runs() {
        let img = staging();
        let boundary = vec![0x40u8; 8];
        // FB(3): three null rows, then a literal, then FC ends the
        // cell with the carry.
        let run = decode_cell_rows(&img, 0, &boundary, &[0xFB, 0x03, 0xFC], 8, 6).expect("decode");
        assert_eq!(run.rows.len(), 6);
        assert!(run.next_cell_skip);
        assert!(run.rows.iter().all(|r| r.iter().all(|&b| b == 0x40)));
        // Invalid counters reject.
        assert_eq!(
            decode_cell_rows(&img, 0, &boundary, &[0xFB, 0x20], 8, 4).unwrap_err(),
            RowStreamError::FbCounterInvalid { counter: 0x20 }
        );
    }

    #[test]
    fn geometry_and_stream_faults_are_typed() {
        let img = staging();
        assert_eq!(
            decode_cell_rows(&img, 0, &[0x40; 6], &[0x00], 6, 1).unwrap_err(),
            RowStreamError::BadGeometry
        );
        assert_eq!(
            decode_cell_rows(&img, 0, &[0x40; 8], &[], 8, 1).unwrap_err(),
            RowStreamError::StreamExhausted { consumed: 0 }
        );
        assert_eq!(
            decode_cell_rows(&img, 24, &[0x40; 8], &[0x00], 8, 1).unwrap_err(),
            RowStreamError::BadBlock
        );
    }
}

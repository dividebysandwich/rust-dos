//! Indeo 3 macroblock-layer binary-tree walk (per-plane).
//!
//! Spec source: `docs/video/indeo/indeo3/spec/03-macroblock-layer.md`.
//!
//! Round 3 lands the binary-tree decomposition that lives inside a
//! plane's bitstream payload (the bytes that begin at the
//! [`bitstream_offset`](super::picture_layer::PlanePrelude::bitstream_offset)
//! computed by `spec/02`). It produces a typed [`CellTree`]: a list
//! of leaf cells, each carrying its strip-relative geometry
//! `(x, y, w, h)` and the decision the leaf encodes (INTRA→VQ,
//! INTER, VQ_NULL copy/skip, VQ_DATA).
//!
//! What this round covers, mapped to the spec/03 sections:
//!
//! * §2.1 — the MSB-first bit reader (bit 7 of byte 0 first).
//! * §2.2 — the four 2-bit node codes (`00` H_SPLIT, `01` V_SPLIT,
//!   `10` INTRA/VQ_NULL leaf, `11` INTER/VQ_DATA leaf), with the
//!   leaf-side semantic switching between the MC_TREE and VQ_TREE.
//! * §3 — the MC_TREE walk over a single root cell whose size
//!   matches the whole plane (§3.1), the H_SPLIT / V_SPLIT halving
//!   (§3.2), the INTRA leaf → VQ_TREE transition on the same cell
//!   (§3.3), and the INTER leaf one-byte MV-index read (§3.4).
//! * §4 — the VQ_TREE walk: H_SPLIT / V_SPLIT halving, the VQ_NULL
//!   leaf plus its sub-code **prefix code** (`00` copy, `01` skip,
//!   `1` unpacker dispatch — spec/04 §4 / spec/06 §1.1, superseding
//!   §4.1's fixed-2-bit fault reading), and the VQ_DATA leaf
//!   one-byte codebook-index read (§4.1).
//!
//! What this round deliberately does **not** do (the spec/03
//! chapter boundary, §7):
//!
//! * No VQ codebook materialisation, dyad / quad delta tables, or
//!   RLE escape decoding — those are `spec/04-vq-codebooks.md` and
//!   `spec/06-entropy.md`. We stop at the leaf-byte fetch, recording
//!   only the raw index byte.
//! * No motion-compensation pixel copy — `spec/05`. The INTER leaf
//!   records only its raw MV-index byte.
//! * No pixel reconstruction or edge fix-up — `spec/07`. The strip
//!   geometry produced here is the tree's logical leaf layout, not
//!   the strip-context pixel-buffer state machine.
//!
//! The leaf geometry follows the spec/03 §2.4 / §3.1 / §3.2
//! "split in half" semantics: the root cell is the plane, an
//! H_SPLIT halves a cell's height (top child visited first), and a
//! V_SPLIT halves a cell's width (left child visited first). The
//! per-binary cell-position lookup tables (`bank+0x100..+0x2ff`,
//! spec/03 §4.2) that the original decoder uses to recover the same
//! geometry from its packed `(edi, esi)` path register are an open
//! table-extraction item (spec/03 §6) and are not needed to produce
//! the leaf layout: the halving arithmetic gives the identical
//! result directly.

use super::header::FrameFlags;
use super::picture_layer::PlanePrelude;

/// Spec/02 §4.1 — luma strip width in samples (`0xa0`).
pub const LUMA_STRIP_WIDTH: u32 = 160;
/// Spec/02 §4.1 — chroma strip width in samples (`0x28`); the luma
/// width divided by the 4:1 chroma subsampling ratio.
pub const CHROMA_STRIP_WIDTH: u32 = 40;

/// Spec/03 §2.2 — the four 2-bit node codes as decoded from the
/// bitstream, before the per-tree leaf semantics are applied.
///
/// The split codes are tree-agnostic; the two leaf codes carry a
/// different meaning in the MC_TREE (INTRA / INTER, see [`Cell`])
/// than in the VQ_TREE (VQ_NULL / VQ_DATA, see [`VqLeaf`]). This
/// enum is the raw decode of the two consumed bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeCode {
    /// `00` — split the current cell horizontally (halve height).
    HSplit,
    /// `01` — split the current cell vertically (halve width).
    VSplit,
    /// `10` — leaf: INTRA in the MC_TREE, VQ_NULL in the VQ_TREE.
    LeafLow,
    /// `11` — leaf: INTER in the MC_TREE, VQ_DATA in the VQ_TREE.
    LeafHigh,
}

impl NodeCode {
    /// True for the two splitter codes (`00`, `01`).
    pub fn is_split(self) -> bool {
        matches!(self, NodeCode::HSplit | NodeCode::VSplit)
    }
}

/// Spec/04 §4 — the sub-action of a VQ_NULL leaf, given by the
/// **prefix code** read immediately after the VQ_NULL node bits.
///
/// The sub-code is *not* a fixed 2-bit field: the reader at
/// `IR32_32.DLL!0x100069d2..0x100069f2` consumes one bit, and only
/// when that bit is 0 consumes a second (`spec/06 §1.1`: "the
/// effective code lengths are 1 and 2 bits respectively — a prefix
/// code, not a fixed-length code"):
///
/// * `1` (1 bit) → dispatch into the per-byte unpacker at
///   `0x10006bac` — the "VQ-data without leaf-byte" hybrid
///   (`spec/04 §4` first bullet, `spec/06 §5.2`).
/// * `0 0` (2 bits) → copy the upper-neighbour row
///   (`0x100069f4`).
/// * `0 1` (2 bits) → mark the cell as a boundary / edge cell
///   (`0x10006a2f`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VqNull {
    /// Sub-code `00` — copy this cell's pixels from the referenced
    /// (previous-frame) cell.
    Copy,
    /// Sub-code `01` — mark the cell skipped; leave its pixels at
    /// the predictor value.
    Skip,
    /// Sub-code `1` (one bit) — dispatch into the per-byte
    /// unpacker: the next bitstream byte is a mode byte read
    /// directly, with no intervening codebook-index byte
    /// (`spec/04 §4` / `§7.3`, `spec/06 §5.2`). The mode-byte
    /// stream consumption itself is the reconstruction stage's
    /// job (the tree walk records the leaf only, exactly as it
    /// records — without consuming — a VQ_DATA cell's mode
    /// bytes).
    Unpacker,
}

/// Spec/03 §3 / §4 — a fully resolved leaf cell of the binary tree.
///
/// Each variant records the cell geometry `(x, y, w, h)` in plane
/// samples plus the decision the leaf encodes. INTRA cells carry
/// their VQ sub-tree's leaves inline (the §3.3 MC_TREE → VQ_TREE
/// transition keeps the same physical cell, then descends into a
/// VQ sub-tree whose own leaves describe sub-cells).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    /// Spec/03 §3.4 — INTER leaf. The cell is reconstructed by
    /// motion compensation; `mv_index` is the raw byte the leaf
    /// reads (§3.4) that indexes the per-plane packed-MV table.
    /// The packed-MV decode + pixel copy are spec/05.
    Inter {
        /// Cell top-left x in plane samples.
        x: u32,
        /// Cell top-left y in plane samples.
        y: u32,
        /// Cell width in plane samples.
        w: u32,
        /// Cell height in plane samples.
        h: u32,
        /// Raw MV-index byte (spec/03 §3.4); index into the
        /// per-plane packed-MV table (spec/02 §3.3, spec/05).
        mv_index: u8,
    },
    /// Spec/03 §3.3 — INTRA leaf: the cell is reconstructed from a
    /// VQ sub-tree. The MC_TREE leaf promotes the same physical
    /// cell to a VQ_TREE walk (§3.3); `vq_leaves` holds that
    /// sub-tree's resolved leaves.
    Intra {
        /// Cell top-left x in plane samples.
        x: u32,
        /// Cell top-left y in plane samples.
        y: u32,
        /// Cell width in plane samples.
        w: u32,
        /// Cell height in plane samples.
        h: u32,
        /// The VQ sub-tree leaves for this INTRA cell.
        vq_leaves: Vec<VqCell>,
    },
}

impl Cell {
    /// The cell geometry `(x, y, w, h)` regardless of variant.
    pub fn geometry(&self) -> (u32, u32, u32, u32) {
        match *self {
            Cell::Inter { x, y, w, h, .. } | Cell::Intra { x, y, w, h, .. } => (x, y, w, h),
        }
    }
}

/// Spec/03 §4 — a fully resolved VQ_TREE leaf (a sub-cell of an
/// INTRA cell).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VqLeaf {
    /// Spec/03 §4.1 — VQ_NULL leaf; either a copy from the
    /// reference cell or a skip.
    Null(VqNull),
    /// Spec/03 §4.1 — VQ_DATA leaf; `codebook_index` is the raw
    /// byte the leaf reads (§4.1), index into the per-plane
    /// codebook table. The codebook materialisation + dyad / quad
    /// delta emission are spec/04 / spec/06.
    Data {
        /// Raw codebook-index byte (spec/03 §4.1).
        codebook_index: u8,
    },
}

/// Spec/03 §4 — a VQ sub-cell: geometry plus the resolved VQ leaf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VqCell {
    /// Sub-cell top-left x in plane samples.
    pub x: u32,
    /// Sub-cell top-left y in plane samples.
    pub y: u32,
    /// Sub-cell width in plane samples.
    pub w: u32,
    /// Sub-cell height in plane samples.
    pub h: u32,
    /// The resolved VQ leaf for this sub-cell.
    pub leaf: VqLeaf,
    /// The absolute input-buffer byte offset at which this cell's
    /// **mode-byte stream** begins, for the data-bearing leaves
    /// (`spec/06 §5.1` step 4: the per-cell unpacker reads from
    /// `[ebp]`, which is the next-unloaded-byte cursor):
    ///
    /// * [`VqLeaf::Data`] — the byte after the codebook-index byte;
    /// * [`VqLeaf::Null`] with [`VqNull::Unpacker`] — the cursor at
    ///   the sub-code bit (`spec/06 §5.2`: no intervening leaf byte);
    /// * `None` for the copy / skip VQ_NULL leaves (no byte payload).
    ///
    /// The structural tree walk does **not** consume the mode-byte
    /// stream (its literal-dyad length is arena-value-dependent), so
    /// this anchor is byte-exact for the *first* data-bearing leaf of
    /// a plane and a lower bound for later ones — the seam the
    /// interleaved reconstruction walk will thread once the
    /// `spec/04 §7.1` arena extraction lands.
    pub data_cursor: Option<usize>,
}

/// Spec/03 §3 — the resolved binary-tree decomposition of one
/// plane: the list of leaf cells in tree-walk (top-then-bottom,
/// left-then-right) order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellTree {
    /// The root cell width in plane samples (= plane width per
    /// spec/03 §3.1).
    pub plane_width: u32,
    /// The root cell height in plane samples (= plane height per
    /// spec/03 §3.1).
    pub plane_height: u32,
    /// Leaf cells in tree-walk order. Each leaf is either an INTER
    /// cell or an INTRA cell (whose VQ sub-tree leaves are nested).
    pub cells: Vec<Cell>,
}

impl CellTree {
    /// Total count of leaf cells (INTRA + INTER), not counting the
    /// nested VQ sub-cells.
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }
}

/// Errors raised while walking the spec/03 binary tree.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MacroblockError {
    /// Spec/03 §2.1 — the bit reader ran out of bytes mid-node.
    /// The plane payload was truncated before the tree completed.
    BitstreamTruncated {
        /// Byte offset (from the input-buffer start) at which the
        /// reader needed another byte but none remained.
        offset: usize,
    },
    /// Spec/03 §3.4 / §4.1 — a leaf needed one more whole byte
    /// (the MV index or codebook index) but the buffer was
    /// exhausted.
    LeafByteTruncated {
        /// Byte offset (from the input-buffer start) the leaf-byte
        /// read needed.
        offset: usize,
    },
    /// The plane's [`bitstream_offset`](PlanePrelude::bitstream_offset)
    /// lies past the end of the supplied input buffer.
    BitstreamOffsetOutOfRange {
        /// The plane prelude's `bitstream_offset`.
        bitstream_offset: usize,
        /// Length of the input buffer the caller supplied.
        buffer_len: usize,
    },
    /// A split would have produced a zero-width or zero-height
    /// child — i.e. the encoder emitted a split below the smallest
    /// representable cell. Spec/03 §2.4 notes the decoder does not
    /// enforce a minimum at the tree level, but a child of size 0
    /// is not a valid cell, so we surface it rather than emit a
    /// degenerate leaf.
    DegenerateSplit {
        /// Whether the offending split was horizontal (height) or
        /// vertical (width).
        horizontal: bool,
        /// The cell dimension being halved (height for H_SPLIT,
        /// width for V_SPLIT).
        dimension: u32,
    },
}

impl core::fmt::Display for MacroblockError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            MacroblockError::BitstreamTruncated { offset } => {
                write!(f, "binary-tree bitstream truncated at byte {offset}")
            }
            MacroblockError::LeafByteTruncated { offset } => {
                write!(f, "leaf-byte read truncated at byte {offset}")
            }
            MacroblockError::BitstreamOffsetOutOfRange {
                bitstream_offset,
                buffer_len,
            } => write!(
                f,
                "plane bitstream_offset {bitstream_offset} past {buffer_len}-byte input buffer"
            ),
            MacroblockError::DegenerateSplit {
                horizontal,
                dimension,
            } => write!(
                f,
                "{} split of a {dimension}-sample cell would yield a zero child",
                if horizontal { "horizontal" } else { "vertical" }
            ),
        }
    }
}

impl std::error::Error for MacroblockError {}

/// Spec/03 §2.1 — the MSB-first sentinel-bit reader over a plane
/// payload.
///
/// This models the original decoder's two-cursor scheme exactly
/// (spec/03 §2.1 / §3.4 / §6 item 7):
///
/// * The bit buffer `bl` holds the byte currently being shifted
///   out bit-by-bit (MSB first). It is refilled from `[ebp]` only
///   when exhausted.
/// * The byte cursor `ebp` (here [`Self::next_byte`]) points at the
///   *next byte not yet loaded into the bit buffer*. Crucially, a
///   leaf byte (the MV index / codebook index of §3.4 / §4.1) is
///   read from `[ebp]` — i.e. from `next_byte`, the byte *after*
///   the one currently being shifted in the bit buffer — and then
///   `ebp` advances. The bit buffer is **not** refilled by the
///   leaf-byte read, so the bit reader's sentinel state survives
///   across it (spec/03 §6 item 7).
struct BitReader<'a> {
    data: &'a [u8],
    /// `ebp` — index (relative to `data`) of the next byte not yet
    /// loaded into the bit buffer. A leaf-byte read consumes this
    /// byte and advances.
    next_byte: usize,
    /// Bits remaining in the current bit buffer, MSB-first. `None`
    /// means the buffer is empty and must be refilled from
    /// `next_byte` before the next bit read.
    bit_buf: Option<(u8, u8)>,
    /// `data[0]`'s offset relative to the whole input buffer, used
    /// to report absolute byte positions in error variants.
    abs_base: usize,
}

impl<'a> BitReader<'a> {
    /// `data` is the slice starting at the plane's
    /// `bitstream_offset`; `abs_base` is that offset relative to
    /// the input buffer (for error byte positions).
    fn new(data: &'a [u8], abs_base: usize) -> Self {
        BitReader {
            data,
            next_byte: 0,
            bit_buf: None,
            abs_base,
        }
    }

    /// Read one bit (MSB-first). Returns the bit value (0 or 1).
    /// Refills the bit buffer from `next_byte` when empty.
    fn read_bit(&mut self) -> Result<u8, MacroblockError> {
        let (byte, consumed) = match self.bit_buf {
            Some(state) => state,
            None => {
                // Refill: load `[ebp]` and advance `ebp`.
                if self.next_byte >= self.data.len() {
                    return Err(MacroblockError::BitstreamTruncated {
                        offset: self.abs_base + self.next_byte,
                    });
                }
                let b = self.data[self.next_byte];
                self.next_byte += 1;
                (b, 0)
            }
        };
        // consumed = number of MSB bits already taken (0..8).
        let bit = (byte >> (7 - consumed)) & 1;
        let consumed = consumed + 1;
        self.bit_buf = if consumed == 8 {
            None
        } else {
            Some((byte, consumed))
        };
        Ok(bit)
    }

    /// Read one whole byte from the shared `ebp` cursor (spec/03
    /// §3.4 / §4.1 leaf-byte read). Reads `[ebp]` and advances
    /// `ebp`, leaving the bit buffer state untouched (spec/03 §6
    /// item 7: the bit reader's sentinel state survives a
    /// leaf-byte read).
    /// The absolute input-buffer offset of the next byte the shared
    /// cursor `ebp` would deliver (`abs_base + next_byte`) — the
    /// position a byte-level consumer (leaf byte / mode-byte stream)
    /// reads from next.
    fn abs_cursor(&self) -> usize {
        self.abs_base + self.next_byte
    }

    fn read_leaf_byte(&mut self) -> Result<u8, MacroblockError> {
        if self.next_byte >= self.data.len() {
            return Err(MacroblockError::LeafByteTruncated {
                offset: self.abs_base + self.next_byte,
            });
        }
        let byte = self.data[self.next_byte];
        self.next_byte += 1;
        Ok(byte)
    }

    /// Spec/03 §2.2 — decode one 2-bit node code (first bit then
    /// second bit, MSB-first).
    fn read_node(&mut self) -> Result<NodeCode, MacroblockError> {
        let first = self.read_bit()?;
        let second = self.read_bit()?;
        Ok(match (first, second) {
            (0, 0) => NodeCode::HSplit,
            (0, 1) => NodeCode::VSplit,
            (1, 0) => NodeCode::LeafLow,
            (1, 1) => NodeCode::LeafHigh,
            _ => unreachable!("read_bit yields only 0 or 1"),
        })
    }
}

/// Decode the spec/03 binary-tree decomposition of a single plane.
///
/// `prelude` is the [`PlanePrelude`] the spec/02 picture layer
/// produced for this plane; its
/// [`bitstream_offset`](PlanePrelude::bitstream_offset) marks where
/// the binary-tree payload begins in `input`. `plane_width` /
/// `plane_height` are the plane dimensions in samples (= picture
/// width × height for luma, ÷4 each for chroma — spec/02 §4).
/// `is_chroma` selects the strip-width constant the V_SPLIT
/// classification uses (informational; the tree-walk geometry does
/// not depend on it, per spec/02 §4.3 — the decoder follows the
/// in-band codes, not the width).
///
/// `flags` is reserved for future MV-context use; spec/03's
/// tree-walk itself does not consult it (the half-pel scaling is a
/// spec/02 concern). It is accepted now so the signature is stable
/// for spec/05.
pub fn decode_plane_tree(
    input: &[u8],
    prelude: &PlanePrelude,
    plane_width: u32,
    plane_height: u32,
    _is_chroma: bool,
    _flags: FrameFlags,
) -> Result<CellTree, MacroblockError> {
    let base = prelude.bitstream_offset;
    if base > input.len() {
        return Err(MacroblockError::BitstreamOffsetOutOfRange {
            bitstream_offset: base,
            buffer_len: input.len(),
        });
    }
    let payload = &input[base..];
    let mut reader = BitReader::new(payload, base);

    let mut cells = Vec::new();
    walk_mc_tree(&mut reader, 0, 0, plane_width, plane_height, &mut cells)?;

    Ok(CellTree {
        plane_width,
        plane_height,
        cells,
    })
}

/// Spec/03 §3 — walk the MC_TREE over a cell, recursing into the
/// two children of each split and emitting a [`Cell`] at each leaf.
fn walk_mc_tree(
    reader: &mut BitReader<'_>,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    out: &mut Vec<Cell>,
) -> Result<(), MacroblockError> {
    match reader.read_node()? {
        NodeCode::HSplit => {
            // §3.2 — halve height; top child first, bottom second.
            let (top_h, bot_h) = halve(h, true)?;
            walk_mc_tree(reader, x, y, w, top_h, out)?;
            walk_mc_tree(reader, x, y + top_h, w, bot_h, out)?;
            Ok(())
        }
        NodeCode::VSplit => {
            // §3.2 — halve width; left child first, right second.
            let (left_w, right_w) = halve(w, false)?;
            walk_mc_tree(reader, x, y, left_w, h, out)?;
            walk_mc_tree(reader, x + left_w, y, right_w, h, out)?;
            Ok(())
        }
        NodeCode::LeafLow => {
            // §3.3 — INTRA leaf: promote this cell to a VQ_TREE
            // walk over the same physical cell.
            let mut vq_leaves = Vec::new();
            walk_vq_tree(reader, x, y, w, h, &mut vq_leaves)?;
            out.push(Cell::Intra {
                x,
                y,
                w,
                h,
                vq_leaves,
            });
            Ok(())
        }
        NodeCode::LeafHigh => {
            // §3.4 — INTER leaf: read one byte = MV index.
            let mv_index = reader.read_leaf_byte()?;
            out.push(Cell::Inter {
                x,
                y,
                w,
                h,
                mv_index,
            });
            Ok(())
        }
    }
}

/// Spec/03 §4 — walk the VQ_TREE over a cell, recursing into the
/// two children of each split and emitting a [`VqCell`] at each
/// leaf.
fn walk_vq_tree(
    reader: &mut BitReader<'_>,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    out: &mut Vec<VqCell>,
) -> Result<(), MacroblockError> {
    match reader.read_node()? {
        NodeCode::HSplit => {
            let (top_h, bot_h) = halve(h, true)?;
            walk_vq_tree(reader, x, y, w, top_h, out)?;
            walk_vq_tree(reader, x, y + top_h, w, bot_h, out)?;
            Ok(())
        }
        NodeCode::VSplit => {
            let (left_w, right_w) = halve(w, false)?;
            walk_vq_tree(reader, x, y, left_w, h, out)?;
            walk_vq_tree(reader, x + left_w, y, right_w, h, out)?;
            Ok(())
        }
        NodeCode::LeafLow => {
            // spec/04 §4 — VQ_NULL leaf: read the sub-code PREFIX
            // code (`spec/06 §1.1`). The first sub-bit
            // (`0x100069d2`) selects the per-byte-unpacker
            // dispatch when 1 (a one-bit code); only when it is 0
            // is a second sub-bit (`0x100069f2`) consumed to
            // select copy-upper (0) vs mark-boundary (1).
            let null = if reader.read_bit()? == 1 {
                VqNull::Unpacker
            } else if reader.read_bit()? == 0 {
                VqNull::Copy
            } else {
                VqNull::Skip
            };
            // spec/06 §5.2 — the unpacker sub-code's mode bytes begin
            // at the byte cursor, with no intervening leaf byte.
            let data_cursor = match null {
                VqNull::Unpacker => Some(reader.abs_cursor()),
                VqNull::Copy | VqNull::Skip => None,
            };
            out.push(VqCell {
                x,
                y,
                w,
                h,
                leaf: VqLeaf::Null(null),
                data_cursor,
            });
            Ok(())
        }
        NodeCode::LeafHigh => {
            // §4.1 — VQ_DATA leaf: read one byte = codebook index.
            let codebook_index = reader.read_leaf_byte()?;
            // spec/06 §5.1 — the per-cell unpacker's mode bytes begin
            // at the byte after the codebook-index byte.
            let data_cursor = Some(reader.abs_cursor());
            out.push(VqCell {
                x,
                y,
                w,
                h,
                leaf: VqLeaf::Data { codebook_index },
                data_cursor,
            });
            Ok(())
        }
    }
}

/// Spec/03 §2.4 / §3.2 — halve a cell dimension. Returns the
/// (first, second) child sizes. The first (top / left) child takes
/// the ceiling half so that a + b = original; the original decoder's
/// halving is mediated by lookup tables (spec/03 §4.2) whose
/// exact split point for odd sizes is an open item (spec/03 §6),
/// but the spec/02 §4 strip widths are all even multiples of the
/// 4×4 block grid, so even halving is the well-formed case. An odd
/// dimension is split ceil-then-floor.
fn halve(dim: u32, horizontal: bool) -> Result<(u32, u32), MacroblockError> {
    if dim < 2 {
        return Err(MacroblockError::DegenerateSplit {
            horizontal,
            dimension: dim,
        });
    }
    let first = dim.div_ceil(2);
    let second = dim - first;
    Ok((first, second))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a [`PlanePrelude`] whose bitstream payload sits at the
    /// start of the test buffer (`bitstream_offset == 0`).
    fn prelude_at(offset: usize) -> PlanePrelude {
        PlanePrelude {
            num_vectors: 0,
            motion_vectors: Vec::new(),
            bitstream_offset: offset,
        }
    }

    /// Pack a sequence of 2-bit node codes (MSB-first) into bytes.
    /// `nodes` are the raw 2-bit values 0b00..0b11.
    fn pack_bits(bits: &[u8]) -> Vec<u8> {
        // bits is a flat MSB-first bit sequence.
        let mut out = Vec::new();
        let mut acc = 0u8;
        let mut n = 0u8;
        for &b in bits {
            acc = (acc << 1) | (b & 1);
            n += 1;
            if n == 8 {
                out.push(acc);
                acc = 0;
                n = 0;
            }
        }
        if n > 0 {
            acc <<= 8 - n;
            out.push(acc);
        }
        out
    }

    /// Expand a 2-bit code into its two MSB-first bits.
    fn code_bits(code: u8) -> [u8; 2] {
        [(code >> 1) & 1, code & 1]
    }

    /// Flatten a list of 2-bit codes into a bit vector.
    fn codes_to_bits(codes: &[u8]) -> Vec<u8> {
        let mut bits = Vec::new();
        for &c in codes {
            bits.extend_from_slice(&code_bits(c));
        }
        bits
    }

    #[test]
    fn strip_width_constants_match_spec_4_1() {
        assert_eq!(LUMA_STRIP_WIDTH, 160);
        assert_eq!(CHROMA_STRIP_WIDTH, 40);
    }

    #[test]
    fn node_codes_decode_msb_first() {
        // 00 01 10 11 packed into one byte = 0b00_01_10_11 = 0x1b.
        let buf = pack_bits(&codes_to_bits(&[0b00, 0b01, 0b10, 0b11]));
        assert_eq!(buf, vec![0x1b]);
        let mut r = BitReader::new(&buf, 0);
        assert_eq!(r.read_node().unwrap(), NodeCode::HSplit);
        assert_eq!(r.read_node().unwrap(), NodeCode::VSplit);
        assert_eq!(r.read_node().unwrap(), NodeCode::LeafLow);
        assert_eq!(r.read_node().unwrap(), NodeCode::LeafHigh);
    }

    #[test]
    fn single_intra_root_with_one_vq_data_leaf() {
        // MC_TREE: INTRA (10) → VQ_TREE: VQ_DATA (11) + 1 leaf byte.
        // Bits: 10 11, then byte 0xAB. VQ_DATA reads at the next
        // byte boundary, so the 4 tree bits occupy byte 0 and the
        // codebook index is byte 1.
        let mut bits = codes_to_bits(&[0b10, 0b11]);
        // pad the rest of byte 0 with zeros (4 bits left).
        bits.extend_from_slice(&[0, 0, 0, 0]);
        let mut buf = pack_bits(&bits);
        buf.push(0xAB);
        let prelude = prelude_at(0);
        let tree = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap();
        assert_eq!(tree.cell_count(), 1);
        match &tree.cells[0] {
            Cell::Intra {
                x,
                y,
                w,
                h,
                vq_leaves,
            } => {
                assert_eq!((*x, *y, *w, *h), (0, 0, 8, 8));
                assert_eq!(vq_leaves.len(), 1);
                assert_eq!(vq_leaves[0].x, 0);
                assert_eq!(vq_leaves[0].w, 8);
                assert_eq!(
                    vq_leaves[0].leaf,
                    VqLeaf::Data {
                        codebook_index: 0xAB
                    }
                );
            }
            other => panic!("expected Intra, got {other:?}"),
        }
    }

    #[test]
    fn single_inter_leaf_reads_mv_index_byte() {
        // MC_TREE: INTER (11) + leaf byte 0x42.
        let mut bits = codes_to_bits(&[0b11]);
        bits.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // pad byte 0
        let mut buf = pack_bits(&bits);
        buf.push(0x42);
        let prelude = prelude_at(0);
        let tree = decode_plane_tree(&buf, &prelude, 16, 16, false, FrameFlags(0)).unwrap();
        assert_eq!(tree.cell_count(), 1);
        match &tree.cells[0] {
            Cell::Inter {
                x,
                y,
                w,
                h,
                mv_index,
            } => {
                assert_eq!((*x, *y, *w, *h), (0, 0, 16, 16));
                assert_eq!(*mv_index, 0x42);
            }
            other => panic!("expected Inter, got {other:?}"),
        }
    }

    #[test]
    fn hsplit_halves_height_top_first() {
        // MC_TREE: H_SPLIT (00), then two INTER leaves (11 + byte).
        // First child is top half; geometry should be
        // (0,0,8,4) then (0,4,8,4) for an 8×8 plane.
        //
        // Cursor model (spec/03 §2.1 / §6.7): the bit reader loads
        // byte 0 into the bit buffer and advances `ebp` to 1; both
        // INTER leaf bytes are then read from `ebp` (bytes 1 and 2)
        // while the tree bits keep draining byte 0. So all six tree
        // bits live in byte 0:  00 11 11 + 2 pad = 0b0011_1100 =
        // 0x3C. byte 1 = first leaf byte, byte 2 = second.
        let buf = vec![0x3C, 0x11, 0x22];
        let prelude = prelude_at(0);
        let tree = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap();
        assert_eq!(tree.cell_count(), 2);
        assert_eq!(tree.cells[0].geometry(), (0, 0, 8, 4));
        assert_eq!(tree.cells[1].geometry(), (0, 4, 8, 4));
        match &tree.cells[0] {
            Cell::Inter { mv_index, .. } => assert_eq!(*mv_index, 0x11),
            other => panic!("expected Inter, got {other:?}"),
        }
        match &tree.cells[1] {
            Cell::Inter { mv_index, .. } => assert_eq!(*mv_index, 0x22),
            other => panic!("expected Inter, got {other:?}"),
        }
    }

    #[test]
    fn vsplit_halves_width_left_first() {
        // MC_TREE: V_SPLIT (01), then two INTER leaves.
        // First child is left half; geometry (0,0,4,8) then
        // (4,0,4,8) for an 8×8 plane.
        // Tree bits: 01 11 11 + 2 pad = 0b0111_1100 = 0x7C. leaf
        // bytes at byte 1 and byte 2 (see cursor note above).
        let buf = vec![0x7C, 0xAA, 0xBB];
        let prelude = prelude_at(0);
        let tree = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap();
        assert_eq!(tree.cell_count(), 2);
        assert_eq!(tree.cells[0].geometry(), (0, 0, 4, 8));
        assert_eq!(tree.cells[1].geometry(), (4, 0, 4, 8));
        match &tree.cells[0] {
            Cell::Inter { mv_index, .. } => assert_eq!(*mv_index, 0xAA),
            other => panic!("expected Inter, got {other:?}"),
        }
        match &tree.cells[1] {
            Cell::Inter { mv_index, .. } => assert_eq!(*mv_index, 0xBB),
            other => panic!("expected Inter, got {other:?}"),
        }
    }

    #[test]
    fn vq_null_copy_and_skip_subcodes() {
        // INTRA (10), then VQ_TREE: H_SPLIT (00) to get two
        // sub-cells; first VQ_NULL+copy (10 00), second
        // VQ_NULL+skip (10 01).
        // Bits: 10 00 10 00 10 01 → 12 bits.
        let bits = codes_to_bits(&[0b10, 0b00, 0b10, 0b00, 0b10, 0b01]);
        let buf = pack_bits(&bits);
        let prelude = prelude_at(0);
        let tree = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap();
        assert_eq!(tree.cell_count(), 1);
        match &tree.cells[0] {
            Cell::Intra { vq_leaves, .. } => {
                assert_eq!(vq_leaves.len(), 2);
                // top sub-cell: copy
                assert_eq!(vq_leaves[0].leaf, VqLeaf::Null(VqNull::Copy));
                assert_eq!((vq_leaves[0].x, vq_leaves[0].y), (0, 0));
                assert_eq!((vq_leaves[0].w, vq_leaves[0].h), (8, 4));
                // bottom sub-cell: skip
                assert_eq!(vq_leaves[1].leaf, VqLeaf::Null(VqNull::Skip));
                assert_eq!((vq_leaves[1].x, vq_leaves[1].y), (0, 4));
                assert_eq!((vq_leaves[1].w, vq_leaves[1].h), (8, 4));
            }
            other => panic!("expected Intra, got {other:?}"),
        }
    }

    #[test]
    fn vq_null_first_bit_one_is_unpacker_dispatch() {
        // spec/04 §4 / spec/06 §5.2 — INTRA (10), VQ_NULL (10),
        // sub-bit 1 → the one-bit "dispatch into the per-byte
        // unpacker" sub-code (NOT a fault; spec/03 §4.1's
        // fault reading is superseded by the spec/04 §4 listing's
        // `jb 0x10006bac` dispatch).
        let mut bits = codes_to_bits(&[0b10, 0b10]);
        bits.push(1); // the 1-bit sub-code
        let buf = pack_bits(&bits);
        let prelude = prelude_at(0);
        let tree = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap();
        match &tree.cells[0] {
            Cell::Intra { vq_leaves, .. } => {
                assert_eq!(vq_leaves.len(), 1);
                assert_eq!(vq_leaves[0].leaf, VqLeaf::Null(VqNull::Unpacker));
            }
            other => panic!("expected Intra, got {other:?}"),
        }
    }

    #[test]
    fn data_cursor_anchors_the_mode_byte_stream() {
        // spec/06 §5.1 — a VQ_DATA leaf's mode bytes begin right
        // after its codebook-index byte. Tree bits `10 11` live in
        // byte 0 (ebp advances to 1 on the bit-buffer load), the
        // leaf byte is byte 1, so the stream anchor is byte 2.
        let mut bits = codes_to_bits(&[0b10, 0b11]);
        bits.extend_from_slice(&[0, 0, 0, 0]);
        let mut buf = pack_bits(&bits);
        buf.push(0xAB);
        let prelude = prelude_at(0);
        let tree = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap();
        match &tree.cells[0] {
            Cell::Intra { vq_leaves, .. } => {
                assert_eq!(vq_leaves[0].data_cursor, Some(2));
            }
            other => panic!("expected Intra, got {other:?}"),
        }

        // spec/06 §5.2 — an unpacker-dispatch VQ_NULL has no leaf
        // byte: the anchor is the cursor right after the sub-code
        // bit (all five tree bits fit in byte 0, so ebp = 1).
        let mut bits = codes_to_bits(&[0b10, 0b10]);
        bits.push(1);
        let buf = pack_bits(&bits);
        let tree = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap();
        match &tree.cells[0] {
            Cell::Intra { vq_leaves, .. } => {
                assert_eq!(vq_leaves[0].leaf, VqLeaf::Null(VqNull::Unpacker));
                assert_eq!(vq_leaves[0].data_cursor, Some(1));
            }
            other => panic!("expected Intra, got {other:?}"),
        }

        // Copy / skip VQ_NULL leaves carry no byte payload.
        let bits = codes_to_bits(&[0b10, 0b10, 0b00]);
        let buf = pack_bits(&bits);
        let tree = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap();
        match &tree.cells[0] {
            Cell::Intra { vq_leaves, .. } => {
                assert_eq!(vq_leaves[0].data_cursor, None);
            }
            other => panic!("expected Intra, got {other:?}"),
        }

        // A non-zero plane base offsets the anchor absolutely.
        let mut bits = codes_to_bits(&[0b10, 0b11]);
        bits.extend_from_slice(&[0, 0, 0, 0]);
        let mut tail = pack_bits(&bits);
        tail.push(0xCD);
        let mut buf = vec![0u8; 3];
        buf.extend_from_slice(&tail);
        let prelude = prelude_at(3);
        let tree = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap();
        match &tree.cells[0] {
            Cell::Intra { vq_leaves, .. } => {
                assert_eq!(vq_leaves[0].data_cursor, Some(5));
            }
            other => panic!("expected Intra, got {other:?}"),
        }
    }

    #[test]
    fn vq_null_unpacker_subcode_consumes_exactly_one_bit() {
        // spec/06 §1.1 — the sub-code is a PREFIX code: `1` is one
        // bit, `00`/`01` are two. Prove the one-bit consumption by
        // following an unpacker leaf with a sibling whose parse
        // would misalign under a fixed 2-bit read:
        //
        // INTRA (10), VQ_TREE: H_SPLIT (00), then
        //   top:    VQ_NULL (10) + sub-bit 1        (3 bits)
        //   bottom: VQ_NULL (10) + sub-bits 01      (4 bits)
        //
        // Total 11 bits. A fixed 2-bit sub-code read would steal
        // the bottom child's leading `1` and misparse.
        let mut bits = codes_to_bits(&[0b10, 0b00, 0b10]);
        bits.push(1); // top sub-code: unpacker (1 bit)
        bits.extend_from_slice(&codes_to_bits(&[0b10]));
        bits.push(0); // bottom sub-code first bit
        bits.push(1); // bottom sub-code second bit → skip
        let buf = pack_bits(&bits);
        let prelude = prelude_at(0);
        let tree = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap();
        match &tree.cells[0] {
            Cell::Intra { vq_leaves, .. } => {
                assert_eq!(vq_leaves.len(), 2);
                assert_eq!(vq_leaves[0].leaf, VqLeaf::Null(VqNull::Unpacker));
                assert_eq!((vq_leaves[0].w, vq_leaves[0].h), (8, 4));
                assert_eq!(vq_leaves[1].leaf, VqLeaf::Null(VqNull::Skip));
                assert_eq!((vq_leaves[1].y, vq_leaves[1].h), (4, 4));
            }
            other => panic!("expected Intra, got {other:?}"),
        }
    }

    #[test]
    fn nested_split_geometry() {
        // INTRA root (10), then VQ_TREE: V_SPLIT (01) of the 8×8
        // cell into two 4×8 halves; left half H_SPLIT (00) into
        // two 4×4 sub-cells, each VQ_NULL+copy (10 00); right
        // half VQ_NULL+skip (10 01).
        // Codes: 10 | 01 | 00 | 10 00 | 10 00 | 10 01
        let bits = codes_to_bits(&[0b10, 0b01, 0b00, 0b10, 0b00, 0b10, 0b00, 0b10, 0b01]);
        let buf = pack_bits(&bits);
        let prelude = prelude_at(0);
        let tree = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap();
        match &tree.cells[0] {
            Cell::Intra { vq_leaves, .. } => {
                assert_eq!(vq_leaves.len(), 3);
                // left-top 4×4 copy
                assert_eq!((vq_leaves[0].x, vq_leaves[0].y), (0, 0));
                assert_eq!((vq_leaves[0].w, vq_leaves[0].h), (4, 4));
                assert_eq!(vq_leaves[0].leaf, VqLeaf::Null(VqNull::Copy));
                // left-bottom 4×4 copy
                assert_eq!((vq_leaves[1].x, vq_leaves[1].y), (0, 4));
                assert_eq!((vq_leaves[1].w, vq_leaves[1].h), (4, 4));
                // right 4×8 skip
                assert_eq!((vq_leaves[2].x, vq_leaves[2].y), (4, 0));
                assert_eq!((vq_leaves[2].w, vq_leaves[2].h), (4, 8));
                assert_eq!(vq_leaves[2].leaf, VqLeaf::Null(VqNull::Skip));
            }
            other => panic!("expected Intra, got {other:?}"),
        }
    }

    #[test]
    fn truncated_bitstream_errors() {
        // Empty payload — first node read fails.
        let buf: Vec<u8> = Vec::new();
        let prelude = prelude_at(0);
        let err = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap_err();
        assert_eq!(err, MacroblockError::BitstreamTruncated { offset: 0 });
    }

    #[test]
    fn leaf_byte_truncated_errors() {
        // INTER (11) but no leaf byte follows.
        let bits = codes_to_bits(&[0b11]);
        let buf = pack_bits(&bits); // one byte, tree bits + pad, no leaf byte
        // The leaf-byte read happens at byte 1, which doesn't exist.
        let prelude = prelude_at(0);
        let err = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap_err();
        assert_eq!(err, MacroblockError::LeafByteTruncated { offset: 1 });
    }

    #[test]
    fn bitstream_offset_out_of_range_errors() {
        let buf = vec![0u8; 4];
        let prelude = prelude_at(8); // past the 4-byte buffer
        let err = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap_err();
        assert_eq!(
            err,
            MacroblockError::BitstreamOffsetOutOfRange {
                bitstream_offset: 8,
                buffer_len: 4,
            }
        );
    }

    #[test]
    fn degenerate_split_errors() {
        // H_SPLIT a 1-sample-high cell — not halvable.
        let bits = codes_to_bits(&[0b00]);
        let buf = pack_bits(&bits);
        let prelude = prelude_at(0);
        let err = decode_plane_tree(&buf, &prelude, 8, 1, false, FrameFlags(0)).unwrap_err();
        assert_eq!(
            err,
            MacroblockError::DegenerateSplit {
                horizontal: true,
                dimension: 1,
            }
        );
    }

    #[test]
    fn odd_dimension_halves_ceil_then_floor() {
        let (a, b) = halve(9, false).unwrap();
        assert_eq!((a, b), (5, 4));
        let (a, b) = halve(8, true).unwrap();
        assert_eq!((a, b), (4, 4));
    }

    #[test]
    fn offset_base_reflected_in_truncation_error() {
        // bitstream_offset > 0 so the truncation error offset is
        // absolute (base + relative).
        let buf = vec![0u8; 4]; // payload starts at offset 4 → empty
        let prelude = prelude_at(4);
        let err = decode_plane_tree(&buf, &prelude, 8, 8, false, FrameFlags(0)).unwrap_err();
        assert_eq!(err, MacroblockError::BitstreamTruncated { offset: 4 });
    }
}

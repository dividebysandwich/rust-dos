//! Indeo 3 picture-layer plane prelude parser (per-plane).
//!
//! Spec source: `docs/video/indeo/indeo3/spec/02-picture-layer.md`.
//!
//! Round 2 lands a structural decode of the per-plane prelude bytes
//! (`num_vectors` + `mc_vectors[]`) that immediately follow each
//! plane offset stored in the bitstream header. The binary-tree /
//! VQ payload that begins after the prelude is not parsed by this
//! module — that work sits on `spec/03-macroblock-layer.md`.
//!
//! The flow:
//!
//! 1. Caller already holds a [`FrameHeader`](super::header::FrameHeader)
//!    obtained from [`FrameHeader::parse`](super::header::FrameHeader::parse).
//! 2. Caller passes the full codec-frame input buffer (the same
//!    `lpInput` the §1 host hands the decoder) into
//!    [`PictureLayer::parse`].
//! 3. [`PictureLayer::parse`] iterates the three planes in
//!    spec/02 §2 order (U → V → Y), classifies each plane as
//!    *present* / *skipped* per the spec/02 §2 range check
//!    (`offset >= 0` treated as i32 + `offset <= data_size/8`),
//!    and for the present planes reads the §3.1 `num_vectors` u32
//!    and the §3.2 `mc_vectors[num_vectors]` two-signed-byte array.
//! 4. The returned [`PictureLayer`] holds one
//!    [`PlanePrelude`] per plane in spec/02 §2 iteration order plus
//!    a precomputed `bitstream_offset` (the absolute offset into
//!    the codec-frame buffer where each plane's binary-tree /
//!    VQ payload begins, = `bsh + plane_offset + 4 + 2*num_vectors`,
//!    per spec/02 §3.4).
//!
//! NULL frames (spec/02 §1, `data_size == 0x80`) skip the plane
//! iteration entirely; in that case [`PictureLayer::parse`] returns
//! a layer with all three planes marked
//! [`PlanePresence::NullFrame`].
//!
//! Half-pel scaling (spec/02 §3.3) is applied to the
//! [`MotionVector::vertical_scaled`] / `horizontal_scaled` fields
//! during parsing; the raw signed bytes are also preserved as
//! [`MotionVector::vertical_raw`] / `horizontal_raw` so callers can
//! reconstruct the packed-MV layout (spec/02 §3.3, packing formula)
//! if they need it.

use super::header::{
    BITSTREAM_HEADER_LEN, FRAME_HEADER_LEN, FrameFlags, FrameHeader, NULL_FRAME_DATA_SIZE_BITS,
};
use super::strip_context::{
    PerPlaneDecodeCall, PlaneRole, StripGeometry, StripSlotDescriptor, chroma_plane_height,
    chroma_plane_width, strip_slot_index,
};

/// Spec/02 §2 — count of planes a codec frame carries.
pub const PLANE_COUNT: usize = 3;

/// Spec/02 §2 — plane iteration order index for the U (chroma)
/// plane. Decoded first per the count-down loop.
pub const PLANE_IDX_U: usize = 2;
/// Spec/02 §2 — plane iteration order index for the V (chroma)
/// plane. Decoded second.
pub const PLANE_IDX_V: usize = 1;
/// Spec/02 §2 — plane iteration order index for the Y (luma)
/// plane. Decoded last.
pub const PLANE_IDX_Y: usize = 0;

/// Spec/02 §3.1 — size in bytes of the `num_vectors` u32 LE.
pub const NUM_VECTORS_FIELD_LEN: usize = 4;

/// Spec/02 §3.2 — size in bytes of one `mc_vectors[]` entry.
pub const MC_VECTOR_ENTRY_LEN: usize = 2;

/// Spec/02 §3.4 — minimum plane-prelude size, in bytes
/// (`num_vectors == 0`, INTRA frame).
pub const MIN_PRELUDE_LEN: usize = NUM_VECTORS_FIELD_LEN;

/// Errors raised while parsing the spec/02 picture layer (the
/// per-plane preludes).
///
/// The spec/02 §2 plane-offset range check raises
/// [`PictureLayerError::PlaneOffsetOutOfRange`] for catastrophic
/// invalid offsets (e.g. an offset that would point past the input
/// buffer) — note however that an offset which is **negative** (as
/// i32) OR strictly greater than the spec/02 §2 budget
/// `data_size/8` is **not** an error: per spec/02 §2 the decoder
/// silently skips that plane and moves to the next `plane_idx`.
/// Such planes surface as [`PlanePresence::Skipped`].
///
/// Only structurally impossible offsets — those whose computed
/// plane base lies outside the input buffer, or whose
/// `num_vectors` field would extend past the input buffer — raise
/// an error.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PictureLayerError {
    /// The plane's computed base address would extend past the
    /// end of the input buffer. The decoder cannot read the
    /// `num_vectors` u32 without overrunning.
    PlaneOffsetOutOfRange {
        /// Plane index (0 = Y, 1 = V, 2 = U).
        plane_idx: usize,
        /// `plane_offset` value the bitstream header carries.
        plane_offset: u32,
        /// Resulting absolute base offset into the input buffer
        /// (`bsh + plane_offset`).
        computed_base: usize,
        /// Length of the input buffer the caller supplied.
        buffer_len: usize,
    },
    /// The plane's motion-vector array would extend past the end
    /// of the input buffer. `num_vectors * 2` bytes are required
    /// to follow the `num_vectors` field at `plane_base + 4`.
    MotionVectorArrayTruncated {
        /// Plane index (0 = Y, 1 = V, 2 = U).
        plane_idx: usize,
        /// `num_vectors` value read from `plane_base + 0`.
        num_vectors: u32,
        /// First byte of the motion-vector array.
        array_start: usize,
        /// Required end of the array (`array_start + num_vectors * 2`).
        array_end: usize,
        /// Length of the input buffer the caller supplied.
        buffer_len: usize,
    },
}

impl core::fmt::Display for PictureLayerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            PictureLayerError::PlaneOffsetOutOfRange {
                plane_idx,
                plane_offset,
                computed_base,
                buffer_len,
            } => write!(
                f,
                "plane {plane_idx}: plane_offset 0x{plane_offset:08x} → base offset {computed_base} would overrun a {buffer_len}-byte input buffer"
            ),
            PictureLayerError::MotionVectorArrayTruncated {
                plane_idx,
                num_vectors,
                array_start,
                array_end,
                buffer_len,
            } => write!(
                f,
                "plane {plane_idx}: mc_vectors[{num_vectors}] = bytes {array_start}..{array_end} overruns a {buffer_len}-byte input buffer"
            ),
        }
    }
}

impl std::error::Error for PictureLayerError {}

/// Spec/02 §3.2 — a single motion-vector entry.
///
/// The wire encoding is two signed bytes (vertical, then
/// horizontal). Spec/02 §3.3 applies a per-component arithmetic
/// right shift to each byte when the matching half-pel flag is
/// set in `frame_flags`; the shifted-out LSB is preserved as a
/// "half-pel offset" sub-field used by the spec/02 §3.3 packing
/// formula.
///
/// For convenience the parsed struct exposes both:
///
/// * `vertical_raw` / `horizontal_raw` — the unmodified signed
///   bytes as they appear on the wire.
/// * `vertical_scaled` / `horizontal_scaled` — the post-shift
///   integer component the packing formula uses for the high
///   bits of the packed-MV index.
/// * `vertical_halfpel_bit` / `horizontal_halfpel_bit` — the
///   LSB shifted out by the half-pel scaling (0 in full-pel
///   paths).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotionVector {
    /// Vertical component as it appears on the wire (signed byte
    /// at `mc_ptr + 0`).
    pub vertical_raw: i8,
    /// Horizontal component as it appears on the wire (signed
    /// byte at `mc_ptr + 1`).
    pub horizontal_raw: i8,
    /// Vertical component after the spec/02 §3.3 half-pel right
    /// shift (= `vertical_raw` when `MV_HALFPEL_VERT` is clear).
    /// The shift is arithmetic, so sign is preserved.
    pub vertical_scaled: i16,
    /// Horizontal component after the spec/02 §3.3 half-pel
    /// right shift (= `horizontal_raw` when `MV_HALFPEL_HORIZ`
    /// is clear).
    pub horizontal_scaled: i16,
    /// LSB of `vertical_raw` (extracted before the right shift)
    /// when `MV_HALFPEL_VERT` is set; 0 otherwise.
    pub vertical_halfpel_bit: u8,
    /// LSB of `horizontal_raw` (extracted before the right shift)
    /// when `MV_HALFPEL_HORIZ` is set; 0 otherwise.
    pub horizontal_halfpel_bit: u8,
}

impl MotionVector {
    /// Compute the packed-MV index per the spec/02 §3.3 formula:
    ///
    /// ```text
    /// packed_mv = ((vert_shifted * 11) << 4 + horiz_shifted) << 2
    ///           + (horiz_lsb << 1)
    ///           + vert_lsb
    /// ```
    ///
    /// The cast widths (`i32`) match the parser's
    /// `IR32_32.DLL!0x100043f6`-`0x10004426` register usage.
    pub fn packed_mv(self) -> i32 {
        let vert_shifted = self.vertical_scaled as i32;
        let horiz_shifted = self.horizontal_scaled as i32;
        let vert_lsb = self.vertical_halfpel_bit as i32;
        let horiz_lsb = self.horizontal_halfpel_bit as i32;
        (((vert_shifted * 11) << 4) + horiz_shifted) << 2 | (horiz_lsb << 1) | vert_lsb
    }
}

/// Spec/02 §9 — typed view of a single plane's byte-level data
/// layout within the codec-frame input buffer.
///
/// The spec/02 §9 "plane-data byte map" diagram identifies four
/// landmark offsets per plane:
///
/// 1. `plane_base = bsh + plane_offset` — start of the plane's
///    data (where `num_vectors` lives).
/// 2. `plane_base + 4` — end of the `num_vectors` u32 / start of
///    the `mc_vectors[]` array.
/// 3. `plane_base + 4 + 2*num_vectors` — end of the prelude /
///    start of the binary-tree + VQ bitstream payload (the same
///    value as [`PlanePrelude::bitstream_offset`]).
/// 4. `payload_upper_bound` — strict upper bound on bytes the
///    binary-tree decoder may consume. The spec/02 §6.1 wording
///    states the decoder does not validate the gap to the next
///    plane and the encoder is free to leave unused bytes between
///    one plane's consumed payload and the start of the next; the
///    structural maximum the decoder may read is therefore the
///    start of the **next present** plane (per the spec/02 §2
///    plane-offset ordering in the bitstream header), falling back
///    to the codec-frame buffer length when no later present plane
///    exists. This is the §10 open-question-4 "end-of-plane
///    padding" surface — the byte budget the decoder may scan,
///    distinguished from the bytes it actually consumes (which
///    only the binary-tree walk knows).
///
/// All four offsets are **absolute** into the codec-frame input
/// buffer (the same buffer
/// [`FrameHeader::parse`](super::header::FrameHeader::parse) and
/// [`PictureLayer::parse`] consume). They are pre-bounds-checked
/// during [`PictureLayer::parse`]: `num_vectors_range.end ≤
/// mc_vectors_range.end ≤ payload_start ≤ payload_upper_bound ≤
/// buffer_len`.
///
/// The map is purely **structural** — it does not consult the
/// binary-tree codes, does not own any payload bytes, and does not
/// alter the parsed [`PlanePrelude`]. It is the typed companion to
/// the §9 ASCII diagram, intended for callers that want to slice
/// the input buffer per-region (e.g. for hex dumps, debugger
/// overlays, or pre-payload structural validation) without
/// reaching into [`PlanePrelude`]'s prelude_len arithmetic
/// themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneByteMap {
    /// Spec/02 §2 — plane index this map describes
    /// (`PLANE_IDX_Y`, `PLANE_IDX_V`, `PLANE_IDX_U`).
    pub plane_idx: usize,
    /// Spec/02 §3 — absolute byte offset of `plane_base`
    /// (= `bsh + plane_offset`).
    pub plane_base: usize,
    /// Spec/02 §9 / §3.1 — absolute byte range of the
    /// `num_vectors` u32 (= `plane_base..plane_base + 4`).
    pub num_vectors_range: core::ops::Range<usize>,
    /// Spec/02 §9 / §3.2 — absolute byte range of the
    /// `mc_vectors[]` array (= `plane_base + 4..plane_base + 4 +
    /// 2*num_vectors`). Empty range when `num_vectors == 0`
    /// (INTRA plane).
    pub mc_vectors_range: core::ops::Range<usize>,
    /// Spec/02 §9 / §3.4 — absolute byte offset of the first byte
    /// of the binary-tree / VQ bitstream payload
    /// (= `plane_base + 4 + 2*num_vectors`). Identical to the
    /// owning [`PlanePrelude::bitstream_offset`].
    pub payload_start: usize,
    /// Spec/02 §6.1 / §10 item 4 — strict upper bound on bytes
    /// the per-plane decoder may scan from this plane's payload.
    /// Equals the next present plane's `plane_base` when one
    /// exists, otherwise the codec-frame buffer length. The
    /// decoder is not required to consume up to this bound — it
    /// stops when the binary-tree walk naturally terminates — but
    /// it is structurally forbidden from reading past it.
    pub payload_upper_bound: usize,
}

impl PlaneByteMap {
    /// Spec/02 §9 — total bytes available to the binary-tree
    /// decoder for this plane's payload
    /// (= `payload_upper_bound - payload_start`). Tolerant of an
    /// upper-bound equal to `payload_start` (returns 0); the
    /// invariant `payload_start ≤ payload_upper_bound` is enforced
    /// by the constructor.
    pub fn payload_budget(&self) -> usize {
        self.payload_upper_bound.saturating_sub(self.payload_start)
    }

    /// Spec/02 §3.4 — total prelude byte length
    /// (`4 + 2*num_vectors`), equivalent to
    /// [`PlanePrelude::prelude_len`]. Computed from the
    /// `mc_vectors_range` so the map is self-describing.
    pub fn prelude_len(&self) -> usize {
        NUM_VECTORS_FIELD_LEN + (self.mc_vectors_range.end - self.mc_vectors_range.start)
    }
}

/// Spec/02 §3 — per-plane prelude (motion-vector array) plus the
/// computed start of the plane's bitstream payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanePrelude {
    /// Spec/02 §3.1 — `num_vectors` u32 read from
    /// `plane_base + 0`.
    pub num_vectors: u32,
    /// Spec/02 §3.2 — motion-vector array of length
    /// `num_vectors`. Each entry carries its raw bytes and its
    /// half-pel-scaled components.
    pub motion_vectors: Vec<MotionVector>,
    /// Absolute offset (from the codec-frame buffer start, i.e.
    /// the same buffer [`PictureLayer::parse`] consumed) where
    /// the plane's binary-tree / VQ bitstream payload begins.
    ///
    /// Per spec/02 §3.4: `bitstream_offset = plane_base + 4 +
    /// 2*num_vectors = bsh + plane_offset + 4 + 2*num_vectors`.
    pub bitstream_offset: usize,
}

impl PlanePrelude {
    /// Spec/02 §3.4 — total prelude byte length
    /// (`4 + 2*num_vectors`).
    pub fn prelude_len(&self) -> usize {
        NUM_VECTORS_FIELD_LEN + MC_VECTOR_ENTRY_LEN * self.num_vectors as usize
    }
}

/// Spec/02 §2 — outcome of the per-plane offset range check.
///
/// A plane whose `plane_offset` is negative (i32) or strictly
/// greater than `data_size/8` is skipped by the decoder per
/// spec/02 §2. We record the reason it was skipped so callers
/// can distinguish "the encoder omitted this plane on purpose"
/// from "no plane data because it's a NULL frame".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanePresence {
    /// Plane prelude was parsed successfully.
    Present(PlanePrelude),
    /// Spec/02 §1 — NULL frame; plane iteration was skipped
    /// for the entire codec frame.
    NullFrame,
    /// Spec/02 §2 — `plane_offset < 0` (interpreted as i32),
    /// i.e. high bit set. The encoder signalled "omit this
    /// plane".
    SkippedNegativeOffset {
        /// `plane_offset` as it appears on the wire (u32 LE).
        plane_offset: u32,
    },
    /// Spec/02 §2 — `plane_offset > data_size/8`. The plane
    /// base would lie past the bitstream-data budget; the
    /// decoder skips this plane.
    SkippedAboveDataBudget {
        /// `plane_offset` as it appears on the wire.
        plane_offset: u32,
        /// `data_size / 8` — the byte budget the check uses.
        budget_bytes: u32,
    },
}

impl PlanePresence {
    /// True iff the plane has a parsed prelude.
    pub fn is_present(&self) -> bool {
        matches!(self, PlanePresence::Present(_))
    }

    /// Borrow the underlying [`PlanePrelude`] if the plane was
    /// parsed.
    pub fn as_prelude(&self) -> Option<&PlanePrelude> {
        match self {
            PlanePresence::Present(p) => Some(p),
            _ => None,
        }
    }
}

/// Spec/02 picture-layer view of an entire codec frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictureLayer {
    /// Per-plane preludes, indexed by spec/02 §2 `plane_idx`
    /// (0 = Y, 1 = V, 2 = U). The plane iteration order itself
    /// is reverse of this index — see
    /// [`PictureLayer::iter_in_decode_order`].
    pub planes: [PlanePresence; PLANE_COUNT],
}

impl PictureLayer {
    /// Decode the picture-layer plane preludes from the full
    /// codec-frame input buffer (the same buffer
    /// [`FrameHeader::parse`] consumed).
    ///
    /// Per spec/02 §1, NULL frames (`data_size == 0x80`) skip
    /// the plane iteration entirely; this method returns a
    /// layer with every plane set to
    /// [`PlanePresence::NullFrame`] in that case.
    ///
    /// For non-NULL frames the method walks the three planes in
    /// spec/02 §2 order (U → V → Y), classifying each as
    /// present-or-skipped per the spec/02 §2 range check, and
    /// for present planes parses the §3.1 `num_vectors` u32 +
    /// §3.2 `mc_vectors[]` array with the §3.3 half-pel scaling
    /// applied per [`FrameFlags`].
    pub fn parse(header: &FrameHeader, input: &[u8]) -> Result<Self, PictureLayerError> {
        if header.bitstream.is_null_frame() {
            return Ok(PictureLayer {
                planes: [
                    PlanePresence::NullFrame,
                    PlanePresence::NullFrame,
                    PlanePresence::NullFrame,
                ],
            });
        }

        // bsh (bitstream-header) base offset into the input
        // buffer; see spec/01 §3.
        let bsh_base = FRAME_HEADER_LEN;

        // Spec/02 §2 — byte budget = data_size / 8 (data_size is
        // in bits; the parser's `sar edx, 0x3` is unsigned-ish
        // here because data_size's high bit is unused in
        // practice).
        let budget_bytes = header.bitstream.data_size / 8;

        // Read offsets in spec/02 §2 order (U, V, Y) but store
        // each into the array slot matching its plane_idx
        // (0 = Y, 1 = V, 2 = U).
        let offsets: [u32; PLANE_COUNT] = [
            header.bitstream.y_offset,
            header.bitstream.v_offset,
            header.bitstream.u_offset,
        ];

        // Default-initialise; will be overwritten per plane.
        let mut planes = [
            PlanePresence::SkippedNegativeOffset { plane_offset: 0 },
            PlanePresence::SkippedNegativeOffset { plane_offset: 0 },
            PlanePresence::SkippedNegativeOffset { plane_offset: 0 },
        ];

        let flags = header.bitstream.frame_flags;

        // Spec/02 §2 — count-down `plane_idx` ∈ {2, 1, 0}.
        for plane_idx in (0..PLANE_COUNT).rev() {
            let plane_offset = offsets[plane_idx];

            // Spec/02 §2 — `plane_offset < 0` when interpreted
            // as i32. The high bit of the u32 being set is
            // equivalent.
            if (plane_offset as i32) < 0 {
                planes[plane_idx] = PlanePresence::SkippedNegativeOffset { plane_offset };
                continue;
            }

            // Spec/02 §2 — `plane_offset > data_size/8`.
            if plane_offset > budget_bytes {
                planes[plane_idx] = PlanePresence::SkippedAboveDataBudget {
                    plane_offset,
                    budget_bytes,
                };
                continue;
            }

            // Plane present — parse the prelude.
            let prelude = parse_plane_prelude(input, bsh_base, plane_idx, plane_offset, flags)?;
            planes[plane_idx] = PlanePresence::Present(prelude);
        }

        Ok(PictureLayer { planes })
    }

    /// Spec/02 §2 — iterate the planes in decode order
    /// (`plane_idx = 2, 1, 0` → U, V, Y).
    pub fn iter_in_decode_order(&self) -> impl Iterator<Item = (usize, &PlanePresence)> + '_ {
        (0..PLANE_COUNT).rev().map(move |i| (i, &self.planes[i]))
    }

    /// Borrow the Y-plane (luma) presence record.
    pub fn y(&self) -> &PlanePresence {
        &self.planes[PLANE_IDX_Y]
    }

    /// Borrow the V-plane (chroma) presence record.
    pub fn v(&self) -> &PlanePresence {
        &self.planes[PLANE_IDX_V]
    }

    /// Borrow the U-plane (chroma) presence record.
    pub fn u(&self) -> &PlanePresence {
        &self.planes[PLANE_IDX_U]
    }

    /// Spec/02 §9 — return the typed byte map for a present plane.
    ///
    /// `plane_idx` selects the plane (`PLANE_IDX_Y`,
    /// `PLANE_IDX_V`, or `PLANE_IDX_U`); `header` is the parsed
    /// frame header the layer was built from (its bsh `*_offset`
    /// fields drive the per-plane base); `buffer_len` is the
    /// length of the codec-frame input buffer (the same buffer
    /// [`PictureLayer::parse`] consumed) — needed to compute the
    /// §6.1 / §10 item 4 `payload_upper_bound` for the
    /// last-decoded plane.
    ///
    /// The §6.1 upper-bound resolution chooses the smallest
    /// "next-plane base" among the OTHER present planes whose
    /// base is strictly greater than this plane's
    /// `payload_start`. Spec/02 §2 admits any plane-iteration
    /// order so we cannot rely on `plane_idx`-numeric ordering;
    /// the upper bound is whichever later plane base actually
    /// follows this plane in the byte layout (or `buffer_len` if
    /// none does).
    ///
    /// Returns `None` when:
    ///
    /// * `plane_idx >= PLANE_COUNT`, or
    /// * `self.planes[plane_idx]` is not
    ///   [`PlanePresence::Present`] (NULL-frame or skipped plane —
    ///   no byte map exists for either).
    pub fn plane_byte_map(
        &self,
        plane_idx: usize,
        header: &FrameHeader,
        buffer_len: usize,
    ) -> Option<PlaneByteMap> {
        if plane_idx >= PLANE_COUNT {
            return None;
        }
        let prelude = self.planes[plane_idx].as_prelude()?;
        let bsh_base = FRAME_HEADER_LEN;
        let offsets: [u32; PLANE_COUNT] = [
            header.bitstream.y_offset,
            header.bitstream.v_offset,
            header.bitstream.u_offset,
        ];
        let plane_offset = offsets[plane_idx];
        let plane_base = bsh_base.saturating_add(plane_offset as usize);
        let mc_start = plane_base + NUM_VECTORS_FIELD_LEN;
        let mc_end = prelude.bitstream_offset;
        let payload_start = prelude.bitstream_offset;

        // Spec/02 §6.1 / §10 item 4 — upper bound is the smallest
        // OTHER-present-plane base strictly greater than this
        // plane's payload_start, falling back to buffer_len.
        let mut upper = buffer_len;
        for (other_idx, presence) in self.planes.iter().enumerate() {
            if other_idx == plane_idx {
                continue;
            }
            if presence.is_present() {
                let other_base = bsh_base.saturating_add(offsets[other_idx] as usize);
                if other_base > payload_start && other_base < upper {
                    upper = other_base;
                }
            }
        }
        // Defensive clamp: a malformed parse would have already
        // raised earlier, but cap at buffer_len in case the
        // caller passed a buffer_len smaller than payload_start.
        if upper < payload_start {
            upper = payload_start;
        }

        Some(PlaneByteMap {
            plane_idx,
            plane_base,
            num_vectors_range: plane_base..mc_start,
            mc_vectors_range: mc_start..mc_end,
            payload_start,
            payload_upper_bound: upper,
        })
    }

    /// Spec/02 §4 + §5 + §6 — build the per-plane decode plan that
    /// bridges this picture-layer view to the strip-context surface.
    ///
    /// Given a parsed plane (its [`PlanePrelude`]) and the parsed
    /// [`FrameHeader`], compose:
    ///
    /// * the spec/02 §4 [`StripGeometry`] (plane dimensions + strip
    ///   width + strip count + last-strip width), with the §4 chroma
    ///   subsampling table applied for V / U;
    /// * the spec/02 §5.1 / §5.2 [`StripSlotDescriptor`] (slot index
    ///   keyed by `(plane_idx, buffer_selector)`, plane role, and
    ///   per-slot `STRIP_WIDTH` / `STRIP_HEIGHT` field offsets); and
    /// * the spec/02 §3.4 [`PlanePrelude::bitstream_offset`] the
    ///   per-plane decoder consumes as its 4th argument (§6 table).
    ///
    /// Returns:
    ///
    /// * `None` if `plane_idx >= PLANE_COUNT` (the only legal values
    ///   are the three plane-index constants).
    /// * `None` if `self.planes[plane_idx]` is anything other than
    ///   [`PlanePresence::Present`] — there is no decode call to plan
    ///   for a NULL-frame plane (§1) or a skipped plane (§2).
    ///
    /// The `buffer_selector` argument is `header.bitstream.frame_flags
    /// .buffer_selector()` (§3.2 bit 9) — taken as a parameter so a
    /// caller modelling the ping-pong frame-to-frame buffer
    /// alternation can pass either value without re-parsing the
    /// header.
    pub fn plane_decode_plan(
        &self,
        plane_idx: usize,
        header: &FrameHeader,
        buffer_selector: bool,
    ) -> Option<PlaneDecodePlan> {
        if plane_idx >= PLANE_COUNT {
            return None;
        }
        let prelude = self.planes[plane_idx].as_prelude()?;
        let luma_width = u32::from(header.bitstream.width);
        let luma_height = u32::from(header.bitstream.height);

        let (plane_width, plane_height, geometry) = if plane_idx == PLANE_IDX_Y {
            (
                luma_width,
                luma_height,
                StripGeometry::for_luma(luma_width, luma_height),
            )
        } else {
            let cw = chroma_plane_width(luma_width);
            let ch = chroma_plane_height(luma_height);
            (cw, ch, StripGeometry::for_chroma(cw, ch))
        };

        let strip_width_field = if geometry.strip_count == 0 {
            geometry.strip_width
        } else {
            // Spec/02 §4.1 — the per-slot `[ctx+0x1c]` field carries
            // the §4.1 strip-width constant by default, overridden to
            // the §4.1 remainder formula for a 1-strip plane. With
            // strip_count = 1 the §4.1 remainder is the only strip's
            // width.
            if geometry.strip_count == 1 {
                geometry.last_strip_width
            } else {
                geometry.strip_width
            }
        };
        let slot_descriptor = StripSlotDescriptor::for_dispatch(
            plane_idx,
            buffer_selector,
            strip_width_field,
            plane_height,
        )?;
        let slot_idx = strip_slot_index(plane_idx, buffer_selector)?;
        let role = PlaneRole::for_slot(slot_idx);
        Some(PlaneDecodePlan {
            plane_idx,
            buffer_selector,
            role,
            plane_width,
            plane_height,
            num_vectors: prelude.num_vectors,
            bitstream_offset: prelude.bitstream_offset,
            geometry,
            slot_descriptor,
        })
    }
}

/// Spec/02 §4 + §5 + §6 — composite per-plane decode plan.
///
/// Built by [`PictureLayer::plane_decode_plan`], this struct bundles
/// the picture-layer side (the plane index, the §4 strip geometry,
/// the §3.4 bitstream-payload offset, the §3.1 motion-vector count)
/// with the strip-context side (the §5.1 / §5.2 slot descriptor and
/// the §5.1 / §6 plane-role classification) at a single typed
/// surface. Callers ready to dispatch the §6 per-plane decode call
/// (`IR32_32.DLL!0x10006538`) can read every per-plane parameter
/// from this struct without re-traversing the picture layer.
///
/// The struct does **not** model the per-plane decoder's seven
/// cdecl arguments themselves — the
/// [`super::strip_context::PerPlaneDecodeCall`] type covers that
/// surface and consumes the [`Self::bitstream_offset`] this plan
/// carries. [`Self::to_decode_call`] is the typed bridge between
/// the two surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneDecodePlan {
    /// Spec/02 §2 plane index (0 = Y, 1 = V, 2 = U).
    pub plane_idx: usize,
    /// Spec/02 §3.2 / §5.1 `frame_flags` bit 9 — secondary (`true`)
    /// vs primary (`false`) buffer-bank selector. Carried so callers
    /// can re-derive [`slot_descriptor`](Self::slot_descriptor) for
    /// the opposite bank without re-parsing the frame header.
    pub buffer_selector: bool,
    /// Spec/02 §5.1 / §6 plane-role classification of the strip-
    /// context slot this plane will write into (Luma for Y, Chroma
    /// for V / U).
    pub role: PlaneRole,
    /// Plane width in samples — luma width for Y, `luma_width / 4`
    /// for V / U per the §4 picture-decomposition table.
    pub plane_width: u32,
    /// Plane height in samples — luma height for Y,
    /// [`chroma_plane_height`] for V / U.
    pub plane_height: u32,
    /// Spec/02 §3.1 `num_vectors` carried over from the parsed
    /// [`PlanePrelude`].
    pub num_vectors: u32,
    /// Spec/02 §3.4 absolute offset (within the codec-frame input
    /// buffer) of the first byte of the plane's binary-tree / VQ
    /// bitstream payload (= `plane_base + 4 + 2*num_vectors`).
    pub bitstream_offset: usize,
    /// Spec/02 §4.1 / §4.2 strip geometry — plane width / height,
    /// per-plane-class strip width (160 for luma, 40 for chroma),
    /// `ceil(plane_width / strip_width)` strip count, and the §4.1
    /// remainder-formula last-strip width.
    pub geometry: StripGeometry,
    /// Spec/02 §5.1 / §5.2 strip-context slot descriptor (slot index,
    /// role, byte offset of the slot inside the strip-context array,
    /// and the §4.1 / §4.4 per-slot `STRIP_WIDTH` / `STRIP_HEIGHT`
    /// values).
    pub slot_descriptor: StripSlotDescriptor,
}

impl PlaneDecodePlan {
    /// Convenience — true iff the plan's plane is the luma plane.
    pub fn is_luma(&self) -> bool {
        self.role.is_luma()
    }

    /// Convenience — true iff the plan's plane is a chroma plane.
    pub fn is_chroma(&self) -> bool {
        self.role.is_chroma()
    }

    /// Spec/02 §3.1 — true iff the plane carries no motion vectors
    /// (an INTRA-coded plane, per `num_vectors == 0`).
    pub fn is_intra(&self) -> bool {
        self.num_vectors == 0
    }

    /// Spec/02 §6 — bridge from this picture-layer plan to the typed
    /// 7-argument [`PerPlaneDecodeCall`] frame the per-plane decoder
    /// (`IR32_32.DLL!0x10006538`) consumes.
    ///
    /// The plan already carries every value the §6 call frame needs:
    ///
    /// * the spec/02 §2 [`plane_idx`](Self::plane_idx),
    /// * the spec/02 §3.2 / §5.1
    ///   [`buffer_selector`](Self::buffer_selector), and
    /// * the spec/02 §3.4
    ///   [`bitstream_offset`](Self::bitstream_offset) (the §6 table's
    ///   4th argument — the binary-tree / VQ payload pointer).
    ///
    /// The §6 codebook-bank discriminant (luma → `+0x1a00` /
    /// chroma → `+0x400`) and the constant offsets for the strip-
    /// context array view (`+0x300c`) and the secondary codebook
    /// pointer (`+0x3004`) are resolved inside
    /// [`PerPlaneDecodeCall::for_plane_and_buffer`].
    ///
    /// `slot_idx_src` and `slot_idx_dst` are set to the same value
    /// per spec/02 §10 item 3 — matching the [`slot_descriptor`]'s
    /// [`slot_index`](super::strip_context::StripSlotDescriptor::slot_index)
    /// that the picture-layer plan already names. A caller modelling
    /// the spec/02 §10 item 3 hypothetical inter-bank read may
    /// override either field on the returned struct after the bridge.
    ///
    /// This method never returns `None` because [`PlaneDecodePlan`]
    /// is only ever constructed from
    /// [`PictureLayer::plane_decode_plan`], which rejects out-of-range
    /// plane indices up front. The return type is `PerPlaneDecodeCall`
    /// directly, not `Option<…>`, to reflect that invariant.
    ///
    /// [`slot_descriptor`]: Self::slot_descriptor
    pub fn to_decode_call(&self) -> PerPlaneDecodeCall {
        // The unwrap is infallible: `PictureLayer::plane_decode_plan`
        // rejects `plane_idx >= PLANE_COUNT` at construction time, so
        // every reachable `PlaneDecodePlan` carries a `plane_idx` that
        // `PerPlaneDecodeCall::for_plane_and_buffer` accepts.
        PerPlaneDecodeCall::for_plane_and_buffer(
            self.plane_idx,
            self.buffer_selector,
            self.bitstream_offset,
        )
        .expect("PlaneDecodePlan only carries spec/02 §2 plane indices")
    }
}

fn parse_plane_prelude(
    input: &[u8],
    bsh_base: usize,
    plane_idx: usize,
    plane_offset: u32,
    flags: FrameFlags,
) -> Result<PlanePrelude, PictureLayerError> {
    // Spec/02 §3 — `plane_base = bsh + plane_offset`.
    let plane_base = bsh_base.saturating_add(plane_offset as usize);

    // The `num_vectors` u32 starts at `[plane_base + 0]`.
    let num_vectors_end = plane_base.saturating_add(NUM_VECTORS_FIELD_LEN);
    if num_vectors_end > input.len() {
        return Err(PictureLayerError::PlaneOffsetOutOfRange {
            plane_idx,
            plane_offset,
            computed_base: plane_base,
            buffer_len: input.len(),
        });
    }

    let num_vectors = u32::from_le_bytes([
        input[plane_base],
        input[plane_base + 1],
        input[plane_base + 2],
        input[plane_base + 3],
    ]);

    let array_start = plane_base + NUM_VECTORS_FIELD_LEN;
    let array_bytes = (num_vectors as usize)
        .checked_mul(MC_VECTOR_ENTRY_LEN)
        .ok_or(PictureLayerError::MotionVectorArrayTruncated {
            plane_idx,
            num_vectors,
            array_start,
            array_end: usize::MAX,
            buffer_len: input.len(),
        })?;
    let array_end = array_start.checked_add(array_bytes).ok_or(
        PictureLayerError::MotionVectorArrayTruncated {
            plane_idx,
            num_vectors,
            array_start,
            array_end: usize::MAX,
            buffer_len: input.len(),
        },
    )?;

    if array_end > input.len() {
        return Err(PictureLayerError::MotionVectorArrayTruncated {
            plane_idx,
            num_vectors,
            array_start,
            array_end,
            buffer_len: input.len(),
        });
    }

    let halfpel_vert = flags.mv_halfpel_vert();
    let halfpel_horiz = flags.mv_halfpel_horiz();

    let mut motion_vectors = Vec::with_capacity(num_vectors as usize);
    for i in 0..num_vectors as usize {
        let off = array_start + i * MC_VECTOR_ENTRY_LEN;
        let vertical_raw = input[off] as i8;
        let horizontal_raw = input[off + 1] as i8;

        let (vertical_scaled, vertical_halfpel_bit) = scale_component(vertical_raw, halfpel_vert);
        let (horizontal_scaled, horizontal_halfpel_bit) =
            scale_component(horizontal_raw, halfpel_horiz);

        motion_vectors.push(MotionVector {
            vertical_raw,
            horizontal_raw,
            vertical_scaled,
            horizontal_scaled,
            vertical_halfpel_bit,
            horizontal_halfpel_bit,
        });
    }

    Ok(PlanePrelude {
        num_vectors,
        motion_vectors,
        bitstream_offset: array_end,
    })
}

/// Spec/02 §3.3 — half-pel component scaling.
///
/// When `halfpel` is true the component undergoes an arithmetic
/// right-shift by 1 (`sar eax, 0x1` in the parser); the shifted-
/// out LSB becomes the half-pel offset sub-field used by the
/// spec/02 §3.3 packing formula. When false the component passes
/// through unmodified and the LSB sub-field is zero.
///
/// The arithmetic shift on a signed byte sign-extends, so −1 (0xff)
/// becomes −1 (0xffff_ffff sar 1 = 0xffff_ffff = −1).
fn scale_component(raw: i8, halfpel: bool) -> (i16, u8) {
    if halfpel {
        let lsb = (raw as u8) & 0x01;
        let shifted = (raw as i16) >> 1;
        (shifted, lsb)
    } else {
        (raw as i16, 0)
    }
}

// Sanity-check anchor: this module's offsets assume bsh sits at
// byte `FRAME_HEADER_LEN` of the codec frame (spec/01 §3) and
// the NULL-frame sentinel is `NULL_FRAME_DATA_SIZE_BITS`.
#[allow(dead_code)]
const _SPEC_01_BSH_ANCHOR: usize = FRAME_HEADER_LEN;
#[allow(dead_code)]
const _SPEC_01_BSH_LEN: usize = BITSTREAM_HEADER_LEN;
#[allow(dead_code)]
const _SPEC_02_NULL_SENTINEL: u32 = NULL_FRAME_DATA_SIZE_BITS;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::header::{COMBINED_HEADER_LEN, MAGIC_FRMH, REQUIRED_DEC_VERSION};

    /// Build a codec-frame buffer whose header parses cleanly and
    /// whose per-plane preludes match the supplied test data.
    ///
    /// `plane_offsets` are the three plane offsets to write into
    /// the bsh `y/v/u` slots (in that order). `plane_data[i]` is
    /// the raw prelude bytes to splice in at
    /// `bsh + plane_offsets[i]`. The header's `data_size` is set
    /// to `data_size_bits`.
    fn build_frame_with_planes(
        plane_offsets: [u32; PLANE_COUNT],
        plane_data: [&[u8]; PLANE_COUNT],
        frame_flags: u16,
        data_size_bits: u32,
    ) -> Vec<u8> {
        let bsh_base = FRAME_HEADER_LEN;
        // Compute the buffer size needed to hold every plane's
        // prelude.
        let mut max_end = COMBINED_HEADER_LEN;
        for i in 0..PLANE_COUNT {
            let off = plane_offsets[i];
            if (off as i32) >= 0 {
                let end = bsh_base + off as usize + plane_data[i].len();
                if end > max_end {
                    max_end = end;
                }
            }
        }
        let mut buf = vec![0u8; max_end];

        // Frame header (§2) — fixed values + recomputed checksum.
        let frame_number: u32 = 0;
        let unknown1: u32 = 0;
        let frame_size: u32 = max_end as u32;
        let check_sum = frame_number ^ unknown1 ^ frame_size ^ MAGIC_FRMH;
        buf[0x00..0x04].copy_from_slice(&frame_number.to_le_bytes());
        buf[0x04..0x08].copy_from_slice(&unknown1.to_le_bytes());
        buf[0x08..0x0c].copy_from_slice(&check_sum.to_le_bytes());
        buf[0x0c..0x10].copy_from_slice(&frame_size.to_le_bytes());

        // Bitstream header (§3).
        let bsh = &mut buf[bsh_base..bsh_base + BITSTREAM_HEADER_LEN];
        bsh[0x00..0x02].copy_from_slice(&REQUIRED_DEC_VERSION.to_le_bytes());
        bsh[0x02..0x04].copy_from_slice(&frame_flags.to_le_bytes());
        bsh[0x04..0x08].copy_from_slice(&data_size_bits.to_le_bytes());
        bsh[0x10..0x14].copy_from_slice(&plane_offsets[0].to_le_bytes());
        bsh[0x14..0x18].copy_from_slice(&plane_offsets[1].to_le_bytes());
        bsh[0x18..0x1c].copy_from_slice(&plane_offsets[2].to_le_bytes());

        // Plane data — splice each plane's prelude bytes into
        // place at bsh + plane_offset.
        for i in 0..PLANE_COUNT {
            let off = plane_offsets[i];
            if (off as i32) >= 0 && !plane_data[i].is_empty() {
                let start = bsh_base + off as usize;
                let end = start + plane_data[i].len();
                buf[start..end].copy_from_slice(plane_data[i]);
            }
        }

        buf
    }

    /// Build the prelude bytes for a single plane:
    /// `num_vectors` u32 LE followed by `num_vectors` pairs of
    /// `(vertical, horizontal)` signed bytes.
    fn prelude_bytes(num_vectors: u32, mv_pairs: &[(i8, i8)]) -> Vec<u8> {
        assert_eq!(num_vectors as usize, mv_pairs.len());
        let mut out = Vec::with_capacity(NUM_VECTORS_FIELD_LEN + mv_pairs.len() * 2);
        out.extend_from_slice(&num_vectors.to_le_bytes());
        for (v, h) in mv_pairs {
            out.push(*v as u8);
            out.push(*h as u8);
        }
        out
    }

    #[test]
    fn plane_idx_constants_match_spec() {
        // §2 — count-down loop counter starts at 2 = U and ends
        // at 0 = Y.
        assert_eq!(PLANE_IDX_U, 2);
        assert_eq!(PLANE_IDX_V, 1);
        assert_eq!(PLANE_IDX_Y, 0);
        assert_eq!(PLANE_COUNT, 3);
    }

    #[test]
    fn null_frame_skips_all_planes() {
        let buf = build_frame_with_planes(
            [0x30, 0x40, 0x50],
            [&[], &[], &[]],
            0,
            NULL_FRAME_DATA_SIZE_BITS,
        );
        let header = FrameHeader::parse(&buf).expect("null header must parse");
        let layer = PictureLayer::parse(&header, &buf).expect("null layer must parse");

        for plane in &layer.planes {
            assert_eq!(plane, &PlanePresence::NullFrame);
            assert!(!plane.is_present());
            assert!(plane.as_prelude().is_none());
        }
    }

    #[test]
    fn intra_frame_three_present_planes_zero_vectors() {
        // INTRA frames have num_vectors == 0 for every plane
        // (§3.1, per the wiki note "Contains '0' for INTRA
        // frames").
        let y_prelude = prelude_bytes(0, &[]);
        let v_prelude = prelude_bytes(0, &[]);
        let u_prelude = prelude_bytes(0, &[]);
        let buf = build_frame_with_planes(
            [0x30, 0x34, 0x38],
            [&y_prelude, &v_prelude, &u_prelude],
            0x0005, // PERIODIC_INTRA | INTRA
            0x4000, // data_size in bits, well above any plane offset
        );
        let header = FrameHeader::parse(&buf).expect("intra header must parse");
        let layer = PictureLayer::parse(&header, &buf).expect("intra layer must parse");

        for (plane_idx, presence) in layer.planes.iter().enumerate() {
            let prelude = presence
                .as_prelude()
                .unwrap_or_else(|| panic!("plane {plane_idx} must be Present"));
            assert_eq!(prelude.num_vectors, 0);
            assert!(prelude.motion_vectors.is_empty());
            assert_eq!(prelude.prelude_len(), 4);
        }

        // §3.4 — bitstream_offset = plane_base + 4 + 0.
        assert_eq!(
            layer.y().as_prelude().unwrap().bitstream_offset,
            FRAME_HEADER_LEN + 0x30 + 4
        );
        assert_eq!(
            layer.v().as_prelude().unwrap().bitstream_offset,
            FRAME_HEADER_LEN + 0x34 + 4
        );
        assert_eq!(
            layer.u().as_prelude().unwrap().bitstream_offset,
            FRAME_HEADER_LEN + 0x38 + 4
        );
    }

    #[test]
    fn inter_frame_parses_motion_vectors_per_plane() {
        // Distinct num_vectors per plane to confirm the parser
        // doesn't share state across planes.
        let y_pairs: &[(i8, i8)] = &[(2, 3), (-4, 5), (-6, -7)];
        let v_pairs: &[(i8, i8)] = &[(1, -1)];
        let u_pairs: &[(i8, i8)] = &[];
        let y_prelude = prelude_bytes(3, y_pairs);
        let v_prelude = prelude_bytes(1, v_pairs);
        let u_prelude = prelude_bytes(0, u_pairs);

        let buf = build_frame_with_planes(
            [0x30, 0x40, 0x50],
            [&y_prelude, &v_prelude, &u_prelude],
            0,      // INTER, full-pel (bits 4 and 5 clear)
            0x4000, // data_size in bits
        );
        let header = FrameHeader::parse(&buf).expect("header must parse");
        let layer = PictureLayer::parse(&header, &buf).expect("layer must parse");

        let y = layer.y().as_prelude().expect("Y present");
        assert_eq!(y.num_vectors, 3);
        assert_eq!(y.motion_vectors.len(), 3);
        for (i, (v, h)) in y_pairs.iter().enumerate() {
            assert_eq!(y.motion_vectors[i].vertical_raw, *v);
            assert_eq!(y.motion_vectors[i].horizontal_raw, *h);
            // Full-pel: scaled == raw, halfpel bits = 0.
            assert_eq!(y.motion_vectors[i].vertical_scaled, *v as i16);
            assert_eq!(y.motion_vectors[i].horizontal_scaled, *h as i16);
            assert_eq!(y.motion_vectors[i].vertical_halfpel_bit, 0);
            assert_eq!(y.motion_vectors[i].horizontal_halfpel_bit, 0);
        }
        assert_eq!(y.prelude_len(), 4 + 3 * 2);
        assert_eq!(y.bitstream_offset, FRAME_HEADER_LEN + 0x30 + 4 + 3 * 2);

        let v = layer.v().as_prelude().expect("V present");
        assert_eq!(v.num_vectors, 1);
        assert_eq!(v.motion_vectors[0].vertical_raw, 1);
        assert_eq!(v.motion_vectors[0].horizontal_raw, -1);
        assert_eq!(v.bitstream_offset, FRAME_HEADER_LEN + 0x40 + 4 + 2);

        let u = layer.u().as_prelude().expect("U present");
        assert_eq!(u.num_vectors, 0);
        assert!(u.motion_vectors.is_empty());
        assert_eq!(u.bitstream_offset, FRAME_HEADER_LEN + 0x50 + 4);
    }

    #[test]
    fn iteration_order_is_u_then_v_then_y() {
        // §2 — decode order is plane_idx 2, 1, 0.
        let buf = build_frame_with_planes(
            [0x30, 0x34, 0x38],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
            ],
            0,
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");
        let order: Vec<usize> = layer.iter_in_decode_order().map(|(i, _)| i).collect();
        assert_eq!(order, vec![PLANE_IDX_U, PLANE_IDX_V, PLANE_IDX_Y]);
    }

    #[test]
    fn negative_plane_offset_skips_plane() {
        // §2 — `plane_offset < 0` (i32). Set V-plane offset to
        // 0xffff_ffff (= -1) to trigger the skip.
        let buf = build_frame_with_planes(
            [0x30, 0xffff_ffff, 0x38],
            [&prelude_bytes(0, &[]), &[], &prelude_bytes(0, &[])],
            0,
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");
        assert!(layer.y().is_present());
        match layer.v() {
            PlanePresence::SkippedNegativeOffset { plane_offset } => {
                assert_eq!(*plane_offset, 0xffff_ffff);
            }
            other => panic!("expected SkippedNegativeOffset, got {other:?}"),
        }
        assert!(layer.u().is_present());
    }

    #[test]
    fn offset_above_data_budget_skips_plane() {
        // §2 — `plane_offset > data_size / 8`. With
        // data_size = 0x4000 bits → 0x800 byte budget; an
        // offset of 0x900 exceeds it.
        let buf = build_frame_with_planes(
            [0x900, 0x38, 0x3c],
            [&[], &prelude_bytes(0, &[]), &prelude_bytes(0, &[])],
            0,
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");
        match layer.y() {
            PlanePresence::SkippedAboveDataBudget {
                plane_offset,
                budget_bytes,
            } => {
                assert_eq!(*plane_offset, 0x900);
                assert_eq!(*budget_bytes, 0x4000 / 8);
            }
            other => panic!("expected SkippedAboveDataBudget, got {other:?}"),
        }
        assert!(layer.v().is_present());
        assert!(layer.u().is_present());
    }

    #[test]
    fn offset_exactly_equal_to_data_budget_is_accepted() {
        // §2 — the spec uses `<=` for the upper bound; offset
        // equal to the budget should *not* be skipped (the
        // parser's `jle` triggers only when offset is strictly
        // less than or equal — i.e. the skip is `jle`, meaning
        // skip-if-budget-is-less-than-or-equal-to-offset; with
        // budget == offset the skip fires per the spec text).
        //
        // Spec wording: "a plane whose offset exceeds this
        // budget is skipped". Equal is the boundary; spec
        // wording "exceeds" implies strict greater-than. Test
        // pins the parser's interpretation as `>` (strict).
        let bsh_room = BITSTREAM_HEADER_LEN as u32;
        let budget = bsh_room; // pick a plane_offset == budget
        let buf = build_frame_with_planes(
            [budget, budget + 8, budget + 16],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
            ],
            0,
            budget * 8, // data_size in bits → byte budget = `budget`
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");
        // Y at exactly budget — accepted.
        assert!(layer.y().is_present());
        // V/U above budget — skipped.
        assert!(matches!(
            layer.v(),
            PlanePresence::SkippedAboveDataBudget { .. }
        ));
        assert!(matches!(
            layer.u(),
            PlanePresence::SkippedAboveDataBudget { .. }
        ));
    }

    #[test]
    fn halfpel_horiz_shifts_horizontal_components() {
        // Bit 4 = MV_HALFPEL_HORIZ.
        // raw horizontal -3 = 0xfd. arithmetic >> 1 = -2
        // (sign-extending right shift). lsb = 1.
        let pairs: &[(i8, i8)] = &[(4, -3), (-2, 7)];
        let prelude = prelude_bytes(2, pairs);
        let buf = build_frame_with_planes(
            [0x30, 0x40, 0x50],
            [&prelude, &prelude_bytes(0, &[]), &prelude_bytes(0, &[])],
            0x0010, // MV_HALFPEL_HORIZ
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");
        let y = layer.y().as_prelude().expect("Y present");
        // First entry: horizontal = -3 → shifted -2, lsb 1.
        assert_eq!(y.motion_vectors[0].vertical_raw, 4);
        assert_eq!(y.motion_vectors[0].horizontal_raw, -3);
        assert_eq!(y.motion_vectors[0].vertical_scaled, 4);
        assert_eq!(y.motion_vectors[0].horizontal_scaled, -2);
        assert_eq!(y.motion_vectors[0].vertical_halfpel_bit, 0);
        assert_eq!(y.motion_vectors[0].horizontal_halfpel_bit, 1);
        // Second entry: horizontal = 7 → shifted 3, lsb 1.
        assert_eq!(y.motion_vectors[1].horizontal_raw, 7);
        assert_eq!(y.motion_vectors[1].horizontal_scaled, 3);
        assert_eq!(y.motion_vectors[1].horizontal_halfpel_bit, 1);
        // Vertical untouched (full-pel).
        assert_eq!(y.motion_vectors[1].vertical_raw, -2);
        assert_eq!(y.motion_vectors[1].vertical_scaled, -2);
        assert_eq!(y.motion_vectors[1].vertical_halfpel_bit, 0);
    }

    #[test]
    fn halfpel_vert_shifts_vertical_components() {
        // Bit 5 = MV_HALFPEL_VERT.
        let pairs: &[(i8, i8)] = &[(5, 0), (-1, 0)];
        let prelude = prelude_bytes(2, pairs);
        let buf = build_frame_with_planes(
            [0x30, 0x40, 0x50],
            [&prelude, &prelude_bytes(0, &[]), &prelude_bytes(0, &[])],
            0x0020, // MV_HALFPEL_VERT only
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");
        let y = layer.y().as_prelude().expect("Y present");
        // Vertical 5 → 2, lsb 1.
        assert_eq!(y.motion_vectors[0].vertical_scaled, 2);
        assert_eq!(y.motion_vectors[0].vertical_halfpel_bit, 1);
        // Vertical -1 → arithmetic sar of 0xff = 0xff = -1, lsb 1.
        assert_eq!(y.motion_vectors[1].vertical_raw, -1);
        assert_eq!(y.motion_vectors[1].vertical_scaled, -1);
        assert_eq!(y.motion_vectors[1].vertical_halfpel_bit, 1);
        // Horizontal untouched.
        assert_eq!(y.motion_vectors[0].horizontal_scaled, 0);
        assert_eq!(y.motion_vectors[0].horizontal_halfpel_bit, 0);
    }

    #[test]
    fn halfpel_both_axes_shifts_both_components() {
        let pairs: &[(i8, i8)] = &[(6, -5)];
        let prelude = prelude_bytes(1, pairs);
        let buf = build_frame_with_planes(
            [0x30, 0x40, 0x50],
            [&prelude, &prelude_bytes(0, &[]), &prelude_bytes(0, &[])],
            0x0030, // MV_HALFPEL_VERT | MV_HALFPEL_HORIZ
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");
        let y = layer.y().as_prelude().expect("Y present");
        // 6 → 3 lsb 0; -5 → arithmetic >> 1 = -3, lsb 1.
        assert_eq!(y.motion_vectors[0].vertical_scaled, 3);
        assert_eq!(y.motion_vectors[0].vertical_halfpel_bit, 0);
        assert_eq!(y.motion_vectors[0].horizontal_scaled, -3);
        assert_eq!(y.motion_vectors[0].horizontal_halfpel_bit, 1);
    }

    #[test]
    fn packed_mv_formula_matches_spec_3_3() {
        // §3.3 packing formula:
        // packed_mv = ((vert_shifted * 11) << 4 + horiz_shifted) << 2
        //           + (horiz_lsb << 1) + vert_lsb
        let mv = MotionVector {
            vertical_raw: -3,
            horizontal_raw: 5,
            vertical_scaled: -2,  // -3 >> 1
            horizontal_scaled: 2, // 5 >> 1
            vertical_halfpel_bit: 1,
            horizontal_halfpel_bit: 1,
        };
        let expected: i32 = ((((-2_i32) * 11) << 4) + 2_i32) << 2 | (1 << 1) | 1;
        assert_eq!(mv.packed_mv(), expected);

        // Full-pel zero check.
        let zero = MotionVector {
            vertical_raw: 0,
            horizontal_raw: 0,
            vertical_scaled: 0,
            horizontal_scaled: 0,
            vertical_halfpel_bit: 0,
            horizontal_halfpel_bit: 0,
        };
        assert_eq!(zero.packed_mv(), 0);
    }

    #[test]
    fn plane_offset_overrunning_buffer_returns_out_of_range() {
        // Build a buffer just big enough for the header. Then
        // overwrite Y's offset to point past the end of the
        // input buffer. This should fail PlaneOffsetOutOfRange.
        let mut buf = build_frame_with_planes(
            [0x30, 0x34, 0x38],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
            ],
            0,
            0x4000_0000, // gigantic data_size so the budget check passes
        );
        // Y offset = buf.len() - 16 + 1 → plane base lands at
        // buf.len() - 15, leaving < 4 bytes; num_vectors read
        // overruns.
        let bad_y_offset = (buf.len() - FRAME_HEADER_LEN + 1) as u32;
        buf[FRAME_HEADER_LEN + 0x10..FRAME_HEADER_LEN + 0x14]
            .copy_from_slice(&bad_y_offset.to_le_bytes());
        // Recompute checksum after editing the bsh (frame
        // header itself is unchanged so check_sum still matches
        // — bsh edits don't affect §2.1).
        let header = FrameHeader::parse(&buf).expect("header still parses");
        let err = PictureLayer::parse(&header, &buf).unwrap_err();
        match err {
            PictureLayerError::PlaneOffsetOutOfRange {
                plane_idx,
                plane_offset,
                ..
            } => {
                assert_eq!(plane_idx, PLANE_IDX_Y);
                assert_eq!(plane_offset, bad_y_offset);
            }
            other => panic!("expected PlaneOffsetOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn truncated_motion_vector_array_returns_error() {
        // Build a minimal buffer whose Y plane claims more
        // motion vectors than fit. We construct the buffer by
        // hand (build_frame_with_planes pads to fit, defeating
        // the truncation test).
        let bsh_base = FRAME_HEADER_LEN;
        let plane_offset: u32 = 0x30;
        // Buffer that ends exactly 6 bytes after the plane
        // base — room for the u32 num_vectors and one full
        // mc_vector pair (2 bytes), but the claimed count is
        // larger.
        let buf_len = bsh_base + plane_offset as usize + 4 + 2;
        let mut buf = vec![0u8; buf_len];

        // Frame header (§2) with a recomputed checksum so
        // FrameHeader::parse accepts the buffer.
        let frame_number: u32 = 0;
        let unknown1: u32 = 0;
        let frame_size: u32 = buf_len as u32;
        let check_sum = frame_number ^ unknown1 ^ frame_size ^ MAGIC_FRMH;
        buf[0x00..0x04].copy_from_slice(&frame_number.to_le_bytes());
        buf[0x04..0x08].copy_from_slice(&unknown1.to_le_bytes());
        buf[0x08..0x0c].copy_from_slice(&check_sum.to_le_bytes());
        buf[0x0c..0x10].copy_from_slice(&frame_size.to_le_bytes());

        // Bitstream header (§3) — Y at 0x30, V and U at huge
        // offsets so they trip the budget skip (no truncation
        // error for them).
        let bsh = &mut buf[bsh_base..bsh_base + BITSTREAM_HEADER_LEN];
        bsh[0x00..0x02].copy_from_slice(&REQUIRED_DEC_VERSION.to_le_bytes());
        bsh[0x02..0x04].copy_from_slice(&0u16.to_le_bytes());
        bsh[0x04..0x08].copy_from_slice(&0x4000u32.to_le_bytes());
        bsh[0x10..0x14].copy_from_slice(&plane_offset.to_le_bytes());
        bsh[0x14..0x18].copy_from_slice(&0x0fff_ffffu32.to_le_bytes());
        bsh[0x18..0x1c].copy_from_slice(&0x0fff_ffffu32.to_le_bytes());

        // Y plane's num_vectors = 5 (claims 10 bytes of array)
        // — but only 2 trailing bytes exist in the buffer.
        let plane_base = bsh_base + plane_offset as usize;
        buf[plane_base..plane_base + 4].copy_from_slice(&5u32.to_le_bytes());

        let header = FrameHeader::parse(&buf).expect("header");
        let err = PictureLayer::parse(&header, &buf).unwrap_err();
        match err {
            PictureLayerError::MotionVectorArrayTruncated {
                plane_idx,
                num_vectors,
                array_end,
                buffer_len,
                ..
            } => {
                assert_eq!(plane_idx, PLANE_IDX_Y);
                assert_eq!(num_vectors, 5);
                assert!(array_end > buffer_len);
            }
            other => panic!("expected MotionVectorArrayTruncated, got {other:?}"),
        }
    }

    #[test]
    fn plane_decode_plan_for_present_intra_y_plane_uses_luma_geometry() {
        // §4 — luma plane uses StripGeometry::for_luma with the
        // picture's full luma (width, height).
        let y_prelude = prelude_bytes(0, &[]);
        let v_prelude = prelude_bytes(0, &[]);
        let u_prelude = prelude_bytes(0, &[]);
        let mut buf = build_frame_with_planes(
            [0x30, 0x34, 0x38],
            [&y_prelude, &v_prelude, &u_prelude],
            0x0005, // INTRA, full-pel, primary buffer (bit 9 clear)
            0x4000,
        );
        // Override width / height to spec/02 §4.2 row 3 — 320×240.
        // The frame-header checksum at bsh edits is not consulted; we
        // patch the bsh in place and re-parse.
        let bsh_off = FRAME_HEADER_LEN;
        buf[bsh_off + 0x0c..bsh_off + 0x0e].copy_from_slice(&240u16.to_le_bytes());
        buf[bsh_off + 0x0e..bsh_off + 0x10].copy_from_slice(&320u16.to_le_bytes());

        let header = FrameHeader::parse(&buf).expect("header parses");
        let layer = PictureLayer::parse(&header, &buf).expect("layer parses");

        let plan = layer
            .plane_decode_plan(PLANE_IDX_Y, &header, false)
            .expect("Y plan exists");
        assert_eq!(plan.plane_idx, PLANE_IDX_Y);
        assert!(plan.is_luma());
        assert!(!plan.is_chroma());
        assert!(plan.is_intra());
        assert_eq!(plan.plane_width, 320);
        assert_eq!(plan.plane_height, 240);
        assert_eq!(plan.num_vectors, 0);
        // §4.2 row 3 with W = 320 → strip_count = 2, aligned.
        assert_eq!(plan.geometry.strip_count, 2);
        assert_eq!(plan.geometry.strip_width, 160);
        assert_eq!(plan.geometry.last_strip_width, 160);
        // §3.4 + round-2 — bitstream_offset = bsh + plane_offset + 4.
        assert_eq!(plan.bitstream_offset, FRAME_HEADER_LEN + 0x30 + 4);
        // §5.1 — primary bank, Y → slot 3.
        assert_eq!(plan.slot_descriptor.slot_idx, 3);
        assert_eq!(plan.slot_descriptor.strip_height, 240);
        // For strip_count > 1 the slot's STRIP_WIDTH field carries
        // the per-plane-class strip-width constant (160), not the
        // remainder.
        assert_eq!(plan.slot_descriptor.strip_width, 160);
    }

    #[test]
    fn plane_decode_plan_for_present_chroma_plane_uses_subsampled_geometry() {
        // §4 — chroma plane uses StripGeometry::for_chroma with
        // (luma_width/4, chroma_plane_height(luma_height)).
        let y_prelude = prelude_bytes(0, &[]);
        let v_prelude = prelude_bytes(2, &[(1, 2), (-3, -4)]);
        let u_prelude = prelude_bytes(0, &[]);
        let mut buf = build_frame_with_planes(
            [0x30, 0x40, 0x50],
            [&y_prelude, &v_prelude, &u_prelude],
            0, // INTER, full-pel, primary buffer (bit 9 clear)
            0x4000,
        );
        let bsh_off = FRAME_HEADER_LEN;
        buf[bsh_off + 0x0c..bsh_off + 0x0e].copy_from_slice(&240u16.to_le_bytes());
        buf[bsh_off + 0x0e..bsh_off + 0x10].copy_from_slice(&320u16.to_le_bytes());

        let header = FrameHeader::parse(&buf).expect("header parses");
        let layer = PictureLayer::parse(&header, &buf).expect("layer parses");

        // V plane → chroma; (320/4, (240/4) & !0x3) = (80, 60).
        let plan = layer
            .plane_decode_plan(PLANE_IDX_V, &header, false)
            .expect("V plan exists");
        assert_eq!(plan.plane_idx, PLANE_IDX_V);
        assert!(plan.is_chroma());
        assert!(!plan.is_luma());
        assert!(!plan.is_intra());
        assert_eq!(plan.num_vectors, 2);
        assert_eq!(plan.plane_width, 80);
        assert_eq!(plan.plane_height, 60);
        // §4.1 — chroma strip width 40; 80/40 = 2 strips, aligned.
        assert_eq!(plan.geometry.strip_width, 40);
        assert_eq!(plan.geometry.strip_count, 2);
        assert_eq!(plan.geometry.last_strip_width, 40);
        // §3.4 — bitstream_offset for V = bsh + 0x40 + 4 + 2*2 = +8.
        assert_eq!(plan.bitstream_offset, FRAME_HEADER_LEN + 0x40 + 4 + 4);
        // §5.1 — primary bank, V → slot 4.
        assert_eq!(plan.slot_descriptor.slot_idx, 4);
        assert_eq!(plan.slot_descriptor.strip_height, 60);
    }

    #[test]
    fn plane_decode_plan_remainder_strip_width_single_strip_uses_picture_width() {
        // §4.2 row 1 — picture width ≤ 160 → 1 luma strip whose
        // width equals the picture width itself. The slot
        // descriptor's STRIP_WIDTH field carries the §4.1 remainder
        // (= picture width) in that case.
        let buf = build_frame_with_planes(
            [0x30, 0x34, 0x38],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
            ],
            0,
            0x4000,
        );
        let mut buf = buf;
        let bsh_off = FRAME_HEADER_LEN;
        // 144 × 112 — smaller than 160 luma strip width.
        buf[bsh_off + 0x0c..bsh_off + 0x0e].copy_from_slice(&112u16.to_le_bytes());
        buf[bsh_off + 0x0e..bsh_off + 0x10].copy_from_slice(&144u16.to_le_bytes());

        let header = FrameHeader::parse(&buf).expect("header parses");
        let layer = PictureLayer::parse(&header, &buf).expect("layer parses");

        let plan = layer
            .plane_decode_plan(PLANE_IDX_Y, &header, false)
            .expect("Y plan exists");
        assert_eq!(plan.geometry.strip_count, 1);
        // Remainder formula: ((144-1) mod 160) + 1 = 144.
        assert_eq!(plan.geometry.last_strip_width, 144);
        // Single-strip plane → slot's STRIP_WIDTH field carries the
        // remainder (= picture width), not the 160 constant.
        assert_eq!(plan.slot_descriptor.strip_width, 144);
    }

    #[test]
    fn plane_decode_plan_secondary_bank_remaps_slot_index() {
        // §5.1 — secondary bank (frame_flags bit 9 set) routes
        // (Y, V, U) → slots (0, 1, 2). Passing buffer_selector =
        // true to plane_decode_plan flips the slot index for the
        // same plane.
        let mut buf = build_frame_with_planes(
            [0x30, 0x34, 0x38],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
            ],
            0x0205, // BUFFER_SELECTOR bit 9 set + INTRA bits
            0x4000,
        );
        let bsh_off = FRAME_HEADER_LEN;
        buf[bsh_off + 0x0c..bsh_off + 0x0e].copy_from_slice(&240u16.to_le_bytes());
        buf[bsh_off + 0x0e..bsh_off + 0x10].copy_from_slice(&320u16.to_le_bytes());

        let header = FrameHeader::parse(&buf).expect("header parses");
        let layer = PictureLayer::parse(&header, &buf).expect("layer parses");

        // Primary bank → Y = 3, V = 4, U = 5.
        assert_eq!(
            layer
                .plane_decode_plan(PLANE_IDX_Y, &header, false)
                .unwrap()
                .slot_descriptor
                .slot_idx,
            3
        );
        // Secondary bank → Y = 0, V = 1, U = 2.
        assert_eq!(
            layer
                .plane_decode_plan(PLANE_IDX_Y, &header, true)
                .unwrap()
                .slot_descriptor
                .slot_idx,
            0
        );
        assert_eq!(
            layer
                .plane_decode_plan(PLANE_IDX_V, &header, true)
                .unwrap()
                .slot_descriptor
                .slot_idx,
            1
        );
        assert_eq!(
            layer
                .plane_decode_plan(PLANE_IDX_U, &header, true)
                .unwrap()
                .slot_descriptor
                .slot_idx,
            2
        );
    }

    #[test]
    fn plane_decode_plan_returns_none_for_null_frame_planes() {
        // §1 — NULL frame skips plane iteration; every plane is
        // PlanePresence::NullFrame. plane_decode_plan must return
        // None for all three plane indices.
        let buf = build_frame_with_planes(
            [0x30, 0x40, 0x50],
            [&[], &[], &[]],
            0,
            NULL_FRAME_DATA_SIZE_BITS,
        );
        let header = FrameHeader::parse(&buf).expect("null header parses");
        let layer = PictureLayer::parse(&header, &buf).expect("null layer parses");
        assert!(
            layer
                .plane_decode_plan(PLANE_IDX_Y, &header, false)
                .is_none()
        );
        assert!(
            layer
                .plane_decode_plan(PLANE_IDX_V, &header, false)
                .is_none()
        );
        assert!(
            layer
                .plane_decode_plan(PLANE_IDX_U, &header, false)
                .is_none()
        );
    }

    #[test]
    fn plane_decode_plan_returns_none_for_skipped_plane() {
        // §2 — a plane with a negative offset is skipped; no decode
        // plan exists for it.
        let buf = build_frame_with_planes(
            [0x30, 0xffff_ffff, 0x38],
            [&prelude_bytes(0, &[]), &[], &prelude_bytes(0, &[])],
            0,
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header parses");
        let layer = PictureLayer::parse(&header, &buf).expect("layer parses");
        assert!(layer.y().is_present());
        assert!(
            layer
                .plane_decode_plan(PLANE_IDX_Y, &header, false)
                .is_some()
        );
        // V is skipped — no plan.
        assert!(
            layer
                .plane_decode_plan(PLANE_IDX_V, &header, false)
                .is_none()
        );
        assert!(layer.u().is_present());
        assert!(
            layer
                .plane_decode_plan(PLANE_IDX_U, &header, false)
                .is_some()
        );
    }

    #[test]
    fn plane_decode_plan_rejects_out_of_range_plane_idx() {
        let buf = build_frame_with_planes(
            [0x30, 0x34, 0x38],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
            ],
            0,
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header parses");
        let layer = PictureLayer::parse(&header, &buf).expect("layer parses");
        assert!(
            layer
                .plane_decode_plan(PLANE_COUNT, &header, false)
                .is_none()
        );
        assert!(
            layer
                .plane_decode_plan(usize::MAX, &header, false)
                .is_none()
        );
    }

    #[test]
    fn to_decode_call_bridges_luma_plan_to_seven_arg_frame() {
        // §6 — bridging a PRIMARY-bank luma plan must produce a
        // PerPlaneDecodeCall whose codebook bank is the luma offset
        // (`+0x1a00`), whose slot_idx_src == slot_idx_dst == 3 (§5.1
        // primary Y), and whose §6 4th-argument bitstream-payload
        // offset is set to the plan's §3.4 bitstream_offset with no
        // transformation.
        let buf = build_frame_with_planes(
            [0x30, 0x34, 0x38],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
            ],
            0x0005, // INTRA + primary bank
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header parses");
        let layer = PictureLayer::parse(&header, &buf).expect("layer parses");

        let plan = layer
            .plane_decode_plan(PLANE_IDX_Y, &header, false)
            .expect("Y plan exists");
        let call = plan.to_decode_call();

        assert_eq!(call.plane_idx, PLANE_IDX_Y);
        assert!(!call.buffer_selector);
        assert_eq!(call.slot_idx_src, plan.slot_descriptor.slot_idx);
        assert_eq!(call.slot_idx_dst, plan.slot_descriptor.slot_idx);
        assert_eq!(call.slot_idx_src, 3);
        assert_eq!(call.bitstream_payload_offset, plan.bitstream_offset);
        assert_eq!(
            call.codebook_bank_offset,
            crate::indeo3::INSTANCE_LUMA_CODEBOOK_BANK
        );
        assert_eq!(
            call.strip_array_view_offset,
            0x300c // INSTANCE_STRIP_ARRAY_VIEW_PTR
        );
        assert_eq!(
            call.secondary_codebook_offset,
            crate::indeo3::INSTANCE_SECONDARY_CODEBOOK_PTR
        );
        assert_eq!(call.instance_state_base_offset, 0);
        assert!(plan.role.is_luma());
        assert!(call.plane_role().is_luma());
    }

    #[test]
    fn to_decode_call_routes_chroma_plan_to_chroma_codebook_bank() {
        // §6 — chroma plans must surface the chroma codebook bank
        // offset (`+0x400`) per the §6 luma-vs-chroma discriminant at
        // `IR32_32.DLL!0x1000458d..0x100045a9`.
        let buf = build_frame_with_planes(
            [0x30, 0x40, 0x50],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(2, &[(1, 2), (-3, -4)]),
                &prelude_bytes(0, &[]),
            ],
            0, // INTER + primary
            0x4000,
        );
        let mut buf = buf;
        let bsh_off = FRAME_HEADER_LEN;
        buf[bsh_off + 0x0c..bsh_off + 0x0e].copy_from_slice(&240u16.to_le_bytes());
        buf[bsh_off + 0x0e..bsh_off + 0x10].copy_from_slice(&320u16.to_le_bytes());
        let header = FrameHeader::parse(&buf).expect("header parses");
        let layer = PictureLayer::parse(&header, &buf).expect("layer parses");

        let v_plan = layer
            .plane_decode_plan(PLANE_IDX_V, &header, false)
            .expect("V plan exists");
        let v_call = v_plan.to_decode_call();
        assert_eq!(v_call.plane_idx, PLANE_IDX_V);
        assert_eq!(
            v_call.codebook_bank_offset,
            crate::indeo3::INSTANCE_CHROMA_CODEBOOK_BANK
        );
        // §5.1 primary V → slot 4.
        assert_eq!(v_call.slot_idx_dst, 4);
        // §3.4 — V's bitstream_offset = bsh + 0x40 + 4 + 2*2.
        assert_eq!(
            v_call.bitstream_payload_offset,
            FRAME_HEADER_LEN + 0x40 + 4 + 4
        );
        assert!(v_call.plane_role().is_chroma());

        // U: primary U → slot 5, chroma bank.
        let u_plan = layer
            .plane_decode_plan(PLANE_IDX_U, &header, false)
            .expect("U plan exists");
        let u_call = u_plan.to_decode_call();
        assert_eq!(u_call.plane_idx, PLANE_IDX_U);
        assert_eq!(
            u_call.codebook_bank_offset,
            crate::indeo3::INSTANCE_CHROMA_CODEBOOK_BANK
        );
        assert_eq!(u_call.slot_idx_dst, 5);
        assert!(u_call.plane_role().is_chroma());
    }

    #[test]
    fn to_decode_call_secondary_bank_routes_slots_to_lower_half() {
        // §5.1 — secondary bank (frame_flags bit 9 set) routes
        // (Y, V, U) → slots (0, 1, 2). The bridge must propagate
        // buffer_selector = true and surface slot index 0 for Y.
        let mut buf = build_frame_with_planes(
            [0x30, 0x34, 0x38],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
            ],
            0x0205, // BUFFER_SELECTOR bit 9 set + INTRA
            0x4000,
        );
        let bsh_off = FRAME_HEADER_LEN;
        buf[bsh_off + 0x0c..bsh_off + 0x0e].copy_from_slice(&240u16.to_le_bytes());
        buf[bsh_off + 0x0e..bsh_off + 0x10].copy_from_slice(&320u16.to_le_bytes());
        let header = FrameHeader::parse(&buf).expect("header parses");
        let layer = PictureLayer::parse(&header, &buf).expect("layer parses");

        let plan = layer
            .plane_decode_plan(PLANE_IDX_Y, &header, true)
            .expect("Y plan with secondary buffer");
        let call = plan.to_decode_call();
        assert!(call.buffer_selector);
        assert_eq!(call.slot_idx_src, 0);
        assert_eq!(call.slot_idx_dst, 0);
        // Even on the secondary bank, the luma codebook bank stays
        // `+0x1a00` — §6 keys it off plane_idx, not the buffer bit.
        assert_eq!(
            call.codebook_bank_offset,
            crate::indeo3::INSTANCE_LUMA_CODEBOOK_BANK
        );
    }

    #[test]
    fn to_decode_call_matches_for_plane_with_full_frameflags() {
        // The bridge constructor must produce a structurally identical
        // call frame to the existing `PerPlaneDecodeCall::for_plane`
        // path that takes a full `FrameFlags`. Cross-check both
        // constructors return the same PerPlaneDecodeCall for the same
        // (plane_idx, buffer_selector, bitstream_payload_offset) triple
        // across all three planes × both banks.
        let mut buf = build_frame_with_planes(
            [0x30, 0x34, 0x38],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
            ],
            0x0005, // primary
            0x4000,
        );
        let bsh_off = FRAME_HEADER_LEN;
        buf[bsh_off + 0x0c..bsh_off + 0x0e].copy_from_slice(&240u16.to_le_bytes());
        buf[bsh_off + 0x0e..bsh_off + 0x10].copy_from_slice(&320u16.to_le_bytes());
        let header = FrameHeader::parse(&buf).expect("header parses");
        let layer = PictureLayer::parse(&header, &buf).expect("layer parses");

        let flags_primary = header.bitstream.frame_flags;
        for plane_idx in [PLANE_IDX_Y, PLANE_IDX_V, PLANE_IDX_U] {
            let plan = layer
                .plane_decode_plan(plane_idx, &header, false)
                .expect("plan exists");
            let via_bridge = plan.to_decode_call();
            let via_flags = crate::indeo3::PerPlaneDecodeCall::for_plane(
                plane_idx,
                flags_primary,
                plan.bitstream_offset,
            )
            .expect("for_plane returns Some for legal plane_idx");
            assert_eq!(via_bridge, via_flags);
        }
    }

    #[test]
    fn plane_byte_map_inter_frame_y_plane_landmarks_match_spec_9() {
        // §9 — for a Y plane with 7 motion vectors at plane_offset
        // 0x30, the byte map exposes:
        //   plane_base       = bsh + 0x30
        //   num_vectors_range = [plane_base, plane_base + 4)
        //   mc_vectors_range  = [plane_base + 4, plane_base + 4 + 14)
        //   payload_start    = plane_base + 4 + 14
        //   payload_upper_bound = next present plane's base
        //                       (V's at 0x80 → bsh + 0x80)
        let pairs: Vec<(i8, i8)> = (0..7).map(|i| (i, -i)).collect();
        let y_prelude = prelude_bytes(7, &pairs);
        let v_prelude = prelude_bytes(0, &[]);
        let u_prelude = prelude_bytes(0, &[]);
        let buf = build_frame_with_planes(
            [0x30, 0x80, 0x90],
            [&y_prelude, &v_prelude, &u_prelude],
            0, // INTER, full-pel
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");

        let map = layer
            .plane_byte_map(PLANE_IDX_Y, &header, buf.len())
            .expect("Y map");
        assert_eq!(map.plane_idx, PLANE_IDX_Y);
        assert_eq!(map.plane_base, FRAME_HEADER_LEN + 0x30);
        assert_eq!(
            map.num_vectors_range,
            (FRAME_HEADER_LEN + 0x30)..(FRAME_HEADER_LEN + 0x30 + 4)
        );
        assert_eq!(
            map.mc_vectors_range,
            (FRAME_HEADER_LEN + 0x30 + 4)..(FRAME_HEADER_LEN + 0x30 + 4 + 14)
        );
        assert_eq!(map.payload_start, FRAME_HEADER_LEN + 0x30 + 4 + 14);
        // §6.1 — next present plane after Y is V at 0x80.
        assert_eq!(map.payload_upper_bound, FRAME_HEADER_LEN + 0x80);
        assert_eq!(map.prelude_len(), 4 + 7 * 2);
        assert_eq!(
            map.payload_budget(),
            (FRAME_HEADER_LEN + 0x80) - (FRAME_HEADER_LEN + 0x30 + 4 + 14)
        );
    }

    #[test]
    fn plane_byte_map_intra_plane_has_empty_mc_range() {
        // §3.1 — INTRA plane has num_vectors == 0, so
        // mc_vectors_range is empty and payload_start follows the
        // num_vectors u32 directly.
        let buf = build_frame_with_planes(
            [0x30, 0x40, 0x50],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
            ],
            0x0005, // INTRA + primary
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");

        let map = layer
            .plane_byte_map(PLANE_IDX_Y, &header, buf.len())
            .expect("Y map");
        assert_eq!(map.num_vectors_range.len(), NUM_VECTORS_FIELD_LEN);
        assert_eq!(map.mc_vectors_range.len(), 0);
        assert!(map.mc_vectors_range.is_empty());
        assert_eq!(map.payload_start, map.plane_base + NUM_VECTORS_FIELD_LEN);
        assert_eq!(map.prelude_len(), 4);
    }

    #[test]
    fn plane_byte_map_last_plane_upper_bound_is_buffer_len() {
        // §6.1 — the LAST plane (by byte ordering — the one with
        // the largest plane_offset) has no later present plane,
        // so its upper bound falls back to the codec-frame
        // buffer length.
        // Build with Y at 0x30, V at 0x40, U at 0x80 (the
        // largest). U is therefore the last by byte order.
        let buf = build_frame_with_planes(
            [0x30, 0x40, 0x80],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(1, &[(2, 3)]),
            ],
            0, // INTER + primary
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");

        // The build_frame_with_planes helper sizes the buffer to
        // exactly hold each plane's prelude with no trailing
        // bytes. Pass a buffer_len value extended by 16 bytes so
        // the §6.1 "buffer_len fallback" is exercised with a
        // realistic non-zero budget (a real-world codec frame
        // would carry the binary-tree payload after the prelude).
        let buf_len_with_room = buf.len() + 16;
        let u_map = layer
            .plane_byte_map(PLANE_IDX_U, &header, buf_len_with_room)
            .expect("U map");
        // No later present plane → upper bound = buffer_len.
        assert_eq!(u_map.payload_upper_bound, buf_len_with_room);
        // Y and V come before U in byte order, so the upper bound
        // for U is *not* affected by them.
        assert!(u_map.payload_upper_bound > u_map.payload_start);
        // payload_budget equals the room we extended by.
        assert_eq!(
            u_map.payload_budget(),
            buf_len_with_room - u_map.payload_start
        );
    }

    #[test]
    fn plane_byte_map_picks_smallest_following_plane_base() {
        // §2 admits any iteration order of planes by plane_offset;
        // the upper bound is the SMALLEST other-plane base
        // strictly greater than this plane's payload_start, not
        // the one matching the iteration order.
        // Y at 0x30, V at 0x80, U at 0x60 — Y's next-by-base is
        // U at 0x60, not V at 0x80.
        let buf = build_frame_with_planes(
            [0x30, 0x80, 0x60],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
            ],
            0x0005, // INTRA + primary
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");
        let y_map = layer
            .plane_byte_map(PLANE_IDX_Y, &header, buf.len())
            .expect("Y map");
        assert_eq!(y_map.payload_upper_bound, FRAME_HEADER_LEN + 0x60);
    }

    #[test]
    fn plane_byte_map_skips_non_present_planes_when_computing_upper_bound() {
        // §2 — a SkippedNegativeOffset plane is not present and
        // must not contribute to the upper bound. Build Y at
        // 0x30, V skipped (offset -1), U at 0x80. Y's upper bound
        // is U's base (0x80), not V's.
        let buf = build_frame_with_planes(
            [0x30, 0xffff_ffff, 0x80],
            [&prelude_bytes(0, &[]), &[], &prelude_bytes(0, &[])],
            0x0005, // INTRA + primary
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");
        // V is skipped → no byte map for V.
        assert!(
            layer
                .plane_byte_map(PLANE_IDX_V, &header, buf.len())
                .is_none()
        );
        let y_map = layer
            .plane_byte_map(PLANE_IDX_Y, &header, buf.len())
            .expect("Y map");
        assert_eq!(y_map.payload_upper_bound, FRAME_HEADER_LEN + 0x80);
    }

    #[test]
    fn plane_byte_map_returns_none_for_null_frame_or_out_of_range() {
        // §1 — NULL frame → every plane is NullFrame → no map.
        let buf = build_frame_with_planes(
            [0x30, 0x40, 0x50],
            [&[], &[], &[]],
            0,
            NULL_FRAME_DATA_SIZE_BITS,
        );
        let header = FrameHeader::parse(&buf).expect("null header");
        let layer = PictureLayer::parse(&header, &buf).expect("null layer");
        for plane_idx in [PLANE_IDX_Y, PLANE_IDX_V, PLANE_IDX_U] {
            assert!(
                layer
                    .plane_byte_map(plane_idx, &header, buf.len())
                    .is_none()
            );
        }
        // Out-of-range plane index → None even on a present layer.
        let buf = build_frame_with_planes(
            [0x30, 0x40, 0x50],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
            ],
            0,
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");
        assert!(
            layer
                .plane_byte_map(PLANE_COUNT, &header, buf.len())
                .is_none()
        );
        assert!(
            layer
                .plane_byte_map(usize::MAX, &header, buf.len())
                .is_none()
        );
    }

    #[test]
    fn plane_byte_map_payload_start_matches_prelude_bitstream_offset() {
        // §9 row 3 — the byte map's payload_start is the same
        // value as the owning PlanePrelude's bitstream_offset.
        // Crosscheck across all three planes for an INTER frame.
        let y_prelude = prelude_bytes(2, &[(1, 2), (3, 4)]);
        let v_prelude = prelude_bytes(1, &[(-1, -2)]);
        let u_prelude = prelude_bytes(3, &[(0, 1), (2, -3), (4, -5)]);
        let buf = build_frame_with_planes(
            [0x30, 0x60, 0x90],
            [&y_prelude, &v_prelude, &u_prelude],
            0,
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");
        for plane_idx in [PLANE_IDX_Y, PLANE_IDX_V, PLANE_IDX_U] {
            let prelude = layer.planes[plane_idx].as_prelude().expect("plane present");
            let map = layer
                .plane_byte_map(plane_idx, &header, buf.len())
                .expect("map");
            assert_eq!(map.payload_start, prelude.bitstream_offset);
            assert_eq!(map.prelude_len(), prelude.prelude_len());
        }
    }

    #[test]
    fn plane_byte_map_clamps_upper_bound_to_payload_start_when_buffer_truncated() {
        // Defensive: if the caller passes a buffer_len smaller
        // than payload_start (a pathological case), the upper
        // bound is clamped to payload_start (yielding a zero
        // payload_budget) rather than producing a Range whose
        // end < start.
        let buf = build_frame_with_planes(
            [0x30, 0x40, 0x50],
            [
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
                &prelude_bytes(0, &[]),
            ],
            0,
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");
        let u_map = layer
            .plane_byte_map(PLANE_IDX_U, &header, 0)
            .expect("U map");
        // buffer_len = 0 < payload_start. Upper bound clamps to
        // payload_start.
        assert_eq!(u_map.payload_upper_bound, u_map.payload_start);
        assert_eq!(u_map.payload_budget(), 0);
    }

    #[test]
    fn byte_map_matches_spec_3_4() {
        // §3.4 — bitstream_offset = plane_base + 4 + 2*num_vectors.
        // Verify for a plane with 7 motion vectors that
        // bitstream_offset = bsh + plane_offset + 4 + 14.
        let pairs: Vec<(i8, i8)> = (0..7).map(|i| (i, -i)).collect();
        let prelude = prelude_bytes(7, &pairs);
        let plane_offset = 0x30_u32;
        let buf = build_frame_with_planes(
            [plane_offset, 0x80, 0x90],
            [&prelude, &prelude_bytes(0, &[]), &prelude_bytes(0, &[])],
            0,
            0x4000,
        );
        let header = FrameHeader::parse(&buf).expect("header");
        let layer = PictureLayer::parse(&header, &buf).expect("layer");
        let y = layer.y().as_prelude().expect("Y present");
        assert_eq!(y.num_vectors, 7);
        assert_eq!(y.motion_vectors.len(), 7);
        assert_eq!(y.prelude_len(), 4 + 7 * 2);
        assert_eq!(
            y.bitstream_offset,
            FRAME_HEADER_LEN + plane_offset as usize + 4 + 7 * 2
        );
        // Each motion vector matches its pair.
        for (i, (v, h)) in pairs.iter().enumerate() {
            assert_eq!(y.motion_vectors[i].vertical_raw, *v);
            assert_eq!(y.motion_vectors[i].horizontal_raw, *h);
        }
    }
}

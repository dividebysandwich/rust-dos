//! Indeo 3 output-buffer write: the spec/07 §4.3 1-bit output
//! upshift, the §5.6 IF09 / YVU9 passthrough surface, and the §5.7
//! strip-to-frame assembly executor.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/07-output-reconstruction.md`.
//!
//! Round 28 lands the output stage the round-27 `frame_exit` handoff
//! targets (`FRAME_OUTPUT_RECONSTRUCTION_RVA`, spec/02 §6.2): once
//! all three planes have decoded into their 0xb0-stride strip pixel
//! buffers (spec/07 §5.1), the YVU-planar output path copies each
//! plane to the host buffer. This module covers, mapped to the
//! spec/07 sections:
//!
//! * §4.3 — the 1-bit upshift from the internal 7-bit-per-byte
//!   representation back to 8-bit output values
//!   ([`upshift_7bit_to_8bit`], `shl byte, 1`). The shift discards
//!   bit 7, so the §4.2 / §4.4 edge-marker sentinel is cleared as a
//!   side effect ([`super::EDGE_MARKER_BIT`] never reaches the output).
//! * §5.3 — the full output-format dispatch decision
//!   ([`select_output_conversion`]): the `sub_4190` selection that
//!   switches on the input `biCompression` (`'IF09'` / [`BI_RGB`] /
//!   [`BI_BITFIELDS`]) and, for the RGB arm, the output `biBitCount`
//!   (8 / 16 / 24, the 24-bpp case split by the colour-space flag),
//!   resolving to one of the seven [`OutputConversion`] variants with
//!   its §5.3-table entry RVA ([`OutputConversion::entry_rva`]). Only
//!   [`OutputConversion::If09Passthrough`] has a landed body (below);
//!   the RGB variants are §5.4-LUT-driven and deferred.
//! * §5.3 / §5.6 — the IF09 / YVU9 passthrough dispatch surface:
//!   the FOURCC the format dispatch compares against
//!   ([`IF09_FOURCC`], referenced at [`IF09_FOURCC_CASE_RVA`]) and
//!   the passthrough conversion function's entry RVA
//!   ([`IF09_PASSTHROUGH_RVA`], the §5.3 table's "IF09 passthrough"
//!   row).
//! * §5.6 step 2 — the output plane order **Y first, then V, then
//!   U** ([`OUTPUT_PLANE_ORDER`]), which is exactly the reverse of
//!   the §5.2 decode-time iteration order
//!   ([`super::PLANE_ITERATION_ORDER`], U → V → Y); a `const _`
//!   cross-check pins the reversal.
//! * §5.7 — the strip-to-frame assembly executor
//!   ([`assemble_plane_if09`]): the conversion-time loop that walks
//!   the plane's strips in left-to-right order, reads each strip's
//!   own 0xb0-stride pixel buffer, applies the §4.3 / §5.6 step 1b
//!   per-byte upshift, and writes the strip's rows into the
//!   corresponding horizontal slice of the caller's output plane
//!   raster. Per §5.7 the assembly happens at conversion time only —
//!   each strip is decoded into its own buffer independently and the
//!   strips meet for the first time here.
//!
//! What this module deliberately does **not** do (the spec/07
//! chapter boundaries):
//!
//! * No YUV → RGB conversion. The §5.3 RGB conversion functions are
//!   LUT-driven (§5.4) and their LUT contents are populated at
//!   codec-init time via register-indirect stores the audit could
//!   not pin (spec/07 §5.4 audit note + §7.2 open question) — the
//!   RGB paths stay deferred until the LUT-population evidence is
//!   staged.
//! * §5.5 — the 4:1:0 → output chroma box-upsampler
//!   ([`upsample_chroma_4x4`]): replicates each chroma sample into a
//!   4×4 block of output positions, the "plain box-filter chroma
//!   upsampling" §5.5 documents (no interpolation, no edge-aware
//!   reconstruction). The IF09 passthrough ([`assemble_plane_if09`])
//!   keeps the chroma planes at their 4:1:0 subsampling (§5.6 closing
//!   paragraph); this standalone upsampler feeds the §5.4 RGB
//!   conversion loops whose LUT bodies stay deferred.
//! * No frame finalisation. The §6 saved-frame-flags / frame-number
//!   state updates and the §6.3 return code are the next chapter
//!   slice above this one.
//! * No plane decode and no strip pixel-buffer ownership. The
//!   caller supplies the decoded strip buffers (spec/05 §4.1
//!   regions); this module only reads them.

use super::mc_arena::MC_ARENA_ROW_STRIDE;
use super::picture_layer::{PLANE_COUNT, PLANE_IDX_U, PLANE_IDX_V, PLANE_IDX_Y};
use super::strip_context::StripGeometry;

/// Spec/07 §4.3 — the output upshift's bit count. "The pixel values
/// shall be shifted one bit left to convert them back to 8-bit
/// values"; the YVU-planar output path applies `shl byte, 1` per
/// byte.
pub const OUTPUT_UPSHIFT_BITS: u32 = 1;

/// Spec/07 §5.6 — the IF09 FOURCC value the output-format dispatch
/// compares against (`'IF09'` = `0x39304649`, little-endian byte
/// order `I`, `F`, `0`, `9`).
pub const IF09_FOURCC: u32 = 0x3930_4649;

// §5.6 — the FOURCC constant spells "IF09" in stream byte order.
const _: () = assert!(IF09_FOURCC == u32::from_le_bytes(*b"IF09"));

/// Spec/07 §5.6 — the RVA of the `case 0x39304649` FOURCC reference
/// in the per-frame decode entry's output-format dispatch.
pub const IF09_FOURCC_CASE_RVA: u32 = 0x1000_4576;

/// Spec/07 §5.3 / §5.6 — the IF09 / YVU9 passthrough conversion
/// function's entry RVA (the §5.3 dispatch-table row "IF09
/// passthrough: 7-bit-to-8-bit upshift + plane copy").
pub const IF09_PASSTHROUGH_RVA: u32 = 0x1000_a53c;

/// Spec/07 §5.6 step 2 — the output-buffer plane order: "Plane
/// order in the output is Y first, then V, then U."
pub const OUTPUT_PLANE_ORDER: [usize; PLANE_COUNT] = [PLANE_IDX_Y, PLANE_IDX_V, PLANE_IDX_U];

// §5.2 vs §5.6 — the output plane order is exactly the reverse of
// the decode-time iteration order (U → V → Y, frame_exit's
// PLANE_ITERATION_ORDER).
const _: () = {
    assert!(OUTPUT_PLANE_ORDER[0] == super::frame_exit::PLANE_ITERATION_ORDER[2]);
    assert!(OUTPUT_PLANE_ORDER[1] == super::frame_exit::PLANE_ITERATION_ORDER[1]);
    assert!(OUTPUT_PLANE_ORDER[2] == super::frame_exit::PLANE_ITERATION_ORDER[0]);
};

// §5.6 — the output plane order is a permutation of 0..PLANE_COUNT.
const _: () = {
    assert!(OUTPUT_PLANE_ORDER[0] != OUTPUT_PLANE_ORDER[1]);
    assert!(OUTPUT_PLANE_ORDER[1] != OUTPUT_PLANE_ORDER[2]);
    assert!(OUTPUT_PLANE_ORDER[0] != OUTPUT_PLANE_ORDER[2]);
    assert!(OUTPUT_PLANE_ORDER[0] < PLANE_COUNT);
    assert!(OUTPUT_PLANE_ORDER[1] < PLANE_COUNT);
    assert!(OUTPUT_PLANE_ORDER[2] < PLANE_COUNT);
};

// ---- §5.3 output-format dispatch -------------------------------------
//
// After all three planes decode, `IR32_32.DLL!sub_4190`
// (`0x10004644..0x10004915`) reads the host's requested output format
// from the output BIH and selects a conversion function pointer into
// `var_24`, then calls it indirectly (`call [var_24]`). The selection
// switches first on the *input* `biCompression` and then, for the
// uncompressed-RGB case, on the *output* `biBitCount`. This module
// models only the dispatch *decision* — which conversion variant the
// host request resolves to, and that variant's entry RVA. The
// conversion arithmetic itself (the §5.4 LUT-driven YUV→RGB matrix)
// stays deferred: its LUTs are populated at codec-init time via
// register-indirect stores the audit could not pin (§5.4 audit note,
// §7.2 open question). The IF09 passthrough row is the one variant
// whose body (`assemble_plane_if09`) is already implemented above.

/// Spec/07 §5.3 — the `biCompression` the host's *input* BIH carries
/// for an uncompressed-RGB output request (the dispatch's
/// `biCompression == 0` arm). `BI_RGB` in the Windows BITMAPINFOHEADER
/// vocabulary.
pub const BI_RGB: u32 = 0;

/// Spec/07 §5.3 — the `biCompression` the host's *input* BIH carries
/// for a bitfield-RGB output request (the dispatch's
/// `biCompression == 3` arm). `BI_BITFIELDS` in the Windows
/// BITMAPINFOHEADER vocabulary.
pub const BI_BITFIELDS: u32 = 3;

/// Spec/07 §5.3 — one selected output-conversion variant.
///
/// The seven variants are the rows of the §5.3 RVA table plus the
/// `biCompression == 3` palette-lookup function. Each carries its
/// conversion-function entry RVA ([`OutputConversion::entry_rva`]);
/// the §5.4 LUT contents the RGB variants index are deferred, so this
/// enum identifies *which* conversion the host requested without
/// performing it. Only [`OutputConversion::If09Passthrough`] has a
/// landed body ([`assemble_plane_if09`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputConversion {
    /// `0x1000a53c` — IF09 / YVU9 planar passthrough (input
    /// `biCompression == 'IF09'`). 7-bit→8-bit upshift + plane copy.
    If09Passthrough,
    /// `0x10008774` — RGB 8-bit indexed, luma-driven palette lookup
    /// (input `BI_RGB`, output `biBitCount == 8`).
    Rgb8Indexed,
    /// `0x10008a50` — RGB 16-bit (5-6-5 or 5-5-5 by colour-space
    /// flag) (input `BI_RGB`, output `biBitCount == 16`).
    Rgb16,
    /// `0x100096fc` — RGB 24-bit, the canonical `IV32 → RGB24` path
    /// (input `BI_RGB`, output `biBitCount == 24`, colour-space flag
    /// clear).
    Rgb24,
    /// `0x10009aa0` — RGB 24-bit alternate variant (input `BI_RGB`,
    /// output `biBitCount == 24`, colour-space flag set).
    Rgb24Alt,
    /// `0x10006060` — bitfield-RGB palette-lookup variant
    /// (`sub_6060`, input `biCompression == 3`). Output bit depth is
    /// taken from the host's field masks rather than `biBitCount`.
    BitfieldPalette,
}

impl OutputConversion {
    /// Spec/07 §5.3 — the conversion function's entry RVA inside
    /// `IR32_32.DLL` (the `var_24` value the dispatch installs and
    /// `call [var_24]` invokes).
    pub const fn entry_rva(self) -> u32 {
        match self {
            OutputConversion::If09Passthrough => IF09_PASSTHROUGH_RVA,
            OutputConversion::Rgb8Indexed => 0x1000_8774,
            OutputConversion::Rgb16 => 0x1000_8a50,
            OutputConversion::Rgb24 => 0x1000_96fc,
            OutputConversion::Rgb24Alt => 0x1000_9aa0,
            OutputConversion::BitfieldPalette => 0x1000_6060,
        }
    }

    /// `true` for the one variant whose conversion body is implemented
    /// in this crate ([`assemble_plane_if09`]). The RGB variants are
    /// §5.4-LUT-driven and deferred until the LUT-population evidence
    /// is staged (§7.2).
    pub const fn is_implemented(self) -> bool {
        matches!(self, OutputConversion::If09Passthrough)
    }
}

/// Spec/07 §5.3 — why an output-format request resolves to no
/// conversion variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputDispatchError {
    /// The input `biCompression` is none of the three the dispatch
    /// switches on (`'IF09'`, `BI_RGB`, `BI_BITFIELDS`).
    UnsupportedCompression {
        /// The host's input `biCompression`.
        compression: u32,
    },
    /// Input `biCompression == BI_RGB`, but the output `biBitCount` is
    /// none of the three RGB depths the dispatch handles (8, 16, 24).
    UnsupportedRgbBitCount {
        /// The host's output `biBitCount`.
        bit_count: u16,
    },
}

/// Spec/07 §5.3 — resolve the host's requested output format to its
/// conversion variant, mirroring the `sub_4190` selection logic.
///
/// `input_compression` is the host's *input* BIH `biCompression`;
/// `output_bit_count` is the *output* BIH `biBitCount` (only consulted
/// for the `BI_RGB` arm); `colour_space_flag` is the palette
/// colour-space bit the 24-bit arm uses to pick between the canonical
/// and alternate RGB24 variants (and which §5.4 notes also selects
/// 5-6-5 vs 5-5-5 inside the 16-bit variant's LUT — that bit-layout
/// choice is internal to [`OutputConversion::Rgb16`] and not split
/// out here).
pub const fn select_output_conversion(
    input_compression: u32,
    output_bit_count: u16,
    colour_space_flag: bool,
) -> Result<OutputConversion, OutputDispatchError> {
    if input_compression == IF09_FOURCC {
        Ok(OutputConversion::If09Passthrough)
    } else if input_compression == BI_RGB {
        match output_bit_count {
            8 => Ok(OutputConversion::Rgb8Indexed),
            16 => Ok(OutputConversion::Rgb16),
            24 => {
                if colour_space_flag {
                    Ok(OutputConversion::Rgb24Alt)
                } else {
                    Ok(OutputConversion::Rgb24)
                }
            }
            other => Err(OutputDispatchError::UnsupportedRgbBitCount { bit_count: other }),
        }
    } else if input_compression == BI_BITFIELDS {
        Ok(OutputConversion::BitfieldPalette)
    } else {
        Err(OutputDispatchError::UnsupportedCompression {
            compression: input_compression,
        })
    }
}

/// Spec/07 §5.3 — the host output `biBitCount` that triggers the
/// per-line stride fix-up at `IR32_32.DLL!sub_5480` (the call at
/// `0x10004901` in `sub_4190`): a 24-bpp output needs the row-by-row
/// copy. The constant lets callers test the fix-up trigger without
/// re-deriving `0x18`.
pub const RGB24_STRIDE_FIXUP_BIT_COUNT: u16 = 24;

/// Spec/07 §5.1 — the strip pixel buffer's allocated row stride the
/// assembly reads at (`0xb0`), aliasing the spec/05 §4.1
/// [`MC_ARENA_ROW_STRIDE`].
pub const FRAME_OUTPUT_SRC_ROW_STRIDE: usize = MC_ARENA_ROW_STRIDE;

// §5.1 — one row stride across the whole pipeline.
const _: () = assert!(FRAME_OUTPUT_SRC_ROW_STRIDE == 0xb0);
const _: () = assert!(FRAME_OUTPUT_SRC_ROW_STRIDE == super::strip_edge::STRIP_EDGE_ROW_STRIDE);

/// Spec/07 §4.3 — upshift one internal 7-bit pixel byte to its
/// 8-bit output value (`shl byte, 1`).
///
/// The shift discards bit 7: per §4.4 the [`super::EDGE_MARKER_BIT`]
/// sentinel "is cleared by the shift" and only the 7-bit content
/// reaches the output. For every byte the result equals
/// `(b & 0x7f) << 1`.
pub const fn upshift_7bit_to_8bit(b: u8) -> u8 {
    b << OUTPUT_UPSHIFT_BITS
}

/// Spec/07 §5.1 / §5.7 — the minimum byte length of one strip's
/// pixel-buffer slice for an assembly walk of `plane_height` rows
/// of `strip_width` visible pixels: the last row only needs its
/// visible pixels, every earlier row advances by the full
/// [`FRAME_OUTPUT_SRC_ROW_STRIDE`].
pub const fn strip_min_buffer_bytes(strip_width: u32, plane_height: u32) -> usize {
    if plane_height == 0 || strip_width == 0 {
        0
    } else {
        (plane_height as usize - 1) * FRAME_OUTPUT_SRC_ROW_STRIDE + strip_width as usize
    }
}

/// Spec/07 §5.7 — failure modes of the strip-to-frame assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlaneAssembleError {
    /// The caller supplied a different number of strip buffers than
    /// the §5.7 / spec/02 §4.1 geometry's `strip_count`.
    StripCountMismatch {
        /// `geometry.strip_count`.
        expected: u32,
        /// `strips.len()`.
        supplied: usize,
    },
    /// The geometry's per-strip widths do not sum to its
    /// `plane_width` — the §5.7 horizontal tiling would leave a gap
    /// or overrun the raster. (Never produced by
    /// [`StripGeometry::for_luma`] / [`StripGeometry::for_chroma`];
    /// guards hand-built geometry values.)
    StripWidthSumMismatch {
        /// `geometry.plane_width`.
        plane_width: u32,
        /// The sum over `geometry.iter_strip_widths()`.
        widths_total: u32,
    },
    /// One strip's width reaches past the strip buffer's allocated
    /// row stride (`0xb0`, spec/07 §5.1) — no such strip layout can
    /// exist.
    StripWidthExceedsRowStride {
        /// Index of the offending strip (left-to-right).
        strip_index: usize,
        /// The supplied strip width (in pixels).
        strip_width: u32,
    },
    /// One strip's pixel-buffer slice is shorter than the
    /// [`strip_min_buffer_bytes`] walk requires.
    StripBufferTooShort {
        /// Index of the offending strip (left-to-right).
        strip_index: usize,
        /// Bytes the walk requires.
        required: usize,
        /// Bytes supplied.
        supplied: usize,
    },
    /// The caller's output row stride is narrower than the plane
    /// width — the §5.7 full-width raster cannot fit one row.
    DstStrideTooNarrow {
        /// `geometry.plane_width`.
        plane_width: u32,
        /// The supplied output row stride (in bytes).
        dst_stride: usize,
    },
    /// The output slice is shorter than the assembled plane
    /// requires.
    DstBufferTooShort {
        /// Bytes the assembled plane requires.
        required: usize,
        /// Bytes supplied.
        supplied: usize,
    },
}

impl core::fmt::Display for PlaneAssembleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PlaneAssembleError::StripCountMismatch { expected, supplied } => write!(
                f,
                "spec/07 §5.7: geometry expects {expected} strip buffer(s); {supplied} supplied"
            ),
            PlaneAssembleError::StripWidthSumMismatch {
                plane_width,
                widths_total,
            } => write!(
                f,
                "spec/07 §5.7: per-strip widths sum to {widths_total}, \
                 not the plane width {plane_width}"
            ),
            PlaneAssembleError::StripWidthExceedsRowStride {
                strip_index,
                strip_width,
            } => write!(
                f,
                "spec/07 §5.1: strip {strip_index} width {strip_width} exceeds \
                 the allocated row stride 0xb0"
            ),
            PlaneAssembleError::StripBufferTooShort {
                strip_index,
                required,
                supplied,
            } => write!(
                f,
                "spec/07 §5.7: strip {strip_index} pixel buffer has {supplied} byte(s); \
                 the assembly walk requires at least {required}"
            ),
            PlaneAssembleError::DstStrideTooNarrow {
                plane_width,
                dst_stride,
            } => write!(
                f,
                "spec/07 §5.7: output row stride {dst_stride} is narrower than \
                 the plane width {plane_width}"
            ),
            PlaneAssembleError::DstBufferTooShort { required, supplied } => write!(
                f,
                "spec/07 §5.7: output slice has {supplied} byte(s); \
                 the assembled plane requires at least {required}"
            ),
        }
    }
}

impl std::error::Error for PlaneAssembleError {}

/// Spec/07 §5.6 / §5.7 — assemble one plane of the IF09 / YVU9
/// passthrough output from its decoded strip pixel buffers.
///
/// Walks the plane's strips in left-to-right order (§5.7), reading
/// `plane_height` rows of each strip's visible pixels from its own
/// [`FRAME_OUTPUT_SRC_ROW_STRIDE`]-stride buffer (§5.6 step 1a),
/// applying the §4.3 1-bit upshift per byte (§5.6 step 1b, which
/// also clears the §4.4 edge-marker bit), and writing the result to
/// the corresponding horizontal slice of `dst` (§5.6 step 1c), whose
/// rows are `dst_stride` bytes apart. Per §5.7, every strip except
/// possibly the last is `geometry.strip_width` pixels wide; the last
/// carries the spec/02 §4.1 remainder width.
///
/// Bytes of `dst` outside the plane's `plane_width × plane_height`
/// raster (stride padding, trailing slack) are left untouched.
///
/// Returns the number of pixel bytes written
/// (`plane_width × plane_height`).
pub fn assemble_plane_if09(
    geometry: &StripGeometry,
    strips: &[&[u8]],
    dst: &mut [u8],
    dst_stride: usize,
) -> Result<usize, PlaneAssembleError> {
    if strips.len() != geometry.strip_count as usize {
        return Err(PlaneAssembleError::StripCountMismatch {
            expected: geometry.strip_count,
            supplied: strips.len(),
        });
    }

    let widths_total: u32 = geometry.iter_strip_widths().sum();
    if widths_total != geometry.plane_width {
        return Err(PlaneAssembleError::StripWidthSumMismatch {
            plane_width: geometry.plane_width,
            widths_total,
        });
    }

    let plane_width = geometry.plane_width as usize;
    let plane_height = geometry.plane_height as usize;

    if plane_width > dst_stride {
        return Err(PlaneAssembleError::DstStrideTooNarrow {
            plane_width: geometry.plane_width,
            dst_stride,
        });
    }

    let dst_required = if plane_height == 0 || plane_width == 0 {
        0
    } else {
        (plane_height - 1) * dst_stride + plane_width
    };
    if dst.len() < dst_required {
        return Err(PlaneAssembleError::DstBufferTooShort {
            required: dst_required,
            supplied: dst.len(),
        });
    }

    let mut x0 = 0usize;
    let mut written = 0usize;
    for (strip_index, strip_width) in geometry.iter_strip_widths().enumerate() {
        if strip_width as usize > FRAME_OUTPUT_SRC_ROW_STRIDE {
            return Err(PlaneAssembleError::StripWidthExceedsRowStride {
                strip_index,
                strip_width,
            });
        }
        let required = strip_min_buffer_bytes(strip_width, geometry.plane_height);
        let strip = strips[strip_index];
        if strip.len() < required {
            return Err(PlaneAssembleError::StripBufferTooShort {
                strip_index,
                required,
                supplied: strip.len(),
            });
        }

        let w = strip_width as usize;
        if w == 0 {
            continue;
        }
        for row in 0..plane_height {
            let src_row = &strip[row * FRAME_OUTPUT_SRC_ROW_STRIDE..][..w];
            let dst_row = &mut dst[row * dst_stride + x0..][..w];
            for (d, s) in dst_row.iter_mut().zip(src_row) {
                *d = upshift_7bit_to_8bit(*s);
            }
        }
        x0 += w;
        written += w * plane_height;
    }

    Ok(written)
}

// ---- §5.5 chroma upsampling (4:1:0 → output sampling) ---------------

/// Spec/07 §5.5 — the chroma → luma upsampling ratio along one axis.
///
/// "The V and U planes have one quarter of the picture's width *and*
/// one quarter of its height" (the 4:1:0 subsampling), so the
/// output-conversion stage replicates each chroma sample across a
/// 4×4 block of luma positions.
pub const CHROMA_UPSAMPLE_FACTOR: usize = 4;

/// Spec/07 §5.5 — failure modes of the chroma box-upsampler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChromaUpsampleError {
    /// The source row stride is narrower than the source chroma width
    /// — one source row cannot fit.
    SrcStrideTooNarrow {
        /// The chroma plane width (in samples).
        chroma_width: u32,
        /// The supplied source row stride (in bytes).
        src_stride: usize,
    },
    /// The source slice is shorter than the chroma plane requires.
    SrcBufferTooShort {
        /// Bytes the source walk requires.
        required: usize,
        /// Bytes supplied.
        supplied: usize,
    },
    /// The destination row stride is narrower than the upsampled
    /// (luma-resolution) width — one output row cannot fit.
    DstStrideTooNarrow {
        /// The upsampled width (`chroma_width × 4`).
        upsampled_width: usize,
        /// The supplied destination row stride (in bytes).
        dst_stride: usize,
    },
    /// The destination slice is shorter than the upsampled plane
    /// requires.
    DstBufferTooShort {
        /// Bytes the upsampled plane requires.
        required: usize,
        /// Bytes supplied.
        supplied: usize,
    },
}

impl core::fmt::Display for ChromaUpsampleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ChromaUpsampleError::SrcStrideTooNarrow {
                chroma_width,
                src_stride,
            } => write!(
                f,
                "spec/07 §5.5: source row stride {src_stride} is narrower than \
                 the chroma width {chroma_width}"
            ),
            ChromaUpsampleError::SrcBufferTooShort { required, supplied } => write!(
                f,
                "spec/07 §5.5: source slice has {supplied} byte(s); \
                 the upsample walk requires at least {required}"
            ),
            ChromaUpsampleError::DstStrideTooNarrow {
                upsampled_width,
                dst_stride,
            } => write!(
                f,
                "spec/07 §5.5: output row stride {dst_stride} is narrower than \
                 the upsampled width {upsampled_width}"
            ),
            ChromaUpsampleError::DstBufferTooShort { required, supplied } => write!(
                f,
                "spec/07 §5.5: output slice has {supplied} byte(s); \
                 the upsampled plane requires at least {required}"
            ),
        }
    }
}

impl std::error::Error for ChromaUpsampleError {}

/// Spec/07 §5.5 — box-filter upsample one chroma plane (V or U) from
/// its 4:1:0 subsampled resolution to full (luma) resolution.
///
/// Per §5.5 the output-conversion stage upsamples chroma by
/// "replicating each chroma sample to a 4×4 block of luma positions"
/// — "plain box-filter chroma upsampling: there is no chroma
/// interpolation, no edge-aware reconstruction". Each output sample at
/// `(row, col)` reads the chroma sample at `(row / 4, col / 4)`
/// (`§5.5`'s "integer division by 4 (or equivalently shift by 2)").
///
/// The source is one assembled chroma plane of `chroma_width ×
/// chroma_height` samples (e.g. the output of [`assemble_plane_if09`]
/// for a V / U plane, or any caller-supplied chroma raster), whose
/// rows are `src_stride` bytes apart. Each source byte already carries
/// the §4.3 1-bit-upshifted 8-bit value; this function copies it
/// verbatim (it performs **no** further upshift — the source is
/// assumed to be in the 8-bit output range already, matching the way
/// §5.4 folds the upshift into the LUT and §5.6 applies it once
/// per-plane).
///
/// The destination is the upsampled plane of `chroma_width × 4` by
/// `chroma_height × 4` samples, whose rows are `dst_stride` bytes
/// apart. Bytes of `dst` outside the upsampled raster (stride
/// padding, trailing slack) are left untouched.
///
/// Returns the number of samples written
/// (`chroma_width × 4 × chroma_height × 4`).
pub fn upsample_chroma_4x4(
    src: &[u8],
    chroma_width: u32,
    chroma_height: u32,
    src_stride: usize,
    dst: &mut [u8],
    dst_stride: usize,
) -> Result<usize, ChromaUpsampleError> {
    let cw = chroma_width as usize;
    let ch = chroma_height as usize;

    if cw > src_stride {
        return Err(ChromaUpsampleError::SrcStrideTooNarrow {
            chroma_width,
            src_stride,
        });
    }

    let src_required = if ch == 0 || cw == 0 {
        0
    } else {
        (ch - 1) * src_stride + cw
    };
    if src.len() < src_required {
        return Err(ChromaUpsampleError::SrcBufferTooShort {
            required: src_required,
            supplied: src.len(),
        });
    }

    let up_w = cw * CHROMA_UPSAMPLE_FACTOR;
    let up_h = ch * CHROMA_UPSAMPLE_FACTOR;

    if up_w > dst_stride {
        return Err(ChromaUpsampleError::DstStrideTooNarrow {
            upsampled_width: up_w,
            dst_stride,
        });
    }

    let dst_required = if up_h == 0 || up_w == 0 {
        0
    } else {
        (up_h - 1) * dst_stride + up_w
    };
    if dst.len() < dst_required {
        return Err(ChromaUpsampleError::DstBufferTooShort {
            required: dst_required,
            supplied: dst.len(),
        });
    }

    // §5.5: for each chroma sample (cy, cx), replicate it into the
    // 4×4 block of output positions [(cy*4 + dy), (cx*4 + dx)] for
    // dy, dx in 0..4 — "each (V, U) pair contributes the chroma
    // component to 16 adjacent RGB pixels".
    for cy in 0..ch {
        let src_row = &src[cy * src_stride..][..cw];
        for dy in 0..CHROMA_UPSAMPLE_FACTOR {
            let out_row_base = (cy * CHROMA_UPSAMPLE_FACTOR + dy) * dst_stride;
            let dst_row = &mut dst[out_row_base..][..up_w];
            for (cx, &s) in src_row.iter().enumerate() {
                let block = &mut dst_row[cx * CHROMA_UPSAMPLE_FACTOR..][..CHROMA_UPSAMPLE_FACTOR];
                block.fill(s);
            }
        }
    }

    Ok(up_w * up_h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::{EDGE_MARKER_BIT, PLANE_ITERATION_ORDER};

    // ---- §4.3 upshift ------------------------------------------------

    #[test]
    fn upshift_doubles_in_range_values() {
        // §4.3: `shl byte, 1` on the 7-bit content doubles it.
        assert_eq!(upshift_7bit_to_8bit(0x00), 0x00);
        assert_eq!(upshift_7bit_to_8bit(0x01), 0x02);
        assert_eq!(upshift_7bit_to_8bit(0x3f), 0x7e);
        assert_eq!(upshift_7bit_to_8bit(0x7f), 0xfe);
    }

    #[test]
    fn upshift_clears_edge_marker() {
        // §4.4: "the marker bit is cleared by the per-plane upshift
        // (since `shl byte, 1` discards bit 7)".
        assert_eq!(upshift_7bit_to_8bit(EDGE_MARKER_BIT), 0x00);
        assert_eq!(upshift_7bit_to_8bit(EDGE_MARKER_BIT | 0x05), 0x0a);
        for b in 0..=u8::MAX {
            assert_eq!(
                upshift_7bit_to_8bit(b),
                upshift_7bit_to_8bit(b & !EDGE_MARKER_BIT),
                "marker-set and marker-clear bytes must upshift identically (b = {b:#04x})"
            );
            assert_eq!(upshift_7bit_to_8bit(b), (b & 0x7f) << 1);
        }
    }

    #[test]
    fn upshift_output_is_always_even() {
        // §4.3: a 1-bit left shift leaves bit 0 clear on every
        // output byte.
        for b in 0..=u8::MAX {
            assert_eq!(upshift_7bit_to_8bit(b) & 0x01, 0);
        }
    }

    // ---- §5.3 / §5.6 dispatch constants -------------------------------

    #[test]
    fn if09_fourcc_spells_if09() {
        assert_eq!(IF09_FOURCC, 0x3930_4649);
        assert_eq!(IF09_FOURCC.to_le_bytes(), *b"IF09");
    }

    #[test]
    fn if09_dispatch_rvas_match_spec() {
        // §5.6: FOURCC referenced at 0x10004576; §5.3 table: the
        // passthrough conversion function enters at 0x1000a53c.
        assert_eq!(IF09_FOURCC_CASE_RVA, 0x1000_4576);
        assert_eq!(IF09_PASSTHROUGH_RVA, 0x1000_a53c);
        // The conversion function lies after the dispatch that
        // selects it in code memory. (`black_box` defeats clippy's
        // constant-folding lint, mirroring mc_bounds.)
        assert!(core::hint::black_box(IF09_FOURCC_CASE_RVA) < IF09_PASSTHROUGH_RVA);
    }

    // ---- §5.3 output-format dispatch ----------------------------------

    #[test]
    fn dispatch_if09_compression_selects_passthrough() {
        // §5.3 first arm: input biCompression == 'IF09' → passthrough,
        // regardless of the output bit count / colour-space flag.
        for bits in [0u16, 8, 16, 24, 32] {
            for flag in [false, true] {
                assert_eq!(
                    select_output_conversion(IF09_FOURCC, bits, flag),
                    Ok(OutputConversion::If09Passthrough)
                );
            }
        }
    }

    #[test]
    fn dispatch_bi_rgb_switches_on_bit_count() {
        // §5.3 second arm: input BI_RGB, output biBitCount picks the
        // RGB variant.
        assert_eq!(
            select_output_conversion(BI_RGB, 8, false),
            Ok(OutputConversion::Rgb8Indexed)
        );
        assert_eq!(
            select_output_conversion(BI_RGB, 16, false),
            Ok(OutputConversion::Rgb16)
        );
        // 16-bit ignores the colour-space flag at the dispatch level
        // (the 5-6-5 / 5-5-5 choice is internal to the variant's LUT).
        assert_eq!(
            select_output_conversion(BI_RGB, 16, true),
            Ok(OutputConversion::Rgb16)
        );
    }

    #[test]
    fn dispatch_rgb24_variant_by_colour_space_flag() {
        // §5.3: biBitCount == 24 picks canonical vs alternate RGB24
        // by the colour-space flag (`0x100096fc | 0x10009aa0`).
        assert_eq!(
            select_output_conversion(BI_RGB, 24, false),
            Ok(OutputConversion::Rgb24)
        );
        assert_eq!(
            select_output_conversion(BI_RGB, 24, true),
            Ok(OutputConversion::Rgb24Alt)
        );
    }

    #[test]
    fn dispatch_bitfields_selects_palette_variant() {
        // §5.3 third arm: input biCompression == 3 → sub_6060.
        for bits in [0u16, 16, 24, 32] {
            assert_eq!(
                select_output_conversion(BI_BITFIELDS, bits, false),
                Ok(OutputConversion::BitfieldPalette)
            );
        }
    }

    #[test]
    fn dispatch_unsupported_compression_faults() {
        // A biCompression the dispatch doesn't switch on (e.g. the
        // 'IV32' FOURCC itself, or an arbitrary value) has no var_24.
        let iv32 = u32::from_le_bytes(*b"IV32");
        assert_eq!(
            select_output_conversion(iv32, 24, false),
            Err(OutputDispatchError::UnsupportedCompression { compression: iv32 })
        );
        assert_eq!(
            select_output_conversion(1, 24, false),
            Err(OutputDispatchError::UnsupportedCompression { compression: 1 })
        );
    }

    #[test]
    fn dispatch_unsupported_rgb_bit_count_faults() {
        // BI_RGB with an output depth outside {8, 16, 24} (the §5.3
        // arm has no else branch for 1 / 4 / 32 bpp).
        for bits in [1u16, 4, 32] {
            assert_eq!(
                select_output_conversion(BI_RGB, bits, false),
                Err(OutputDispatchError::UnsupportedRgbBitCount { bit_count: bits })
            );
        }
    }

    #[test]
    fn conversion_entry_rvas_match_spec_table() {
        // §5.3 RVA table — exact entry RVAs per row.
        assert_eq!(OutputConversion::If09Passthrough.entry_rva(), 0x1000_a53c);
        assert_eq!(OutputConversion::Rgb8Indexed.entry_rva(), 0x1000_8774);
        assert_eq!(OutputConversion::Rgb16.entry_rva(), 0x1000_8a50);
        assert_eq!(OutputConversion::Rgb24.entry_rva(), 0x1000_96fc);
        assert_eq!(OutputConversion::Rgb24Alt.entry_rva(), 0x1000_9aa0);
        assert_eq!(OutputConversion::BitfieldPalette.entry_rva(), 0x1000_6060);
        // The passthrough variant's RVA aliases the standalone const.
        assert_eq!(
            OutputConversion::If09Passthrough.entry_rva(),
            IF09_PASSTHROUGH_RVA
        );
    }

    #[test]
    fn only_if09_passthrough_is_implemented() {
        // §5.4: the RGB variants are LUT-driven and deferred (§7.2);
        // only the passthrough body is landed (assemble_plane_if09).
        assert!(OutputConversion::If09Passthrough.is_implemented());
        for v in [
            OutputConversion::Rgb8Indexed,
            OutputConversion::Rgb16,
            OutputConversion::Rgb24,
            OutputConversion::Rgb24Alt,
            OutputConversion::BitfieldPalette,
        ] {
            assert!(!v.is_implemented(), "{v:?} body is not landed yet");
        }
    }

    #[test]
    fn bi_rgb_and_bi_bitfields_match_windows_vocabulary() {
        // §5.3 — BI_RGB == 0, BI_BITFIELDS == 3 (BITMAPINFOHEADER).
        assert_eq!(BI_RGB, 0);
        assert_eq!(BI_BITFIELDS, 3);
        // Neither aliases the IF09 FOURCC, so the dispatch arms are
        // disjoint.
        assert_ne!(BI_RGB, IF09_FOURCC);
        assert_ne!(BI_BITFIELDS, IF09_FOURCC);
    }

    #[test]
    fn rgb24_stride_fixup_trigger_is_24bpp() {
        // §5.3: the sub_5480 per-line fix-up fires for 24-bpp output.
        assert_eq!(RGB24_STRIDE_FIXUP_BIT_COUNT, 24);
        // The two 24-bpp variants are exactly the ones a 24-bpp
        // output request resolves to.
        assert_eq!(
            select_output_conversion(BI_RGB, RGB24_STRIDE_FIXUP_BIT_COUNT, false),
            Ok(OutputConversion::Rgb24)
        );
        assert_eq!(
            select_output_conversion(BI_RGB, RGB24_STRIDE_FIXUP_BIT_COUNT, true),
            Ok(OutputConversion::Rgb24Alt)
        );
    }

    #[test]
    fn output_plane_order_is_y_v_u() {
        // §5.6 step 2: "Plane order in the output is Y first, then
        // V, then U."
        assert_eq!(
            OUTPUT_PLANE_ORDER,
            [
                crate::indeo3::PLANE_IDX_Y,
                crate::indeo3::PLANE_IDX_V,
                crate::indeo3::PLANE_IDX_U
            ]
        );
    }

    #[test]
    fn output_plane_order_reverses_decode_order() {
        // §5.2 decodes U → V → Y; §5.6 emits Y → V → U.
        let mut reversed = PLANE_ITERATION_ORDER;
        reversed.reverse();
        assert_eq!(OUTPUT_PLANE_ORDER, reversed);
    }

    #[test]
    fn src_row_stride_matches_pipeline() {
        assert_eq!(FRAME_OUTPUT_SRC_ROW_STRIDE, 0xb0);
        assert_eq!(FRAME_OUTPUT_SRC_ROW_STRIDE, MC_ARENA_ROW_STRIDE);
        assert_eq!(
            FRAME_OUTPUT_SRC_ROW_STRIDE,
            crate::indeo3::STRIP_EDGE_ROW_STRIDE
        );
    }

    // ---- strip_min_buffer_bytes ---------------------------------------

    #[test]
    fn strip_min_buffer_bytes_formula() {
        // Last row needs only its visible pixels.
        assert_eq!(strip_min_buffer_bytes(160, 8), 7 * 0xb0 + 160);
        assert_eq!(strip_min_buffer_bytes(40, 4), 3 * 0xb0 + 40);
        assert_eq!(strip_min_buffer_bytes(160, 1), 160);
        assert_eq!(strip_min_buffer_bytes(160, 0), 0);
        assert_eq!(strip_min_buffer_bytes(0, 8), 0);
    }

    // ---- §5.7 assembly: happy paths -----------------------------------

    /// Build a strip buffer of `rows` rows whose visible bytes are
    /// `base + row` (constant per row), with stride padding filled
    /// with `0x55`.
    fn make_strip(width: usize, rows: usize, base: u8) -> Vec<u8> {
        let mut buf = vec![0x55u8; strip_min_buffer_bytes(width as u32, rows as u32)];
        for r in 0..rows {
            for c in 0..width {
                buf[r * FRAME_OUTPUT_SRC_ROW_STRIDE + c] = base + r as u8;
            }
        }
        buf
    }

    #[test]
    fn single_strip_luma_assembly() {
        // 160×4 luma plane: one strip, tight output stride.
        let g = StripGeometry::for_luma(160, 4);
        assert_eq!(g.strip_count, 1);
        let s0 = make_strip(160, 4, 0x10);
        let mut dst = vec![0u8; 160 * 4];
        let written = assemble_plane_if09(&g, &[&s0], &mut dst, 160).unwrap();
        assert_eq!(written, 160 * 4);
        for r in 0..4 {
            for c in 0..160 {
                assert_eq!(dst[r * 160 + c], (0x10 + r as u8) << 1);
            }
        }
    }

    #[test]
    fn two_strip_luma_assembly_concatenates_left_to_right() {
        // §5.7: "For a 320-pixel-wide frame, there are exactly 2
        // strips per plane; the conversion loop processes them in
        // left-to-right order."
        let g = StripGeometry::for_luma(320, 2);
        assert_eq!(g.strip_count, 2);
        let s0 = make_strip(160, 2, 0x10);
        let s1 = make_strip(160, 2, 0x20);
        let mut dst = vec![0u8; 320 * 2];
        let written = assemble_plane_if09(&g, &[&s0, &s1], &mut dst, 320).unwrap();
        assert_eq!(written, 320 * 2);
        for r in 0..2 {
            for c in 0..160 {
                assert_eq!(dst[r * 320 + c], (0x10 + r as u8) << 1, "strip 0 r{r} c{c}");
                assert_eq!(
                    dst[r * 320 + 160 + c],
                    (0x20 + r as u8) << 1,
                    "strip 1 r{r} c{c}"
                );
            }
        }
    }

    #[test]
    fn remainder_last_strip_assembly() {
        // 176-wide luma: strips of width [160, 16] per the spec/02
        // §4.1 remainder formula.
        let g = StripGeometry::for_luma(176, 2);
        assert_eq!(g.strip_count, 2);
        assert_eq!(g.last_strip_width, 16);
        let s0 = make_strip(160, 2, 0x01);
        let s1 = make_strip(16, 2, 0x30);
        let mut dst = vec![0xEEu8; 176 * 2];
        let written = assemble_plane_if09(&g, &[&s0, &s1], &mut dst, 176).unwrap();
        assert_eq!(written, 176 * 2);
        assert_eq!(dst[159], 0x01 << 1);
        assert_eq!(dst[160], 0x30 << 1);
        assert_eq!(dst[175], 0x30 << 1);
        assert_eq!(dst[176], (0x01 + 1) << 1); // row 1, strip 0
    }

    #[test]
    fn chroma_plane_assembly() {
        // 80-wide chroma plane: two 40-wide strips (§5.7 strip
        // widths are 40 px for chroma).
        let g = StripGeometry::for_chroma(80, 3);
        assert_eq!(g.strip_count, 2);
        assert_eq!(g.strip_width, 40);
        let s0 = make_strip(40, 3, 0x05);
        let s1 = make_strip(40, 3, 0x45);
        let mut dst = vec![0u8; 80 * 3];
        let written = assemble_plane_if09(&g, &[&s0, &s1], &mut dst, 80).unwrap();
        assert_eq!(written, 80 * 3);
        // 0x45 has no bit 7; doubles to 0x8a.
        assert_eq!(dst[40], 0x8a);
        assert_eq!(dst[2 * 80 + 79], ((0x45u8 + 2) << 1));
    }

    #[test]
    fn wide_dst_stride_leaves_padding_untouched() {
        // Output rows at a 0xb0 stride: bytes past the plane width
        // must keep their pre-fill.
        let g = StripGeometry::for_luma(160, 2);
        let s0 = make_strip(160, 2, 0x08);
        let mut dst = vec![0xEEu8; 0xb0 * 2];
        assemble_plane_if09(&g, &[&s0], &mut dst, 0xb0).unwrap();
        for r in 0..2 {
            assert_eq!(dst[r * 0xb0], (0x08 + r as u8) << 1);
            for c in 160..0xb0 {
                assert_eq!(dst[r * 0xb0 + c], 0xEE, "padding touched at r{r} c{c}");
            }
        }
        // Trailing slack beyond the last row's visible pixels also
        // untouched.
        assert_eq!(dst[0xb0 + 160], 0xEE);
    }

    #[test]
    fn edge_marker_bytes_assemble_with_marker_dropped() {
        // §4.4: marker-bearing bytes reach the output as their
        // 7-bit content, doubled.
        let g = StripGeometry::for_luma(160, 1);
        let mut s0 = make_strip(160, 1, 0x00);
        s0[3] = EDGE_MARKER_BIT | 0x21;
        let mut dst = vec![0u8; 160];
        assemble_plane_if09(&g, &[&s0], &mut dst, 160).unwrap();
        assert_eq!(dst[3], 0x21 << 1);
    }

    #[test]
    fn zero_height_plane_writes_nothing() {
        let g = StripGeometry::for_luma(320, 0);
        assert_eq!(g.strip_count, 2);
        let empty: &[u8] = &[];
        let mut dst = [0xAAu8; 4];
        let written = assemble_plane_if09(&g, &[empty, empty], &mut dst, 320).unwrap();
        assert_eq!(written, 0);
        assert_eq!(dst, [0xAA; 4]);
    }

    #[test]
    fn zero_width_plane_writes_nothing() {
        let g = StripGeometry::for_luma(0, 8);
        assert_eq!(g.strip_count, 0);
        let mut dst = [0xAAu8; 4];
        let written = assemble_plane_if09(&g, &[], &mut dst, 0).unwrap();
        assert_eq!(written, 0);
        assert_eq!(dst, [0xAA; 4]);
    }

    // ---- §5.7 assembly: error paths -----------------------------------

    #[test]
    fn strip_count_mismatch_rejected() {
        let g = StripGeometry::for_luma(320, 2);
        let s0 = make_strip(160, 2, 0);
        let mut dst = vec![0u8; 320 * 2];
        let err = assemble_plane_if09(&g, &[&s0], &mut dst, 320).unwrap_err();
        assert_eq!(
            err,
            PlaneAssembleError::StripCountMismatch {
                expected: 2,
                supplied: 1
            }
        );
        assert!(err.to_string().contains("spec/07 §5.7"));
    }

    #[test]
    fn strip_width_sum_mismatch_rejected() {
        // Hand-built geometry whose widths tile 320 + 16 ≠ 400.
        let g = StripGeometry {
            role: crate::indeo3::PlaneRole::Luma,
            plane_width: 400,
            plane_height: 2,
            strip_width: 160,
            strip_count: 2,
            last_strip_width: 16,
        };
        let s = make_strip(160, 2, 0);
        let mut dst = vec![0u8; 400 * 2];
        let err = assemble_plane_if09(&g, &[&s, &s], &mut dst, 400).unwrap_err();
        assert_eq!(
            err,
            PlaneAssembleError::StripWidthSumMismatch {
                plane_width: 400,
                widths_total: 176
            }
        );
    }

    #[test]
    fn strip_width_exceeding_row_stride_rejected() {
        // Hand-built geometry with an impossible 0xb1-wide strip.
        let g = StripGeometry {
            role: crate::indeo3::PlaneRole::Luma,
            plane_width: 0xb1,
            plane_height: 1,
            strip_width: 0xb1,
            strip_count: 1,
            last_strip_width: 0xb1,
        };
        let s = vec![0u8; 0x200];
        let mut dst = vec![0u8; 0xb1];
        let err = assemble_plane_if09(&g, &[&s], &mut dst, 0xb1).unwrap_err();
        assert_eq!(
            err,
            PlaneAssembleError::StripWidthExceedsRowStride {
                strip_index: 0,
                strip_width: 0xb1
            }
        );
    }

    #[test]
    fn short_strip_buffer_rejected() {
        let g = StripGeometry::for_luma(160, 4);
        let s0 = vec![0u8; strip_min_buffer_bytes(160, 4) - 1];
        let mut dst = vec![0u8; 160 * 4];
        let err = assemble_plane_if09(&g, &[&s0], &mut dst, 160).unwrap_err();
        assert_eq!(
            err,
            PlaneAssembleError::StripBufferTooShort {
                strip_index: 0,
                required: strip_min_buffer_bytes(160, 4),
                supplied: strip_min_buffer_bytes(160, 4) - 1
            }
        );
    }

    #[test]
    fn narrow_dst_stride_rejected() {
        let g = StripGeometry::for_luma(320, 2);
        let s = make_strip(160, 2, 0);
        let mut dst = vec![0u8; 320 * 2];
        let err = assemble_plane_if09(&g, &[&s, &s], &mut dst, 319).unwrap_err();
        assert_eq!(
            err,
            PlaneAssembleError::DstStrideTooNarrow {
                plane_width: 320,
                dst_stride: 319
            }
        );
    }

    #[test]
    fn short_dst_buffer_rejected() {
        let g = StripGeometry::for_luma(160, 4);
        let s = make_strip(160, 4, 0);
        // Required: 3 * 160 + 160 = 640; supply one byte less.
        let mut dst = vec![0u8; 160 * 4 - 1];
        let err = assemble_plane_if09(&g, &[&s], &mut dst, 160).unwrap_err();
        assert_eq!(
            err,
            PlaneAssembleError::DstBufferTooShort {
                required: 160 * 4,
                supplied: 160 * 4 - 1
            }
        );
    }

    #[test]
    fn error_display_cites_spec_sections() {
        let errs: [PlaneAssembleError; 6] = [
            PlaneAssembleError::StripCountMismatch {
                expected: 2,
                supplied: 1,
            },
            PlaneAssembleError::StripWidthSumMismatch {
                plane_width: 400,
                widths_total: 176,
            },
            PlaneAssembleError::StripWidthExceedsRowStride {
                strip_index: 0,
                strip_width: 0xb1,
            },
            PlaneAssembleError::StripBufferTooShort {
                strip_index: 0,
                required: 10,
                supplied: 9,
            },
            PlaneAssembleError::DstStrideTooNarrow {
                plane_width: 320,
                dst_stride: 319,
            },
            PlaneAssembleError::DstBufferTooShort {
                required: 640,
                supplied: 639,
            },
        ];
        for e in errs {
            assert!(e.to_string().contains("spec/07"), "{e}");
        }
    }

    // ---- §5.5 chroma upsampling --------------------------------------

    #[test]
    fn upsample_factor_is_four() {
        // §5.5: each chroma sample replicates to a 4×4 block.
        assert_eq!(CHROMA_UPSAMPLE_FACTOR, 4);
    }

    #[test]
    fn single_sample_fills_4x4_block() {
        // §5.5: one (V, U) sample contributes to 16 adjacent pixels.
        let src = [0x42u8];
        let mut dst = [0u8; 16];
        let written = upsample_chroma_4x4(&src, 1, 1, 1, &mut dst, 4).unwrap();
        assert_eq!(written, 16);
        assert!(dst.iter().all(|&b| b == 0x42), "{dst:?}");
    }

    #[test]
    fn two_by_two_chroma_box_replicates_each_quadrant() {
        // §5.5: integer-divide output indices by 4 to pick the chroma
        // source. A 2×2 chroma plane upsamples to an 8×8 raster of
        // four 4×4 quadrants.
        let src = [
            0x10, 0x20, // chroma row 0
            0x30, 0x40, // chroma row 1
        ];
        let up_w = 8usize;
        let mut dst = vec![0u8; up_w * 8];
        let written = upsample_chroma_4x4(&src, 2, 2, 2, &mut dst, up_w).unwrap();
        assert_eq!(written, 64);
        for oy in 0..8 {
            for ox in 0..8 {
                let expected = src[(oy / 4) * 2 + (ox / 4)];
                assert_eq!(dst[oy * up_w + ox], expected, "({oy},{ox})");
            }
        }
    }

    #[test]
    fn source_stride_padding_is_skipped() {
        // The source may carry stride padding (e.g. an assembled plane
        // with a wider row stride than its visible width). Padding
        // bytes past `chroma_width` must not leak into the output.
        let cw = 2usize;
        let src_stride = 5usize; // 3 padding bytes per row
        let src = [
            0x01, 0x02, 0xff, 0xff, 0xff, // row 0 (+ padding)
            0x03, 0x04, 0xff, 0xff, 0xff, // row 1 (+ padding)
        ];
        let up_w = cw * 4;
        let mut dst = vec![0u8; up_w * 8];
        upsample_chroma_4x4(&src, cw as u32, 2, src_stride, &mut dst, up_w).unwrap();
        // No 0xff (padding) byte may appear in the upsampled plane.
        assert!(!dst.contains(&0xff), "padding leaked: {dst:?}");
        // Top-left quadrant is all 0x01, top-right all 0x02.
        assert_eq!(dst[0], 0x01);
        assert_eq!(dst[4], 0x02);
    }

    #[test]
    fn dst_stride_padding_left_untouched() {
        // Output bytes outside the upsampled raster (stride slack) are
        // not overwritten.
        let src = [0x55u8];
        let dst_stride = 7usize; // upsampled width is 4; 3 bytes slack
        let mut dst = vec![0xaau8; dst_stride * 4];
        upsample_chroma_4x4(&src, 1, 1, 1, &mut dst, dst_stride).unwrap();
        for row in 0..4 {
            for col in 0..dst_stride {
                let v = dst[row * dst_stride + col];
                if col < 4 {
                    assert_eq!(v, 0x55, "raster ({row},{col})");
                } else {
                    assert_eq!(v, 0xaa, "slack ({row},{col}) overwritten");
                }
            }
        }
    }

    #[test]
    fn zero_dimensions_write_nothing() {
        let mut dst = [0xaau8; 4];
        assert_eq!(upsample_chroma_4x4(&[], 0, 0, 0, &mut dst, 0), Ok(0));
        assert_eq!(upsample_chroma_4x4(&[], 0, 4, 0, &mut dst, 0), Ok(0));
        assert_eq!(upsample_chroma_4x4(&[], 4, 0, 4, &mut dst, 16), Ok(0));
        assert!(dst.iter().all(|&b| b == 0xaa));
    }

    #[test]
    fn copies_source_verbatim_no_upshift() {
        // §5.5 / §5.4: the source already carries the 8-bit-range
        // value; the box upsampler does no further upshift. An odd
        // byte (bit 0 set) survives unchanged — proof there is no
        // `shl byte, 1`.
        let src = [0x7fu8];
        let mut dst = [0u8; 16];
        upsample_chroma_4x4(&src, 1, 1, 1, &mut dst, 4).unwrap();
        assert!(dst.iter().all(|&b| b == 0x7f), "{dst:?}");
    }

    #[test]
    fn feeds_assemble_plane_if09_output() {
        // End-to-end: assemble a chroma plane via the IF09 path, then
        // box-upsample it. The assembled (upshifted) chroma plane
        // becomes the upsampler's source.
        let geom = StripGeometry::for_chroma(2, 2);
        // One strip, 2 wide, 2 tall; 7-bit internal values.
        let mut strip = vec![0u8; FRAME_OUTPUT_SRC_ROW_STRIDE * 2];
        strip[0] = 0x08;
        strip[1] = 0x10;
        strip[FRAME_OUTPUT_SRC_ROW_STRIDE] = 0x18;
        strip[FRAME_OUTPUT_SRC_ROW_STRIDE + 1] = 0x20;
        let mut chroma = vec![0u8; 4];
        assemble_plane_if09(&geom, &[&strip], &mut chroma, 2).unwrap();
        // The assembled plane is the upshifted (×2) chroma.
        assert_eq!(chroma, vec![0x10, 0x20, 0x30, 0x40]);

        let mut up = vec![0u8; 8 * 8];
        let written = upsample_chroma_4x4(&chroma, 2, 2, 2, &mut up, 8).unwrap();
        assert_eq!(written, 64);
        // Top-left 4×4 quadrant carries the upshifted 0x08 → 0x10.
        for oy in 0..4 {
            for ox in 0..4 {
                assert_eq!(up[oy * 8 + ox], 0x10);
            }
        }
        // Bottom-right quadrant carries 0x20 → 0x40.
        for oy in 4..8 {
            for ox in 4..8 {
                assert_eq!(up[oy * 8 + ox], 0x40);
            }
        }
    }

    #[test]
    fn src_stride_too_narrow_rejected() {
        let mut dst = vec![0u8; 16];
        assert_eq!(
            upsample_chroma_4x4(&[0u8; 4], 4, 1, 3, &mut dst, 16),
            Err(ChromaUpsampleError::SrcStrideTooNarrow {
                chroma_width: 4,
                src_stride: 3,
            })
        );
    }

    #[test]
    fn src_buffer_too_short_rejected() {
        let mut dst = vec![0u8; 4 * 16];
        // 2×2 chroma at stride 2 needs (2-1)*2 + 2 = 4 bytes.
        assert_eq!(
            upsample_chroma_4x4(&[0u8; 3], 2, 2, 2, &mut dst, 8),
            Err(ChromaUpsampleError::SrcBufferTooShort {
                required: 4,
                supplied: 3,
            })
        );
    }

    #[test]
    fn dst_stride_too_narrow_rejected() {
        let mut dst = vec![0u8; 64];
        // chroma_width 2 → upsampled width 8; a stride of 7 is too
        // narrow.
        assert_eq!(
            upsample_chroma_4x4(&[0u8; 4], 2, 2, 2, &mut dst, 7),
            Err(ChromaUpsampleError::DstStrideTooNarrow {
                upsampled_width: 8,
                dst_stride: 7,
            })
        );
    }

    #[test]
    fn dst_buffer_too_short_rejected() {
        // 1×1 chroma → 4×4 upsampled; with stride 4 the plane needs
        // (4-1)*4 + 4 = 16 bytes.
        let mut dst = vec![0u8; 15];
        assert_eq!(
            upsample_chroma_4x4(&[0x01u8], 1, 1, 1, &mut dst, 4),
            Err(ChromaUpsampleError::DstBufferTooShort {
                required: 16,
                supplied: 15,
            })
        );
    }

    #[test]
    fn chroma_upsample_error_display_cites_spec() {
        let errs: [ChromaUpsampleError; 4] = [
            ChromaUpsampleError::SrcStrideTooNarrow {
                chroma_width: 4,
                src_stride: 3,
            },
            ChromaUpsampleError::SrcBufferTooShort {
                required: 4,
                supplied: 3,
            },
            ChromaUpsampleError::DstStrideTooNarrow {
                upsampled_width: 8,
                dst_stride: 7,
            },
            ChromaUpsampleError::DstBufferTooShort {
                required: 16,
                supplied: 15,
            },
        ];
        for e in errs {
            assert!(e.to_string().contains("spec/07 §5.5"), "{e}");
        }
    }
}

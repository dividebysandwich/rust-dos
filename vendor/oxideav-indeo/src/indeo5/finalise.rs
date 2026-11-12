//! Indeo 5 decoder finalisation — return codes, reference rotation,
//! output-written flag, and host-buffer row order.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/08-output-reconstruction.md`
//! §6.3 (row order), §8.1 (reference-buffer promotion), §8.2 (per-frame
//! state flag), and §8.5 (`ICDecompress` return values).
//!
//! After the host buffer write, the per-frame decoder performs a small
//! set of cleanup actions before returning from `ICDecompress`
//! (`spec/08 §8`): it promotes (or drops) the current frame as a
//! reference per its frame type (`spec/08 §8.1`), sets the "output
//! written" state bit (`spec/08 §8.2`), and returns one of three codes
//! (`spec/08 §8.5`). This module models those table-free finalisation
//! decisions; it does not touch the (gated) coefficient path.

use crate::indeo5::format::OutputFormat;
use crate::indeo5::header::FrameType;

/// `spec/08 §8.5` — the `ICDecompress` return code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeReturn {
    /// `0` (`ICERR_OK`) — decode succeeded, host buffer populated.
    Ok,
    /// `2` (`ICERR_BADFORMAT`) — output assembly failed (per-plane
    /// recompose produced no output buffer).
    BadFormat,
    /// `5` — the codec-specific "frame decoded but not displayed" code
    /// (NULL frame, or a droppable-INTER frame the decoder skipped).
    FrameSkipped,
}

impl DecodeReturn {
    /// The raw integer return value the binary places in `eax`
    /// (`spec/08 §8.5`: `0` / `2` / `5`).
    #[inline]
    pub fn code(self) -> i32 {
        match self {
            DecodeReturn::Ok => 0,
            DecodeReturn::BadFormat => 2,
            DecodeReturn::FrameSkipped => 5,
        }
    }
}

/// `spec/08 §8.1` — the post-decode reference-buffer action for a frame,
/// dispatched by frame type via the 4-entry jump table at `0x1003fc18`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceRotation {
    /// Promote the current frame to the primary reference (INTRA/INTER,
    /// `spec/08 §8.1` handler `0x1003f9df`).
    Promote,
    /// Promote with a conditional secondary-chroma swap
    /// (DROPPABLE_INTER_SCAL, handler `0x1003f981`).
    PromoteWithChromaSwap,
    /// Do **not** promote — the droppable frame is not retained
    /// (DROPPABLE_INTER, handler `0x1003fa1b`).
    NoPromote,
}

/// `spec/08 §8.1` — the reference-buffer rotation for a frame type.
///
/// NULL frames produce no output (`spec/08 §6.4`) and do not rotate the
/// reference (the previous reference is retained for the host's re-use);
/// modelled here as [`ReferenceRotation::NoPromote`].
pub fn reference_rotation(frame_type: FrameType) -> ReferenceRotation {
    match frame_type {
        FrameType::Intra | FrameType::Inter => ReferenceRotation::Promote,
        FrameType::DroppableInterScalability => ReferenceRotation::PromoteWithChromaSwap,
        FrameType::DroppableInter => ReferenceRotation::NoPromote,
        FrameType::Null => ReferenceRotation::NoPromote,
    }
}

/// `spec/08 §6.4` — whether a frame produces a host-buffer write.
///
/// NULL frames (`frame_type == 4`) produce no output (the host re-uses
/// the previous frame's display, `spec/08 §6.4`); every coded frame
/// type does. The droppable-INTER return-code nuance (`spec/08 §8.5`,
/// where the decoder *may* skip output) is a runtime choice, not a
/// wire-format property, so it is not modelled here.
#[inline]
pub fn frame_produces_output(frame_type: FrameType) -> bool {
    frame_type != FrameType::Null
}

/// `spec/08 §8.2` — the per-frame "output written for this frame" state
/// bit (bit 26) OR-ed into `[ebx+0x128]` after the host write.
pub const OUTPUT_WRITTEN_FLAG: u32 = 0x0400_0000;

/// `spec/08 §8.2` — set the "output written" bit in the per-frame state
/// flags after the host buffer write.
#[inline]
pub fn mark_output_written(state_flags: u32) -> u32 {
    state_flags | OUTPUT_WRITTEN_FLAG
}

/// `spec/08 §8.2` — whether the "output written" bit is set (the next
/// `ICDecompress` fast-skip guard at `0x1003fa79`).
#[inline]
pub fn is_output_written(state_flags: u32) -> bool {
    state_flags & OUTPUT_WRITTEN_FLAG != 0
}

/// `spec/08 §6.3` — the host-buffer row-write order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowOrder {
    /// Row 0 written first, row `height-1` last (YUV outputs).
    TopDown,
    /// Last frame row written to the first host row (the BMP
    /// bottom-up convention, RGB outputs).
    BottomUp,
}

/// `spec/08 §6.3` — the row-write order for an output format: **top-down**
/// for the YUV formats (`Yvu9`/`Yuy2`/`Yv12`/`I420`), **bottom-up** for
/// the RGB formats (the BMP `biHeight` convention).
pub fn output_row_order(format: OutputFormat) -> RowOrder {
    if format.is_rgb() {
        RowOrder::BottomUp
    } else {
        RowOrder::TopDown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_codes() {
        // spec/08 §8.5.
        assert_eq!(DecodeReturn::Ok.code(), 0);
        assert_eq!(DecodeReturn::BadFormat.code(), 2);
        assert_eq!(DecodeReturn::FrameSkipped.code(), 5);
    }

    #[test]
    fn reference_rotation_by_frame_type() {
        // spec/08 §8.1.
        assert_eq!(
            reference_rotation(FrameType::Intra),
            ReferenceRotation::Promote
        );
        assert_eq!(
            reference_rotation(FrameType::Inter),
            ReferenceRotation::Promote
        );
        assert_eq!(
            reference_rotation(FrameType::DroppableInterScalability),
            ReferenceRotation::PromoteWithChromaSwap
        );
        assert_eq!(
            reference_rotation(FrameType::DroppableInter),
            ReferenceRotation::NoPromote
        );
        assert_eq!(
            reference_rotation(FrameType::Null),
            ReferenceRotation::NoPromote
        );
    }

    #[test]
    fn null_frame_produces_no_output() {
        // spec/08 §6.4.
        assert!(!frame_produces_output(FrameType::Null));
        assert!(frame_produces_output(FrameType::Intra));
        assert!(frame_produces_output(FrameType::Inter));
        assert!(frame_produces_output(FrameType::DroppableInter));
    }

    #[test]
    fn output_written_flag_is_bit_26() {
        assert_eq!(OUTPUT_WRITTEN_FLAG, 1 << 26);
        let flags = mark_output_written(0);
        assert!(is_output_written(flags));
        assert_eq!(flags, 0x0400_0000);
        // Idempotent + preserves other bits.
        let with_others = mark_output_written(0x0000_00ff);
        assert!(is_output_written(with_others));
        assert_eq!(with_others & 0xff, 0xff);
        assert!(!is_output_written(0));
    }

    #[test]
    fn row_order_top_down_for_yuv_bottom_up_for_rgb() {
        // spec/08 §6.3.
        assert_eq!(output_row_order(OutputFormat::Yvu9), RowOrder::TopDown);
        assert_eq!(output_row_order(OutputFormat::Yv12), RowOrder::TopDown);
        assert_eq!(output_row_order(OutputFormat::I420), RowOrder::TopDown);
        assert_eq!(output_row_order(OutputFormat::Yuy2), RowOrder::TopDown);
        assert_eq!(output_row_order(OutputFormat::Rgb), RowOrder::BottomUp);
    }
}

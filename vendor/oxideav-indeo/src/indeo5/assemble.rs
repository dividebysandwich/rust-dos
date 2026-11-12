//! Indeo 5 whole-frame output assembly.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/08-output-reconstruction.md`
//! §1 (plane assembly), §3.3 (bias-and-clamp), §5 (chroma geometry),
//! §6.2 (host buffer write).
//!
//! This is the top-level thread over the spec/08 output stage: given the
//! three per-plane signed reconstruction buffers the wavelet
//! recomposition produces (`spec/08 §0`/§1.2) and a planar host output
//! format, it
//!
//! 1. validates the chroma planes' geometry against the format's
//!    subsampling (`spec/08 §5.1` — luma `W × H` implies chroma
//!    `ceil(W/s) × ceil(H/s)`),
//! 2. converts each plane through the `spec/08 §3.0` saturate-then-bias
//!    (visiting planes in the `spec/08 §1.3` `U → V → Y` decode order,
//!    matching the binary's reverse record walk), and
//! 3. concatenates the planes into the format's host layout
//!    (`spec/08 §5.3`/`§6.2` via [`pack_planar`]).
//!
//! Planar formats keep chroma at its subsampled resolution in the host
//! buffer (`spec/08 §5.3`); the box-filter upsample
//! ([`upsample_chroma`](crate::indeo5::upsample_chroma)) is a
//! *consumer-side* step for full-resolution display, not part of the
//! planar host write. The packed (`Yuy2`) and RGB formats are rejected
//! here for the same reasons [`pack_planar`] defers them
//! (`spec/08 §9.4`/`§9.1`).

use crate::indeo5::format::OutputFormat;
use crate::indeo5::output::{OutputPlane, ReconstructionPlane};
use crate::indeo5::pack::{HostBuffer, pack_planar};
use crate::indeo5::planes::{FramePlanes, PlaneRole};

/// Errors the whole-frame assembly can raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AssembleError {
    /// The requested output format is not a planar YUV format — the
    /// packed `Yuy2` byte interleave is deferred (`spec/08 §9.4`) and
    /// RGB needs the docs-gapped YUV→RGB LUT (`spec/08 §9.1`).
    NotPlanar {
        /// The rejected format's `[ebx+0x70]` selector (`spec/08 §2.2`).
        selector: u32,
    },
    /// A chroma plane's dimensions do not match the subsampled geometry
    /// the format implies for the luma dimensions (`spec/08 §5.1`).
    ChromaGeometryMismatch {
        /// Which chroma plane mismatched.
        role: PlaneRole,
        /// The supplied plane dimensions.
        got: (u32, u32),
        /// The `ceil(luma / scale)` dimensions expected.
        expected: (u32, u32),
    },
}

impl core::fmt::Display for AssembleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AssembleError::NotPlanar { selector } => write!(
                f,
                "indeo5 assemble: output-format selector {selector} is not planar YUV (spec/08 §9.1/§9.4 deferral)"
            ),
            AssembleError::ChromaGeometryMismatch {
                role,
                got,
                expected,
            } => write!(
                f,
                "indeo5 assemble: {role:?} plane is {}x{}, expected {}x{} (spec/08 §5.1)",
                got.0, got.1, expected.0, expected.1
            ),
        }
    }
}

impl std::error::Error for AssembleError {}

/// `spec/08 §1`/`§3.3`/`§5.3`/`§6.2` — assemble one frame's three
/// signed reconstruction planes into a packed planar host buffer.
///
/// `luma` / `chroma_v` / `chroma_u` are the per-plane reconstruction
/// buffers in the `spec/08 §1.1` record roles. The chroma planes must
/// carry the format's subsampled geometry (`spec/08 §5.1`); the planes
/// are bias-and-clamp converted in `U → V → Y` order (`spec/08 §1.3`)
/// and packed in the format's plane order (`spec/08 §5.3`).
pub fn assemble_frame(
    luma: &ReconstructionPlane,
    chroma_v: &ReconstructionPlane,
    chroma_u: &ReconstructionPlane,
    format: OutputFormat,
) -> Result<HostBuffer, AssembleError> {
    let Some(subsampling) = format.subsampling() else {
        return Err(AssembleError::NotPlanar {
            selector: format.selector(),
        });
    };

    // spec/08 §5.1 — chroma geometry check against the luma dimensions.
    let expected = subsampling.chroma_dims(luma.width, luma.height);
    for (role, plane) in [
        (PlaneRole::ChromaV, chroma_v),
        (PlaneRole::ChromaU, chroma_u),
    ] {
        if (plane.width, plane.height) != expected {
            return Err(AssembleError::ChromaGeometryMismatch {
                role,
                got: (plane.width, plane.height),
                expected,
            });
        }
    }

    // spec/08 §1.3 — convert in the binary's U → V → Y decode order.
    // (The conversion is per-plane independent; the order is preserved
    // to mirror the documented walk.)
    let u_out: OutputPlane = chroma_u.to_output_plane();
    let v_out: OutputPlane = chroma_v.to_output_plane();
    let y_out: OutputPlane = luma.to_output_plane();

    let frame = FramePlanes {
        luma: y_out,
        chroma_v: v_out,
        chroma_u: u_out,
    };

    // spec/08 §5.3/§6.2 — planar concatenation. The format is planar by
    // the subsampling gate above, so pack_planar cannot return None.
    Ok(pack_planar(&frame, format).expect("planar format has a plane order"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo5::output::plane_stride;

    /// Build a reconstruction plane filled with a constant coefficient.
    fn recon(w: u32, h: u32, coeff: i32) -> ReconstructionPlane {
        let stride = plane_stride(w);
        ReconstructionPlane::new(w, h, stride, vec![coeff; (stride * h) as usize]).unwrap()
    }

    #[test]
    fn yvu9_end_to_end_geometry() {
        // 8x8 luma, 4:1:0 -> 2x2 chroma. coeff 0 -> pixel 128.
        let hb = assemble_frame(
            &recon(8, 8, 0),
            &recon(2, 2, -512), // V -> 0
            &recon(2, 2, 508),  // U -> 255
            OutputFormat::Yvu9,
        )
        .unwrap();
        // 64 luma + 4 V + 4 U bytes in Y,V,U order.
        assert_eq!(hb.data.len(), 72);
        assert_eq!(hb.plane_bytes(PlaneRole::Luma), &[128u8; 64][..]);
        assert_eq!(hb.plane_bytes(PlaneRole::ChromaV), &[0u8; 4][..]);
        assert_eq!(hb.plane_bytes(PlaneRole::ChromaU), &[255u8; 4][..]);
        // spec/08 §5.3: V before U for YVU9.
        assert!(hb.placement(PlaneRole::ChromaV).offset < hb.placement(PlaneRole::ChromaU).offset);
    }

    #[test]
    fn i420_end_to_end_swaps_chroma() {
        // 4x4 luma, 4:2:0 -> 2x2 chroma; I420 puts U before V.
        let hb = assemble_frame(
            &recon(4, 4, 0),
            &recon(2, 2, 0),
            &recon(2, 2, 0),
            OutputFormat::I420,
        )
        .unwrap();
        assert_eq!(hb.data.len(), 16 + 4 + 4);
        assert!(hb.placement(PlaneRole::ChromaU).offset < hb.placement(PlaneRole::ChromaV).offset);
    }

    #[test]
    fn rejects_non_planar_formats() {
        let e = assemble_frame(
            &recon(4, 4, 0),
            &recon(1, 1, 0),
            &recon(1, 1, 0),
            OutputFormat::Rgb,
        )
        .unwrap_err();
        assert_eq!(e, AssembleError::NotPlanar { selector: 5 });
        let e = assemble_frame(
            &recon(4, 4, 0),
            &recon(2, 2, 0),
            &recon(2, 2, 0),
            OutputFormat::Yuy2,
        )
        .unwrap_err();
        assert_eq!(e, AssembleError::NotPlanar { selector: 2 });
    }

    #[test]
    fn rejects_chroma_geometry_mismatch() {
        // 8x8 luma at 4:1:0 needs 2x2 chroma; give 4x4 V.
        let e = assemble_frame(
            &recon(8, 8, 0),
            &recon(4, 4, 0),
            &recon(2, 2, 0),
            OutputFormat::Yvu9,
        )
        .unwrap_err();
        assert_eq!(
            e,
            AssembleError::ChromaGeometryMismatch {
                role: PlaneRole::ChromaV,
                got: (4, 4),
                expected: (2, 2),
            }
        );
    }

    #[test]
    fn odd_luma_uses_ceil_chroma_dims() {
        // 5x5 luma at 4:1:0 -> ceil(5/4)=2 per axis.
        let hb = assemble_frame(
            &recon(5, 5, 0),
            &recon(2, 2, 0),
            &recon(2, 2, 0),
            OutputFormat::Yvu9,
        )
        .unwrap();
        assert_eq!(hb.data.len(), 25 + 4 + 4);
    }

    #[test]
    fn error_display_cites_spec() {
        let s = AssembleError::NotPlanar { selector: 5 }.to_string();
        assert!(s.contains("spec/08"), "{s}");
        let s = AssembleError::ChromaGeometryMismatch {
            role: PlaneRole::ChromaU,
            got: (1, 1),
            expected: (2, 2),
        }
        .to_string();
        assert!(s.contains("spec/08 §5.1"), "{s}");
    }
}

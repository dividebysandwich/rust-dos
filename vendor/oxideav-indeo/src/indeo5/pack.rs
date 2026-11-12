//! Indeo 5 output-stage planar host-buffer packing.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/08-output-reconstruction.md`
//! §5.3 (per-output-format chroma layout) and §6.2 (host buffer write).
//!
//! For the three **planar** output formats the per-plane writer kernels
//! (`spec/08 §3.1`) write each plane contiguously into the host buffer
//! in the format's plane order (`spec/08 §5.3`):
//!
//! | format | plane order | chroma |
//! | ------ | ----------- | ------ |
//! | `Yvu9` / `IF09` | Y, V, U | 4:1:0 |
//! | `Yv12` | Y, V, U | 4:2:0 |
//! | `I420` / `IYUV` | Y, U, V (U/V swap) | 4:2:0 |
//!
//! Each plane is written at its native resolution — luma at full
//! resolution, chroma at the subsampled resolution the format's
//! [`subsampling`](crate::indeo5::OutputFormat::subsampling) implies. The
//! per-plane offset triple `[ebx+0x10..0x18]` (`spec/08 §3.6`/`§5.3`) is
//! the byte offset of each plane in the host buffer; this module
//! computes that layout and concatenates the tightly-packed planes.
//!
//! The packed (`Yuy2`) and RGB formats are **not** handled here: `Yuy2`
//! interleaves `Y0 U Y1 V` per 4-byte unit whose exact chroma-sampling
//! rule is deferred (`spec/08 §9.4`), and RGB needs the docs-gapped
//! YUV→RGB LUT (`spec/08 §9.1`). [`pack_planar`] returns `None` for
//! those formats.

use crate::indeo5::format::OutputFormat;
use crate::indeo5::planes::{FramePlanes, PlaneRole};

/// One plane's placement within the packed host buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanePlacement {
    /// Which plane this is.
    pub role: PlaneRole,
    /// Byte offset of the plane's first pixel in the host buffer
    /// (`spec/08 §3.6` `[ebx+0x10..0x18]` per-plane offset).
    pub offset: usize,
    /// Plane length in bytes (`width * height`).
    pub len: usize,
}

/// A packed planar host buffer (`spec/08 §5.3`/`§6.2`).
///
/// `data` is the concatenation of the three planes in the format's
/// plane order; `placements` records each plane's `(role, offset, len)`
/// in buffer order so a consumer can locate any plane by role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostBuffer {
    /// The packed plane bytes (all three planes concatenated).
    pub data: Vec<u8>,
    /// Per-plane placement in buffer order (`spec/08 §3.6`).
    pub placements: [PlanePlacement; 3],
}

impl HostBuffer {
    /// The `(offset, len)` placement of a given plane role.
    pub fn placement(&self, role: PlaneRole) -> PlanePlacement {
        // Exactly one placement matches each role by construction.
        *self
            .placements
            .iter()
            .find(|p| p.role == role)
            .expect("every role is placed once")
    }

    /// The bytes of a given plane role.
    pub fn plane_bytes(&self, role: PlaneRole) -> &[u8] {
        let p = self.placement(role);
        &self.data[p.offset..p.offset + p.len]
    }
}

/// `spec/08 §5.3`/`§6.2` — pack the three reconstructed planes into a
/// planar host buffer for a planar output format.
///
/// The planes are concatenated in `format.plane_order()` order, each at
/// its supplied native resolution (luma full, chroma subsampled). The
/// caller is responsible for having built the chroma planes at the
/// resolution the format implies (e.g. via
/// [`upsample_chroma`](crate::indeo5::upsample_chroma) is **not** used
/// here — planar output keeps chroma subsampled).
///
/// Returns `None` for the packed (`Yuy2`) and RGB formats, which are not
/// simple planar concatenations (`spec/08 §9.4`/`§9.1`).
pub fn pack_planar(planes: &FramePlanes, format: OutputFormat) -> Option<HostBuffer> {
    let order = format.plane_order()?;

    // Compute each plane's byte length and running offset.
    let mut data = Vec::new();
    let mut placements: [PlanePlacement; 3] = [PlanePlacement {
        role: PlaneRole::Luma,
        offset: 0,
        len: 0,
    }; 3];

    let mut offset = 0usize;
    for (i, &role) in order.iter().enumerate() {
        let plane = planes.plane(role);
        let len = plane.pixels.len();
        placements[i] = PlanePlacement { role, offset, len };
        data.extend_from_slice(&plane.pixels);
        offset += len;
    }

    Some(HostBuffer { data, placements })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo5::output::OutputPlane;

    fn plane(w: u32, h: u32, fill: u8) -> OutputPlane {
        OutputPlane {
            width: w,
            height: h,
            pixels: vec![fill; (w * h) as usize],
        }
    }

    // Luma 4x4 (fill 1), chroma 1x1 for 4:1:0 (V=2, U=3).
    fn frame_410() -> FramePlanes {
        FramePlanes {
            luma: plane(4, 4, 1),
            chroma_v: plane(1, 1, 2),
            chroma_u: plane(1, 1, 3),
        }
    }

    #[test]
    fn yvu9_packs_y_v_u() {
        // spec/08 §5.3: Y, then V, then U.
        let hb = pack_planar(&frame_410(), OutputFormat::Yvu9).unwrap();
        // 16 luma + 1 V + 1 U.
        assert_eq!(hb.data.len(), 18);
        assert_eq!(&hb.data[0..16], &[1u8; 16]);
        assert_eq!(hb.data[16], 2); // V
        assert_eq!(hb.data[17], 3); // U
        // Placements in buffer order: Y, V, U.
        assert_eq!(hb.placements[0].role, PlaneRole::Luma);
        assert_eq!(hb.placements[1].role, PlaneRole::ChromaV);
        assert_eq!(hb.placements[2].role, PlaneRole::ChromaU);
        assert_eq!(hb.placements[1].offset, 16);
        assert_eq!(hb.placements[2].offset, 17);
    }

    #[test]
    fn i420_swaps_u_and_v() {
        // spec/08 §5.3: I420 is Y, U, V (swap vs YV12).
        let frame = FramePlanes {
            luma: plane(4, 4, 1),
            chroma_v: plane(2, 2, 2),
            chroma_u: plane(2, 2, 3),
        };
        let hb = pack_planar(&frame, OutputFormat::I420).unwrap();
        // 16 luma + 4 U + 4 V.
        assert_eq!(hb.data.len(), 24);
        assert_eq!(hb.placements[1].role, PlaneRole::ChromaU);
        assert_eq!(hb.placements[2].role, PlaneRole::ChromaV);
        // U (fill 3) comes before V (fill 2).
        assert_eq!(&hb.data[16..20], &[3u8; 4]);
        assert_eq!(&hb.data[20..24], &[2u8; 4]);
    }

    #[test]
    fn yv12_keeps_v_before_u() {
        let frame = FramePlanes {
            luma: plane(4, 4, 1),
            chroma_v: plane(2, 2, 2),
            chroma_u: plane(2, 2, 3),
        };
        let hb = pack_planar(&frame, OutputFormat::Yv12).unwrap();
        assert_eq!(hb.placements[1].role, PlaneRole::ChromaV);
        assert_eq!(hb.placements[2].role, PlaneRole::ChromaU);
        assert_eq!(&hb.data[16..20], &[2u8; 4]); // V first
        assert_eq!(&hb.data[20..24], &[3u8; 4]); // U second
    }

    #[test]
    fn plane_bytes_lookup_by_role() {
        let hb = pack_planar(&frame_410(), OutputFormat::Yvu9).unwrap();
        assert_eq!(hb.plane_bytes(PlaneRole::Luma), &[1u8; 16]);
        assert_eq!(hb.plane_bytes(PlaneRole::ChromaV), &[2u8]);
        assert_eq!(hb.plane_bytes(PlaneRole::ChromaU), &[3u8]);
    }

    #[test]
    fn packed_and_rgb_return_none() {
        // spec/08 §9.4 / §9.1: not a planar concat.
        assert!(pack_planar(&frame_410(), OutputFormat::Yuy2).is_none());
        assert!(pack_planar(&frame_410(), OutputFormat::Rgb).is_none());
    }

    #[test]
    fn offsets_are_cumulative() {
        let hb = pack_planar(&frame_410(), OutputFormat::Yvu9).unwrap();
        let mut acc = 0;
        for p in &hb.placements {
            assert_eq!(p.offset, acc);
            acc += p.len;
        }
        assert_eq!(acc, hb.data.len());
    }
}

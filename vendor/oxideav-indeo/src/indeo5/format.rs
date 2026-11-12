//! Indeo 5 output-format dispatch (FOURCC routing + selector).
//!
//! Spec source: `docs/video/indeo/indeo5/spec/08-output-reconstruction.md`
//! §2.2 (output-format query / selector), §2.3 (`ICDecompressBegin`
//! FOURCC dispatch), and §5.3 (per-format chroma layout).
//!
//! Indeo 5 supports five host output formats (`spec/08 §2.2`/`§2.3`),
//! selected at `ICDecompressBegin` time from the host's requested
//! `biCompression` FOURCC and cached in the codec-instance selector slot
//! `[ebx+0x70]`:
//!
//! | selector | format | FOURCC(s) | chroma | plane order |
//! | -------- | ------ | --------- | ------ | ----------- |
//! | 1 | `Yvu9` | `IF09` / `YVU9` | 4:1:0 planar | Y, V, U |
//! | 2 | `Yuy2` | `YUY2` | 4:2:2 packed | interleaved |
//! | 3 | `Yv12` | `YV12` | 4:2:0 planar | Y, V, U |
//! | 4 | `I420` | `I420` / `IYUV` | 4:2:0 planar | Y, U, V |
//! | 5 | `Rgb`  | `BI_RGB` (0) | none (post YUV→RGB) | packed |
//!
//! The RGB path's per-pixel YUV→RGB conversion needs the 3 KB
//! per-instance LUT (`spec/08 §3.7`/`§4`) whose contents are an
//! Extractor docs-gap (`spec/08 §9.1`); this module routes to `Rgb` but
//! the RGB pixel conversion itself is not implemented here.

use crate::indeo5::chroma::ChromaSubsampling;
use crate::indeo5::planes::PlaneRole;

/// `spec/08 §2.2` — `'IF09'` FOURCC (`0x39304649`, the YVU9-family tag
/// the codec advertises for its own output).
pub const FOURCC_IF09: u32 = 0x39304649;
/// `spec/08 §2.3` — `'YVU9'` FOURCC (`0x39555659`).
pub const FOURCC_YVU9: u32 = 0x39555659;
/// `spec/08 §2.2` — `'YV12'` FOURCC (`0x32315659`).
pub const FOURCC_YV12: u32 = 0x32315659;
/// `spec/08 §2.2` — `'I420'` FOURCC (`0x30323449`).
pub const FOURCC_I420: u32 = 0x30323449;
/// `spec/08 §2.3` — `'IYUV'` FOURCC (`0x56555949`), an I420 alias.
pub const FOURCC_IYUV: u32 = 0x56555949;
/// `spec/08 §2.2` — `'YUY2'` FOURCC (`0x32595559`).
pub const FOURCC_YUY2: u32 = 0x32595559;
/// `spec/08 §2.2` — `BI_RGB` `biCompression` value (`0`, uncompressed
/// RGB).
pub const BI_RGB: u32 = 0;

/// The host chroma layout for one output format (`spec/08 §5.3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChromaLayout {
    /// 4:1:0 planar (`YVU9`/`IF09`).
    Planar410,
    /// 4:2:0 planar (`YV12`/`I420`).
    Planar420,
    /// 4:2:2 packed (`YUY2`, `Y0 U Y1 V` per 4-byte unit).
    Packed422,
    /// No chroma plane — RGB output after per-pixel YUV→RGB.
    Rgb,
}

/// An Indeo 5 host output format (`spec/08 §2.2`/`§2.3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputFormat {
    /// `YVU9` / `IF09` — 4:1:0 planar, plane order Y, V, U.
    Yvu9,
    /// `YUY2` — 4:2:2 packed (`Y0 U Y1 V`).
    Yuy2,
    /// `YV12` — 4:2:0 planar, plane order Y, V, U.
    Yv12,
    /// `I420` / `IYUV` — 4:2:0 planar, plane order Y, U, V (U/V swap
    /// vs `YV12`).
    I420,
    /// `BI_RGB` — packed RGB after per-pixel YUV→RGB (LUT docs-gapped,
    /// `spec/08 §9.1`).
    Rgb,
}

impl OutputFormat {
    /// `spec/08 §2.3` — route a host `biCompression` FOURCC to its
    /// output format. `BI_RGB` (`0`) → [`OutputFormat::Rgb`]. Returns
    /// `None` for an unrecognised FOURCC (the codec's error path,
    /// `ICERR_BADFORMAT`).
    pub fn from_fourcc(fourcc: u32) -> Option<Self> {
        match fourcc {
            FOURCC_IF09 | FOURCC_YVU9 => Some(OutputFormat::Yvu9),
            FOURCC_YUY2 => Some(OutputFormat::Yuy2),
            FOURCC_YV12 => Some(OutputFormat::Yv12),
            FOURCC_I420 | FOURCC_IYUV => Some(OutputFormat::I420),
            BI_RGB => Some(OutputFormat::Rgb),
            _ => None,
        }
    }

    /// `spec/08 §2.2` — the codec-instance selector value stored at
    /// `[ebx+0x70]` for this format (`1`=Yvu9, `2`=Yuy2, `3`=Yv12,
    /// `4`=I420, `5`=Rgb).
    pub fn selector(self) -> u32 {
        match self {
            OutputFormat::Yvu9 => 1,
            OutputFormat::Yuy2 => 2,
            OutputFormat::Yv12 => 3,
            OutputFormat::I420 => 4,
            OutputFormat::Rgb => 5,
        }
    }

    /// Inverse of [`selector`](OutputFormat::selector) — the format for
    /// a `[ebx+0x70]` selector value (`spec/08 §2.2`).
    pub fn from_selector(sel: u32) -> Option<Self> {
        match sel {
            1 => Some(OutputFormat::Yvu9),
            2 => Some(OutputFormat::Yuy2),
            3 => Some(OutputFormat::Yv12),
            4 => Some(OutputFormat::I420),
            5 => Some(OutputFormat::Rgb),
            _ => None,
        }
    }

    /// `spec/08 §5.3` — the host chroma layout for this format.
    pub fn chroma_layout(self) -> ChromaLayout {
        match self {
            OutputFormat::Yvu9 => ChromaLayout::Planar410,
            OutputFormat::Yv12 | OutputFormat::I420 => ChromaLayout::Planar420,
            OutputFormat::Yuy2 => ChromaLayout::Packed422,
            OutputFormat::Rgb => ChromaLayout::Rgb,
        }
    }

    /// `spec/08 §5.1`/`§5.3` — the decode-internal chroma subsampling
    /// the reconstruction planes carry for this output format
    /// (4:1:0 for `Yvu9`, 4:2:0 for the `Yv12`/`I420` family). `None`
    /// for `Yuy2`/`Rgb`, whose host layout is derived by re-packing /
    /// converting the decoded planes rather than by a fixed
    /// reconstruction subsampling.
    pub fn subsampling(self) -> Option<ChromaSubsampling> {
        match self {
            OutputFormat::Yvu9 => Some(ChromaSubsampling::Yvu9),
            OutputFormat::Yv12 | OutputFormat::I420 => Some(ChromaSubsampling::Yv12),
            OutputFormat::Yuy2 | OutputFormat::Rgb => None,
        }
    }

    /// `spec/08 §5.3` — the host planar plane order (`Y, V, U` for
    /// `Yvu9`/`Yv12`; `Y, U, V` for `I420`). `None` for the packed
    /// (`Yuy2`) and RGB formats, which are not plane-ordered.
    pub fn plane_order(self) -> Option<[PlaneRole; 3]> {
        match self {
            OutputFormat::Yvu9 | OutputFormat::Yv12 => {
                Some([PlaneRole::Luma, PlaneRole::ChromaV, PlaneRole::ChromaU])
            }
            OutputFormat::I420 => Some([PlaneRole::Luma, PlaneRole::ChromaU, PlaneRole::ChromaV]),
            OutputFormat::Yuy2 | OutputFormat::Rgb => None,
        }
    }

    /// Whether this is a planar YUV output (`Yvu9`/`Yv12`/`I420`).
    pub fn is_planar(self) -> bool {
        self.plane_order().is_some()
    }

    /// Whether this is the packed `Yuy2` output.
    pub fn is_packed(self) -> bool {
        matches!(self, OutputFormat::Yuy2)
    }

    /// Whether this is the RGB output (LUT-gated, `spec/08 §9.1`).
    pub fn is_rgb(self) -> bool {
        matches!(self, OutputFormat::Rgb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourcc_ascii_round_trips() {
        // Confirm the FOURCC constants decode to their ASCII tags.
        assert_eq!(&FOURCC_IF09.to_le_bytes(), b"IF09");
        assert_eq!(&FOURCC_YVU9.to_le_bytes(), b"YVU9");
        assert_eq!(&FOURCC_YV12.to_le_bytes(), b"YV12");
        assert_eq!(&FOURCC_I420.to_le_bytes(), b"I420");
        assert_eq!(&FOURCC_IYUV.to_le_bytes(), b"IYUV");
        assert_eq!(&FOURCC_YUY2.to_le_bytes(), b"YUY2");
    }

    #[test]
    fn from_fourcc_routes_all_formats() {
        // spec/08 §2.3.
        assert_eq!(
            OutputFormat::from_fourcc(FOURCC_IF09),
            Some(OutputFormat::Yvu9)
        );
        assert_eq!(
            OutputFormat::from_fourcc(FOURCC_YVU9),
            Some(OutputFormat::Yvu9)
        );
        assert_eq!(
            OutputFormat::from_fourcc(FOURCC_YUY2),
            Some(OutputFormat::Yuy2)
        );
        assert_eq!(
            OutputFormat::from_fourcc(FOURCC_YV12),
            Some(OutputFormat::Yv12)
        );
        assert_eq!(
            OutputFormat::from_fourcc(FOURCC_I420),
            Some(OutputFormat::I420)
        );
        assert_eq!(
            OutputFormat::from_fourcc(FOURCC_IYUV),
            Some(OutputFormat::I420)
        );
        assert_eq!(OutputFormat::from_fourcc(BI_RGB), Some(OutputFormat::Rgb));
    }

    #[test]
    fn from_fourcc_rejects_unknown() {
        assert_eq!(OutputFormat::from_fourcc(0x12345678), None);
    }

    #[test]
    fn selector_round_trips() {
        // spec/08 §2.2 selector table.
        for f in [
            OutputFormat::Yvu9,
            OutputFormat::Yuy2,
            OutputFormat::Yv12,
            OutputFormat::I420,
            OutputFormat::Rgb,
        ] {
            assert_eq!(OutputFormat::from_selector(f.selector()), Some(f));
        }
        assert_eq!(OutputFormat::Yvu9.selector(), 1);
        assert_eq!(OutputFormat::Rgb.selector(), 5);
        assert_eq!(OutputFormat::from_selector(0), None);
        assert_eq!(OutputFormat::from_selector(6), None);
    }

    #[test]
    fn chroma_layouts() {
        // spec/08 §5.3.
        assert_eq!(OutputFormat::Yvu9.chroma_layout(), ChromaLayout::Planar410);
        assert_eq!(OutputFormat::Yv12.chroma_layout(), ChromaLayout::Planar420);
        assert_eq!(OutputFormat::I420.chroma_layout(), ChromaLayout::Planar420);
        assert_eq!(OutputFormat::Yuy2.chroma_layout(), ChromaLayout::Packed422);
        assert_eq!(OutputFormat::Rgb.chroma_layout(), ChromaLayout::Rgb);
    }

    #[test]
    fn subsampling_per_format() {
        assert_eq!(
            OutputFormat::Yvu9.subsampling(),
            Some(ChromaSubsampling::Yvu9)
        );
        assert_eq!(
            OutputFormat::Yv12.subsampling(),
            Some(ChromaSubsampling::Yv12)
        );
        assert_eq!(
            OutputFormat::I420.subsampling(),
            Some(ChromaSubsampling::Yv12)
        );
        assert_eq!(OutputFormat::Yuy2.subsampling(), None);
        assert_eq!(OutputFormat::Rgb.subsampling(), None);
    }

    #[test]
    fn plane_order_swaps_u_v_for_i420() {
        // spec/08 §5.3: YV12 is Y,V,U; I420 swaps to Y,U,V.
        assert_eq!(
            OutputFormat::Yv12.plane_order(),
            Some([PlaneRole::Luma, PlaneRole::ChromaV, PlaneRole::ChromaU])
        );
        assert_eq!(
            OutputFormat::I420.plane_order(),
            Some([PlaneRole::Luma, PlaneRole::ChromaU, PlaneRole::ChromaV])
        );
        assert_eq!(
            OutputFormat::Yvu9.plane_order(),
            Some([PlaneRole::Luma, PlaneRole::ChromaV, PlaneRole::ChromaU])
        );
        assert_eq!(OutputFormat::Yuy2.plane_order(), None);
        assert_eq!(OutputFormat::Rgb.plane_order(), None);
    }

    #[test]
    fn format_class_predicates() {
        assert!(OutputFormat::Yvu9.is_planar());
        assert!(OutputFormat::I420.is_planar());
        assert!(!OutputFormat::Yuy2.is_planar());
        assert!(OutputFormat::Yuy2.is_packed());
        assert!(OutputFormat::Rgb.is_rgb());
        assert!(!OutputFormat::Yv12.is_rgb());
    }
}

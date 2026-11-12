//! Indeo 5 output-stage per-plane record set and iteration order.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/08-output-reconstruction.md`
//! §1.1 (per-plane record layout) and §1.3 (per-plane iteration order).
//!
//! The codec instance carries three per-plane records after wavelet
//! recomposition, in **`Y, V, U`** memory order (`spec/08 §1.1`, the
//! `[ebx+0x17c]`/`[ebx+0x1b0]`/`[ebx+0x1e4]` slots). The output writer
//! iterates them in **reverse** — `U → V → Y` (`spec/08 §1.3`, the
//! `plane_record[2]` start with a `-0x34` decrement per iteration). The
//! reverse walk is a decode-time convenience (it matches the per-frame
//! reconstruction-arena fill order `spec/07 §1.5`); each plane is still
//! written to its correct output position regardless of visit order.
//!
//! Per-plane band count (`spec/08 §1.1`) is `3·levels + 1` — `1` for a
//! non-decomposed plane (`chroma_levels = 0` chroma, or `luma_levels =
//! 0` luma) and `4` for the `luma_levels = 1` wavelet-decomposed luma
//! plane. The band count selects the per-plane writer in the `spec/08
//! §3.1` 8-way dispatch (`1` vs `4`).

use crate::indeo5::output::OutputPlane;

/// One of the three Indeo 5 planes (`spec/08 §1.1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneRole {
    /// Luma (Y). Record slot `[ebx+0x17c]`, record index 0.
    Luma,
    /// Chroma V. Record slot `[ebx+0x1b0]`, record index 1.
    ChromaV,
    /// Chroma U. Record slot `[ebx+0x1e4]`, record index 2.
    ChromaU,
}

/// `spec/08 §1.1` — the per-plane record layout order in the codec
/// instance (`Y, V, U`).
pub const PLANE_RECORD_ORDER: [PlaneRole; 3] =
    [PlaneRole::Luma, PlaneRole::ChromaV, PlaneRole::ChromaU];

/// `spec/08 §1.3` — the output-writer per-plane iteration order
/// (`U → V → Y`, the reverse of the record layout).
pub const OUTPUT_ITERATION_ORDER: [PlaneRole; 3] =
    [PlaneRole::ChromaU, PlaneRole::ChromaV, PlaneRole::Luma];

impl PlaneRole {
    /// The plane's record index (`0` = Y, `1` = V, `2` = U) in the
    /// `spec/08 §1.1` record layout.
    #[inline]
    pub fn record_index(self) -> usize {
        match self {
            PlaneRole::Luma => 0,
            PlaneRole::ChromaV => 1,
            PlaneRole::ChromaU => 2,
        }
    }

    /// Whether this is the luma plane.
    #[inline]
    pub fn is_luma(self) -> bool {
        matches!(self, PlaneRole::Luma)
    }

    /// Whether this is a chroma plane.
    #[inline]
    pub fn is_chroma(self) -> bool {
        !self.is_luma()
    }
}

/// `spec/08 §1.1` — the per-plane band count from a decomposition-level
/// count: `3·levels + 1` (`spec/02 §1.5`). `0` levels → `1` band
/// (non-decomposed), `1` level → `4` bands (the wavelet-decomposed luma
/// case), `2` levels → `7` bands.
#[inline]
pub fn num_bands(levels: u32) -> u32 {
    3 * levels + 1
}

/// The three reconstructed output planes of one frame, held in
/// `Y, V, U` record order (`spec/08 §1.1`).
///
/// [`iter_output_order`](FramePlanes::iter_output_order) yields them in
/// the `spec/08 §1.3` `U → V → Y` writer order; [`plane`] fetches by
/// role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramePlanes {
    /// Luma plane (Y).
    pub luma: OutputPlane,
    /// Chroma V plane (already upsampled to luma resolution, or at
    /// subsampled resolution per the output-format contract).
    pub chroma_v: OutputPlane,
    /// Chroma U plane.
    pub chroma_u: OutputPlane,
}

impl FramePlanes {
    /// Fetch the plane for a given role (`spec/08 §1.1`).
    #[inline]
    pub fn plane(&self, role: PlaneRole) -> &OutputPlane {
        match role {
            PlaneRole::Luma => &self.luma,
            PlaneRole::ChromaV => &self.chroma_v,
            PlaneRole::ChromaU => &self.chroma_u,
        }
    }

    /// Iterate the three planes in the `spec/08 §1.3` output-writer
    /// order (`U → V → Y`), yielding `(role, plane)` pairs.
    pub fn iter_output_order(&self) -> impl Iterator<Item = (PlaneRole, &OutputPlane)> {
        OUTPUT_ITERATION_ORDER
            .iter()
            .map(move |&role| (role, self.plane(role)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(w: u32, h: u32, fill: u8) -> OutputPlane {
        OutputPlane {
            width: w,
            height: h,
            pixels: vec![fill; (w * h) as usize],
        }
    }

    #[test]
    fn record_order_is_y_v_u() {
        // spec/08 §1.1.
        assert_eq!(
            PLANE_RECORD_ORDER,
            [PlaneRole::Luma, PlaneRole::ChromaV, PlaneRole::ChromaU]
        );
        assert_eq!(PlaneRole::Luma.record_index(), 0);
        assert_eq!(PlaneRole::ChromaV.record_index(), 1);
        assert_eq!(PlaneRole::ChromaU.record_index(), 2);
    }

    #[test]
    fn output_order_is_u_v_y() {
        // spec/08 §1.3: reverse of record layout.
        assert_eq!(
            OUTPUT_ITERATION_ORDER,
            [PlaneRole::ChromaU, PlaneRole::ChromaV, PlaneRole::Luma]
        );
    }

    #[test]
    fn role_classification() {
        assert!(PlaneRole::Luma.is_luma());
        assert!(!PlaneRole::Luma.is_chroma());
        assert!(PlaneRole::ChromaU.is_chroma());
        assert!(PlaneRole::ChromaV.is_chroma());
    }

    #[test]
    fn num_bands_from_levels() {
        // spec/08 §1.1 / spec/02 §1.5: 3*levels + 1.
        assert_eq!(num_bands(0), 1);
        assert_eq!(num_bands(1), 4);
        assert_eq!(num_bands(2), 7);
    }

    #[test]
    fn plane_fetch_by_role() {
        let f = FramePlanes {
            luma: plane(4, 4, 1),
            chroma_v: plane(1, 1, 2),
            chroma_u: plane(1, 1, 3),
        };
        assert_eq!(f.plane(PlaneRole::Luma).pixels[0], 1);
        assert_eq!(f.plane(PlaneRole::ChromaV).pixels[0], 2);
        assert_eq!(f.plane(PlaneRole::ChromaU).pixels[0], 3);
    }

    #[test]
    fn iter_visits_u_v_y_in_order() {
        let f = FramePlanes {
            luma: plane(4, 4, 1),
            chroma_v: plane(1, 1, 2),
            chroma_u: plane(1, 1, 3),
        };
        let roles: Vec<PlaneRole> = f.iter_output_order().map(|(r, _)| r).collect();
        assert_eq!(
            roles,
            vec![PlaneRole::ChromaU, PlaneRole::ChromaV, PlaneRole::Luma]
        );
        // First-yielded plane is U (fill 3).
        let first = f.iter_output_order().next().unwrap();
        assert_eq!(first.1.pixels[0], 3);
    }
}

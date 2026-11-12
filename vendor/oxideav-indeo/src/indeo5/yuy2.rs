//! Indeo 5 packed `YUY2` host output (`spec/08 §2.2` format 2, `§3.4`
//! chroma-upsampling writer) — the vendor decoder's 4:2:2 view of a
//! 4:1:0 picture, reproduced byte-exactly.
//!
//! The vendor's `YUY2` buffer is `width × height × 2` bytes of `Y0 U
//! Y1 V` units. Its chroma is **not** the native 4:1:0 plane
//! replicated: each chroma plane is first doubled along both axes by
//! a separable, sample-cosited linear interpolation — the horizontal
//! pass places the native sample at every even column and the
//! truncating average `(left + right) >> 1` at every odd one, then the
//! vertical pass does the same over the interpolated rows (`(above +
//! below) >> 1`, computed from the already horizontally interpolated
//! values) — with the plane's last column / row replicated past the
//! edge; the resulting `(w/2) × (h/2)` plane is then written twice per
//! row pair (rows `2k` and `2k+1` identical) to fill the 4:2:2 grid.
//!
//! **Fixture-arbitrated (r459).** On `fixtures/intra-320x240-indeo5`
//! the even/even positions of the vendor's chroma equal the crate's
//! native planes sample-for-sample, every odd position is the
//! truncating two-sample average along its axis, and the diagonal
//! positions are the horizontal-then-vertical composition (0 / 38 400
//! mismatches per plane; the vertical-then-horizontal order, the
//! four-sample `>> 2` and every round-half-up variant mismatch
//! 98–2 000 positions). With the luma plane pixel-exact, the whole
//! 153 600-byte host buffer reproduces the vendor's reference decode.
//! The filter's location among the `spec/08 §3` writers is not pinned
//! by the spec text (which describes replication); the numbers are.

use super::pack::HostBuffer;
use super::planes::PlaneRole;

/// Double a chroma plane along both axes with the vendor's cosited
/// linear interpolation: even positions carry the native samples, odd
/// positions the truncating average of their two neighbours,
/// horizontal pass first, edges replicated. Returns a `(2·w) × (2·h)`
/// plane.
pub fn upsample_chroma_2x(plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    assert_eq!(plane.len(), w * h, "chroma plane geometry");
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let ow = 2 * w;
    // Horizontal pass: h rows of 2w.
    let mut wide = vec![0u8; ow * h];
    for y in 0..h {
        let row = &plane[y * w..y * w + w];
        let out = &mut wide[y * ow..y * ow + ow];
        for x in 0..w {
            let a = u16::from(row[x]);
            let b = u16::from(row[(x + 1).min(w - 1)]);
            out[2 * x] = row[x];
            out[2 * x + 1] = ((a + b) >> 1) as u8;
        }
    }
    // Vertical pass over the interpolated rows: 2h rows of 2w.
    let mut out = vec![0u8; ow * 2 * h];
    for y in 0..h {
        let above = &wide[y * ow..y * ow + ow];
        let below_y = (y + 1).min(h - 1);
        let below = &wide[below_y * ow..below_y * ow + ow];
        out[2 * y * ow..2 * y * ow + ow].copy_from_slice(above);
        let mid = &mut out[(2 * y + 1) * ow..(2 * y + 1) * ow + ow];
        for x in 0..ow {
            mid[x] = ((u16::from(above[x]) + u16::from(below[x])) >> 1) as u8;
        }
    }
    out
}

/// Pack a decoded 4:1:0 host buffer as the vendor's `YUY2` output for
/// a `width × height` luma picture: luma at full resolution, each
/// chroma plane doubled by [`upsample_chroma_2x`] and then
/// row-duplicated to 4:2:2, interleaved as `Y0 U Y1 V` per pair of
/// luma samples. Returns `width × height × 2` bytes; `None` when the
/// width is odd (the packed unit needs sample pairs) or the buffer's
/// planes are not a `width × height` luma plane with 4:1:0
/// (`ceil(dim / 4)`) chroma planes.
pub fn pack_yuy2(buf: &HostBuffer, width: usize, height: usize) -> Option<Vec<u8>> {
    let (w, h) = (width, height);
    let luma = buf.plane_bytes(PlaneRole::Luma);
    let u = buf.plane_bytes(PlaneRole::ChromaU);
    let v = buf.plane_bytes(PlaneRole::ChromaV);
    if w == 0 || h == 0 || w % 2 != 0 || luma.len() != w * h {
        return None;
    }
    let cw = w.div_ceil(4);
    let ch = h.div_ceil(4);
    if u.len() != cw * ch || v.len() != cw * ch {
        return None;
    }
    let u2 = upsample_chroma_2x(u, cw, ch);
    let v2 = upsample_chroma_2x(v, cw, ch);
    let uw = 2 * cw;
    let mut out = vec![0u8; w * h * 2];
    for y in 0..h {
        // 4:2:2 row y reads interpolated chroma row y / 2 (row pairs
        // are identical).
        let cy = (y / 2).min(2 * ch - 1);
        let urow = &u2[cy * uw..cy * uw + uw];
        let vrow = &v2[cy * uw..cy * uw + uw];
        let lrow = &luma[y * w..y * w + w];
        let orow = &mut out[y * w * 2..(y + 1) * w * 2];
        for x2 in 0..w / 2 {
            let cx = x2.min(uw - 1);
            orow[4 * x2] = lrow[2 * x2];
            orow[4 * x2 + 1] = urow[cx];
            orow[4 * x2 + 2] = lrow[2 * x2 + 1];
            orow[4 * x2 + 3] = vrow[cx];
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsample_is_cosited_and_truncating() {
        // 2x2 plane -> 4x4.
        let p = [10u8, 20, 30, 41];
        let o = upsample_chroma_2x(&p, 2, 2);
        // Row 0: horizontal pass with the right edge replicated.
        assert_eq!(&o[0..4], &[10, 15, 20, 20]);
        // Row 1 = truncating average of the interpolated rows 0 and 2:
        // (10+30)>>1, (15+35)>>1, (20+41)>>1, (20+41)>>1.
        assert_eq!(&o[4..8], &[20, 25, 30, 30]);
        assert_eq!(&o[8..12], &[30, 35, 41, 41]);
        assert_eq!(&o[12..16], &[30, 35, 41, 41]); // bottom edge replicate
    }

    #[test]
    fn pack_geometry() {
        // 8x4 luma, 2x1 chroma.
        let mut data = vec![0u8; 32];
        for (i, b) in data.iter_mut().enumerate() {
            *b = i as u8;
        }
        data.extend_from_slice(&[100, 120]); // V (YVU9 order)
        data.extend_from_slice(&[200, 220]); // U
        use super::super::pack::PlanePlacement;
        let buf = HostBuffer {
            data,
            placements: [
                PlanePlacement {
                    role: PlaneRole::Luma,
                    offset: 0,
                    len: 32,
                },
                PlanePlacement {
                    role: PlaneRole::ChromaV,
                    offset: 32,
                    len: 2,
                },
                PlanePlacement {
                    role: PlaneRole::ChromaU,
                    offset: 34,
                    len: 2,
                },
            ],
        };
        let out = pack_yuy2(&buf, 8, 4).unwrap();
        assert!(pack_yuy2(&buf, 16, 2).is_none()); // chroma geometry mismatch
        assert_eq!(out.len(), 64);
        // Row 0: Y0 U Y1 V ... with U = [200, 210, 220, 220].
        assert_eq!(&out[0..8], &[0, 200, 1, 100, 2, 210, 3, 110]);
        assert_eq!(&out[8..16], &[4, 220, 5, 120, 6, 220, 7, 120]);
        // Rows 0/1 share chroma row 0; rows 2/3 share row 1 (= row 0
        // here: bottom edge).
        assert_eq!(out[16 + 1], 200);
        assert_eq!(out[48 + 1], 200);
    }
}

//! Band recombination into the 8-bit output planes (spec 08).

use super::band::BandGeom;

/// A window in plane samples: first row, first column, height, width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Window {
    pub y: usize,
    pub x: usize,
    pub h: usize,
    pub w: usize,
}

/// An 8-bit output plane with room for the columns and rows the
/// recombination writes past the visible picture (spec 08, 8.3).
#[derive(Debug, Clone)]
pub(crate) struct OutPlane {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub data: Vec<u8>,
}

impl OutPlane {
    pub fn new(width: usize, height: usize) -> OutPlane {
        let stride = width.next_multiple_of(32) + 32;
        let rows = height.next_multiple_of(2) + 2;
        OutPlane {
            width,
            height,
            stride,
            data: vec![0; stride * rows],
        }
    }
}

/// One band plane (spec 08, 8.5): `pixel = clamp(s, -128, 127) + 128`
/// over the window, columns widened to multiples of 16.
pub(crate) fn one_band(area: &[i16], b: &BandGeom, win: Window, out: &mut OutPlane) {
    let x0 = win.x & !15;
    let w = (15 + win.x + win.w - x0) & !15;
    // Never write past the band's pitch or the output plane.
    let w = w
        .min(b.pitch.saturating_sub(x0))
        .min(out.stride.saturating_sub(x0));
    for row in win.y..win.y + win.h {
        if row >= b.height.next_multiple_of(16) || row >= out.data.len() / out.stride {
            break;
        }
        let src = &area[b.offset + row * b.pitch + x0..][..w];
        let dst = &mut out.data[row * out.stride + x0..][..w];
        for (d, &s) in dst.iter_mut().zip(src) {
            *d = (s.clamp(-128, 127) + 128) as u8;
        }
    }
}

/// Four band plane (spec 08, 8.6): the DLL's integer synthesis filter
/// over the window, with rows started on an even row and columns widened
/// to multiples of 32.
pub(crate) fn four_band(area: &[i16], bands: &[BandGeom], win: Window, out: &mut OutPlane) {
    let mut y = win.y;
    let mut h = win.h;
    h += y & 1;
    y &= !1;
    let x0 = win.x & !31;
    let wpp = (31 + win.x + win.w - x0) & !31;
    let pitch = bands[0].pitch;
    let nbr = h.div_ceil(2);
    let nbc = (wpp / 2).min(pitch.saturating_sub(x0 / 2));
    let wpp = wpp.min(out.stride.saturating_sub(x0));
    if nbr == 0 || nbc == 0 || wpp == 0 {
        return;
    }
    let br0 = y / 2;
    let bc0 = x0 / 2;
    let (lo, vo, ho, dof) = (
        bands[0].offset as isize,
        bands[1].offset as isize,
        bands[2].offset as isize,
        bands[3].offset as isize,
    );
    let p = pitch as isize;
    // Each output row pair reads ten band rows: L and H at r and rn_c,
    // V and D at r, rp and rn_m. They are fetched once per row pair into
    // `n` entries each, entry k holding band column bc0 - 1 + k; samples
    // outside the plane area read zero.
    let n = nbc + 2;
    let fetch = |dst: &mut [i32], base: isize, r: isize| {
        let start = base + r * p + bc0 as isize - 1;
        if start >= 0 && start as usize + dst.len() <= area.len() {
            for (d, &s) in dst.iter_mut().zip(&area[start as usize..]) {
                *d = s as i32;
            }
        } else {
            for (k, d) in dst.iter_mut().enumerate() {
                let i = start + k as isize;
                *d = if i >= 0 && (i as usize) < area.len() {
                    area[i as usize] as i32
                } else {
                    0
                };
            }
        }
    };
    let mut buf = vec![0i32; 10 * n];
    // Per band column: V's s1 and t1, D's S and T.
    let (mut s1, mut t1, mut ds, mut dt) = (vec![0; n], vec![0; n], vec![0; n], vec![0; n]);
    let out_rows = out.data.len() / out.stride;
    for i in 0..nbr {
        let r = (br0 + i) as isize;
        let last_row = i == nbr - 1;
        let rp = if i == 0 { r } else { r - 1 };
        let rn_c = if last_row { r } else { r + 1 };
        let rn_m = if last_row { r - 1 } else { r + 1 };
        let orow = y + 2 * i;
        if orow + 1 >= out_rows {
            break;
        }
        let mut rows = buf.chunks_mut(n);
        let mut next = |base: isize, row: isize| -> &[i32] {
            let dst = rows.next().expect("ten rows");
            fetch(dst, base, row);
            dst
        };
        let (l0, l1) = (next(lo, r), next(lo, rn_c));
        let (v0, vp, vm) = (next(vo, r), next(vo, rp), next(vo, rn_m));
        let (h0, h1) = (next(ho, r), next(ho, rn_c));
        let (d0, dp, dm) = (next(dof, r), next(dof, rp), next(dof, rn_m));
        for b in 0..n {
            s1[b] = v0[b] + vp[b];
            t1[b] = vp[b] - 6 * v0[b] + vm[b];
            ds[b] = d0[b] + dp[b];
            dt[b] = dp[b] + dm[b];
        }
        for j in 0..nbc {
            // Entries of band columns c, cl and cr.
            let k = j + 1;
            let kl = if j == 0 { k } else { j };
            let kr = j + 2;
            // Band L.
            let (l, lr, ln, lnr) = (l0[k], l0[kr], l1[k], l1[kr]);
            let mut ee = 16 * l;
            let mut eo = 8 * (l + lr);
            let mut oe = 8 * (l + ln);
            let mut oo = 4 * (l + lr + ln + lnr);
            // Band V (vertical high pass).
            let (s1c, s1r, t1c, t1r) = (s1[k], s1[kr], t1[k], t1[kr]);
            ee += 8 * s1c;
            eo += 4 * (s1c + s1r);
            oe += 4 * t1c;
            oo += 2 * (t1c + t1r);
            // Band H (horizontal high pass).
            let s2 = |a: &[i32]| a[k] + a[kl];
            let t2 = |a: &[i32]| a[kl] - 6 * a[k] + a[kr];
            let (s2r, s2n, t2r, t2n) = (s2(h0), s2(h1), t2(h0), t2(h1));
            ee += 8 * s2r;
            eo += 4 * t2r;
            oe += 4 * (s2r + s2n);
            oo += 2 * (t2r + t2n);
            // Band D.
            let (scl, sc, scr) = (ds[kl], ds[k], ds[kr]);
            let (tcl, tc, tcr) = (dt[kl], dt[k], dt[kr]);
            let (ecl, ec, ecr) = (d0[kl], d0[k], d0[kr]);
            ee += 4 * (scl + sc);
            eo += 2 * (scl - 6 * sc + scr);
            oe += 2 * (tcl + tc - 6 * (ecl + ec));
            oo += (tcl - 6 * tc + tcr) - 6 * (ecl - 6 * ec + ecr);
            let px = |s: i32| (((s as i16) >> 6) as i32 + 128).clamp(0, 255) as u8;
            let col = x0 + 2 * j;
            if col + 1 >= out.stride {
                break;
            }
            let e = orow * out.stride + col;
            out.data[e] = px(ee);
            out.data[e + 1] = px(eo);
            let o = e + out.stride;
            out.data[o] = px(oe);
            out.data[o + 1] = px(oo);
        }
        // The last written column takes its left neighbour's value.
        let last = x0 + wpp - 1;
        for rr in [orow, orow + 1] {
            let b = rr * out.stride;
            out.data[b + last] = out.data[b + last - 1];
        }
    }
}

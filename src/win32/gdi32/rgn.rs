//! The region functions: regions as objects the program makes, combines
//! and asks about.

use super::region::{ERROR, Op, RGN_AND, RGN_COPY, RGN_DIFF, RGN_OR, RGN_XOR};
use super::{
    ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, Kind, Rect, Region, TRUE, add, draw,
    gdi, region_of, set_error, set_region,
};
use crate::cpu::Cpu;
use crate::win32::user32::{read_words, write_words};

const ALTERNATE: u32 = 1;
const WINDING: u32 = 2;
const RDH_RECTANGLES: u32 = 1;

/// A new region object holding `region`.
pub fn new_region(cpu: &mut Cpu, region: Region) -> u32 {
    add(cpu, Kind::Region { region })
}

/// The region of the rectangle with corners (`l`, `t`) and (`r`, `b`),
/// in either order.
fn rect_region(l: i32, t: i32, r: i32, b: i32) -> Region {
    Region::from_rect(Rect::new(l, t, r, b))
}

/// CreateRectRgn(left, top, right, bottom).
pub fn create_rect_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = rect_region(args[0] as i32, args[1] as i32, args[2] as i32, args[3] as i32);
    Some(new_region(cpu, r))
}

/// CreateRectRgnIndirect(rect).
pub fn create_rect_rgn_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = Rect::read(cpu, args[0]);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(new_region(cpu, Region::from_rect(r)))
}

/// The region of a rectangle with corners rounded to quarters of an
/// ellipse `ew` by `eh`, as CreateRoundRectRgn makes it.
pub fn round_rect_region(r: Rect, ew: i32, eh: i32) -> Region {
    let r = r.ordered();
    if r.is_empty() {
        return Region::new();
    }
    let ew = ew.abs().min(r.width());
    let eh = eh.abs().min(r.height());
    if ew < 2 || eh < 2 {
        return Region::from_rect(r);
    }
    let (a, b) = (ew as f64 / 2.0, eh as f64 / 2.0);
    let mut spans = Vec::with_capacity(r.height() as usize);
    for y in r.top..r.bottom {
        let cy = if y < r.top + eh / 2 {
            r.top as f64 + b
        } else if y >= r.bottom - eh / 2 {
            r.bottom as f64 - b
        } else {
            spans.push((y, r.left, r.right));
            continue;
        };
        let dy = (y as f64 + 0.5 - cy) / b;
        let dx = a * (1.0 - dy * dy).max(0.0).sqrt();
        let x0 = (r.left as f64 + a - dx).round() as i32;
        let x1 = (r.right as f64 - a + dx).round() as i32;
        spans.push((y, x0, x1));
    }
    Region::from_spans(&spans)
}

/// CreateEllipticRgn(left, top, right, bottom).
pub fn create_elliptic_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = Rect::new(args[0] as i32, args[1] as i32, args[2] as i32, args[3] as i32).ordered();
    let region = round_rect_region(r, r.width(), r.height());
    Some(new_region(cpu, region))
}

/// CreateEllipticRgnIndirect(rect).
pub fn create_elliptic_rgn_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = Rect::read(cpu, args[0]);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let r = r.ordered();
    Some(new_region(cpu, round_rect_region(r, r.width(), r.height())))
}

/// CreateRoundRectRgn(left, top, right, bottom, width, height).
pub fn create_round_rect_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = Rect::new(args[0] as i32, args[1] as i32, args[2] as i32, args[3] as i32);
    let region = round_rect_region(r, args[4] as i32, args[5] as i32);
    Some(new_region(cpu, region))
}

/// The polygons of `counts` points each at `at`.
fn read_figures(cpu: &mut Cpu, at: u32, counts: &[u32]) -> Vec<Vec<(i32, i32)>> {
    let mut figures = Vec::with_capacity(counts.len());
    let mut p = at;
    for &n in counts {
        let words = read_words(cpu, p, 2 * n as usize);
        figures.push(
            words
                .chunks(2)
                .map(|w| (w[0] as i32, w[1] as i32))
                .collect(),
        );
        p += 8 * n;
    }
    figures
}

fn polygon_rgn(cpu: &mut Cpu, at: u32, counts: &[u32], mode: u32) -> Option<u32> {
    if mode != ALTERNATE && mode != WINDING {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    if counts.iter().map(|&c| c as u64).sum::<u64>() > 0x10_0000 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let figures = read_figures(cpu, at, counts);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let region = draw::polygon_region(&figures, mode == WINDING);
    Some(new_region(cpu, region))
}

/// CreatePolygonRgn(points, count, mode).
pub fn create_polygon_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if (args[1] as i32) < 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    polygon_rgn(cpu, args[0], &[args[1]], args[2])
}

/// CreatePolyPolygonRgn(points, counts, polygons, mode).
pub fn create_poly_polygon_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[2] > 0x1_0000 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let counts = read_words(cpu, args[1], args[2] as usize);
    polygon_rgn(cpu, args[0], &counts, args[3])
}

/// ExtCreateRegion(xform, size, data): the region of the RGNDATA's
/// rectangles. Windows 95 has no world transforms and leaves `xform`.
pub fn ext_create_region(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[2];
    if at == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let header = read_words(cpu, at, 4);
    if cpu.bus.guest_faulted() {
        return None;
    }
    // A header that is too short or not of rectangles fails with no
    // error; the size argument and a longer header's size don't matter,
    // and the rectangles follow the header Windows knows.
    if header[0] < 32 || header[1] != RDH_RECTANGLES {
        return Some(0);
    }
    // The XFORM's scale and offset move the rectangles; a rotation or
    // shear isn't kept.
    let xform = (args[0] != 0).then(|| {
        let w = read_words(cpu, args[0], 6);
        w.into_iter().map(|v| f32::from_bits(v) as f64).collect::<Vec<f64>>()
    });
    let count = header[2];
    let mut region = Region::new();
    for i in 0..count {
        let mut r = Rect::read(cpu, at + 32 + 16 * i);
        if let Some(x) = &xform {
            let map = |v: i32, scale: f64, offset: f64| (v as f64 * scale + offset + 0.5).floor() as i32;
            let (l, rt) = (map(r.left, x[0], x[4]), map(r.right, x[0], x[4]));
            let (t, b) = (map(r.top, x[3], x[5]), map(r.bottom, x[3], x[5]));
            r = Rect::new(l.min(rt), t.min(b), l.max(rt), t.max(b));
        }
        if r.left < r.right && r.top < r.bottom {
            region = region.or_rect(&r);
        }
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(new_region(cpu, region))
}

/// CombineRgn(dest, src1, src2, mode): what the result is.
pub fn combine_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (dest, a, b, mode) = (args[0], args[1], args[2], args[3]);
    if gdi(cpu).region(dest).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(ERROR);
    }
    let Some(ra) = region_of(cpu, a) else {
        return Some(ERROR);
    };
    let result = if mode == RGN_COPY {
        ra
    } else {
        let op = match mode {
            RGN_AND => Op::And,
            RGN_OR => Op::Or,
            RGN_XOR => Op::Xor,
            RGN_DIFF => Op::Diff,
            _ => return Some(ERROR),
        };
        let Some(rb) = region_of(cpu, b) else {
            return Some(ERROR);
        };
        ra.combine(&rb, op)
    };
    let kind = result.kind();
    set_region(cpu, dest, result);
    Some(kind)
}

/// EqualRgn(a, b).
pub fn equal_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (Some(a), Some(b)) = (region_of(cpu, args[0]), region_of(cpu, args[1])) else {
        return Some(ERROR);
    };
    Some((a == b) as u32)
}

/// OffsetRgn(hrgn, dx, dy).
pub fn offset_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(r) = gdi(cpu).region_mut(args[0]) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(ERROR);
    };
    r.offset(args[1] as i32, args[2] as i32);
    Some(r.kind())
}

/// GetRgnBox(hrgn, rect).
pub fn get_rgn_box(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(r) = region_of(cpu, args[0]) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(ERROR);
    };
    r.bounds().write(cpu, args[1]);
    Some(r.kind())
}

/// GetRegionData(hrgn, size, data): the bytes of its RGNDATA, which it
/// writes if `size` holds them.
pub fn get_region_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (handle, size, out) = (args[0], args[1], args[2]);
    let Some(r) = region_of(cpu, handle) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let bytes = 16 * r.rects.len() as u32;
    let needed = 32 + bytes;
    if out == 0 {
        return Some(needed);
    }
    if size < needed {
        return Some(0);
    }
    let b = r.bounds();
    write_words(
        cpu,
        out,
        &[
            32,
            RDH_RECTANGLES,
            r.rects.len() as u32,
            bytes,
            b.left as u32,
            b.top as u32,
            b.right as u32,
            b.bottom as u32,
        ],
    );
    for (i, rect) in r.rects.iter().enumerate() {
        rect.write(cpu, out + 32 + 16 * i as u32);
    }
    Some(needed)
}

/// PtInRegion(hrgn, x, y).
pub fn pt_in_region(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(r) = region_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    Some(r.contains(args[1] as i32, args[2] as i32) as u32)
}

/// RectInRegion(hrgn, rect).
pub fn rect_in_region(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(r) = region_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    let rect = Rect::read(cpu, args[1]).ordered();
    Some(r.overlaps(&rect) as u32)
}

/// SetRectRgn(hrgn, left, top, right, bottom).
pub fn set_rect_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = rect_region(args[1] as i32, args[2] as i32, args[3] as i32, args[4] as i32);
    if !set_region(cpu, args[0], r) {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    Some(TRUE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win32::gdi32::region::{COMPLEXREGION, NULLREGION, SIMPLEREGION};

    #[test]
    fn round_rect_region_is_symmetric() {
        let r = round_rect_region(Rect::new(0, 0, 10, 10), 10, 10);
        let b = r.bounds();
        assert_eq!(b, Rect::new(0, 0, 10, 10));
        for y in 0..10 {
            for x in 0..10 {
                assert_eq!(r.contains(x, y), r.contains(9 - x, y), "({x}, {y})");
                assert_eq!(r.contains(x, y), r.contains(x, 9 - y), "({x}, {y})");
            }
        }
        assert!(!r.contains(0, 0));
        assert!(r.contains(5, 5));
    }

    #[test]
    fn small_corners_make_a_rectangle() {
        let r = round_rect_region(Rect::new(0, 0, 10, 10), 1, 1);
        assert_eq!(r.kind(), SIMPLEREGION);
        assert_eq!(Region::new().kind(), NULLREGION);
        assert_ne!(COMPLEXREGION, SIMPLEREGION);
    }
}

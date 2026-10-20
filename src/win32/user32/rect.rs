//! The rectangle functions.

use super::{FALSE, TRUE};
use crate::cpu::Cpu;
use crate::win32::gdi32::Rect;

/// The RECT at `at`, or None for a null pointer.
fn read(cpu: &mut Cpu, at: u32) -> Option<Rect> {
    (at != 0).then(|| Rect::read(cpu, at))
}

/// Whether `r` is empty, as IsRectEmpty has it: a rectangle that runs
/// backwards is.
fn empty(r: &Rect) -> bool {
    r.right <= r.left || r.bottom <= r.top
}

/// SetRect(rect, left, top, right, bottom).
pub fn set_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(FALSE);
    }
    let r = Rect {
        left: args[1] as i32,
        top: args[2] as i32,
        right: args[3] as i32,
        bottom: args[4] as i32,
    };
    r.write(cpu, args[0]);
    Some(TRUE)
}

pub fn set_rect_empty(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(FALSE);
    }
    Rect::default().write(cpu, args[0]);
    Some(TRUE)
}

/// CopyRect(dest, src).
pub fn copy_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(FALSE);
    }
    let Some(r) = read(cpu, args[1]) else {
        return Some(FALSE);
    };
    r.write(cpu, args[0]);
    Some(TRUE)
}

pub fn is_rect_empty(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(read(cpu, args[0]).is_none_or(|r| empty(&r)) as u32)
}

pub fn equal_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (Some(a), Some(b)) = (read(cpu, args[0]), read(cpu, args[1])) else {
        return Some(FALSE);
    };
    Some((a == b) as u32)
}

/// The intersection of `a` and `b`, or None.
fn intersect(a: &Rect, b: &Rect) -> Option<Rect> {
    if empty(a) || empty(b) {
        return None;
    }
    let r = Rect {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (!empty(&r)).then_some(r)
}

/// IntersectRect(dest, a, b).
pub fn intersect_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (Some(a), Some(b)) = (read(cpu, args[1]), read(cpu, args[2])) else {
        return Some(FALSE);
    };
    if args[0] == 0 {
        return Some(FALSE);
    }
    let r = intersect(&a, &b);
    r.unwrap_or_default().write(cpu, args[0]);
    Some(r.is_some() as u32)
}

/// UnionRect(dest, a, b).
pub fn union_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (Some(a), Some(b)) = (read(cpu, args[1]), read(cpu, args[2])) else {
        return Some(FALSE);
    };
    if args[0] == 0 {
        return Some(FALSE);
    }
    let r = match (empty(&a), empty(&b)) {
        (true, true) => {
            Rect::default().write(cpu, args[0]);
            return Some(FALSE);
        }
        (true, false) => b,
        (false, true) => a,
        (false, false) => Rect {
            left: a.left.min(b.left),
            top: a.top.min(b.top),
            right: a.right.max(b.right),
            bottom: a.bottom.max(b.bottom),
        },
    };
    r.write(cpu, args[0]);
    Some(TRUE)
}

/// SubtractRect(dest, a, b): `a` less `b` where that leaves a
/// rectangle, else `a`.
pub fn subtract_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (Some(a), Some(b)) = (read(cpu, args[1]), read(cpu, args[2])) else {
        return Some(FALSE);
    };
    if args[0] == 0 {
        return Some(FALSE);
    }
    if empty(&a) {
        Rect::default().write(cpu, args[0]);
        return Some(FALSE);
    }
    let mut d = a;
    if let Some(t) = intersect(&a, &b) {
        if t == d {
            Rect::default().write(cpu, args[0]);
            return Some(FALSE);
        }
        if t.top == d.top && t.bottom == d.bottom {
            if t.left == d.left {
                d.left = t.right;
            } else if t.right == d.right {
                d.right = t.left;
            }
        } else if t.left == d.left && t.right == d.right {
            if t.top == d.top {
                d.top = t.bottom;
            } else if t.bottom == d.bottom {
                d.bottom = t.top;
            }
        }
    }
    d.write(cpu, args[0]);
    Some(TRUE)
}

/// OffsetRect(rect, dx, dy).
pub fn offset_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(r) = read(cpu, args[0]) else {
        return Some(FALSE);
    };
    let (dx, dy) = (args[1] as i32, args[2] as i32);
    let r = Rect {
        left: r.left.wrapping_add(dx),
        top: r.top.wrapping_add(dy),
        right: r.right.wrapping_add(dx),
        bottom: r.bottom.wrapping_add(dy),
    };
    r.write(cpu, args[0]);
    Some(TRUE)
}

/// InflateRect(rect, dx, dy).
pub fn inflate_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(r) = read(cpu, args[0]) else {
        return Some(FALSE);
    };
    let (dx, dy) = (args[1] as i32, args[2] as i32);
    let r = Rect {
        left: r.left.wrapping_sub(dx),
        top: r.top.wrapping_sub(dy),
        right: r.right.wrapping_add(dx),
        bottom: r.bottom.wrapping_add(dy),
    };
    r.write(cpu, args[0]);
    Some(TRUE)
}

/// PtInRect(rect, x, y): the point is passed by value.
pub fn pt_in_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(r) = read(cpu, args[0]) else {
        return Some(FALSE);
    };
    let (x, y) = (args[1] as i32, args[2] as i32);
    Some((x >= r.left && x < r.right && y >= r.top && y < r.bottom) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_of_disjoint_rectangles_is_none() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(10, 0, 20, 10);
        assert_eq!(intersect(&a, &b), None);
        assert_eq!(intersect(&a, &Rect::new(5, 5, 15, 15)), Some(Rect::new(5, 5, 10, 10)));
    }
}

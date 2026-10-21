//! Rectangles and regions. A region is a list of rectangles that don't
//! overlap, in bands: each band a run of rows whose rectangles are the
//! same, sorted by top and then by left, as Windows keeps them
//! (GetRegionData gives them in that order). The operations work on the
//! grid the rectangles' edges make, which is plenty fast for the regions
//! windows and clipping make.

use crate::cpu::Cpu;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    /// The rectangle `w` wide and `h` high at (`x`, `y`).
    pub const fn sized(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect::new(x, y, x + w, y + h)
    }

    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    /// Whether it has no area, as IsRectEmpty says.
    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub fn intersect(&self, other: &Rect) -> Rect {
        let r = Rect::new(
            self.left.max(other.left),
            self.top.max(other.top),
            self.right.min(other.right),
            self.bottom.min(other.bottom),
        );
        if r.is_empty() { Rect::default() } else { r }
    }

    /// The smallest rectangle around both; an empty one doesn't count.
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        Rect::new(
            self.left.min(other.left),
            self.top.min(other.top),
            self.right.max(other.right),
            self.bottom.max(other.bottom),
        )
    }

    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(
            self.left + dx,
            self.top + dy,
            self.right + dx,
            self.bottom + dy,
        )
    }

    pub fn inflate(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(
            self.left - dx,
            self.top - dy,
            self.right + dx,
            self.bottom + dy,
        )
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    /// With left above right and top above bottom.
    pub fn ordered(&self) -> Rect {
        Rect::new(
            self.left.min(self.right),
            self.top.min(self.bottom),
            self.left.max(self.right),
            self.top.max(self.bottom),
        )
    }

    /// The RECT at `at` in the program's memory.
    pub fn read(cpu: &mut Cpu, at: u32) -> Rect {
        let mut b = [0u8; 16];
        cpu.bus.guest_read_bytes(at, &mut b);
        let v = |i: usize| i32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        Rect::new(v(0), v(1), v(2), v(3))
    }

    pub fn write(&self, cpu: &mut Cpu, at: u32) {
        cpu.bus.guest_write_bytes(at, &self.bytes());
    }

    pub fn bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        for (i, v) in [self.left, self.top, self.right, self.bottom]
            .into_iter()
            .enumerate()
        {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        b
    }
}

/// How two regions combine (CombineRgn's modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    And,
    Or,
    Xor,
    Diff,
}

pub const RGN_AND: u32 = 1;
pub const RGN_OR: u32 = 2;
pub const RGN_XOR: u32 = 3;
pub const RGN_DIFF: u32 = 4;
pub const RGN_COPY: u32 = 5;

/// What a region is, as the region functions return it.
pub const ERROR: u32 = 0;
pub const NULLREGION: u32 = 1;
pub const SIMPLEREGION: u32 = 2;
pub const COMPLEXREGION: u32 = 3;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Region {
    pub rects: Vec<Rect>,
}

impl Region {
    pub fn new() -> Region {
        Region::default()
    }

    pub fn from_rect(r: Rect) -> Region {
        let r = r.ordered();
        if r.is_empty() {
            Region::new()
        } else {
            Region { rects: vec![r] }
        }
    }

    /// The region of runs of pixels (row, first, after last), sorted by
    /// row and then by first, which don't overlap within a row.
    pub fn from_spans(spans: &[(i32, i32, i32)]) -> Region {
        let mut rects: Vec<Rect> = Vec::with_capacity(spans.len());
        for &(y, x0, x1) in spans {
            if x1 <= x0 {
                continue;
            }
            match rects.last_mut() {
                Some(l) if l.top == y && x0 <= l.right => l.right = l.right.max(x1),
                _ => rects.push(Rect::new(x0, y, x1, y + 1)),
            }
        }
        let mut region = Region { rects };
        region.merge_bands();
        region
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// NULLREGION, SIMPLEREGION or COMPLEXREGION.
    pub fn kind(&self) -> u32 {
        match self.rects.len() {
            0 => NULLREGION,
            1 => SIMPLEREGION,
            _ => COMPLEXREGION,
        }
    }

    pub fn bounds(&self) -> Rect {
        self.rects
            .iter()
            .fold(Rect::default(), |acc, r| acc.union(r))
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.rects.iter().any(|r| r.contains(x, y))
    }

    /// Whether any part of `r` is in it.
    pub fn overlaps(&self, r: &Rect) -> bool {
        self.rects.iter().any(|x| !x.intersect(r).is_empty())
    }

    pub fn offset(&mut self, dx: i32, dy: i32) {
        for r in &mut self.rects {
            *r = r.offset(dx, dy);
        }
    }

    pub fn offset_by(&self, dx: i32, dy: i32) -> Region {
        let mut r = self.clone();
        r.offset(dx, dy);
        r
    }

    pub fn combine(&self, other: &Region, op: Op) -> Region {
        // Fast paths for what windows and clipping do most.
        match op {
            Op::And if self.is_empty() || other.is_empty() => return Region::new(),
            Op::And if self.rects.len() == 1 && other.rects.len() == 1 => {
                return Region::from_rect(self.rects[0].intersect(&other.rects[0]));
            }
            Op::Or | Op::Xor if other.is_empty() => return self.clone(),
            Op::Or | Op::Xor if self.is_empty() => return other.clone(),
            Op::Diff if self.is_empty() => return Region::new(),
            Op::Diff if other.is_empty() || !other.overlaps(&self.bounds()) => {
                return self.clone();
            }
            _ => {}
        }
        let mut xs: Vec<i32> = Vec::new();
        let mut ys: Vec<i32> = Vec::new();
        for r in self.rects.iter().chain(&other.rects) {
            xs.extend([r.left, r.right]);
            ys.extend([r.top, r.bottom]);
        }
        xs.sort_unstable();
        xs.dedup();
        ys.sort_unstable();
        ys.dedup();
        let mut rects = Vec::new();
        for band in ys.windows(2) {
            let (top, bottom) = (band[0], band[1]);
            let mut run: Option<i32> = None;
            for cell in xs.windows(2) {
                let (left, right) = (cell[0], cell[1]);
                let a = self.contains(left, top);
                let b = other.contains(left, top);
                let inside = match op {
                    Op::And => a && b,
                    Op::Or => a || b,
                    Op::Xor => a != b,
                    Op::Diff => a && !b,
                };
                match (inside, run) {
                    (true, None) => run = Some(left),
                    (false, Some(start)) => {
                        rects.push(Rect::new(start, top, left, bottom));
                        run = None;
                    }
                    _ => {}
                }
                if inside && cell[1] == *xs.last().unwrap() {
                    rects.push(Rect::new(run.take().unwrap(), top, right, bottom));
                }
            }
        }
        let mut region = Region { rects };
        region.merge_bands();
        region
    }

    /// Join bands that touch and have the same rectangles across.
    fn merge_bands(&mut self) {
        let mut out: Vec<Rect> = Vec::with_capacity(self.rects.len());
        let mut prev: (usize, usize) = (0, 0);
        let mut i = 0;
        while i < self.rects.len() {
            let top = self.rects[i].top;
            let mut j = i;
            while j < self.rects.len() && self.rects[j].top == top {
                j += 1;
            }
            let band = &self.rects[i..j];
            let (ps, pe) = prev;
            let joins = pe > ps
                && out[ps].bottom == top
                && pe - ps == band.len()
                && out[ps..pe]
                    .iter()
                    .zip(band)
                    .all(|(a, b)| a.left == b.left && a.right == b.right);
            if joins {
                let bottom = band[0].bottom;
                for r in &mut out[ps..pe] {
                    r.bottom = bottom;
                }
            } else {
                let start = out.len();
                out.extend_from_slice(band);
                prev = (start, out.len());
            }
            i = j;
        }
        self.rects = out;
    }

    pub fn and(&self, other: &Region) -> Region {
        self.combine(other, Op::And)
    }

    pub fn or(&self, other: &Region) -> Region {
        self.combine(other, Op::Or)
    }

    pub fn diff(&self, other: &Region) -> Region {
        self.combine(other, Op::Diff)
    }

    pub fn and_rect(&self, r: &Rect) -> Region {
        self.and(&Region::from_rect(*r))
    }

    pub fn or_rect(&self, r: &Rect) -> Region {
        self.or(&Region::from_rect(*r))
    }

    pub fn diff_rect(&self, r: &Rect) -> Region {
        self.diff(&Region::from_rect(*r))
    }
}

crate::state_fields!(Rect {
    left,
    top,
    right,
    bottom
});
crate::state_fields!(Region { rects });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rectangle_minus_its_middle_is_four_bands_of_rectangles() {
        let outer = Region::from_rect(Rect::new(0, 0, 30, 30));
        let hole = outer.diff_rect(&Rect::new(10, 10, 20, 20));
        assert_eq!(
            hole.rects,
            vec![
                Rect::new(0, 0, 30, 10),
                Rect::new(0, 10, 10, 20),
                Rect::new(20, 10, 30, 20),
                Rect::new(0, 20, 30, 30),
            ]
        );
        assert_eq!(hole.kind(), COMPLEXREGION);
        assert_eq!(hole.or_rect(&Rect::new(10, 10, 20, 20)), outer);
    }

    #[test]
    fn bands_with_the_same_rectangles_join() {
        let a = Region::from_rect(Rect::new(0, 0, 10, 10)).or_rect(&Rect::new(0, 10, 10, 20));
        assert_eq!(a.rects, vec![Rect::new(0, 0, 10, 20)]);
        let x = a.combine(&Region::from_rect(Rect::new(5, 0, 15, 20)), Op::Xor);
        assert_eq!(
            x.rects,
            vec![Rect::new(0, 0, 5, 20), Rect::new(10, 0, 15, 20)]
        );
    }
}

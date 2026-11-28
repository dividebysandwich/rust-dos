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

    /// The pixels it covers: none when it is empty.
    pub fn pixels(&self) -> u64 {
        if self.is_empty() {
            0
        } else {
            self.width() as u64 * self.height() as u64
        }
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
        // A sweep down the rows where either region's rectangles start or
        // end. Between two such rows each region is the same spans on
        // every row, those of the rectangles that cross the band, and the
        // result's spans there come from walking both lists once. Its
        // cost grows with the rectangles each band crosses; testing every
        // cell of the grid the edges make against every rectangle grows
        // with their product, and child windows with round or polygon
        // regions, a rectangle a row each, made that tens of milliseconds
        // a BeginPaint.
        let mut ys: Vec<i32> = Vec::with_capacity(2 * (self.rects.len() + other.rects.len()));
        for r in self.rects.iter().chain(&other.rects) {
            ys.extend([r.top, r.bottom]);
        }
        ys.sort_unstable();
        ys.dedup();
        let a = by_top(&self.rects);
        let b = by_top(&other.rects);
        let (mut next_a, mut next_b) = (0, 0);
        let (mut live_a, mut live_b) = (Vec::new(), Vec::new());
        let (mut spans_a, mut spans_b, mut spans) = (Vec::new(), Vec::new(), Vec::new());
        let mut rects = Vec::new();
        for band in ys.windows(2) {
            let (top, bottom) = (band[0], band[1]);
            enter(&a, &mut next_a, &mut live_a, top);
            enter(&b, &mut next_b, &mut live_b, top);
            spans_of(&live_a, &mut spans_a);
            spans_of(&live_b, &mut spans_b);
            combine_spans(&spans_a, &spans_b, op, &mut spans);
            rects.extend(
                spans
                    .iter()
                    .map(|&(left, right)| Rect::new(left, top, right, bottom)),
            );
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

/// The rectangles sorted by top. A region's are already, apart from one
/// a program gave as it liked.
fn by_top(rects: &[Rect]) -> std::borrow::Cow<'_, [Rect]> {
    if rects.is_sorted_by_key(|r| r.top) {
        std::borrow::Cow::Borrowed(rects)
    } else {
        let mut v = rects.to_vec();
        v.sort_by_key(|r| r.top);
        std::borrow::Cow::Owned(v)
    }
}

/// The band starting at row `top`: the rectangles of `rects` (sorted by
/// top) from `next` on that start by then join `live`, and those that
/// end by then leave it. The rows of a band are those between two edges,
/// so each rectangle left crosses the whole band.
fn enter(rects: &[Rect], next: &mut usize, live: &mut Vec<Rect>, top: i32) {
    while *next < rects.len() && rects[*next].top <= top {
        live.push(rects[*next]);
        *next += 1;
    }
    live.retain(|r| r.bottom > top);
}

/// The spans the rectangles cover on a row, left to right, with those
/// that overlap or touch joined.
fn spans_of(live: &[Rect], spans: &mut Vec<(i32, i32)>) {
    spans.clear();
    spans.extend(
        live.iter()
            .filter(|r| r.right > r.left)
            .map(|r| (r.left, r.right)),
    );
    spans.sort_unstable();
    let mut n = 0;
    for i in 0..spans.len() {
        let s = spans[i];
        if n > 0 && s.0 <= spans[n - 1].1 {
            spans[n - 1].1 = spans[n - 1].1.max(s.1);
        } else {
            spans[n] = s;
            n += 1;
        }
    }
    spans.truncate(n);
}

/// The spans of a row where `op` of the two rows' spans holds, left to
/// right, as long as they run.
fn combine_spans(a: &[(i32, i32)], b: &[(i32, i32)], op: Op, out: &mut Vec<(i32, i32)>) {
    out.clear();
    let (mut i, mut j) = (0, 0);
    let mut x = i32::MIN;
    loop {
        while i < a.len() && a[i].1 <= x {
            i += 1;
        }
        while j < b.len() && b[j].1 <= x {
            j += 1;
        }
        if i == a.len() && j == b.len() {
            break;
        }
        let in_a = i < a.len() && a[i].0 <= x;
        let in_b = j < b.len() && b[j].0 <= x;
        // Where the next span starts or the one x is in ends.
        let edge = |s: &[(i32, i32)], k: usize, inside: bool| match s.get(k) {
            Some(&(l, r)) => {
                if inside {
                    r
                } else {
                    l
                }
            }
            None => i32::MAX,
        };
        let to = edge(a, i, in_a).min(edge(b, j, in_b));
        let inside = match op {
            Op::And => in_a && in_b,
            Op::Or => in_a || in_b,
            Op::Xor => in_a != in_b,
            Op::Diff => in_a && !in_b,
        };
        if inside {
            match out.last_mut() {
                Some(last) if last.1 == x => last.1 = to,
                _ => out.push((x, to)),
            }
        }
        x = to;
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

    /// What `combine` gives, from a test of each cell of the grid the
    /// rectangles' edges make, joined into runs and bands the same way.
    fn by_cells(a: &Region, b: &Region, op: Op) -> Region {
        let mut xs: Vec<i32> = Vec::new();
        let mut ys: Vec<i32> = Vec::new();
        for r in a.rects.iter().chain(&b.rects) {
            xs.extend([r.left, r.right]);
            ys.extend([r.top, r.bottom]);
        }
        xs.sort_unstable();
        xs.dedup();
        ys.sort_unstable();
        ys.dedup();
        let mut spans = Vec::new();
        for band in ys.windows(2) {
            for cell in xs.windows(2) {
                let (x, y) = (cell[0], band[0]);
                let (p, q) = (a.contains(x, y), b.contains(x, y));
                let inside = match op {
                    Op::And => p && q,
                    Op::Or => p || q,
                    Op::Xor => p != q,
                    Op::Diff => p && !q,
                };
                if inside {
                    for row in band[0]..band[1] {
                        spans.push((row, cell[0], cell[1]));
                    }
                }
            }
        }
        spans.sort_unstable();
        Region::from_spans(&spans)
    }

    /// A region made of `n` random rectangles in a 40x40 square, joined
    /// the way programs build them.
    fn random_region(seed: &mut u32, n: usize) -> Region {
        let mut next = |m: i32| {
            *seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (*seed >> 16) as i32 % m
        };
        let mut r = Region::new();
        for _ in 0..n {
            let (x, y) = (next(40), next(40));
            let rect = Rect::new(x, y, x + next(20), y + next(20));
            r = match next(3) {
                0 => r.diff_rect(&rect),
                _ => r.or_rect(&rect),
            };
        }
        r
    }

    #[test]
    fn the_sweep_gives_what_the_grid_of_cells_gives() {
        let mut seed = 7;
        for _ in 0..400 {
            let n = (seed >> 20) as usize % 8;
            let a = random_region(&mut seed, n);
            let b = random_region(&mut seed, 3);
            for op in [Op::And, Op::Or, Op::Xor, Op::Diff] {
                assert_eq!(a.combine(&b, op), by_cells(&a, &b, op), "{a:?} {op:?} {b:?}");
            }
        }
    }

    #[test]
    fn rectangles_out_of_order_or_overlapping_still_combine() {
        // ExtCreateRegion takes rectangles as the program lists them.
        let given = Region {
            rects: vec![
                Rect::new(0, 10, 10, 20),
                Rect::new(5, 0, 15, 12),
                Rect::new(0, 0, 0, 5),
            ],
        };
        let got = given.or_rect(&Rect::new(20, 0, 30, 5));
        assert_eq!(got, by_cells(&given, &Region::from_rect(Rect::new(20, 0, 30, 5)), Op::Or));
        assert_eq!(
            got.rects,
            vec![
                Rect::new(5, 0, 15, 5),
                Rect::new(20, 0, 30, 5),
                Rect::new(5, 5, 15, 10),
                Rect::new(0, 10, 15, 12),
                Rect::new(0, 12, 10, 20),
            ]
        );
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

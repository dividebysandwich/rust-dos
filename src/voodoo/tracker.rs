//! Which bytes of a 3dfx card's memory the OpenGL renderer has newer than
//! the card, and the other way round, for every card. The card's memory
//! is what the program, the 2D engine, GDI, the software picture and a
//! save state see; the renderer keeps its own copy of what it draws.
//! Every byte is in one of these states:
//!
//! - the same in both;
//! - newer in the card's memory, written since the renderer last had it
//!   (dirty). A Banshee's or Voodoo3's renderer keeps all of the card's
//!   memory as one texture (`gl::CardMemory`), a page of 4 KB a row, and
//!   gets the dirty pages again before it reads them, so that moving the
//!   picture to memory the renderer has (a flip) costs nothing;
//! - newer on the GPU, in the renderer's copy of the memory, where a
//!   Banshee's or Voodoo3's 2D operations drew (`gpu`), page by page;
//! - newer on the GPU, in a 3D buffer the renderer draws at its own
//!   resolution: rows of any card's colour and auxiliary buffers, which
//!   jobs left to the renderer drew (`voodoo_readback=gpu`). These are
//!   read back by rows of the buffer, at the card's resolution, rather
//!   than by pages.
//!
//! Whatever reads or writes the card's memory where it is behind the GPU
//! reads those bytes back first. A byte is never both dirty and newer in
//! the renderer's copy of the memory: the GPU only draws into memory it
//! has. The 3D buffers are apart from that copy, and a Voodoo Graphics'
//! or Voodoo2's renderer keeps no copy, so its pages stay dirty.
//!
//! Memory read back for the program is hot for a while (`cool`): the
//! program reads or writes it, and a 2D operation there is cheaper on the
//! CPU than drawn on the GPU and read back again.

/// Bytes a page, a row of the renderer's texture of the memory.
pub const PAGE: u32 = 4096;
const SHIFT: u32 = PAGE.trailing_zeros();

/// A set of byte ranges, in order, apart from one another: ranges that
/// meet or overlap become one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ranges(Vec<(u32, u32)>);

impl Ranges {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// The ranges, first to last.
    pub fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.0.iter().copied()
    }

    /// Add bytes `start..end`, joining the ranges they meet.
    pub fn add(&mut self, start: u32, end: u32) {
        if start >= end {
            return;
        }
        let first = self.0.partition_point(|r| r.1 < start);
        let last = self.0.partition_point(|r| r.0 <= end);
        let joined = if first < last {
            (self.0[first].0.min(start), self.0[last - 1].1.max(end))
        } else {
            (start, end)
        };
        self.0.splice(first..last, [joined]);
    }

    /// Whether any of bytes `start..end` is in the set.
    pub fn any_in(&self, start: u32, end: u32) -> bool {
        let i = self.0.partition_point(|r| r.1 <= start);
        start < end && self.0.get(i).is_some_and(|r| r.0 < end)
    }

    /// Take bytes `start..end` out of the set: the parts of the ranges
    /// within them, first to last.
    pub fn take(&mut self, start: u32, end: u32) -> Vec<(u32, u32)> {
        if start >= end {
            return Vec::new();
        }
        let first = self.0.partition_point(|r| r.1 <= start);
        let last = self.0.partition_point(|r| r.0 < end);
        if first >= last {
            return Vec::new();
        }
        let taken = self.0[first..last]
            .iter()
            .map(|&(a, b)| (a.max(start), b.min(end)))
            .collect();
        let (head, tail) = (self.0[first].0, self.0[last - 1].1);
        let left = [(head, start), (end, tail)];
        self.0
            .splice(first..last, left.into_iter().filter(|(a, b)| a < b));
        taken
    }
}

#[derive(Clone, Debug, Default)]
pub struct Tracker {
    /// The bytes tracked, in whole pages.
    size: u32,
    /// Pages newer in the card's memory, a bit each.
    dirty: Vec<u64>,
    /// Whole pages newer in the renderer's copy of the memory.
    gpu: Ranges,
    /// Bytes newer in the renderer's 3D buffers.
    buffers: Ranges,
    /// Bytes of the 3D buffers read back since a Banshee's texture units
    /// last looked (`take_read_back`).
    read: Ranges,
    /// Bytes read back for the program since the last `cool`.
    hot: Ranges,
}

/// Add page `page` to `runs`, byte ranges of whole pages in order.
fn add_run(runs: &mut Vec<(u32, u32)>, page: usize) {
    let (a, b) = (page as u32 * PAGE, (page as u32 + 1) * PAGE);
    match runs.last_mut() {
        Some(last) if last.1 == a => last.1 = b,
        _ => runs.push((a, b)),
    }
}

fn get(bits: &[u64], page: usize) -> bool {
    bits.get(page / 64)
        .is_some_and(|w| w >> (page % 64) & 1 != 0)
}

fn set(bits: &mut [u64], page: usize, on: bool) {
    if let Some(w) = bits.get_mut(page / 64) {
        if on {
            *w |= 1 << (page % 64);
        } else {
            *w &= !(1 << (page % 64));
        }
    }
}

impl Tracker {
    /// The pages of `bytes` of memory, all dirty: the renderer has none of
    /// them yet.
    pub fn new(bytes: usize) -> Self {
        let pages = bytes.div_ceil(PAGE as usize);
        let mut p = Self {
            size: (pages as u64 * PAGE as u64).min(u32::MAX as u64) as u32,
            dirty: vec![0; pages.div_ceil(64)],
            ..Self::default()
        };
        p.mark_all();
        p
    }

    /// The pages bytes `start..end` touch, as page numbers.
    fn span(&self, start: u32, end: u32) -> std::ops::Range<usize> {
        if start >= end {
            return 0..0;
        }
        let last = ((end as u64 - 1) >> SHIFT) as usize + 1;
        (start >> SHIFT) as usize..last.min(self.dirty.len() * 64)
    }

    /// The whole pages bytes `start..end` touch, as bytes, within the
    /// memory: none if there are no bytes.
    fn pages(&self, start: u32, end: u32) -> (u32, u32) {
        if start >= end {
            return (0, 0);
        }
        let end = (end as u64).div_ceil(PAGE as u64) * PAGE as u64;
        (start & !(PAGE - 1), end.min(self.size as u64) as u32)
    }

    /// Bytes `start..end` of the card's memory were written: the renderer
    /// gets them again. The caller read back the pages there that were
    /// newer on the GPU before it wrote (`take_gpu`).
    pub fn mark(&mut self, start: u32, end: u32) {
        debug_assert!(
            !self.gpu_in(start, end),
            "{start:#x}..{end:#x} is the GPU's"
        );
        for page in self.span(start, end) {
            set(&mut self.dirty, page, true);
        }
    }

    /// The renderer starts over: it has none of the memory. What only its
    /// copy of the memory had is gone with it, so the caller reads that
    /// back first.
    pub fn mark_all(&mut self) {
        self.dirty.fill(u64::MAX);
        self.gpu.clear();
    }

    /// The renderer lost pages sent to it, which the recording dropped:
    /// every page goes again but those newer on the GPU, which it still
    /// has.
    pub fn resend(&mut self) {
        self.dirty.fill(u64::MAX);
        for (a, b) in self.gpu.iter() {
            for page in (a >> SHIFT) as usize..(b >> SHIFT) as usize {
                set(&mut self.dirty, page, false);
            }
        }
    }

    /// The runs of dirty pages within bytes `start..end`, as byte ranges
    /// of whole pages, which are clean from now on: the caller sends them
    /// to the renderer.
    pub fn take_dirty(&mut self, start: u32, end: u32) -> Vec<(u32, u32)> {
        let mut runs = Vec::new();
        for page in self.span(start, end) {
            if get(&self.dirty, page) {
                set(&mut self.dirty, page, false);
                add_run(&mut runs, page);
            }
        }
        runs
    }

    /// Whether any page is newer in the renderer's copy of the memory.
    #[inline]
    pub fn any_gpu(&self) -> bool {
        !self.gpu.is_empty()
    }

    /// The GPU drew into bytes `start..end` of its copy of the memory,
    /// whose pages it has (the caller sent the dirty ones first): the
    /// card's memory there is out of date until they are read back.
    pub fn drawn_on_gpu(&mut self, start: u32, end: u32) {
        let (start, end) = self.pages(start, end);
        debug_assert!(
            self.span(start, end).all(|p| !get(&self.dirty, p)),
            "{start:#x}..{end:#x} is dirty"
        );
        self.gpu.add(start, end);
    }

    /// Whether any of bytes `start..end` is on a page newer in the
    /// renderer's copy of the memory.
    #[inline]
    pub fn gpu_in(&self, start: u32, end: u32) -> bool {
        !self.gpu.is_empty() && self.gpu.any_in(start, end)
    }

    /// The runs of pages within bytes `start..end` that are newer in the
    /// renderer's copy of the memory, as byte ranges; the same in both
    /// from now on, as the caller reads them back, and hot if `hot` (the
    /// program reads or writes them).
    pub fn take_gpu(&mut self, start: u32, end: u32, hot: bool) -> Vec<(u32, u32)> {
        if self.gpu.is_empty() {
            return Vec::new();
        }
        let (start, end) = self.pages(start, end);
        let runs = self.gpu.take(start, end);
        if hot {
            for &(a, b) in &runs {
                self.hot.add(a, b);
            }
        }
        runs
    }

    /// The renderer drew bytes `start..end` into a 3D buffer of its own,
    /// which the card's memory doesn't have until they are read back by
    /// rows of the buffer.
    pub fn drawn_in_buffer(&mut self, start: u32, end: u32) {
        self.buffers.add(start, end.min(self.size));
    }

    /// Whether any byte is newer in the renderer's 3D buffers.
    #[inline]
    pub fn any_in_buffers(&self) -> bool {
        !self.buffers.is_empty()
    }

    /// Whether any of bytes `start..end` is newer in the renderer's 3D
    /// buffers.
    #[inline]
    pub fn buffers_in(&self, start: u32, end: u32) -> bool {
        !self.buffers.is_empty() && self.buffers.any_in(start, end)
    }

    /// The bytes within `start..end` newer in the renderer's 3D buffers,
    /// first to last, which the caller reads back: hot from now on if
    /// `hot` (the program reads or writes them).
    pub fn take_buffers(&mut self, start: u32, end: u32, hot: bool) -> Vec<(u32, u32)> {
        let taken = self.buffers.take(start, end);
        for &(a, b) in &taken {
            self.read.add(a, b);
            if hot {
                self.hot.add(a, b);
            }
        }
        taken
    }

    /// Whether any of bytes `start..end` was read back from the 3D
    /// buffers since the last call for them: the memory there changed
    /// under the textures decoded from it.
    pub fn take_read_back(&mut self, start: u32, end: u32) -> bool {
        !self.read.is_empty() && !self.read.take(start, end).is_empty()
    }

    /// The bytes newer in the renderer's 3D buffers are lost, or can't be
    /// read back any more.
    pub fn forget_buffers(&mut self) {
        self.buffers.clear();
    }

    /// Whether any of bytes `start..end` was read back since the last
    /// `cool`.
    pub fn hot_in(&self, start: u32, end: u32) -> bool {
        self.hot.any_in(start, end)
    }

    /// Nothing is hot any more: the program may have stopped reading it.
    pub fn cool(&mut self) {
        self.hot.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_join_where_they_meet_or_overlap() {
        let mut r = Ranges::default();
        r.add(10, 20);
        r.add(30, 40);
        r.add(5, 5);
        assert_eq!(r.iter().collect::<Vec<_>>(), [(10, 20), (30, 40)]);
        // Meeting the first, overlapping nothing.
        r.add(20, 25);
        assert_eq!(r.iter().collect::<Vec<_>>(), [(10, 25), (30, 40)]);
        // Across both, and past the last.
        r.add(12, 45);
        assert_eq!(r.iter().collect::<Vec<_>>(), [(10, 45)]);
        r.add(0, 2);
        r.add(60, 70);
        assert_eq!(r.iter().collect::<Vec<_>>(), [(0, 2), (10, 45), (60, 70)]);
        assert!(r.any_in(44, 50) && r.any_in(1, 2) && !r.any_in(2, 10));
        assert!(!r.any_in(45, 60) && !r.any_in(70, 100) && !r.any_in(15, 15));
    }

    #[test]
    fn ranges_taken_leave_what_is_outside_them() {
        let mut r = Ranges::default();
        r.add(0, 100);
        r.add(200, 300);
        r.add(400, 500);
        assert_eq!(r.take(50, 250), [(50, 100), (200, 250)]);
        assert_eq!(
            r.iter().collect::<Vec<_>>(),
            [(0, 50), (250, 300), (400, 500)]
        );
        // From inside one range: it splits in two.
        assert_eq!(r.take(420, 430), [(420, 430)]);
        assert_eq!(
            r.iter().collect::<Vec<_>>(),
            [(0, 50), (250, 300), (400, 420), (430, 500)]
        );
        assert!(r.take(300, 400).is_empty() && r.take(10, 10).is_empty());
        assert_eq!(r.take(0, 1000).len(), 4);
        assert!(r.is_empty());
    }

    #[test]
    fn dirty_pages_come_out_once_in_runs() {
        let mut p = Tracker::new(16 * PAGE as usize);
        assert_eq!(p.take_dirty(0, 16 * PAGE), [(0, 16 * PAGE)]);
        assert!(p.take_dirty(0, 16 * PAGE).is_empty());
        p.mark(PAGE + 10, PAGE + 11);
        p.mark(2 * PAGE, 2 * PAGE + 1);
        p.mark(5 * PAGE - 1, 5 * PAGE + 1);
        // Only the pages asked for come out; the others stay dirty.
        assert_eq!(p.take_dirty(0, 4 * PAGE), [(PAGE, 3 * PAGE)]);
        assert_eq!(p.take_dirty(0, 16 * PAGE), [(4 * PAGE, 6 * PAGE)]);
    }

    #[test]
    fn pages_drawn_on_the_gpu_are_read_back_once() {
        let mut p = Tracker::new(8 * PAGE as usize);
        p.take_dirty(0, 8 * PAGE);
        assert!(!p.any_gpu());
        p.drawn_on_gpu(PAGE, 3 * PAGE);
        assert!(p.any_gpu() && p.gpu_in(2 * PAGE + 5, 2 * PAGE + 6));
        assert!(!p.gpu_in(0, PAGE) && !p.gpu_in(3 * PAGE, 4 * PAGE));
        assert_eq!(p.take_gpu(0, 2 * PAGE, true), [(PAGE, 2 * PAGE)]);
        assert_eq!(p.take_gpu(0, 8 * PAGE, true), [(2 * PAGE, 3 * PAGE)]);
        assert!(!p.any_gpu());
        // Read back, a page can be written again.
        p.mark(PAGE, PAGE + 1);
        assert_eq!(p.take_dirty(0, 8 * PAGE), [(PAGE, 2 * PAGE)]);
    }

    #[test]
    fn pages_drawn_and_read_back_on_the_gpu_are_whole_pages() {
        let mut p = Tracker::new(8 * PAGE as usize);
        p.take_dirty(0, 8 * PAGE);
        // A 2D operation over part of two pages has both, and pages that
        // meet are one run.
        p.drawn_on_gpu(PAGE + 100, 2 * PAGE + 1);
        p.drawn_on_gpu(3 * PAGE, 3 * PAGE + 1);
        assert!(p.gpu_in(PAGE, PAGE + 1) && p.gpu_in(3 * PAGE - 1, 3 * PAGE));
        // A read of no bytes reads back nothing, one of one byte its page.
        assert!(p.take_gpu(PAGE + 5, PAGE + 5, false).is_empty());
        assert_eq!(p.take_gpu(PAGE + 5, PAGE + 6, false), [(PAGE, 2 * PAGE)]);
        assert_eq!(p.take_gpu(0, 8 * PAGE, false), [(2 * PAGE, 4 * PAGE)]);
        // Past the memory's end, nothing is drawn or read.
        p.drawn_on_gpu(7 * PAGE, 9 * PAGE);
        assert_eq!(p.take_gpu(0, u32::MAX, false), [(7 * PAGE, 8 * PAGE)]);
    }

    #[test]
    fn pages_read_back_for_the_program_are_hot_until_they_cool() {
        let mut p = Tracker::new(8 * PAGE as usize);
        p.take_dirty(0, 8 * PAGE);
        p.drawn_on_gpu(PAGE, 3 * PAGE);
        assert!(!p.hot_in(0, 8 * PAGE));
        p.take_gpu(2 * PAGE, 3 * PAGE, true);
        assert!(p.hot_in(2 * PAGE + 1, 2 * PAGE + 2) && !p.hot_in(0, 2 * PAGE));
        // Read back for the emulator (a save state), a page stays cool.
        p.take_gpu(PAGE, 2 * PAGE, false);
        assert!(!p.hot_in(PAGE, 2 * PAGE));
        p.cool();
        assert!(!p.hot_in(0, 8 * PAGE));
    }

    #[test]
    fn pages_resent_are_all_but_those_newer_on_the_gpu() {
        let mut p = Tracker::new(8 * PAGE as usize);
        p.take_dirty(0, 8 * PAGE);
        p.drawn_on_gpu(2 * PAGE, 3 * PAGE);
        p.resend();
        let runs = [(0, 2 * PAGE), (3 * PAGE, 8 * PAGE)];
        assert_eq!(p.take_dirty(0, 8 * PAGE), runs);
        assert!(p.gpu_in(2 * PAGE, 3 * PAGE));
    }

    #[test]
    fn bytes_in_the_buffers_are_read_back_as_asked_not_by_pages() {
        // Rows of a 3D buffer, 1280 bytes each: rows 0..480 of one at 0
        // and of the next right after it are one range, which reads
        // take apart.
        let mut p = Tracker::new(4 << 20);
        p.take_dirty(0, 4 << 20);
        let row = 1280;
        p.drawn_in_buffer(0, 480 * row);
        p.drawn_in_buffer(480 * row, 960 * row);
        assert!(p.any_in_buffers() && !p.any_gpu());
        assert!(p.buffers_in(479 * row, 481 * row) && !p.buffers_in(960 * row, u32::MAX));
        assert_eq!(
            p.take_buffers(100 * row, 106 * row, true),
            [(100 * row, 106 * row)]
        );
        assert!(p.take_buffers(100 * row, 106 * row, true).is_empty());
        // Read back for the program, the rows are hot; read back for the
        // emulator, they stay cool.
        assert!(p.hot_in(105 * row, 107 * row) && !p.hot_in(106 * row, 107 * row));
        assert_eq!(
            p.take_buffers(0, 480 * row, false),
            [(0, 100 * row), (106 * row, 480 * row)]
        );
        assert!(!p.hot_in(0, 100 * row) && !p.hot_in(106 * row, 480 * row));
        // Either way the texture units hear of them once.
        assert!(p.take_read_back(105 * row, 106 * row));
        assert!(!p.take_read_back(105 * row, 106 * row));
        assert!(!p.take_read_back(480 * row, 481 * row));
        // They are kept apart from the pages of the renderer's copy of the
        // memory: neither reads back the other's.
        p.drawn_on_gpu(0, PAGE);
        assert!(p.take_gpu(480 * row, 481 * row, true).is_empty());
        assert!(!p.hot_in(480 * row, 481 * row));
        p.forget_buffers();
        assert!(!p.any_in_buffers() && p.any_gpu());
    }
}

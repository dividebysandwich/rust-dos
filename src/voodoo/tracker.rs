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
//! CPU than drawn on the GPU and read back again. It stays hot while the
//! program goes on reading it (`touch`), and cools once a period goes by
//! without a read.
//!
//! A page where none of that is behind a program's read or write of it
//! through BAR1 lets the program's next ones skip the bookkeeping
//! (`direct`): its reads where nothing there is newer on the GPU, its
//! writes where the page is dirty already as well. The page loses that
//! when it changes state, at the end of a period if it is hot (so that a
//! read keeps it hot), or for writes when the renderer takes it or the
//! texture units have to hear of what was written.

/// Bytes a page, a row of the renderer's texture of the memory.
pub const PAGE: u32 = 4096;
const SHIFT: u32 = PAGE.trailing_zeros();

/// What a program's reads (`DIRECT_READ`) and writes (`DIRECT_WRITE`) of
/// a page through BAR1 may skip all of this for (`Tracker::direct`).
pub const DIRECT_READ: u8 = 1;
pub const DIRECT_WRITE: u8 = 2;
/// A page given `DIRECT_WRITE` since the texture units last heard of
/// writes to it (`Tracker::take_unstamped`).
const UNSTAMPED: u8 = 4;

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
    /// Bytes of the 3D buffers that changed in the card's memory since a
    /// Banshee's texture units last looked (`take_changed`): read back
    /// from the renderer, or drawn into in software.
    changed: Ranges,
    /// Bytes read back for the program since the last `cool`, and those
    /// hot before it that the program read again in the period before.
    hot: Ranges,
    /// The hot bytes the program read since the last `cool`, which stay
    /// hot through the next. The read that turns bytes hot isn't one of
    /// them: bytes read back once cool at the next `cool`, however soon.
    read: Ranges,
    /// For each page, what a program's own reads and writes of it may
    /// do by themselves (`DIRECT_READ`, `DIRECT_WRITE`), and `UNSTAMPED`.
    direct: Vec<u8>,
    /// The pages with `UNSTAMPED`, as page numbers.
    unstamped: Vec<u32>,
    /// How many times pages lost what `direct` let them do: whoever
    /// copied it looks again when this changes.
    generation: u64,
    /// How many times a page was let do more, for `/api/status`.
    grants: u64,
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
            direct: vec![0; pages],
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

    /// The renderer starts over, or the card's memory was loaded: the
    /// renderer has none of the memory, and what only it had, in its copy
    /// of the memory or in its 3D buffers, is gone. The caller reads that
    /// back first.
    pub fn mark_all(&mut self) {
        self.dirty.fill(u64::MAX);
        self.gpu.clear();
        self.buffers.clear();
        // What pages were let do went with what they had.
        self.direct.fill(0);
        self.unstamped.clear();
        self.generation += 1;
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
        let mut taken = false;
        for page in self.span(start, end) {
            if get(&self.dirty, page) {
                set(&mut self.dirty, page, false);
                add_run(&mut runs, page);
                // A write there has to mark it again.
                if let Some(d) = self.direct.get_mut(page) {
                    taken |= *d & DIRECT_WRITE != 0;
                    *d &= !DIRECT_WRITE;
                }
            }
        }
        self.generation += taken as u64;
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
        self.revoke(start, end);
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
                self.revoke(a, b);
            }
        }
        runs
    }

    /// The renderer drew bytes `start..end` into a 3D buffer of its own,
    /// which the card's memory doesn't have until they are read back by
    /// rows of the buffer.
    pub fn drawn_in_buffer(&mut self, start: u32, end: u32) {
        self.buffers.add(start, end.min(self.size));
        self.revoke(start, end);
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
            self.changed.add(a, b);
            if hot {
                self.hot.add(a, b);
                self.revoke(a, b);
            }
        }
        taken
    }

    /// Bytes `start..end` of a 3D buffer are drawn into in the card's
    /// memory, by the software rasterizer.
    pub fn drawn_in_memory(&mut self, start: u32, end: u32) {
        self.changed.add(start, end.min(self.size));
    }

    /// Whether any of bytes `start..end` of the 3D buffers changed in the
    /// card's memory (`changed`) since the last call for them: the
    /// textures decoded from there are out of date.
    pub fn take_changed(&mut self, start: u32, end: u32) -> bool {
        !self.changed.is_empty() && !self.changed.take(start, end).is_empty()
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

    /// Whether anything was read back since the last `cool`.
    pub fn any_hot(&self) -> bool {
        !self.hot.is_empty()
    }

    /// The program reads or writes bytes `start..end`: those of them
    /// that are hot stay hot through the next `cool`, though nothing
    /// there was behind the GPU to read back.
    #[inline]
    pub fn touch(&mut self, start: u32, end: u32) {
        if self.hot.is_empty() || !self.hot.any_in(start, end) {
            return;
        }
        let first = self.hot.0.partition_point(|r| r.1 <= start);
        let last = self.hot.0.partition_point(|r| r.0 < end);
        for i in first..last {
            let (a, b) = self.hot.0[i];
            self.read.add(a.max(start), b.min(end));
        }
    }

    /// A period went by: what the program didn't read in it cools, as it
    /// may have stopped reading there, and what it read stays hot.
    pub fn cool(&mut self) {
        // Reads that skip the bookkeeping don't keep their page hot: the
        // pages hot until now go through it again, once, to stay hot.
        for (a, b) in std::mem::take(&mut self.hot).iter() {
            self.revoke(a, b);
        }
        self.hot = std::mem::take(&mut self.read);
    }

    /// What a program's own reads and writes of the page with byte `at`
    /// may do by themselves (`DIRECT_READ`, `DIRECT_WRITE`).
    #[inline]
    pub fn direct_at(&self, at: u32) -> u8 {
        self.direct
            .get((at >> SHIFT) as usize)
            .map_or(0, |d| d & (DIRECT_READ | DIRECT_WRITE))
    }

    /// `direct_at` for every page, first to last.
    pub fn direct(&self) -> impl Iterator<Item = u8> + '_ {
        self.direct.iter().map(|d| d & (DIRECT_READ | DIRECT_WRITE))
    }

    /// Changes every time a page loses what `direct` let it do.
    pub fn direct_generation(&self) -> u64 {
        self.generation
    }

    /// How many times a page was let do more (`grant`).
    pub fn direct_grants(&self) -> u64 {
        self.grants
    }

    /// A program read (`write` false) or wrote the page with byte `at`
    /// through BAR1, with all that goes with it done: its reads need
    /// nothing else from now on where nothing on it is newer on the GPU
    /// or in a 3D buffer. So do its writes, if the caller found nothing
    /// else a write there does (`write`) and the page is dirty already,
    /// so that a write has nothing to mark. The texture units hear of
    /// those writes later (`take_unstamped`). A read or write of hot
    /// bytes keeps them hot (`touch`): the hot bytes of the page stay hot
    /// through the next `cool` as if the program read them all, and that
    /// `cool` takes what the page was given.
    pub fn grant(&mut self, at: u32, write: bool) {
        let page = (at >> SHIFT) as usize;
        let Some(&had) = self.direct.get(page) else {
            return;
        };
        let want = if write && get(&self.dirty, page) {
            DIRECT_READ | DIRECT_WRITE
        } else {
            DIRECT_READ
        };
        if had & want == want {
            return;
        }
        let (start, end) = (page as u32 * PAGE, (page as u32 + 1) * PAGE);
        if self.gpu_in(start, end) || self.buffers_in(start, end) {
            return;
        }
        self.touch(start, end);
        let mut now = had | want;
        if want & DIRECT_WRITE != 0 && had & UNSTAMPED == 0 {
            now |= UNSTAMPED;
            self.unstamped.push(page as u32);
        }
        self.direct[page] = now;
        self.grants += 1;
    }

    /// The pages bytes `start..end` touch lose what `direct` let them do.
    fn revoke(&mut self, start: u32, end: u32) {
        let mut lost = false;
        for page in self.span(start, end) {
            let Some(d) = self.direct.get_mut(page) else {
                break;
            };
            lost |= *d & (DIRECT_READ | DIRECT_WRITE) != 0;
            *d &= UNSTAMPED;
        }
        self.generation += lost as u64;
    }

    /// The recording's 3D buffers moved: a write may now have a pixel of
    /// theirs to tell it. Every page loses `DIRECT_WRITE`.
    pub fn revoke_writes(&mut self) {
        let mut lost = false;
        for &page in &self.unstamped {
            let d = &mut self.direct[page as usize];
            lost |= *d & DIRECT_WRITE != 0;
            *d &= !DIRECT_WRITE;
        }
        self.generation += lost as u64;
    }

    /// The pages given `DIRECT_WRITE` since the last call, as byte ranges
    /// of whole pages, first to last: the program may have written them
    /// without the texture units hearing of it, which the caller tells
    /// them. Their writes go through the card again until it gives them
    /// `DIRECT_WRITE` again, so that the next caller hears of those too.
    pub fn take_unstamped(&mut self) -> Vec<(u32, u32)> {
        if self.unstamped.is_empty() {
            return Vec::new();
        }
        let mut pages = std::mem::take(&mut self.unstamped);
        pages.sort_unstable();
        let mut runs = Vec::new();
        let mut lost = false;
        for &page in &pages {
            let d = &mut self.direct[page as usize];
            lost |= *d & DIRECT_WRITE != 0;
            *d &= DIRECT_READ;
            add_run(&mut runs, page as usize);
        }
        self.generation += lost as u64;
        pages.clear();
        self.unstamped = pages;
        runs
    }

    /// Two periods without reads went by: nothing is hot.
    #[cfg(test)]
    pub fn cool_all(&mut self) {
        self.cool();
        self.cool();
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
        // Read back once and not read again, it cools at the next cool,
        // however soon that comes.
        p.cool();
        assert!(!p.any_hot());
    }

    #[test]
    fn hot_bytes_the_program_goes_on_reading_stay_hot() {
        // A program that reads the same rows every frame (Test Drive 4's
        // two pixels of the back buffer) finds them in the memory after
        // the first read back, with nothing behind the GPU there: its
        // reads keep them hot, and they never go back to the GPU only to
        // be read back again.
        let mut p = Tracker::new(8 * PAGE as usize);
        p.take_dirty(0, 8 * PAGE);
        p.drawn_on_gpu(PAGE, 3 * PAGE);
        p.take_gpu(2 * PAGE, 3 * PAGE, true);
        for _ in 0..4 {
            p.touch(2 * PAGE + 10, 2 * PAGE + 20);
            p.cool();
            assert!(p.hot_in(2 * PAGE + 10, 2 * PAGE + 20));
            // Only the bytes read stay: the rest of the page cooled.
            assert!(!p.hot_in(2 * PAGE + 20, 3 * PAGE));
        }
        // A read of bytes that aren't hot leaves them cool, and a period
        // without reads cools the rest.
        p.touch(5 * PAGE, 6 * PAGE);
        p.cool();
        assert!(!p.any_hot());
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
        // Either way the texture units hear of them once, as they do of
        // rows drawn into in software.
        assert!(p.take_changed(105 * row, 106 * row));
        assert!(!p.take_changed(105 * row, 106 * row));
        assert!(!p.take_changed(480 * row, 481 * row));
        p.drawn_in_memory(480 * row, 482 * row);
        assert!(p.take_changed(481 * row, 490 * row));
        // They are kept apart from the pages of the renderer's copy of the
        // memory: neither reads back the other's.
        p.drawn_on_gpu(0, PAGE);
        assert!(p.take_gpu(480 * row, 481 * row, true).is_empty());
        assert!(!p.hot_in(480 * row, 481 * row));
        p.forget_buffers();
        assert!(!p.any_in_buffers() && p.any_gpu());
        // A renderer starting over has neither.
        p.drawn_in_buffer(0, row);
        p.mark_all();
        assert!(!p.any_in_buffers() && !p.any_gpu());
    }

    #[test]
    fn pages_go_direct_while_nothing_is_behind_them() {
        const RW: u8 = DIRECT_READ | DIRECT_WRITE;
        let mut p = Tracker::new(8 * PAGE as usize);
        p.take_dirty(0, 8 * PAGE);
        // A clean page's writes have it to mark: only its reads go direct
        // until a write marked it.
        p.grant(PAGE + 5, true);
        assert_eq!(p.direct_at(PAGE), DIRECT_READ);
        p.mark(PAGE, PAGE + 1);
        p.grant(PAGE + 5, true);
        assert_eq!(p.direct_at(2 * PAGE - 1), RW);
        // Nothing the caller found a write does (`write` false): reads.
        p.mark(3 * PAGE, 3 * PAGE + 1);
        p.grant(3 * PAGE, false);
        assert_eq!(p.direct_at(3 * PAGE), DIRECT_READ);
        assert_eq!(
            p.direct().collect::<Vec<_>>(),
            [0, RW, 0, DIRECT_READ, 0, 0, 0, 0]
        );
        // Pages newer on the GPU or in a 3D buffer get nothing.
        p.drawn_on_gpu(4 * PAGE, 5 * PAGE);
        p.drawn_in_buffer(5 * PAGE + 10, 5 * PAGE + 20);
        for page in 4..6 {
            p.grant(page * PAGE, false);
            assert_eq!(p.direct_at(page * PAGE), 0, "page {page}");
        }
        // A hot page's hot bytes stay hot through the next period, which
        // takes what the page was given: the next read keeps them hot.
        p.drawn_on_gpu(6 * PAGE, 7 * PAGE);
        p.take_gpu(6 * PAGE, 7 * PAGE, true);
        p.grant(6 * PAGE, false);
        assert_eq!(p.direct_at(6 * PAGE), DIRECT_READ);
        p.cool();
        assert!(p.hot_in(6 * PAGE, 7 * PAGE));
        assert_eq!(p.direct_at(6 * PAGE), 0);
        assert_eq!(p.direct_at(PAGE), RW, "a cool page keeps it");
        p.cool();
        assert!(!p.any_hot());
        // Past the memory, nothing.
        p.grant(8 * PAGE, false);
        assert_eq!(p.direct_at(8 * PAGE), 0);
    }

    #[test]
    fn pages_lose_direct_access_as_their_state_changes() {
        const RW: u8 = DIRECT_READ | DIRECT_WRITE;
        let mut p = Tracker::new(8 * PAGE as usize);
        for page in 0..8 {
            p.grant(page * PAGE, true);
        }
        assert!(p.direct().all(|d| d == RW));
        let generation = p.direct_generation();
        // The renderer takes a page: writes mark it again.
        p.take_dirty(0, PAGE);
        assert_eq!(p.direct_at(0), DIRECT_READ);
        assert_ne!(p.direct_generation(), generation);
        // The GPU draws a page (which it has), or a 3D buffer is drawn
        // there: neither.
        p.take_dirty(PAGE, 2 * PAGE);
        p.drawn_on_gpu(PAGE, PAGE + 1);
        p.drawn_in_buffer(2 * PAGE + 100, 3 * PAGE + 1);
        assert_eq!(
            p.direct().collect::<Vec<_>>(),
            [DIRECT_READ, 0, 0, 0, RW, RW, RW, RW]
        );
        // The buffers move: no page's writes go direct.
        let generation = p.direct_generation();
        p.revoke_writes();
        assert_eq!(p.direct_at(4 * PAGE), DIRECT_READ);
        assert_ne!(p.direct_generation(), generation);
        // A taking that loses nothing changes nothing.
        let generation = p.direct_generation();
        p.take_dirty(0, 8 * PAGE);
        p.revoke_writes();
        assert_eq!(p.direct_generation(), generation);
        p.mark_all();
        assert!(p.direct().all(|d| d == 0));
    }

    #[test]
    fn pages_written_directly_reach_the_texture_units_once() {
        let mut p = Tracker::new(8 * PAGE as usize);
        p.grant(5 * PAGE, true);
        p.grant(2 * PAGE, true);
        p.grant(3 * PAGE, true);
        p.grant(3 * PAGE + 4, true);
        // A page given writes again after it lost them is one page still.
        p.take_dirty(2 * PAGE, 3 * PAGE);
        p.mark(2 * PAGE, 3 * PAGE);
        p.grant(2 * PAGE, true);
        assert_eq!(
            p.take_unstamped(),
            [(2 * PAGE, 4 * PAGE), (5 * PAGE, 6 * PAGE)]
        );
        // Their writes go through the card from now on, which marks them.
        assert_eq!(p.direct_at(2 * PAGE), DIRECT_READ);
        assert!(p.take_unstamped().is_empty());
        p.grant(5 * PAGE, true);
        assert_eq!(p.take_unstamped(), [(5 * PAGE, 6 * PAGE)]);
    }
}

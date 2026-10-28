//! The OpenGL renderer's copy of a Banshee's or Voodoo3's memory, page by
//! page. The renderer keeps all of the card's memory as one texture
//! (`gl::CardMemory`), which the picture is composed from and which the
//! 2D operations it does itself draw into (`gpu`). A page of 4 KB, one
//! row of that texture, is in one of three states:
//!
//! - the same in both;
//! - newer in the card's memory, written since the renderer last had it
//!   (dirty): the renderer gets it again before it reads it, so that
//!   moving the picture to memory the renderer has (a flip) costs
//!   nothing;
//! - newer on the GPU, which drew into it while the card's memory was
//!   left as it was: whatever reads or writes the card's memory there
//!   reads the page back from the GPU first.
//!
//! A page is never both dirty and newer on the GPU: the GPU only draws
//! into pages it has.
//!
//! A page read back is hot for a while (`cool`): the program reads or
//! writes it, and a 2D operation there is cheaper on the CPU than drawn
//! on the GPU and read back again.

/// Bytes a page, a row of the renderer's texture of the memory.
pub const PAGE: u32 = 4096;
const SHIFT: u32 = PAGE.trailing_zeros();

#[derive(Clone, Debug, Default)]
pub struct Pages {
    dirty: Vec<u64>,
    gpu: Vec<u64>,
    /// How many pages are newer on the GPU, so that the memory's
    /// accesses look no further while there are none.
    gpu_count: usize,
    /// Pages read back since the last `cool`.
    hot: Vec<u64>,
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

impl Pages {
    /// The pages of `bytes` of memory, all dirty: the renderer has none of
    /// them yet.
    pub fn new(bytes: usize) -> Self {
        let words = bytes.div_ceil(PAGE as usize).div_ceil(64);
        let mut p = Self {
            dirty: vec![0; words],
            gpu: vec![0; words],
            gpu_count: 0,
            hot: vec![0; words],
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

    /// Bytes `start..end` of the card's memory were written: the renderer
    /// gets them again. The caller read back the pages there that were
    /// newer on the GPU before it wrote (`take_gpu`).
    pub fn mark(&mut self, start: u32, end: u32) {
        for page in self.span(start, end) {
            debug_assert!(!get(&self.gpu, page), "page {page} is the GPU's");
            set(&mut self.dirty, page, true);
        }
    }

    /// The renderer starts over: it has none of the memory. What only it
    /// had is gone with it, so the caller reads that back first.
    pub fn mark_all(&mut self) {
        self.dirty.fill(u64::MAX);
        self.gpu.fill(0);
        self.gpu_count = 0;
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

    /// Whether any page is newer on the GPU.
    #[inline]
    pub fn any_gpu(&self) -> bool {
        self.gpu_count != 0
    }

    /// The GPU drew into bytes `start..end`, whose pages it has (the
    /// caller sent the dirty ones first): the card's memory there is out
    /// of date until they are read back.
    pub fn drawn_on_gpu(&mut self, start: u32, end: u32) {
        for page in self.span(start, end) {
            debug_assert!(!get(&self.dirty, page), "page {page} is dirty");
            if !get(&self.gpu, page) {
                set(&mut self.gpu, page, true);
                self.gpu_count += 1;
            }
        }
    }

    /// Whether any of bytes `start..end` is on a page newer on the GPU.
    #[inline]
    pub fn gpu_in(&self, start: u32, end: u32) -> bool {
        self.gpu_count != 0 && self.span(start, end).any(|p| get(&self.gpu, p))
    }

    /// The runs of pages within bytes `start..end` that are newer on the
    /// GPU, as byte ranges; the same in both from now on, as the caller
    /// reads them back.
    pub fn take_gpu(&mut self, start: u32, end: u32) -> Vec<(u32, u32)> {
        let mut runs = Vec::new();
        if self.gpu_count == 0 {
            return runs;
        }
        for page in self.span(start, end) {
            if get(&self.gpu, page) {
                set(&mut self.gpu, page, false);
                set(&mut self.hot, page, true);
                self.gpu_count -= 1;
                add_run(&mut runs, page);
            }
        }
        runs
    }

    /// Whether any of bytes `start..end` is on a page read back since the
    /// last `cool`.
    pub fn hot_in(&self, start: u32, end: u32) -> bool {
        self.span(start, end).any(|p| get(&self.hot, p))
    }

    /// No page is hot any more: the program may have stopped reading
    /// them.
    pub fn cool(&mut self) {
        self.hot.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_pages_come_out_once_in_runs() {
        let mut p = Pages::new(16 * PAGE as usize);
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
        let mut p = Pages::new(8 * PAGE as usize);
        p.take_dirty(0, 8 * PAGE);
        assert!(!p.any_gpu());
        p.drawn_on_gpu(PAGE, 3 * PAGE);
        assert!(p.any_gpu() && p.gpu_in(2 * PAGE + 5, 2 * PAGE + 6));
        assert!(!p.gpu_in(0, PAGE) && !p.gpu_in(3 * PAGE, 4 * PAGE));
        assert_eq!(p.take_gpu(0, 2 * PAGE), [(PAGE, 2 * PAGE)]);
        assert_eq!(p.take_gpu(0, 8 * PAGE), [(2 * PAGE, 3 * PAGE)]);
        assert!(!p.any_gpu());
        // Read back, a page can be written again.
        p.mark(PAGE, PAGE + 1);
        assert_eq!(p.take_dirty(0, 8 * PAGE), [(PAGE, 2 * PAGE)]);
    }

    #[test]
    fn pages_read_back_are_hot_until_they_cool() {
        let mut p = Pages::new(8 * PAGE as usize);
        p.take_dirty(0, 8 * PAGE);
        p.drawn_on_gpu(PAGE, 3 * PAGE);
        assert!(!p.hot_in(0, 8 * PAGE));
        p.take_gpu(2 * PAGE, 3 * PAGE);
        assert!(p.hot_in(2 * PAGE + 1, 2 * PAGE + 2) && !p.hot_in(0, 2 * PAGE));
        p.cool();
        assert!(!p.hot_in(0, 8 * PAGE));
    }
}

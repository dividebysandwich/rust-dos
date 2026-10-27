//! The OpenGL renderer's copy of a Banshee's or Voodoo3's memory, page by
//! page. The renderer keeps all of the card's memory as one texture
//! (`gl::CardMemory`), which the picture is composed from. A page of
//! 4 KB, one row of that texture, is either the same in both or newer in
//! the card's memory, written since the renderer last had it (dirty):
//! the renderer gets a dirty page again before it reads it, so that
//! moving the picture to memory the renderer has (a flip) costs nothing.

/// Bytes a page, a row of the renderer's texture of the memory.
pub const PAGE: u32 = 4096;
const SHIFT: u32 = PAGE.trailing_zeros();

#[derive(Clone, Debug, Default)]
pub struct Pages {
    dirty: Vec<u64>,
}

/// Add page `page` to `runs`, byte ranges of whole pages in order.
fn add_run(runs: &mut Vec<(u32, u32)>, page: usize) {
    let (a, b) = (page as u32 * PAGE, (page as u32 + 1) * PAGE);
    match runs.last_mut() {
        Some(last) if last.1 == a => last.1 = b,
        _ => runs.push((a, b)),
    }
}

impl Pages {
    /// The pages of `bytes` of memory, all dirty: the renderer has none of
    /// them yet.
    pub fn new(bytes: usize) -> Self {
        let pages = bytes.div_ceil(PAGE as usize);
        let mut p = Self {
            dirty: vec![0; pages.div_ceil(64)],
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

    /// Bytes `start..end` of the card's memory were written: the renderer
    /// gets them again.
    pub fn mark(&mut self, start: u32, end: u32) {
        for page in self.span(start, end) {
            Self::set(&mut self.dirty, page, true);
        }
    }

    /// The renderer starts over: it has none of the memory.
    pub fn mark_all(&mut self) {
        self.dirty.fill(u64::MAX);
    }

    /// The runs of dirty pages within bytes `start..end`, as byte ranges
    /// of whole pages, which are clean from now on: the caller sends them
    /// to the renderer.
    pub fn take_dirty(&mut self, start: u32, end: u32) -> Vec<(u32, u32)> {
        let mut runs = Vec::new();
        for page in self.span(start, end) {
            if Self::get(&self.dirty, page) {
                Self::set(&mut self.dirty, page, false);
                add_run(&mut runs, page);
            }
        }
        runs
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
}

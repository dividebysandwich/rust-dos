//! Drawing held back while the OpenGL renderer draws the picture. The
//! software rasterizer only has to keep the card's memory for what reads
//! it: frame buffer reads, the pixel counters, save states, screenshots.
//! Until one comes, jobs wait here, and a job whose pixels a later
//! fastfill covers before anything could see them is dropped, so the
//! frames nobody reads back cost nothing to draw. What is left runs in
//! order when something reads (`Voodoo::catch_up`), so the memory ends up
//! as it would have been.
//!
//! Which job's pixels stay visible is worked out on tiles: each buffer
//! (colour buffers 0-2, the auxiliary buffer) is 8x8 tiles over the
//! picture. A job writes and reads the tiles its screen rectangle touches,
//! and a fastfill kills the tiles it covers whole. A job that could touch
//! memory outside the buffers' pictures (no clipping, a clip rectangle
//! bigger than the picture, rows that wrap) is kept, and taken to read
//! everything.

use super::mem::Vram;
use super::raster::RasterState;
use super::workers::{Job, Word};

/// Tiles per buffer side.
const TILES: u32 = 8;
const ALL: u64 = u64::MAX;
/// The auxiliary buffer's slot.
const AUX: usize = 3;

/// Where the buffers are, as the tiles see them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileLayout {
    /// Word offsets of colour buffers 0-2 and the auxiliary buffer.
    pub bases: [Option<usize>; 4],
    pub width: u32,
    pub height: u32,
    pub rowpixels: u32,
    /// Whether the buffers are apart, so that tiles of one are not tiles
    /// of another. Without, nothing is dropped.
    pub prunable: bool,
}

impl TileLayout {
    /// The buffers at `bases` (word offsets) in `words` of frame buffer.
    pub fn new(bases: [Option<usize>; 4], width: u32, height: u32, rowpixels: u32, words: usize) -> Self {
        let size = height as usize * rowpixels as usize;
        let mut prunable = width > 0 && height > 0 && width <= rowpixels;
        for (i, a) in bases.iter().enumerate() {
            let Some(a) = *a else { continue };
            if a + size > words {
                prunable = false;
            }
            for b in bases.iter().skip(i + 1).flatten() {
                if a < b + size && *b < a + size {
                    prunable = false;
                }
            }
        }
        Self { bases, width, height, rowpixels, prunable }
    }

    fn slot(&self, dest: usize) -> Option<usize> {
        self.bases[..AUX].iter().position(|&b| b == Some(dest))
    }

    /// Tile `k`'s first column (or row) of `size`: pixel `p` is in tile
    /// `p * TILES / size`.
    fn edge(k: u32, size: u32) -> u32 {
        (k * size).div_ceil(TILES)
    }

    /// The tiles a rectangle of pixels touches, or covers whole.
    fn tiles(&self, (x0, x1, y0, y1): (u32, u32, u32, u32), whole: bool) -> u64 {
        if x0 >= x1 || y0 >= y1 {
            return 0;
        }
        let (w, h) = (self.width, self.height);
        let mut mask = 0;
        for ty in 0..TILES {
            let (r0, r1) = (Self::edge(ty, h), Self::edge(ty + 1, h));
            if r0 >= r1 {
                continue;
            }
            let rows = if whole { y0 <= r0 && r1 <= y1 } else { y0 < r1 && r0 < y1 };
            if !rows {
                continue;
            }
            for tx in 0..TILES {
                let (c0, c1) = (Self::edge(tx, w), Self::edge(tx + 1, w));
                if c0 >= c1 {
                    continue;
                }
                let cols = if whole { x0 <= c0 && c1 <= x1 } else { x0 < c1 && c0 < x1 };
                if cols {
                    mask |= 1 << (ty * TILES + tx);
                }
            }
        }
        mask
    }
}

/// What a job does to the buffers' tiles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Access {
    pub reads: [u64; 4],
    pub writes: [u64; 4],
    /// Tiles written whole: what was there before is gone.
    pub kills: [u64; 4],
    /// It may touch memory outside the tiles: kept, and taken to read
    /// everything.
    pub pinned: bool,
}

impl Access {
    const PINNED: Access = Access { reads: [0; 4], writes: [0; 4], kills: [0; 4], pinned: true };

    fn add(&mut self, other: &Access) {
        for r in 0..4 {
            self.reads[r] |= other.reads[r];
            self.writes[r] |= other.writes[r];
            self.kills[r] |= other.kills[r];
        }
        self.pinned |= other.pinned;
    }
}

/// Rows `y0..y1` as a triangle or fastfill counts them, as buffer rows
/// `r0..r1` (the Y origin applied); None if they could wrap around.
fn buffer_rows(st: &RasterState, y0: i32, y1: i32) -> Option<(i32, i32)> {
    if y0 >= y1 {
        return Some((0, 0));
    }
    if st.fbz_mode & (1 << 17) == 0 {
        return Some((y0, y1));
    }
    let origin = st.yorigin as i32;
    let (r0, r1) = (origin - (y1 - 1), origin - y0 + 1);
    (r0 >= 0 && r1 <= 0x400).then_some((r0, r1))
}

/// The rectangle `x0..x1` by buffer rows `r0..r1` in pixels of the
/// picture; None if it reaches outside it.
fn inside(layout: &TileLayout, x0: i32, x1: i32, r0: i32, r1: i32) -> Option<(u32, u32, u32, u32)> {
    let (x0, r0) = (x0.max(0), r0.max(0));
    if x0 >= x1 || r0 >= r1 {
        return Some((0, 0, 0, 0));
    }
    (x1 as u32 <= layout.width && r1 as u32 <= layout.height).then_some((x0 as u32, x1 as u32, r0 as u32, r1 as u32))
}

/// Whether a triangle may draw right of the buffer's rows or below its
/// `height`, into memory of other rows, which a different worker draws:
/// the workers could then draw its pixels there out of order.
pub fn spills(st: &RasterState, verts: &[(f32, f32); 3], height: u32) -> bool {
    let max = |c: fn(&(f32, f32)) -> f32| verts.iter().map(c).fold(f32::NEG_INFINITY, f32::max);
    let min = |c: fn(&(f32, f32)) -> f32| verts.iter().map(c).fold(f32::INFINITY, f32::min);
    let (maxx, miny, maxy) = (max(|v| v.0), min(|v| v.1), max(|v| v.1));
    if !maxx.is_finite() || !miny.is_finite() || !maxy.is_finite() {
        return true;
    }
    let mut reach = maxx.ceil().min(8192.0) as i32 + 2;
    let y0 = (miny.floor().max(-8192.0) as i32 - 1).max(-2048);
    let y1 = (maxy.ceil().min(8192.0) as i32 + 2).min(4096);
    let clip_bottom = (st.clip_low_y_high_y & 0x3FF) as i32;
    if st.fbz_mode & 1 != 0 {
        reach = reach.min((st.clip_left_right & 0x3FF) as i32);
        if clip_bottom <= height as i32 {
            // No row below the clip rectangle is drawn, wherever the Y
            // origin takes the triangle's (one that reaches past the origin
            // wraps around to rows far below, as most at a screen's edge do).
            return reach > st.rowpixels as i32;
        }
    }
    let Some((r0, mut r1)) = buffer_rows(st, y0, y1) else { return true };
    if st.fbz_mode & 1 != 0 {
        r1 = r1.min(clip_bottom);
    }
    reach > st.rowpixels as i32 || (r1 > height as i32 && r1 > r0.max(0))
}

/// The same for a fastfill of columns `..x1` and rows `y0..y1`.
pub fn fill_spills(st: &RasterState, x1: i32, y0: i32, y1: i32, height: u32) -> bool {
    match buffer_rows(st, y0, y1) {
        Some((r0, r1)) => x1 > st.rowpixels as i32 || (r1 > height as i32 && r1 > r0.max(0)),
        None => true,
    }
}

/// A triangle with vertices `verts` (in pixels) drawn with `st`.
pub fn triangle(layout: &TileLayout, st: &RasterState, verts: &[(f32, f32); 3]) -> Access {
    let fbz = st.fbz_mode;
    let (rgb, aux_write) = (fbz & (1 << 9) != 0, fbz & (1 << 10) != 0 && st.aux.is_some());
    if !rgb && !aux_write {
        return Access::default();
    }
    if !layout.prunable || verts.iter().any(|v| !v.0.is_finite() || !v.1.is_finite()) {
        return Access::PINNED;
    }
    let Some(slot) = layout.slot(st.dest) else { return Access::PINNED };
    // The pixels the scan conversion can reach: its rows and columns are
    // rounded vertex coordinates, with a pixel to spare.
    let bound = |f: fn(f32, f32) -> f32, c: fn(&(f32, f32)) -> f32| verts.iter().map(c).fold(c(&verts[0]), f);
    let (minx, maxx) = (bound(f32::min, |v| v.0), bound(f32::max, |v| v.0));
    let (miny, maxy) = (bound(f32::min, |v| v.1), bound(f32::max, |v| v.1));
    let clamp = |v: f32| v.clamp(-8192.0, 8192.0) as i32;
    let (mut x0, mut x1) = (clamp(minx.floor()) - 1, clamp(maxx.ceil()) + 2);
    let (y0, y1) = ((clamp(miny.floor()) - 1).max(-2048), (clamp(maxy.ceil()) + 2).min(4096));
    let Some((mut r0, mut r1)) = buffer_rows(st, y0, y1) else { return Access::PINNED };
    if fbz & 1 != 0 {
        let (lr, ly) = (st.clip_left_right, st.clip_low_y_high_y);
        x0 = x0.max(((lr >> 16) & 0x3FF) as i32);
        x1 = x1.min((lr & 0x3FF) as i32);
        r0 = r0.max(((ly >> 16) & 0x3FF) as i32);
        r1 = r1.min((ly & 0x3FF) as i32);
    }
    let Some(rect) = inside(layout, x0, x1, r0, r1) else { return Access::PINNED };
    let touched = layout.tiles(rect, false);
    let mut access = Access::default();
    if rgb {
        access.writes[slot] = touched;
    }
    if aux_write {
        access.writes[AUX] = touched;
    }
    let depth_func = (fbz >> 5) & 7;
    if fbz & (1 << 4) != 0 && (1..=6).contains(&depth_func) && st.aux.is_some() {
        access.reads[AUX] = touched;
    }
    if st.alpha_mode & (1 << 4) != 0 {
        access.reads[slot] = touched;
        if st.aux.is_some() {
            access.reads[AUX] = touched;
        }
    }
    access
}

/// A fastfill of columns `x0..x1` and rows `y0..y1` with `st`.
pub fn fastfill(layout: &TileLayout, st: &RasterState, (x0, x1, y0, y1): (i32, i32, i32, i32)) -> Access {
    let fbz = st.fbz_mode;
    let (rgb, aux) = (fbz & (1 << 9) != 0, fbz & (1 << 10) != 0 && st.aux.is_some());
    if !rgb && !aux {
        return Access::default();
    }
    if !layout.prunable {
        return Access::PINNED;
    }
    let Some(slot) = layout.slot(st.dest) else { return Access::PINNED };
    let Some((r0, r1)) = buffer_rows(st, y0, y1) else { return Access::PINNED };
    let Some(rect) = inside(layout, x0, x1, r0, r1) else { return Access::PINNED };
    let (touched, whole) = (layout.tiles(rect, false), layout.tiles(rect, true));
    let mut access = Access::default();
    for (on, s) in [(rgb, slot), (aux, AUX)] {
        if on {
            access.writes[s] = touched;
            access.kills[s] = whole;
        }
    }
    access
}

/// Frame buffer words written.
pub fn words(layout: &TileLayout, words: &[Word]) -> Access {
    if !layout.prunable {
        return Access::PINNED;
    }
    let mut access = Access::default();
    let (w, h, rowpixels) = (layout.width as usize, layout.height as usize, layout.rowpixels as usize);
    'word: for word in words {
        let (at, row) = (word.at as usize, word.row as usize);
        if row < h {
            for (slot, base) in layout.bases.iter().enumerate() {
                let Some(start) = base.map(|b| b + row * rowpixels) else { continue };
                if at >= start && at < start + w {
                    access.writes[slot] |= layout.tiles(((at - start) as u32, (at - start) as u32 + 1, row as u32, row as u32 + 1), false);
                    continue 'word;
                }
            }
        }
        return Access::PINNED;
    }
    access
}

/// Jobs a backlog holds before it prunes itself and, if still too many,
/// hands the oldest on.
const MAX_JOBS: usize = 16_384;
/// Frame buffer words it holds at most.
const MAX_WORDS: usize = 1 << 20;

/// The jobs held back, oldest first.
#[derive(Default)]
pub struct Backlog {
    pub layout: TileLayout,
    jobs: Vec<(Job, Access)>,
    words: usize,
    /// Jobs pushed since the last pruning.
    since_prune: usize,
    /// Jobs dropped, for the debugger.
    pub pruned: u64,
}

impl Backlog {
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// Hold `job`. What comes back has to go to the workers now: the
    /// oldest jobs, when there are too many.
    pub fn push(&mut self, job: Job, access: Access) -> Vec<Job> {
        let kills = access.kills.iter().any(|&k| k != 0);
        self.jobs.push((job, access));
        self.since_prune += 1;
        // A fill may have made the frames before it invisible; prune
        // when enough came since the last time.
        if kills && self.since_prune * 4 >= self.jobs.len() {
            self.prune();
        }
        self.overflow()
    }

    /// Frame buffer words written, added to the last job if it has
    /// words too.
    pub fn push_words(&mut self, fb: &Vram, new: &[Word]) -> Vec<Job> {
        let access = words(&self.layout, new);
        self.words += new.len();
        if let Some((Job::Pixels { words, .. }, last)) = self.jobs.last_mut() {
            words.extend_from_slice(new);
            last.add(&access);
            return self.overflow();
        }
        self.push(Job::Pixels { fb: fb.clone(), words: new.to_vec() }, access)
    }

    fn overflow(&mut self) -> Vec<Job> {
        if self.jobs.len() <= MAX_JOBS && self.words <= MAX_WORDS {
            return Vec::new();
        }
        self.prune();
        if self.jobs.len() <= MAX_JOBS / 2 && self.words <= MAX_WORDS / 2 {
            return Vec::new();
        }
        let older = self.jobs.len().div_ceil(2);
        let out: Vec<Job> = self.jobs.drain(..older).map(|(job, _)| job).collect();
        self.count_words();
        out
    }

    fn count_words(&mut self) {
        self.words = self.jobs.iter().map(|(job, _)| if let Job::Pixels { words, .. } = job { words.len() } else { 0 }).sum();
    }

    /// Drop the jobs whose pixels nothing can see any more: everything is
    /// visible after the last job, and going back, a job's writes are
    /// visible where the tiles are and not killed by a later fill.
    pub fn prune(&mut self) {
        self.since_prune = 0;
        let mut live = [ALL; 4];
        let mut keep = vec![true; self.jobs.len()];
        for (i, (_, a)) in self.jobs.iter().enumerate().rev() {
            if a.pinned {
                live = [ALL; 4];
            } else if (0..4).all(|r| a.writes[r] & live[r] == 0) {
                keep[i] = false;
            } else {
                for (r, live) in live.iter_mut().enumerate() {
                    *live = (*live & !a.kills[r]) | a.reads[r];
                }
            }
        }
        let before = self.jobs.len();
        let mut k = keep.into_iter();
        self.jobs.retain(|_| k.next().unwrap_or(true));
        self.pruned += (before - self.jobs.len()) as u64;
        self.count_words();
    }

    /// Everything left, pruned, oldest first.
    pub fn take(&mut self) -> Vec<Job> {
        self.prune();
        self.words = 0;
        self.jobs.drain(..).map(|(job, _)| job).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 640x480 with two colour buffers and an auxiliary buffer.
    fn layout() -> TileLayout {
        let size = 640 * 480;
        TileLayout::new([Some(0), Some(size), None, Some(2 * size)], 640, 480, 640, 3 * size)
    }

    fn write(slot: usize, tiles: u64) -> Access {
        let mut a = Access::default();
        a.writes[slot] = tiles;
        a
    }

    fn fill(slot: usize) -> Access {
        let mut a = write(slot, ALL);
        a.kills[slot] = ALL;
        a
    }

    fn backlog(accesses: &[Access]) -> Backlog {
        let mut b = Backlog { layout: layout(), ..Default::default() };
        for a in accesses {
            b.jobs.push((Job::Pixels { fb: Vram::new(16), words: Vec::new() }, *a));
        }
        b
    }

    #[test]
    fn a_fill_hides_what_was_drawn_before() {
        let mut b = backlog(&[write(0, 1), write(1, 1), fill(0), write(0, 2)]);
        b.prune();
        assert_eq!(b.len(), 3, "the triangle in buffer 0 goes, the one in buffer 1 stays");
        assert_eq!(b.pruned, 1);
        assert_eq!(b.jobs[0].1, write(1, 1));
    }

    #[test]
    fn a_job_that_reads_keeps_what_it_reads() {
        // Blending in buffer 1 over the depth test of aux: the aux job
        // before it stays although a fill kills aux later.
        let mut reader = write(1, 1);
        reader.reads[AUX] = 1;
        let mut b = backlog(&[write(AUX, 1), reader, fill(AUX)]);
        b.prune();
        assert_eq!(b.len(), 3);
    }

    #[test]
    fn partial_fills_kill_only_their_tiles() {
        let mut viewport = write(0, 0b11);
        viewport.kills[0] = 0b01;
        let mut b = backlog(&[write(0, 0b01), write(0, 0b10), viewport]);
        b.prune();
        assert_eq!(b.len(), 2);
        assert_eq!(b.jobs[0].1, write(0, 0b10));
    }

    #[test]
    fn pinned_jobs_keep_everything_before_them() {
        let mut b = backlog(&[write(0, 1), Access::PINNED, fill(0)]);
        b.prune();
        assert_eq!(b.len(), 3, "the pinned job may read the first one's pixels");
        let mut b = backlog(&[write(0, 1), fill(0), Access::PINNED]);
        b.prune();
        assert_eq!(b.len(), 2, "but not through a fill before it");
    }

    #[test]
    fn jobs_that_write_nothing_go() {
        let mut b = backlog(&[Access::default()]);
        b.prune();
        assert!(b.is_empty());
    }

    #[test]
    fn tiles_of_rectangles() {
        let l = layout();
        assert_eq!(l.tiles((0, 640, 0, 480), true), ALL);
        assert_eq!(l.tiles((0, 80, 0, 60), true), 1);
        assert_eq!(l.tiles((0, 79, 0, 60), true), 0, "a tile not covered whole");
        assert_eq!(l.tiles((79, 81, 59, 60), false), 0b11);
        assert_eq!(l.tiles((0, 640, 420, 480), false), 0xFF << 56);
    }

    #[test]
    fn overlapping_buffers_prune_nothing() {
        let size = 640 * 480;
        let l = TileLayout::new([Some(0), Some(size / 2), None, None], 640, 480, 640, 2 * size);
        assert!(!l.prunable);
        let l = TileLayout::new([Some(0), None, None, None], 640, 480, 512, size);
        assert!(!l.prunable, "wider than a row");
    }

    #[test]
    fn words_outside_the_picture_are_pinned() {
        let l = layout();
        let row = |row: u16, x: u32| Word { row, at: row as u32 * 640 + x, value: 0 };
        assert_eq!(words(&l, &[row(0, 0)]), write(0, 1));
        assert!(words(&l, &[row(480, 0)]).pinned);
        let aux = Word { row: 1, at: 2 * 640 * 480 + 640 + 639, value: 0 };
        assert_eq!(words(&l, &[aux]), write(AUX, 1 << 7));
    }
}

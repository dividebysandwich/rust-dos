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
//! (up to three colour buffers, the auxiliary buffer) is 8x8 tiles over
//! the picture. A job writes and reads the tiles its screen rectangle
//! touches, and a fastfill kills the tiles it covers whole, as does a
//! clear of the depth for the tiles of a colour buffer the triangles
//! after it draw over (`Cleared`). A job that could touch memory outside
//! the buffers' pictures (no clipping, a clip rectangle bigger than the
//! picture, rows that wrap) is kept, and taken to read everything.

use super::mem::Vram;
use super::mirror::{pixel_at, span};
use super::raster::{RasterState, Stride, TmuRaster, screen_y};
use super::setup::render_triangle;
use super::workers::{Job, Word};

/// Tiles per buffer side.
const TILES: u32 = 8;
const ALL: u64 = u64::MAX;
/// The auxiliary buffer's slot.
const AUX: usize = 3;

/// Where the buffers are, as the tiles see them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileLayout {
    /// Word offsets of up to three colour buffers and the auxiliary
    /// buffer.
    pub bases: [Option<usize>; 4],
    pub width: u32,
    pub height: u32,
    /// The rows of the colour buffers and of the auxiliary buffer.
    pub stride: Stride,
    pub aux_stride: Stride,
    /// Whether the buffers are apart, so that tiles of one are not tiles
    /// of another. Without, nothing is dropped.
    pub prunable: bool,
}

impl TileLayout {
    /// The buffers at `bases` (word offsets) in `words` of frame buffer,
    /// `width` by `height` pixels in rows of `stride` (`aux_stride` for
    /// the auxiliary buffer).
    pub fn new(
        bases: [Option<usize>; 4],
        (width, height): (u32, u32),
        stride: Stride,
        aux_stride: Stride,
        words: usize,
    ) -> Self {
        let mut prunable = width > 0 && height > 0;
        let mut spans = [(0, 0); 4];
        for (slot, base) in bases.iter().enumerate() {
            let Some(base) = *base else { continue };
            let stride = if slot == AUX { aux_stride } else { stride };
            prunable &= width as usize <= stride.pixels();
            spans[slot] = (base, base + span(stride, height));
            prunable &= spans[slot].1 <= words;
        }
        for (i, a) in bases.iter().enumerate() {
            for (j, b) in bases.iter().enumerate().skip(i + 1) {
                let ((a0, a1), (b0, b1)) = (spans[i], spans[j]);
                if a.is_some() && b.is_some() && a0 < b1 && b0 < a1 {
                    prunable = false;
                }
            }
        }
        Self {
            bases,
            width,
            height,
            stride,
            aux_stride,
            prunable,
        }
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
    /// It writes memory outside the tiles but reads none: kept, and what
    /// it does to the tiles counts as for any other job.
    pub kept: bool,
    /// A triangle drawing with a colour buffer's pixels as its texture.
    pub textured: bool,
}

impl Access {
    const PINNED: Access = Access {
        reads: [0; 4],
        writes: [0; 4],
        kills: [0; 4],
        pinned: true,
        kept: false,
        textured: false,
    };

    fn add(&mut self, other: &Access) {
        for r in 0..4 {
            self.reads[r] |= other.reads[r];
            self.writes[r] |= other.writes[r];
            self.kills[r] |= other.kills[r];
        }
        self.pinned |= other.pinned;
        self.kept |= other.kept;
        self.textured |= other.textured;
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
    // A Banshee's rows don't wrap around: those above the origin aren't
    // drawn.
    (st.ymask != 0x3FF || r0 >= 0 && r1 <= 0x400).then_some((r0, r1))
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
    let (_, clip_right, _, clip_bottom) = st.clip();
    if st.fbz_mode & 1 != 0 {
        reach = reach.min(clip_right);
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
    if elsewhere(layout, st) {
        return Access::PINNED;
    }
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
        let (left, right, low, high) = st.clip();
        x0 = x0.max(left);
        x1 = x1.min(right);
        r0 = r0.max(low);
        r1 = r1.min(high);
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
    let textures = textures_read(layout, st);
    for (slot, reads) in access.reads.iter_mut().enumerate() {
        if textures & 1 << slot != 0 {
            *reads = ALL;
        }
    }
    access.textured = textures & 7 != 0;
    access
}

/// Whether `st` draws with rows or an auxiliary buffer other than the
/// layout's, so that its pixels aren't where the tiles say.
fn elsewhere(layout: &TileLayout, st: &RasterState) -> bool {
    let aux = st.aux.is_some()
        && (st.aux != layout.bases[AUX] || st.aux_stride != layout.aux_stride);
    st.stride != layout.stride || aux
}

/// The buffers (a bit for each slot) whose memory the texture units of
/// `st` may read texels from. A Banshee's units read the frame buffer's
/// memory, so a triangle can draw with a colour buffer's pixels: what
/// was drawn there before it has to stay.
fn textures_read(layout: &TileLayout, st: &RasterState) -> u8 {
    let mut slots = 0;
    for t in st.tmu.iter().flatten() {
        if t.lodmin >= 8 << 8 || !t.ram.same(&st.fb) {
            continue;
        }
        let (start, end) = texture_bytes(t);
        for (slot, base) in layout.bases.iter().enumerate() {
            let Some(base) = *base else { continue };
            let stride = if slot == AUX { layout.aux_stride } else { layout.stride };
            let first = 2 * base as u64;
            let last = 2 * (base + span(stride, layout.height)) as u64;
            if start < last && first < end {
                slots |= 1 << slot;
            }
        }
    }
    slots
}

/// The bytes the texture `t` may take texels from, first to past the
/// last: the levels its LOD range picks, or all of the memory where they
/// wrap around. The rasterizer clamps the LOD to lodmin..lodmax, and a
/// level the texture hasn't takes the next (`texture`). The levels
/// outside the range aren't read: Glide points texBaseAddr below a
/// texture whose largest level is smaller than 256x256, so those lie in
/// other memory, a colour buffer's among it.
fn texture_bytes(t: &TmuRaster) -> (u64, u64) {
    let wide = ((t.mode >> 8) & 0xF >= 8) as u32;
    let memory = t.mask as u64 + 1;
    let first = (t.lodmin.min(t.lodmax) >> 8).clamp(0, 8) as usize;
    let last = ((t.lodmax >> 8) + 1).clamp(0, 8) as usize;
    let (mut start, mut end) = (memory, 0);
    for (level, &base) in t.lodoffset.iter().enumerate().take(last + 1).skip(first) {
        let texels = ((t.wmask >> level) + 1) as u64 * ((t.hmask >> level) + 1) as u64;
        let last = base as u64 + (texels << wide);
        if last > memory {
            return (0, memory);
        }
        start = start.min(base as u64);
        end = end.max(last);
    }
    (start, end)
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
    // A fill of the depth alone draws into no colour buffer.
    let slot = match layout.slot(st.dest) {
        Some(slot) => slot,
        None if !rgb => AUX,
        None => return Access::PINNED,
    };
    if elsewhere(layout, st) {
        return Access::PINNED;
    }
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

/// Rows `top..bottom` of the buffer at word `base` filled whole, every
/// word of them. With rows wider than the picture, the words right of it
/// are outside the tiles: the job is kept, but as it reads nothing, it
/// still hides what was drawn in the tiles before it.
pub fn rows(layout: &TileLayout, base: usize, (top, bottom): (u32, u32)) -> Access {
    let slot = layout.bases.iter().position(|&b| b == Some(base));
    let Some(slot) = slot.filter(|_| layout.prunable) else {
        return Access::PINNED;
    };
    let stride = if slot == AUX { layout.aux_stride } else { layout.stride };
    if bottom > layout.height || stride.tiled {
        return Access::PINNED;
    }
    let rect = (0, layout.width, top, bottom);
    let mut access = Access::default();
    access.writes[slot] = layout.tiles(rect, false);
    access.kills[slot] = layout.tiles(rect, true);
    access.kept = (layout.width as usize) < stride.pixels();
    access
}

/// Frame buffer words written.
pub fn words(layout: &TileLayout, words: &[Word]) -> Access {
    if !layout.prunable {
        return Access::PINNED;
    }
    let mut access = Access::default();
    'word: for word in words {
        for (slot, base) in layout.bases.iter().enumerate() {
            let Some(at) = base.and_then(|b| (word.at as usize).checked_sub(b)) else {
                continue;
            };
            let stride = match slot {
                AUX => layout.aux_stride,
                _ => layout.stride,
            };
            let Some((x, y)) = pixel_at(stride, at) else {
                continue;
            };
            if x < layout.width && y < layout.height {
                access.writes[slot] |= layout.tiles((x, x + 1, y, y + 1), false);
                continue 'word;
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

/// The jobs held back, oldest first, each with its number.
#[derive(Default)]
pub struct Backlog {
    pub layout: TileLayout,
    jobs: Vec<(Job, Access, u64)>,
    words: usize,
    /// Which jobs pruning keeps, held on to between prunings.
    keep: Vec<bool>,
    /// Jobs pushed since the last pruning.
    since_prune: usize,
    /// The last job's number.
    numbered: u64,
    /// What the triangles drew over since the depth was last cleared.
    cleared: Option<Cleared>,
    /// For each colour buffer, the columns the jobs held may draw into
    /// are left of this one.
    reach: [u32; 3],
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
        self.numbered += 1;
        self.reach(&job, &access);
        self.track(&job, &access);
        let kills = access.kills.iter().any(|&k| k != 0);
        self.jobs.push((job, access, self.numbered));
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
        if matches!(self.jobs.last(), Some((Job::Pixels { .. }, ..))) {
            let (mut job, mut last, n) = self.jobs.pop().expect("a last job");
            self.reach(&job, &access);
            self.track(&job, &access);
            if let Job::Pixels { words, .. } = &mut job {
                words.extend_from_slice(new);
            }
            last.add(&access);
            self.jobs.push((job, last, n));
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
        let out: Vec<Job> = self.jobs.drain(..older).map(|(job, ..)| job).collect();
        self.count_words();
        out
    }

    fn count_words(&mut self) {
        self.words = self
            .jobs
            .iter()
            .map(|(job, ..)| match job {
                Job::Pixels { words, .. } => words.len(),
                _ => 0,
            })
            .sum();
    }

    /// Drop the jobs whose pixels nothing can see any more: everything is
    /// visible after the last job, and going back, a job's writes are
    /// visible where the tiles are and not killed by a later fill. A job
    /// dropped still kills its tiles: the tiles of a colour buffer a
    /// depth clear says are drawn over (`Cleared`) are drawn over by the
    /// jobs after it, whether the clear stays or not.
    pub fn prune(&mut self) {
        self.since_prune = 0;
        let mut live = [ALL; 4];
        self.keep.clear();
        self.keep.resize(self.jobs.len(), true);
        for (i, (_, a, _)) in self.jobs.iter().enumerate().rev() {
            if a.pinned {
                live = [ALL; 4];
            } else if !a.kept && (0..4).all(|r| a.writes[r] & live[r] == 0) {
                self.keep[i] = false;
                for (r, live) in live.iter_mut().enumerate() {
                    *live &= !a.kills[r];
                }
            } else {
                for (r, live) in live.iter_mut().enumerate() {
                    *live = (*live & !a.kills[r]) | a.reads[r];
                }
            }
        }
        let before = self.jobs.len();
        let mut k = self.keep.iter().copied();
        self.jobs.retain(|_| k.next().unwrap_or(true));
        self.pruned += (before - self.jobs.len()) as u64;
        self.count_words();
    }

    /// Everything left, pruned, oldest first.
    pub fn take(&mut self) -> Vec<Job> {
        self.prune();
        self.words = 0;
        self.cleared = None;
        self.reach = [0; 3];
        self.jobs.drain(..).map(|(job, ..)| job).collect()
    }

    /// Widen `reach` to the columns `job` may draw into. A triangle with
    /// clipping draws left of the clip rectangle's right edge less one
    /// (`raster::scanline`), so a game clipping at the picture's width
    /// never draws its last column.
    fn reach(&mut self, job: &Job, access: &Access) {
        let width = self.layout.width as i32;
        for (slot, reach) in self.reach.iter_mut().enumerate() {
            if access.writes[slot] == 0 && !access.pinned {
                continue;
            }
            let right = match job {
                _ if access.pinned => width,
                Job::Triangle { st, .. } if st.fbz_mode & 1 != 0 => st.clip().1 - 1,
                Job::Fastfill { x1, .. } => *x1,
                _ => width,
            };
            *reach = (*reach).max(right.clamp(0, width) as u32);
        }
    }

    /// Follow what `job`, about to be held as number `numbered`, draws
    /// over after a depth clear (`Cleared`), and give the clear's job the
    /// tiles of a colour buffer drawn over whole since.
    fn track(&mut self, job: &Job, access: &Access) {
        match job {
            Job::Fastfill { st, .. } if clears_depth(st, access) => {
                self.cleared = Some(Cleared::new(self.numbered, &self.layout));
                return;
            }
            // Drawing with a colour buffer's pixels as its texture, it
            // reads what was there before the clear wherever no triangle
            // has drawn over it yet.
            Job::Triangle { .. } if access.textured => {}
            Job::Triangle { st, verts, .. } if !access.pinned => {
                let Some(cleared) = &mut self.cleared else {
                    return;
                };
                match cleared.cover(&self.layout, st, verts, &self.reach) {
                    Some((_, 0)) => return,
                    Some((slot, tiles)) => {
                        let fill = cleared.fill;
                        let held = self.jobs.iter_mut().rev().find(|(.., n)| *n == fill);
                        if let Some((_, clear, _)) = held {
                            clear.kills[slot] |= tiles;
                            return;
                        }
                    }
                    None => {}
                }
            }
            _ if !access.pinned && access.writes[AUX] == 0 => return,
            _ => {}
        }
        // The depth may no longer be the cleared one where later
        // triangles test it, or the clear went to the workers.
        self.cleared = None;
    }
}

/// Whether a fastfill fills the whole auxiliary buffer with the farthest
/// depth, FFFFh, which every depth value is less than or equal to.
fn clears_depth(st: &RasterState, access: &Access) -> bool {
    let fbz = st.fbz_mode;
    let depth = fbz & (1 << 10) != 0 && fbz & (1 << 18) == 0 && st.aux.is_some();
    depth && access.kills[AUX] == ALL && st.za_color as u16 == 0xFFFF
}

/// What the triangles drew over since a fastfill cleared the depth
/// (`clears_depth`). A program that draws over the whole picture every
/// frame needs no colour clear, and none of its fills kill the tiles of
/// the colour buffers. But a triangle that writes its colour whatever is
/// there (no blending, alpha or chroma test, stipple or alpha planes) and
/// whose depth test is "always" or "less than or equal" draws every pixel
/// it covers while the depth there is still the cleared one. The colour
/// such a pixel ends up with doesn't depend on what it was before the
/// clear. Where a triangle may have written the depth first, a later one
/// may fail its test, and only the pixels it covers with the depth still
/// cleared count. Once a tile of a colour buffer is drawn over whole, the
/// clear's job kills it, and the jobs before the clear that drew there
/// can go.
///
/// The pixels a triangle covers are the ones the rasterizer draws
/// (`render_triangle`, then the clipping `raster::scanline` does). Any
/// other job that writes the depth, one that may write outside the
/// tiles, or a triangle whose texture is in a colour buffer's memory (a
/// Banshee's, which reads what was there before the clear) ends the
/// following.
struct Cleared {
    /// The fill's job number.
    fill: u64,
    /// Words of a bitmap row.
    row: usize,
    /// For each colour buffer, the pixels drawn over since the clear, a
    /// bit each; empty until a triangle draws into it.
    drawn: [Vec<u64>; 3],
    /// The pixels whose depth a triangle may have written since.
    depth_written: Vec<u64>,
    /// The tiles of each colour buffer found drawn over whole.
    killed: [u64; 3],
}

impl Cleared {
    fn new(fill: u64, layout: &TileLayout) -> Self {
        let row = (layout.width as usize).div_ceil(64);
        Self {
            fill,
            row,
            drawn: Default::default(),
            depth_written: vec![0; row * layout.height as usize],
            killed: [0; 3],
        }
    }

    /// The triangle `verts` drawn with `st`: its colour buffer and the
    /// tiles of it now drawn over whole for the first time, or None if
    /// it can't be followed. A tile is drawn over whole when it is
    /// everywhere left of `reach`, which no job held draws past.
    fn cover(
        &mut self,
        layout: &TileLayout,
        st: &RasterState,
        verts: &[(f32, f32); 3],
        reach: &[u32; 3],
    ) -> Option<(usize, u64)> {
        let slot = layout.slot(st.dest)?;
        let (fbz, am) = (st.fbz_mode, st.alpha_mode);
        let writes_depth = fbz & (1 << 10) != 0 && st.aux.is_some();
        // Chroma key, stipple, alpha mask, alpha planes; alpha test and
        // blending.
        let tested = fbz & (1 << 1 | 1 << 2 | 1 << 13 | 1 << 18) != 0 || am & 0x11 != 0;
        let func = if fbz & (1 << 4) != 0 { (fbz >> 5) & 7 } else { 7 };
        // Whether it draws over each pixel it covers, and whether only
        // where the depth is still cleared. Without an auxiliary buffer
        // every depth test but "never" passes (`Row::begin`).
        let over = match func {
            _ if fbz & (1 << 9) == 0 || tested => None,
            7 => Some(false),
            1..=6 if st.aux.is_none() => Some(false),
            3 => Some(true),
            _ => None,
        };
        if over.is_none() && !writes_depth {
            return Some((slot, 0));
        }
        let (w, h) = (layout.width as i32, layout.height as i32);
        let row = self.row;
        if over.is_some() && self.drawn[slot].is_empty() {
            self.drawn[slot] = vec![0; row * h as usize];
        }
        let flip = fbz & (1 << 17) != 0;
        let clip = (fbz & 1 != 0).then(|| st.clip());
        let (mut left_most, mut right_most, mut top, mut bottom) = (w, 0, h, 0);
        let (drawn, depth_written) = (&mut self.drawn[slot], &mut self.depth_written);
        render_triangle(*verts, |y, mut x0, mut x1| {
            let scry = screen_y(st, y, flip);
            if let Some((left, right, low, high)) = clip {
                if scry < low || scry >= high {
                    return;
                }
                x0 = x0.max(left);
                if x1 >= right {
                    x1 = right - 1;
                }
            }
            let (x0, x1) = (x0.max(0), x1.min(w));
            if scry < 0 || scry >= h || x0 >= x1 {
                return;
            }
            left_most = left_most.min(x0);
            right_most = right_most.max(x1);
            top = top.min(scry);
            bottom = bottom.max(scry + 1);
            let at = scry as usize * row;
            for (i, bits) in span_words(x0 as usize, x1 as usize) {
                let i = at + i;
                match over {
                    Some(false) => drawn[i] |= bits,
                    Some(true) => drawn[i] |= bits & !depth_written[i],
                    None => {}
                }
                if writes_depth {
                    depth_written[i] |= bits;
                }
            }
        });
        if over.is_none() || left_most >= right_most {
            return Some((slot, 0));
        }
        let rect = (
            left_most as u32,
            right_most as u32,
            top as u32,
            bottom as u32,
        );
        let mut killed = 0;
        let mut candidates = layout.tiles(rect, false) & !self.killed[slot];
        while candidates != 0 {
            let tile = candidates.trailing_zeros();
            candidates &= candidates - 1;
            let (tx, ty) = (tile % TILES, tile / TILES);
            let (edge, w, h) = (TileLayout::edge, layout.width, layout.height);
            let (c0, c1) = (edge(tx, w), edge(tx + 1, w).min(reach[slot]));
            let (r0, r1) = (edge(ty, h), edge(ty + 1, h));
            let whole = c0 >= c1
                || (r0..r1).all(|y| {
                    let at = y as usize * row;
                    span_words(c0 as usize, c1 as usize)
                        .all(|(i, bits)| drawn[at + i] & bits == bits)
                });
            if whole {
                killed |= 1 << tile;
            }
        }
        self.killed[slot] |= killed;
        Some((slot, killed))
    }
}

/// The words of a bitmap row that pixels `x0..x1` are in, and their bits.
fn span_words(x0: usize, x1: usize) -> impl Iterator<Item = (usize, u64)> {
    (x0 / 64..x1.div_ceil(64)).map(move |i| {
        let (lo, hi) = (x0.max(i * 64) - i * 64, x1.min(i * 64 + 64) - i * 64);
        let bits = if hi - lo == 64 {
            u64::MAX
        } else {
            ((1u64 << (hi - lo)) - 1) << lo
        };
        (i, bits)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 640x480 with two colour buffers and an auxiliary buffer.
    fn layout() -> TileLayout {
        let size = 640 * 480;
        let bases = [Some(0), Some(size), None, Some(2 * size)];
        let stride = Stride::linear(640);
        TileLayout::new(bases, (640, 480), stride, stride, 3 * size)
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
            let words = Vec::new();
            b.jobs.push((Job::Pixels { fb: Vram::new(16), words }, *a, 0));
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
        b.prune();
        assert_eq!(b.len(), 3, "pruning again must retain the surviving jobs");
        assert_eq!(b.pruned, 1);
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
        let rows = Stride::linear(640);
        let bases = [Some(0), Some(size / 2), None, None];
        let l = TileLayout::new(bases, (640, 480), rows, rows, 2 * size);
        assert!(!l.prunable);
        let narrow = Stride::linear(512);
        let one = [Some(0), None, None, None];
        let l = TileLayout::new(one, (640, 480), narrow, narrow, size);
        assert!(!l.prunable, "wider than a row");
    }

    #[test]
    fn whole_rows_filled_kill_their_tiles() {
        let l = layout();
        assert_eq!(rows(&l, 640 * 480, (0, 480)), fill(1));
        let top = rows(&l, 2 * 640 * 480, (0, 60));
        assert_eq!((top.writes[AUX], top.kills[AUX]), (0xFF, 0xFF));
        assert!(rows(&l, 1234, (0, 480)).pinned, "not a buffer");
        let rows640 = Stride::linear(640);
        let one = [Some(0), None, None, None];
        let wide = TileLayout::new(one, (600, 480), rows640, rows640, 640 * 480);
        let wide = rows(&wide, 0, (0, 480));
        assert!(wide.kept && !wide.pinned, "words right of the picture");
        assert_eq!((wide.writes[0], wide.kills[0]), (ALL, ALL));
    }

    #[test]
    fn kept_jobs_stay_but_hide_what_is_under_them() {
        let mut wide = fill(0);
        wide.kept = true;
        let mut b = backlog(&[write(0, 1), wide, fill(0)]);
        b.prune();
        assert_eq!(b.len(), 2, "the first job goes, the kept one stays");
        assert_eq!(b.jobs[0].1, wide);
    }

    #[test]
    fn words_outside_the_picture_are_pinned() {
        let l = layout();
        let row = |row: u16, x: u32| Word {
            row,
            at: row as u32 * 640 + x,
            value: 0,
        };
        assert_eq!(words(&l, &[row(0, 0)]), write(0, 1));
        // Row 480 of buffer 0 is the memory of buffer 1's first pixel.
        assert_eq!(words(&l, &[row(480, 0)]), write(1, 1));
        assert!(words(&l, &[row(3 * 480, 0)]).pinned);
        let aux = Word {
            row: 1,
            at: 2 * 640 * 480 + 640 + 639,
            value: 0,
        };
        assert_eq!(words(&l, &[aux]), write(AUX, 1 << 7));
        // Rows wider than the picture: the words right of it are pinned.
        let wide = Stride::linear(1024);
        let one = [Some(0), None, None, None];
        let l = TileLayout::new(one, (640, 480), wide, wide, 1024 * 480);
        let word = |at: u32| Word {
            row: 0,
            at,
            value: 0,
        };
        assert!(words(&l, &[word(700)]).pinned);
    }

    #[test]
    fn tiled_buffers_find_their_tiles() {
        // 640x480 in rows of 10 tiles (64x32 pixels each), the auxiliary
        // buffer right after the colour buffer's 15 rows of tiles.
        let tiles = Stride {
            words: 10 * 2048,
            tiled: true,
        };
        let size = 15 * tiles.words;
        let bases = [Some(0), None, None, Some(size)];
        let l = TileLayout::new(bases, (640, 480), tiles, tiles, 2 * size);
        assert!(l.prunable);
        // Pixel (639, 479): the last word of the last tile.
        let last = Word {
            row: 479,
            at: size as u32 - 1,
            value: 0,
        };
        assert_eq!(words(&l, &[last]), write(0, 1 << 63));
        // Pixel (65, 33) of the auxiliary buffer.
        let at = size + tiles.words + 2048 + 64 + 1;
        let aux = Word {
            row: 33,
            at: at as u32,
            value: 0,
        };
        assert_eq!(words(&l, &[aux]), write(AUX, 1));
        let bases = [Some(0), Some(size - 1), None, None];
        let overlapping = TileLayout::new(bases, (640, 480), tiles, tiles, 2 * size);
        assert!(!overlapping.prunable, "tiles span their rows");
    }
}

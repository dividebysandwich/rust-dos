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
//! (up to three colour buffers, the auxiliary buffer) is 32x32 tiles over
//! the picture. A job writes and reads the rectangle of tiles its screen
//! rectangle touches, and a fastfill kills the tiles it covers whole, as
//! do the triangles of a frame for the tiles of a colour buffer they draw
//! over whole (`Cleared`). A job that could touch memory outside the
//! buffers' pictures (no clipping, a clip rectangle bigger than the
//! picture, rows that wrap) is kept, and taken to read everything.

use super::mem::Vram;
use super::mirror::{pixel_at, span};
use super::raster::{RasterState, Stride, TmuRaster, screen_y};
use super::setup::render_triangle;
use super::workers::{Job, Word};
use std::collections::{HashMap, VecDeque};

/// Tiles per buffer side.
const TILES: u32 = 32;
/// The auxiliary buffer's slot.
const AUX: usize = 3;

/// A rectangle of tiles: columns `x0..x1`, rows `y0..y1`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    x0: u8,
    x1: u8,
    y0: u8,
    y1: u8,
}

impl Rect {
    const EMPTY: Rect = Rect {
        x0: 0,
        x1: 0,
        y0: 0,
        y1: 0,
    };
    #[cfg(test)]
    const FULL: Rect = Rect {
        x0: 0,
        x1: TILES as u8,
        y0: 0,
        y1: TILES as u8,
    };

    fn new(x0: u32, x1: u32, y0: u32, y1: u32) -> Self {
        if x0 >= x1 || y0 >= y1 {
            return Rect::EMPTY;
        }
        let [x0, x1, y0, y1] = [x0, x1, y0, y1].map(|v| v.min(TILES) as u8);
        Rect { x0, x1, y0, y1 }
    }

    pub fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    /// The smallest rectangle around both.
    fn union(self, other: Rect) -> Rect {
        match (self.is_empty(), other.is_empty()) {
            (true, _) => other,
            (_, true) => self,
            _ => Rect {
                x0: self.x0.min(other.x0),
                x1: self.x1.max(other.x1),
                y0: self.y0.min(other.y0),
                y1: self.y1.max(other.y1),
            },
        }
    }

    /// Its columns as bits of a row of `Tiles`.
    fn columns(&self) -> u32 {
        let ones = |n: u8| {
            if n as u32 >= TILES {
                u32::MAX
            } else {
                (1u32 << n) - 1
            }
        };
        ones(self.x1) & !ones(self.x0)
    }

    fn rows(&self) -> std::ops::Range<usize> {
        self.y0 as usize..self.y1 as usize
    }
}

/// What the triangles of a frame drew over in each colour buffer: the
/// tiles drawn over whole, and once the frame is over, the pixels (rows
/// of `row` words, a bit each; empty where it drew over none).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Covered {
    tiles: [Tiles; 3],
    pixels: [Vec<u64>; 3],
    row: usize,
}

/// A set of tiles: a row of bits for each row of tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tiles([u32; TILES as usize]);

impl Default for Tiles {
    fn default() -> Self {
        Tiles::NONE
    }
}

impl Tiles {
    const NONE: Tiles = Tiles([0; TILES as usize]);
    const ALL: Tiles = Tiles([u32::MAX; TILES as usize]);

    fn remove(&mut self, rect: Rect) {
        if rect.is_empty() {
            return;
        }
        let columns = rect.columns();
        for row in &mut self.0[rect.rows()] {
            *row &= !columns;
        }
    }

    /// Whether any of the tiles of `rect` is in the set.
    fn meets(&self, rect: Rect) -> bool {
        if rect.is_empty() {
            return false;
        }
        let columns = rect.columns();
        self.0[rect.rows()].iter().any(|row| row & columns != 0)
    }

    fn contains(&self, x: u32, y: u32) -> bool {
        self.0[y as usize] & 1 << x != 0
    }

    fn insert(&mut self, x: u32, y: u32) {
        self.0[y as usize] |= 1 << x;
    }

    fn and_not(&mut self, other: &Tiles) {
        for (row, o) in self.0.iter_mut().zip(&other.0) {
            *row &= !o;
        }
    }

    fn or(&mut self, other: &Tiles) {
        for (row, o) in self.0.iter_mut().zip(&other.0) {
            *row |= o;
        }
    }

    /// The tiles of `rect` in both.
    fn and_rect(&self, rect: Rect) -> Tiles {
        let mut t = Tiles::NONE;
        if !rect.is_empty() {
            let columns = rect.columns();
            for y in rect.rows() {
                t.0[y] = self.0[y] & columns;
            }
        }
        t
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&row| row == 0)
    }
}

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
    fn tiles(&self, (x0, x1, y0, y1): (u32, u32, u32, u32), whole: bool) -> Rect {
        if x0 >= x1 || y0 >= y1 {
            return Rect::default();
        }
        let (cols, rows) = (
            Self::range(x0, x1, self.width, whole),
            Self::range(y0, y1, self.height, whole),
        );
        match (cols, rows) {
            (Some((tx0, tx1)), Some((ty0, ty1))) => Rect::new(tx0, tx1, ty0, ty1),
            _ => Rect::default(),
        }
    }

    /// The tiles along a side of `size` pixels that pixels `p0..p1` touch,
    /// or cover whole, first to past the last.
    fn range(p0: u32, p1: u32, size: u32, whole: bool) -> Option<(u32, u32)> {
        let mut found = None;
        for k in 0..TILES {
            let (c0, c1) = (Self::edge(k, size), Self::edge(k + 1, size));
            let hit = c0 < c1
                && if whole {
                    p0 <= c0 && c1 <= p1
                } else {
                    p0 < c1 && c0 < p1
                };
            if hit {
                found = Some(found.map_or((k, k + 1), |(first, _)| (first, k + 1)));
            }
        }
        found
    }

    /// The tiles of the whole picture.
    fn all(&self) -> Rect {
        self.tiles((0, self.width, 0, self.height), true)
    }
}

/// What a job does to the buffers' tiles.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Access {
    /// Tiles whose pixels it reads where it writes them (the depth test,
    /// blending): what was there matters only where what it writes
    /// stays visible.
    pub reads: [Rect; 4],
    pub writes: [Rect; 4],
    /// Tiles written whole: what was there before is gone.
    pub kills: [Rect; 4],
    /// What the jobs after it drew over, when it began a frame
    /// (`Cleared`): what was there before is gone.
    pub covered: Option<Box<Covered>>,
    /// It may touch memory outside the tiles: kept, and taken to read
    /// everything.
    pub pinned: bool,
    /// It writes memory outside the tiles but reads none: kept, and what
    /// it does to the tiles counts as for any other job.
    pub kept: bool,
    /// The buffers (a bit for each slot) a triangle takes texels from,
    /// anywhere in them.
    pub sampled: u8,
}

impl Access {
    const PINNED: Access = Access {
        reads: [Rect::EMPTY; 4],
        writes: [Rect::EMPTY; 4],
        kills: [Rect::EMPTY; 4],
        covered: None,
        pinned: true,
        kept: false,
        sampled: 0,
    };

    fn add(&mut self, other: &Access) {
        // Only frame buffer writes add up, and they kill nothing.
        for r in 0..4 {
            self.reads[r] = self.reads[r].union(other.reads[r]);
            self.writes[r] = self.writes[r].union(other.writes[r]);
        }
        self.pinned |= other.pinned;
        self.kept |= other.kept;
        self.sampled |= other.sampled;
    }

    /// A triangle drawing with a colour buffer's pixels as its texture.
    fn textured(&self) -> bool {
        self.sampled & 7 != 0
    }

    fn kills_any(&self) -> bool {
        self.kills.iter().any(|k| !k.is_empty()) || self.covered.is_some()
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
    access.sampled = textures_read(layout, st);
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
    for (start, end) in frame_buffer_texels(st) {
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

/// The texture units of `st` that take texels from the frame buffer's
/// memory, each with the bytes it takes them from. A unit whose lodmin
/// is 8 or more takes no texels (`raster`).
fn frame_buffer_units(st: &RasterState) -> impl Iterator<Item = (usize, (u64, u64))> + '_ {
    st.tmu.iter().enumerate().filter_map(|(unit, t)| {
        let t = t
            .as_ref()
            .filter(|t| t.lodmin < 8 << 8 && t.ram.same(&st.fb))?;
        Some((unit, texture_bytes(t)))
    })
}

/// The bytes of the frame buffer's memory the texture units of `st`
/// take texels from, a range for each unit that reads there.
fn frame_buffer_texels(st: &RasterState) -> impl Iterator<Item = (u64, u64)> + '_ {
    frame_buffer_units(st).map(|(_, range)| range)
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

/// The card's memory the jobs given out or held back may read or write,
/// for a write by the CPU that needn't wait for them when it is
/// elsewhere (`Voodoo::banshee_texture_download`): the bytes their
/// texture units take texels from in the frame buffer's memory, the
/// words texture data queued goes into (`Job::Bytes`), and the lowest
/// byte of a buffer they draw into or of a word they write. A buffer is
/// taken to reach from there to the end of the memory, which Glide's
/// buffers on a Banshee do, above its textures.
#[derive(Clone, Debug)]
pub struct Footprint {
    textures: Vec<(u64, u64)>,
    writes_from: u64,
}

impl Default for Footprint {
    fn default() -> Self {
        Self {
            textures: Vec::new(),
            writes_from: u64::MAX,
        }
    }
}

/// Texture ranges kept apart; past them, the nearest range takes in a
/// new one.
const FOOTPRINT_TEXTURES: usize = 64;

/// The most texture memory `Backlog::spare` copies for a texture.
const SPARE_BYTES: u64 = 4 << 20;

impl Footprint {
    /// The jobs are all done: they reach nothing.
    pub fn clear(&mut self) {
        self.textures.clear();
        self.writes_from = u64::MAX;
    }

    /// What `job` may read and write.
    pub fn add(&mut self, job: &Job) {
        self.add_writes(job);
        match job {
            Job::Triangle { st, .. } => {
                for range in frame_buffer_texels(st) {
                    self.texture(range);
                }
            }
            // Later data over these bytes has to come after them, and so
            // does data in the same words: a worker writes half a word by
            // reading it first.
            Job::Bytes { at, bytes, .. } => {
                let (start, end) = (*at as u64, (*at + bytes.len()) as u64);
                self.texture((start & !1, end.next_multiple_of(2)));
            }
            _ => {}
        }
    }

    /// What `job` may write.
    pub fn add_writes(&mut self, job: &Job) {
        match job {
            Job::Triangle { st, .. } | Job::Fastfill { st, .. } => {
                for base in std::iter::once(st.dest).chain(st.aux) {
                    self.writes_from = self.writes_from.min(2 * base as u64);
                }
            }
            Job::Pixels { words, .. } => self.words(words),
            Job::Rows { base, .. } => self.writes_from = self.writes_from.min(2 * *base as u64),
            // They don't reach to the end of the memory: see `add`.
            Job::Bytes { .. } => {}
            #[cfg(test)]
            Job::Panic | Job::Hold(_) => {}
        }
    }

    /// Frame buffer words to be written.
    pub fn words(&mut self, words: &[Word]) {
        if let Some(low) = words.iter().map(|w| w.at).min() {
            self.writes_from = self.writes_from.min(2 * low as u64);
        }
    }

    fn texture(&mut self, (start, end): (u64, u64)) {
        if start >= end || self.textures.iter().any(|&(a, b)| a <= start && end <= b) {
            return;
        }
        if self.textures.len() < FOOTPRINT_TEXTURES {
            self.textures.push((start, end));
            return;
        }
        // The range nearest to it takes it in.
        let gap = |&(a, b): &(u64, u64)| start.saturating_sub(b).max(a.saturating_sub(end));
        if let Some(near) = self.textures.iter_mut().min_by_key(|r| gap(r)) {
            *near = (near.0.min(start), near.1.max(end));
        }
    }

    /// Whether the jobs may write below byte `end`.
    pub fn writes(&self, end: u64) -> bool {
        end > self.writes_from
    }

    /// Whether bytes `start..end` are memory the jobs may read or write.
    pub fn touches(&self, start: u64, end: u64) -> bool {
        self.writes(end) || self.textures.iter().any(|&(a, b)| start < b && a < end)
    }

    /// Add what the jobs of `other` may read and write.
    pub fn merge(&mut self, other: &Footprint) {
        self.writes_from = self.writes_from.min(other.writes_from);
        for &range in &other.textures {
            self.texture(range);
        }
    }
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
    // Words side by side in a row go in as one run: finding a run's tiles
    // costs as much as a word's.
    let mut run: Option<(usize, u32, u32, u32)> = None;
    let add = |access: &mut Access, (slot, x0, x1, y): (usize, u32, u32, u32)| {
        let tiles = layout.tiles((x0, x1, y, y + 1), false);
        access.writes[slot] = access.writes[slot].union(tiles);
    };
    // Each buffer's last word worked out, and its pixel: the word after
    // it in a linear buffer is the next pixel, without a division.
    let mut last: [Option<(usize, u32, u32)>; 4] = [None; 4];
    'word: for word in words {
        for (slot, base) in layout.bases.iter().enumerate() {
            let Some(at) = base.and_then(|b| (word.at as usize).checked_sub(b)) else {
                continue;
            };
            let stride = match slot {
                AUX => layout.aux_stride,
                _ => layout.stride,
            };
            let (x, y) = match last[slot] {
                Some((prev, x, y)) if !stride.tiled && at == prev + 1 => {
                    if x as usize + 1 < stride.words {
                        (x + 1, y)
                    } else {
                        (0, y + 1)
                    }
                }
                _ => match pixel_at(stride, at) {
                    Some(p) => p,
                    None => continue,
                },
            };
            last[slot] = Some((at, x, y));
            if x < layout.width && y < layout.height {
                match &mut run {
                    Some((s, _, x1, ry)) if *s == slot && *ry == y && *x1 == x => *x1 = x + 1,
                    _ => {
                        if let Some(done) = run.replace((slot, x, x + 1, y)) {
                            add(&mut access, done);
                        }
                    }
                }
                continue 'word;
            }
        }
        return Access::PINNED;
    }
    if let Some(done) = run {
        add(&mut access, done);
    }
    access
}

/// Jobs a backlog holds before it prunes itself and, if still too many,
/// hands the oldest on.
const MAX_JOBS: usize = 65_536;
/// Frame buffer words it holds at most.
const MAX_WORDS: usize = 1 << 20;
/// Jobs held that keep the pixels the triangles after them drew over
/// (`Covered`), a bitmap of the picture each, at most: the newest ones,
/// which the frames before them lie under.
const PIXEL_MARKS: usize = 16;

/// The textures held triangles take texels from in the frame buffer's
/// memory (`texture_bytes`): each range of bytes, with the numbers of the
/// jobs that read it and through which texture unit.
#[derive(Default)]
struct Readers {
    ranges: Vec<Reader>,
    index: HashMap<(u64, u64), usize>,
}

type Reader = ((u64, u64), Vec<(u64, usize)>);

impl Readers {
    fn clear(&mut self) {
        self.ranges.clear();
        self.index.clear();
    }

    /// The texture units of triangle `job`, numbered `n`, that read the
    /// frame buffer's memory.
    fn add(&mut self, job: &Job, n: u64) {
        let Job::Triangle { st, .. } = job else {
            return;
        };
        for (unit, range) in frame_buffer_units(st) {
            if range.0 >= range.1 {
                continue;
            }
            let at = *self.index.entry(range).or_insert_with(|| {
                self.ranges.push((range, Vec::new()));
                self.ranges.len() - 1
            });
            self.ranges[at].1.push((n, unit));
        }
    }

    fn touches(&self, start: u64, end: u64) -> bool {
        self.ranges.iter().any(|&((a, b), _)| start < b && a < end)
    }

    /// Take out the ranges that overlap bytes `start..end`.
    fn take(&mut self, start: u64, end: u64) -> Vec<Reader> {
        let (hit, kept): (Vec<Reader>, Vec<Reader>) = std::mem::take(&mut self.ranges)
            .into_iter()
            .partition(|&((a, b), _)| start < b && a < end);
        self.ranges = kept;
        self.index = self
            .ranges
            .iter()
            .enumerate()
            .map(|(i, (range, _))| (*range, i))
            .collect();
        hit
    }

    /// Put back ranges taken out.
    fn put_back(&mut self, taken: Vec<Reader>) {
        for (range, readers) in taken {
            self.index.insert(range, self.ranges.len());
            self.ranges.push((range, readers));
        }
    }
}

/// The jobs held back, oldest first, each with what it does to the
/// tiles and its number.
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
    /// What the triangles drew over since the mark (`Cleared`).
    cleared: Option<Cleared>,
    /// The numbers of the jobs given pixels the triangles after them
    /// drew over, oldest first (`PIXEL_MARKS`).
    pixel_marks: VecDeque<u64>,
    /// For each colour buffer, the columns the jobs held may draw into
    /// are left of this one.
    reach: [u32; 3],
    /// The memory the jobs held may write, and the textures they read.
    writes: Footprint,
    readers: Readers,
    /// Jobs dropped, and jobs handed on to be drawn, for the debugger.
    pub pruned: u64,
    pub released: u64,
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
        self.writes.add_writes(&job);
        self.readers.add(&job, self.numbered);
        self.reach(&job, &access);
        let covered = self.track(&job, &access);
        let mut kills = access.kills_any();
        self.jobs.push((job, access, self.numbered));
        if let Some((slot, tiles)) = covered {
            kills |= self.kill_before_mark(slot, tiles);
        }
        self.since_prune += 1;
        // A fill, or triangles drawn over whole tiles, may have made the
        // frames before invisible; prune when enough came since the last
        // time.
        if kills && self.since_prune * 4 >= self.jobs.len() {
            self.prune();
        }
        self.overflow()
    }

    /// Give the job that began what `Cleared` follows the `tiles` of
    /// colour buffer `slot` drawn over whole since. False, and the
    /// following ends, if that job is no longer held.
    fn kill_before_mark(&mut self, slot: usize, tiles: Tiles) -> bool {
        let Some(mark) = self.cleared.as_ref().map(|c| c.mark) else {
            return false;
        };
        match self.held(mark) {
            Some(i) => {
                let access = &mut self.jobs[i].1;
                access.covered.get_or_insert_with(Default::default).tiles[slot].or(&tiles);
                true
            }
            None => {
                self.cleared = None;
                false
            }
        }
    }

    /// The program swapped buffers: a new frame begins, and what it draws
    /// over hides the frame drawn before it into the same buffer. The
    /// following starts again with the next triangle.
    pub fn swapped(&mut self) {
        self.end_frame();
    }

    /// Stop following what the triangles draw over (`Cleared`): the job
    /// that began it gets the pixels they drew over. Past `PIXEL_MARKS`
    /// such jobs, the oldest one's pixels go.
    fn end_frame(&mut self) {
        let Some(mut cleared) = self.cleared.take() else {
            return;
        };
        for drawn in &mut cleared.drawn {
            if drawn.iter().all(|&w| w == 0) {
                *drawn = Vec::new();
            }
        }
        if cleared.drawn.iter().all(|d| d.is_empty()) {
            return;
        }
        let Some(i) = self.held(cleared.mark) else {
            return;
        };
        let covered = self.jobs[i].1.covered.get_or_insert_with(Default::default);
        covered.pixels = cleared.drawn;
        covered.row = cleared.row;
        self.pixel_marks.push_back(cleared.mark);
        while self.pixel_marks.len() > PIXEL_MARKS {
            let Some(old) = self.pixel_marks.pop_front() else {
                break;
            };
            let Some(i) = self.held(old) else {
                continue;
            };
            let access = &mut self.jobs[i].1;
            if let Some(covered) = &mut access.covered {
                covered.pixels = Default::default();
                if covered.tiles.iter().all(Tiles::is_empty) {
                    access.covered = None;
                }
            }
        }
    }

    /// Frame buffer words written, added to the last job if it has
    /// words too.
    pub fn push_words(&mut self, fb: &Vram, new: &[Word]) -> Vec<Job> {
        let access = words(&self.layout, new);
        self.words += new.len();
        if matches!(self.jobs.last(), Some((Job::Pixels { .. }, ..))) {
            self.writes.words(new);
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
        self.released += out.len() as u64;
        self.count_words();
        self.refresh_footprint();
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
    /// visible where the tiles are and not killed by a later fill or
    /// mark (`Cleared`: a depth clear, or a frame's first triangle after
    /// a swap). A triangle is also dropped when the frames after it drew
    /// over every pixel it covers. A job dropped still kills its tiles:
    /// the tiles of a colour buffer a mark says are drawn over are drawn
    /// over by the jobs after it, whether the mark stays or not.
    pub fn prune(&mut self) {
        self.since_prune = 0;
        let mut live = [Tiles::ALL; 4];
        self.keep.clear();
        self.keep.resize(self.jobs.len(), true);
        // The pixels of each colour buffer the frames after drew over,
        // since a job that may read anything.
        let mut over: [Vec<u64>; 3] = Default::default();
        let row = (self.layout.width as usize).div_ceil(64);
        for (i, (job, a, _)) in self.jobs.iter().enumerate().rev() {
            if a.pinned {
                over = Default::default();
                live = [Tiles::ALL; 4];
                continue;
            }
            let mut drop = !a.kept && (0..4).all(|r| !live[r].meets(a.writes[r]));
            // A triangle in tiles still seen may yet draw only pixels the
            // frames after drew over.
            if !drop
                && !a.kept
                && a.writes[AUX].is_empty()
                && let Job::Triangle { st, verts, .. } = job
                && let Some(slot) = self.layout.slot(st.dest)
                && (0..AUX).all(|r| r == slot || a.writes[r].is_empty())
                && !over[slot].is_empty()
            {
                drop = covered_by(&self.layout, st, verts, &over[slot], row);
            }
            // A triangle kept that takes texels from a buffer may read
            // any pixel drawn before it.
            if !drop && a.sampled != 0 {
                over = Default::default();
            }
            // What it reads in place matters where what it writes is
            // seen, in any buffer: the tiles are the same pixels.
            let mut seen = Tiles::NONE;
            if !drop {
                for (r, live) in live.iter().enumerate() {
                    seen.or(&live.and_rect(a.writes[r]));
                }
            }
            for (r, live) in live.iter_mut().enumerate() {
                live.remove(a.kills[r]);
                if let Some(covered) = a.covered.as_ref().filter(|_| r < AUX) {
                    live.and_not(&covered.tiles[r]);
                }
                if drop {
                    continue;
                }
                if a.sampled & 1 << r != 0 {
                    *live = Tiles::ALL;
                } else {
                    live.or(&seen.and_rect(a.reads[r]));
                }
            }
            if drop {
                self.keep[i] = false;
            }
            if let Some(covered) = &a.covered {
                for (over, pixels) in over.iter_mut().zip(&covered.pixels) {
                    if pixels.is_empty() || covered.row != row {
                        continue;
                    }
                    if over.is_empty() {
                        over.clone_from(pixels);
                    } else {
                        over.iter_mut().zip(pixels).for_each(|(o, p)| *o |= p);
                    }
                }
            }
        }
        let before = self.jobs.len();
        let mut k = self.keep.iter().copied();
        self.jobs.retain(|_| k.next().unwrap_or(true));
        if self.jobs.len() < before {
            self.pruned += (before - self.jobs.len()) as u64;
            self.count_words();
            self.refresh_footprint();
        }
    }

    /// The index of the job numbered `n`, if it is held.
    fn held(&self, n: u64) -> Option<usize> {
        self.jobs.binary_search_by_key(&n, |&(.., m)| m).ok()
    }

    /// Work out again what the jobs held reach, now that some are gone.
    fn refresh_footprint(&mut self) {
        self.writes.clear();
        self.readers.clear();
        for (job, _, n) in &self.jobs {
            self.writes.add_writes(job);
            self.readers.add(job, *n);
        }
        let jobs = &self.jobs;
        self.pixel_marks
            .retain(|n| jobs.binary_search_by_key(n, |&(.., m)| m).is_ok());
    }

    /// Everything left, pruned, oldest first, with the memory it may read
    /// and write.
    pub fn take(&mut self) -> (Vec<Job>, Footprint) {
        self.prune();
        self.released += self.jobs.len() as u64;
        self.words = 0;
        self.cleared = None;
        self.pixel_marks.clear();
        self.reach = [0; 3];
        let mut reach = std::mem::take(&mut self.writes);
        for &(range, _) in &self.readers.ranges {
            reach.texture(range);
        }
        self.readers.clear();
        (self.jobs.drain(..).map(|(job, ..)| job).collect(), reach)
    }

    /// Make bytes `start..end` of the frame buffer's memory free to be
    /// written while the jobs are held: the triangles that take texels
    /// from there take them from a copy of their texture's levels as they
    /// are now, in memory of their own. `given_out` is what the jobs the
    /// workers have, which draw before these, may read and write, and
    /// `read_back` gets the bytes about to be copied
    /// (`Voodoo::gpu_sync_quiet`). False if a job draws into those bytes
    /// or into a texture to copy, a job given out reads them, or a
    /// texture is too large to copy (more than 4 MB, or all of the memory
    /// where its levels wrap round): the jobs have to be drawn first.
    pub fn spare(
        &mut self,
        (start, end): (u64, u64),
        given_out: &Footprint,
        mut read_back: impl FnMut(u64, u64),
    ) -> bool {
        if given_out.touches(start, end) || self.writes.writes(end) {
            return false;
        }
        if !self.readers.touches(start, end) {
            return true;
        }
        let hit = self.readers.take(start, end);
        // What is in memory now is what the triangles would read only if
        // no job before them draws into it.
        let writes = &self.writes;
        let copyable = |&((first, last), _): &Reader| {
            last - (first & !1) <= SPARE_BYTES && !writes.writes(last) && !given_out.writes(last)
        };
        if !hit.iter().all(copyable) {
            self.readers.put_back(hit);
            return false;
        }
        for ((first, last), readers) in hit {
            // From a word's start, so that bytes keep their halves.
            let first = first & !1;
            let mut copy: Option<Vram> = None;
            for (n, unit) in readers {
                let Some(i) = self.held(n) else { continue };
                let (Job::Triangle { st, .. }, access, _) = &mut self.jobs[i] else {
                    continue;
                };
                let fb = st.fb.clone();
                let Some(t) = st.tmu[unit].as_mut().filter(|t| t.ram.same(&fb)) else {
                    continue;
                };
                let ram = copy
                    .get_or_insert_with(|| {
                        read_back(first, last);
                        let ram = Vram::new(((last - first) as usize).next_power_of_two().max(2));
                        let words = first as usize / 2..(last as usize).div_ceil(2);
                        for (i, w) in words.enumerate() {
                            ram.set(i, fb.get(w));
                        }
                        ram
                    })
                    .clone();
                t.mask = ram.bytes() as u32 - 1;
                t.ram = ram;
                for base in &mut t.lodoffset {
                    *base = base.wrapping_sub(first as u32);
                }
                // Its texels no longer come from the buffers.
                access.sampled = textures_read(&self.layout, st);
            }
        }
        true
    }

    /// Widen `reach` to the columns `job` may draw into. A triangle with
    /// clipping draws the columns left of the clip rectangle's right edge
    /// (`raster::scanline`).
    fn reach(&mut self, job: &Job, access: &Access) {
        let width = self.layout.width as i32;
        for (slot, reach) in self.reach.iter_mut().enumerate() {
            if access.writes[slot].is_empty() && !access.pinned {
                continue;
            }
            let right = match job {
                _ if access.pinned => width,
                Job::Triangle { st, .. } if st.fbz_mode & 1 != 0 => st.clip().1,
                Job::Fastfill { x1, .. } => *x1,
                _ => width,
            };
            *reach = (*reach).max(right.clamp(0, width) as u32);
        }
    }

    /// Follow what `job`, about to be held as number `numbered`, draws
    /// over (`Cleared`): the tiles of a colour buffer now drawn over whole
    /// for the first time, for the job that began the following to kill.
    fn track(&mut self, job: &Job, access: &Access) -> Option<(usize, Tiles)> {
        match job {
            Job::Fastfill { st, .. } if clears_depth(&self.layout, st, access) => {
                self.end_frame();
                self.cleared = Some(Cleared::new(self.numbered, &self.layout, true));
                return None;
            }
            // Drawing with a colour buffer's pixels as its texture, it
            // reads what was there before wherever no triangle has drawn
            // over it yet.
            Job::Triangle { .. } if access.textured() => {}
            Job::Triangle { st, verts, .. } if !access.pinned => {
                let (numbered, layout) = (self.numbered, &self.layout);
                let cleared = self
                    .cleared
                    .get_or_insert_with(|| Cleared::new(numbered, layout, false));
                match cleared.cover(layout, st, verts, &self.reach) {
                    Some((_, tiles)) if tiles.is_empty() => return None,
                    Some(covered) => return Some(covered),
                    None => {}
                }
            }
            // Only where the depth was cleared does a job that writes it
            // matter: later triangles may then fail their test.
            _ if !access.pinned
                && (access.writes[AUX].is_empty()
                    || !self.cleared.as_ref().is_some_and(|c| c.depth)) =>
            {
                return None;
            }
            _ => {}
        }
        // The jobs before may show through what the triangles drew, or
        // the depth may no longer be the cleared one where later triangles
        // test it.
        self.end_frame();
        None
    }
}

/// Whether a fastfill fills the whole auxiliary buffer with the farthest
/// depth, FFFFh, which every depth value is less than or equal to.
fn clears_depth(layout: &TileLayout, st: &RasterState, access: &Access) -> bool {
    let fbz = st.fbz_mode;
    let depth = fbz & (1 << 10) != 0 && fbz & (1 << 18) == 0 && st.aux.is_some();
    depth && access.kills[AUX] == layout.all() && st.za_color as u16 == 0xFFFF
}

/// What the triangles drew over since a job, the mark: the first
/// triangle of a frame (`Backlog::swapped`), or a fastfill that cleared
/// the depth (`clears_depth`). A program that draws over the whole
/// picture every frame needs no colour clear, and none of its fills kill
/// the tiles of the colour buffers. But a triangle that writes its colour
/// whatever is there (no blending, alpha or chroma test, stipple or alpha
/// planes) and without a depth test draws every pixel it covers, and the
/// colour such a pixel ends up with doesn't depend on what it was before
/// the mark. After a depth clear, so does one whose depth test is "less
/// than or equal" while the depth there is still the cleared one: where a
/// triangle may have written the depth first, a later one may fail its
/// test, and only the pixels it covers with the depth still cleared
/// count. Once a tile of a colour buffer is drawn over whole, the mark's
/// job kills it, and the jobs before it that drew there can go.
///
/// Jobs after the mark that read a pixel before it is drawn over
/// (blending, the depth test) write only that pixel, which is drawn over
/// later. The pixels a triangle covers are the ones the rasterizer draws
/// (`render_triangle`, then the clipping `raster::scanline` does). A job
/// that may write outside the tiles, a triangle whose texture is in a
/// colour buffer's memory (a Banshee's, which reads what was there before
/// the mark), and after a depth clear any other job that writes the
/// depth, end the following.
struct Cleared {
    /// The mark's job number.
    mark: u64,
    /// Whether the mark cleared the depth.
    depth: bool,
    /// Words of a bitmap row.
    row: usize,
    /// For each colour buffer, the pixels drawn over since the mark, a
    /// bit each; empty until a triangle draws into it.
    drawn: [Vec<u64>; 3],
    /// After a depth clear, the pixels whose depth a triangle may have
    /// written since.
    depth_written: Vec<u64>,
    /// The tiles of each colour buffer found drawn over whole.
    killed: [Tiles; 3],
}

impl Cleared {
    fn new(mark: u64, layout: &TileLayout, depth: bool) -> Self {
        let row = (layout.width as usize).div_ceil(64);
        Self {
            mark,
            depth,
            row,
            drawn: Default::default(),
            depth_written: if depth {
                vec![0; row * layout.height as usize]
            } else {
                Vec::new()
            },
            killed: [Tiles::NONE; 3],
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
    ) -> Option<(usize, Tiles)> {
        let slot = layout.slot(st.dest)?;
        let (fbz, am) = (st.fbz_mode, st.alpha_mode);
        let writes_depth = self.depth && fbz & (1 << 10) != 0 && st.aux.is_some();
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
            3 if self.depth => Some(true),
            _ => None,
        };
        if over.is_none() && !writes_depth {
            return Some((slot, Tiles::NONE));
        }
        let (w, h) = (layout.width as i32, layout.height as i32);
        let row = self.row;
        if over.is_some() && self.drawn[slot].is_empty() {
            self.drawn[slot] = vec![0; row * h as usize];
        }
        let (mut left_most, mut right_most, mut top, mut bottom) = (w, 0, h, 0);
        let (drawn, depth_written) = (&mut self.drawn[slot], &mut self.depth_written);
        spans(layout, st, verts, |scry, x0, x1| {
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
            return Some((slot, Tiles::NONE));
        }
        let rect = (
            left_most as u32,
            right_most as u32,
            top as u32,
            bottom as u32,
        );
        let mut killed = Tiles::NONE;
        let candidates = layout.tiles(rect, false);
        let tiles = candidates
            .rows()
            .flat_map(|ty| (candidates.x0..candidates.x1).map(move |tx| (tx as u32, ty as u32)));
        for (tx, ty) in tiles.filter(|&(tx, ty)| !self.killed[slot].contains(tx, ty)) {
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
                killed.insert(tx, ty);
            }
        }
        self.killed[slot].or(&killed);
        Some((slot, killed))
    }
}

/// The rows of the picture the triangle `verts` drawn with `st` covers,
/// as the rasterizer draws them (`render_triangle`, then the clipping
/// `raster::scanline` does): `f` gets each row and its columns
/// `x0..x1`.
fn spans(
    layout: &TileLayout,
    st: &RasterState,
    verts: &[(f32, f32); 3],
    mut f: impl FnMut(i32, i32, i32),
) {
    let (w, h) = (layout.width as i32, layout.height as i32);
    let flip = st.fbz_mode & (1 << 17) != 0;
    let clip = (st.fbz_mode & 1 != 0).then(|| st.clip());
    render_triangle(*verts, |y, mut x0, mut x1| {
        let scry = screen_y(st, y, flip);
        if let Some((left, right, low, high)) = clip {
            if scry < low || scry >= high {
                return;
            }
            x0 = x0.max(left);
            x1 = x1.min(right);
        }
        let (x0, x1) = (x0.max(0), x1.min(w));
        if scry >= 0 && scry < h && x0 < x1 {
            f(scry, x0, x1);
        }
    });
}

/// Whether every pixel colour buffer `slot` of the triangle `verts`
/// drawn with `st` covers is in `bitmap` (rows of `row` words).
fn covered_by(
    layout: &TileLayout,
    st: &RasterState,
    verts: &[(f32, f32); 3],
    bitmap: &[u64],
    row: usize,
) -> bool {
    let mut all = true;
    spans(layout, st, verts, |y, x0, x1| {
        let at = y as usize * row;
        all &= span_words(x0 as usize, x1 as usize).all(|(i, bits)| bitmap[at + i] & bits == bits);
    });
    all
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

    /// Tile `x`, `y`: 20x15 pixels of a 640x480 picture.
    fn tile(x: u32, y: u32) -> Rect {
        Rect::new(x, x + 1, y, y + 1)
    }

    fn write(slot: usize, tiles: Rect) -> Access {
        let mut a = Access::default();
        a.writes[slot] = tiles;
        a
    }

    fn fill(slot: usize) -> Access {
        let mut a = write(slot, Rect::FULL);
        a.kills[slot] = Rect::FULL;
        a
    }

    fn backlog(accesses: &[Access]) -> Backlog {
        let mut b = Backlog { layout: layout(), ..Default::default() };
        for (n, a) in accesses.iter().enumerate() {
            let words = Vec::new();
            b.jobs.push((
                Job::Pixels {
                    fb: Vram::new(16),
                    words,
                },
                a.clone(),
                n as u64,
            ));
        }
        b
    }

    #[test]
    fn a_fill_hides_what_was_drawn_before() {
        let mut b = backlog(&[
            write(0, tile(0, 0)),
            write(1, tile(0, 0)),
            fill(0),
            write(0, tile(1, 0)),
        ]);
        b.prune();
        assert_eq!(b.len(), 3, "the triangle in buffer 0 goes, the one in buffer 1 stays");
        assert_eq!(b.pruned, 1);
        assert_eq!(b.jobs[0].1, write(1, tile(0, 0)));
        b.prune();
        assert_eq!(b.len(), 3, "pruning again must retain the surviving jobs");
        assert_eq!(b.pruned, 1);
    }

    #[test]
    fn a_job_that_reads_keeps_what_it_reads() {
        // Blending in buffer 1 over the depth test of aux: the aux job
        // before it stays although a fill kills aux later.
        let mut reader = write(1, tile(0, 0));
        reader.reads[AUX] = tile(0, 0);
        let mut b = backlog(&[write(AUX, tile(0, 0)), reader, fill(AUX)]);
        b.prune();
        assert_eq!(b.len(), 3);
    }

    #[test]
    fn what_a_job_reads_in_place_matters_only_where_it_is_seen() {
        // Blending over tiles 0 and 1 of buffer 0; a later fill hides tile
        // 0. The job before that drew tile 0 alone goes, the one that drew
        // tile 1 stays.
        let mut blend = write(0, Rect::new(0, 2, 0, 1));
        blend.reads[0] = Rect::new(0, 2, 0, 1);
        let mut viewport = write(0, tile(0, 0));
        viewport.kills[0] = tile(0, 0);
        let mut b = backlog(&[write(0, tile(0, 0)), write(0, tile(1, 0)), blend, viewport]);
        b.prune();
        assert_eq!(b.len(), 3);
        assert_eq!(b.jobs[0].1, write(0, tile(1, 0)));
    }

    #[test]
    fn tiles_a_frame_drew_over_hide_the_frame_before() {
        // The mark of a frame in buffer 0 drew over tiles (0, 0) and
        // (5, 5): the jobs before that drew only there go.
        let mut mark = write(0, tile(0, 0));
        let mut covered = [Tiles::NONE; 3];
        covered[0].insert(0, 0);
        covered[0].insert(5, 5);
        mark.covered = Some(Box::new(Covered {
            tiles: covered,
            ..Default::default()
        }));
        let accesses = [
            write(0, tile(0, 0)),
            write(0, tile(5, 5)),
            write(0, tile(6, 5)),
            write(1, tile(0, 0)),
            mark,
        ];
        let mut b = backlog(&accesses);
        b.prune();
        assert_eq!(b.len(), 3);
        assert_eq!(b.jobs[0].1, write(0, tile(6, 5)));
        assert_eq!(b.jobs[1].1, write(1, tile(0, 0)));
    }

    #[test]
    fn a_triangle_whose_pixels_a_later_frame_drew_over_goes() {
        // A small triangle in tile (0, 0) of buffer 0, which stays seen:
        // the frame after drew over the triangle's pixels but not all of
        // the tile's.
        let mut st = crate::voodoo::raster::tests::two_unit_state([1 << 9, 0, 0, 0, 0, 0]);
        st.dest = 0;
        st.tmu = [None, None];
        let verts = [(2.0, 2.0), (10.0, 2.0), (2.0, 10.0)];
        let l = layout();
        let triangle = || Job::Triangle {
            st: st.clone(),
            p: Default::default(),
            verts,
            texcount: 0,
        };
        let row = 640usize.div_ceil(64);
        let mut pixels = vec![0u64; row * 480];
        for y in 0..12 {
            pixels[y * row] = (1 << 12) - 1;
        }
        assert!(covered_by(&l, &st, &verts, &pixels, row));
        assert!(!covered_by(&l, &st, &verts, &vec![0; row * 480], row));
        let mut mark = write(0, tile(0, 0));
        mark.covered = Some(Box::new(Covered {
            pixels: [pixels, Vec::new(), Vec::new()],
            row,
            ..Default::default()
        }));
        let mut b = Backlog {
            layout: l,
            ..Default::default()
        };
        b.jobs.push((triangle(), write(0, tile(0, 0)), 0));
        b.jobs.push((triangle(), write(0, tile(0, 0)), 1));
        b.jobs.push((
            Job::Pixels {
                fb: Vram::new(16),
                words: Vec::new(),
            },
            mark,
            2,
        ));
        b.jobs.push((triangle(), write(0, tile(0, 0)), 3));
        b.prune();
        let left: Vec<u64> = b.jobs.iter().map(|(.., n)| *n).collect();
        assert_eq!(
            left,
            [2, 3],
            "the triangles before the frame go, the one after stays"
        );
    }

    #[test]
    fn partial_fills_kill_only_their_tiles() {
        let mut viewport = write(0, Rect::new(0, 2, 0, 1));
        viewport.kills[0] = tile(0, 0);
        let mut b = backlog(&[write(0, tile(0, 0)), write(0, tile(1, 0)), viewport]);
        b.prune();
        assert_eq!(b.len(), 2);
        assert_eq!(b.jobs[0].1, write(0, tile(1, 0)));
    }

    #[test]
    fn pinned_jobs_keep_everything_before_them() {
        let mut b = backlog(&[write(0, tile(0, 0)), Access::PINNED, fill(0)]);
        b.prune();
        assert_eq!(b.len(), 3, "the pinned job may read the first one's pixels");
        let mut b = backlog(&[write(0, tile(0, 0)), fill(0), Access::PINNED]);
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
        assert_eq!(l.tiles((0, 640, 0, 480), true), Rect::FULL);
        assert_eq!(l.all(), Rect::FULL);
        assert_eq!(l.tiles((0, 20, 0, 15), true), tile(0, 0));
        assert!(
            l.tiles((0, 19, 0, 15), true).is_empty(),
            "a tile not covered whole"
        );
        assert_eq!(l.tiles((19, 21, 14, 15), false), Rect::new(0, 2, 0, 1));
        assert_eq!(l.tiles((0, 640, 465, 480), false), Rect::new(0, 32, 31, 32));
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
        let four = Rect::new(0, 32, 0, 4);
        assert_eq!((top.writes[AUX], top.kills[AUX]), (four, four));
        assert!(rows(&l, 1234, (0, 480)).pinned, "not a buffer");
        let rows640 = Stride::linear(640);
        let one = [Some(0), None, None, None];
        let wide = TileLayout::new(one, (600, 480), rows640, rows640, 640 * 480);
        let wide = rows(&wide, 0, (0, 480));
        assert!(wide.kept && !wide.pinned, "words right of the picture");
        assert_eq!((wide.writes[0], wide.kills[0]), (Rect::FULL, Rect::FULL));
    }

    #[test]
    fn kept_jobs_stay_but_hide_what_is_under_them() {
        let mut wide = fill(0);
        wide.kept = true;
        let mut b = backlog(&[write(0, tile(0, 0)), wide.clone(), fill(0)]);
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
        assert_eq!(words(&l, &[row(0, 0)]), write(0, tile(0, 0)));
        // Row 480 of buffer 0 is the memory of buffer 1's first pixel.
        assert_eq!(words(&l, &[row(480, 0)]), write(1, tile(0, 0)));
        assert!(words(&l, &[row(3 * 480, 0)]).pinned);
        let aux = Word {
            row: 1,
            at: 2 * 640 * 480 + 640 + 639,
            value: 0,
        };
        assert_eq!(words(&l, &[aux]), write(AUX, tile(31, 0)));
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
    fn words_one_after_another_find_the_tiles_each_one_does() {
        // `words` works out a word's pixel from the word before it: across
        // the end of a row, and from one buffer's last row into the next
        // buffer, the run touches the tiles the words alone do.
        let check = |l: &TileLayout, ats: std::ops::Range<u32>| {
            let run: Vec<Word> = ats
                .map(|at| Word {
                    row: 0,
                    at,
                    value: 0,
                })
                .collect();
            let alone: Vec<Access> = run
                .iter()
                .map(|w| words(l, std::slice::from_ref(w)))
                .collect();
            let together = words(l, &run);
            if alone.iter().any(|a| a.pinned) {
                assert!(together.pinned);
                return;
            }
            let mut writes = [Rect::default(); 4];
            for a in &alone {
                for (w, &r) in writes.iter_mut().zip(&a.writes) {
                    *w = w.union(r);
                }
            }
            assert_eq!(together.writes, writes);
        };
        let size = 640 * 480;
        let l = layout();
        check(&l, 600..1400);
        check(&l, size - 700..size + 700);
        check(&l, 3 * size - 10..3 * size + 10);
        let wide = Stride::linear(1024);
        let one = [Some(0), None, None, None];
        let l = TileLayout::new(one, (640, 480), wide, wide, 1024 * 480);
        check(&l, 0..640);
        check(&l, 1024..1664);
        check(&l, 600..700);
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
        assert_eq!(words(&l, &[last]), write(0, tile(31, 31)));
        // Pixel (65, 33) of the auxiliary buffer.
        let at = size + tiles.words + 2048 + 64 + 1;
        let aux = Word {
            row: 33,
            at: at as u32,
            value: 0,
        };
        assert_eq!(words(&l, &[aux]), write(AUX, tile(3, 2)));
        let bases = [Some(0), Some(size - 1), None, None];
        let overlapping = TileLayout::new(bases, (640, 480), tiles, tiles, 2 * size);
        assert!(!overlapping.prunable, "tiles span their rows");
    }

    #[test]
    fn a_footprint_holds_the_textures_read_and_the_buffers_written() {
        let mut f = Footprint::default();
        assert!(!f.touches(0, u64::MAX), "nothing queued reaches nothing");
        f.texture((0x1000, 0x3000));
        f.texture((0x8000, 0x9000));
        let word = Word {
            row: 0,
            at: 0x40_0000,
            value: 0,
        };
        f.add(&Job::Pixels {
            fb: Vram::new(16),
            words: vec![word],
        });
        assert!(f.touches(0x2FFF, 0x3001) && f.touches(0x8FFF, 0x9000));
        assert!(!f.touches(0x3000, 0x8000), "between the textures");
        assert!(f.touches(0x80_0000, 0x80_0004), "above the words written");
        assert!(!f.touches(0x7F_FFFC, 0x80_0000));
        // A range within one held already adds nothing.
        f.texture((0x1800, 0x2000));
        // Past so many textures, the nearest range takes a new one in:
        // the last three become one, the first ones stay apart.
        for i in 0..FOOTPRINT_TEXTURES as u64 {
            f.texture((0x10_0000 + 0x100 * i, 0x10_0080 + 0x100 * i));
        }
        assert!(!f.touches(0x3000, 0x3004) && !f.touches(0x10_4000, 0x10_4004));
        assert!(
            !f.touches(0x10_3C80, 0x10_3D00),
            "between ranges kept apart"
        );
        assert!(f.touches(0x10_3D80, 0x10_3E00) && f.touches(0x10_3E80, 0x10_3F00));
        let mut g = Footprint::default();
        g.merge(&f);
        assert!(g.touches(0x2FFF, 0x3001) && g.touches(0x80_0000, 0x80_0004));
        assert!(!g.touches(0x3000, 0x8000));
        f.clear();
        assert!(!f.touches(0, u64::MAX));
    }

    /// A backlog holding a triangle that takes its 256x256 texture of
    /// bytes from the frame buffer's memory at 300001h (to 32AAAAh with
    /// its smaller levels), with the frame buffer and the levels' bases.
    fn texturing_backlog() -> (Backlog, Vram, [u32; 9]) {
        let mut st = crate::voodoo::raster::tests::two_unit_state([0; 6]);
        let base = 0x30_0001;
        let t = st.tmu[0].as_mut().expect("unit 0");
        t.ram = st.fb.clone();
        t.mask = st.fb.bytes() as u32 - 1;
        for offset in &mut t.lodoffset {
            *offset += base;
        }
        let old = t.lodoffset;
        st.tmu[1] = None;
        // The buffers above the texture, as Glide puts them.
        (st.dest, st.aux) = (0x1C_0000, Some(0x1E_0000));
        let fb = st.fb.clone();
        let p = Default::default();
        let job = Job::Triangle {
            st,
            p,
            verts: [(0.0, 0.0); 3],
            texcount: 1,
        };
        let mut b = Backlog {
            layout: layout(),
            ..Default::default()
        };
        assert!(b.push(job, Access::default()).is_empty());
        (b, fb, old)
    }

    /// A job writing the frame buffer's word `at`.
    fn word_at(fb: &Vram, at: u32) -> Job {
        Job::Pixels {
            fb: fb.clone(),
            words: vec![Word {
                row: 0,
                at,
                value: 0,
            }],
        }
    }

    const DOWNLOAD: (u64, u64) = (0x31_0000, 0x31_1000);

    #[test]
    fn a_triangle_held_takes_its_texels_from_a_copy_when_spared() {
        let (mut b, fb, old) = texturing_backlog();
        let before: Vec<u8> = (0..fb.bytes()).map(|a| fb.byte(a)).collect();
        let mut read_back = Vec::new();
        assert!(b.readers.touches(DOWNLOAD.0, DOWNLOAD.1));
        let given_out = Footprint::default();
        assert!(b.spare(DOWNLOAD, &given_out, |a, z| read_back.push((a, z))));
        assert_eq!(read_back, [(0x30_0000, 0x32_AAAA)], "the bytes copied");
        assert!(
            !b.readers.touches(DOWNLOAD.0, DOWNLOAD.1),
            "nothing reads the texture"
        );
        for a in DOWNLOAD.0..DOWNLOAD.1 {
            fb.set_byte(a as usize, 0xEE);
        }
        let Job::Triangle { st, .. } = &b.jobs[0].0 else {
            panic!("a triangle")
        };
        let t = st.tmu[0].as_ref().expect("unit 0");
        assert!(!t.ram.same(&fb));
        for (level, (&now, &was)) in t.lodoffset.iter().zip(&old).enumerate() {
            let texels = (256 >> level) * (256 >> level);
            for i in 0..texels {
                let a = (now.wrapping_add(i) & t.mask) as usize;
                assert_eq!(t.ram.byte(a), before[(was + i) as usize], "level {level}");
            }
        }
        // A job that writes there has to be drawn first.
        let _ = b.push(word_at(&fb, 0x18_8100), Access::default());
        assert!(!b.spare(DOWNLOAD, &given_out, |_, _| {}));
    }

    #[test]
    fn texels_a_job_before_draws_into_are_not_copied() {
        // A job held writes the texture's smaller levels, above the bytes
        // downloaded: the triangle after it has to read what it wrote.
        let (mut b, fb, _) = texturing_backlog();
        let _ = b.push(word_at(&fb, 0x19_0000), Access::default());
        assert!(!b.spare(DOWNLOAD, &Footprint::default(), |_, _| {}));
        assert!(
            b.readers.touches(DOWNLOAD.0, DOWNLOAD.1),
            "the readers stay"
        );
        // The same for a job the workers have.
        let (mut b, fb, _) = texturing_backlog();
        let mut given_out = Footprint::default();
        given_out.add(&word_at(&fb, 0x19_0000));
        assert!(!b.spare(DOWNLOAD, &given_out, |_, _| {}));
        let Job::Triangle { st, .. } = &b.jobs[0].0 else {
            panic!("a triangle")
        };
        assert!(st.tmu[0].as_ref().expect("unit 0").ram.same(&fb));
    }

    #[test]
    fn taking_the_jobs_gives_what_they_reach() {
        let (mut b, _, _) = texturing_backlog();
        // Kept by pruning.
        b.jobs[0].1 = Access::PINNED;
        let (jobs, reach) = b.take();
        assert_eq!(jobs.len(), 1);
        assert!(reach.touches(0x30_0000, 0x30_0002), "the texture read");
        assert!(reach.touches(0x38_0000, 0x38_0002), "the buffer drawn into");
        assert!(!reach.touches(0x32_AAAA, 0x38_0000));
        assert!(b.readers.ranges.is_empty() && !b.writes.writes(u64::MAX));
    }

    #[test]
    fn only_the_newest_marks_keep_their_pixels() {
        let row = 640usize.div_ceil(64);
        let mut b = Backlog {
            layout: layout(),
            ..Default::default()
        };
        for n in 0..PIXEL_MARKS as u64 + 2 {
            b.jobs
                .push((word_at(&Vram::new(16), 0), Access::default(), n));
            let mut cleared = Cleared::new(n, &b.layout, false);
            cleared.drawn[0] = vec![1; row * 480];
            cleared.drawn[1] = vec![0; row * 480];
            b.cleared = Some(cleared);
            b.end_frame();
        }
        let kept: Vec<u64> = b
            .jobs
            .iter()
            .filter(|(_, a, _)| a.covered.is_some())
            .map(|(.., n)| *n)
            .collect();
        assert_eq!(kept, (2..PIXEL_MARKS as u64 + 2).collect::<Vec<_>>());
        let covered = b.jobs[2].1.covered.as_ref().expect("pixels");
        assert!(covered.pixels[1].is_empty(), "a bitmap of none drawn over");
    }
}

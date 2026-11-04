//! 2D operations a Banshee's or Voodoo3's OpenGL renderer does in its
//! copy of the card's memory, for the built-in Windows' DirectDraw and
//! Direct3D: a blit, a fill, a window's Present. The card records the
//! operation for the renderer and leaves its own memory as it was; the
//! pages the operation drew into are then the GPU's (`tracker`), and
//! whatever reads or writes the card's memory there (the program, GDI,
//! a blit done here, the software picture, a save state) has them read
//! back from the GPU first (`gpu_sync`). A blit whose pixels are read
//! again and again from the program goes on in software: the read back
//! would cost more than the blit.
//!
//! The renderer is in the front end, which hands the card a link to it
//! (`GpuLink`) while it draws the card's picture. The operations, and
//! everything recorded before them, go to the renderer as they come, so
//! that its copy of the memory is never behind the operations recorded:
//! a read back then only has to read.

use super::backlog::Access;
#[cfg(test)]
use super::mirror::Plane;
use super::mirror::{Blit, BlitSource, Command, LayoutKey, row_bytes, rows_of};
use super::raster::{RasterState, Stride};
use super::tracker::{PAGE, Tracker};
use super::workers::{Job, Word};
use super::{NONE, Voodoo};

/// The bytes of the memory a job left to the renderer draws into, a range
/// for each buffer (`gpu_job_bytes`).
pub(crate) type JobBytes = [Option<(u32, u32)>; 4];

/// Rows a bit each in `ReadRows`: up to 2,048 rows a buffer.
const ROW_WORDS: usize = 32;

/// What the reads back of the buffers (`gpu_catch_up_in`) covered, for
/// the debugger: whether a cache of the rows read, kept until something
/// draws over them, would have saved reads. Each buffer (the three colour
/// buffers, then the auxiliary one) has a bit for each of its rows that
/// was drawn since it was last read. Until the first read every bit is
/// set and nothing marks them, so a program that reads nothing back pays
/// nothing.
pub(crate) struct ReadRows {
    drawn: [[u64; ROW_WORDS]; 4],
    armed: bool,
    /// Rows read back, summed over the reads.
    pub rows: u64,
    /// Reads of part of a buffer rather than all its rows.
    pub partial: u64,
    /// Rows read that were drawn since the last read of their buffer,
    /// and those that weren't, which a cache would have had.
    pub drawn_rows: u64,
    pub undrawn_rows: u64,
}

impl Default for ReadRows {
    fn default() -> Self {
        Self {
            drawn: [[!0; ROW_WORDS]; 4],
            armed: false,
            rows: 0,
            partial: 0,
            drawn_rows: 0,
            undrawn_rows: 0,
        }
    }
}

impl ReadRows {
    /// The words of a buffer's bits that rows `top..bottom` are in, with
    /// the mask of each.
    fn words(top: u32, bottom: u32) -> impl Iterator<Item = (usize, u64)> {
        let end = 64 * ROW_WORDS as u32;
        let (top, bottom) = (top.min(end), bottom.min(end));
        (top / 64..bottom.div_ceil(64)).map(move |w| {
            let lo = (w * 64).max(top) - w * 64;
            let hi = (w * 64 + 64).min(bottom) - w * 64;
            let mask = if hi - lo == 64 {
                !0
            } else {
                ((1u64 << (hi - lo)) - 1) << lo
            };
            (w as usize, mask)
        })
    }

    /// Whether drawing has to be marked: something was read back.
    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Rows `top..bottom` of buffer `slot` were drawn.
    pub fn mark(&mut self, slot: usize, (top, bottom): (u32, u32)) {
        for (w, mask) in Self::words(top, bottom) {
            self.drawn[slot][w] |= mask;
        }
    }

    /// Rows `top..bottom` of buffer `slot`, of `height`, were read back.
    pub fn read(&mut self, slot: usize, (top, bottom): (u32, u32), height: u32) {
        self.armed = true;
        let rows = bottom.saturating_sub(top) as u64;
        self.rows += rows;
        if top > 0 || bottom < height {
            self.partial += 1;
        }
        let mut drawn = 0;
        for (w, mask) in Self::words(top, bottom) {
            drawn += (self.drawn[slot][w] & mask).count_ones() as u64;
            self.drawn[slot][w] &= !mask;
        }
        let drawn = drawn.min(rows);
        self.drawn_rows += drawn;
        self.undrawn_rows += rows - drawn;
    }
}

/// The OpenGL renderer, as the card reaches it between frames.
pub trait GpuLink {
    /// Whether the renderer is there and keeps a copy of the card's
    /// memory to do 2D operations in.
    fn ready(&self) -> bool;

    /// Do `commands`, the recording so far, now.
    fn run(&mut self, commands: Vec<Command>);

    /// Bytes of the renderer's copy of the card's memory from byte `at`
    /// on, into `out`. False, with `out` as it was, if the renderer has
    /// no copy to read them from (it went).
    fn read(&mut self, at: u32, out: &mut [u8]) -> bool;

    /// Rows `top..bottom` of a buffer's pixels as the renderer drew them,
    /// at the card's resolution, row by row from the top: colour buffer
    /// `dest` (its word offset) in 5-6-5, or with None the depth buffer.
    /// False, with `out` as it was, if it has no such buffer or `out`
    /// isn't the rows' size.
    fn read_buffer(&mut self, _dest: Option<u32>, _rows: (u32, u32), _out: &mut [u16]) -> bool {
        false
    }
}

/// The renderer's copy of the memory, as the card keeps track of it.
#[derive(Default)]
pub struct Gpu {
    pub(super) tracker: Tracker,
    link: Option<Box<dyn GpuLink>>,
    /// Pages read back from the GPU, for `/api/status`.
    read_back: u64,
    /// 2D operations done on the GPU.
    blits: u64,
    /// Commands of a Banshee's 2D engine drawn on the GPU (`banshee_2d_gpu`).
    pub(super) engine: u64,
    /// Those commands, and the ones left to the CPU, by kind.
    pub(super) engine_counts: EngineCounts,
    /// 2D operations left to the CPU as the program reads or writes
    /// where they draw (the tracker's hot pages).
    pub(super) hot: u64,
    /// Frames since the hot pages last cooled.
    frames: u32,
    /// Pages only the GPU had were lost, which the log said.
    lost: bool,
}

/// For `/api/status`: the commands of a Banshee's 2D engine whose bytes
/// were newer on the GPU (`banshee_2d_gpu`).
#[derive(Default)]
pub(crate) struct EngineCounts {
    /// Those drawn on the GPU, by how.
    pub drawn: std::collections::BTreeMap<&'static str, u64>,
    /// Those drawn on the GPU, by the destination's format.
    pub formats: std::collections::BTreeMap<String, u64>,
    /// Those left to the CPU, by why.
    pub left: std::collections::BTreeMap<&'static str, u64>,
}

/// Frames a page read back from the GPU stays hot: about a second.
pub(crate) const HOT_FRAMES: u32 = 60;

impl Gpu {
    pub fn new(bytes: usize) -> Self {
        Self {
            tracker: Tracker::new(bytes),
            ..Self::default()
        }
    }
}

impl Voodoo {
    /// Give the card the renderer to do its 2D operations with, or take
    /// it away (the renderer stops): what only the GPU had is read back
    /// first.
    pub fn set_gpu_link(&mut self, link: Option<Box<dyn GpuLink>>) {
        self.gpu_sync_all();
        let gpu = self.gpu.get_mut();
        gpu.link = link;
    }

    /// Whether the card has a link to the renderer (`set_gpu_link`).
    pub fn gpu_linked(&self) -> bool {
        self.gpu.borrow().link.is_some()
    }

    /// Whether reads of the buffers get the renderer's pictures of them
    /// (`voodoo_readback=gpu`) rather than what the software renderer
    /// draws from the same commands. Off, what only the renderer had is
    /// read back first.
    pub fn set_gpu_reads(&mut self, on: bool) {
        if !on && self.gpu_reads {
            self.gpu_catch_up();
        }
        self.gpu_reads = on;
    }

    /// Whether `job`, which the backlog would hold with `access`, is the
    /// renderer's alone: reads of the buffers get the renderer's pictures
    /// of them, and the job draws nothing outside them, nor into alpha
    /// planes, which the renderer keeps in its colour buffers. On a
    /// Banshee or Voodoo3 the job draws into the recording's buffers, the
    /// software picture isn't wanted (it would read them back every
    /// frame), and the program didn't read back memory there lately (it
    /// is hot): a program that reads what the 3D draws every frame waits
    /// less for the software rasterizer than for the GPU.
    ///
    /// The bytes the job draws into (`gpu_job_bytes`) if it is, for
    /// `gpu_left`.
    pub(crate) fn gpu_takes(&self, job: &Job, access: &Access) -> Option<JobBytes> {
        if let Job::Triangle { st, .. } | Job::Fastfill { st, .. } = job
            && st.fbz_mode & (1 << 18) != 0
        {
            self.gpu_alpha.set(true);
        }
        let takes = self.gpu_reads
            && !self.gpu_alpha.get()
            && !access.pinned
            && !access.kept
            && self.gpu.borrow().link.is_some()
            && !(self.banshee.is_some() && self.software_picture);
        if !takes {
            return None;
        }
        let bytes = self.gpu_job_bytes(job)?;
        if self.banshee.is_some() {
            // A depth clear goes with the colour buffer it clears for: one
            // drawn in software has its depth in the memory too, rather than
            // read back before its first triangle every frame.
            let key = self.mirror_layout()?;
            let gpu = self.gpu.borrow();
            let hot = |&(a, b): &(u32, u32)| gpu.tracker.hot_in(a, b);
            if bytes.iter().flatten().any(hot) || self.gpu_colour_hot(&key) {
                return None;
            }
        }
        Some(bytes)
    }

    /// Whether the colour buffer the 3D engine draws into (colBufferAddr)
    /// is one of `key`'s with rows hot: its triangles are drawn in
    /// software, and a clear of the depth they test against goes into the
    /// memory with them rather than be read back before them every frame.
    pub(crate) fn gpu_colour_hot(&self, key: &LayoutKey) -> bool {
        let col = self.fbi.rgboffs[0];
        key.color.contains(&col) && {
            let (start, end) = row_bytes(col / 2, key.stride, (0, key.height));
            self.gpu.borrow().tracker.hot_in(start, end)
        }
    }

    /// `job` is drawn here, in software. The memory is brought up to the
    /// renderer for it: on a Banshee only the rows of the buffers it
    /// draws into (its textures came back before it was recorded,
    /// `gpu_texels`), which leaves the others on the GPU; on the other
    /// cards all of it. On a Banshee those rows change in the memory the
    /// texture units read, which `gpu_texels` hears of.
    pub(crate) fn gpu_draws_here(&self, job: &Job) {
        let bytes = self.banshee.is_some().then(|| self.gpu_job_bytes(job));
        let Some(Some(bytes)) = bytes else {
            self.gpu_catch_up();
            return;
        };
        for (start, end) in bytes.into_iter().flatten() {
            if self.gpu_behind.get() && self.gpu.borrow().tracker.buffers_in(start, end) {
                self.gpu_catch_up_in(Some((start, end)));
            }
            self.gpu.borrow_mut().tracker.drawn_in_memory(start, end);
        }
    }

    /// The bytes of the memory a job left to the renderer draws into, as
    /// all the rows of its buffers: its colour buffer and the auxiliary
    /// buffer, or every colour buffer for a job of another kind. On a
    /// Banshee they are the recording's buffers, None if the job draws
    /// colours into one the recording doesn't have.
    fn gpu_job_bytes(&self, job: &Job) -> Option<JobBytes> {
        let mut bytes = [None; 4];
        let mut n = 0;
        let mut mark = |base: u32, stride: Stride, rows: (u32, u32)| {
            if base != NONE && n < bytes.len() {
                bytes[n] = Some(row_bytes(base / 2, stride, rows));
                n += 1;
            }
        };
        if self.banshee.is_some() {
            let key = self.mirror_layout()?;
            let all = (0, key.height);
            match job {
                Job::Triangle { st, .. } | Job::Fastfill { st, .. } => {
                    let dest = st.dest as u32 * 2;
                    if st.fbz_mode & (1 << 9) != 0 {
                        if !key.color.contains(&dest) {
                            return None;
                        }
                        mark(dest, key.stride, all);
                    }
                    // The auxiliary buffer, if a triangle tests or writes
                    // depth, or uses alpha planes there (whose alpha it
                    // may blend with without writing it), or a fastfill
                    // writes it (`fastfill_row`, which tests nothing): a
                    // job that doesn't leaves it to whatever reads it.
                    let aux = match job {
                        Job::Fastfill { .. } => 1 << 10,
                        _ => 1 << 4 | 1 << 10 | 1 << 18,
                    };
                    if st.fbz_mode & aux != 0 {
                        mark(key.aux, key.aux_stride, all);
                    }
                }
                _ => {
                    for offs in key.color {
                        mark(offs, key.stride, all);
                    }
                    mark(key.aux, key.aux_stride, all);
                }
            }
            return Some(bytes);
        }
        let fbi = &self.fbi;
        let all = (0, fbi.height);
        match job {
            Job::Triangle { st, .. } | Job::Fastfill { st, .. } => {
                mark(st.dest as u32 * 2, fbi.stride, all);
                mark(fbi.auxoffs, fbi.aux_stride, all);
            }
            // A Voodoo2's SGRAM fill of a buffer's rows (`sgram_fill_rows`).
            Job::Rows { base, rows, .. } => {
                let aux = fbi.auxoffs != NONE && *base == fbi.auxoffs as usize / 2;
                let stride = if aux { fbi.aux_stride } else { fbi.stride };
                mark(*base as u32 * 2, stride, *rows);
            }
            _ => {
                for offs in fbi.rgboffs {
                    mark(offs, fbi.stride, all);
                }
                mark(fbi.auxoffs, fbi.aux_stride, all);
            }
        }
        Some(bytes)
    }

    /// The buffers a job left to the renderer draws into are behind it
    /// in memory: `bytes`, the rows of its colour buffer and the
    /// auxiliary buffer that `gpu_takes` gave, in the tracker.
    pub(crate) fn gpu_left(&self, job: &Job, bytes: JobBytes) {
        self.gpu_behind.set(true);
        self.gpu_only.set(self.gpu_only.get() + 1);
        if self.gpu_rows.borrow().armed() {
            self.gpu_mark_drawn(job);
        }
        let mut gpu = self.gpu.borrow_mut();
        for (start, end) in bytes.into_iter().flatten() {
            gpu.tracker.drawn_in_buffer(start, end);
        }
    }

    /// A Banshee's triangle draws with texels from memory that is or was
    /// one of its 3D buffers (a buffer drawn and then drawn with as a
    /// texture, from this layout or an earlier one): the memory there
    /// gets what was drawn before, read back from the renderer's 3D
    /// buffers where they are newer, and the recording decodes the
    /// texture again from it. The renderer then draws with its own
    /// picture at the card's resolution, never with memory that is
    /// behind it, and so does the software rasterizer. The same goes for
    /// pages a 2D operation drew in the renderer's copy of the memory
    /// (`gpu_blit`): a 2D engine's copy of a buffer into a texture.
    pub(crate) fn gpu_texels(&mut self, st: &RasterState, texcount: usize) {
        let Some(key) = self.mirror_layout() else {
            return;
        };
        for unit in 0..texcount.min(self.tmu.len()) {
            let Some(t) = &st.tmu[unit] else {
                continue;
            };
            // The levels the LOD range reads, not the ones Glide's base
            // address puts below the texture: those lie in other memory,
            // often the depth buffer under it, whose rows a textured
            // triangle would otherwise read back every time (#487).
            let (start, end) = super::mirror::texel_span(t);
            let (start, end) = (start as u32, end as u32);
            // Every texture is looked up, not only those under the
            // buffers drawn now: a buffer of the layout before (a texture
            // drawn into as a render target) was read back as the layout
            // changed, and the bytes it left in `changed` are only found
            // here.
            // Read back for the card itself, the rows don't turn hot: the
            // program didn't ask for them.
            if self.gpu.get_mut().tracker.buffers_in(start, end) {
                self.gpu_catch_up_in(Some((start, end)));
            }
            // Pages only the renderer's copy of the memory has come back
            // after the jobs queued before, which read the texels as they
            // were, and the recording decodes the texture again.
            let drawn_2d = self.gpu.get_mut().tracker.gpu_in(start, end);
            if drawn_2d {
                if self.workers_queue() {
                    self.catch_up_workers();
                }
                self.gpu_sync_quiet(start, end);
            }
            // The memory under the texture changed since it was last
            // decoded if rows there were read back (just now, or for the
            // program or the 2D engine), or drawn into in software
            // (`gpu_draws_here`), whether those jobs are drawn yet or not.
            // Frame buffer writes still queued may land there too, if the
            // texture is under the buffers drawn now: they go only into
            // those (colBufferAddr's and the auxiliary buffer, which
            // `mirror_sync` just put in the layout, catching up the
            // writes queued for the layout before). What is queued is
            // drawn first, and the recording decodes it again.
            let changed = self.gpu.get_mut().tracker.take_changed(start, end);
            let words = !self.words.get_mut().is_empty() && key.overlaps(start, end);
            if changed || words || drawn_2d {
                if words || self.outstanding.get() || !self.backlog.get_mut().is_empty() {
                    self.catch_up_workers();
                }
                for tmu in &mut self.tmu {
                    tmu.note_written(start, end);
                }
            }
        }
    }

    /// The buffer slot of `ReadRows` for a buffer's key in the recording
    /// (a colour buffer's word offset, None for the auxiliary buffer):
    /// the colour buffers in the recording's order, which on a Banshee
    /// isn't the registers' (`banshee_mirror_key`).
    fn gpu_rows_slot(&self, key: Option<u32>) -> usize {
        let colour = self.mirror_layout().map_or(self.fbi.rgboffs, |k| k.color);
        key.and_then(|k| colour.iter().position(|&o| o != NONE && o / 2 == k))
            .unwrap_or(3)
    }

    /// Mark the rows `job` draws into in `ReadRows`, generously.
    fn gpu_mark_drawn(&self, job: &Job) {
        let height = self.fbi.height;
        let mut reads = self.gpu_rows.borrow_mut();
        // Rows y0..y1 as the card counts them, as rows of the buffer.
        let rows = |st: &super::raster::RasterState, y0: i32, y1: i32| {
            let (y0, y1) = if st.fbz_mode & (1 << 17) != 0 {
                (st.yorigin as i32 - y1 + 1, st.yorigin as i32 - y0 + 1)
            } else {
                (y0, y1)
            };
            let clamp = |y: i32| y.clamp(0, height as i32) as u32;
            (clamp(y0), clamp(y1))
        };
        match job {
            Job::Triangle { st, verts, .. } => {
                let ys = verts.iter().map(|v| v.1);
                let low = ys.clone().fold(f32::INFINITY, f32::min);
                let high = ys.fold(f32::NEG_INFINITY, f32::max);
                let r = if low.is_finite() && high.is_finite() {
                    rows(st, low.floor() as i32, high.ceil() as i32 + 1)
                } else {
                    (0, height)
                };
                reads.mark(self.gpu_rows_slot(Some(st.dest as u32)), r);
                reads.mark(3, r);
            }
            Job::Fastfill { st, y0, y1, .. } => {
                let r = rows(st, *y0, *y1);
                reads.mark(self.gpu_rows_slot(Some(st.dest as u32)), r);
                reads.mark(3, r);
            }
            Job::Rows { base, rows, .. } => {
                let aux = self.fbi.auxoffs != super::NONE && *base == self.fbi.auxoffs as usize / 2;
                let slot = if aux {
                    3
                } else {
                    self.gpu_rows_slot(Some(*base as u32))
                };
                reads.mark(slot, *rows);
            }
            _ => {
                for slot in 0..4 {
                    reads.mark(slot, (0, height));
                }
            }
        }
    }

    /// Bring the buffers in memory up to the renderer's pictures of them,
    /// if jobs were left to it: what it has recorded goes to it first,
    /// and it reads the buffers back.
    pub(crate) fn gpu_catch_up(&self) {
        self.gpu_catch_up_in(None);
    }

    /// The same for the rows of the buffers bytes `start..end` of the
    /// memory hold, which the program reads or writes: on a Banshee they
    /// are hot from then on (`gpu_takes`).
    pub(crate) fn gpu_catch_up_hot(&self, start: u32, end: u32) {
        self.gpu_catch_up_rows(Some((start, end)), self.banshee.is_some());
    }

    /// The same for the rows of the buffers that bytes `start..end` of
    /// the memory hold (with None all of them): only the rows behind
    /// that are asked for are read back, at the card's resolution.
    ///
    /// The software jobs and frame buffer writes still queued came before
    /// the jobs left to the renderer, which drew them too: they go into
    /// the memory first, and the rows read back go over them, not the
    /// other way round.
    pub(crate) fn gpu_catch_up_in(&self, range: Option<(u32, u32)>) {
        self.gpu_catch_up_rows(range, false);
    }

    /// `gpu_catch_up_in`, the rows read back hot if `hot`.
    fn gpu_catch_up_rows(&self, range: Option<(u32, u32)>, hot: bool) {
        if !self.gpu_behind.get() {
            return;
        }
        self.catch_up_workers();
        let forget = || {
            self.gpu_behind.set(false);
            self.gpu.borrow_mut().tracker.forget_buffers();
        };
        let (commands, key) = {
            let mut mirror = self.mirror.borrow_mut();
            let Some(mirror) = mirror.as_mut() else {
                forget();
                return;
            };
            (mirror.take_commands(), mirror.layout().copied())
        };
        let Some(key) = key else {
            forget();
            return;
        };
        let mut gpu = self.gpu.borrow_mut();
        let gpu = &mut *gpu;
        let Some(link) = gpu.link.as_mut() else {
            if !std::mem::replace(&mut gpu.lost, true) {
                eprintln!("[3DFX] the OpenGL renderer went with the buffers only it had");
            }
            gpu.tracker.forget_buffers();
            self.gpu_behind.set(false);
            return;
        };
        link.run(commands);
        let layout = key.layout();
        // The rows asked for, buffer by buffer: the buffer's key in the
        // recording (a colour buffer's word offset, None for the auxiliary
        // buffer) and the rows.
        let asked = match range {
            None => layout
                .color
                .iter()
                .map(|&offs| Some(offs))
                .chain(layout.aux.map(|_| None))
                .map(|k| (k, 0, layout.height))
                .collect(),
            Some((start, end)) => key.rows_in(start, end),
        };
        let width = layout.width as usize;
        let ram = &self.fbi.ram;
        let mut read = false;
        for (buffer, top, bottom) in asked {
            let (base, stride) = match (buffer, layout.aux) {
                (Some(offs), _) => (offs, layout.stride),
                (None, Some(offs)) => (offs, layout.aux_stride),
                (None, None) => continue,
            };
            // The bytes of those rows that are behind, read back by rows.
            let (start, end) = row_bytes(base, stride, (top, bottom));
            for bytes in gpu.tracker.take_buffers(start, end, hot) {
                let (top, bottom) = rows_of(base, stride, bytes);
                let bottom = bottom.min(layout.height);
                if top >= bottom {
                    continue;
                }
                let mut pixels = vec![0u16; width * (bottom - top) as usize];
                if link.read_buffer(buffer, (top, bottom), &mut pixels) {
                    read = true;
                    let slot = self.gpu_rows_slot(buffer);
                    let mut reads = self.gpu_rows.borrow_mut();
                    reads.read(slot, (top, bottom), layout.height);
                    for (y, row) in (top as usize..).zip(pixels.chunks_exact(width)) {
                        let start = base as usize + stride.row(y);
                        for (x, &value) in row.iter().enumerate() {
                            let at = start + stride.col(x);
                            if at < ram.len() {
                                ram.set(at, value);
                            }
                        }
                    }
                }
            }
        }
        // Read back whole, the memory has what the renderer can give: the
        // rest is in no buffer it has.
        if range.is_none() {
            gpu.tracker.forget_buffers();
        }
        if read {
            self.gpu_read_backs.set(self.gpu_read_backs.get() + 1);
        }
        self.gpu_behind.set(gpu.tracker.any_in_buffers());
    }

    /// Whether 2D operations may go to the GPU: the renderer draws the
    /// picture, gave the card a link to it and keeps the card's memory.
    pub fn gpu_ready(&self) -> bool {
        let linked = self
            .gpu
            .borrow()
            .link
            .as_ref()
            .is_some_and(|link| link.ready());
        self.recording() && self.banshee.is_some() && linked
    }

    /// Do `blit` on the GPU, if it can be: the renderer has a link, and
    /// the destination is no buffer of the 3D engine's, which draws into
    /// the card's memory itself. False leaves it to the caller, in
    /// software.
    pub fn gpu_blit(&mut self, blit: Blit) -> bool {
        // While the software picture is wanted (a recording, the settings
        // window), it reads back every page the GPU drew each frame.
        if !self.gpu_ready() || self.software_picture || blit.rects.is_empty() {
            return false;
        }
        let (start, end) = blit.dst_span();
        if start >= end || end as usize > self.fbi.ram.bytes() {
            return false;
        }
        let key = self.mirror_layout();
        // The 3D engine's buffers are on the GPU as pictures of their
        // own, and the software renderer draws them into the memory
        // without a word to `tracker`: a page the blit draws into or reads
        // must hold none of them (`mirror_sync` reads back the pages
        // under buffers that move).
        // Where the program reads or writes the result, a blit on the CPU
        // costs less than one on the GPU read back.
        if self.gpu.get_mut().tracker.hot_in(start, end) {
            self.gpu.get_mut().hot += 1;
            return false;
        }
        let touches = |a: u32, b: u32| {
            key.is_some_and(|k| k.overlaps(a & !(PAGE - 1), b.div_ceil(PAGE) * PAGE))
        };
        if touches(start, end) {
            return false;
        }
        match &blit.src {
            BlitSource::Memory { .. } => {
                let (a, b) = blit.src_span().unwrap_or_default();
                if touches(a, b) || b as usize > self.fbi.ram.bytes() {
                    return false;
                }
            }
            BlitSource::Buffer { buffer, .. } => {
                if !key.is_some_and(|k| k.color.contains(&(buffer * 2))) {
                    return false;
                }
            }
            BlitSource::Fill(_) => {}
        }
        // What was recorded before goes first, which leaves the recording
        // room for the pages and the blit; then the renderer gets the
        // pages the blit reads and draws into that it lacks.
        let Some(mirror) = self.mirror.get_mut().as_mut() else {
            return false;
        };
        let mut commands = mirror.take_commands();
        self.gpu_resend_dropped();
        let mut spans = vec![(start, end)];
        spans.extend(blit.src_span());
        for (a, b) in spans {
            self.send_pages(a, b);
        }
        let Some(mirror) = self.mirror.get_mut().as_mut() else {
            return false;
        };
        mirror.blit(blit);
        commands.extend(mirror.take_commands());
        let gpu = self.gpu.get_mut();
        if let Some(link) = gpu.link.as_mut() {
            link.run(commands);
        }
        gpu.tracker.drawn_on_gpu(start, end);
        gpu.blits += 1;
        // A software picture, where one is wanted, is drawn again.
        self.display_dirty = true;
        true
    }

    /// Record `command`, a fill (`Command::Fill`) or a copy between colour
    /// buffers (`Command::Copy`) that draws into rows `rows` of one of the
    /// 3D buffers the renderer draws (a colour buffer's word offset, None
    /// for the auxiliary buffer), for the renderer alone: those rows are
    /// newer on the GPU from now on, as a job left to it makes them
    /// (`gpu_left`). False, with nothing recorded, where it can't be: the
    /// buffers' reads don't get the renderer's pictures of them
    /// (`voodoo_readback=exact`), the auxiliary buffer may hold alpha,
    /// which the renderer keeps in its colour buffers, the software
    /// picture is wanted, or the recording has no such buffer.
    pub(crate) fn gpu_draw_buffer(
        &mut self,
        command: Command,
        buffer: Option<u32>,
        rows: (u32, u32),
    ) -> bool {
        let takes = self.gpu_reads
            && !self.gpu_alpha.get()
            && !self.software_picture
            && self.gpu.get_mut().link.is_some();
        let Some(key) = self.mirror_layout().filter(|_| takes) else {
            return false;
        };
        let (base, stride) = match buffer {
            Some(offs) if key.color.contains(&(offs * 2)) => (offs, key.stride),
            None if key.aux != NONE => (key.aux / 2, key.aux_stride),
            _ => return false,
        };
        let Some(mirror) = self.mirror.get_mut() else {
            return false;
        };
        match command {
            Command::Fill(fill) => mirror.fill(fill),
            Command::Copy(copy) => mirror.copy(copy),
            _ => return false,
        }
        self.gpu_behind.set(true);
        if self.gpu_rows.get_mut().armed() {
            let slot = self.gpu_rows_slot(buffer);
            self.gpu_rows.get_mut().mark(slot, rows);
        }
        let (start, end) = row_bytes(base, stride, rows);
        self.gpu.get_mut().tracker.drawn_in_buffer(start, end);
        self.display_dirty = true;
        true
    }

    /// The recording dropped what it had (nobody took it), and the pages
    /// of memory with it: the renderer gets them all again, but for those
    /// it has newer.
    pub(crate) fn gpu_resend_dropped(&mut self) {
        if self
            .mirror
            .get_mut()
            .as_mut()
            .is_some_and(|m| m.take_dropped())
        {
            self.gpu.get_mut().tracker.resend();
        }
    }

    /// The colour buffer of the 3D engine's at byte `at` of the memory,
    /// `stride` bytes a row, for `BlitSource::Buffer`: its word offset,
    /// if the renderer draws one there.
    pub fn gpu_color_buffer(&self, at: u32, stride: u32) -> Option<u32> {
        let key = self.mirror_layout()?;
        let linear = !key.stride.tiled && key.stride.words as u32 * 2 == stride;
        (linear && key.color.contains(&at)).then_some(at / 2)
    }

    /// A frame of the card's picture went by: pages read back long
    /// enough ago may be drawn on the GPU again.
    pub(crate) fn gpu_frame(&mut self) {
        let gpu = self.gpu.get_mut();
        gpu.frames += 1;
        if gpu.frames >= HOT_FRAMES {
            gpu.frames = 0;
            gpu.tracker.cool();
        }
    }

    /// 16-bit pixels written into buffers of the 3D engine's: each one's
    /// byte address in the memory and its value. While the renderer
    /// draws and the engine's drawing is held back, they go after that
    /// drawing in the same queue, and to the renderer's pictures of the
    /// buffers, without the software renderer drawing all that first:
    /// what reads the memory later still finds them where they belong.
    /// False, with nothing written, if any of them is not on a buffer's
    /// pixel or nothing is held back: the caller writes them into the
    /// memory itself.
    pub fn write_buffer_pixels(&mut self, pixels: &[(u32, u16)]) -> bool {
        if !self.deferring() || pixels.is_empty() {
            return false;
        }
        self.mirror_sync();
        let Some(key) = self.mirror_layout() else {
            return false;
        };
        let mut words = Vec::with_capacity(pixels.len());
        let mut placed = Vec::with_capacity(pixels.len());
        let (mut low, mut high) = (u32::MAX, 0);
        for &(at, value) in pixels {
            let Some((dest, x, y)) = (at % 2 == 0).then(|| key.locate(at / 2)).flatten() else {
                return false;
            };
            let row = u16::try_from(y).unwrap_or(u16::MAX);
            words.push(Word {
                row,
                at: at / 2,
                value,
            });
            placed.push((dest, x, y, value));
            (low, high) = (low.min(at), high.max(at + 2));
        }
        self.queue_words(&words);
        // The texture units read the same memory.
        for tmu in &mut self.tmu {
            tmu.note_written(low, high);
        }
        if let Some(mirror) = self.mirror.get_mut() {
            for (dest, x, y, value) in placed {
                mirror.pixel(dest, x, y, value, key.width);
            }
        }
        self.display_dirty = true;
        true
    }

    /// Record the dirty pages within bytes `start..end` for the renderer.
    pub(crate) fn send_pages(&mut self, start: u32, end: u32) {
        let size = self.fbi.ram.bytes() as u32;
        let runs = self.gpu.get_mut().tracker.take_dirty(start, end);
        for (a, b) in runs {
            let mut bytes = vec![0; (b - a) as usize];
            self.fbi.ram.read_bytes(a as usize, &mut bytes);
            if let Some(mirror) = self.mirror.get_mut() {
                mirror.memory(size, a, bytes);
            }
        }
    }

    /// Bytes `start..end` of the card's memory are about to be read or
    /// written here for the program: read back the pages there that only
    /// the GPU has, which turn hot.
    #[inline]
    pub(crate) fn gpu_sync(&self, start: u32, end: u32) {
        self.gpu_sync_buffers(start, end);
        if self.gpu.borrow().tracker.any_gpu() {
            self.read_back(start, end, true);
        }
    }

    /// The rows of the 3D buffers within bytes `start..end` that are
    /// behind the renderer, read back for the program (`gpu_sync`).
    #[inline]
    fn gpu_sync_buffers(&self, start: u32, end: u32) {
        if self.gpu_behind.get() && self.gpu.borrow().tracker.buffers_in(start, end) {
            self.gpu_catch_up_hot(start, end);
        }
    }

    /// `len` bytes from byte `addr` on (which wrap at the memory's end)
    /// are about to be read or written here (`gpu_sync`).
    #[inline]
    pub(crate) fn gpu_sync_bytes(&self, addr: u32, len: usize) {
        if !self.gpu.borrow().tracker.any_gpu() && !self.gpu_behind.get() {
            return;
        }
        let start = addr & self.fbi.mask;
        let end = start as u64 + len as u64;
        if end > self.fbi.mask as u64 + 1 {
            self.gpu_sync(0, self.fbi.ram.bytes() as u32);
        } else {
            self.gpu_sync(start, end as u32);
        }
    }

    /// The pages within bytes `start..end` that only the GPU has, read
    /// back for the emulator itself (`Backlog::spare` copying texels).
    /// They don't turn hot, as the program didn't ask for them.
    pub(crate) fn gpu_sync_quiet(&self, start: u32, end: u32) {
        if self.gpu.borrow().tracker.any_gpu() {
            self.read_back(start, end, false);
        }
    }

    /// All the memory only the GPU has, read back for the emulator itself:
    /// a save state, the software picture, the renderer going. It doesn't
    /// turn hot, as the program didn't ask for it.
    pub(crate) fn gpu_sync_all(&self) {
        self.gpu_catch_up();
        if self.gpu.borrow().tracker.any_gpu() {
            self.read_back(0, self.fbi.ram.bytes() as u32, false);
        }
    }

    /// Read back the pages within bytes `start..end` that only the GPU
    /// has, hot from now on if `hot`.
    #[cold]
    fn read_back(&self, start: u32, end: u32, hot: bool) {
        let mut gpu = self.gpu.borrow_mut();
        let gpu = &mut *gpu;
        let runs = gpu.tracker.take_gpu(start, end, hot);
        let mut bytes = Vec::new();
        for (a, b) in runs {
            bytes.resize((b - a) as usize, 0);
            let read = gpu
                .link
                .as_mut()
                .is_some_and(|link| link.read(a, &mut bytes));
            if read {
                self.fbi.ram.write_bytes(a as usize, &bytes);
                gpu.read_back += ((b - a) / PAGE) as u64;
            } else if !std::mem::replace(&mut gpu.lost, true) {
                // Without the renderer, its pages are lost: the memory
                // keeps what it had before they were drawn.
                eprintln!("[3DFX] the OpenGL renderer went with pages only it had");
            }
        }
    }

    /// For `/api/status`: the 2D operations done on the GPU, and the
    /// pages read back from it.
    pub(crate) fn gpu_describe(&self) -> serde_json::Value {
        let gpu = self.gpu.borrow();
        serde_json::json!({
            "linked": gpu.link.is_some(),
            "blits": gpu.blits,
            "engine_commands": gpu.engine,
            "engine_drawn": gpu.engine_counts.drawn,
            "engine_formats": gpu.engine_counts.formats,
            "engine_left_to_cpu": gpu.engine_counts.left,
            "left_to_cpu": gpu.hot,
            "pages_read_back": gpu.read_back,
        })
    }
}

/// For the tests: what the OpenGL renderer keeps of the card, and does
/// with the commands it gets, at the card's resolution: its copy of the
/// memory, and its pictures of the colour buffers (5-6-5) and of the
/// depth buffer. It draws no triangles: a draw gives every pixel of its
/// box a value of its own, which a read back then shows.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Model {
    pub memory: Vec<u8>,
    width: u32,
    height: u32,
    colour: std::collections::HashMap<u32, Vec<u16>>,
    depth: Option<Vec<u16>>,
    draws: u32,
}

#[cfg(test)]
impl Model {
    /// Pixel (`x`, `y`) of colour buffer `dest` (its word offset), or
    /// with None of the depth buffer.
    pub fn pixel(&self, dest: Option<u32>, x: u32, y: u32) -> u16 {
        let buffer = match dest {
            Some(dest) => &self.colour[&dest],
            None => self.depth.as_ref().unwrap(),
        };
        buffer[(y * self.width + x) as usize]
    }

    /// Do `commands`, as `VoodooGl::run_commands` does.
    pub fn apply(&mut self, commands: &[Command]) {
        use super::mirror::Convert;
        for command in commands {
            let (w, h) = (self.width, self.height);
            match command {
                Command::Resync(snapshot) => {
                    let layout = &snapshot.layout;
                    (self.width, self.height) = (layout.width, layout.height);
                    let colour = layout
                        .color
                        .iter()
                        .copied()
                        .zip(snapshot.color.iter().cloned());
                    self.colour = colour.collect();
                    self.depth = snapshot.aux.clone();
                }
                Command::Draw(draw) => {
                    self.draws += 1;
                    let xs = draw.vertices.iter().map(|v| v.pos[0]);
                    let ys = draw.vertices.iter().map(|v| v.pos[1]);
                    let (x0, x1) = (xs.clone().fold(f32::MAX, f32::min), xs.fold(0.0, f32::max));
                    let (y0, y1) = (ys.clone().fold(f32::MAX, f32::min), ys.fold(0.0, f32::max));
                    let clamp = |v: f32, most: u32| (v.max(0.0) as u32).min(most);
                    let (x0, x1, y0, y1) = (clamp(x0, w), clamp(x1, w), clamp(y0, h), clamp(y1, h));
                    let draws = self.draws;
                    if let Some(buffer) = self.colour.get_mut(&draw.state.dest) {
                        for y in y0..y1 {
                            for x in x0..x1 {
                                let v = (draws * 40503 + x * 31 + y * 1009) as u16;
                                buffer[(y * w + x) as usize] = v;
                            }
                        }
                    }
                    if let Some(depth) = self
                        .depth
                        .as_mut()
                        .filter(|_| draw.state.fbz_mode & 1 << 10 != 0)
                    {
                        for y in y0..y1 {
                            for x in x0..x1 {
                                depth[(y * w + x) as usize] = draws as u16;
                            }
                        }
                    }
                }
                Command::Fill(fill) => {
                    let [l, r, t, b] = fill.rect;
                    let (l, r, t, b) = (l.min(w), r.min(w), t.min(h), b.min(h));
                    let dest = if self.colour.contains_key(&fill.dest) {
                        Some(fill.dest)
                    } else {
                        self.colour.keys().next().copied()
                    };
                    for y in t..b {
                        for x in l..r {
                            let at = (y * w + x) as usize;
                            if let (Some(c), Some(dest)) = (fill.color, dest) {
                                let v = match &fill.pattern {
                                    Some(p) => p[(y as usize & 3) * 4 + (x as usize & 3)],
                                    None => narrow(c) as u16,
                                };
                                self.colour.get_mut(&dest).unwrap()[at] = v;
                            }
                            if let (Some(aux), Some(depth), false) =
                                (fill.aux, self.depth.as_mut(), fill.alpha_planes)
                            {
                                depth[at] = aux;
                            }
                        }
                    }
                }
                Command::Pixels(p) => {
                    let buffer = match p.dest {
                        Some(dest) => self.colour.get_mut(&dest),
                        None => self.depth.as_mut(),
                    };
                    if let Some(buffer) = buffer {
                        let at = (p.y * w + p.x) as usize;
                        buffer[at..at + p.values.len()].copy_from_slice(&p.values);
                    }
                }
                Command::Texture(_) | Command::FreeTexture(_) => {}
                Command::Memory { size, at, bytes } => {
                    if self.memory.len() < *size as usize {
                        self.memory.resize(*size as usize, 0);
                    }
                    let at = *at as usize;
                    self.memory[at..at + bytes.len()].copy_from_slice(bytes);
                }
                Command::Copy(c) => {
                    let [l, r, t, b] = c.rect;
                    let Some(from) = self.colour.get(&c.from).cloned() else {
                        continue;
                    };
                    let Some(to) = self.colour.get_mut(&c.to) else {
                        continue;
                    };
                    for y in t..b {
                        for x in l..r {
                            let (sx, sy) = (c.src[0] + x - l, c.src[1] + y - t);
                            to[(y * w + x) as usize] = from[(sy * w + sx) as usize];
                        }
                    }
                }
                Command::Blit(blit) => {
                    // As blit.frag draws it, from the memory as it was.
                    let before = self.memory.clone();
                    let len = before.len();
                    let address =
                        |p: &Plane, x: i32, y: i32| p.at(x as u32, y as u32) as usize % len;
                    let pixel = |a: usize, bpp: u32| {
                        (0..bpp as usize)
                            .fold(0u32, |v, i| v | (before[(a + i) % len] as u32) << (8 * i))
                    };
                    let keyed = |v: u32, k: &super::mirror::Key| {
                        if k.masks == [0; 3] {
                            return v >= k.low && v <= k.high;
                        }
                        k.masks
                            .iter()
                            .all(|&m| m == 0 || ((v & m) >= (k.low & m) && (v & m) <= (k.high & m)))
                    };
                    let [ax, ay, ar, ab] = blit.area;
                    for r in &blit.rects {
                        for y in r[1].max(0)..r[3] {
                            for x in r[0].max(0)..r[2] {
                                let mut value = 0;
                                let src = match &blit.src {
                                    BlitSource::Fill(v) => {
                                        value = *v;
                                        None
                                    }
                                    BlitSource::Memory { rect, .. }
                                    | BlitSource::Buffer { rect, .. } => Some(*rect),
                                };
                                if let Some([sl, st, sr, sb]) = src {
                                    let (dw, dh, sw, sh) = (ar - ax, ab - ay, sr - sl, sb - st);
                                    let (mut u, mut v) = (x - ax, y - ay);
                                    if blit.mirror[0] {
                                        u = dw - 1 - u;
                                    }
                                    if blit.mirror[1] {
                                        v = dh - 1 - v;
                                    }
                                    let (sx, sy) = (sl + u * sw / dw, st + v * sh / dh);
                                    value = match &blit.src {
                                        BlitSource::Memory { plane, .. } => {
                                            pixel(address(plane, sx, sy), plane.bpp)
                                        }
                                        BlitSource::Buffer { buffer, .. } => {
                                            let b = &self.colour[buffer];
                                            let (sx, sy) = (
                                                sx.clamp(0, w as i32 - 1),
                                                sy.clamp(0, h as i32 - 1),
                                            );
                                            super::blit::rgb888(
                                                b[(sy as u32 * w + sx as u32) as usize],
                                            )
                                        }
                                        BlitSource::Fill(_) => unreachable!(),
                                    };
                                    if blit.src_key.as_ref().is_some_and(|k| keyed(value, k)) {
                                        continue;
                                    }
                                    value = match blit.convert {
                                        Convert::None => value,
                                        Convert::Widen => super::blit::rgb888(value as u16),
                                        Convert::Narrow => narrow(value),
                                        Convert::ClearTop => value & 0xFF_FFFF,
                                    };
                                }
                                let d = &blit.dst;
                                let at = address(d, x, y);
                                if blit
                                    .dst_key
                                    .as_ref()
                                    .is_some_and(|k| !keyed(pixel(at, d.bpp), k))
                                {
                                    continue;
                                }
                                for i in 0..d.bpp as usize {
                                    self.memory[(at + i) % len] = (value >> (8 * i)) as u8;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 8-8-8 as 5-6-5, each channel's top bits.
#[cfg(test)]
fn narrow(p: u32) -> u32 {
    ((p >> 19) & 31) << 11 | ((p >> 10) & 63) << 5 | ((p >> 3) & 31)
}

/// For the tests: a renderer that does what `Model` says, and keeps a
/// copy of the card's memory to do 2D operations in.
#[cfg(test)]
pub(crate) struct ModelRenderer(pub std::rc::Rc<std::cell::RefCell<Model>>);

#[cfg(test)]
impl GpuLink for ModelRenderer {
    fn ready(&self) -> bool {
        true
    }

    fn run(&mut self, commands: Vec<Command>) {
        self.0.borrow_mut().apply(&commands);
    }

    fn read(&mut self, at: u32, out: &mut [u8]) -> bool {
        let model = self.0.borrow();
        let at = at as usize;
        let Some(bytes) = model.memory.get(at..at + out.len()) else {
            return false;
        };
        out.copy_from_slice(bytes);
        true
    }

    fn read_buffer(
        &mut self,
        dest: Option<u32>,
        (top, bottom): (u32, u32),
        out: &mut [u16],
    ) -> bool {
        let model = self.0.borrow();
        let w = model.width as usize;
        let buffer = match dest {
            Some(dest) => model.colour.get(&dest),
            None => model.depth.as_ref(),
        };
        let Some(buffer) = buffer.filter(|_| out.len() == w * (bottom - top) as usize) else {
            return false;
        };
        out.copy_from_slice(&buffer[top as usize * w..bottom as usize * w]);
        true
    }
}

/// For the tests: an OpenGL renderer 640 pixels wide that drew each
/// colour buffer in one colour and the depth buffer at one value, and
/// notes what it is asked.
#[cfg(test)]
pub(crate) struct TestRenderer(pub std::rc::Rc<std::cell::RefCell<Vec<String>>>);

#[cfg(test)]
impl GpuLink for TestRenderer {
    fn ready(&self) -> bool {
        false
    }

    fn run(&mut self, commands: Vec<Command>) {
        self.0.borrow_mut().push(format!("run {}", commands.len()));
    }

    fn read(&mut self, _at: u32, _out: &mut [u8]) -> bool {
        false
    }

    fn read_buffer(&mut self, dest: Option<u32>, (top, bottom): (u32, u32), out: &mut [u16]) -> bool {
        assert_eq!(out.len(), 640 * (bottom - top) as usize);
        out.fill(dest.map_or(0x7777, |d| 0x1234 ^ d as u16));
        self.0
            .borrow_mut()
            .push(format!("read {dest:?} {top}..{bottom}"));
        true
    }
}

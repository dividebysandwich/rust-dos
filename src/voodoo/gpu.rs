//! 2D operations a Banshee's or Voodoo3's OpenGL renderer does in its
//! copy of the card's memory (`pages`), for the built-in Windows'
//! DirectDraw and Direct3D: a blit, a fill, a window's Present. The card
//! records the operation for the renderer and leaves its own memory as it
//! was; the pages the operation drew into are then the GPU's, and
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

use super::Voodoo;
use super::backlog::Access;
use super::mirror::{Blit, BlitSource, Command};
use super::pages::{PAGE, Pages};
use super::workers::{Job, Word};

/// Rows `top..bottom` of a buffer the renderer drew and the memory
/// doesn't have yet: the buffer by its key in the recording (a colour
/// buffer's word offset, None for the auxiliary buffer), and the rows.
pub(crate) type Stale = (Option<u32>, (u32, u32));

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
    pub(super) pages: Pages,
    link: Option<Box<dyn GpuLink>>,
    /// Pages read back from the GPU, for `/api/status`.
    read_back: u64,
    /// 2D operations done on the GPU.
    blits: u64,
    /// 2D operations left to the CPU as the program reads or writes
    /// where they draw (`Pages`' hot pages).
    hot: u64,
    /// Frames since the hot pages last cooled.
    frames: u32,
    /// Pages only the GPU had were lost, which the log said.
    lost: bool,
}

/// Frames a page read back from the GPU stays hot: about a second.
const HOT_FRAMES: u32 = 60;

impl Gpu {
    pub fn new(bytes: usize) -> Self {
        Self {
            pages: Pages::new(bytes),
            ..Self::default()
        }
    }
}

impl Voodoo {
    /// Give the card the renderer to do its 2D operations with, or take
    /// it away (the renderer stops): what only the GPU had is read back
    /// first.
    pub fn set_gpu_link(&mut self, link: Option<Box<dyn GpuLink>>) {
        self.gpu_catch_up();
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
    /// planes, which the renderer keeps in its colour buffers. The
    /// Banshee's 3D engine draws into memory its 2D side and its texture
    /// units read, and stays exact.
    pub(crate) fn gpu_takes(&self, job: &Job, access: &Access) -> bool {
        if let Job::Triangle { st, .. } | Job::Fastfill { st, .. } = job
            && st.fbz_mode & (1 << 18) != 0
        {
            self.gpu_alpha.set(true);
        }
        self.gpu_reads
            && self.banshee.is_none()
            && !self.gpu_alpha.get()
            && !access.pinned
            && !access.kept
            && self.gpu.borrow().link.is_some()
    }

    /// The buffers a job left to the renderer draws into are behind it
    /// in memory: its colour buffer and the auxiliary buffer, all their
    /// rows.
    pub(crate) fn gpu_left(&self, job: &Job) {
        self.gpu_behind.set(true);
        self.gpu_only.set(self.gpu_only.get() + 1);
        let height = self.fbi.height;
        let mut stale = self.gpu_stale.borrow_mut();
        // The rows join those of the buffer behind that they meet.
        let mut mark = |key: Option<u32>, (mut top, mut bottom): (u32, u32)| {
            stale.retain(|&(k, (a, b))| {
                let meets = k == key && a <= bottom && top <= b;
                if meets {
                    (top, bottom) = (top.min(a), bottom.max(b));
                }
                !meets
            });
            stale.push((key, (top, bottom)));
        };
        match job {
            Job::Triangle { st, .. } | Job::Fastfill { st, .. } => {
                mark(Some(st.dest as u32), (0, height));
                mark(None, (0, height));
            }
            // A Voodoo2's SGRAM fill of a buffer's rows (`sgram_fill_rows`).
            Job::Rows { base, rows, .. } => {
                let aux = self.fbi.auxoffs != super::NONE && *base == self.fbi.auxoffs as usize / 2;
                mark((!aux).then_some(*base as u32), *rows);
            }
            _ => {
                for offs in self.fbi.rgboffs.iter().filter(|&&o| o != super::NONE) {
                    mark(Some(offs / 2), (0, height));
                }
                mark(None, (0, height));
            }
        }
    }

    /// Bring the buffers in memory up to the renderer's pictures of them,
    /// if jobs were left to it: what it has recorded goes to it first,
    /// and it reads the buffers back.
    pub(crate) fn gpu_catch_up(&self) {
        self.gpu_catch_up_in(None);
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
        if !self.gpu_behind.get() {
            return;
        }
        self.catch_up_workers();
        let forget = || {
            self.gpu_behind.set(false);
            self.gpu_stale.borrow_mut().clear();
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
        let Some(link) = gpu.link.as_mut() else {
            if !std::mem::replace(&mut gpu.lost, true) {
                eprintln!("[3DFX] the OpenGL renderer went with the buffers only it had");
            }
            forget();
            return;
        };
        link.run(commands);
        // The rows asked for that are behind, buffer by buffer.
        let mut stale = self.gpu_stale.borrow_mut();
        let asked = match range {
            None => stale
                .iter()
                .map(|&(k, (top, bottom))| (k, top, bottom))
                .collect(),
            Some((start, end)) => key.rows_in(start, end),
        };
        let layout = key.layout();
        let width = layout.width as usize;
        let ram = &self.fbi.ram;
        let mut read = false;
        for (buffer, top, bottom) in asked {
            let (base, stride) = match (buffer, layout.aux) {
                (Some(offs), _) => (offs, layout.stride),
                (None, Some(offs)) => (offs, layout.aux_stride),
                (None, None) => {
                    stale.retain(|&(k, _)| k.is_some());
                    continue;
                }
            };
            // The rows of it that are behind, and what is left of them.
            let mut left = Vec::new();
            stale.retain(|&(k, (s0, s1))| {
                let (top, bottom) = (top.max(s0), bottom.min(s1).min(layout.height));
                if k != buffer || top >= bottom {
                    return true;
                }
                let mut pixels = vec![0u16; width * (bottom - top) as usize];
                if link.read_buffer(buffer, (top, bottom), &mut pixels) {
                    read = true;
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
                left.extend(
                    [(k, (s0, top)), (k, (bottom, s1))]
                        .into_iter()
                        .filter(|(_, (a, b))| a < b),
                );
                false
            });
            stale.extend(left);
        }
        if read {
            self.gpu_read_backs.set(self.gpu_read_backs.get() + 1);
        }
        self.gpu_behind.set(!stale.is_empty());
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
        // without a word to `pages`: a page the blit draws into or reads
        // must hold none of them (`mirror_sync` reads back the pages
        // under buffers that move).
        // Where the program reads or writes the result, a blit on the CPU
        // costs less than one on the GPU read back.
        if self.gpu.get_mut().pages.hot_in(start, end) {
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
        gpu.pages.drawn_on_gpu(start, end);
        gpu.blits += 1;
        // A software picture, where one is wanted, is drawn again.
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
            self.gpu.get_mut().pages.resend();
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
            gpu.pages.cool();
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
        let runs = self.gpu.get_mut().pages.take_dirty(start, end);
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
        if self.gpu.borrow().pages.any_gpu() {
            self.read_back(start, end, true);
        }
    }

    /// `len` bytes from byte `addr` on (which wrap at the memory's end)
    /// are about to be read or written here (`gpu_sync`).
    #[inline]
    pub(crate) fn gpu_sync_bytes(&self, addr: u32, len: usize) {
        if !self.gpu.borrow().pages.any_gpu() {
            return;
        }
        let start = addr & self.fbi.mask;
        let end = start as u64 + len as u64;
        if end > self.fbi.mask as u64 + 1 {
            self.read_back(0, self.fbi.ram.bytes() as u32, true);
        } else {
            self.read_back(start, end as u32, true);
        }
    }

    /// The pages within bytes `start..end` that only the GPU has, read
    /// back for the emulator itself (`Backlog::spare` copying texels).
    /// They don't turn hot, as the program didn't ask for them.
    pub(crate) fn gpu_sync_quiet(&self, start: u32, end: u32) {
        if self.gpu.borrow().pages.any_gpu() {
            self.read_back(start, end, false);
        }
    }

    /// All the pages only the GPU has, read back for the emulator itself:
    /// a save state, the software picture, the renderer going. They don't
    /// turn hot, as the program didn't ask for them.
    pub(crate) fn gpu_sync_all(&self) {
        if self.gpu.borrow().pages.any_gpu() {
            self.read_back(0, self.fbi.ram.bytes() as u32, false);
        }
    }

    /// Read back the pages within bytes `start..end` that only the GPU
    /// has, hot from now on if `hot`.
    #[cold]
    fn read_back(&self, start: u32, end: u32, hot: bool) {
        let mut gpu = self.gpu.borrow_mut();
        let gpu = &mut *gpu;
        let runs = gpu.pages.take_gpu(start, end, hot);
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
            "left_to_cpu": gpu.hot,
            "pages_read_back": gpu.read_back,
        })
    }
}

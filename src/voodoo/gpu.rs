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
use super::workers::Word;
use super::mirror::{Blit, BlitSource, Command};
use super::pages::{PAGE, Pages};

/// The OpenGL renderer, as the card reaches it between frames.
pub trait GpuLink {
    /// Do `commands`, the recording so far, now.
    fn run(&mut self, commands: Vec<Command>);

    /// Bytes of the renderer's copy of the card's memory from byte `at`
    /// on, into `out`.
    fn read(&mut self, at: u32, out: &mut [u8]);
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
        self.gpu_sync_all();
        let gpu = self.gpu.get_mut();
        gpu.link = link;
    }

    /// Whether the card has a link to the renderer (`set_gpu_link`).
    pub fn gpu_linked(&self) -> bool {
        self.gpu.borrow().link.is_some()
    }

    /// Whether 2D operations may go to the GPU: the renderer draws the
    /// picture and gave the card a link to it.
    pub fn gpu_ready(&self) -> bool {
        self.mirror.is_some() && self.banshee.is_some() && self.gpu.borrow().link.is_some()
    }

    /// Do `blit` on the GPU, if it can be: the renderer has a link, and
    /// the destination is no buffer of the 3D engine's, which draws into
    /// the card's memory itself. False leaves it to the caller, in
    /// software.
    pub fn gpu_blit(&mut self, blit: Blit) -> bool {
        if !self.gpu_ready() || blit.rects.is_empty() {
            return false;
        }
        let (start, end) = blit.dst_span();
        if start >= end || end as usize > self.fbi.ram.bytes() {
            return false;
        }
        let key = self.mirror.as_ref().and_then(|m| m.layout().copied());
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
                if !key.is_some_and(|k| k.layout().color.contains(buffer)) {
                    return false;
                }
            }
            BlitSource::Fill(_) => {}
        }
        // The renderer gets the pages the blit reads and draws into that
        // it lacks first, then the blit after everything recorded.
        let mut spans = vec![(start, end)];
        spans.extend(blit.src_span());
        for (a, b) in spans {
            self.send_pages(a, b);
        }
        let Some(mirror) = self.mirror.as_mut() else {
            return false;
        };
        mirror.blit(blit);
        let commands = mirror.take_commands();
        let gpu = self.gpu.get_mut();
        if let Some(link) = gpu.link.as_mut() {
            link.run(commands);
        }
        gpu.pages.drawn_on_gpu(start, end);
        gpu.blits += 1;
        true
    }

    /// The colour buffer of the 3D engine's at byte `at` of the memory,
    /// `stride` bytes a row, for `BlitSource::Buffer`: its word offset,
    /// if the renderer draws one there.
    pub fn gpu_color_buffer(&self, at: u32, stride: u32) -> Option<u32> {
        let key = self.mirror.as_ref()?.layout()?;
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
        let Some(key) = self.mirror.as_ref().and_then(|m| m.layout().copied()) else {
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
            words.push(Word { row, at: at / 2, value });
            placed.push((dest, x, y, value));
            (low, high) = (low.min(at), high.max(at + 2));
        }
        self.queue_words(&words);
        // The texture units read the same memory.
        for tmu in &mut self.tmu {
            tmu.note_written(low, high);
        }
        if let Some(mirror) = &mut self.mirror {
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
            if let Some(mirror) = &mut self.mirror {
                mirror.memory(size, a, bytes);
            }
        }
    }

    /// Bytes `start..end` of the card's memory are about to be read or
    /// written here: read back the pages there that only the GPU has.
    #[inline]
    pub(crate) fn gpu_sync(&self, start: u32, end: u32) {
        if self.gpu.borrow().pages.any_gpu() {
            self.read_back(start, end);
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
            self.gpu_sync_all();
        } else {
            self.read_back(start, end as u32);
        }
    }

    /// All the pages only the GPU has, read back: before a save state,
    /// or when the renderer goes.
    pub(crate) fn gpu_sync_all(&self) {
        self.gpu_sync(0, self.fbi.ram.bytes() as u32);
    }

    #[cold]
    fn read_back(&self, start: u32, end: u32) {
        let mut gpu = self.gpu.borrow_mut();
        let runs = gpu.pages.take_gpu(start, end);
        if runs.is_empty() {
            return;
        }
        let Some(link) = gpu.link.as_mut() else {
            // Without the renderer, its pages are lost: the memory keeps
            // what it had before they were drawn.
            self.log_once_gpu_lost();
            return;
        };
        let mut bytes = Vec::new();
        for &(a, b) in &runs {
            bytes.resize((b - a) as usize, 0);
            link.read(a, &mut bytes);
            self.fbi.ram.write_bytes(a as usize, &bytes);
        }
        let pages: u32 = runs.iter().map(|&(a, b)| (b - a) / PAGE).sum();
        gpu.read_back += pages as u64;
    }

    fn log_once_gpu_lost(&self) {
        eprintln!("[3DFX] the OpenGL renderer went with pages only it had");
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

//! Between the card and a host renderer ("Between the card and the
//! host renderer" in `docs/toadforce.md`).
//!
//! The card draws level 1's primitives in software by itself
//! (`raster`). A host renderer, the display's OpenGL one, can be
//! attached to draw them instead (`Host`). The card and the display
//! run on one thread, so the card calls the host directly.
//!
//! - **What the host gets.** The card's renderer hands it the queued
//!   triangles of a run of commands (`Batch`): for each lowered state
//!   the triangles as screen vertices, after the card's transform,
//!   lighting, clipping and culling. A triangle the card culls, or one
//!   with no area, never reaches the host. The clip rectangle stays in
//!   the state (`DrawState::clip`), for the host to cut the pixels by.
//!   Slot `n`'s coordinates are in `tex[n]` of every vertex, whichever
//!   set of the program's vertex they came from.
//! - **Points and lines** reach the host as triangles too, so the
//!   card's rules decide their pixels and the host's own rules for
//!   lines and points are never used. A point is the two triangles
//!   of its square. A line is a square for each run of its pixels in
//!   a row, with the line's values at the square's corners
//!   (`raster::setup::Prim::squares`): a host that fills a triangle
//!   by the pixel centres inside it, with the top and left edges its
//!   own, covers the line's pixels and no other. A triangle drawn as
//!   its edges or its corners (`FILLMODE`) is those lines and points.
//! - **What the host declines.** A state `Host::declines` names and a
//!   draw that reads the surface it writes are drawn by the software
//!   renderer, in the card's order with the host's.
//! - **Who has the newer pixels.** A surface the host drew into is
//!   ahead on the host (`Gpu::ahead`), and card memory is stale there
//!   until `Gpu::settle` reads it back. The card's own work on the
//!   processor (a draw the host declined, a `BLT` it left) settles the
//!   bytes it touches first.
//! - **What the program does with a surface that is ahead.** Nothing
//!   is read back for a program that is given a surface's address, or
//!   for a driver that writes one. The card notes each byte the
//!   processor writes there (`Gpu::writing`): card memory has those,
//!   and the host the rest. Before the host next uses the surface, the
//!   written pixels go to its copy (`Host::write`), as rectangles of
//!   whole pixels. A surface written in parts of pixels, or in more
//!   rectangles than the host takes, is read back around what was
//!   written instead. A read by the program of bytes the host has
//!   (`Gpu::read`) is answered from the host's pixels, fetched once
//!   for as long as the host's copy stays as it is. So a program that
//!   locks its back buffer and writes all of it, as one with drawn
//!   backgrounds does every frame, costs one upload, and one that only
//!   reads costs the read-back a lock used to.
//! - **What the processor wrote.** `Pages` counts the writes to each
//!   page of card memory. A host keeps a `Stamp` of the bytes a copy
//!   was made from and makes the copy again when the stamp is stale.
//!   Reading a surface back writes card memory what the host's copy
//!   holds, and isn't counted, and neither are the host's own draws:
//!   a host that draws into a target drops its other copies that share
//!   a byte with it (`Host::draw`). A copy that is ahead is never made
//!   again from card memory, whatever its stamp says: a write to a
//!   neighbour in one of its pages changes the stamp and none of its
//!   pixels.
//! - **Views.** A surface with the offset, the pitch and the format of
//!   another and no more of its width and height is the top left part
//!   of it (`within`), as the depth buffer of a small render target is
//!   of the one the screen's target has. The host keeps one copy for
//!   them, of the largest it was given, and the card keeps that one in
//!   its list of what is ahead.
//! - **`CLEAR`.** The card offers the host each buffer a `CLEAR` writes
//!   (`Host::clear`). A host whose copy of the buffer is what card
//!   memory holds, or newer, writes the copy, and the buffer is ahead
//!   there. So does a host for a `CLEAR` of every pixel of the buffer,
//!   which needs nothing of what card memory holds. Otherwise the
//!   processor writes card memory.
//! - **`FILL` and `BLT`.** The card offers the host those that write a
//!   surface, or copy from one, that is ahead (`Host::fill`,
//!   `Host::blt`), and the destination is ahead after it. The others
//!   are the processor's. `BLT` copies bits, whatever format its
//!   surfaces are named with: one over the bytes of a depth buffer the
//!   host has (`alias`), as a DirectDraw blit between two z-buffers
//!   is, copies the host's depth.
//! - **The picture on the screen.** The screen asks whether the host
//!   has the surface shown (`Gpu::shows`), and draws the window from
//!   the host's copy when it has. The surface stays ahead. A frame
//!   somebody needs in card memory (a screenshot, a recording, the
//!   debugger, a deterministic run) is read back as before.
//! - **A texture that is a surface.** A draw may sample a texture one
//!   of whose images is a surface that is ahead (`image_of`): a render
//!   target sampled, or the face of a cube map drawn into. The card
//!   leaves the surface ahead, and the host fills the image from its
//!   copy.
//!
//! With no host attached none of this does anything: the pages aren't
//! counted and nothing is ever ahead.

use super::format::{Format, Rect, Surface};
use super::level1::{Target, Texture};
use super::raster::pipeline::{DrawState, SLOTS};
use super::raster::texel::Rgba;
use super::raster::vertex::ScreenVertex;
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::sync::Arc;

/// The bytes of a page of card memory, as the writes are counted.
pub const PAGE: u32 = 4096;

/// A vertex of a triangle queued for a host, laid out as a host's
/// vertex buffer has it (`BYTES` bytes with no gaps), so a run's
/// vertices go there in one copy: the position (x, y, z and 1 / w),
/// the two colours, and the coordinates of each slot, where the
/// shader looks for them.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Vertex {
    pub place: [f32; 4],
    pub diffuse: Rgba,
    pub specular: Rgba,
    pub tex: [[f32; 4]; SLOTS],
}

impl Vertex {
    /// The bytes of one.
    pub const BYTES: usize = 24 + 16 * SLOTS;

    /// `v` with the colours of `colours` (the vertex itself, or the
    /// one that gives a flat-shaded triangle its colours), each slot
    /// taking the coordinate set `sets` names for it.
    pub fn of(v: &ScreenVertex, colours: &ScreenVertex, sets: &[u8]) -> Self {
        let mut tex = [[0.0; 4]; SLOTS];
        for (slot, to) in tex.iter_mut().enumerate() {
            *to = v.tex[sets.get(slot).map_or(slot, |&set| set as usize)];
        }
        Vertex {
            place: [v.x, v.y, v.z, v.rhw],
            diffuse: colours.diffuse,
            specular: colours.specular,
            tex,
        }
    }

    /// The vertex with its coordinates in the slots' sets, for a
    /// renderer that takes screen vertices.
    #[cfg(test)]
    pub fn screen(&self) -> ScreenVertex {
        let mut v = ScreenVertex {
            x: self.place[0],
            y: self.place[1],
            z: self.place[2],
            rhw: self.place[3],
            diffuse: self.diffuse,
            specular: self.specular,
            ..ScreenVertex::default()
        };
        v.tex[..SLOTS].copy_from_slice(&self.tex);
        v
    }

    /// The bytes of `vertices`, as a vertex buffer takes them.
    pub fn bytes(vertices: &[Vertex]) -> &[u8] {
        const { assert!(size_of::<Vertex>() == Vertex::BYTES) };
        // SAFETY: a `Vertex` is `repr(C)` of `f32`s and bytes with no
        // padding (the assertion above), so every byte of the slice
        // is initialised, and a byte has no alignment to keep. The
        // bytes are those of `vertices`, borrowed as long.
        unsafe { std::slice::from_raw_parts(vertices.as_ptr().cast(), size_of_val(vertices)) }
    }
}

/// The triangles queued with one state.
pub struct Run {
    pub state: Arc<DrawState>,
    /// Their vertices in `Batch::vertices`, three a triangle. Those of
    /// a flat-shaded triangle (`Shape::flat`) all have the colours of
    /// the vertex that gives it them.
    pub vertices: Range<usize>,
}

/// What a host draws at once: runs for one render target, in the
/// card's order.
pub struct Batch<'a> {
    pub target: Target,
    pub runs: &'a [Run],
    pub vertices: &'a [Vertex],
}

/// What a `CLEAR` writes to one buffer of the render target.
#[derive(Clone, Copy)]
pub struct Clear<'a> {
    /// The colour buffer or the depth buffer.
    pub surface: Surface,
    pub rects: &'a [Rect],
    /// The pixel value, and the bits of a pixel it replaces: all of
    /// them, or the depth or the stencil alone of a `D24S8` pixel.
    pub value: u32,
    pub mask: u32,
}

impl Clear<'_> {
    /// Whether every bit of a pixel is replaced.
    pub fn whole(&self) -> bool {
        self.mask == self.surface.format.defined_bits() || self.mask == 0xFFFF_FFFF
    }

    /// Write it to `memory`, where the surface is.
    pub fn write(&self, memory: &mut [u8]) {
        let s = &self.surface;
        for rect in self.rects {
            if self.whole() {
                super::blit::fill(memory, s, rect, self.value);
                continue;
            }
            // A part of a pixel's bits is of a 32-bit depth format.
            let len = rect.width as usize * 4;
            for y in rect.top..rect.top + rect.height {
                let at = s.at(rect.left, y);
                for pixel in memory[at..at + len].as_chunks_mut::<4>().0 {
                    let old = u32::from_le_bytes(*pixel);
                    *pixel = (old & !self.mask | self.value & self.mask).to_le_bytes();
                }
            }
        }
    }
}

/// A renderer on the host that draws the card's triangles.
pub trait Host {
    /// What the debugger's status calls it.
    fn name(&self) -> &str;

    /// Why it doesn't draw triangles with `state`, in the words the
    /// status counts such draws under, or None for a state it draws.
    /// A draw it refuses goes to the software renderer.
    fn declines(&self, state: &DrawState) -> Option<Decline>;

    /// Draw `batch`. `memory` is card memory, for the textures and for
    /// a target the host has no copy of or a stale one, by `pages`.
    /// The card has read back whatever of it was ahead on the host,
    /// apart from the batch's own target, the surfaces the target's
    /// are views of or that are views of them (`within`), and the
    /// surfaces that are images of the batch's textures (`image_of`).
    ///
    /// The host's copy of a target's surface is the one of the largest
    /// surface it was given there: a copy of a smaller one is made
    /// larger, with its pixels kept if it was ahead. A copy that is
    /// ahead is drawn into as it is, and one that isn't is made again
    /// from `memory` when its stamp is stale. An image of a texture
    /// that is a surface ahead on the host is filled from the host's
    /// copy, with what a read-back and a decode of card memory would
    /// give.
    ///
    /// The host's draws aren't counted in `pages`, and neither is a
    /// read-back. So the host drops every copy it has of other bytes
    /// that share one with the target's colour or depth buffer: a
    /// surface over the same memory with another size or format, or a
    /// texture there that isn't the target exactly. The card reads the
    /// target back before anything uses those bytes again, and the
    /// host copies them anew.
    ///
    /// Gives the surfaces of the copies drawn into, which are ahead
    /// after it: the target's, or larger ones they are views of. The
    /// card reads back all of such a copy, not the part a batch drew.
    fn draw(&mut self, batch: &Batch, memory: &[u8], pages: &Pages) -> Vec<Surface>;

    /// Write `clear` to the host's copy of its surface: the surface is
    /// then ahead on the host, and the host drops its other copies
    /// that share a byte with it, as for a draw. The host takes a
    /// `CLEAR` when its copy is ahead or is what card memory holds (by
    /// `pages`), and one of every bit of every pixel of the copy
    /// whatever the copy holds, or with no copy yet. The surface of
    /// the copy written, which is ahead after it. None otherwise, or
    /// when the host can't write those bits exactly; nothing is
    /// changed, and the processor writes card memory.
    fn clear(&mut self, clear: &Clear, memory: &[u8], pages: &Pages) -> Option<Surface>;

    /// `FILL`: write the low bytes of `value` to each pixel of `rect`
    /// of `dst`, when the host's copy of `dst` is ahead and is of a
    /// colour buffer. The surface of that copy, which is ahead after
    /// it. None when the host leaves the fill to the processor, with
    /// nothing changed.
    fn fill(
        &mut self,
        dst: &Surface,
        rect: &Rect,
        value: u32,
        memory: &[u8],
        pages: &Pages,
    ) -> Option<Surface>;

    /// `BLT`: copy `src_rect` of `src` to `dst_rect` of `dst`, when
    /// the host's copy of either is ahead. The command copies bits,
    /// and the formats it names say only how many a pixel has: the
    /// copies are the ones of the bytes the two name (`alias`), and
    /// their formats say what the bits are, so a copy between two
    /// depth buffers named as colour is one of the host's depths. A
    /// depth source card memory holds gets a copy of its own. The
    /// surface of the host's copy of `dst`, which is ahead after it.
    /// None when the host leaves the copy to the processor, with
    /// nothing changed: for a stretch, a format the host draws into no
    /// target of, pixels of two sizes, copies of two kinds (colour
    /// and depth, or depths of two layouts), a colour key on depths,
    /// a colour key with a source the host has ahead or
    /// with more runs of pixels to copy than it writes one by one
    /// (`KEYED_RUNS`), and a source the host has ahead that
    /// shares a byte with `dst` without being a view of it or it of
    /// the source (`related`), or that is such a view, with the two
    /// rectangles sharing a pixel.
    #[allow(clippy::too_many_arguments)]
    fn blt(
        &mut self,
        dst: &Surface,
        dst_rect: &Rect,
        src: &Surface,
        src_rect: &Rect,
        key: Option<u32>,
        memory: &[u8],
        pages: &Pages,
    ) -> Option<Surface>;

    /// Copy `rects` of `surface` from `memory`, card memory, to the
    /// host's copy of it, which is ahead and stays so: the processor
    /// wrote those pixels, and the copy has the others. False when the
    /// host has no such copy or can't write it, with nothing changed:
    /// the card then reads the surface back around what was written.
    fn write(&mut self, surface: &Surface, rects: &[Rect], memory: &[u8]) -> bool;

    /// Write the pixels of `surface`, a colour or a depth buffer the
    /// host drew into, to `memory`. False when the host has no such
    /// surface, and `memory` is as it was. With `pages`, `memory` is
    /// card memory, which then holds what the copy does: the copy is
    /// no longer ahead. Without, it is a copy of card memory for a
    /// saved state, and the host's copy stays ahead.
    fn read_back(&mut self, surface: &Surface, memory: &mut [u8], pages: Option<&Pages>) -> bool;

    /// Whether the host's copy has every pixel of `surface` as it is
    /// now: it is ahead, or what card memory holds (by `pages`). The
    /// screen then shows the surface from the copy, with nothing read
    /// back. A host that can't show its copies has none.
    fn shows(&mut self, surface: &Surface, pages: &Pages) -> bool {
        let _ = (surface, pages);
        false
    }

    /// Drop every copy: card memory is all there is, as after a saved
    /// state was loaded.
    fn forget(&mut self);

    /// Counts of its own work for the debugger's status, if it keeps
    /// any.
    fn status(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
}

/// The writes the processor made to each page of card memory, while a
/// host is attached.
#[derive(Debug, Default)]
pub struct Pages {
    /// Counted up with every write.
    generation: u64,
    counts: Vec<u64>,
}

/// What `Pages` held for a range of bytes when a copy was made of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    generation: u64,
    sum: u64,
}

impl Pages {
    /// Counters for `memory` bytes of card memory.
    fn new(memory: u32) -> Self {
        Pages {
            generation: 1,
            counts: vec![0; memory.div_ceil(PAGE) as usize],
        }
    }

    /// The pages of the bytes `range`, first to after last.
    fn pages(&self, range: (u32, u32)) -> Range<usize> {
        let first = (range.0 / PAGE) as usize;
        let end = (range.1.div_ceil(PAGE) as usize).min(self.counts.len());
        first.min(end)..end
    }

    /// The processor wrote `len` bytes at `offset`.
    #[inline]
    pub fn wrote(&mut self, offset: u32, len: u32) {
        if self.counts.is_empty() || len == 0 {
            return;
        }
        self.generation += 1;
        let pages = self.pages((offset, offset.saturating_add(len)));
        for count in &mut self.counts[pages] {
            *count += 1;
        }
    }

    /// The stamp of the bytes `range` as they are now.
    pub fn stamp(&self, range: (u32, u32)) -> Stamp {
        Stamp {
            generation: self.generation,
            sum: self.counts[self.pages(range)].iter().sum(),
        }
    }

    /// Whether the bytes `range` were written since `stamp` was taken
    /// of them. A stamp that is still good is brought up to now, so
    /// the next look at an unchanged memory is one comparison.
    pub fn changed(&self, range: (u32, u32), stamp: &mut Stamp) -> bool {
        if stamp.generation == self.generation {
            return false;
        }
        let now = self.stamp(range);
        if now.sum != stamp.sum {
            return true;
        }
        *stamp = now;
        false
    }
}

/// The bytes of `surface` in card memory, first to after last.
pub fn range(surface: &Surface) -> (u32, u32) {
    (surface.offset, surface.end().min(u32::MAX as u64) as u32)
}

fn overlap(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// A format as its pixels are kept: `X8R8G8B8` and `A8R8G8B8` name the
/// same four bytes, and a host keeps all four for either.
fn kept(format: Format) -> Format {
    match format {
        Format::X8R8G8B8 => Format::A8R8G8B8,
        format => format,
    }
}

/// Whether `view` is the top left part of `whole`, or all of it: they
/// start at one byte with one pitch, their pixels are kept the same
/// way, and `view` is no wider and no higher.
pub fn within(view: &Surface, whole: &Surface) -> bool {
    view.offset == whole.offset
        && view.pitch == whole.pitch
        && kept(view.format) == kept(whole.format)
        && view.width <= whole.width
        && view.height <= whole.height
}

/// Whether one of the two is the other's bytes (`alias`).
fn same_bytes(a: &Surface, b: &Surface) -> bool {
    alias(a, b) || alias(b, a)
}

/// Whether one of the two is a view of the other.
pub fn related(a: &Surface, b: &Surface) -> bool {
    within(a, b) || within(b, a)
}

/// Whether `named` is the bytes of the top left part of `whole`, or of
/// all of it, whatever format either names them with: `within`, for
/// pixels of one size. A `BLT` copies bits, so one that names a depth
/// buffer's bytes as colour copies depth.
pub fn alias(named: &Surface, whole: &Surface) -> bool {
    named.offset == whole.offset
        && named.pitch == whole.pitch
        && named.format.bytes() == whole.format.bytes()
        && named.width <= whole.width
        && named.height <= whole.height
}

/// The level and the face of `texture` that `surface` is, when it is
/// one exactly: the image starts where the surface does, with its
/// pitch and size, in a format a host has colour buffers of, and the
/// texture has no colour key, whose test is of card memory's bits.
pub fn image_of(texture: &Texture, surface: &Surface) -> Option<(u32, u32)> {
    let colour = matches!(
        texture.format,
        Format::R5G6B5 | Format::X8R8G8B8 | Format::A8R8G8B8
    );
    if !colour || texture.key.is_some() || kept(texture.format) != kept(surface.format) {
        return None;
    }
    (0..texture.levels)
        .filter(|&level| texture.level_size(level) == (surface.width, surface.height))
        .flat_map(|level| (0..texture.faces()).map(move |face| (level, face)))
        .find(|&(level, face)| texture.image(level, face) == (surface.offset, surface.pitch))
}

/// Whether `texture` has a byte of `bytes`.
fn samples(texture: &Texture, bytes: (u32, u32)) -> bool {
    let mut shares = false;
    super::raster::pipeline::texture_reads(texture, |r| {
        shares |= r.0 < bytes.1 as u64 && (bytes.0 as u64) < r.1;
    });
    shares
}

/// What a surface was read back for, for the status: each read-back
/// stalls the host's GPU, and which kind a game has many of says what
/// to move to the host next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// What the program wrote of a surface the host couldn't take
    /// (`Host::write`), and whatever names nothing else.
    Program,
    /// The program read a surface it has the address of (`Lock`,
    /// `GetDC`): the host's pixels were fetched for it (`Gpu::read`).
    Lock,
    /// A driver read or wrote the bytes itself (GDI, a DirectDraw blit
    /// on the processor).
    Driver,
    /// The screen shows the surface.
    Show,
    /// `FILL` or `BLT` on the processor reads or writes it.
    Blit,
    /// `CLEAR` writes bytes it shares: on the processor, which the
    /// host declined, or on the host into another surface.
    Clear,
    /// A draw of the host's samples it, or draws into bytes it shares.
    Sampled,
    /// A draw the host declined draws into it or samples it.
    Declined,
    /// A draw's vertices or indices are in it.
    Buffers,
    /// A guest driver's write of `RING_PUT`, after which card memory
    /// holds everything.
    Ring,
    /// The renderer changed, or a recording (`capture`) takes card
    /// memory before and after a run.
    Other,
}

impl Cause {
    const NAMES: [&str; 11] = [
        "program", "lock", "driver", "show", "blit", "clear", "sampled", "declined", "buffers",
        "ring", "other",
    ];
}

/// Why a draw went to the software renderer with a host attached, for
/// the status (`declined_for`): the rasteriser's reason, then the
/// host's (`Host::declines`). The names are the debugger's API, listed
/// in `docs/llm-debugging.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// A draw that reads what it writes, drawn pixel by pixel in order.
    Serial,
    /// The host is gone, as its window and context are.
    NoHost,
    /// The host has no framebuffer the card's surfaces fit.
    HostBroken,
    /// The target's colour format isn't one the host draws into.
    TargetFormat,
    /// A stage samples a texture that isn't bound.
    NoTexture,
    /// A bump-mapped stage samples a cube map.
    BumpedCube,
    /// A colour key on a block-compressed texture.
    KeyOnBlock,
    /// One texture read by the shader on one stage and sampled by the
    /// host's filter on another.
    SampledBothWays,
    /// A first level above 0 with one filter to magnify and another to
    /// minify.
    FirstLevelTwoFilters,
    /// A mipmapped texture magnified linearly and minified by point.
    LinearMagPointMin,
    /// A colour key with linear mipmapping.
    KeyLinearMip,
    /// One texture with different sampling states on two stages.
    TwoSamplings,
    /// `SRCALPHASAT` as the destination's blend factor.
    SrcAlphaSatDestination,
    /// The state's shader didn't compile.
    ProgramFailed,
}

impl Decline {
    pub const NAMES: [&str; 14] = [
        "serial state",
        "no host",
        "host broken",
        "target format",
        "no texture",
        "bumped cube map",
        "key on a block format",
        "texture sampled both ways",
        "first level with two filters",
        "linear magnify, point minify, mipped",
        "key with linear mip",
        "texture with two samplings",
        "SRCALPHASAT destination",
        "program failed",
    ];

    /// The name the status counts it under.
    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }
}

/// The most rectangles of written pixels the card hands a host for one
/// surface (`Host::write`). A surface written in more is read back
/// around them.
const WRITTEN_RECTS: usize = 1024;

/// The bytes of a surface the processor wrote while the surface was
/// ahead on the host: a bit for each byte, from the surface's first.
#[derive(Default)]
struct Written {
    /// Empty until the first write.
    bits: Vec<u64>,
    any: bool,
}

impl Written {
    /// The bytes `from` to before `to` of the surface, of `len` bytes,
    /// were written.
    fn note(&mut self, from: u32, to: u32, len: u32) {
        let (from, to) = (from as usize, to.min(len) as usize);
        if from >= to {
            return;
        }
        if self.bits.is_empty() {
            self.bits = vec![0; len.div_ceil(64) as usize];
        }
        self.any = true;
        let (first, last) = (from / 64, (to - 1) / 64);
        let low = !0u64 << (from % 64);
        let high = !0u64 >> (63 - (to - 1) % 64);
        if first == last {
            self.bits[first] |= low & high;
            return;
        }
        self.bits[first] |= low;
        self.bits[first + 1..last].fill(!0);
        self.bits[last] |= high;
    }

    #[inline]
    fn has(&self, byte: u32) -> bool {
        let word = self.bits.get(byte as usize / 64);
        word.is_some_and(|w| w >> (byte % 64) & 1 != 0)
    }

    fn clear(&mut self) {
        self.bits.fill(0);
        self.any = false;
    }

    /// The runs of written bytes from `from` to before `to`, first to
    /// after last, in order.
    fn runs(&self, from: u32, to: u32, mut each: impl FnMut(u32, u32)) {
        let mut at = from;
        while at < to {
            let word = self.bits.get(at as usize / 64).copied().unwrap_or(0) >> (at % 64);
            let left = 64 - at % 64;
            if word == 0 {
                at += left;
                continue;
            }
            let start = at + word.trailing_zeros();
            if start >= to {
                break;
            }
            // The run goes on while the words are all ones.
            let mut end = start;
            loop {
                let word = self.bits.get(end as usize / 64).copied().unwrap_or(0) >> (end % 64);
                let ones = (!word).trailing_zeros().min(64 - end % 64);
                end += ones;
                if ones == 0 || !end.is_multiple_of(64) || end >= to {
                    break;
                }
            }
            let end = end.min(to);
            each(start, end);
            at = end;
        }
    }

    /// The written pixels of `surface` as rectangles, the runs of a row
    /// that the rows below repeat joined. None when a pixel was written
    /// in part, or there are more than `WRITTEN_RECTS`.
    fn rects(&self, surface: &Surface) -> Option<Vec<Rect>> {
        let bytes = surface.format.bytes();
        let row_len = surface.width * bytes;
        let mut rects: Vec<Rect> = Vec::new();
        // The rectangles the row above ended with: where in `rects`.
        let mut open = 0;
        let mut whole = true;
        for y in 0..surface.height {
            let row = y * surface.pitch;
            let start = rects.len();
            self.runs(row, row + row_len, |from, to| {
                let (from, to) = (from - row, to - row);
                whole &= from % bytes == 0 && to % bytes == 0;
                rects.push(Rect {
                    left: from / bytes,
                    top: y,
                    width: (to - from) / bytes,
                    height: 1,
                });
            });
            if !whole || rects.len() > 2 * WRITTEN_RECTS {
                return None;
            }
            // A row with the runs of the one above makes its rectangles
            // one higher.
            let above = &rects[open..start];
            let same = above.len() == rects.len() - start
                && above
                    .iter()
                    .zip(&rects[start..])
                    .all(|(a, b)| (a.left, a.width) == (b.left, b.width) && a.top + a.height == y);
            if same && start > open {
                rects.truncate(start);
                rects[open..].iter_mut().for_each(|r| r.height += 1);
            } else {
                open = start;
            }
        }
        (rects.len() <= WRITTEN_RECTS).then_some(rects)
    }
}

/// A surface that is ahead on the host.
struct Ahead {
    surface: Surface,
    /// What the processor wrote of it since: card memory has those
    /// bytes, and the host the others.
    written: Written,
}

/// The host's pixels of surfaces that are ahead, fetched for the
/// program's reads of them (`Gpu::read`).
#[derive(Default)]
struct Fetched {
    /// As large as card memory once used: each surface's pixels are
    /// where card memory has the surface.
    bytes: Vec<u8>,
    /// The surfaces whose pixels are there, as the host has them now.
    of: Vec<Surface>,
}

/// The card's side of a host renderer: the host, the pages, and the
/// surfaces that are ahead on the host.
#[derive(Default)]
pub struct Gpu {
    /// Behind a cell, for a saved state, which reads the surfaces back
    /// into a copy of card memory without changing the card, and for
    /// the program's reads, which have the bus shared.
    host: RefCell<Option<Box<dyn Host>>>,
    pub pages: Pages,
    /// The surfaces whose pixels the host has and card memory hasn't.
    ahead: Vec<Ahead>,
    fetched: RefCell<Fetched>,
    /// The screen shows the picture from the host's copy this frame
    /// (`shows`): the next `shown` reads nothing back.
    presents: bool,
    /// How many surfaces were read back.
    read_backs: u64,
    /// The draws that went to the software renderer with a host
    /// attached, by reason: the rasteriser's (`Renderer::draw`) and the
    /// host's (`Host::declines`).
    declined_for: [u64; Decline::NAMES.len()],
    /// The read-backs by what asked for them (`Cause`).
    by_cause: [u64; Cause::NAMES.len()],
    /// The surfaces fetched for the program's reads (`read`), which
    /// are read-backs for `Cause::Lock`.
    fetches: Cell<u64>,
    /// The times written pixels went to the host's copy (`Host::
    /// write`).
    writes: u64,
}

// A copy of a card has no host: it draws in software, from what card
// memory holds.
impl Clone for Gpu {
    fn clone(&self) -> Self {
        Gpu::default()
    }
}

impl std::fmt::Debug for Gpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Gpu({}, {} ahead)", self.name(), self.ahead.len())
    }
}

impl Gpu {
    /// Attach `host`, or with None go back to the software renderer.
    /// The caller has settled everything first. The host drops the
    /// copies it has: they were stamped by other pages, maybe another
    /// card's, and card memory now has every pixel.
    pub fn set_host(&mut self, mut host: Option<Box<dyn Host>>, memory: u32) {
        debug_assert!(self.ahead.is_empty());
        if let Some(host) = &mut host {
            host.forget();
        }
        self.pages = match host {
            Some(_) => Pages::new(memory),
            None => Pages::default(),
        };
        *self.fetched.get_mut() = Fetched::default();
        *self.host.get_mut() = host;
    }

    #[inline]
    pub fn attached(&self) -> bool {
        self.host.borrow().is_some()
    }

    /// The host's name, or "software".
    pub fn name(&self) -> String {
        match &*self.host.borrow() {
            Some(host) => host.name().to_string(),
            None => "software".to_string(),
        }
    }

    /// The host's own counts for the status: null without a host, or
    /// with one that keeps none.
    pub fn status(&self) -> serde_json::Value {
        match &*self.host.borrow() {
            Some(host) => host.status(),
            None => serde_json::Value::Null,
        }
    }

    /// Whether the host has pixels card memory hasn't.
    #[inline]
    pub fn ahead(&self) -> bool {
        !self.ahead.is_empty()
    }

    /// How many surfaces were read back.
    pub fn read_backs(&self) -> u64 {
        self.read_backs + self.fetches.get()
    }

    /// How many times written pixels went to the host's copy of a
    /// surface.
    pub fn writes(&self) -> u64 {
        self.writes
    }

    /// Why the host doesn't draw triangles with `state`: None when it
    /// draws them.
    pub fn declines(&self, state: &DrawState) -> Option<Decline> {
        match &*self.host.borrow() {
            Some(host) => host.declines(state),
            None => Some(Decline::NoHost),
        }
    }

    /// Count a draw that went to the software renderer for `why`.
    pub fn decline(&mut self, why: Decline) {
        self.declined_for[why as usize] += 1;
    }

    /// How many draws went to the software renderer with a host
    /// attached.
    pub fn declined(&self) -> u64 {
        self.declined_for.iter().sum()
    }

    /// The declined draws by reason, every reason named, for the
    /// status.
    pub fn declined_for(&self) -> serde_json::Value {
        let counts = Decline::NAMES.iter().zip(self.declined_for);
        counts
            .map(|(name, count)| (name.to_string(), serde_json::json!(count)))
            .collect::<serde_json::Map<_, _>>()
            .into()
    }

    /// Read back the surfaces that are ahead on the host and share a
    /// byte with `range` (all of them for None), counted for `why`.
    #[inline]
    pub fn settle(&mut self, memory: &mut [u8], range: Option<(u32, u32)>, why: Cause) {
        if !self.ahead.is_empty() {
            self.read_back(memory, range, why);
        }
    }

    /// The read-backs by cause, for the status.
    pub fn causes(&self) -> serde_json::Value {
        let mut by_cause = self.by_cause;
        by_cause[Cause::Lock as usize] += self.fetches.get();
        let counts = Cause::NAMES.iter().zip(by_cause);
        counts
            .map(|(name, count)| (name.to_string(), serde_json::json!(count)))
            .collect::<serde_json::Map<_, _>>()
            .into()
    }

    #[cold]
    fn read_back(&mut self, memory: &mut [u8], range: Option<(u32, u32)>, why: Cause) {
        let _profile = crate::profile::zone(crate::profile::Zone::ToadForceReadBack);
        let Gpu {
            host,
            ahead,
            read_backs,
            by_cause,
            pages,
            fetched,
            ..
        } = self;
        fetched.get_mut().of.clear();
        let Some(host) = host.get_mut() else {
            ahead.clear();
            return;
        };
        ahead.retain(|a| {
            let bytes = self::range(&a.surface);
            if range.is_some_and(|r| !overlap(r, bytes)) {
                return true;
            }
            // What the processor wrote of it stays: the host's pixels
            // go around it.
            let mut kept: Vec<(u32, Vec<u8>)> = Vec::new();
            if a.written.any {
                a.written.runs(0, bytes.1 - bytes.0, |from, to| {
                    let (from, to) = ((bytes.0 + from) as usize, (bytes.0 + to) as usize);
                    kept.push((from as u32, memory[from..to].to_vec()));
                });
            }
            host.read_back(&a.surface, memory, Some(pages));
            for (at, run) in &kept {
                memory[*at as usize..*at as usize + run.len()].copy_from_slice(run);
            }
            if !kept.is_empty() {
                // The host's copy isn't what card memory holds.
                pages.wrote(bytes.0, bytes.1 - bytes.0);
            }
            *read_backs += 1;
            by_cause[why as usize] += 1;
            false
        });
    }

    /// Card memory as it is with every surface read back, when some
    /// are ahead: for a saved state, which changes nothing.
    pub fn settled(&self, memory: &[u8]) -> Option<Vec<u8>> {
        if self.ahead.is_empty() {
            return None;
        }
        let mut copy = memory.to_vec();
        if let Some(host) = self.host.borrow_mut().as_mut() {
            for a in &self.ahead {
                host.read_back(&a.surface, &mut copy, None);
                if !a.written.any {
                    continue;
                }
                let start = a.surface.offset;
                a.written.runs(0, range(&a.surface).1 - start, |from, to| {
                    let run = (start + from) as usize..(start + to) as usize;
                    copy[run.clone()].copy_from_slice(&memory[run]);
                });
            }
        }
        Some(copy)
    }

    /// `surface` is ahead on the host, which has one copy for it and
    /// its views: the list keeps the largest. Nothing of them is
    /// written: `push` came first.
    fn mark(&mut self, surface: Surface) {
        if self.ahead.iter().any(|a| within(&surface, &a.surface)) {
            return;
        }
        self.ahead.retain(|a| !within(&a.surface, &surface));
        self.ahead.push(Ahead {
            surface,
            written: Written::default(),
        });
    }

    /// The processor is about to write `len` bytes at `offset`: where a
    /// surface that is ahead has them, card memory has those bytes of
    /// it from now on.
    #[cold]
    pub fn writing(&mut self, offset: u32, len: u32) {
        let end = offset.saturating_add(len);
        for a in &mut self.ahead {
            let bytes = range(&a.surface);
            if overlap((offset, end), bytes) {
                let (from, to) = (offset.max(bytes.0), end.min(bytes.1));
                a.written
                    .note(from - bytes.0, to - bytes.0, bytes.1 - bytes.0);
            }
        }
    }

    /// The program reads `len` bytes at `offset` by itself, up to 4:
    /// their value where a surface that is ahead has one of them, the
    /// host's pixels for what the processor didn't write since. None
    /// where card memory has them all. The host's pixels of the
    /// surface are fetched once, and again after the host changed its
    /// copy.
    #[cold]
    pub fn read(&self, offset: u32, len: u32, memory: &[u8]) -> Option<u32> {
        let end = offset.saturating_add(len);
        let a = self
            .ahead
            .iter()
            .find(|a| overlap((offset, end), range(&a.surface)))?;
        let bytes = range(&a.surface);
        // The bytes between a row's last pixel and the next row's first
        // are no pixel's: the host has none of them, and a read-back
        // leaves them as card memory has them.
        let s = &a.surface;
        let (pitch, row) = (s.pitch.max(1), s.width * s.format.bytes());
        let hosts = |at: u32| {
            at >= bytes.0
                && at < bytes.1
                && (at - bytes.0) % pitch < row
                && !a.written.has(at - bytes.0)
        };
        if !(offset..end).any(&hosts) {
            return None;
        }
        let mut fetched = self.fetched.borrow_mut();
        if !fetched.of.contains(&a.surface) {
            let _profile = crate::profile::zone(crate::profile::Zone::ToadForceReadBack);
            if fetched.bytes.len() != memory.len() {
                fetched.bytes = vec![0; memory.len()];
            }
            let mut host = self.host.borrow_mut();
            if !host
                .as_mut()?
                .read_back(&a.surface, &mut fetched.bytes, None)
            {
                return None;
            }
            fetched.of.push(a.surface);
            self.fetches.set(self.fetches.get() + 1);
        }
        let mut value = [0u8; 4];
        for (i, at) in (offset..end).enumerate().take(4) {
            let from = if hosts(at) { &fetched.bytes } else { memory };
            value[i] = from.get(at as usize).copied().unwrap_or(0);
        }
        Some(u32::from_le_bytes(value))
    }

    /// Before the host uses its copies: what the processor wrote of
    /// the surfaces that are ahead goes to them (`Host::write`). One
    /// the host doesn't write is read back around what was written.
    #[inline]
    fn push(&mut self, memory: &mut [u8]) {
        if self.ahead.iter().any(|a| a.written.any) {
            self.push_written(memory);
        }
    }

    #[cold]
    fn push_written(&mut self, memory: &mut [u8]) {
        let Gpu {
            host,
            ahead,
            fetched,
            writes,
            ..
        } = self;
        fetched.get_mut().of.clear();
        let mut left = Vec::new();
        for a in ahead {
            if !a.written.any {
                continue;
            }
            let _profile = crate::profile::zone(crate::profile::Zone::ToadForceHost);
            let rects = a.written.rects(&a.surface);
            // Bytes between rows alone are no pixel's: the host's copy
            // has nothing of them to change.
            if rects.as_ref().is_some_and(Vec::is_empty) {
                a.written.clear();
                continue;
            }
            let wrote = match (rects, host.get_mut().as_mut()) {
                (Some(rects), Some(host)) => host.write(&a.surface, &rects, memory),
                _ => false,
            };
            if wrote {
                a.written.clear();
                *writes += 1;
            } else {
                left.push(range(&a.surface));
            }
        }
        for bytes in left {
            self.settle(memory, Some(bytes), Cause::Program);
        }
    }

    /// Read back, for `why`, the surfaces that are ahead and share a
    /// byte with `surface` without being it, a view of it or what it
    /// is a view of: with `bits`, without being its bytes under
    /// another format's name either (`alias`).
    fn settle_others(&mut self, surface: &Surface, bits: bool, memory: &mut [u8], why: Cause) {
        let bytes = range(surface);
        let own = |s: &Surface| related(s, surface) || bits && same_bytes(s, surface);
        let other: Vec<(u32, u32)> = self
            .ahead
            .iter()
            .filter(|a| !own(&a.surface))
            .map(|a| range(&a.surface))
            .filter(|&r| overlap(r, bytes))
            .collect();
        for r in other {
            self.settle(memory, Some(r), why);
        }
    }

    /// Have the host draw `batch`, whose target is then ahead on it.
    /// Surfaces that are ahead and share bytes with the target's
    /// without being them or views of them are read back first, and so
    /// are those the bytes `reads` lie in: the host makes its copies
    /// from card memory. One that is exactly an image of each texture
    /// that samples its bytes stays ahead, and the host fills the
    /// image from its copy.
    pub fn draw(&mut self, batch: &Batch, reads: &[(u32, u32)], memory: &mut [u8]) {
        self.push(memory);
        self.fetched.get_mut().of.clear();
        let colour = batch.target.colour;
        let depth = batch.target.depth_surface();
        let own = |s: &Surface| related(s, &colour) || depth.is_some_and(|d| related(s, &d));
        let drawn = |r: (u32, u32)| {
            overlap(r, range(&colour)) || depth.is_some_and(|d| overlap(r, range(&d)))
        };
        let textures = || {
            let runs = batch.runs.iter();
            runs.flat_map(|run| run.state.tex.iter().flatten().map(|tex| &tex.texture))
        };
        let other: Vec<(u32, u32)> = self
            .ahead
            .iter()
            .map(|a| &a.surface)
            .filter(|s| !own(s))
            .filter(|s| {
                let r = range(s);
                if drawn(r) {
                    return true;
                }
                reads.iter().any(|&read| overlap(r, read))
                    && textures().any(|t| samples(t, r) && image_of(t, s).is_none())
            })
            .map(range)
            .collect();
        for r in other {
            self.settle(memory, Some(r), Cause::Sampled);
        }
        let Some(host) = self.host.get_mut() else {
            return;
        };
        let _profile = crate::profile::zone(crate::profile::Zone::ToadForceHost);
        // The host's copies, which may be of surfaces the target's are
        // views of: what is ahead is all of such a copy.
        for surface in host.draw(batch, memory, &self.pages) {
            self.mark(surface);
        }
    }

    /// Have the host write `clear` to its copy of the surface, which
    /// is then ahead on it. False when the host leaves it to the
    /// processor. Other surfaces that are ahead and share bytes with
    /// this one are read back first.
    pub fn clear(&mut self, clear: &Clear, memory: &mut [u8]) -> bool {
        if !self.attached() {
            return false;
        }
        self.push(memory);
        self.fetched.get_mut().of.clear();
        self.settle_others(&clear.surface, false, memory, Cause::Clear);
        let Some(host) = self.host.get_mut() else {
            return false;
        };
        let _profile = crate::profile::zone(crate::profile::Zone::ToadForceHost);
        match host.clear(clear, memory, &self.pages) {
            Some(written) => {
                self.mark(written);
                true
            }
            None => false,
        }
    }

    /// Whether the host shows `surface`, the picture on the screen,
    /// from its own copy (`Host::shows`). The frame's read-back of the
    /// picture (`take_presents`) is then left out.
    pub fn shows(&mut self, surface: &Surface, memory: &mut [u8]) -> bool {
        self.push(memory);
        let shows = match self.host.get_mut() {
            Some(host) => host.shows(surface, &self.pages),
            None => false,
        };
        self.presents = shows;
        shows
    }

    /// Whether the host shows this frame's picture, asked once a frame:
    /// false again until the next `shows`.
    pub fn take_presents(&mut self) -> bool {
        std::mem::take(&mut self.presents)
    }

    /// Whether a surface that is ahead has the pixels of `surface`:
    /// with `bits`, its bytes under any format's name of that pixel
    /// size.
    fn has_ahead(&self, surface: &Surface, bits: bool) -> bool {
        let mut ahead = self.ahead.iter().map(|a| &a.surface);
        ahead.any(|s| related(s, surface) || bits && same_bytes(s, surface))
    }

    /// Have the host do a `FILL`, when the destination is ahead on it
    /// and stays so. False when the processor is to write card memory.
    pub fn fill(&mut self, dst: &Surface, rect: &Rect, value: u32, memory: &mut [u8]) -> bool {
        if !self.has_ahead(dst, false) {
            return false;
        }
        self.push(memory);
        self.fetched.get_mut().of.clear();
        self.settle_others(dst, false, memory, Cause::Blit);
        let Some(host) = self.host.get_mut() else {
            return false;
        };
        let _profile = crate::profile::zone(crate::profile::Zone::ToadForceHost);
        match host.fill(dst, rect, value, memory, &self.pages) {
            Some(now) => {
                self.mark(now);
                true
            }
            None => false,
        }
    }

    /// Have the host do a `BLT`, when the source or the destination is
    /// ahead on it, under whatever format's name: the destination then
    /// is. False when the processor is to copy in card memory.
    pub fn blt(
        &mut self,
        dst: &Surface,
        dst_rect: &Rect,
        src: &Surface,
        src_rect: &Rect,
        key: Option<u32>,
        memory: &mut [u8],
    ) -> bool {
        if !self.has_ahead(dst, true) && !self.has_ahead(src, true) {
            return false;
        }
        self.push(memory);
        self.fetched.get_mut().of.clear();
        self.settle_others(dst, true, memory, Cause::Blit);
        self.settle_others(src, true, memory, Cause::Blit);
        let Some(host) = self.host.get_mut() else {
            return false;
        };
        let _profile = crate::profile::zone(crate::profile::Zone::ToadForceHost);
        match host.blt(dst, dst_rect, src, src_rect, key, memory, &self.pages) {
            Some(now) => {
                self.mark(now);
                true
            }
            None => false,
        }
    }

    /// Card memory is all there is: a saved state was loaded.
    pub fn forget(&mut self) {
        self.ahead.clear();
        self.fetched.get_mut().of.clear();
        if let Some(host) = self.host.get_mut() {
            host.forget();
        }
        // Every stamp is stale.
        let pages = self.pages.counts.len();
        self.pages.wrote(0, (pages as u32).saturating_mul(PAGE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reasons are the debugger's API: each has its own name, and
    /// `docs/llm-debugging.md` lists every one.
    #[test]
    fn the_debugging_guide_lists_every_reason_for_a_declined_draw() {
        let guide = include_str!("../../docs/llm-debugging.md");
        let guide = guide.split_whitespace().collect::<Vec<_>>().join(" ");
        for (i, name) in Decline::NAMES.iter().enumerate() {
            assert!(guide.contains(&format!("`{name}`")), "{name}");
            assert!(!Decline::NAMES[..i].contains(name), "{name} twice");
        }
        assert_eq!(Decline::ProgramFailed.name(), "program failed");
    }

    #[test]
    fn a_stamp_is_stale_once_a_page_of_its_bytes_is_written() {
        let mut pages = Pages::new(1 << 20);
        let range = (PAGE + 100, 3 * PAGE);
        let mut stamp = pages.stamp(range);
        assert!(!pages.changed(range, &mut stamp));
        // A write beside the range leaves it, and the stamp is brought
        // up to the new generation.
        pages.wrote(0, PAGE);
        pages.wrote(3 * PAGE, 8);
        assert!(!pages.changed(range, &mut stamp));
        assert_eq!(stamp, pages.stamp(range));
        // One in its first page, before its first byte, counts: pages
        // are what is counted.
        pages.wrote(PAGE, 1);
        assert!(pages.changed(range, &mut stamp));
        let mut stamp = pages.stamp(range);
        pages.wrote(3 * PAGE - 1, 2);
        assert!(pages.changed(range, &mut stamp));
    }

    #[test]
    fn nothing_is_counted_without_a_host() {
        let mut pages = Pages::default();
        pages.wrote(0, 100);
        let mut stamp = pages.stamp((0, 100));
        assert!(!pages.changed((0, 100), &mut stamp));
    }
}

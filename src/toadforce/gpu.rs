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
//! - **What the host declines.** A state `Host::takes` refuses, lines,
//!   points, triangles that aren't filled and draws that read the
//!   surface they write are drawn by the software renderer, in the
//!   card's order with the host's.
//! - **Who has the newer pixels.** A surface the host drew into is
//!   ahead on the host (`Gpu::ahead`), and card memory is stale there
//!   until `Gpu::settle` reads it back. Whatever reads or writes card
//!   memory on the processor settles the bytes it touches first.
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
//!   are the processor's.
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
use super::raster::pipeline::DrawState;
use super::raster::vertex::ScreenVertex;
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::sync::Arc;

/// The bytes of a page of card memory, as the writes are counted.
pub const PAGE: u32 = 4096;

/// The triangles queued with one state.
pub struct Run {
    pub state: Arc<DrawState>,
    /// Their vertices in `Batch::vertices`: three a triangle, and four
    /// where the state is flat-shaded (`Shape::flat`), the fourth the
    /// vertex that gives the triangle its colours.
    pub vertices: Range<usize>,
}

impl Run {
    /// The vertices of one triangle of the run.
    pub fn stride(&self) -> usize {
        if self.state.shape.flat { 4 } else { 3 }
    }
}

/// What a host draws at once: runs for one render target, in the
/// card's order.
pub struct Batch<'a> {
    pub target: Target,
    pub runs: &'a [Run],
    pub vertices: &'a [ScreenVertex],
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

    /// Whether it draws triangles with `state`. One it refuses goes to
    /// the software renderer.
    fn takes(&self, state: &DrawState) -> bool;

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
    /// of `dst`, when the host's copy of `dst` is ahead. The surface of
    /// that copy, which is ahead after it. None when the host leaves
    /// the fill to the processor, with nothing changed.
    fn fill(
        &mut self,
        dst: &Surface,
        rect: &Rect,
        value: u32,
        memory: &[u8],
        pages: &Pages,
    ) -> Option<Surface>;

    /// `BLT`: copy `src_rect` of `src` to `dst_rect` of `dst`, when
    /// the host's copy of either is ahead. The surface of the host's
    /// copy of `dst`, which is ahead after it. None when the host
    /// leaves the copy to the processor, with nothing changed: for a
    /// stretch, a colour key, formats it keeps no copies of, and a
    /// copy onto its own source.
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

/// Whether one of the two is a view of the other.
pub fn related(a: &Surface, b: &Surface) -> bool {
    within(a, b) || within(b, a)
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
    /// The program's own read or write of the frame buffer, and
    /// whatever names nothing else.
    Program,
    /// A driver gave the program the surface's address (`Lock`,
    /// `GetDC`).
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

/// The card's side of a host renderer: the host, the pages, and the
/// surfaces that are ahead on the host.
#[derive(Default)]
pub struct Gpu {
    /// Behind a cell, for a saved state, which reads the surfaces back
    /// into a copy of card memory without changing the card.
    host: RefCell<Option<Box<dyn Host>>>,
    pub pages: Pages,
    /// The surfaces whose pixels the host has and card memory hasn't.
    ahead: Vec<Surface>,
    /// The screen shows the picture from the host's copy this frame
    /// (`shows`): the next `shown` reads nothing back.
    presents: bool,
    /// How many surfaces were read back, and how many draws went to
    /// the software renderer with a host attached.
    pub read_backs: u64,
    pub declined: u64,
    /// The read-backs by what asked for them (`Cause`).
    by_cause: [u64; Cause::NAMES.len()],
    /// The program's reads of bytes that were ahead on the host, which
    /// got what card memory had (`Gpu::stale_read`).
    stale_reads: Cell<u64>,
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

    pub fn stale_reads(&self) -> u64 {
        self.stale_reads.get()
    }

    /// Whether the host draws triangles with `state`.
    pub fn takes(&self, state: &DrawState) -> bool {
        self.host.borrow().as_ref().is_some_and(|h| h.takes(state))
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
        let counts = Cause::NAMES.iter().zip(self.by_cause);
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
            ..
        } = self;
        let Some(host) = host.get_mut() else {
            ahead.clear();
            return;
        };
        ahead.retain(|surface| {
            if range.is_some_and(|r| !overlap(r, self::range(surface))) {
                return true;
            }
            host.read_back(surface, memory, Some(pages));
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
            for surface in &self.ahead {
                host.read_back(surface, &mut copy, None);
            }
        }
        Some(copy)
    }

    /// `surface` is ahead on the host, which has one copy for it and
    /// its views: the list keeps the largest.
    fn mark(&mut self, surface: Surface) {
        if self.ahead.iter().any(|s| within(&surface, s)) {
            return;
        }
        self.ahead.retain(|s| !within(s, &surface));
        self.ahead.push(surface);
    }

    /// Read back, for `why`, the surfaces that are ahead and share a
    /// byte with `surface` without being it, a view of it or what it
    /// is a view of.
    fn settle_others(&mut self, surface: &Surface, memory: &mut [u8], why: Cause) {
        let bytes = range(surface);
        let other: Vec<(u32, u32)> = self
            .ahead
            .iter()
            .filter(|s| !related(s, surface))
            .map(range)
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
        self.settle_others(&clear.surface, memory, Cause::Clear);
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
    pub fn shows(&mut self, surface: &Surface) -> bool {
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

    /// Whether a surface that is ahead has the pixels of `surface`.
    fn has_ahead(&self, surface: &Surface) -> bool {
        self.ahead.iter().any(|s| related(s, surface))
    }

    /// Have the host do a `FILL`, when the destination is ahead on it
    /// and stays so. False when the processor is to write card memory.
    pub fn fill(&mut self, dst: &Surface, rect: &Rect, value: u32, memory: &mut [u8]) -> bool {
        if !self.has_ahead(dst) {
            return false;
        }
        self.settle_others(dst, memory, Cause::Blit);
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
    /// ahead on it: the destination then is. False when the processor
    /// is to copy in card memory.
    pub fn blt(
        &mut self,
        dst: &Surface,
        dst_rect: &Rect,
        src: &Surface,
        src_rect: &Rect,
        key: Option<u32>,
        memory: &mut [u8],
    ) -> bool {
        if !self.has_ahead(dst) && !self.has_ahead(src) {
            return false;
        }
        self.settle_others(dst, memory, Cause::Blit);
        self.settle_others(src, memory, Cause::Blit);
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

    /// The program read `len` bytes at `offset` by itself, which the
    /// card can't answer with a read-back: count it if those bytes are
    /// ahead on the host.
    #[cold]
    pub fn stale_read(&self, offset: u32, len: u32) {
        let read = (offset, offset.saturating_add(len));
        if self.ahead.iter().any(|s| overlap(read, range(s))) {
            self.stale_reads.set(self.stale_reads.get() + 1);
        }
    }

    /// Card memory is all there is: a saved state was loaded.
    pub fn forget(&mut self) {
        self.ahead.clear();
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

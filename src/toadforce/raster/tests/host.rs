//! The card with a host renderer attached (`toadforce::gpu`): a scene
//! drawn through a host, and read back, is byte for byte what the
//! software renderer draws.
//!
//! The host here keeps a copy of each surface it draws into, as an
//! OpenGL one keeps a framebuffer, and draws into the copy with the
//! software renderer's own spans. Card memory gets the copy only when
//! the card asks for it, so a read-back the card leaves out shows as
//! a difference. It keeps the bytes of each texture it samples too,
//! as an OpenGL one keeps a texture, and copies them again only when
//! their stamp is stale: a write the card leaves out of `Pages` shows
//! as a difference as well.

use super::*;
use crate::toadforce::blit;
use crate::toadforce::command;
use crate::toadforce::format::Rect;
use crate::toadforce::gpu::{self, Batch, Decline, Host, Pages, Stamp};
use crate::toadforce::raster::mem::Mem;
use crate::toadforce::raster::pipeline::{self, DrawState};
use crate::toadforce::raster::setup::Prim;
use crate::toadforce::raster::span;
use std::cell::RefCell;
use std::rc::Rc;

/// What the host was asked for.
#[derive(Default)]
struct Log {
    /// Copies of targets made from card memory.
    uploads: u32,
    /// Copies of textures made from card memory.
    texture_uploads: u32,
    batches: u32,
    triangles: u32,
    read_backs: u32,
    forgets: u32,
    /// Buffers a `CLEAR` wrote in the host's copy.
    clears: u32,
    /// `FILL`s and `BLT`s done in the host's copies.
    fills: u32,
    blits: u32,
    /// The times the processor's pixels were put in a copy (`write`).
    writes: u32,
    /// Images of textures taken from the host's copy of the surface
    /// they are.
    images_from_copies: u32,
}

/// Picks the states a host refuses, and why.
type Declines = fn(&DrawState) -> Option<Decline>;

/// A surface as the host has it.
struct Copy {
    surface: Surface,
    stamp: Stamp,
    /// The pixels, row after row without what is between the rows.
    pixels: Vec<u8>,
    /// The pixels are newer than card memory's.
    ahead: bool,
}

/// The bytes of card memory a texture sampled, as the host has them.
struct Texels {
    range: (u32, u32),
    stamp: Stamp,
    bytes: Vec<u8>,
}

struct TestHost {
    copies: Vec<Copy>,
    textures: Vec<Texels>,
    /// The states the host refuses.
    declines: Declines,
    log: Rc<RefCell<Log>>,
}

/// Whether two ranges of card memory share a byte.
fn shares(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// The bytes of each row of `s` in card memory.
fn rows(s: &Surface) -> impl Iterator<Item = std::ops::Range<usize>> {
    let (s, len) = (*s, (s.width * s.format.bytes()) as usize);
    (0..s.height).map(move |y| s.at(0, y)..s.at(0, y) + len)
}

/// The pixels of `s` in `memory`.
fn pixels(s: &Surface, memory: &[u8]) -> Vec<u8> {
    rows(s)
        .flat_map(|row| memory[row].iter().copied())
        .collect()
}

/// `pixels` put where `s` is in `memory`.
fn put(s: &Surface, pixels: &[u8], memory: &mut [u8]) {
    let mut at = 0;
    for row in rows(s) {
        let len = row.len();
        memory[row].copy_from_slice(&pixels[at..at + len]);
        at += len;
    }
}

/// `s` as it lies in a copy's pixels: rows with nothing between
/// them, from the first byte.
fn packed(s: &Surface) -> Surface {
    Surface {
        offset: 0,
        pitch: s.width * s.format.bytes(),
        ..*s
    }
}

/// The view `view` of `whole` as it lies in the pixels of a copy of
/// `whole`.
fn packed_view(view: &Surface, whole: &Surface) -> Surface {
    Surface {
        offset: 0,
        pitch: whole.width * whole.format.bytes(),
        ..*view
    }
}

impl TestHost {
    /// Drop the copies of other surfaces that share a byte with
    /// `surfaces`, which the host writes.
    fn drop_copies(&mut self, surfaces: &[Surface]) {
        self.copies.retain(|c| {
            surfaces.iter().any(|s| gpu::related(s, &c.surface))
                || !surfaces
                    .iter()
                    .any(|s| shares(gpu::range(s), gpu::range(&c.surface)))
        });
    }

    /// Drop the bytes kept of textures that share one with `surfaces`.
    fn drop_textures(&mut self, surfaces: &[Surface]) {
        self.textures
            .retain(|t| !surfaces.iter().any(|s| shares(t.range, gpu::range(s))));
    }

    /// The host's copy of `surface`, as `ToadForceGl::copy` has it: the
    /// copy of the largest surface it is a view of, filled from
    /// `memory` when it isn't ahead and is stale, and with `fresh` not
    /// filled at all when the caller writes every pixel.
    fn copy(&mut self, surface: &Surface, memory: &[u8], pages: &Pages, fresh: bool) -> usize {
        let found = self
            .copies
            .iter()
            .position(|c| gpu::within(surface, &c.surface));
        if let Some(at) = found {
            let copy = &mut self.copies[at];
            let whole = copy.surface == *surface;
            if !copy.ahead
                && !(fresh && whole)
                && pages.changed(gpu::range(&copy.surface), &mut copy.stamp)
            {
                copy.pixels = pixels(&copy.surface, memory);
                copy.stamp = pages.stamp(gpu::range(&copy.surface));
                self.log.borrow_mut().uploads += 1;
            }
            return at;
        }
        let mut copy = Copy {
            surface: *surface,
            stamp: pages.stamp(gpu::range(surface)),
            pixels: pixels(surface, memory),
            ahead: false,
        };
        if fresh {
            // Nothing of card memory: the caller writes every pixel.
            copy.pixels.fill(0xEE);
        } else {
            self.log.borrow_mut().uploads += 1;
        }
        // The copy of a view of it, whose pixels are newer where it is
        // ahead.
        let old = self
            .copies
            .iter()
            .position(|c| gpu::within(&c.surface, surface));
        if let Some(old) = old {
            let view = self.copies.remove(old);
            if view.ahead && !fresh {
                put(
                    &packed_view(&view.surface, surface),
                    &view.pixels,
                    &mut copy.pixels,
                );
                copy.ahead = true;
            }
        }
        self.copies.push(copy);
        self.copies.len() - 1
    }
}

impl Host for TestHost {
    fn name(&self) -> &str {
        "test"
    }

    fn declines(&self, state: &DrawState) -> Option<Decline> {
        (self.declines)(state)
    }

    fn draw(&mut self, batch: &Batch, memory: &[u8], pages: &Pages) -> Vec<Surface> {
        self.log.borrow_mut().batches += 1;
        let surfaces: Vec<Surface> = std::iter::once(batch.target.colour)
            .chain(batch.target.depth_surface())
            .collect();
        // Neither these draws nor a read-back of them is counted: the
        // copies of other bytes of the target's are dropped
        // (`Host::draw`).
        self.drop_copies(&surfaces);
        for surface in &surfaces {
            self.copy(surface, memory, pages, false);
        }
        // Where they are once both are there: a copy that grew took
        // the place of another.
        let copies: Vec<usize> = surfaces
            .iter()
            .filter_map(|s| self.copies.iter().position(|c| gpu::within(s, &c.surface)))
            .collect();
        let drawn: Vec<Surface> = copies.iter().map(|&at| self.copies[at].surface).collect();
        self.drop_textures(&drawn);
        let mut log = self.log.borrow_mut();
        // Card memory with the host's copies of the textures and of the
        // target over it is what the spans draw into.
        let mut scratch = memory.to_vec();
        let mut reads = Vec::new();
        for run in batch.runs {
            pipeline::reads(&run.state, &mut reads);
        }
        for range in reads {
            let end = (range.1 as usize).min(memory.len());
            let bytes = range.0 as usize..end.max(range.0 as usize);
            let found = self.textures.iter().position(|t| t.range == range);
            let at = found.unwrap_or_else(|| {
                self.textures.push(Texels {
                    range,
                    stamp: Stamp::default(),
                    bytes: Vec::new(),
                });
                self.textures.len() - 1
            });
            let texels = &mut self.textures[at];
            if found.is_none() || pages.changed(range, &mut texels.stamp) {
                texels.bytes = memory[bytes.clone()].to_vec();
                texels.stamp = pages.stamp(range);
                log.texture_uploads += 1;
            }
            scratch[bytes].copy_from_slice(&texels.bytes);
        }
        // An image of a texture that is a surface the host has ahead
        // is the host's copy (`gpu::image_of`), which has the bytes a
        // read-back would write.
        for run in batch.runs {
            for tex in run.state.tex.iter().flatten() {
                for copy in &self.copies {
                    if copy.ahead && gpu::image_of(&tex.texture, &copy.surface).is_some() {
                        put(&copy.surface, &copy.pixels, &mut scratch);
                        log.images_from_copies += 1;
                    }
                }
            }
        }
        for &at in &copies {
            let copy = &self.copies[at];
            put(&copy.surface, &copy.pixels, &mut scratch);
        }
        let slots: [u8; pipeline::SLOTS] = std::array::from_fn(|i| i as u8);
        let mem = Mem::new(&mut scratch);
        let mut chunk = span::Chunk::new();
        for run in batch.runs {
            let state = &*run.state;
            let ctx = span::Ctx::new(mem, state);
            let sets = &slots[..state.coords.len()];
            for t in batch.vertices[run.vertices.clone()].as_chunks::<3>().0 {
                log.triangles += 1;
                // A flat-shaded triangle's vertices all have its
                // colours.
                let t = t.map(|v| v.screen());
                let v = [&t[0], &t[1], &t[2]];
                let Some(prim) = Prim::triangle(v, &t[0], &state.shape, state.clip, sets)
                else {
                    continue;
                };
                for y in prim.rows.0..prim.rows.1 {
                    ctx.row(&prim, y, &mut chunk);
                }
            }
        }
        for at in copies {
            let copy = &mut self.copies[at];
            copy.pixels = pixels(&copy.surface, &scratch);
            copy.ahead = true;
        }
        drawn
    }

    fn clear(&mut self, clear: &gpu::Clear, memory: &[u8], pages: &Pages) -> Option<Surface> {
        let surface = clear.surface;
        // A copy that is what card memory holds, or newer, or a clear
        // of every bit of every pixel of the copy.
        let found = self
            .copies
            .iter_mut()
            .find(|c| gpu::within(&surface, &c.surface));
        let (whole, current) = match found {
            Some(c) => (
                (c.surface.width, c.surface.height) == (surface.width, surface.height),
                c.ahead || !pages.changed(gpu::range(&c.surface), &mut c.stamp),
            ),
            None => (true, false),
        };
        let all = Rect {
            left: 0,
            top: 0,
            width: surface.width,
            height: surface.height,
        };
        let fresh = whole && clear.whole() && clear.rects.contains(&all);
        if !current && !fresh {
            return None;
        }
        self.drop_copies(&[surface]);
        let at = self.copy(&surface, memory, pages, fresh);
        let written = self.copies[at].surface;
        self.drop_textures(&[written]);
        let copy = &mut self.copies[at];
        // The copy's rows have nothing between them.
        let rows = Surface {
            offset: 0,
            pitch: written.width * written.format.bytes(),
            ..written
        };
        gpu::Clear {
            surface: rows,
            ..*clear
        }
        .write(&mut copy.pixels);
        copy.ahead = true;
        self.log.borrow_mut().clears += 1;
        Some(written)
    }

    fn fill(
        &mut self,
        dst: &Surface,
        rect: &Rect,
        value: u32,
        _: &[u8],
        _: &Pages,
    ) -> Option<Surface> {
        let copy = self
            .copies
            .iter_mut()
            .find(|c| gpu::within(dst, &c.surface))
            .filter(|c| c.ahead && !depth(c.surface.format))?;
        let surface = copy.surface;
        blit::fill(&mut copy.pixels, &packed(&surface), rect, value);
        self.drop_textures(&[surface]);
        self.log.borrow_mut().fills += 1;
        Some(surface)
    }

    fn blt(
        &mut self,
        dst: &Surface,
        dst_rect: &Rect,
        src: &Surface,
        src_rect: &Rect,
        key: Option<u32>,
        memory: &[u8],
        pages: &Pages,
    ) -> Option<Surface> {
        let size = (src_rect.width, src_rect.height);
        if key.is_some()
            || size != (dst_rect.width, dst_rect.height)
            || dst.format.bytes() != src.format.bytes()
        {
            return None;
        }
        // The copies of the bytes the two name, under whatever format:
        // the copies' formats say what the bits are.
        let find = |s: &Surface| self.copies.iter().find(|c| gpu::alias(s, &c.surface));
        let from = find(src).filter(|c| c.ahead);
        let to = find(dst);
        if from.is_none() && !to.is_some_and(|c| c.ahead) {
            return None;
        }
        let kept = |c: &Copy| c.surface.format;
        let format = to.map(kept).or(from.map(kept)).unwrap_or(dst.format);
        let kind = |f: Format| (depth(f), f.bytes());
        if !(colour(format) || depth(format)) || from.is_some_and(|c| kind(kept(c)) != kind(format))
        {
            return None;
        }
        let (dst, src) = (&Surface { format, ..*dst }, &Surface { format, ..*src });
        // What `ToadForceGl::blt` leaves to the processor, though the
        // copies here could do it: a source the host has over other
        // bytes of the destination's, and one over the destination's
        // own pixels.
        let related = gpu::related(src, dst);
        let shared = !related && shares(gpu::range(src), gpu::range(dst));
        let apart = src_rect.left + size.0 <= dst_rect.left
            || dst_rect.left + size.0 <= src_rect.left
            || src_rect.top + size.1 <= dst_rect.top
            || dst_rect.top + size.1 <= src_rect.top;
        if from.is_some() && (shared || related && !apart) {
            return None;
        }
        // The source's rows: the copy's, or card memory's.
        let (source, rows) = match from {
            Some(copy) => (packed(&copy.surface), copy.pixels.clone()),
            None => (*src, memory.to_vec()),
        };
        let all = Rect {
            left: 0,
            top: 0,
            width: dst.width,
            height: dst.height,
        };
        self.drop_copies(&[*dst]);
        let at = self.copy(dst, memory, pages, *dst_rect == all);
        let surface = self.copies[at].surface;
        self.drop_textures(&[surface]);
        let copy = &mut self.copies[at];
        // Two buffers, so the source is whole while the copy is
        // written.
        let offset = rows.len();
        let mut both = rows;
        both.extend_from_slice(&copy.pixels);
        let to = Surface {
            offset: offset as u32,
            ..packed(&surface)
        };
        blit::blt(&mut both, &to, dst_rect, &source, src_rect, None);
        copy.pixels = both[offset..].to_vec();
        copy.ahead = true;
        self.log.borrow_mut().blits += 1;
        Some(surface)
    }

    fn write(&mut self, surface: &Surface, rects: &[Rect], memory: &[u8]) -> bool {
        let copy = self
            .copies
            .iter_mut()
            .find(|c| c.ahead && gpu::within(surface, &c.surface));
        let Some(copy) = copy else {
            return false;
        };
        let whole = copy.surface;
        // The rectangles from card memory, behind the copy's rows in
        // one buffer.
        let offset = copy.pixels.len();
        let mut both = copy.pixels.clone();
        both.extend_from_slice(memory);
        let from = Surface {
            offset: whole.offset + offset as u32,
            ..whole
        };
        for rect in rects {
            blit::blt(&mut both, &packed(&whole), rect, &from, rect, None);
        }
        copy.pixels = both[..offset].to_vec();
        self.log.borrow_mut().writes += 1;
        true
    }

    fn read_back(&mut self, surface: &Surface, memory: &mut [u8], pages: Option<&Pages>) -> bool {
        self.log.borrow_mut().read_backs += 1;
        let found = self
            .copies
            .iter_mut()
            .find(|c| gpu::within(surface, &c.surface));
        match found {
            Some(copy) => {
                let view = pixels(&packed_view(surface, &copy.surface), &copy.pixels);
                put(surface, &view, memory);
                if let Some(pages) = pages
                    && copy.surface == *surface
                {
                    copy.ahead = false;
                    copy.stamp = pages.stamp(gpu::range(&copy.surface));
                }
                true
            }
            None => false,
        }
    }

    fn shows(&mut self, surface: &Surface, pages: &Pages) -> bool {
        let found = self
            .copies
            .iter_mut()
            .find(|c| gpu::within(surface, &c.surface));
        found.is_some_and(|c| c.ahead || !pages.changed(gpu::range(&c.surface), &mut c.stamp))
    }

    fn forget(&mut self) {
        self.log.borrow_mut().forgets += 1;
        self.copies.clear();
        self.textures.clear();
    }
}

/// Whether `format` is a depth buffer's, which `FILL` leaves to the
/// processor.
fn depth(format: Format) -> bool {
    format.in_mask(crate::toadforce::caps::DEPTH_FORMATS)
}

/// Whether `format` is one a host draws into, the only ones `BLT`
/// takes.
fn colour(format: Format) -> bool {
    matches!(format, Format::R5G6B5 | Format::X8R8G8B8 | Format::A8R8G8B8)
}

fn takes_all(_: &DrawState) -> Option<Decline> {
    None
}

/// A host that leaves a state with `POINTSPRITEENABLE` on to the
/// software renderer, so a scene can put a draw of the processor's
/// among the host's: the state changes nothing of a line or a
/// triangle. It counts under `no texture`.
fn declines_sprites(state: &DrawState) -> Option<Decline> {
    state.shape.point_sprite.then_some(Decline::NoTexture)
}

/// One that also has no program for flat shading yet.
fn declines_flat(state: &DrawState) -> Option<Decline> {
    declines_sprites(state).or(state.shape.flat.then_some(Decline::ProgramFailed))
}

const FORMAT: Format = Format::X8R8G8B8;
const DEPTH_FORMAT: Format = Format::D16;

/// A rig with a host that refuses the states `declines` picks.
fn hosted(declines: Declines) -> (Rig, Rc<RefCell<Log>>) {
    let mut rig = Rig::new(FORMAT, Some(DEPTH_FORMAT));
    let log = Rc::new(RefCell::new(Log::default()));
    let host = TestHost {
        copies: Vec::new(),
        textures: Vec::new(),
        declines,
        log: log.clone(),
    };
    rig.card.set_host(Some(Box::new(host)), &mut rig.memory);
    (rig, log)
}

/// Run `scene` on a card that draws in software and on one with a
/// host, and check that card memory is the same once the host's
/// pixels are read back. Gives the hosted rig and what its host was
/// asked for.
fn same(declines: Declines, scene: fn(&mut Rig)) -> (Rig, Rc<RefCell<Log>>) {
    let mut software = Rig::new(FORMAT, Some(DEPTH_FORMAT));
    scene(&mut software);
    let (mut rig, log) = hosted(declines);
    scene(&mut rig);
    // What the host has and card memory hasn't is what the read-back
    // brings.
    if rig.card.ahead() {
        assert!(rig.memory != software.memory);
    }
    rig.card.settle(&mut rig.memory, None);
    assert!(!rig.card.ahead());
    assert!(rig.memory == software.memory);
    (rig, log)
}

/// The processor writes `bytes` to card memory at `at`, as the bus
/// does it: the card notes them where the host has newer pixels, and
/// the write is counted.
fn poke(rig: &mut Rig, at: usize, bytes: &[u8]) {
    let (first, len) = (at as u32, bytes.len() as u32);
    if rig.card.ahead() {
        rig.card.writing(first, len);
    }
    rig.memory[at..at + bytes.len()].copy_from_slice(bytes);
    rig.card.wrote(first, len);
}

/// The program reads the pixel at `at` of card memory, as the bus does
/// it: from the host where it has a newer one.
fn peek(rig: &Rig, at: usize) -> u32 {
    let memory = &rig.memory;
    let ahead = rig.card.ahead();
    let hosts = ahead.then(|| rig.card.read_ahead(at as u32, 4, memory));
    hosts.flatten().unwrap_or_else(|| {
        u32::from_le_bytes([memory[at], memory[at + 1], memory[at + 2], memory[at + 3]])
    })
}

fn rect(left: u32, top: u32, width: u32, height: u32) -> Rect {
    Rect {
        left,
        top,
        width,
        height,
    }
}

/// The rig's own render target.
fn target(rig: &Rig) -> Target {
    Target {
        colour: rig.colour(),
        depth: rig.depth.map(|format| Depth {
            offset: DEPTH,
            pitch: SIZE * format.bytes(),
            format,
        }),
    }
}

/// Triangles over each other with depth and blending.
fn triangles(rig: &mut Rig) {
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0);
    rig.states(&[(RS_ZENABLE, 1)]);
    assert_eq!(rig.draw(4, &corner(2.0, 2.0, 40.0, 0.5, 0xFFFF_0000)), 0);
    assert_eq!(rig.draw(4, &corner(10.0, 6.0, 40.0, 0.7, 0xFF00_FF00)), 0);
    rig.states(&[
        (RS_ALPHABLENDENABLE, 1),
        (RS_SRCBLEND, 5),
        (RS_DESTBLEND, 6),
    ]);
    assert_eq!(rig.draw(5, &square(30.0, 0x8000_00FF)), 0);
}

#[test]
fn what_a_host_drew_is_in_card_memory_once_it_is_read_back() {
    let (mut rig, log) = hosted(takes_all);
    triangles(&mut rig);
    // The clear of the whole target and the triangles are the host's
    // alone: card memory has neither, and gave the host nothing.
    assert!(rig.card.ahead());
    assert_eq!((rig.px(0, 0), rig.depth(4, 4)), (0, 0));
    assert_eq!((log.borrow().read_backs, log.borrow().uploads), (0, 0));

    let (settled, log) = same(takes_all, triangles);
    assert!(!settled.drawn(0x20_2020).is_empty());
    assert!(settled.depth(4, 4) < 0xFFFF);
    let log = log.borrow();
    // Two triangles and a strip of two, in three batches, with the
    // colour and the depth buffer never copied and read back once.
    assert_eq!((log.batches, log.triangles), (3, 4));
    assert_eq!((log.uploads, log.read_backs), (0, 2));
}

/// Draws of the host and of the software renderer, with the card's
/// other commands and a write of the processor between them.
fn mixed(rig: &mut Rig) {
    let colour = rig.colour();
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0);
    rig.states(&[(RS_ZENABLE, 1)]);
    // One run of commands: triangles, lines the host doesn't take,
    // triangles again, a clear of a corner and a triangle over it.
    let error = rig.run(|w| {
        encode::draw(w, 4, TL, 3, &corner(2.0, 2.0, 40.0, 0.5, 0xFFFF_0000));
        let line = [
            vertex_z(0.0, 20.5, 0.1, 0xFFFF_FFFF, 0.0, 0.0),
            vertex_z(60.0, 24.5, 0.1, 0xFFFF_FFFF, 0.0, 0.0),
        ]
        .concat();
        encode::render_state(w, &[(RS_POINTSPRITEENABLE, 1)]);
        encode::draw(w, 2, TL, 2, &line);
        encode::render_state(w, &[(RS_POINTSPRITEENABLE, 0)]);
        encode::draw(w, 4, TL, 3, &corner(6.0, 12.0, 30.0, 0.3, 0xFF00_FF00));
        encode::clear(w, CLEAR_COLOUR, 0x33_3333, 0.0, 0, &[rect(56, 0, 8, 8)]);
        encode::draw(w, 4, TL, 3, &corner(52.0, 0.0, 10.0, 0.3, 0xFFFF_FFFF));
    });
    assert_eq!(error, 0);
    // A state the host declines, between two it takes.
    rig.states(&[(RS_SHADEMODE, 1)]);
    let flat = [
        vertex_z(30.0, 30.0, 0.2, 0xFF11_2233, 0.0, 0.0),
        vertex_z(60.0, 30.0, 0.2, 0xFF44_5566, 0.0, 0.0),
        vertex_z(30.0, 60.0, 0.2, 0xFF77_8899, 0.0, 0.0),
    ]
    .concat();
    assert_eq!(rig.draw(4, &flat), 0);
    rig.states(&[
        (RS_SHADEMODE, 2),
        (RS_ALPHABLENDENABLE, 1),
        (RS_SRCBLEND, 5),
        (RS_DESTBLEND, 6),
    ]);
    assert_eq!(rig.draw(5, &square(36.0, 0x8000_00FF)), 0);
    // FILL and BLT, each with a draw after it.
    let error = rig.run(|w| {
        command::encode::fill(w, &colour, &rect(10, 10, 8, 8), 0x12_3456);
        encode::draw(w, 4, TL, 3, &corner(12.0, 12.0, 20.0, 0.2, 0x80FF_FF00));
        command::encode::blt(
            w,
            &colour,
            &rect(40, 44, 16, 16),
            &colour,
            &rect(8, 8, 16, 16),
            None,
        );
        encode::draw(w, 4, TL, 3, &corner(44.0, 48.0, 8.0, 0.2, 0x80FF_00FF));
    });
    assert_eq!(error, 0);
    // The processor writes into the target, and a draw goes over a
    // part of what it wrote.
    poke(rig, colour.at(4, 50), &[0xAA; 16]);
    assert_eq!(rig.draw(4, &corner(0.0, 44.0, 12.0, 0.1, 0x80FF_FF00)), 0);
    // A triangle whose vertices are in the target, where the host
    // drew last.
    let at = colour.at(0, 62);
    let words = corner(20.0, 40.0, 10.0, 0.0, 0xFFFF_FFFF);
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    poke(rig, at, &bytes);
    let error = rig.run(|w| {
        encode::draw(w, 4, TL, 3, &corner(0.0, 61.0, 3.0, 0.0, 0xFF00_0000));
        encode::draw_buffer(w, 4, TL, 32, at as u32, 3);
    });
    assert_eq!(error, 0);
}

#[test]
fn the_hosts_draws_and_the_processors_keep_the_cards_order() {
    // The lines, and the flat triangle where the host refuses it.
    let hosts: [(Declines, u32); 2] = [(declines_sprites, 1), (declines_flat, 2)];
    for (declines, declined) in hosts {
        let (rig, log) = same(declines, mixed);
        // What the processor wrote and no draw covered is there.
        assert_eq!(rig.px(7, 50), 0xAAAA_AAAA);
        let host = rig.card.describe(Rig::live());
        assert_eq!(host["renderer"], "test");
        assert_eq!(host["host"]["declined_draws"], declined);
        // Each is counted under its reason.
        let reasons = &host["host"]["declined_for"];
        assert_eq!(reasons["no texture"], 1);
        assert_eq!(reasons["program failed"], u64::from(declined - 1));
        let log = log.borrow();
        assert_eq!(host["host"]["read_backs"], log.read_backs);
        // What the processor wrote into the target went to the host's
        // copy of it.
        assert!(log.uploads >= 2 && log.writes >= 1);
    }
}

/// Lines, points and triangles drawn as their outlines and their
/// corners, in the states that change which pixels they have.
fn lines_and_points(rig: &mut Rig) {
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0);
    rig.states(&[(RS_ZENABLE, 1)]);
    let line = |x0: f32, y0: f32, x1: f32, y1: f32| {
        [
            vertex_z(x0, y0, 0.25, 0xFFFF_FFFF, 0.0, 0.0),
            vertex_z(x1, y1, 0.75, 0xFF40_80C0, 0.0, 0.0),
        ]
        .concat()
    };
    for centre in [0, 1] {
        rig.states(&[(RS_PIXEL_CENTRE, centre)]);
        // From one point into every octant, with ends on pixel
        // centres, between them and off the target, then lines of one
        // pixel and of none.
        let mut lines = Vec::new();
        for (dx, dy) in [
            (20.0, 0.0),
            (20.0, 7.0),
            (20.0, 20.0),
            (7.3, 20.0),
            (0.0, 20.5),
            (-7.0, 20.0),
            (-20.0, 20.0),
            (-20.0, 6.75),
            (-20.5, 0.0),
            (-40.0, -9.0),
            (-20.0, -20.0),
            (-6.5, -40.0),
            (0.0, -20.0),
            (7.0, -19.5),
            (20.0, -20.0),
            (40.0, -7.25),
        ] {
            lines.extend(line(31.0, 30.5, 31.0 + dx, 30.5 + dy));
        }
        lines.extend(line(3.0, 3.0, 3.0, 3.0));
        lines.extend(line(5.5, 3.5, 5.75, 3.5));
        lines.extend(line(8.25, 3.0, 9.0, 3.5));
        let count = (lines.len() / 8) as u32;
        // With and without the last pixel, shaded and flat.
        for (last, shade) in [(1, 2), (0, 2), (0, 1), (1, 1)] {
            rig.states(&[(RS_LASTPIXEL, last), (RS_SHADEMODE, shade)]);
            assert_eq!(rig.run(|w| encode::draw(w, 2, TL, count, &lines)), 0);
        }
        rig.states(&[(RS_LASTPIXEL, 1), (RS_SHADEMODE, 2)]);
        // A strip, clipped by a viewport smaller than the target.
        let strip = [line(-10.0, 50.0, 30.0, 58.0), line(70.0, 44.0, 40.0, 70.0)].concat();
        let part = Viewport {
            x: 4,
            y: 40,
            width: 50,
            height: 20,
            min_z: 0.0,
            max_z: 1.0,
        };
        assert_eq!(rig.run(|w| encode::viewport(w, &part)), 0);
        assert_eq!(rig.run(|w| encode::draw(w, 3, TL, 4, &strip)), 0);
        let whole = Viewport {
            x: 0,
            y: 0,
            width: SIZE,
            height: SIZE,
            ..part
        };
        assert_eq!(rig.run(|w| encode::viewport(w, &whole)), 0);
        // Points of every size, at centres, on edges and half outside.
        for size in [1.0f32, 1.5, 2.0, 3.0, 7.25] {
            rig.states(&[(RS_POINTSIZE, size.to_bits())]);
            let mut points = Vec::new();
            for (x, y) in [
                (10.0, 12.0),
                (20.5, 12.5),
                (30.25, 12.75),
                (0.0, 40.0),
                (63.5, 63.5),
            ] {
                points.extend(vertex_z(x, y, 0.5, 0xFF10_F020, 0.0, 0.0));
            }
            assert_eq!(rig.run(|w| encode::draw(w, 1, TL, 5, &points)), 0);
        }
        // A triangle as its outline and as its corners, facing each
        // way, with the back culled.
        rig.states(&[(RS_POINTSIZE, 3.0f32.to_bits()), (RS_CULLMODE, 3)]);
        for fill in [2, 1, 3] {
            rig.states(&[(RS_FILLMODE, fill)]);
            let front = corner(34.0, 2.0, 20.0, 0.1, 0xFFE0_E000);
            assert_eq!(rig.draw(4, &front), 0);
            let back = [
                vertex_z(2.0, 34.0, 0.1, 0xFF00_E0E0, 0.0, 0.0),
                vertex_z(2.0, 54.0, 0.1, 0xFF00_E0E0, 0.0, 0.0),
                vertex_z(22.0, 34.0, 0.1, 0xFF00_E0E0, 0.0, 0.0),
            ]
            .concat();
            assert_eq!(rig.draw(4, &back), 0);
        }
        rig.states(&[(RS_CULLMODE, 1)]);
    }
}

#[test]
fn the_host_gets_lines_and_points_as_the_triangles_of_their_pixels() {
    // The test host fills the triangles it is given by the card's
    // rule for triangles, so equal colours mean the squares of a line
    // and of a point cover the line's and the point's pixels and no
    // other, with their colours.
    let mut software = Rig::new(FORMAT, Some(DEPTH_FORMAT));
    lines_and_points(&mut software);
    let (mut rig, log) = hosted(takes_all);
    lines_and_points(&mut rig);
    rig.card.settle(&mut rig.memory, None);
    let host = rig.card.describe(Rig::live());
    assert_eq!(host["host"]["declined_draws"], 0);
    assert!(log.borrow().triangles > 400);
    // A depth is the line's plane worked out again from a square's
    // corners, which rounds to the next step of the buffer at a pixel
    // half-way between two.
    let mut apart = 0;
    for at in (DEPTH as usize..).step_by(2).take((SIZE * SIZE) as usize) {
        let depth = |rig: &Rig| u16::from_le_bytes([rig.memory[at], rig.memory[at + 1]]);
        let (a, b) = (depth(&software), depth(&rig));
        assert!(a.abs_diff(b) <= 1, "depth {a} and {b}");
        apart += (a != b) as u32;
        rig.memory[at..at + 2].copy_from_slice(&a.to_le_bytes());
    }
    assert_eq!(apart, 4, "depths that differ");
    assert!(rig.memory == software.memory);
}

#[test]
fn a_write_of_the_processor_into_a_target_goes_to_the_hosts_copy() {
    let (mut rig, log) = hosted(takes_all);
    let part = [rect(0, 0, 32, 32)];
    let error = rig.run(|w| encode::clear(w, CLEAR_COLOUR, 0xFF20_2020, 0.0, 0, &part));
    assert_eq!(error, 0);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 8.0, 0.0, 0xFFFF_0000)), 0);
    assert_eq!(rig.draw(4, &corner(20.0, 0.0, 8.0, 0.0, 0xFFFF_0000)), 0);
    // A clear of a part of a target the host has no copy of is the
    // processor's. Two draws into a target nobody else touched: one
    // copy of the colour and one of the depth buffer.
    assert_eq!((log.borrow().clears, log.borrow().uploads), (0, 2));
    // A write beside the target leaves the copies good.
    poke(&mut rig, BUFFER as usize, &[1; 64]);
    assert_eq!(rig.draw(4, &corner(40.0, 0.0, 8.0, 0.0, 0xFFFF_0000)), 0);
    assert_eq!((log.borrow().uploads, log.borrow().read_backs), (2, 0));
    // One into the colour buffer reads nothing back: the card notes
    // the pixel, and the next draw has the host write it into its
    // copy, which is copied from card memory no more.
    let at = rig.colour().at(1, 40);
    poke(&mut rig, at, &[0x55; 4]);
    assert_eq!((log.borrow().read_backs, log.borrow().writes), (0, 0));
    assert_eq!(rig.draw(4, &corner(0.0, 20.0, 8.0, 0.0, 0xFF00_FF00)), 0);
    assert_eq!((log.borrow().uploads, log.borrow().writes), (2, 1));
    assert_eq!(log.borrow().read_backs, 0);
    rig.card.settle(&mut rig.memory, None);
    assert_eq!(rig.px(1, 40), 0x5555_5555);
    assert_eq!(rig.px(1, 1), 0xFF_0000);
    assert_eq!(rig.px(1, 21), 0x00_FF00);
}

#[test]
fn the_program_reads_a_surface_that_is_ahead_from_the_hosts_pixels() {
    let (mut rig, log) = hosted(takes_all);
    triangles(&mut rig);
    let colour = rig.colour();
    assert!(rig.card.ahead());
    // Card memory has nothing of the picture, and a read through the
    // bus gets the host's pixel: the surface is fetched once for any
    // number of reads.
    assert_eq!(rig.px(4, 34), 0);
    assert_eq!(peek(&rig, colour.at(4, 34)) & 0xFF_FFFF, 0xFF_0000);
    assert_eq!(peek(&rig, colour.at(60, 2)) & 0xFF_FFFF, 0x20_2020);
    assert_eq!(log.borrow().read_backs, 1);
    assert!(rig.card.ahead());
    // A pixel the program wrote is card memory's, and one beside it
    // the host's still. A read across the two has each byte from its
    // own.
    poke(&mut rig, colour.at(4, 34), &[1, 2, 3, 4]);
    assert_eq!(peek(&rig, colour.at(4, 34)), 0x0403_0201);
    assert_eq!(peek(&rig, colour.at(5, 34)) & 0xFF_FFFF, 0xFF_0000);
    assert_eq!(peek(&rig, colour.at(4, 34) + 2), 0x0000_0403);
    assert_eq!(log.borrow().read_backs, 1);
    // Bytes outside every surface that is ahead are card memory's.
    assert_eq!(rig.card.read_ahead(BUFFER, 4, &rig.memory), None);
    // The status counts the fetch as a read-back for the lock.
    let host = rig.card.describe(Rig::live());
    assert_eq!(host["host"]["read_backs"], 1);
    assert_eq!(host["host"]["read_backs_for"]["lock"], 1);
    // The host draws again: its pixels are fetched anew, with the
    // program's pixel in them.
    assert_eq!(rig.draw(4, &corner(40.0, 40.0, 8.0, 0.0, 0xFF00_00FF)), 0);
    assert_eq!(log.borrow().writes, 1);
    assert_eq!(peek(&rig, colour.at(41, 41)) & 0xFF_FFFF, 0x00_00FF);
    assert_eq!(peek(&rig, colour.at(4, 34)), 0x0403_0201);
    assert_eq!(log.borrow().read_backs, 2);
    // A saved state has the host's pixels.
    let saved = rig.card.settled(&rig.memory).expect("a surface ahead");
    let at = colour.at(41, 41);
    assert_eq!(saved[at..at + 3], [0xFF, 0, 0]);
    assert_eq!(saved[colour.at(4, 34)..][..4], [1, 2, 3, 4]);
}

#[test]
fn the_bytes_between_the_rows_of_a_surface_ahead_are_read_from_card_memory() {
    let (mut rig, log) = hosted(takes_all);
    // A surface half as wide as its pitch: the right half of each row
    // but the last is no pixel of it, and holds what card memory has.
    let whole = texture_surface();
    let half = Surface {
        width: SIZE / 2,
        ..whole
    };
    let between = whole.at(SIZE / 2 + 3, 5);
    poke(&mut rig, between, &[9, 8, 7, 6]);
    draw_into(&mut rig, half, 0xFF20_4060);
    assert!(rig.card.ahead());
    assert_eq!(peek(&rig, between), 0x0607_0809);
    // A pixel of it is the host's, fetched once, as a read-back brings
    // it, which leaves the bytes between the rows as they were.
    let at = half.at(3, 5);
    let pixel = peek(&rig, at);
    assert_ne!(pixel, 0);
    assert_eq!(peek(&rig, between), 0x0607_0809);
    assert_eq!(log.borrow().read_backs, 1);
    rig.card.settle(&mut rig.memory, None);
    assert_eq!(rig.memory[at..at + 4], pixel.to_le_bytes());
    assert_eq!(rig.memory[between..between + 4], [9, 8, 7, 6]);
}

#[test]
fn a_surface_written_in_parts_of_pixels_is_read_back_around_them() {
    let (mut rig, log) = hosted(takes_all);
    triangles(&mut rig);
    let colour = rig.colour();
    // Two bytes of a pixel: the host takes whole pixels, so the next
    // draw has the surface read back, with the two bytes kept, and
    // copied again.
    let at = colour.at(4, 34);
    poke(&mut rig, at + 1, &[0xAB, 0xCD]);
    let uploads = log.borrow().uploads;
    assert_eq!(rig.draw(4, &corner(40.0, 40.0, 8.0, 0.0, 0xFF00_00FF)), 0);
    assert_eq!((log.borrow().writes, log.borrow().read_backs), (0, 1));
    assert_eq!(log.borrow().uploads, uploads + 1);
    let host = rig.card.describe(Rig::live());
    assert_eq!(host["host"]["read_backs_for"]["program"], 1);
    rig.card.settle(&mut rig.memory, None);
    assert_eq!(rig.memory[at..at + 3], [0, 0xAB, 0xCD]);
    assert_eq!(rig.px(41, 41), 0x00_00FF);
    assert_eq!(rig.px(5, 34), 0xFF_0000);
}

/// A scene's depth kept in a second depth buffer the processor wrote,
/// and put back with a `BLT` that names both as colour, as DirectDraw
/// blits between two z-buffers: the triangle drawn after it is cut by
/// the depth that was put back.
fn depth_put_back(rig: &mut Rig) {
    let depth = target(rig).depth_surface().expect("a depth buffer");
    let named = |s: &Surface| Surface {
        format: Format::R5G6B5,
        ..*s
    };
    let kept = Surface {
        offset: BUFFER,
        ..depth
    };
    // The kept depth: near on the left half, far on the right.
    for y in 0..kept.height {
        for x in 0..kept.width {
            let z: u16 = if x < kept.width / 2 { 0x0010 } else { 0xFFFF };
            poke(rig, kept.at(x, y), &z.to_le_bytes());
        }
    }
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0);
    rig.states(&[(RS_ZENABLE, 1)]);
    assert_eq!(rig.draw(4, &corner(2.0, 2.0, 40.0, 0.5, 0xFFFF_0000)), 0);
    let all = rect(0, 0, depth.width, depth.height);
    let error =
        rig.run(|w| command::encode::blt(w, &named(&depth), &all, &named(&kept), &all, None));
    assert_eq!(error, 0);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 128.0, 0.5, 0xFF00_FF00)), 0);
}

#[test]
fn a_blt_between_depth_buffers_named_as_colour_copies_the_hosts_depth() {
    let (mut rig, log) = hosted(takes_all);
    depth_put_back(&mut rig);
    // The host copied the depth: nothing was read back for it.
    assert_eq!((log.borrow().blits, log.borrow().read_backs), (1, 0));
    let (rig, _) = same(takes_all, depth_put_back);
    // The triangle shows where the kept depth is far.
    assert_eq!(rig.px(50, 50), 0x00_FF00);
    assert_ne!(rig.px(10, 50), 0x00_FF00);
}

/// A surface the card draws into and then samples.
fn texture_surface() -> Surface {
    Surface {
        offset: TEXTURE,
        pitch: SIZE * 4,
        width: SIZE,
        height: SIZE,
        format: FORMAT,
    }
}

/// Draw into a surface, and then draw with it as the texture.
fn render_to_texture(rig: &mut Rig) {
    let surface = texture_surface();
    let first = Target {
        colour: surface,
        depth: None,
    };
    assert_eq!(rig.run(|w| encode::render_target(w, Some(&first))), 0);
    rig.clear(CLEAR_COLOUR, 0xFF10_3050, 0.0, 0);
    assert_eq!(rig.draw(5, &square(40.0, 0xFFC0_8040)), 0);
    assert_eq!(rig.draw(4, &corner(8.0, 8.0, 50.0, 0.0, 0xFF20_E0A0)), 0);

    let second = target(rig);
    let texture = Texture {
        format: FORMAT,
        width: SIZE,
        height: SIZE,
        levels: 1,
        images: vec![(surface.offset, surface.pitch)],
        ..Texture::default()
    };
    let error = rig.run(|w| {
        encode::render_target(w, Some(&second));
        encode::define_texture(w, 1, &texture);
        encode::set_texture(w, 0, 1);
    });
    assert_eq!(error, 0);
    rig.stage(
        0,
        &[(TSS_COLOROP, OP_SELECT_ARG1), (TSS_COLORARG1, ARG_TEXTURE)],
    );
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF00_0000, 1.0, 0);
    assert_eq!(rig.draw(5, &square(48.0, 0xFFFF_FFFF)), 0);
}

#[test]
fn a_surface_the_host_drew_into_is_sampled_as_a_texture() {
    let (rig, _) = same(takes_all, render_to_texture);
    // The second triangle of the first target, through the texture.
    assert_eq!(rig.px(12, 20), 0x20_E0A0);
    assert_eq!(rig.px(40, 2), 0x10_3050);
    // And with the software renderer drawing the second target.
    fn declines_textured(state: &DrawState) -> Option<Decline> {
        state.tex[0].is_some().then_some(Decline::ProgramFailed)
    }
    same(declines_textured, render_to_texture);
}

/// The square from (`x`, `y`) with sides of `size`, as a strip of four
/// vertices, with the texture once over it.
fn quad(x: f32, y: f32, size: f32) -> Vec<u32> {
    let white = 0xFFFF_FFFF;
    [
        vertex(x, y, white, 0.0, 0.0),
        vertex(x + size, y, white, 1.0, 0.0),
        vertex(x, y + size, white, 0.0, 1.0),
        vertex(x + size, y + size, white, 1.0, 1.0),
    ]
    .concat()
}

/// Make `target` the render target, with a viewport of all of it.
fn aim(rig: &mut Rig, target: &Target) {
    let viewport = Viewport {
        x: 0,
        y: 0,
        width: target.colour.width,
        height: target.colour.height,
        min_z: 0.0,
        max_z: 1.0,
    };
    let error = rig.run(|w| {
        encode::render_target(w, Some(target));
        encode::viewport(w, &viewport);
    });
    assert_eq!(error, 0);
}

/// Sample the texture at `TEXTURE` after each way card memory changes
/// under it, each time into a square of its own: the processor's
/// write, `FILL`, `BLT`, `CLEAR`, the software renderer's draw and two
/// of the host's.
fn texture_writes(rig: &mut Rig) {
    let surface = texture_surface();
    let texels: Vec<u8> = (0..SIZE * SIZE)
        .flat_map(|i| (0xFF00_0000 | i.wrapping_mul(0x0001_0307)).to_le_bytes())
        .collect();
    poke(rig, TEXTURE as usize, &texels);
    let texture = Texture {
        format: FORMAT,
        width: SIZE,
        height: SIZE,
        levels: 1,
        images: vec![(surface.offset, surface.pitch)],
        ..Texture::default()
    };
    let error = rig.run(|w| {
        encode::define_texture(w, 1, &texture);
        encode::set_texture(w, 0, 1);
    });
    assert_eq!(error, 0);
    let sampled = [(TSS_COLOROP, OP_SELECT_ARG1), (TSS_COLORARG1, ARG_TEXTURE)];
    let plain = [(TSS_COLOROP, OP_SELECT_ARG1), (TSS_COLORARG1, ARG_DIFFUSE)];
    rig.stage(0, &sampled);
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF00_0000, 1.0, 0);
    assert_eq!(rig.draw(5, &quad(0.0, 0.0, 16.0)), 0);

    let main = target(rig);
    let error = rig.run(|w| {
        command::encode::fill(w, &surface, &rect(0, 0, 32, 32), 0x00FF_00FF);
        encode::draw(w, 5, TL, 4, &quad(16.0, 0.0, 16.0));
        // From the target, which the host has drawn into.
        command::encode::blt(
            w,
            &surface,
            &rect(32, 32, 32, 32),
            &main.colour,
            &rect(0, 0, 32, 32),
            None,
        );
        encode::draw(w, 5, TL, 4, &quad(32.0, 0.0, 16.0));
    });
    assert_eq!(error, 0);
    poke(rig, surface.at(0, 40), &[0x80; 256]);
    assert_eq!(rig.draw(5, &quad(48.0, 0.0, 16.0)), 0);

    let into = Target {
        colour: surface,
        depth: None,
    };
    aim(rig, &into);
    let error = rig.run(|w| {
        encode::clear(w, CLEAR_COLOUR, 0x40_4040, 0.0, 0, &[rect(0, 48, 64, 16)]);
    });
    assert_eq!(error, 0);
    aim(rig, &main);
    assert_eq!(rig.draw(5, &quad(0.0, 16.0, 16.0)), 0);

    // Into the texture with nothing bound: lines the host doesn't
    // take, which the software renderer draws, and then two triangles
    // of the host's.
    let line = [
        vertex(0.0, 8.5, 0xFFFF_FFFF, 0.0, 0.0),
        vertex(64.0, 12.5, 0xFFFF_FFFF, 0.0, 0.0),
    ]
    .concat();
    let draws: [(u32, Vec<u32>); 3] = [
        (2, line),
        (4, corner(0.0, 0.0, 40.0, 0.0, 0xFF20_E0A0)),
        (4, corner(20.0, 20.0, 40.0, 0.0, 0xFF80_2040)),
    ];
    for (i, (primitive, vertices)) in draws.iter().enumerate() {
        rig.stage(0, &plain);
        assert_eq!(rig.run(|w| encode::set_texture(w, 0, 0)), 0);
        aim(rig, &into);
        rig.states(&[(RS_POINTSPRITEENABLE, (*primitive == 2) as u32)]);
        assert_eq!(rig.draw(*primitive, vertices), 0);
        rig.states(&[(RS_POINTSPRITEENABLE, 0)]);
        aim(rig, &main);
        assert_eq!(rig.run(|w| encode::set_texture(w, 0, 1)), 0);
        rig.stage(0, &sampled);
        let x = 16.0 * (i + 1) as f32;
        assert_eq!(rig.draw(5, &quad(x, 16.0, 16.0)), 0);
    }
}

#[test]
fn a_texture_the_host_keeps_is_copied_again_after_each_write() {
    let (rig, log) = same(declines_sprites, texture_writes);
    // A copy at first, and one after each of the seven writes.
    assert_eq!(log.borrow().texture_uploads, 8);
    // The lines.
    let host = rig.card.describe(Rig::live());
    assert_eq!(host["host"]["declined_draws"], 1);
}

/// Draw into two surfaces over the same bytes by turns, and into two
/// targets whose depth buffers are the same memory at two sizes.
fn overlapping_targets(rig: &mut Rig) {
    let main = target(rig);
    let whole = texture_surface();
    let part = Surface {
        offset: whole.at(16, 16) as u32,
        width: 32,
        height: 32,
        ..whole
    };
    let on = |colour: Surface| Target {
        colour,
        depth: None,
    };
    aim(rig, &on(whole));
    rig.clear(CLEAR_COLOUR, 0xFF10_1010, 0.0, 0);
    let draws = [
        (whole, corner(0.0, 0.0, 48.0, 0.0, 0xFFFF_0000)),
        (part, corner(8.0, 8.0, 40.0, 0.0, 0xFF00_FF00)),
        (whole, corner(30.0, 30.0, 30.0, 0.0, 0xFF00_00FF)),
        (part, corner(0.0, 0.0, 20.0, 0.0, 0xFFFF_FF00)),
    ];
    for (surface, vertices) in &draws {
        aim(rig, &on(*surface));
        assert_eq!(rig.draw(4, vertices), 0);
    }

    // A 32x32 target with the top left of the main target's depth
    // buffer for its own.
    let small = Target {
        colour: Surface {
            offset: BUFFER,
            pitch: 32 * FORMAT.bytes(),
            width: 32,
            height: 32,
            format: FORMAT,
        },
        depth: main.depth,
    };
    aim(rig, &main);
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0);
    rig.states(&[(RS_ZENABLE, 1)]);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 40.0, 0.5, 0xFFFF_0000)), 0);
    aim(rig, &small);
    rig.clear(CLEAR_COLOUR, 0xFF00_0000, 0.0, 0);
    assert_eq!(rig.draw(4, &corner(4.0, 4.0, 24.0, 0.2, 0xFF00_FF00)), 0);
    aim(rig, &main);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 60.0, 0.3, 0xFF00_00FF)), 0);
}

#[test]
fn surfaces_over_the_same_bytes_take_each_others_pixels() {
    let (rig, _) = same(takes_all, overlapping_targets);
    // The small target's depth kept the last triangle out of its
    // corner, and the main target's kept it out of the red one's.
    assert_eq!(rig.px(6, 6), 0xFF_0000);
    assert_eq!(rig.px(45, 10), 0x00_00FF);
}

/// The first half of a scene, and the half after a state was saved.
fn before_the_save(rig: &mut Rig) {
    triangles(rig);
}

fn after_the_load(rig: &mut Rig) {
    assert_eq!(rig.draw(4, &corner(20.0, 20.0, 30.0, 0.2, 0x80FF_FFFF)), 0);
    rig.states(&[(RS_ZENABLE, 0)]);
    assert_eq!(rig.draw(4, &corner(0.0, 30.0, 30.0, 0.9, 0xFF80_8080)), 0);
}

#[test]
fn a_saved_state_has_what_the_host_drew_and_a_loaded_one_starts_from_memory() {
    use crate::savestate::{Reader, State, Writer};
    let mut software = Rig::new(FORMAT, Some(DEPTH_FORMAT));
    before_the_save(&mut software);
    let (mut rig, log) = hosted(takes_all);
    before_the_save(&mut rig);

    // Saving reads back into a copy: the card and its memory stay.
    let stale = rig.memory.clone();
    let saved = rig.card.settled(&rig.memory).unwrap();
    let mut w = Writer::new();
    rig.card.save(&mut w);
    assert!(saved == software.memory);
    assert!(rig.memory == stale && rig.card.ahead());
    // A card with nothing ahead has no copy to make.
    assert!(software.card.settled(&software.memory).is_none());

    // The same card goes on as if nothing was saved.
    after_the_load(&mut software);
    after_the_load(&mut rig);
    rig.card.settle(&mut rig.memory, None);
    assert!(rig.memory == software.memory);

    // The state loaded, with a host attached that still has the
    // pixels of after the save: it drops them. (It dropped what it had
    // once before, when it was attached.)
    let forgets = log.borrow().forgets;
    rig.card.load(&mut Reader::new(&w.buf)).unwrap();
    rig.memory = saved.clone();
    rig.card.forget();
    assert_eq!(log.borrow().forgets, forgets + 1);
    assert!(!rig.card.ahead());
    let uploads = log.borrow().uploads;
    after_the_load(&mut rig);
    assert_eq!(log.borrow().uploads, uploads + 2);
    rig.card.settle(&mut rig.memory, None);
    assert!(rig.memory == software.memory);
}

#[test]
fn a_host_taken_away_leaves_its_pixels_in_card_memory() {
    let mut software = Rig::new(FORMAT, Some(DEPTH_FORMAT));
    triangles(&mut software);
    let (mut rig, _) = hosted(takes_all);
    triangles(&mut rig);
    assert!(rig.card.ahead());
    rig.card.set_host(None, &mut rig.memory);
    assert!(!rig.card.ahead());
    assert!(rig.memory == software.memory);
    let status = rig.card.describe(Rig::live());
    assert_eq!(status["renderer"], "software");
    // The software renderer draws from here on.
    after_the_load(&mut software);
    after_the_load(&mut rig);
    assert!(rig.memory == software.memory);
    assert!(!rig.card.ahead());
}

#[test]
fn a_guests_ring_has_its_draws_in_card_memory_when_the_write_returns() {
    let mut software = Rig::new(FORMAT, Some(DEPTH_FORMAT));
    triangles(&mut software);
    let (mut rig, log) = hosted(takes_all);
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0);
    let mut words = Vec::new();
    encode::render_state(&mut words, &[(RS_ZENABLE, 1)]);
    encode::draw(
        &mut words,
        4,
        TL,
        3,
        &corner(2.0, 2.0, 40.0, 0.5, 0xFFFF_0000),
    );
    encode::draw(
        &mut words,
        4,
        TL,
        3,
        &corner(10.0, 6.0, 40.0, 0.7, 0xFF00_FF00),
    );
    encode::render_state(
        &mut words,
        &[
            (RS_ALPHABLENDENABLE, 1),
            (RS_SRCBLEND, 5),
            (RS_DESTBLEND, 6),
        ],
    );
    encode::draw(&mut words, 5, TL, 4, &square(30.0, 0x8000_00FF));
    for (i, word) in words.iter().enumerate() {
        rig.card.ring_write(4 * i as u32, *word, 4);
    }
    let put = 4 * words.len() as u32;
    rig.card
        .write(regs::RING_PUT, put, &mut rig.memory, Rig::live());
    assert_eq!(rig.card.read(regs::ERROR, Rig::live()), 0);
    assert!(log.borrow().triangles == 4 && !rig.card.ahead());
    assert!(rig.memory == software.memory);
}

#[test]
fn a_card_without_a_host_counts_nothing_and_is_never_ahead() {
    let mut rig = Rig::new(FORMAT, Some(DEPTH_FORMAT));
    mixed(&mut rig);
    assert!(!rig.card.ahead());
    assert!(rig.card.settled(&rig.memory).is_none());
    let status = rig.card.describe(Rig::live());
    assert_eq!(status["renderer"], "software");
    assert_eq!(status["host"]["read_backs"], 0);
    assert_eq!(status["host"]["declined_draws"], 0);
}

/// A frame after a frame, and a clear of a corner.
fn two_frames(rig: &mut Rig) {
    triangles(rig);
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF40_1010, 0.8, 0);
    assert_eq!(rig.draw(4, &corner(4.0, 4.0, 30.0, 0.6, 0xFF00_00FF)), 0);
    let corner = [rect(56, 0, 8, 8)];
    let error = rig.run(|w| encode::clear(w, CLEAR_COLOUR, 0x33_3333, 0.0, 0, &corner));
    assert_eq!(error, 0);
}

#[test]
fn a_clear_runs_on_the_host_where_the_host_has_the_buffer() {
    let (mut rig, log) = same(takes_all, two_frames);
    // Each frame's clear wrote both of the host's buffers, the first
    // with no copy there yet, and the corner one of them, with nothing
    // read back or copied in between.
    assert_eq!(log.borrow().clears, 5);
    assert_eq!((log.borrow().uploads, log.borrow().read_backs), (0, 2));
    // After a write of the processor the host's copy is stale. A
    // clear of a part of it is the processor's.
    let at = rig.colour().at(0, 0);
    poke(&mut rig, at, &[1, 2, 3, 4]);
    let corner = [rect(0, 0, 8, 8)];
    let error = rig.run(|w| encode::clear(w, CLEAR_COLOUR, 0xFF00_0000, 0.0, 0, &corner));
    assert_eq!(error, 0);
    assert_eq!(log.borrow().clears, 5);
    assert!(!rig.card.ahead());
    assert_eq!(rig.px(0, 0), 0);
    // A clear of all of it is the host's, which copies nothing first.
    poke(&mut rig, at, &[1, 2, 3, 4]);
    rig.clear(CLEAR_COLOUR, 0xFF00_0000, 0.0, 0);
    assert_eq!((log.borrow().clears, log.borrow().uploads), (6, 0));
    assert!(rig.card.ahead());
    rig.card.settle(&mut rig.memory, None);
    assert_eq!(rig.px(0, 0), 0);
}

/// A target that starts and ends inside pages of card memory.
fn unaligned() -> Surface {
    Surface {
        offset: TEXTURE + 256,
        ..small_surface()
    }
}

/// A draw, a write of the processor beside the target in a page they
/// share, and another draw.
fn neighbours(rig: &mut Rig) {
    let colour = unaligned();
    let target = Target {
        colour,
        depth: None,
    };
    assert_eq!(rig.run(|w| encode::render_target(w, Some(&target))), 0);
    rig.clear(CLEAR_COLOUR, 0xFF20_2020, 1.0, 0);
    assert_eq!(rig.draw(4, &corner(2.0, 2.0, 20.0, 0.5, 0xFFFF_0000)), 0);
    // The byte before the colour buffer's first is in its first page.
    poke(rig, colour.offset as usize - 1, &[0x77]);
    assert_eq!(rig.draw(4, &corner(10.0, 6.0, 20.0, 0.7, 0xFF00_FF00)), 0);
}

#[test]
fn a_write_beside_a_target_that_is_ahead_leaves_the_hosts_pixels() {
    // The stamp of the copy is stale after the write, and card memory
    // has none of the first draw: a copy made again from it would
    // lose the draw.
    let (rig, log) = same(takes_all, neighbours);
    let at = unaligned().at(4, 8);
    assert_eq!(rig.memory[at..at + 4], [0, 0, 0xFF, 0]);
    assert_eq!((log.borrow().uploads, log.borrow().read_backs), (0, 1));
}

/// A surface of a quarter of the screen's pixels.
fn small_surface() -> Surface {
    Surface {
        pitch: SIZE * 2,
        width: SIZE / 2,
        height: SIZE / 2,
        ..texture_surface()
    }
}

/// Two render targets of different sizes with one depth buffer, as a
/// game's small targets share the screen's.
fn shared_depth(rig: &mut Rig) {
    let big = target(rig);
    let small = Target {
        colour: small_surface(),
        depth: big.depth,
    };
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0);
    rig.states(&[(RS_ZENABLE, 1)]);
    assert_eq!(rig.draw(4, &corner(2.0, 2.0, 60.0, 0.5, 0xFFFF_0000)), 0);
    // The small target clears its part of the depth buffer and draws
    // there.
    assert_eq!(rig.run(|w| encode::render_target(w, Some(&small))), 0);
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF10_3050, 1.0, 0);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 20.0, 0.25, 0xFF00_FF00)), 0);
    // The screen's target again: the depth the small one left is
    // tested against.
    assert_eq!(rig.run(|w| encode::render_target(w, Some(&big))), 0);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 50.0, 0.4, 0xFF00_00FF)), 0);
}

#[test]
fn targets_of_two_sizes_share_a_depth_buffer_without_a_read_back() {
    let (mut rig, log) = hosted(takes_all);
    shared_depth(&mut rig);
    assert_eq!((log.borrow().read_backs, log.borrow().uploads), (0, 0));
    let (_, log) = same(takes_all, shared_depth);
    // The two colour buffers and the one depth buffer, at the end.
    assert_eq!(log.borrow().read_backs, 3);
}

/// The small target first, so the host's copy of the depth buffer
/// starts as the small view and grows.
fn shared_depth_small_first(rig: &mut Rig) {
    let big = target(rig);
    let small = Target {
        colour: small_surface(),
        depth: big.depth,
    };
    rig.states(&[(RS_ZENABLE, 1)]);
    assert_eq!(rig.run(|w| encode::render_target(w, Some(&small))), 0);
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF10_3050, 0.5, 0);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 20.0, 0.25, 0xFF00_FF00)), 0);
    assert_eq!(rig.run(|w| encode::render_target(w, Some(&big))), 0);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 60.0, 0.4, 0xFF00_00FF)), 0);
}

#[test]
fn a_copy_of_a_view_grows_with_its_pixels() {
    let (_, log) = same(takes_all, shared_depth_small_first);
    assert_eq!(log.borrow().read_backs, 3);
}

/// The small target draws into the host's copy of the whole depth
/// buffer while card memory has all of it, the processor writes a
/// part of the buffer the small target doesn't reach, and the screen's
/// target draws over that part.
fn depth_view_then_a_write(rig: &mut Rig) {
    shared_depth(rig);
    rig.card.settle(&mut rig.memory, None);
    let big = target(rig);
    let small = Target {
        colour: small_surface(),
        depth: big.depth,
    };
    assert_eq!(rig.run(|w| encode::render_target(w, Some(&small))), 0);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 20.0, 0.25, 0xFF00_FF00)), 0);
    rig.card.settle(&mut rig.memory, None);
    // The nearest depth in the bottom rows.
    let rows = (DEPTH + (SIZE - 4) * SIZE * 2) as usize;
    poke(rig, rows, &[0; (SIZE * 2 * 4) as usize]);
    assert_eq!(rig.run(|w| encode::render_target(w, Some(&big))), 0);
    let size = 2.0 * SIZE as f32;
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, size, 0.5, 0xFF00_00FF)), 0);
}

#[test]
fn a_draw_into_a_view_leaves_the_whole_copy_to_be_read_back() {
    // The host's copy is of the whole depth buffer, so the card reads
    // all of it back: a copy left ahead after a read-back of the view
    // would never take the processor's write.
    let (rig, _) = same(takes_all, depth_view_then_a_write);
    assert_eq!(rig.depth(0, SIZE - 1), 0);
}

/// Draw into a surface, sample it, draw into it again and sample it
/// again, with the host's copy of it read back in between by a write
/// of the processor elsewhere in it.
fn render_to_texture_twice(rig: &mut Rig) {
    render_to_texture(rig);
    let surface = texture_surface();
    let (first, second) = (
        Target {
            colour: surface,
            depth: None,
        },
        target(rig),
    );
    let error = rig.run(|w| {
        encode::render_target(w, Some(&first));
        encode::set_texture(w, 0, 0);
    });
    assert_eq!(error, 0);
    rig.stage(0, &[(TSS_COLORARG1, ARG_DIFFUSE)]);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 30.0, 0.0, 0xFF11_2233)), 0);
    let error = rig.run(|w| {
        encode::render_target(w, Some(&second));
        encode::set_texture(w, 0, 1);
    });
    assert_eq!(error, 0);
    rig.stage(0, &[(TSS_COLORARG1, ARG_TEXTURE)]);
    assert_eq!(rig.draw(5, &square(48.0, 0xFFFF_FFFF)), 0);
}

#[test]
fn a_texture_that_is_a_target_comes_from_the_hosts_copy() {
    let (mut rig, log) = hosted(takes_all);
    render_to_texture_twice(&mut rig);
    // The surface sampled stayed ahead: nothing was read back.
    assert_eq!(log.borrow().read_backs, 0);
    assert!(log.borrow().images_from_copies >= 2);
    let (rig, _) = same(takes_all, render_to_texture_twice);
    assert_eq!(rig.px(2, 6), 0x11_2233);
}

/// Fills and copies of surfaces the host has ahead, and of ones it
/// hasn't.
fn blits(rig: &mut Rig) {
    let colour = rig.colour();
    let surface = texture_surface();
    let other = Target {
        colour: surface,
        depth: None,
    };
    let screen = target(rig);
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0);
    assert_eq!(rig.draw(4, &corner(2.0, 2.0, 40.0, 0.5, 0xFFFF_0000)), 0);
    let error = rig.run(|w| {
        // Into the target, and from it into a surface in card memory.
        command::encode::fill(w, &colour, &rect(4, 30, 20, 10), 0x1234_5678);
        command::encode::blt(
            w,
            &surface,
            &rect(0, 0, 32, 32),
            &colour,
            &rect(0, 0, 32, 32),
            None,
        );
        // Within the target, apart and over itself.
        command::encode::blt(
            w,
            &colour,
            &rect(40, 40, 16, 16),
            &colour,
            &rect(0, 0, 16, 16),
            None,
        );
        command::encode::blt(
            w,
            &colour,
            &rect(4, 4, 16, 16),
            &colour,
            &rect(0, 0, 16, 16),
            None,
        );
        // What the processor does: a key, and a stretch.
        let key = Some(0x20_2020);
        command::encode::blt(
            w,
            &colour,
            &rect(0, 48, 8, 8),
            &colour,
            &rect(0, 0, 8, 8),
            key,
        );
        command::encode::blt(
            w,
            &colour,
            &rect(48, 0, 16, 16),
            &colour,
            &rect(0, 0, 8, 8),
            None,
        );
    });
    assert_eq!(error, 0);
    // Draw into the surface copied to, and copy all of it back.
    assert_eq!(rig.run(|w| encode::render_target(w, Some(&other))), 0);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 20.0, 0.0, 0xFF00_FF00)), 0);
    assert_eq!(rig.run(|w| encode::render_target(w, Some(&screen))), 0);
    let all = rect(0, 0, SIZE, SIZE);
    assert_eq!(
        rig.run(|w| command::encode::blt(w, &colour, &all, &surface, &all, None)),
        0
    );
}

#[test]
fn fills_and_copies_run_on_the_host_where_a_surface_is_ahead() {
    let (mut rig, log) = hosted(takes_all);
    blits(&mut rig);
    assert_eq!(log.borrow().fills, 1);
    assert!(log.borrow().blits >= 3);
    let (rig, _) = same(takes_all, blits);
    assert_eq!(rig.px(5, 31), 0x1234_5678);
}

#[test]
fn the_screen_shows_a_surface_from_the_hosts_copy_while_it_is_current() {
    let (mut rig, log) = hosted(takes_all);
    let colour = rig.colour();
    // The host has no copy of a surface nothing was drawn into.
    assert!(!rig.card.host_shows(&colour, &mut rig.memory));
    assert!(!rig.card.host_presents());

    triangles(&mut rig);
    // The copy is ahead: the screen shows it, and the frame's read-back
    // of the picture is left out, once for each time it is asked.
    assert!(rig.card.host_shows(&colour, &mut rig.memory));
    assert!(rig.card.host_presents());
    assert!(!rig.card.host_presents());
    assert!(rig.card.ahead());
    assert_eq!(log.borrow().read_backs, 0);

    // A screenshot reads the picture back: the copy is what card memory
    // holds, and still shown.
    let (first, end) = gpu::range(&colour);
    rig.card.settle(&mut rig.memory, Some((first, end)));
    assert_eq!(log.borrow().read_backs, 1);
    assert!(rig.card.host_shows(&colour, &mut rig.memory));

    // The processor draws into it, as GDI does: card memory is the
    // picture until the host has the surface again.
    poke(&mut rig, first as usize, &[1, 2, 3, 4]);
    assert!(!rig.card.host_shows(&colour, &mut rig.memory));
    assert!(!rig.card.host_presents());
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 30.0, 0.0, 0xFF11_2233)), 0);
    assert!(rig.card.host_shows(&colour, &mut rig.memory));
}

/// Make `colour` the render target, with no depth buffer.
fn aim_at(rig: &mut Rig, colour: Surface) {
    let target = Target {
        colour,
        depth: None,
    };
    aim(rig, &target);
}

/// Make `colour` the render target, with no depth buffer and no
/// texture, and draw a corner of `diffuse` into it.
fn draw_into(rig: &mut Rig, colour: Surface, diffuse: u32) {
    aim_at(rig, colour);
    assert_eq!(rig.run(|w| encode::set_texture(w, 0, 0)), 0);
    rig.stage(0, &[(TSS_COLORARG1, ARG_DIFFUSE)]);
    assert_eq!(rig.draw(4, &corner(1.0, 2.0, 40.0, 0.0, diffuse)), 0);
}

/// Draw the texture `render_to_texture` defined into a square of the
/// screen's target at (`x`, `y`).
fn sample_into_screen(rig: &mut Rig, x: f32, y: f32) {
    let screen = target(rig);
    aim(rig, &screen);
    assert_eq!(rig.run(|w| encode::set_texture(w, 0, 1)), 0);
    rig.stage(0, &[(TSS_COLORARG1, ARG_TEXTURE)]);
    assert_eq!(rig.draw(5, &quad(x, y, 16.0)), 0);
}

/// A write of the processor into a surface the host has ahead, before
/// each of the paths that keep it on the host: a sample of it as a
/// texture, a `BLT` from it and into it, a `CLEAR` of a part of it
/// and of all of it. Each sees the write.
fn writes_into_a_surface_ahead(rig: &mut Rig) {
    render_to_texture(rig);
    let s = texture_surface();
    let screen = rig.colour();
    let poke_at = |rig: &mut Rig, x: u32, y: u32| {
        poke(rig, s.at(x, y), &[0x11, 0x22, 0x33, 0x44]);
    };
    draw_into(rig, s, 0xFF20_4060);
    poke_at(rig, 3, 3);
    sample_into_screen(rig, 0.0, 0.0);

    draw_into(rig, s, 0xFF60_4020);
    poke_at(rig, 5, 5);
    let blt = |rig: &mut Rig, dst: &Surface, to: Rect, src: &Surface, from: Rect| {
        let error = rig.run(|w| command::encode::blt(w, dst, &to, src, &from, None));
        assert_eq!(error, 0);
    };
    blt(rig, &screen, rect(20, 0, 16, 16), &s, rect(0, 0, 16, 16));

    draw_into(rig, s, 0xFF40_6020);
    poke_at(rig, 6, 6);
    blt(rig, &s, rect(0, 0, 8, 8), &screen, rect(40, 40, 8, 8));
    sample_into_screen(rig, 40.0, 0.0);

    draw_into(rig, s, 0xFF20_6040);
    poke_at(rig, 7, 7);
    aim_at(rig, s);
    let part = [rect(0, 0, 4, 4)];
    let error = rig.run(|w| encode::clear(w, CLEAR_COLOUR, 0xFF0A_0B0C, 0.0, 0, &part));
    assert_eq!(error, 0);
    sample_into_screen(rig, 0.0, 20.0);

    draw_into(rig, s, 0xFF40_2060);
    poke_at(rig, 9, 9);
    aim_at(rig, s);
    rig.clear(CLEAR_COLOUR, 0xFF0D_0E0F, 0.0, 0);
    sample_into_screen(rig, 20.0, 20.0);
}

#[test]
fn a_write_into_a_surface_that_is_ahead_is_seen_by_every_host_path() {
    let (mut rig, log) = hosted(takes_all);
    writes_into_a_surface_ahead(&mut rig);
    // The clear of all of the surface was the host's, and the copy it
    // wrote is shown.
    assert!(rig.card.host_shows(&texture_surface(), &mut rig.memory));
    assert!(log.borrow().blits >= 1);
    let (rig, _) = same(takes_all, writes_into_a_surface_ahead);
    // The squares sampled after the write of a part, and after the
    // clear of all of it.
    assert_eq!(rig.px(0, 20), 0x0A_0B0C);
    assert_eq!(rig.px(24, 24), 0x0D_0E0F);
}

/// A clear of all of a target the host has no copy of, which starts
/// and ends inside pages it shares with other bytes, then a write of
/// the processor to one of those bytes, and a clear of the target with
/// a rectangle that isn't all of it.
fn clears_of_a_target_without_a_copy(rig: &mut Rig) {
    let colour = unaligned();
    draw_into(rig, colour, 0xFF20_2020);
    rig.card.settle(&mut rig.memory, None);
    // The copy is stale after this, and the clear needs none of it.
    poke(rig, colour.at(1, 1), &[9, 9, 9, 9]);
    rig.clear(CLEAR_COLOUR, 0xFF30_4050, 0.0, 0);
    // Beside it, in its first and last pages: no read-back, and the
    // read-back of the surface later leaves the bytes as they are.
    poke(rig, colour.offset as usize - 4, &[1, 2, 3, 4]);
    poke(rig, gpu::range(&colour).1 as usize, &[5, 6, 7, 8]);
    // A clear of a part of a surface the host has no copy of is the
    // processor's.
    let other = Surface {
        offset: BUFFER,
        ..small_surface()
    };
    aim_at(rig, other);
    let part = [rect(0, 0, 8, 8)];
    let error = rig.run(|w| encode::clear(w, CLEAR_COLOUR, 0xFF00_7700, 0.0, 0, &part));
    assert_eq!(error, 0);
}

#[test]
fn a_whole_clear_without_a_copy_leaves_the_bytes_beside_the_surface() {
    let (mut rig, log) = hosted(takes_all);
    clears_of_a_target_without_a_copy(&mut rig);
    // The whole clear was the host's, with nothing copied first; the
    // clear of a part of a surface it has no copy of wasn't.
    assert_eq!(log.borrow().clears, 1);
    assert_eq!(log.borrow().uploads, 1);
    assert!(rig.card.ahead());
    let (rig, _) = same(takes_all, clears_of_a_target_without_a_copy);
    let colour = unaligned();
    assert_eq!(rig.memory[colour.offset as usize - 4..][..4], [1, 2, 3, 4]);
    let after = gpu::range(&colour).1 as usize;
    assert_eq!(rig.memory[after..][..4], [5, 6, 7, 8]);
    assert_eq!(rig.word(colour.at(1, 1), 4), 0x30_4050);
}

/// The colour buffer's copy stale and the depth buffer's ahead, a
/// clear of the depth alone, and the reverse.
fn one_buffer_stale(rig: &mut Rig) {
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0);
    rig.states(&[(RS_ZENABLE, 1)]);
    assert_eq!(rig.draw(4, &corner(2.0, 2.0, 40.0, 0.5, 0xFFFF_0000)), 0);
    rig.card.settle(&mut rig.memory, None);
    let at = rig.colour().at(30, 30);
    poke(rig, at, &[1, 2, 3, 4]);
    let part = [rect(0, 0, 32, 32)];
    let error = rig.run(|w| encode::clear(w, CLEAR_DEPTH, 0, 0.25, 0, &part));
    assert_eq!(error, 0);
    assert_eq!(rig.draw(4, &corner(20.0, 20.0, 40.0, 0.4, 0xFF00_FF00)), 0);
    rig.card.settle(&mut rig.memory, None);
    poke(rig, DEPTH as usize + 2, &[0, 0]);
    let error = rig.run(|w| encode::clear(w, CLEAR_COLOUR, 0xFF00_0080, 0.0, 0, &part));
    assert_eq!(error, 0);
    assert_eq!(rig.draw(4, &corner(4.0, 4.0, 40.0, 0.3, 0xFF00_00FF)), 0);
}

#[test]
fn a_clear_of_one_buffer_takes_the_others_stale_copy_from_card_memory() {
    same(takes_all, one_buffer_stale);
}

/// Two targets of other pitches and formats over the bytes of one the
/// host has ahead, each drawn into and seen through the other.
fn other_views_of_the_same_bytes(rig: &mut Rig) {
    let whole = texture_surface();
    let narrow = Surface {
        pitch: whole.pitch / 2,
        width: SIZE / 2,
        ..whole
    };
    let packed = Surface {
        format: Format::R5G6B5,
        ..whole
    };
    draw_into(rig, whole, 0xFF40_8020);
    draw_into(rig, narrow, 0xFF20_4080);
    draw_into(rig, whole, 0xFF80_2040);
    draw_into(rig, packed, 0xFF10_2030);
    sample_into_screen(rig, 0.0, 0.0);
    draw_into(rig, whole, 0xFF30_2010);
}

#[test]
fn surfaces_of_other_pitches_and_formats_over_one_copy_agree() {
    // `render_to_texture` defines the texture `sample_into_screen`
    // draws.
    fn scene(rig: &mut Rig) {
        render_to_texture(rig);
        other_views_of_the_same_bytes(rig);
    }
    same(takes_all, scene);
}

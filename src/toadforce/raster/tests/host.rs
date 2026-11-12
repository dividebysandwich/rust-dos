//! The card with a host renderer attached (`toadforce::gpu`): a scene
//! drawn through a host, and read back, is byte for byte what the
//! software renderer draws.
//!
//! The host here keeps a copy of each surface it draws into, as an
//! OpenGL one keeps a texture, and draws into the copy with the
//! software renderer's own spans. Card memory gets the copy only when
//! the card asks for it, so a read-back the card leaves out shows as
//! a difference.

use super::*;
use crate::toadforce::command;
use crate::toadforce::gpu::{self, Batch, Host, Pages, Stamp};
use crate::toadforce::raster::mem::Mem;
use crate::toadforce::raster::pipeline::{self, DrawState};
use crate::toadforce::raster::setup::Prim;
use crate::toadforce::raster::span;
use std::cell::RefCell;
use std::rc::Rc;

/// What the host was asked for.
#[derive(Default)]
struct Log {
    /// Copies made from card memory.
    uploads: u32,
    batches: u32,
    triangles: u32,
    read_backs: u32,
    forgets: u32,
}

/// Picks the states a host refuses.
type Declines = fn(&DrawState) -> bool;

/// A surface as the host has it.
struct Copy {
    surface: Surface,
    stamp: Stamp,
    /// The pixels, row after row without what is between the rows.
    pixels: Vec<u8>,
}

struct TestHost {
    copies: Vec<Copy>,
    /// The states the host refuses.
    declines: Declines,
    log: Rc<RefCell<Log>>,
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

impl Host for TestHost {
    fn name(&self) -> &str {
        "test"
    }

    fn takes(&self, state: &DrawState) -> bool {
        !(self.declines)(state)
    }

    fn draw(&mut self, batch: &Batch, memory: &[u8], pages: &Pages) {
        let mut log = self.log.borrow_mut();
        log.batches += 1;
        let surfaces: Vec<Surface> = std::iter::once(batch.target.colour)
            .chain(batch.target.depth_surface())
            .collect();
        // Card memory with the host's copies of the target over it is
        // what the spans draw into.
        let mut scratch = memory.to_vec();
        for surface in &surfaces {
            let range = gpu::range(surface);
            let found = self.copies.iter().position(|c| c.surface == *surface);
            let at = found.unwrap_or_else(|| {
                self.copies.push(Copy {
                    surface: *surface,
                    stamp: Stamp::default(),
                    pixels: Vec::new(),
                });
                self.copies.len() - 1
            });
            let copy = &mut self.copies[at];
            if found.is_none() || pages.changed(range, &mut copy.stamp) {
                copy.pixels = pixels(surface, memory);
                copy.stamp = pages.stamp(range);
                log.uploads += 1;
            }
            put(surface, &copy.pixels, &mut scratch);
        }
        let slots: [u8; pipeline::SLOTS] = std::array::from_fn(|i| i as u8);
        let mem = Mem::new(&mut scratch);
        let mut chunk = span::Chunk::new();
        for run in batch.runs {
            let state = &*run.state;
            let ctx = span::Ctx::new(mem, state);
            let sets = &slots[..state.coords.len()];
            for t in batch.vertices[run.vertices.clone()].chunks(run.stride()) {
                log.triangles += 1;
                let provoking = t.get(3).unwrap_or(&t[0]);
                let v = [&t[0], &t[1], &t[2]];
                let Some(prim) = Prim::triangle(v, provoking, &state.shape, state.clip, sets)
                else {
                    continue;
                };
                for y in prim.rows.0..prim.rows.1 {
                    ctx.row(&prim, y, &mut chunk);
                }
            }
        }
        for surface in &surfaces {
            let copy = self.copies.iter_mut().find(|c| c.surface == *surface);
            copy.unwrap().pixels = pixels(surface, &scratch);
        }
    }

    fn read_back(&mut self, surface: &Surface, memory: &mut [u8]) -> bool {
        self.log.borrow_mut().read_backs += 1;
        match self.copies.iter().find(|c| c.surface == *surface) {
            Some(copy) => {
                put(surface, &copy.pixels, memory);
                true
            }
            None => false,
        }
    }

    fn forget(&mut self) {
        self.log.borrow_mut().forgets += 1;
        self.copies.clear();
    }
}

fn takes_all(_: &DrawState) -> bool {
    false
}

/// A host that has no shader for flat shading yet.
fn declines_flat(state: &DrawState) -> bool {
    state.shape.flat
}

const FORMAT: Format = Format::X8R8G8B8;
const DEPTH_FORMAT: Format = Format::D16;

/// A rig with a host that refuses the states `declines` picks.
fn hosted(declines: Declines) -> (Rig, Rc<RefCell<Log>>) {
    let mut rig = Rig::new(FORMAT, Some(DEPTH_FORMAT));
    let log = Rc::new(RefCell::new(Log::default()));
    let host = TestHost {
        copies: Vec::new(),
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
    rig.card.settle(&mut rig.memory, None);
    assert!(!rig.card.ahead());
    assert!(rig.memory == software.memory);
    (rig, log)
}

/// The processor writes `bytes` to card memory at `at`, as the bus
/// does it: what the host drew there is read back first, and the
/// write is counted.
fn poke(rig: &mut Rig, at: usize, bytes: &[u8]) {
    let (first, len) = (at as u32, bytes.len() as u32);
    rig.card.settle(&mut rig.memory, Some((first, first + len)));
    rig.memory[at..at + bytes.len()].copy_from_slice(bytes);
    rig.card.wrote(first, len);
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
    // The clear ran on the processor, and the triangles are the
    // host's alone.
    assert!(rig.card.ahead());
    assert!(rig.drawn(0x20_2020).is_empty());
    assert_eq!(rig.depth(4, 4), 0xFFFF);
    assert_eq!(log.borrow().read_backs, 0);

    let (settled, log) = same(takes_all, triangles);
    assert!(!settled.drawn(0x20_2020).is_empty());
    assert!(settled.depth(4, 4) < 0xFFFF);
    let log = log.borrow();
    // Two triangles and a strip of two, in three batches, with the
    // colour and the depth buffer copied once and read back once.
    assert_eq!((log.batches, log.triangles), (3, 4));
    assert_eq!((log.uploads, log.read_backs), (2, 2));
}

/// Draws of the host and of the software renderer, with the card's
/// other commands and a write of the processor between them.
fn mixed(rig: &mut Rig) {
    let colour = rig.colour();
    rig.clear(CLEAR_COLOUR | CLEAR_DEPTH, 0xFF20_2020, 1.0, 0);
    rig.states(&[(RS_ZENABLE, 1)]);
    // One run of commands: triangles, lines the host never gets,
    // triangles again, a clear of a corner and a triangle over it.
    let error = rig.run(|w| {
        encode::draw(w, 4, TL, 3, &corner(2.0, 2.0, 40.0, 0.5, 0xFFFF_0000));
        let line = [
            vertex_z(0.0, 20.5, 0.1, 0xFFFF_FFFF, 0.0, 0.0),
            vertex_z(60.0, 24.5, 0.1, 0xFFFF_FFFF, 0.0, 0.0),
        ]
        .concat();
        encode::draw(w, 2, TL, 2, &line);
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
    let hosts: [(Declines, u32); 2] = [(takes_all, 1), (declines_flat, 2)];
    for (declines, declined) in hosts {
        let (rig, log) = same(declines, mixed);
        // What the processor wrote and no draw covered is there.
        assert_eq!(rig.px(7, 50), 0xAAAA_AAAA);
        let host = rig.card.describe(Rig::live());
        assert_eq!(host["renderer"], "test");
        assert_eq!(host["host"]["declined_draws"], declined);
        assert_eq!(host["host"]["stale_reads"], 0);
        let log = log.borrow();
        assert_eq!(host["host"]["read_backs"], log.read_backs);
        // Every write of the processor into the target made the host
        // copy it again.
        assert!(log.uploads > 2 && log.read_backs > 2);
    }
}

#[test]
fn a_write_of_the_processor_makes_the_host_copy_the_target_again() {
    let (mut rig, log) = hosted(takes_all);
    rig.clear(CLEAR_COLOUR, 0xFF20_2020, 0.0, 0);
    assert_eq!(rig.draw(4, &corner(0.0, 0.0, 8.0, 0.0, 0xFFFF_0000)), 0);
    assert_eq!(rig.draw(4, &corner(20.0, 0.0, 8.0, 0.0, 0xFFFF_0000)), 0);
    // Two draws into a target nobody else touched: one copy of the
    // colour and one of the depth buffer.
    assert_eq!(log.borrow().uploads, 2);
    // A write beside the target leaves the copies good.
    poke(&mut rig, BUFFER as usize, &[1; 64]);
    assert_eq!(rig.draw(4, &corner(40.0, 0.0, 8.0, 0.0, 0xFFFF_0000)), 0);
    assert_eq!((log.borrow().uploads, log.borrow().read_backs), (2, 0));
    // One into the colour buffer reads it back, and the next draw
    // copies it again, the colour buffer alone.
    let at = rig.colour().at(1, 40);
    poke(&mut rig, at, &[0x55; 4]);
    assert_eq!(log.borrow().read_backs, 1);
    assert_eq!(rig.px(1, 1), 0xFF_0000);
    assert_eq!(rig.draw(4, &corner(0.0, 20.0, 8.0, 0.0, 0xFF00_FF00)), 0);
    assert_eq!(log.borrow().uploads, 3);
    rig.card.settle(&mut rig.memory, None);
    assert_eq!(rig.px(1, 40), 0x5555_5555);
    assert_eq!(rig.px(1, 21), 0x00_FF00);
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
    fn declines_textured(state: &DrawState) -> bool {
        state.tex[0].is_some()
    }
    same(declines_textured, render_to_texture);
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
    // pixels of after the save: it drops them.
    rig.card.load(&mut Reader::new(&w.buf)).unwrap();
    rig.memory = saved.clone();
    rig.card.forget();
    assert_eq!(log.borrow().forgets, 1);
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

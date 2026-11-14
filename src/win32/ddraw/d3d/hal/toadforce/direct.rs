//! A draw of screen vertices, from the program's memory to the card's
//! command queue.
//!
//! A program that transforms its own vertices (`D3DFVF_XYZRHW`) hands
//! Direct3D what the card draws. The pipeline works out nothing for a
//! triangle of them that needs no cutting: it copies the position and
//! the colours, picks each stage's set of texture coordinates, and
//! culls by the triangle's winding. `draw` here does those three for a
//! whole draw at once and writes one `DRAW_INDEXED` where the card
//! reads it:
//!
//! - The vertices go from the program's memory into the queue once.
//!   Where the program's layout is the card's (`Plan::same`: the
//!   colours present, and each stage in use taking the set of its own
//!   number), they are read there as they are. Otherwise they are read
//!   into a buffer the HAL keeps and each word of a card vertex is
//!   taken from its place in the program's (`Plan::parts`). The plan
//!   is made once a draw from the layout and the bind, for any FVF
//!   code Direct3D takes.
//! - The indices name the vertices as the program's did, three for
//!   each triangle the cull mode keeps (`pipeline::area`, the
//!   pipeline's own sum). A strip and a fan become the list of their
//!   triangles.
//!
//! The card then gets the same triangles of the same vertices, in the
//! same order, as the pipeline gives it for the draw: the test
//! `a_direct_draw_is_the_pipelines_triangles` holds the two against
//! each other.
//!
//! The pipeline keeps a draw (`draw` gives None) that it does more for:
//! points and lines, the fill modes that draw them, flat shading, which
//! gives a triangle's vertices one colour, a clip status or extents the
//! program reads back, any vertex outside the band or the depths the
//! pipeline cuts at (`pipeline::tl_inside`), which may leave a triangle
//! to cut, more vertices than one command holds, and vertices in
//! memory that isn't plain RAM.

use super::super::super::device::Device;
use super::super::super::pipeline::{
    self, FILL_POINT, FILL_WIREFRAME, Layout, Part, SHADE_FLAT, Setup, TRIANGLEFAN, TRIANGLELIST,
    TRIANGLESTRIP,
};
use super::{COMMAND_WORDS, Card, VERTEX_WORDS, bind, drew, hal};
use crate::cpu::Cpu;
use crate::toadforce::command::header;
use crate::toadforce::level1;

/// The words of a card's screen vertex at most: the position, the two
/// colours and four sets of four numbers.
const WORDS_MOST: usize = VERTEX_WORDS + 4 * 4;

/// Where a draw's vertices are in the program's memory, which the
/// caller has probed: `count` of them from `at`, and its indices if it
/// has them, where they are and how many.
#[derive(Clone, Copy, Debug)]
pub struct Source {
    pub at: u32,
    pub count: u32,
    pub indices: Option<(u32, u32)>,
}

/// Where a word of a card vertex comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum From {
    /// This word of the program's vertex.
    Word(u32),
    /// The same for every vertex.
    Value(u32),
}

/// How the program's vertices of one layout become the card's for one
/// bind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Plan {
    parts: [From; WORDS_MOST],
    /// The words of a card vertex.
    words: usize,
    /// The program's vertex is the card's, word for word.
    same: bool,
}

impl Plan {
    /// The plan for transformed vertices of `layout` drawn as `card`
    /// was bound for `setup`: the words `pipeline::screen` and
    /// `Raster::add` give the card for such a vertex.
    fn new(layout: &Layout, setup: &Setup, card: &Card) -> Plan {
        let mut parts = [From::Value(0); WORDS_MOST];
        for (word, part) in parts.iter_mut().enumerate().take(4) {
            *part = From::Word(word as u32);
        }
        // A vertex without a diffuse colour is white, and one without a
        // specular colour has none (`pipeline::transformed`).
        let colour = |at: Option<u32>, absent: u32| match at {
            Some(at) => From::Word(at / 4),
            None => From::Value(absent),
        };
        parts[4] = colour(layout.diffuse, 0xFFFF_FFFF);
        parts[5] = colour(layout.specular, 0xFF00_0000);
        let numbers = card.numbers();
        let mut words = VERTEX_WORDS;
        for stage in 0..card.sets {
            let set = setup.coords[stage].screen_parts(layout, stage < setup.screen_stages);
            for part in &set[..numbers] {
                parts[words] = match *part {
                    Part::Word(word) => From::Word(word),
                    Part::Number(number) => From::Value(number.to_bits()),
                };
                words += 1;
            }
        }
        let same = words as u32 * 4 == layout.size
            && parts[..words]
                .iter()
                .enumerate()
                .all(|(word, part)| *part == From::Word(word as u32));
        Plan { parts, words, same }
    }
}

/// The buffers a draw reads the program's memory into, kept from draw
/// to draw for their room.
#[derive(Clone, Debug, Default)]
pub struct Scratch {
    /// The vertices of a draw whose layout isn't the card's.
    vertices: Vec<u8>,
    /// The indices as the program has them, and those of the triangles
    /// kept.
    indices: Vec<u16>,
    kept: Vec<u16>,
}

/// The bytes of `words`, to be filled from the program's memory:
/// `from_le` then makes them the host's numbers.
fn bytes_of<T: Word>(words: &mut [T]) -> &mut [u8] {
    // SAFETY: `T` is `u16` or `u32`, which have no padding and take
    // every bit pattern, and a byte has no alignment to keep. The
    // bytes are those of `words`, borrowed as long.
    unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr().cast(), size_of_val(words)) }
}

/// A number `bytes_of` gives the bytes of.
trait Word: Copy {
    fn host(le: Self) -> Self;
}

impl Word for u16 {
    fn host(le: u16) -> u16 {
        u16::from_le(le)
    }
}

impl Word for u32 {
    fn host(le: u32) -> u32 {
        u32::from_le(le)
    }
}

/// The little-endian numbers in `words`, read there as bytes, as the
/// host's: nothing to do on a little-endian host.
fn from_le<T: Word>(words: &mut [T]) {
    if cfg!(target_endian = "big") {
        for word in words {
            *word = T::host(*word);
        }
    }
}

/// The places in a draw's order of the vertices of triangle `t` of a
/// draw of `kind`, as `pipeline::assemble` has them.
fn corners(kind: u32, t: usize) -> [usize; 3] {
    match kind {
        TRIANGLELIST => [3 * t, 3 * t + 1, 3 * t + 2],
        // Every other triangle of a strip is wound the other way.
        TRIANGLESTRIP if t % 2 == 1 => [t + 1, t, t + 2],
        TRIANGLESTRIP => [t, t + 1, t + 2],
        _ => [0, t + 1, t + 2],
    }
}

impl Plan {
    /// Add the card's vertices of the program's in `bytes`, each
    /// `size` bytes, to `words`.
    fn gather(&self, words: &mut Vec<u32>, size: usize, bytes: &[u8]) {
        for vertex in bytes.chunks_exact(size) {
            words.extend(self.parts[..self.words].iter().map(|part| match *part {
                From::Word(word) => {
                    let at = 4 * word as usize;
                    u32::from_le_bytes(vertex[at..at + 4].try_into().unwrap())
                }
                From::Value(value) => value,
            }));
        }
    }
}

/// A draw of `count` screen vertices as the card's command.
struct Draw<'a> {
    plan: &'a Plan,
    /// The card's layout of the vertices (`Card::layout`).
    fvf: u32,
    setup: &'a Setup,
    /// D3DPT_TRIANGLELIST, TRIANGLESTRIP or TRIANGLEFAN.
    kind: u32,
    count: usize,
    /// The program's indices, if the draw has them.
    order: Option<&'a [u16]>,
}

/// How many triangles `named` vertices make at most in a draw of
/// `kind`: as many as the draw names, through its indices or as they
/// are.
fn triangles(kind: u32, named: usize) -> usize {
    match kind {
        TRIANGLELIST => named / 3,
        _ => named.saturating_sub(2),
    }
}

impl Draw<'_> {
    /// How many triangles the draw's vertices make at most.
    fn triangles(&self) -> usize {
        let named = self.order.map_or(self.count, |order| order.len());
        triangles(self.kind, named)
    }

    /// Add the draw's `DRAW_INDEXED` to `words`: the triangles drawn.
    /// `vertices` adds the card's vertices, or says false when it
    /// can't. None, with `words` as they were, when it can't or a
    /// vertex is one the pipeline might cut a triangle at. `kept` is
    /// room for the indices of the triangles drawn.
    fn command(
        &self,
        words: &mut Vec<u32>,
        kept: &mut Vec<u16>,
        vertices: impl FnOnce(&mut Vec<u32>) -> bool,
    ) -> Option<u32> {
        let (plan, setup, count) = (self.plan, self.setup, self.count);
        let start = words.len();
        words.extend([0, TRIANGLELIST, self.fvf, count as u32, 0]);
        let first = words.len();
        let place = |words: &[u32], i: usize| -> [f32; 3] {
            let at = first + i * plan.words;
            [words[at], words[at + 1], words[at + 2]].map(f32::from_bits)
        };
        let mut inside = pipeline::tl_inside(setup);
        if !vertices(words)
            || words.len() != first + count * plan.words
            || !(0..count).all(|i| inside(place(words, i)))
        {
            words.truncate(start);
            return None;
        }
        // The triangles the pipeline draws: those of vertices the draw
        // has, which the cull mode keeps.
        kept.clear();
        for t in 0..self.triangles() {
            let tri = corners(self.kind, t).map(|i| self.order.map_or(i, |o| o[i] as usize));
            if tri.iter().any(|&i| i >= count) {
                continue;
            }
            let area = pipeline::area(3, |i| {
                let [x, y, _] = place(words, tri[i]);
                (x, y)
            });
            if !pipeline::culled(setup.cull, area) {
                kept.extend(tri.map(|i| i as u16));
            }
        }
        if kept.is_empty() {
            words.truncate(start);
            return Some(0);
        }
        words.extend(
            kept.chunks(2)
                .map(|pair| pair[0] as u32 | (pair.get(1).copied().unwrap_or(0) as u32) << 16),
        );
        words[start] = header(level1::DRAW_INDEXED, words.len() - start);
        words[start + 4] = kept.len() as u32;
        Some(kept.len() as u32 / 3)
    }
}

/// Draw `kind` primitives of the device `d`'s transformed vertices of
/// `layout` at `from`: the triangles drawn, or None for a draw the
/// pipeline keeps (see the module comment), with nothing drawn.
pub fn draw(
    cpu: &mut Cpu,
    d: &Device,
    setup: &Setup,
    layout: &Layout,
    kind: u32,
    from: &Source,
) -> Option<u32> {
    let whole = layout.rhw
        && (TRIANGLELIST..=TRIANGLEFAN).contains(&kind)
        && !matches!(setup.fill, FILL_POINT | FILL_WIREFRAME)
        && setup.shade != SHADE_FLAT
        && d.clip_status[0] & 3 == 0;
    let count = from.count as usize;
    let triangles = triangles(kind, from.indices.map_or(count, |(_, n)| n as usize));
    if !whole || triangles == 0 || from.count > level1::DRAW_VERTICES {
        return None;
    }
    let card = bind(cpu, d, setup)?;
    let plan = Plan::new(layout, setup, &card);
    let (fvf, vertex_words) = card.layout();
    debug_assert_eq!(plan.words, vertex_words);
    if 5 + count * plan.words + (3 * triangles).div_ceil(2) > COMMAND_WORDS {
        return None;
    }

    let mut scratch = std::mem::take(&mut hal(cpu).scratch);
    // What isn't read into the queue is read before the queue is out
    // of the card, from any memory.
    if let Some((at, n)) = from.indices {
        scratch.indices.resize(n as usize, 0);
        cpu.bus.guest_read_bytes(at, bytes_of(&mut scratch.indices));
        from_le(&mut scratch.indices);
    }
    if !plan.same {
        scratch.vertices.resize(count * layout.size as usize, 0);
        cpu.bus.guest_read_bytes(from.at, &mut scratch.vertices);
    }
    let Scratch {
        vertices,
        indices,
        kept,
    } = &mut scratch;
    let order = from.indices.map(|_| &indices[..]);
    let draw = Draw {
        plan: &plan,
        fvf,
        setup,
        kind,
        count,
        order,
    };
    let drawn = cpu.bus.toadforce_submit_with(|bus, words| {
        draw.command(words, kept, |words| {
            if !plan.same {
                plan.gather(words, layout.size as usize, vertices);
                return true;
            }
            let first = words.len();
            words.resize(first + count * plan.words, 0);
            let read = bus.guest_read_ram(from.at, bytes_of(&mut words[first..]));
            from_le(&mut words[first..]);
            read
        })
    });
    hal(cpu).scratch = scratch;
    if drawn.is_some_and(|triangles| triangles != 0) {
        drew(cpu, &card);
    }
    drawn
}

#[cfg(test)]
mod tests {
    use super::super::super::super::pipeline::{Coord, IDENTITY, TTFF_PROJECTED};
    use super::super::{Raster, VERTEX_BAND};
    use super::*;
    use crate::toadforce::command;
    use crate::toadforce::level1::{Command, Indices, Vertices};

    /// Numbers that are the same on every run.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 16) as u32
        }

        fn below(&mut self, n: u32) -> u32 {
            self.next() % n
        }

        /// A number from `low` to `high`.
        fn between(&mut self, low: f32, high: f32) -> f32 {
            low + (high - low) * (self.below(1 << 20) as f32 / (1 << 20) as f32)
        }
    }

    /// The triangles the draw commands in `words` give the card, each
    /// as the words of its three vertices.
    fn triangles(words: &[u32]) -> Vec<[Vec<u32>; 3]> {
        let mut found = Vec::new();
        let mut at = 0;
        while at < words.len() {
            let len = command::length(words[at]);
            let Ok(command::Command::Level1(Command::Draw(draw))) =
                command::decode(&words[at..at + len], 1, 1 << 24)
            else {
                panic!("not a draw the card takes at word {at}");
            };
            at += len;
            assert_eq!(draw.primitive, TRIANGLELIST);
            let Vertices::Inline(vertices) = draw.vertices else {
                panic!("vertices in a buffer");
            };
            let vertex = |i: usize| vertices[i * draw.fvf.words..][..draw.fvf.words].to_vec();
            let order: Vec<usize> = match draw.indices {
                Indices::None => (0..draw.vertex_count()).collect(),
                Indices::Inline { words, count } => (0..count as usize)
                    .map(|i| (words[i / 2] >> (16 * (i % 2)) & 0xFFFF) as usize)
                    .collect(),
                Indices::Buffer { .. } => panic!("indices in a buffer"),
            };
            found.extend(order.as_chunks::<3>().0.iter().map(|t| t.map(vertex)));
        }
        found
    }

    /// A draw made up from `seed`: the layout, the setup and the card
    /// as a bind makes it for the setup, the kind, the program's
    /// vertices as bytes, and its indices if it has them.
    #[allow(clippy::type_complexity)]
    fn made_up(seed: u64) -> (Layout, Setup, Card, u32, Vec<u8>, Option<Vec<u16>>) {
        let mut r = Random(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        // Positions alone, the old transformed vertex, one colour, two
        // sets, a set of three numbers, sets of four and of one, and
        // four sets.
        let codes = [
            0x004,
            0x1C4,
            0x044,
            0x2C4,
            0x144 | 1 << 16,
            0x284 | 2 << 16 | 3 << 18,
            0x4C4,
        ];
        let layout = Layout::of(codes[r.below(codes.len() as u32) as usize]).unwrap();
        let mut setup = Setup {
            guard_band: VERTEX_BAND,
            clip: r.below(2) == 0,
            cull: 1 + r.below(3),
            screen_stages: [4, 4, 4, 2][r.below(4) as usize],
            ..Setup::default()
        };
        let sets = r.below(5) as usize;
        let mut projected = false;
        for stage in 0..sets {
            // A stage takes its own set, another one, or one the
            // vertex lacks, and is projected now and then, with a
            // count or without.
            // What makes coordinates for a vertex to transform (a
            // generated set, a matrix's count) leaves these alone.
            let index = [stage as u32, r.below(4), 7, stage as u32 | 2 << 16][r.below(4) as usize];
            let flags = [
                0,
                0,
                2,
                TTFF_PROJECTED,
                TTFF_PROJECTED | 2,
                TTFF_PROJECTED | 3,
            ];
            let flags = flags[r.below(6) as usize];
            projected |= flags & TTFF_PROJECTED != 0;
            setup.coords[stage] = Coord::of(index, flags, &IDENTITY);
        }
        let card = Card {
            sets,
            projected,
            cube: r.below(6) == 0,
            ..Card::default()
        };
        let kind = TRIANGLELIST + r.below(3);
        let count = 3 + r.below(40) as usize;
        // One draw in eight has a vertex the pipeline cuts at.
        let stray = (r.below(8) == 0).then(|| (r.below(count as u32) as usize, r.below(6)));
        let mut bytes = Vec::new();
        for i in 0..count {
            let mut words = vec![0u32; layout.size as usize / 4];
            let mut place = [
                r.between(-200.0, 900.0),
                r.between(-200.0, 700.0),
                r.between(0.0, 1.0),
                r.between(0.01, 2.0),
            ];
            match stray {
                Some((at, 0)) if at == i => place[0] = -20000.0,
                Some((at, 1)) if at == i => place[1] = 17000.0,
                Some((at, 2)) if at == i => place[2] = -0.25,
                Some((at, 3)) if at == i => place[2] = 1.5,
                Some((at, 4)) if at == i => place[0] = f32::NAN,
                Some((at, 5)) if at == i => place[2] = f32::INFINITY,
                _ => {}
            }
            // Corners that fall on each other give triangles with no
            // area.
            if i > 0 && r.below(10) == 0 {
                place[0] = 100.0;
                place[1] = 100.0;
            }
            for (word, number) in words.iter_mut().zip(place) {
                *word = number.to_bits();
            }
            for word in words.iter_mut().skip(4) {
                *word = r.between(-4.0, 4.0).to_bits();
            }
            for at in [layout.diffuse, layout.specular].into_iter().flatten() {
                words[at as usize / 4] = r.next();
            }
            bytes.extend(words.iter().flat_map(|word| word.to_le_bytes()));
        }
        let order = (r.below(3) != 0).then(|| {
            (0..3 + r.below(60))
                .map(|_| match r.below(50) {
                    // An index past the vertices now and then.
                    0 => count as u16 + 2,
                    _ => r.below(count as u32) as u16,
                })
                .collect()
        });
        (layout, setup, card, kind, bytes, order)
    }

    #[test]
    fn a_direct_draw_is_the_pipelines_triangles() {
        let (mut direct_draws, mut kept_draws) = (0, 0);
        for seed in 0..6000 {
            let (layout, setup, card, kind, bytes, order) = made_up(seed);
            let size = layout.size as usize;
            let count = bytes.len() / size;

            // Through the pipeline, as `draw::draw_set_up` does.
            let vertices: Vec<_> = bytes.chunks_exact(size).map(|v| layout.read(v)).collect();
            let wide: Option<Vec<u32>> = order
                .as_ref()
                .map(|order| order.iter().map(|&i| i as u32).collect());
            let all: Vec<u32> = (0..count as u32).collect();
            let prims = pipeline::assemble(kind, wide.as_deref().unwrap_or(&all)).unwrap();
            let processed = pipeline::process(&setup, &vertices, &layout);
            let mut cpu = Cpu::new(std::path::PathBuf::from("."));
            let mut raster = Raster::new(&mut cpu, card);
            let drawn = pipeline::draw(&setup, &processed, &prims, &mut raster);
            raster.close();
            let through = std::mem::take(&mut raster.words);

            // And from the program's memory.
            let plan = Plan::new(&layout, &setup, &card);
            let (fvf, vertex_words) = card.layout();
            assert_eq!(plan.words, vertex_words, "seed {seed}");
            let draw = Draw {
                plan: &plan,
                fvf,
                setup: &setup,
                kind,
                count,
                order: order.as_deref(),
            };
            let mut words = vec![0xDEAD_BEEF];
            let made = draw.command(&mut words, &mut Vec::new(), |words| {
                plan.gather(words, size, &bytes);
                true
            });
            assert_eq!(words[0], 0xDEAD_BEEF);
            let mut inside_of = pipeline::tl_inside(&setup);
            let inside = bytes.chunks_exact(size).all(|v| {
                let v = layout.read(v);
                inside_of([v.position[0], v.position[1], v.position[2]])
            });
            match made {
                None => {
                    assert!(!inside, "seed {seed}: a draw of vertices inside was kept");
                    assert_eq!(words.len(), 1, "seed {seed}: words left behind");
                    kept_draws += 1;
                }
                Some(count) => {
                    assert!(inside, "seed {seed}");
                    assert_eq!(count, drawn.triangles, "seed {seed}");
                    assert!(command::framed(&words[1..]), "seed {seed}");
                    assert_eq!(
                        triangles(&words[1..]),
                        triangles(&through),
                        "seed {seed}: layout {:#X}, kind {kind}, {card:?}",
                        layout.fvf
                    );
                    direct_draws += 1;
                }
            }
        }
        // Both ends of the test ran.
        assert!(
            direct_draws > 4000 && kept_draws > 300,
            "{direct_draws}, {kept_draws}"
        );
    }

    #[test]
    fn the_old_transformed_vertex_is_the_cards_word_for_word() {
        let setup = Setup::default();
        let card = Card {
            sets: 1,
            ..Card::default()
        };
        // D3DFVF_TLVERTEX with stage 0 on its set 0.
        let tl = Layout::of(pipeline::FVF_TLVERTEX).unwrap();
        assert!(Plan::new(&tl, &setup, &card).same);
        // Without a specular colour the card's vertex has a word more.
        let plain = Layout::of(0x144).unwrap();
        let plan = Plan::new(&plain, &setup, &card);
        assert!(!plan.same);
        assert_eq!(plan.parts[5], From::Value(0xFF00_0000));
        assert_eq!(plan.parts[6..8], [From::Word(5), From::Word(6)]);
        // A stage on the vertex's second set takes its words.
        let two = Layout::of(0x2C4).unwrap();
        let mut second = Setup::default();
        second.coords[0] = Coord::of(1, 0, &IDENTITY);
        let plan = Plan::new(&two, &second, &card);
        assert!(!plan.same);
        assert_eq!(plan.parts[6..8], [From::Word(8), From::Word(9)]);
    }

    /// The pipeline takes a colour apart and the card's vertex puts it
    /// together again: the direct path copies its word.
    #[test]
    fn a_colour_through_the_pipeline_is_the_programs() {
        for byte in 0..=255u32 {
            let c = byte * 0x0101_0101;
            assert_eq!(pipeline::color(pipeline::rgba(c)), c);
        }
    }

    #[test]
    fn numbers_read_as_bytes_are_the_hosts() {
        let mut words = [0u32; 2];
        bytes_of(&mut words).copy_from_slice(&[1, 2, 3, 4, 0xFF, 0, 0, 0x80]);
        from_le(&mut words);
        assert_eq!(words, [0x0403_0201, 0x8000_00FF]);
        let mut halves = [0u16; 2];
        bytes_of(&mut halves).copy_from_slice(&[1, 2, 0xFE, 0xFF]);
        from_le(&mut halves);
        assert_eq!(halves, [0x0201, 0xFFFE]);
    }
}

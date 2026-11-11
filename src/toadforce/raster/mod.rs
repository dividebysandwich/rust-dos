//! The software renderer: level 1's draws into card memory ("The
//! software renderer" in `docs/toadforce.md`). It is the definition of
//! what the card draws, which the OpenGL renderer is tested against.
//!
//! A draw goes through these, in this order:
//!
//! - `vertex`: the vertex layout a draw names, and a vertex in screen
//!   space.
//! - `geometry`, `light` and `clip`: a vertex the card transforms,
//!   from its program's space to the screen, with its colours and its
//!   texture coordinates, and the primitives cut at the planes.
//! - `pipeline`: the lowering of the card's state to the steps a
//!   fragment goes through, with the numbers and textures of the draw
//!   beside them.
//! - `setup`: the pixels a triangle, a line or a point covers and its
//!   vertices' values at each.
//! - `span`: the steps run over the pixels of a row.
//! - `sample` and `texel`: a texture's colour at a coordinate, read
//!   from card memory, in every format of the level.
//! - `combine`: the arithmetic of the stages, the tests and the blend.
//! - `mem` and `pool`: card memory as several threads use it, and the
//!   threads.
//! - `math`: the logarithm, exponential, power and cosine the above
//!   need, the same on every host.
//!
//! `Renderer` here ties them together. The draws of one run of
//! commands are queued and drawn together: every thread takes the
//! primitives in the order they came and draws the rows it owns (row
//! `r` belongs to share `r % n`), so a pixel sees the primitives in the
//! card's order and the picture is the same for any number of threads.
//! The card empties the queue before any other command touches the
//! memory and at the end of the run, so nothing is queued when the
//! guest runs, a state is saved, or the screen is shown.

pub mod clip;
pub mod combine;
pub mod geometry;
pub mod light;
pub mod math;
pub mod mem;
pub mod pipeline;
pub mod pool;
pub mod sample;
pub mod setup;
pub mod span;
pub mod texel;
pub mod vertex;

use super::format::Rect;
use super::level1::{Draw, Indices, Level1, Target, Vertices};
use super::regs::Error;
use mem::Mem;
use pipeline::{DrawState, Lowered};
use pool::Pool;
use setup::Prim;
use std::ops::Range;
use std::sync::Arc;
use vertex::ScreenVertex;

/// Queued primitives that are drawn with one state.
struct Run {
    state: Arc<DrawState>,
    prims: Range<usize>,
}

/// What a draw command queued.
pub struct Drawn {
    /// The triangles, lines and points.
    pub primitives: u64,
    /// The bytes of card memory they may write: from the first to
    /// after the last.
    pub span: (u32, u32),
}

/// Rows of primitives below this are drawn on the calling thread:
/// waking the others costs more than they save.
const SMALL: i64 = 512;

#[derive(Default)]
pub struct Renderer {
    /// Made at the first batch that is worth it.
    pool: Option<Pool>,
    /// The threads beside the caller's: fixed by a test, or the host's
    /// count, looked up at the first batch.
    workers: Option<usize>,
    runs: Vec<Run>,
    prims: Vec<Prim>,
    /// The target of what is queued.
    target: Option<Target>,
    /// Kept from draw to draw for their room.
    vertices: Vec<ScreenVertex>,
    lit: Vec<geometry::Vertex>,
    /// For each vertex of a draw the card transforms, its place in
    /// `lit`, or `u32::MAX` while no index has named it.
    places: Vec<u32>,
    order: Vec<u32>,
}

/// One primitive of a draw, by its vertices' places in the draw's
/// order.
enum Piece {
    Point(usize),
    Line(usize, usize),
    /// The triangle's vertices, and which of the three are its first
    /// and last in the draw's order, which a strip and a fan turn
    /// round.
    Triangle([usize; 3], (usize, usize)),
}

/// The primitives `n` vertices make of the kind `primitive`.
fn pieces(primitive: u32, n: usize, mut f: impl FnMut(Piece)) {
    match primitive {
        1 => (0..n).for_each(|i| f(Piece::Point(i))),
        2 => (0..n / 2).for_each(|i| f(Piece::Line(2 * i, 2 * i + 1))),
        3 => (1..n).for_each(|i| f(Piece::Line(i - 1, i))),
        4 => (0..n / 3).for_each(|i| f(Piece::Triangle([3 * i, 3 * i + 1, 3 * i + 2], (0, 2)))),
        5 => (2..n).for_each(|i| {
            // Every other triangle of a strip has its first two
            // vertices the other way round, to wind as the rest.
            if i % 2 == 0 {
                f(Piece::Triangle([i - 2, i - 1, i], (0, 2)));
            } else {
                f(Piece::Triangle([i - 1, i - 2, i], (1, 2)));
            }
        }),
        _ => (2..n).for_each(|i| f(Piece::Triangle([i - 1, i, 0], (0, 1)))),
    }
}

/// Queue the convex polygon of `corners`: a triangle, or what the
/// planes left of one. `provoking` gives it its colours where it is
/// flat-shaded. The corners are taken by reference or by value, so a
/// triangle of a draw's own vertices isn't copied.
fn polygon<V: std::borrow::Borrow<ScreenVertex>>(
    prims: &mut Vec<Prim>,
    corners: &[V],
    provoking: &ScreenVertex,
    state: &DrawState,
    sets: &[u8],
) {
    let (shape, clip) = (&state.shape, state.clip);
    let n = corners.len();
    let at = |i: usize| corners[i].borrow();
    let fan = |i: usize| [at(0), at(i), at(i + 1)];
    match shape.fill {
        // Points and lines go by the winding only: their pixels
        // needn't be in the rows the triangle's are.
        1 | 2 if !(1..n - 1).any(|i| Prim::facing(fan(i), shape.cull).is_some()) => {}
        1 => {
            for i in 0..n {
                prims.extend(Prim::point(at(i), shape, clip, sets).into_iter().flatten());
            }
        }
        2 => {
            for i in 0..n {
                let (a, b) = (at(i), at((i + 1) % n));
                prims.extend(Prim::line(a, b, provoking, shape, clip, sets));
            }
        }
        _ => {
            for i in 1..n - 1 {
                prims.extend(Prim::triangle(fan(i), provoking, shape, clip, sets));
            }
        }
    }
}

// A copy of a card draws as the card does, and has nothing queued.
impl Clone for Renderer {
    fn clone(&self) -> Self {
        Renderer {
            workers: self.workers,
            ..Renderer::default()
        }
    }
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Renderer({} queued)", self.prims.len())
    }
}

impl Renderer {
    /// A renderer with `workers` threads beside the caller's, whatever
    /// the host has.
    pub fn with_workers(workers: usize) -> Self {
        Renderer {
            workers: Some(workers),
            ..Renderer::default()
        }
    }

    /// Queue the primitives of `draw`, with the state `l`. Gives how
    /// many there are and the bytes of card memory they may write, or
    /// None when there are none.
    pub fn draw(
        &mut self,
        l: &mut Level1,
        draw: &Draw,
        memory: &mut [u8],
    ) -> Result<Option<Drawn>, Error> {
        // Vertices and indices in card memory are read now, so what the
        // queue writes there is drawn first.
        let buffers = [
            match draw.vertices {
                Vertices::Buffer {
                    offset,
                    stride,
                    count,
                } if count > 0 => {
                    let last = (count as u64 - 1) * stride as u64;
                    let end = offset as u64 + last + 4 * draw.fvf.words as u64;
                    Some((offset as u64, end))
                }
                _ => None,
            },
            match draw.indices {
                Indices::Buffer { offset, count } => {
                    Some((offset as u64, offset as u64 + 2 * count as u64))
                }
                _ => None,
            },
        ];
        if let Some(target) = &self.target
            && buffers
                .iter()
                .flatten()
                .any(|&r| pipeline::writes(target, r))
        {
            self.flush(memory);
        }

        // The order of the vertices, each one of the draw's.
        let count = draw.vertex_count();
        self.order.clear();
        match draw.indices {
            Indices::None => self.order.extend(0..count as u32),
            Indices::Inline { words, count } => self
                .order
                .extend((0..count as usize).map(|i| words[i / 2] >> (16 * (i % 2)) & 0xFFFF)),
            Indices::Buffer { offset, count } => {
                let bytes = &memory[offset as usize..][..2 * count as usize];
                self.order.extend(
                    bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| u16::from_le_bytes(*b) as u32),
                );
            }
        }
        if self.order.iter().any(|&i| i as usize >= count) {
            return Err(Error::BadRange);
        }
        let state = Lowered::of(l)?;

        // A vertex in screen space goes to the rasteriser as it is,
        // and the others through transform and lighting first.
        let geometry =
            (!draw.fvf.transformed).then(|| geometry::Setup::new(l, &state.coords, &draw.fvf));
        // Word `i` of vertex `n`.
        let word = |n: usize, i: usize| match draw.vertices {
            Vertices::Inline(words) => words[n * draw.fvf.words + i],
            Vertices::Buffer { offset, stride, .. } => {
                let at = offset as usize + n * stride as usize + 4 * i;
                u32::from_le_bytes(memory[at..at + 4].try_into().unwrap())
            }
        };
        self.vertices.clear();
        self.lit.clear();
        match &geometry {
            None => self.vertices.extend(
                (0..count).map(|n| ScreenVertex::read(&draw.fvf, |i| word(n, i))),
            ),
            // Only the vertices the indices name go through transform
            // and lighting, each once, however many the draw has: an
            // indexed draw often names a few of a large buffer. The
            // order then counts in `lit`.
            Some(setup) => {
                self.places.clear();
                self.places.resize(count, u32::MAX);
                for i in self.order.iter_mut() {
                    let place = &mut self.places[*i as usize];
                    if *place == u32::MAX {
                        *place = self.lit.len() as u32;
                        let n = *i as usize;
                        self.lit.push(setup.vertex(|w| word(n, w)));
                    }
                    *i = *place;
                }
            }
        }

        // What is queued is for one target, and a draw that reads what
        // it writes is alone in the queue.
        if self.target != Some(state.target) || state.serial {
            self.flush(memory);
        }
        self.target = Some(state.target);
        let first = self.prims.len();
        match &geometry {
            Some(setup) => self.assemble_lit(draw.primitive, &state, setup),
            None => self.assemble(draw.primitive, &state),
        }
        let primitives = (self.prims.len() - first) as u64;
        if primitives == 0 {
            return Ok(None);
        }
        let colour = state.target.colour;
        let serial = state.serial;
        self.runs.push(Run {
            state,
            prims: first..self.prims.len(),
        });
        if serial {
            self.flush(memory);
        }
        let whole = Rect {
            left: 0,
            top: 0,
            width: colour.width,
            height: colour.height,
        };
        Ok(Some(Drawn {
            primitives,
            span: colour.span(&whole),
        }))
    }

    /// The primitives of the vertices in `self.order`, queued.
    fn assemble(&mut self, primitive: u32, state: &DrawState) {
        let Renderer {
            vertices,
            order,
            prims,
            ..
        } = self;
        let (shape, clip, sets) = (&state.shape, state.clip, &state.sets[..]);
        let v = |i: usize| &vertices[order[i] as usize];
        pieces(primitive, order.len(), |piece| match piece {
            Piece::Point(a) => {
                prims.extend(Prim::point(v(a), shape, clip, sets).into_iter().flatten());
            }
            Piece::Line(a, b) => {
                let provoking = if shape.flat_last { v(b) } else { v(a) };
                prims.extend(Prim::line(v(a), v(b), provoking, shape, clip, sets));
            }
            Piece::Triangle(t, ends) => {
                let provoking = v(t[if shape.flat_last { ends.1 } else { ends.0 }]);
                polygon(prims, &t.map(v), provoking, state, sets);
            }
        });
    }

    /// The primitives of the vertices in `self.order`, which went
    /// through `setup`: cut at its planes, put on the screen and
    /// queued.
    fn assemble_lit(&mut self, primitive: u32, state: &DrawState, setup: &geometry::Setup) {
        let Renderer {
            lit, order, prims, ..
        } = self;
        let (shape, clip) = (&state.shape, state.clip);
        // A slot's coordinates are worked out for the slot.
        let slots: [u8; pipeline::SLOTS] = std::array::from_fn(|i| i as u8);
        let sets = &slots[..state.coords.len()];
        let v = |i: usize| &lit[order[i] as usize];
        // What crosses a plane in force is cut there.
        pieces(primitive, order.len(), |piece| match piece {
            Piece::Point(a) => {
                if v(a).out == 0 {
                    let a = setup.project(v(a));
                    prims.extend(Prim::point(&a, shape, clip, sets).into_iter().flatten());
                }
            }
            Piece::Line(a, b) => {
                let (a, b) = (v(a), v(b));
                let out = a.out | b.out;
                if a.out & b.out != 0 {
                    return;
                }
                let provoking = setup.project(if shape.flat_last { b } else { a });
                if let Some((a, b)) = clip::line(*a, *b, out) {
                    let (a, b) = (setup.project(&a), setup.project(&b));
                    prims.extend(Prim::line(&a, &b, &provoking, shape, clip, sets));
                }
            }
            Piece::Triangle(t, ends) => {
                let t = t.map(v);
                let out = t[0].out | t[1].out | t[2].out;
                if t[0].out & t[1].out & t[2].out != 0 {
                    return;
                }
                // The side that shows gives the colours.
                let back = setup.back(t);
                let side = |v: &geometry::Vertex| if back { v.turned() } else { *v };
                let provoking =
                    setup.project(&side(t[if shape.flat_last { ends.1 } else { ends.0 }]));
                // Most triangles cross no plane, and go to the screen
                // without the room a cut one needs.
                if out == 0 {
                    let corners = t.map(|v| setup.project(&side(v)));
                    polygon(prims, &corners, &provoking, state, sets);
                    return;
                }
                let mut cut = clip::Polygon::triangle(t.map(side));
                cut.cut(out);
                let mut corners = [ScreenVertex::default(); clip::CORNERS];
                let n = cut.corners().len();
                if n < 3 {
                    return;
                }
                for (corner, v) in corners.iter_mut().zip(cut.corners()) {
                    *corner = setup.project(v);
                }
                polygon(prims, &corners[..n], &provoking, state, sets);
            }
        });
    }

    /// Draw what is queued.
    pub fn flush(&mut self, memory: &mut [u8]) {
        if self.runs.is_empty() {
            return;
        }
        let (runs, prims) = (&self.runs, &self.prims);
        let mem = Mem::new(memory);
        let work = |share: usize, shares: usize| {
            let mut chunk = span::Chunk::new();
            for run in runs {
                let ctx = span::Ctx::new(mem, &run.state);
                for prim in &prims[run.prims.clone()] {
                    // The first of the primitive's rows this share
                    // owns.
                    let (top, bottom) = prim.rows;
                    let skip = (share as i32 - top).rem_euclid(shares as i32);
                    for y in (top + skip..bottom).step_by(shares) {
                        ctx.row(prim, y, &mut chunk);
                    }
                }
            }
        };
        let rows: i64 = prims.iter().map(|p| (p.rows.1 - p.rows.0) as i64).sum();
        let serial = runs.iter().any(|run| run.state.serial);
        let workers = *self.workers.get_or_insert_with(Pool::default_workers);
        if serial || rows < SMALL || workers == 0 {
            work(0, 1);
        } else {
            let pool = self.pool.get_or_insert_with(|| Pool::new(workers));
            let shares = pool.shares();
            pool.run(&|share| work(share, shares));
        }
        self.runs.clear();
        self.prims.clear();
        self.target = None;
    }
}

#[cfg(test)]
mod tests;

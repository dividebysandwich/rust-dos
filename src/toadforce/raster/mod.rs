//! The software renderer: level 1's draws into card memory ("The
//! software renderer" in `docs/toadforce.md`). It is the definition of
//! what the card draws, which the OpenGL renderer is tested against.
//!
//! A draw goes through these, in this order:
//!
//! - `vertex`: the vertex layout a draw names, and a vertex in screen
//!   space.
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
//! - `math`: the logarithm and exponential the above need, the same on
//!   every host.
//!
//! `Renderer` here ties them together. The draws of one run of
//! commands are queued and drawn together: every thread takes the
//! primitives in the order they came and draws the rows it owns (row
//! `r` belongs to share `r % n`), so a pixel sees the primitives in the
//! card's order and the picture is the same for any number of threads.
//! The card empties the queue before any other command touches the
//! memory and at the end of the run, so nothing is queued when the
//! guest runs, a state is saved, or the screen is shown.

pub mod combine;
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
    /// The threads beside the caller's, where a test fixes them.
    workers: Option<usize>,
    runs: Vec<Run>,
    prims: Vec<Prim>,
    /// The target of what is queued.
    target: Option<Target>,
    /// Kept from draw to draw for their room.
    vertices: Vec<ScreenVertex>,
    order: Vec<u32>,
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

        self.vertices.clear();
        match draw.vertices {
            Vertices::Inline(words) => self.vertices.extend(
                words
                    .chunks_exact(draw.fvf.words)
                    .map(|v| ScreenVertex::read(&draw.fvf, |i| v[i])),
            ),
            Vertices::Buffer { offset, stride, .. } => {
                self.vertices.extend((0..count).map(|n| {
                    let at = offset as usize + n * stride as usize;
                    ScreenVertex::read(&draw.fvf, |i| {
                        let at = at + 4 * i;
                        u32::from_le_bytes(memory[at..at + 4].try_into().unwrap())
                    })
                }));
            }
        }

        // What is queued is for one target, and a draw that reads what
        // it writes is alone in the queue.
        if self.target != Some(state.target) || state.serial {
            self.flush(memory);
        }
        self.target = Some(state.target);
        let first = self.prims.len();
        self.assemble(draw.primitive, &state);
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
        let n = order.len();
        let point = |prims: &mut Vec<Prim>, a: &ScreenVertex| {
            prims.extend(Prim::point(a, shape, clip, sets).into_iter().flatten());
        };
        let line = |prims: &mut Vec<Prim>, a: &ScreenVertex, b: &ScreenVertex| {
            let provoking = if shape.flat_last { b } else { a };
            prims.extend(Prim::line(a, b, provoking, shape, clip, sets));
        };
        // `ends` are the triangle's first and last vertex in the draw's
        // order, which a strip and a fan turn round.
        let triangle = |prims: &mut Vec<Prim>, t: [&ScreenVertex; 3], ends: (usize, usize)| {
            let provoking = if shape.flat_last {
                t[ends.1]
            } else {
                t[ends.0]
            };
            let Some(prim) = Prim::triangle(t, provoking, shape, clip, sets) else {
                return;
            };
            match shape.fill {
                1 => t.into_iter().for_each(|a| point(prims, a)),
                2 => {
                    for i in 0..3 {
                        let (a, b) = (t[i], t[(i + 1) % 3]);
                        prims.extend(Prim::line(a, b, provoking, shape, clip, sets));
                    }
                }
                _ => prims.push(prim),
            }
        };
        match primitive {
            1 => (0..n).for_each(|i| point(prims, v(i))),
            2 => (0..n / 2).for_each(|i| line(prims, v(2 * i), v(2 * i + 1))),
            3 => (1..n).for_each(|i| line(prims, v(i - 1), v(i))),
            4 => (0..n / 3).for_each(|i| {
                triangle(prims, [v(3 * i), v(3 * i + 1), v(3 * i + 2)], (0, 2));
            }),
            5 => (2..n).for_each(|i| {
                // Every other triangle of a strip has its first two
                // vertices the other way round, to wind as the rest.
                if i % 2 == 0 {
                    triangle(prims, [v(i - 2), v(i - 1), v(i)], (0, 2));
                } else {
                    triangle(prims, [v(i - 1), v(i - 2), v(i)], (1, 2));
                }
            }),
            _ => (2..n).for_each(|i| triangle(prims, [v(i - 1), v(i), v(0)], (0, 1))),
        }
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
        let workers = self.workers.unwrap_or_else(Pool::default_workers);
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

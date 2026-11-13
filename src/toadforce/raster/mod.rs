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
//!
//! With a host renderer attached (`super::gpu`), the primitives of a
//! state the host takes are queued for it as the screen vertices of
//! triangles, a point as its two and a line as the squares of its
//! pixels, and the rest is queued as before. The queue holds one kind
//! at a time:
//! a draw of the other kind empties it first, so the host's draws and
//! the software's reach a target in the card's order.

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
use super::gpu::{self, Decline, Gpu};
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
    /// The triangles queued for the host renderer. While there are
    /// any, `runs` is empty.
    host_runs: Vec<gpu::Run>,
    host_vertices: Vec<ScreenVertex>,
    /// The primitives queued for the host so far, counted as the
    /// software renderer's are: a triangle and a line one, a point
    /// its two triangles.
    host_count: u64,
}

/// Where a draw's primitives are queued.
enum Queue<'a> {
    Software(&'a mut Vec<Prim>),
    /// For the host, which fills triangles and nothing else: the
    /// triangles' vertices, as `gpu::Run::vertices` has them, and the
    /// count of the primitives they came from. A point goes there as
    /// its two triangles and a line as the squares of its pixels
    /// (`Prim::squares`), so the host covers the pixels the card's
    /// rules give, and its own rules for lines and points are never
    /// asked.
    Host(&'a mut Vec<ScreenVertex>, &'a mut u64),
}

/// A vertex with each slot's coordinates where the host looks for
/// them.
fn slotted(v: &ScreenVertex, sets: &[u8]) -> ScreenVertex {
    let mut out = *v;
    for (slot, &set) in sets.iter().enumerate() {
        out.tex[slot] = v.tex[set as usize];
    }
    out
}

/// A clockwise triangle of a point or of a line's square, turned
/// round where the state culls clockwise triangles: no triangle the
/// host gets is one the state's `CULLMODE` culls, so a host that
/// applies it again draws them all.
fn kept(t: [ScreenVertex; 3], shape: &pipeline::Shape) -> [ScreenVertex; 3] {
    match shape.cull {
        2 => [t[0], t[2], t[1]],
        _ => t,
    }
}

impl Queue<'_> {
    /// Queue the point at `v`.
    fn point(&mut self, v: &ScreenVertex, state: &DrawState, sets: &[u8]) {
        let shape = &state.shape;
        match self {
            Queue::Software(prims) => {
                prims.extend(
                    Prim::point(v, shape, state.clip, sets)
                        .into_iter()
                        .flatten(),
                );
            }
            Queue::Host(vertices, count) => {
                for t in Prim::point_triangles(v, shape) {
                    // One with no area has no pixels.
                    if Prim::facing([&t[0], &t[1], &t[2]], 1).is_none() {
                        continue;
                    }
                    let t = kept(t, shape);
                    vertices.extend(t.iter().map(|c| slotted(c, sets)));
                    if shape.flat {
                        vertices.push(slotted(v, sets));
                    }
                    **count += 1;
                }
            }
        }
    }

    /// Queue the line from `a` to `b`.
    fn line(
        &mut self,
        a: &ScreenVertex,
        b: &ScreenVertex,
        provoking: &ScreenVertex,
        state: &DrawState,
        sets: &[u8],
    ) {
        let shape = &state.shape;
        let Some(line) = Prim::line(a, b, provoking, shape, state.clip, sets) else {
            return;
        };
        match self {
            Queue::Software(prims) => prims.push(line),
            Queue::Host(vertices, count) => {
                line.squares(shape, |c| {
                    for t in [[0, 1, 2], [1, 3, 2]] {
                        vertices.extend(kept(t.map(|i| c[i]), shape));
                        // The corners have the pixels' colours.
                        if shape.flat {
                            vertices.push(c[0]);
                        }
                    }
                });
                **count += 1;
            }
        }
    }
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
/// triangle of a draw's own vertices isn't copied. `outline` has a
/// bit for each corner that is the triangle's own and one for each
/// edge, from a corner to the next, that is on one of the triangle's:
/// those are the points and the lines of a triangle that isn't filled
/// (`clip::Polygon::outline`).
fn polygon<V: std::borrow::Borrow<ScreenVertex>>(
    queue: &mut Queue,
    corners: &[V],
    outline: (u32, u32),
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
            for i in (0..n).filter(|i| outline.0 >> i & 1 != 0) {
                queue.point(at(i), state, sets);
            }
        }
        2 => {
            for i in (0..n).filter(|i| outline.1 >> i & 1 != 0) {
                queue.line(at(i), at((i + 1) % n), provoking, state, sets);
            }
        }
        _ => match queue {
            Queue::Software(prims) => {
                for i in 1..n - 1 {
                    prims.extend(Prim::triangle(fan(i), provoking, shape, clip, sets));
                }
            }
            // The triangles the card doesn't cull.
            Queue::Host(vertices, count) => {
                for i in 1..n - 1 {
                    if Prim::facing(fan(i), shape.cull).is_some() {
                        vertices.extend(fan(i).map(|v| slotted(v, sets)));
                        if shape.flat {
                            vertices.push(slotted(provoking, sets));
                        }
                        **count += 1;
                    }
                }
            }
        },
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

    /// Draw with `workers` threads beside the caller's from now on.
    #[cfg(test)]
    pub(crate) fn fix_workers(&mut self, workers: usize) {
        self.workers = Some(workers);
        self.pool = None;
    }

    /// Queue the primitives of `draw`, with the state `l`: for the
    /// host of `gpu` where it draws them, and for the software
    /// renderer otherwise. Gives how many there are and the bytes of
    /// card memory they may write, or None when there are none.
    pub fn draw(
        &mut self,
        l: &mut Level1,
        draw: &Draw,
        memory: &mut [u8],
        gpu: &mut Gpu,
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
            self.flush(memory, gpu);
        }
        // And what the host drew there is read back.
        if gpu.ahead() {
            for (first, end) in buffers.into_iter().flatten() {
                let range = (first as u32, end.min(u32::MAX as u64) as u32);
                gpu.settle(memory, Some(range), gpu::Cause::Buffers);
            }
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

        // The host draws the primitives of the states it takes. A
        // draw that reads what it writes stays with the software
        // renderer, which draws it pixel by pixel in order. `declined`
        // is why a host doesn't draw it, and None with no host.
        let attached = gpu.attached();
        let declined = if !attached {
            None
        } else if state.serial {
            Some(Decline::Serial)
        } else {
            gpu.declines(&state)
        };
        let hosted = attached && declined.is_none();
        // What is queued is for one target and one renderer, and a
        // draw that reads what it writes is alone in the queue.
        let other = match hosted {
            true => !self.runs.is_empty(),
            false => !self.host_runs.is_empty(),
        };
        if self.target != Some(state.target) || state.serial || other {
            self.flush(memory, gpu);
        }
        if let Some(why) = declined {
            gpu.decline(why);
            // The software renderer draws over the pixels the host
            // left, and samples those it drew.
            if gpu.ahead() {
                let colour = state.target.colour;
                gpu.settle(memory, Some(gpu::range(&colour)), gpu::Cause::Declined);
                if let Some(depth) = state.target.depth_surface() {
                    gpu.settle(memory, Some(gpu::range(&depth)), gpu::Cause::Declined);
                }
                let mut reads = Vec::new();
                pipeline::reads(&state, &mut reads);
                for read in reads {
                    gpu.settle(memory, Some(read), gpu::Cause::Declined);
                }
            }
        }
        self.target = Some(state.target);
        let first = match hosted {
            true => self.host_vertices.len(),
            false => self.prims.len(),
        };
        let counted = self.host_count;
        match &geometry {
            Some(setup) => self.assemble_lit(draw.primitive, &state, setup, hosted),
            None => self.assemble(draw.primitive, &state, hosted),
        }
        let colour = state.target.colour;
        if hosted {
            let vertices = first..self.host_vertices.len();
            let run = gpu::Run { state, vertices };
            if run.vertices.is_empty() {
                return Ok(None);
            }
            let primitives = self.host_count - counted;
            // A draw with the state of the one before it is one run
            // with it: the host sets its state up once for both.
            match self.host_runs.last_mut() {
                Some(last) if Arc::ptr_eq(&last.state, &run.state) || *last.state == *run.state => {
                    last.vertices.end = run.vertices.end;
                }
                _ => self.host_runs.push(run),
            }
            return Ok(Some(Drawn {
                primitives,
                span: gpu::range(&colour),
            }));
        }
        let primitives = (self.prims.len() - first) as u64;
        if primitives == 0 {
            return Ok(None);
        }
        let serial = state.serial;
        self.runs.push(Run {
            state,
            prims: first..self.prims.len(),
        });
        if serial {
            self.flush(memory, gpu);
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

    /// The primitives of the vertices in `self.order`, queued: for
    /// the host if `hosted`.
    fn assemble(&mut self, primitive: u32, state: &DrawState, hosted: bool) {
        let Renderer {
            vertices,
            order,
            prims,
            host_vertices,
            host_count,
            ..
        } = self;
        let mut queue = match hosted {
            true => Queue::Host(host_vertices, host_count),
            false => Queue::Software(prims),
        };
        let (shape, sets) = (&state.shape, &state.sets[..]);
        let v = |i: usize| &vertices[order[i] as usize];
        pieces(primitive, order.len(), |piece| match piece {
            Piece::Point(a) => queue.point(v(a), state, sets),
            Piece::Line(a, b) => {
                let provoking = if shape.flat_last { v(b) } else { v(a) };
                queue.line(v(a), v(b), provoking, state, sets);
            }
            Piece::Triangle(t, ends) => {
                let provoking = v(t[if shape.flat_last { ends.1 } else { ends.0 }]);
                polygon(&mut queue, &t.map(v), (7, 7), provoking, state, sets);
            }
        });
    }

    /// The primitives of the vertices in `self.order`, which went
    /// through `setup`: cut at its planes, put on the screen and
    /// queued, for the host if `hosted`.
    fn assemble_lit(
        &mut self,
        primitive: u32,
        state: &DrawState,
        setup: &geometry::Setup,
        hosted: bool,
    ) {
        let Renderer {
            lit,
            order,
            prims,
            host_vertices,
            host_count,
            ..
        } = self;
        let mut queue = match hosted {
            true => Queue::Host(host_vertices, host_count),
            false => Queue::Software(prims),
        };
        let shape = &state.shape;
        // A slot's coordinates are worked out for the slot.
        let slots: [u8; pipeline::SLOTS] = std::array::from_fn(|i| i as u8);
        let sets = &slots[..state.coords.len()];
        let v = |i: usize| &lit[order[i] as usize];
        // What crosses a plane in force is cut there.
        pieces(primitive, order.len(), |piece| match piece {
            Piece::Point(a) => {
                if v(a).out == 0 {
                    queue.point(&setup.project(v(a)), state, sets);
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
                    queue.line(&a, &b, &provoking, state, sets);
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
                    polygon(&mut queue, &corners, (7, 7), &provoking, state, sets);
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
                polygon(&mut queue, &corners[..n], cut.outline(), &provoking, state, sets);
            }
        });
    }

    /// Draw what is queued, or have the host of `gpu` draw it.
    pub fn flush(&mut self, memory: &mut [u8], gpu: &mut Gpu) {
        if !self.host_runs.is_empty() {
            // The textures the host is about to copy from card memory.
            let mut reads = Vec::new();
            if gpu.ahead() {
                for run in &self.host_runs {
                    pipeline::reads(&run.state, &mut reads);
                }
            }
            let batch = gpu::Batch {
                target: self.target.expect("what is queued has a target"),
                runs: &self.host_runs,
                vertices: &self.host_vertices,
            };
            gpu.draw(&batch, &reads, memory);
            self.host_runs.clear();
            self.host_vertices.clear();
            self.target = None;
            return;
        }
        if self.runs.is_empty() {
            self.target = None;
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
        // The host's copies of what was drawn into are stale.
        if let Some(target) = self.target.take() {
            let (first, end) = gpu::range(&target.colour);
            gpu.pages.wrote(first, end - first);
            if let Some(depth) = target.depth_surface() {
                let (first, end) = gpu::range(&depth);
                gpu.pages.wrote(first, end - first);
            }
        }
    }
}

#[cfg(test)]
mod tests;

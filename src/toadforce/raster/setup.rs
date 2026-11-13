//! Set-up: which pixels a triangle, a line or a point covers, and what
//! its vertices' values are at each ("Rasterising" in
//! `docs/toadforce.md`).
//!
//! Positions are rounded to 1/256 of a pixel and the cover is worked
//! out in whole numbers from there, so a pixel on the edge two
//! triangles share belongs to exactly one of them. Each row's pixels
//! come straight from the edges' equations and each pixel's values from
//! its own coordinates, never from the pixel before: the rows can be
//! drawn in any order, by any number of threads.

use super::pipeline::{SLOTS, Shape};
use super::texel::Rgba;
use super::vertex::ScreenVertex;

/// Positions are limited to this many pixels either side of 0, which
/// keeps the edge equations inside 64 bits.
pub const LIMIT: f32 = 32768.0;

/// A value that changes linearly over the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Plane {
    /// The value at the primitive's origin.
    pub at: f32,
    /// The change for a pixel to the right, and for a pixel down.
    pub dx: f32,
    pub dy: f32,
}

impl Plane {
    /// The value `fx` pixels right of the origin and `fy` below it.
    #[inline]
    pub fn eval(&self, fx: f32, fy: f32) -> f32 {
        self.at + self.dx * fx + self.dy * fy
    }

    pub fn flat(&self) -> bool {
        self.dx == 0.0 && self.dy == 0.0
    }
}

/// A colour's four channels from their planes over w.
#[inline]
pub fn colour(planes: &[Plane; 4], fx: f32, fy: f32, w: f32) -> Rgba {
    // A number that is none casts to 0.
    planes.map(|p| (p.eval(fx, fy) * w + 0.5).clamp(0.0, 255.0) as u8)
}

/// An edge from `a` to the next vertex, in 1/256 of a pixel. A point
/// is on the inner side where `dx * (y - ay) - dy * (x - ax)` is at
/// least `bias`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Edge {
    ax: i64,
    ay: i64,
    dx: i64,
    dy: i64,
    /// 0 for a top or a left edge, whose pixels belong to the triangle,
    /// and 1 for the others.
    bias: i64,
}

/// The pixels of a primitive.
#[derive(Clone, Debug, PartialEq)]
enum Cover {
    /// Those inside a triangle's three edges, between the columns the
    /// clip rectangle leaves (the last outside).
    Edges {
        edges: [Edge; 3],
        centre: i64,
        columns: (i32, i32),
    },
    /// A line's: for each row from the primitive's first, the first
    /// pixel and the one after the last.
    Rows(Box<[(i32, i32)]>),
}

/// A triangle, a line or a point ready to be drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Prim {
    cover: Cover,
    /// The rows the primitive may have pixels in, the last outside.
    pub rows: (i32, i32),
    /// The origin of the planes, as the pixel whose centre it is.
    pub ox: f32,
    pub oy: f32,
    /// Depth.
    pub z: Plane,
    /// 1 / w.
    pub q: Plane,
    /// The colours' channels times 1 / w.
    pub diffuse: [Plane; 4],
    pub specular: [Plane; 4],
    /// The colours, where the whole primitive has one of each.
    pub flat: Option<(Rgba, Rgba)>,
    /// Each slot's coordinates (s, t, r, q) times 1 / w.
    pub tex: [[Plane; 4]; SLOTS],
}

/// A position in 1/256 of a pixel, or None for one that is no number.
fn snap(v: f32) -> Option<i64> {
    v.is_finite()
        .then(|| (v.clamp(-LIMIT, LIMIT) as f64 * 256.0).round() as i64)
}

/// The planes of a primitive: `plane` makes one from a value at each of
/// its vertices.
#[allow(clippy::too_many_arguments)]
fn planes<const V: usize>(
    cover: Cover,
    rows: (i32, i32),
    origin: (f64, f64),
    v: [&ScreenVertex; V],
    provoking: &ScreenVertex,
    shape: &Shape,
    sets: &[u8],
    plane: &dyn Fn([f64; V]) -> Plane,
) -> Prim {
    let q = v.map(|v| v.rhw as f64);
    let each = |f: &dyn Fn(&ScreenVertex) -> f64| std::array::from_fn::<f64, V, _>(|i| f(v[i]));
    let over_w =
        |f: &dyn Fn(&ScreenVertex) -> f32| plane(std::array::from_fn(|i| f(v[i]) as f64 * q[i]));
    let same = |f: &dyn Fn(&ScreenVertex) -> Rgba| v.iter().all(|x| f(x) == f(v[0]));
    let flat = if shape.flat {
        Some((provoking.diffuse, provoking.specular))
    } else if same(&|v| v.diffuse) && same(&|v| v.specular) {
        Some((v[0].diffuse, v[0].specular))
    } else {
        None
    };
    let mut tex = [[Plane::default(); 4]; SLOTS];
    for (slot, &set) in tex.iter_mut().zip(sets) {
        for (i, plane) in slot.iter_mut().enumerate() {
            *plane = over_w(&|v| v.tex[set as usize][i]);
        }
    }
    let half = if shape.half_centre { 0.5 } else { 0.0 };
    Prim {
        cover,
        rows,
        ox: (origin.0 - half) as f32,
        oy: (origin.1 - half) as f32,
        z: plane(each(&|v| v.z as f64)),
        q: plane(q),
        diffuse: [0, 1, 2, 3].map(|ch| over_w(&|v| v.diffuse[ch] as f32)),
        specular: [0, 1, 2, 3].map(|ch| over_w(&|v| v.specular[ch] as f32)),
        flat,
        tex,
    }
}

impl Prim {
    /// The positions of the triangle of `v` in 1/256 of a pixel and
    /// twice its area, positive when it is clockwise on the screen.
    /// None for a triangle with a position that is no number, with no
    /// area, or that `cull` (`CULLMODE`) culls.
    pub fn facing(v: [&ScreenVertex; 3], cull: u32) -> Option<([(i64, i64); 3], i64)> {
        let mut p = [(0i64, 0i64); 3];
        for (p, v) in p.iter_mut().zip(v) {
            *p = (snap(v.x)?, snap(v.y)?);
        }
        let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[1].1 - p[0].1) * (p[2].0 - p[0].0);
        // With y down, a positive area is a clockwise triangle.
        let culled = match cull {
            2 => area > 0,
            3 => area < 0,
            _ => false,
        };
        (area != 0 && !culled).then_some((p, area))
    }

    /// The triangle of `v`, drawn as `shape` says inside `clip`, with
    /// the coordinate sets `sets` in its slots. `provoking` gives a
    /// flat-shaded triangle its colours. None for a triangle with no
    /// pixels: one with a position that is no number, with no area,
    /// culled, or outside `clip`.
    pub fn triangle(
        v: [&ScreenVertex; 3],
        provoking: &ScreenVertex,
        shape: &Shape,
        clip: [i32; 4],
        sets: &[u8],
    ) -> Option<Self> {
        let (p, area) = Self::facing(v, shape.cull)?;
        // The edges go clockwise.
        let order = if area > 0 { [0, 1, 2] } else { [0, 2, 1] };
        let edges = [0, 1, 2].map(|i| {
            let (a, b) = (p[order[i]], p[order[(i + 1) % 3]]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let top_left = dy < 0 || (dy == 0 && dx > 0);
            Edge {
                ax: a.0,
                ay: a.1,
                dx,
                dy,
                bias: if top_left { 0 } else { 1 },
            }
        });
        let centre = if shape.half_centre { 128 } else { 0 };
        let row = |y: i64| (y - centre).div_euclid(256) as i32;
        let top = row(p.iter().map(|p| p.1).min().unwrap()).max(clip[1]);
        let bottom = (row(p.iter().map(|p| p.1).max().unwrap()) + 1).min(clip[3]);
        if top >= bottom || clip[0] >= clip[2] {
            return None;
        }

        // The planes, from the rounded positions.
        let pos = p.map(|p| (p.0 as f64 / 256.0, p.1 as f64 / 256.0));
        let (x1, y1) = (pos[1].0 - pos[0].0, pos[1].1 - pos[0].1);
        let (x2, y2) = (pos[2].0 - pos[0].0, pos[2].1 - pos[0].1);
        let det = x1 * y2 - x2 * y1;
        let plane = |a: [f64; 3]| {
            let (d1, d2) = (a[1] - a[0], a[2] - a[0]);
            Plane {
                at: a[0] as f32,
                dx: ((d1 * y2 - d2 * y1) / det) as f32,
                dy: ((d2 * x1 - d1 * x2) / det) as f32,
            }
        };
        let cover = Cover::Edges {
            edges,
            centre,
            columns: (clip[0], clip[2]),
        };
        Some(planes(
            cover,
            (top, bottom),
            pos[0],
            v,
            provoking,
            shape,
            sets,
            &plane,
        ))
    }

    /// The line from `a` to `b`. Where it is longer across than down it
    /// has one pixel in each column whose centre is between the ends,
    /// the pixel whose centre is nearest the line (of two as near, the
    /// one further down); otherwise the same with rows. The column or
    /// row of `a`'s end counts when its centre is at the end, and that
    /// of `b`'s end only with `shape.last_pixel`. None for a line with
    /// no pixels.
    pub fn line(
        a: &ScreenVertex,
        b: &ScreenVertex,
        provoking: &ScreenVertex,
        shape: &Shape,
        clip: [i32; 4],
        sets: &[u8],
    ) -> Option<Self> {
        let p = [(snap(a.x)?, snap(a.y)?), (snap(b.x)?, snap(b.y)?)];
        let centre: i64 = if shape.half_centre { 128 } else { 0 };
        let (dx, dy) = (p[1].0 - p[0].0, p[1].1 - p[0].1);
        if clip[0] >= clip[2] || clip[1] >= clip[3] {
            return None;
        }
        // Along the longer axis `m`, with `n` the other: the pixels
        // whose centres `m * 256 + centre` are from a's end to b's.
        let across = dx.abs() >= dy.abs();
        let (m0, n0, dm, dn) = if across {
            (p[0].0, p[0].1, dx, dy)
        } else {
            (p[0].1, p[0].0, dy, dx)
        };
        let (m_clip, n_clip) = if across {
            ((clip[0], clip[2]), (clip[1], clip[3]))
        } else {
            ((clip[1], clip[3]), (clip[0], clip[2]))
        };
        // The first and last pixel along `m`, in the order of the axis.
        let ceil = |v: i64| -(-v).div_euclid(256);
        let floor = |v: i64| v.div_euclid(256);
        let end = m0 + dm;
        let (first, last) = if dm == 0 {
            // A line of no length is its one pixel, with the last
            // pixel drawn.
            if !shape.last_pixel {
                return None;
            }
            let at = floor(m0 - centre + 128);
            (at, at)
        } else if dm > 0 {
            let last = if shape.last_pixel {
                floor(end - centre)
            } else {
                ceil(end - centre) - 1
            };
            (ceil(m0 - centre), last)
        } else {
            let first = if shape.last_pixel {
                ceil(end - centre)
            } else {
                floor(end - centre) + 1
            };
            (first, floor(m0 - centre))
        };
        let first = first.max(m_clip.0 as i64);
        let last = last.min(m_clip.1 as i64 - 1);
        if first > last {
            return None;
        }
        // The pixel along `n` for each along `m`: the one whose centre
        // is nearest the line there, and of two as near the one further
        // down or right. The line's place is a fraction, so this is
        // floor((n - centre + 128) / 256) in whole numbers.
        let other = |m: i64| {
            if dm == 0 {
                return (n0 - centre + 128).div_euclid(256);
            }
            let num = (n0 - centre + 128) * dm + dn * (m * 256 + centre - m0);
            let den = 256 * dm;
            if den < 0 {
                (-num).div_euclid(-den)
            } else {
                num.div_euclid(den)
            }
        };
        let pixels: Vec<(i32, i32)> = (first..=last)
            .filter_map(|m| {
                let n = other(m);
                let inside = (n_clip.0 as i64..n_clip.1 as i64).contains(&n);
                let (m, n) = (m as i32, n as i32);
                inside.then_some(if across { (m, n) } else { (n, m) })
            })
            .collect();
        let top = pixels.iter().map(|p| p.1).min()?;
        let bottom = pixels.iter().map(|p| p.1).max()? + 1;
        let mut spans = vec![(0, 0); (bottom - top) as usize];
        for (x, y) in pixels {
            let span = &mut spans[(y - top) as usize];
            *span = if span.0 == span.1 {
                (x, x + 1)
            } else {
                (span.0.min(x), span.1.max(x + 1))
            };
        }
        // Values change along the line and not across it.
        let pos = p.map(|p| (p.0 as f64 / 256.0, p.1 as f64 / 256.0));
        let (lx, ly) = (pos[1].0 - pos[0].0, pos[1].1 - pos[0].1);
        let len2 = lx * lx + ly * ly;
        let plane = |v: [f64; 2]| {
            let d = if len2 == 0.0 {
                0.0
            } else {
                (v[1] - v[0]) / len2
            };
            Plane {
                at: v[0] as f32,
                dx: (d * lx) as f32,
                dy: (d * ly) as f32,
            }
        };
        let rows = (top, top + spans.len() as i32);
        Some(planes(
            Cover::Rows(spans.into_boxed_slice()),
            rows,
            pos[0],
            [a, b],
            provoking,
            shape,
            sets,
            &plane,
        ))
    }

    /// The two triangles of the point at `v`: a square of `size` pixels
    /// a side around it, with the vertex's values all over, or with
    /// `shape.point_sprite` every coordinate set going from (0, 0) at
    /// its top left to (1, 1) at its bottom right.
    pub fn point(
        v: &ScreenVertex,
        shape: &Shape,
        clip: [i32; 4],
        sets: &[u8],
    ) -> [Option<Self>; 2] {
        let t = Self::point_triangles(v, shape);
        // A point has no back.
        let shape = Shape { cull: 1, ..*shape };
        t.map(|t| Prim::triangle([&t[0], &t[1], &t[2]], v, &shape, clip, sets))
    }

    /// The corners of the two triangles of the point at `v`
    /// (`Prim::point`), which are drawn whichever way they face.
    pub fn point_triangles(v: &ScreenVertex, shape: &Shape) -> [[ScreenVertex; 3]; 2] {
        let size = v
            .psize
            .unwrap_or(shape.point_size)
            .clamp(shape.point_min, shape.point_max.max(shape.point_min));
        let half = size / 2.0;
        let corner = |right: bool, down: bool| {
            let mut c = *v;
            c.x += if right { half } else { -half };
            c.y += if down { half } else { -half };
            if shape.point_sprite {
                let st = [right as u8 as f32, down as u8 as f32, 0.0, 1.0];
                c.tex = [st; super::vertex::SETS];
            }
            c
        };
        let c = [
            corner(false, false),
            corner(true, false),
            corner(false, true),
            corner(true, true),
        ];
        [[c[0], c[1], c[2]], [c[1], c[3], c[2]]]
    }

    /// A line's pixels as squares for a renderer that fills triangles
    /// (`gpu::Host`): each is the four corners of a run of the line's
    /// pixels in one row (top left, top right, bottom left, bottom
    /// right), and covers those pixels' centres and no other's.
    /// `tex[n]` of a corner is slot `n`'s coordinates.
    ///
    /// The corners' depth, 1 / w and coordinates are the line's planes
    /// at the corners, so what a renderer interpolates from them with
    /// perspective is the planes at each pixel's centre, and their
    /// change from a pixel to the next is the planes' too. The colours
    /// are the pixel's own, rounded as `span.rs` rounds them: a line
    /// whose colours change has a square for each pixel. A pixel whose
    /// corners the planes give no positive 1 / w has its centre's
    /// values at every corner.
    pub fn squares(&self, shape: &Shape, mut out: impl FnMut(&[ScreenVertex; 4])) {
        let Cover::Rows(spans) = &self.cover else {
            return;
        };
        // A pixel's left and top side from its number.
        let side = if shape.half_centre { 0.0 } else { -0.5 };
        let at = |fx: f32, fy: f32, q: f32, colours: (Rgba, Rgba)| {
            let mut v = ScreenVertex {
                z: self.z.eval(fx, fy),
                rhw: q,
                diffuse: colours.0,
                specular: colours.1,
                ..ScreenVertex::default()
            };
            for (to, planes) in v.tex.iter_mut().zip(&self.tex) {
                *to = planes.map(|p| p.eval(fx, fy) / q);
            }
            v
        };
        // The square of the pixels `x` of row `y`, unless it is of
        // several and the planes aren't sound at its corners.
        let mut square = |x: (i32, i32), y: i32, colours: (Rgba, Rgba)| {
            let (fx, fy) = (x.0 as f32 - self.ox, y as f32 - self.oy);
            let wide = (x.1 - x.0) as f32;
            // The corners, from the first pixel's centre.
            let offsets = [
                (-0.5, -0.5),
                (wide - 0.5, -0.5),
                (-0.5, 0.5),
                (wide - 0.5, 0.5),
            ];
            let q = offsets.map(|(dx, dy)| self.q.eval(fx + dx, fy + dy));
            let sound = q.iter().all(|q| q.is_finite() && *q > 0.0);
            if !sound && wide > 1.0 {
                return false;
            }
            let centre = self.q.eval(fx, fy);
            let mut corners = [ScreenVertex::default(); 4];
            for (i, (dx, dy)) in offsets.into_iter().enumerate() {
                corners[i] = match sound {
                    true => at(fx + dx, fy + dy, q[i], colours),
                    false => at(fx, fy, centre, colours),
                };
                corners[i].x = x.0 as f32 + side + dx + 0.5;
                corners[i].y = y as f32 + side + dy + 0.5;
            }
            out(&corners);
            true
        };
        for (row, &(first, end)) in spans.iter().enumerate() {
            let y = self.rows.0 + row as i32;
            // One square for the row where the colours are the same
            // all along the line.
            if let Some(flat) = self.flat
                && square((first, end), y, flat)
            {
                continue;
            }
            for x in first..end {
                // The pixel's place as `span.rs` works it out.
                let fx = first as f32 - self.ox + (x - first) as f32;
                let fy = y as f32 - self.oy;
                let w = 1.0 / self.q.eval(fx, fy);
                let colours = self.flat.unwrap_or_else(|| {
                    (
                        colour(&self.diffuse, fx, fy, w),
                        colour(&self.specular, fx, fy, w),
                    )
                });
                square((x, x + 1), y, colours);
            }
        }
    }

    /// The pixels of row `y` the primitive covers: the first, and the
    /// one after the last.
    pub fn span(&self, y: i32) -> Option<(i32, i32)> {
        let (edges, centre, columns) = match &self.cover {
            Cover::Edges {
                edges,
                centre,
                columns,
            } => (edges, *centre, *columns),
            Cover::Rows(spans) => {
                let span = *spans.get(y.checked_sub(self.rows.0)? as usize)?;
                return (span.0 < span.1).then_some(span);
            }
        };
        let py = y as i64 * 256 + centre;
        let (mut first, mut last) = (columns.0 as i64, columns.1 as i64 - 1);
        for e in edges {
            // The edge's value at column x is k - 256 * dy * x.
            let k = e.dx * (py - e.ay) + e.dy * (e.ax - centre) - e.bias;
            if e.dy > 0 {
                last = last.min(k.div_euclid(256 * e.dy));
            } else if e.dy < 0 {
                first = first.max(-k.div_euclid(-256 * e.dy));
            } else if k < 0 {
                return None;
            }
        }
        (first <= last).then_some((first as i32, last as i32 + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPE: Shape = Shape {
        half_centre: false,
        cull: 1,
        fill: 3,
        flat: false,
        flat_last: false,
        last_pixel: true,
        point_size: 1.0,
        point_min: 1.0,
        point_max: 64.0,
        point_sprite: false,
    };
    const CLIP: [i32; 4] = [0, 0, 64, 64];

    fn vertex(x: f32, y: f32) -> ScreenVertex {
        ScreenVertex {
            x,
            y,
            ..ScreenVertex::default()
        }
    }

    fn triangle(p: [(f32, f32); 3], shape: &Shape) -> Option<Prim> {
        let v = p.map(|(x, y)| vertex(x, y));
        Prim::triangle([&v[0], &v[1], &v[2]], &v[0], shape, CLIP, &[])
    }

    /// The pixels a triangle covers, as (x, y).
    fn pixels(t: &Prim) -> Vec<(i32, i32)> {
        (t.rows.0..t.rows.1)
            .flat_map(|y| {
                let (a, b) = t.span(y).unwrap_or((0, 0));
                (a..b).map(move |x| (x, y))
            })
            .collect()
    }

    #[test]
    fn two_triangles_of_a_square_cover_each_of_its_pixels_once() {
        // Pixel centres at whole coordinates: the square from 0 to 10
        // has the centres 0 to 9, its top and left edges included.
        let a = triangle([(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)], &SHAPE).unwrap();
        let b = triangle([(10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], &SHAPE).unwrap();
        let (a, b) = (pixels(&a), pixels(&b));
        assert_eq!((a.len(), b.len()), (55, 45));
        let mut all: Vec<_> = a.iter().chain(&b).copied().collect();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 100);
        assert!(
            all.iter()
                .all(|&(x, y)| (0..10).contains(&x) && (0..10).contains(&y))
        );
        // The other diagonal, and the other winding.
        let c = triangle([(0.0, 0.0), (10.0, 10.0), (10.0, 0.0)], &SHAPE).unwrap();
        let d = triangle([(0.0, 0.0), (0.0, 10.0), (10.0, 10.0)], &SHAPE).unwrap();
        let mut all: Vec<_> = pixels(&c).into_iter().chain(pixels(&d)).collect();
        assert_eq!(all.len(), 100);
        all.sort();
        all.dedup();
        assert_eq!(all.len(), 100);
    }

    #[test]
    fn the_pixel_centre_moves_the_cover_by_half_a_pixel() {
        // A square from 0.5 to 2.5: centres 1 and 2 at whole
        // coordinates, centres 0.5 and 1.5 (pixels 0 and 1) at half.
        let square = |shape: &Shape| {
            let a = triangle([(0.5, 0.5), (2.5, 0.5), (0.5, 2.5)], shape).unwrap();
            let b = triangle([(2.5, 0.5), (2.5, 2.5), (0.5, 2.5)], shape).unwrap();
            let mut all: Vec<_> = pixels(&a).into_iter().chain(pixels(&b)).collect();
            all.sort();
            all
        };
        assert_eq!(square(&SHAPE), [(1, 1), (1, 2), (2, 1), (2, 2)]);
        let half = Shape {
            half_centre: true,
            ..SHAPE
        };
        assert_eq!(square(&half), [(0, 0), (0, 1), (1, 0), (1, 1)]);
    }

    #[test]
    fn culling_goes_by_the_winding_on_the_screen() {
        let clockwise = [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)];
        let counter = [(0.0, 0.0), (0.0, 10.0), (10.0, 0.0)];
        let cull = |cull| Shape { cull, ..SHAPE };
        assert!(triangle(clockwise, &cull(2)).is_none());
        assert!(triangle(counter, &cull(2)).is_some());
        assert!(triangle(clockwise, &cull(3)).is_some());
        assert!(triangle(counter, &cull(3)).is_none());
        // No area, no number, and outside the clip rectangle.
        assert!(triangle([(0.0, 0.0), (5.0, 5.0), (10.0, 10.0)], &SHAPE).is_none());
        assert!(triangle([(0.0, f32::NAN), (5.0, 0.0), (0.0, 5.0)], &SHAPE).is_none());
        assert!(triangle([(70.0, 70.0), (80.0, 70.0), (70.0, 80.0)], &SHAPE).is_none());
    }

    #[test]
    fn the_clip_rectangle_and_huge_positions_bound_the_rows() {
        let t = triangle([(-100.0, -100.0), (1e9, -100.0), (-100.0, 1e9)], &SHAPE).unwrap();
        assert_eq!(t.rows, (0, 64));
        assert_eq!(t.span(0), Some((0, 64)));
        assert_eq!(t.span(63), Some((0, 64)));
        let p = pixels(&triangle([(60.0, 60.0), (70.0, 60.0), (60.0, 70.0)], &SHAPE).unwrap());
        assert_eq!(p.len(), 16);
    }

    #[test]
    fn values_are_the_vertices_at_the_vertices_and_linear_between() {
        let mut v = [vertex(2.0, 2.0), vertex(10.0, 2.0), vertex(2.0, 18.0)];
        v[0].z = 0.0;
        v[1].z = 0.5;
        v[2].z = 1.0;
        v[1].diffuse = [0, 0, 0, 255];
        v[1].rhw = 0.5;
        v[0].tex[3] = [1.0, 2.0, 0.0, 1.0];
        v[1].tex[3] = [3.0, 2.0, 0.0, 1.0];
        v[2].tex[3] = [1.0, 6.0, 0.0, 1.0];
        let t = Prim::triangle([&v[0], &v[1], &v[2]], &v[0], &SHAPE, CLIP, &[3]).unwrap();
        assert_eq!((t.ox, t.oy), (2.0, 2.0));
        assert_eq!(t.z.eval(0.0, 0.0), 0.0);
        assert_eq!(t.z.eval(8.0, 0.0), 0.5);
        assert_eq!(t.z.eval(0.0, 16.0), 1.0);
        assert_eq!(t.z.eval(4.0, 8.0), 0.75);
        // 1 / w, and a coordinate over w.
        assert_eq!(t.q.eval(8.0, 0.0), 0.5);
        assert_eq!(t.tex[0][0].eval(8.0, 0.0), 1.5);
        assert_eq!(t.tex[0][1].eval(0.0, 16.0), 6.0);
        // Half way along the top edge on the screen is a third of the
        // way in the texture: (1 + 1.5) / 2 over (1 + 0.5) / 2.
        let s = t.tex[0][0].eval(4.0, 0.0) / t.q.eval(4.0, 0.0);
        assert!((s - 5.0 / 3.0).abs() < 1e-6);
        assert_eq!(t.flat, None);
        assert_eq!(t.diffuse[0].eval(0.0, 0.0), 255.0);
        assert_eq!(t.diffuse[0].eval(8.0, 0.0), 0.0);
        // One colour at every vertex, or flat shading: no planes read.
        v[1].diffuse = [255; 4];
        let t = Prim::triangle([&v[0], &v[1], &v[2]], &v[0], &SHAPE, CLIP, &[]).unwrap();
        assert_eq!(t.flat, Some(([255; 4], [0; 4])));
        v[2].diffuse = [9; 4];
        let flat = Shape {
            flat: true,
            ..SHAPE
        };
        let t = Prim::triangle([&v[0], &v[1], &v[2]], &v[2], &flat, CLIP, &[]).unwrap();
        assert_eq!(t.flat, Some(([9; 4], [0; 4])));
    }
}

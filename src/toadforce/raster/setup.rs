//! Triangle set-up: which pixels a triangle covers, and what its
//! vertices' values are at each ("Rasterising" in `docs/toadforce.md`).
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
    /// The value at the triangle's origin.
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

/// A triangle ready to be drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Triangle {
    edges: [Edge; 3],
    /// Where a pixel's centre is in it, in 1/256 of a pixel.
    centre: i64,
    /// The columns the clip rectangle leaves, the last outside.
    columns: (i32, i32),
    /// The rows the triangle may have pixels in, the last outside.
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
    /// The colours, where the whole triangle has one of each.
    pub flat: Option<(Rgba, Rgba)>,
    /// Each slot's coordinates (s, t, r, q) times 1 / w.
    pub tex: [[Plane; 4]; SLOTS],
}

/// A position in 1/256 of a pixel, or None for one that is no number.
fn snap(v: f32) -> Option<i64> {
    v.is_finite()
        .then(|| (v.clamp(-LIMIT, LIMIT) as f64 * 256.0).round() as i64)
}

impl Triangle {
    /// The triangle of `v`, drawn as `shape` says inside `clip`, with
    /// the coordinate sets `sets` in its slots. `provoking` gives a
    /// flat-shaded triangle its colours. None for a triangle with no
    /// pixels: one with a position that is no number, with no area,
    /// culled, or outside `clip`.
    pub fn new(
        v: [&ScreenVertex; 3],
        provoking: &ScreenVertex,
        shape: &Shape,
        clip: [i32; 4],
        sets: &[u8],
    ) -> Option<Self> {
        let mut p = [(0i64, 0i64); 3];
        for (p, v) in p.iter_mut().zip(v) {
            *p = (snap(v.x)?, snap(v.y)?);
        }
        let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[1].1 - p[0].1) * (p[2].0 - p[0].0);
        // With y down, a positive area is a clockwise triangle.
        let culled = match shape.cull {
            2 => area > 0,
            3 => area < 0,
            _ => false,
        };
        if area == 0 || culled {
            return None;
        }
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
        let q = v.map(|v| v.rhw as f64);
        let over_w =
            |f: &dyn Fn(&ScreenVertex) -> f32| plane([0, 1, 2].map(|i| f(v[i]) as f64 * q[i]));
        let same = |f: &dyn Fn(&ScreenVertex) -> Rgba| f(v[0]) == f(v[1]) && f(v[1]) == f(v[2]);
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
        let half = centre as f64 / 256.0;
        Some(Triangle {
            edges,
            centre,
            columns: (clip[0], clip[2]),
            rows: (top, bottom),
            ox: (pos[0].0 - half) as f32,
            oy: (pos[0].1 - half) as f32,
            z: plane(v.map(|v| v.z as f64)),
            q: plane(q),
            diffuse: [0, 1, 2, 3].map(|ch| over_w(&|v| v.diffuse[ch] as f32)),
            specular: [0, 1, 2, 3].map(|ch| over_w(&|v| v.specular[ch] as f32)),
            flat,
            tex,
        })
    }

    /// The pixels of row `y` the triangle covers: the first, and the
    /// one after the last.
    pub fn span(&self, y: i32) -> Option<(i32, i32)> {
        let py = y as i64 * 256 + self.centre;
        let (mut first, mut last) = (self.columns.0 as i64, self.columns.1 as i64 - 1);
        for e in &self.edges {
            // The edge's value at column x is k - 256 * dy * x.
            let k = e.dx * (py - e.ay) + e.dy * (e.ax - self.centre) - e.bias;
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

    fn triangle(p: [(f32, f32); 3], shape: &Shape) -> Option<Triangle> {
        let v = p.map(|(x, y)| vertex(x, y));
        Triangle::new([&v[0], &v[1], &v[2]], &v[0], shape, CLIP, &[])
    }

    /// The pixels a triangle covers, as (x, y).
    fn pixels(t: &Triangle) -> Vec<(i32, i32)> {
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
        let t = Triangle::new([&v[0], &v[1], &v[2]], &v[0], &SHAPE, CLIP, &[3]).unwrap();
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
        let t = Triangle::new([&v[0], &v[1], &v[2]], &v[0], &SHAPE, CLIP, &[]).unwrap();
        assert_eq!(t.flat, Some(([255; 4], [0; 4])));
        v[2].diffuse = [9; 4];
        let flat = Shape {
            flat: true,
            ..SHAPE
        };
        let t = Triangle::new([&v[0], &v[1], &v[2]], &v[2], &flat, CLIP, &[]).unwrap();
        assert_eq!(t.flat, Some(([9; 4], [0; 4])));
    }
}

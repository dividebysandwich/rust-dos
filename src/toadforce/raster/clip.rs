//! Cutting primitives at planes, before the division by w
//! ("Clipping" in chapter 9 of `docs/toadforce-spec.md`).

use super::geometry::{PLANES, Vertex};

/// The corners a triangle cut at every plane can have: each plane adds
/// one at most.
pub const CORNERS: usize = 3 + PLANES;

/// A convex polygon's corners, in order.
#[derive(Clone, Copy)]
pub struct Polygon {
    corners: [Vertex; CORNERS],
    count: usize,
    /// Bit n: corner n is one of the triangle's own.
    own: u32,
    /// Bit n: the edge from corner n to the next is on one of the
    /// triangle's own.
    edges: u32,
}

impl Polygon {
    pub fn triangle(v: [Vertex; 3]) -> Polygon {
        let mut corners = [Vertex::default(); CORNERS];
        corners[..3].copy_from_slice(&v);
        Polygon {
            corners,
            count: 3,
            own: 7,
            edges: 7,
        }
    }

    pub fn corners(&self) -> &[Vertex] {
        &self.corners[..self.count]
    }

    /// The corners that are the triangle's own, and the edges that
    /// are on the triangle's own, a bit for each (`Polygon::own`,
    /// `Polygon::edges`): what a triangle drawn as points or lines
    /// shows.
    pub fn outline(&self) -> (u32, u32) {
        (self.own, self.edges)
    }

    /// What of the polygon is inside each plane of `planes` (a bit for
    /// each): nothing, where less than a triangle is.
    pub fn cut(&mut self, planes: u16) {
        for plane in (0..PLANES).filter(|p| planes >> p & 1 != 0) {
            let from = *self;
            (self.count, self.own, self.edges) = (0, 0, 0);
            let n = from.count;
            for i in 0..n {
                let (a, b) = (&from.corners[i], &from.corners[(i + 1) % n]);
                let (inside, next) = (a.dist[plane] >= 0.0, b.dist[plane] >= 0.0);
                let edge = from.edges >> i & 1 != 0;
                if inside {
                    self.push(*a, from.own >> i & 1 != 0, edge);
                }
                if inside != next {
                    // From the corner inside, whichever way round the
                    // edge goes, so that two primitives that share
                    // the edge get the same corner. The edge that
                    // comes back in goes on along the one that was
                    // cut; the one that leaves runs along the plane.
                    let (a, b) = if inside { (a, b) } else { (b, a) };
                    self.push(crossing(a, b, plane), false, edge && !inside);
                }
            }
            if self.count < 3 {
                self.count = 0;
                return;
            }
        }
    }

    /// Add the corner `v`. Rounding can make a polygon cross a plane
    /// more than twice, and so have more corners than a convex one
    /// can: those past the room are left out.
    fn push(&mut self, v: Vertex, own: bool, edge: bool) {
        if self.count < CORNERS {
            self.corners[self.count] = v;
            self.own |= (own as u32) << self.count;
            self.edges |= (edge as u32) << self.count;
            self.count += 1;
        }
    }
}

/// Where the edge from `a`, inside `plane`, to `b`, outside it, meets
/// the plane.
fn crossing(a: &Vertex, b: &Vertex, plane: usize) -> Vertex {
    let t = a.dist[plane] / (a.dist[plane] - b.dist[plane]);
    let mut v = a.lerp(b, t);
    v.dist[plane] = 0.0;
    v
}

/// What of the line from `a` to `b` is inside each plane of `planes`.
pub fn line(mut a: Vertex, mut b: Vertex, planes: u16) -> Option<(Vertex, Vertex)> {
    for plane in (0..PLANES).filter(|p| planes >> p & 1 != 0) {
        match (a.dist[plane] >= 0.0, b.dist[plane] >= 0.0) {
            (true, true) => {}
            (true, false) => b = crossing(&a, &b, plane),
            (false, true) => a = crossing(&b, &a, plane),
            (false, false) => return None,
        }
    }
    Some((a, b))
}

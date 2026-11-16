//! Cutting primitives at planes, before the division by w
//! ("Clipping" in chapter 9 of `docs/toadforce-spec.md`).

use super::geometry::{PLANE_FAR, PLANE_NEAR, PLANES, Vertex};

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
///
/// The corner is on the plane exactly. At the near plane its z is 0
/// and at the far one its z is its w, whatever the rounding of the
/// way along the edge made of them, so its depth is the end of the
/// depth range and no further: a pixel with a depth past the range is
/// left out where the depth test is on ("Values at a pixel" in chapter
/// 9), and a corner a rounding past it would lose the pixels beside the
/// cut.
fn crossing(a: &Vertex, b: &Vertex, plane: usize) -> Vertex {
    let t = a.dist[plane] / (a.dist[plane] - b.dist[plane]);
    let mut v = a.lerp(b, t);
    v.dist[plane] = 0.0;
    match plane {
        PLANE_NEAR => v.clip[2] = 0.0,
        PLANE_FAR => v.clip[2] = v.clip[3],
        _ => {}
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A vertex at `clip` with its distances to the near and far
    /// planes.
    fn vertex(clip: [f32; 4]) -> Vertex {
        let mut v = Vertex {
            clip,
            ..Vertex::default()
        };
        v.dist[PLANE_NEAR] = clip[2];
        v.dist[PLANE_FAR] = clip[3] - clip[2];
        v
    }

    /// The corner an edge gets at the near plane has a z of 0, and at
    /// the far plane a z that is its w: its depth is the range's end,
    /// where the way along the edge alone rounds a share of them past
    /// it.
    #[test]
    fn a_corner_at_the_near_or_far_plane_is_on_it_exactly() {
        let mut rounded = [0, 0];
        for i in 0..200u32 {
            let f = |n: u32| (i * n % 97) as f32 / 97.0;
            let inside = vertex([f(3), f(5), 0.1 + f(7), 1.3 + f(11)]);
            let near = vertex([f(13), f(17), -0.1 - f(19), 0.7 + f(23)]);
            let far = vertex([f(29), f(31), 2.1 + f(37), 1.1 + f(41)]);
            for (n, (plane, outside)) in [(PLANE_NEAR, near), (PLANE_FAR, far)].iter().enumerate() {
                let t = inside.dist[*plane] / (inside.dist[*plane] - outside.dist[*plane]);
                let [_, _, z, w] = inside.lerp(outside, t).clip;
                let end = [0.0, w][n];
                rounded[n] += (z != end) as u32;
                let [_, _, z, w] = crossing(&inside, outside, *plane).clip;
                assert_eq!(z, [0.0, w][n], "{i} {plane}");
            }
        }
        assert!(rounded[0] > 0 && rounded[1] > 0, "{rounded:?}");
    }
}

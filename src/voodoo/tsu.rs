//! The Voodoo2's triangle setup engine: the host gives it vertices (X, Y
//! and the parameters sSetupMode asks for, as floats, through the sV*
//! registers or command FIFO packets), and it works out the start values
//! and gradients the triangle registers take, culls back faces, and draws
//! independent triangles, strips and fans. From MAME's `begin_triangle`,
//! `draw_triangle` and `setup_and_draw_triangle` (voodoo_2.cpp).

use super::regs::*;
use super::Voodoo;

/// A vertex as the setup engine keeps it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub a: f32,
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub z: f32,
    pub wb: f32,
    pub w0: f32,
    pub s0: f32,
    pub t0: f32,
    pub w1: f32,
    pub s1: f32,
    pub t1: f32,
}

crate::state_fields!(Vertex { x, y, a, r, g, b, z, wb, w0, s0, t0, w1, s1, t1 });

/// sSetupMode's bits.
const SETUP_RGB: u32 = 1 << 0;
const SETUP_ALPHA: u32 = 1 << 1;
const SETUP_Z: u32 = 1 << 2;
const SETUP_WB: u32 = 1 << 3;
const SETUP_W0: u32 = 1 << 4;
const SETUP_ST0: u32 = 1 << 5;
const SETUP_W1: u32 = 1 << 6;
const SETUP_ST1: u32 = 1 << 7;
const FAN: u32 = 1 << 16;
const CULL: u32 = 1 << 17;
const CULL_NEGATIVE: u32 = 1 << 18;
const NO_PING_PONG: u32 = 1 << 19;

/// A float to a register's fixed point, truncated as the x86 FPU and C's
/// casts do (out of range gives the integer indefinite).
fn fixed32(v: f32) -> i32 {
    if v.is_finite() && v.abs() < 2147483648.0 { v as i32 } else { i32::MIN }
}

fn fixed64(v: f32) -> i64 {
    if v.is_finite() && v.abs() < 9.223372e18 { v as i64 } else { i64::MIN }
}

/// A 24-bit register value, as the colour registers keep it.
fn s24(v: i32) -> i32 {
    (v << 8) >> 8
}

impl Voodoo {
    /// The vertex the sV* registers hold.
    fn register_vertex(&self) -> Vertex {
        let f = |r: usize| f32::from_bits(self.reg[r]);
        Vertex {
            x: f(S_VX),
            y: f(S_VY),
            a: f(S_ALPHA),
            r: f(S_RED),
            g: f(S_GREEN),
            b: f(S_BLUE),
            z: f(S_VZ),
            wb: f(S_WB),
            w0: f(S_WTMU0),
            s0: f(S_S_W0),
            t0: f(S_T_W0),
            w1: f(S_WTMU1),
            s1: f(S_S_WTMU1),
            t1: f(S_T_WTMU1),
        }
    }

    /// sARGB: the colour as the four float registers.
    pub(crate) fn setup_argb(&mut self, data: u32) {
        self.reg[S_ALPHA] = ((data >> 24) as f32).to_bits();
        self.reg[S_RED] = (((data >> 16) & 0xFF) as f32).to_bits();
        self.reg[S_GREEN] = (((data >> 8) & 0xFF) as f32).to_bits();
        self.reg[S_BLUE] = ((data & 0xFF) as f32).to_bits();
    }

    /// sBeginTriCMD: the registers' vertex starts a strip or fan.
    pub(crate) fn begin_triangle(&mut self) {
        self.start_vertices(self.register_vertex());
    }

    /// sDrawTriCMD: the registers' vertex goes on the strip or fan.
    pub(crate) fn draw_triangle(&mut self) {
        self.add_vertex(self.register_vertex());
    }

    /// `v` alone, as all three vertices.
    pub(crate) fn start_vertices(&mut self, v: Vertex) {
        self.svert = [v; 3];
        self.sverts = 1;
    }

    /// `v` after the others: in a strip it and the last two make the
    /// triangle, in a fan it, the last and the first. From the third on,
    /// each draws.
    pub(crate) fn add_vertex(&mut self, v: Vertex) {
        if self.reg[S_SETUP_MODE] & FAN == 0 {
            self.svert[0] = self.svert[1];
        }
        self.svert[1] = self.svert[2];
        self.svert[2] = v;
        self.sverts += 1;
        if self.sverts >= 3 {
            self.setup_and_draw();
        }
    }

    /// The triangle of the three vertices: culled if it faces the way
    /// sSetupMode culls, otherwise its parameters into the triangle
    /// registers and drawn.
    fn setup_and_draw(&mut self) {
        let [v0, v1, v2] = self.svert;
        let mode = self.reg[S_SETUP_MODE];
        let divisor = (v0.x - v1.x) * (v0.y - v2.y) - (v0.x - v2.x) * (v0.y - v1.y);
        if mode & CULL != 0 {
            let mut sign = (mode & CULL_NEGATIVE != 0) as u32;
            // A strip's triangles turn the other way every other one.
            if mode & FAN == 0 && mode & NO_PING_PONG == 0 {
                sign ^= (self.sverts - 3) & 1;
            }
            if (divisor < 0.0) as u32 == sign {
                return;
            }
        }
        let divisor = 1.0 / divisor;

        let f = &mut self.fbi;
        let xy = |v: f32| fixed32(v * 16.0) as i16;
        (f.ax, f.ay, f.bx, f.by, f.cx, f.cy) = (xy(v0.x), xy(v0.y), xy(v1.x), xy(v1.y), xy(v2.x), xy(v2.y));

        let (dx1, dx2, dy1, dy2) = (v0.y - v2.y, v0.y - v1.y, v0.x - v1.x, v0.x - v2.x);
        // A parameter's start value and gradients, scaled.
        let gradients = |p: fn(&Vertex) -> f32, scale: f32| {
            let (a, b, c) = (p(&v0), p(&v1), p(&v2));
            let div = scale * divisor;
            (a * scale, ((a - b) * dx1 - (a - c) * dx2) * div, ((a - c) * dy1 - (a - b) * dy2) * div)
        };
        let colour = |p: fn(&Vertex) -> f32| {
            let (start, dx, dy) = gradients(p, 4096.0);
            (s24(fixed32(start)), s24(fixed32(dx)), s24(fixed32(dy)))
        };
        let wide = |p: fn(&Vertex) -> f32| {
            let (start, dx, dy) = gradients(p, 4294967296.0);
            (fixed64(start), fixed64(dx), fixed64(dy))
        };

        if mode & SETUP_RGB != 0 {
            (f.startr, f.drdx, f.drdy) = colour(|v| v.r);
            (f.startg, f.dgdx, f.dgdy) = colour(|v| v.g);
            (f.startb, f.dbdx, f.dbdy) = colour(|v| v.b);
        }
        if mode & SETUP_ALPHA != 0 {
            (f.starta, f.dadx, f.dady) = colour(|v| v.a);
        }
        if mode & SETUP_Z != 0 {
            let (start, dx, dy) = gradients(|v| v.z, 4096.0);
            (f.startz, f.dzdx, f.dzdy) = (fixed32(start), fixed32(dx), fixed32(dy));
        }
        let (wb, w0, w1) = (wide(|v| v.wb), wide(|v| v.w0), wide(|v| v.w1));
        let (st0, st1) = ([wide(|v| v.s0), wide(|v| v.t0)], [wide(|v| v.s1), wide(|v| v.t1)]);
        if mode & SETUP_WB != 0 {
            (f.startw, f.dwdx, f.dwdy) = wb;
        }
        for (unit, t) in self.tmu.iter_mut().enumerate() {
            let w = if mode & SETUP_W1 != 0 && unit == 1 {
                Some(w1)
            } else if mode & SETUP_W0 != 0 {
                Some(w0)
            } else if mode & SETUP_WB != 0 {
                Some(wb)
            } else {
                None
            };
            if let Some(w) = w {
                (t.startw, t.dwdx, t.dwdy) = w;
            }
            let st = if mode & SETUP_ST1 != 0 && unit == 1 {
                Some(st1)
            } else if mode & SETUP_ST0 != 0 {
                Some(st0)
            } else {
                None
            };
            if let Some([s, tc]) = st {
                (t.starts, t.dsdx, t.dsdy) = s;
                (t.startt, t.dtdx, t.dtdy) = tc;
            }
        }
        self.triangle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voodoo::{Board, Voodoo};

    fn vertex(x: f32, y: f32, r: f32) -> Vertex {
        Vertex { x, y, r, ..Default::default() }
    }

    #[test]
    fn gradients_of_a_right_triangle() {
        let mut v = Voodoo::with_workers(Board::Voodoo2, 0);
        v.reg[S_SETUP_MODE] = SETUP_RGB;
        // Red goes up by 1 a pixel across X, 2 down Y.
        v.start_vertices(vertex(0.0, 0.0, 10.0));
        v.add_vertex(vertex(10.0, 0.0, 20.0));
        v.add_vertex(vertex(0.0, 10.0, 30.0));
        let f = &v.fbi;
        assert_eq!((f.ax, f.ay, f.bx, f.by, f.cx, f.cy), (0, 0, 160, 0, 0, 160));
        assert_eq!((f.startr, f.drdx, f.drdy), (10 << 12, 1 << 12, 2 << 12));
        assert_eq!(f.startg, 0);
        assert_eq!(v.reg[FBI_TRIANGLES_OUT], 1);
    }

    #[test]
    fn strips_cull_every_other_triangle_the_other_way() {
        let mut v = Voodoo::with_workers(Board::Voodoo2, 0);
        // Cull the triangles of positive area.
        v.reg[S_SETUP_MODE] = CULL;
        v.start_vertices(vertex(0.0, 0.0, 0.0));
        v.add_vertex(vertex(0.0, 10.0, 0.0));
        // 0/0, 0/10, 10/0: negative, drawn.
        v.add_vertex(vertex(10.0, 0.0, 0.0));
        assert_eq!(v.reg[FBI_TRIANGLES_OUT], 1);
        // 0/10, 10/0, 10/10 turns the other way, which the ping-pong
        // makes the same: drawn too.
        v.add_vertex(vertex(10.0, 10.0, 0.0));
        assert_eq!(v.reg[FBI_TRIANGLES_OUT], 2);
        // Without the ping-pong correction it is culled.
        v.reg[S_SETUP_MODE] = CULL | NO_PING_PONG;
        v.add_vertex(vertex(0.0, 20.0, 0.0));
        assert_eq!(v.reg[FBI_TRIANGLES_OUT], 2);
    }

    #[test]
    fn fans_keep_their_first_vertex() {
        let mut v = Voodoo::with_workers(Board::Voodoo2, 0);
        v.reg[S_SETUP_MODE] = FAN;
        v.start_vertices(vertex(1.0, 1.0, 0.0));
        v.add_vertex(vertex(2.0, 1.0, 0.0));
        v.add_vertex(vertex(2.0, 2.0, 0.0));
        v.add_vertex(vertex(1.0, 2.0, 0.0));
        assert_eq!(v.svert[0].x, 1.0);
        assert_eq!((v.fbi.ax, v.fbi.bx, v.fbi.cx), (16, 32, 16));
    }
}

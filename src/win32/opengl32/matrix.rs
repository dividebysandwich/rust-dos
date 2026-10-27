//! The matrix stacks: modelview, projection and each texture unit's.
//!
//! OpenGL's matrices are column-major and multiply column vectors on
//! their right; Direct3D's are row-major and multiply row vectors on
//! their left. The same sixteen floats are the same transform in both,
//! so a stack's top goes to the inner device as it is.

use super::super::ddraw::d3d::pipeline::{IDENTITY, Matrix};
use super::{
    Context, GL_INVALID_ENUM, GL_STACK_OVERFLOW, GL_STACK_UNDERFLOW, MODELVIEW_DEPTH,
    PROJECTION_DEPTH, TEXTURE_DEPTH, ctx, d, error, f, read_doubles, read_floats,
};
use crate::cpu::Cpu;

pub const GL_MODELVIEW: u32 = 0x1700;
pub const GL_PROJECTION: u32 = 0x1701;
pub const GL_TEXTURE: u32 = 0x1702;

/// The product a * b of two column-major matrices.
pub fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    let mut r = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            r[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    r
}

/// m * (x, y, z, w).
pub fn apply(m: &Matrix, v: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|row| (0..4).map(|k| m[k * 4 + row] * v[k]).sum())
}

/// The stack glMatrixMode chose, and its greatest depth.
fn stack(c: &mut Context) -> (&mut Vec<Matrix>, usize) {
    match c.matrix_mode {
        GL_PROJECTION => (&mut c.projection, PROJECTION_DEPTH),
        GL_TEXTURE => {
            let unit = c.active_unit as usize;
            (&mut c.texture_matrix[unit], TEXTURE_DEPTH)
        }
        _ => (&mut c.modelview, MODELVIEW_DEPTH),
    }
}

/// The top of the current stack.
fn top(c: &mut Context) -> &mut Matrix {
    stack(c)
        .0
        .last_mut()
        .expect("a matrix stack is never empty")
}

/// Multiply the current matrix by `m` on its right.
fn mult(cpu: &mut Cpu, m: &Matrix) {
    if let Some(c) = ctx(cpu) {
        let t = top(c);
        *t = mul(t, m);
    }
}

/// glMatrixMode(mode).
pub fn matrix_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if ![GL_MODELVIEW, GL_PROJECTION, GL_TEXTURE].contains(&args[0]) {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if let Some(c) = ctx(cpu) {
        c.matrix_mode = args[0];
    }
    Some(0)
}

/// glLoadIdentity().
pub fn load_identity(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    if let Some(c) = ctx(cpu) {
        *top(c) = IDENTITY;
    }
    Some(0)
}

fn floats16(cpu: &mut Cpu, at: u32) -> Option<Matrix> {
    let v = read_floats(cpu, at, 16)?;
    Some(std::array::from_fn(|i| v[i]))
}

fn doubles16(cpu: &mut Cpu, at: u32) -> Option<Matrix> {
    let v = read_doubles(cpu, at, 16)?;
    Some(std::array::from_fn(|i| v[i] as f32))
}

/// glLoadMatrixf(m).
pub fn load_matrixf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(m) = floats16(cpu, args[0])
        && let Some(c) = ctx(cpu)
    {
        *top(c) = m;
    }
    Some(0)
}

/// glLoadMatrixd(m).
pub fn load_matrixd(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(m) = doubles16(cpu, args[0])
        && let Some(c) = ctx(cpu)
    {
        *top(c) = m;
    }
    Some(0)
}

/// glMultMatrixf(m).
pub fn mult_matrixf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(m) = floats16(cpu, args[0]) {
        mult(cpu, &m);
    }
    Some(0)
}

/// glMultMatrixd(m).
pub fn mult_matrixd(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(m) = doubles16(cpu, args[0]) {
        mult(cpu, &m);
    }
    Some(0)
}

/// glPushMatrix().
pub fn push_matrix(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    let (s, depth) = stack(c);
    if s.len() >= depth {
        error(cpu, GL_STACK_OVERFLOW);
        return Some(0);
    }
    let t = *s.last().expect("a matrix stack is never empty");
    s.push(t);
    Some(0)
}

/// glPopMatrix().
pub fn pop_matrix(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    let (s, _) = stack(c);
    if s.len() <= 1 {
        error(cpu, GL_STACK_UNDERFLOW);
        return Some(0);
    }
    s.pop();
    Some(0)
}

fn translate(cpu: &mut Cpu, x: f32, y: f32, z: f32) {
    let mut m = IDENTITY;
    m[12] = x;
    m[13] = y;
    m[14] = z;
    mult(cpu, &m);
}

/// glTranslatef(x, y, z).
pub fn translatef(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    translate(cpu, f(args, 0), f(args, 1), f(args, 2));
    Some(0)
}

/// glTranslated(x, y, z).
pub fn translated(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    translate(cpu, d(args, 0) as f32, d(args, 2) as f32, d(args, 4) as f32);
    Some(0)
}

fn scale(cpu: &mut Cpu, x: f32, y: f32, z: f32) {
    let mut m = IDENTITY;
    m[0] = x;
    m[5] = y;
    m[10] = z;
    mult(cpu, &m);
}

/// glScalef(x, y, z).
pub fn scalef(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    scale(cpu, f(args, 0), f(args, 1), f(args, 2));
    Some(0)
}

/// glScaled(x, y, z).
pub fn scaled(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    scale(cpu, d(args, 0) as f32, d(args, 2) as f32, d(args, 4) as f32);
    Some(0)
}

/// The rotation by `angle` degrees about the axis (x, y, z).
pub fn rotation(angle: f32, x: f32, y: f32, z: f32) -> Matrix {
    let len = (x * x + y * y + z * z).sqrt();
    if len == 0.0 {
        return IDENTITY;
    }
    let (x, y, z) = (x / len, y / len, z / len);
    let (s, c) = angle.to_radians().sin_cos();
    let t = 1.0 - c;
    [
        x * x * t + c,
        y * x * t + z * s,
        x * z * t - y * s,
        0.0,
        x * y * t - z * s,
        y * y * t + c,
        y * z * t + x * s,
        0.0,
        x * z * t + y * s,
        y * z * t - x * s,
        z * z * t + c,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

/// glRotatef(angle, x, y, z).
pub fn rotatef(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let m = rotation(f(args, 0), f(args, 1), f(args, 2), f(args, 3));
    mult(cpu, &m);
    Some(0)
}

/// glRotated(angle, x, y, z).
pub fn rotated(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [a, x, y, z] = [0, 2, 4, 6].map(|i| d(args, i) as f32);
    mult(cpu, &rotation(a, x, y, z));
    Some(0)
}

/// The six doubles of glOrtho and glFrustum.
fn box_of(args: &[u32]) -> [f64; 6] {
    std::array::from_fn(|i| d(args, 2 * i))
}

/// glOrtho(left, right, bottom, top, near, far).
pub fn ortho(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [l, r, b, t, n, fa] = box_of(args);
    if l == r || b == t || n == fa {
        error(cpu, super::GL_INVALID_VALUE);
        return Some(0);
    }
    let mut m = IDENTITY;
    m[0] = (2.0 / (r - l)) as f32;
    m[5] = (2.0 / (t - b)) as f32;
    m[10] = (-2.0 / (fa - n)) as f32;
    m[12] = (-(r + l) / (r - l)) as f32;
    m[13] = (-(t + b) / (t - b)) as f32;
    m[14] = (-(fa + n) / (fa - n)) as f32;
    mult(cpu, &m);
    Some(0)
}

/// glFrustum(left, right, bottom, top, near, far).
pub fn frustum(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [l, r, b, t, n, fa] = box_of(args);
    if n <= 0.0 || fa <= 0.0 || l == r || b == t || n == fa {
        error(cpu, super::GL_INVALID_VALUE);
        return Some(0);
    }
    let mut m = [0.0f32; 16];
    m[0] = (2.0 * n / (r - l)) as f32;
    m[5] = (2.0 * n / (t - b)) as f32;
    m[8] = ((r + l) / (r - l)) as f32;
    m[9] = ((t + b) / (t - b)) as f32;
    m[10] = (-(fa + n) / (fa - n)) as f32;
    m[11] = -1.0;
    m[14] = (-2.0 * fa * n / (fa - n)) as f32;
    mult(cpu, &m);
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation_moves_points() {
        let mut m = IDENTITY;
        m[12] = 1.0;
        m[13] = 2.0;
        m[14] = 3.0;
        assert_eq!(apply(&m, [1.0, 1.0, 1.0, 1.0]), [2.0, 3.0, 4.0, 1.0]);
    }

    #[test]
    fn products_apply_right_first() {
        // Scale by 2, then translate by 1 on x: T * S.
        let mut t = IDENTITY;
        t[12] = 1.0;
        let mut s = IDENTITY;
        s[0] = 2.0;
        let m = mul(&t, &s);
        assert_eq!(apply(&m, [1.0, 0.0, 0.0, 1.0])[0], 3.0);
    }

    #[test]
    fn rotation_about_z_turns_x_into_y() {
        let m = rotation(90.0, 0.0, 0.0, 1.0);
        let v = apply(&m, [1.0, 0.0, 0.0, 1.0]);
        assert!((v[0]).abs() < 1e-6 && (v[1] - 1.0).abs() < 1e-6);
    }
}

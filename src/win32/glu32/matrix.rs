//! GLU's matrix functions. gluOrtho2D is glOrtho with a near and far of
//! -1 and 1; gluPerspective, gluLookAt and gluPickMatrix multiply the
//! current matrix by the one they make, as Windows 98 SE's GLU32 does
//! through glMultMatrixd, glMultMatrixf, glTranslate and glScalef;
//! gluProject and gluUnProject only read the matrices the program gives
//! them. The arithmetic is the SGI sample implementation's, which
//! Microsoft's GLU is, in doubles; matrices are column-major, as
//! OpenGL's.

use super::super::opengl32::{self as gl, read_doubles};
use crate::cpu::Cpu;

type M = [f64; 16];

const IDENTITY: M = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// Multiply OpenGL's current matrix by `m` on its right.
fn mult(cpu: &mut Cpu, m: &M) {
    let m: [f32; 16] = std::array::from_fn(|i| m[i] as f32);
    gl::mult_matrix(cpu, &m);
}

/// gluPerspective's matrix: a field of view of `fovy` degrees, the
/// aspect ratio and the near and far planes; None where Windows' makes
/// none (the planes at one distance, no field of view, no aspect).
pub fn perspective_matrix(fovy: f64, aspect: f64, near: f64, far: f64) -> Option<M> {
    let radians = fovy / 2.0 * std::f64::consts::PI / 180.0;
    let dz = far - near;
    let sine = radians.sin();
    if dz == 0.0 || sine == 0.0 || aspect == 0.0 {
        return None;
    }
    let cotangent = radians.cos() / sine;
    let mut m = IDENTITY;
    m[0] = cotangent / aspect;
    m[5] = cotangent;
    m[10] = -(far + near) / dz;
    m[11] = -1.0;
    m[14] = -2.0 * near * far / dz;
    m[15] = 0.0;
    Some(m)
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `v` made one long; a vector of no length stays as it is.
fn normalize(v: [f64; 3]) -> [f64; 3] {
    let r = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if r == 0.0 { v } else { v.map(|c| c / r) }
}

/// gluLookAt's matrix: the eye at `eye` looking at `center`, `up` the
/// way up.
pub fn look_at_matrix(eye: [f64; 3], center: [f64; 3], up: [f64; 3]) -> M {
    let forward = normalize(std::array::from_fn(|i| center[i] - eye[i]));
    let side = normalize(cross(forward, up));
    let up = cross(side, forward);
    let mut m = IDENTITY;
    for i in 0..3 {
        m[i * 4] = side[i];
        m[i * 4 + 1] = up[i];
        m[i * 4 + 2] = -forward[i];
    }
    // Then glTranslated(-eye).
    let mut t = IDENTITY;
    t[12] = -eye[0];
    t[13] = -eye[1];
    t[14] = -eye[2];
    mul(&m, &t)
}

/// gluPickMatrix's matrix: a picking region of `w` by `h` at (`x`,
/// `y`) in the viewport `vp`; None for a region of no size, where
/// Windows' does nothing.
pub fn pick_matrix(x: f64, y: f64, w: f64, h: f64, vp: [i32; 4]) -> Option<M> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let vp = vp.map(f64::from);
    // glTranslatef, then glScalef, in floats.
    let tx = ((vp[2] - 2.0 * (x - vp[0])) / w) as f32 as f64;
    let ty = ((vp[3] - 2.0 * (y - vp[1])) / h) as f32 as f64;
    let sx = (vp[2] / w) as f32 as f64;
    let sy = (vp[3] / h) as f32 as f64;
    let mut m = IDENTITY;
    m[0] = sx;
    m[5] = sy;
    m[12] = tx;
    m[13] = ty;
    Some(m)
}

/// The product a * b of two column-major matrices.
fn mul(a: &M, b: &M) -> M {
    std::array::from_fn(|i| {
        let (col, row) = (i / 4, i % 4);
        (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum()
    })
}

/// m * v.
fn apply(m: &M, v: [f64; 4]) -> [f64; 4] {
    std::array::from_fn(|row| (0..4).map(|k| m[k * 4 + row] * v[k]).sum())
}

/// The inverse of `m`, None for a matrix that has none.
fn invert(m: &M) -> Option<M> {
    // Gauss-Jordan elimination with partial pivoting, on rows of the
    // matrix as OpenGL lays it out (column-major: element (r, c) is
    // m[c * 4 + r]).
    let mut a = [[0.0f64; 8]; 4];
    for (r, row) in a.iter_mut().enumerate() {
        for c in 0..4 {
            row[c] = m[c * 4 + r];
        }
        row[4 + r] = 1.0;
    }
    for col in 0..4 {
        let pivot = (col..4).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col] == 0.0 {
            return None;
        }
        a.swap(col, pivot);
        let p = a[col][col];
        for v in a[col].iter_mut() {
            *v /= p;
        }
        for r in 0..4 {
            if r != col {
                let f = a[r][col];
                if f != 0.0 {
                    let pivot_row = a[col];
                    for (v, p) in a[r].iter_mut().zip(pivot_row) {
                        *v -= f * p;
                    }
                }
            }
        }
    }
    Some(std::array::from_fn(|i| a[i % 4][4 + i / 4]))
}

/// gluProject's arithmetic: the window coordinates of `obj`, None when
/// the point goes to w = 0.
pub fn project(obj: [f64; 3], model: &M, proj: &M, vp: [i32; 4]) -> Option<[f64; 3]> {
    let eye = apply(model, [obj[0], obj[1], obj[2], 1.0]);
    let clip = apply(proj, eye);
    if clip[3] == 0.0 {
        return None;
    }
    let ndc = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
    let [x, y, z] = ndc.map(|v| v * 0.5 + 0.5);
    Some([
        x * f64::from(vp[2]) + f64::from(vp[0]),
        y * f64::from(vp[3]) + f64::from(vp[1]),
        z,
    ])
}

/// gluUnProject's arithmetic: the object coordinates of the window
/// point `win`, None when the matrices have no inverse or the point
/// goes to w = 0.
pub fn un_project(win: [f64; 3], model: &M, proj: &M, vp: [i32; 4]) -> Option<[f64; 3]> {
    let inverse = invert(&mul(proj, model))?;
    let x = (win[0] - f64::from(vp[0])) / f64::from(vp[2]);
    let y = (win[1] - f64::from(vp[1])) / f64::from(vp[3]);
    let v = [x, y, win[2]].map(|c| c * 2.0 - 1.0);
    let out = apply(&inverse, [v[0], v[1], v[2], 1.0]);
    if out[3] == 0.0 {
        return None;
    }
    Some([out[0] / out[3], out[1] / out[3], out[2] / out[3]])
}

/// The doubles of the arguments from `i`.
fn doubles<const N: usize>(args: &[u32], i: usize) -> [f64; N] {
    std::array::from_fn(|k| gl::d(args, i + 2 * k))
}

/// gluOrtho2D(left, right, bottom, top): glOrtho with a near and far of
/// -1 and 1.
pub fn ortho_2d(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut a = args[..8].to_vec();
    for v in [-1.0f64, 1.0] {
        let bits = v.to_bits();
        a.extend([bits as u32, (bits >> 32) as u32]);
    }
    gl::ortho(cpu, &a)
}

/// gluPerspective(fovy, aspect, zNear, zFar).
pub fn perspective(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [fovy, aspect, near, far] = doubles(args, 0);
    if let Some(m) = perspective_matrix(fovy, aspect, near, far) {
        mult(cpu, &m);
    }
    Some(0)
}

/// gluLookAt(eyex, eyey, eyez, centerx, centery, centerz, upx, upy,
/// upz). Windows' multiplies by the rotation in floats
/// (glMultMatrixf), then translates in doubles; both reach OpenGL's
/// matrix as floats here.
pub fn look_at(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v: [f64; 9] = doubles(args, 0);
    let m = look_at_matrix([v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]);
    mult(cpu, &m);
    Some(0)
}

/// The viewport at `at`: four integers.
fn viewport(cpu: &mut Cpu, at: u32) -> Option<[i32; 4]> {
    if at == 0 || !cpu.bus.guest_probe(at, 16, false) {
        return None;
    }
    Some(std::array::from_fn(|i| {
        cpu.bus.guest_read_32(at + 4 * i as u32) as i32
    }))
}

/// Sixteen doubles at `at`.
fn matrix_at(cpu: &mut Cpu, at: u32) -> Option<M> {
    let v = read_doubles(cpu, at, 16)?;
    Some(std::array::from_fn(|i| v[i]))
}

/// gluPickMatrix(x, y, width, height, viewport).
pub fn pick_matrix_api(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [x, y, w, h] = doubles(args, 0);
    if let Some(vp) = viewport(cpu, args[8])
        && let Some(m) = pick_matrix(x, y, w, h, vp)
    {
        mult(cpu, &m);
    }
    Some(0)
}

/// Write `v` as three doubles at the addresses `at`.
fn write_doubles(cpu: &mut Cpu, at: [u32; 3], v: [f64; 3]) -> bool {
    if at
        .iter()
        .any(|&a| a == 0 || !cpu.bus.guest_probe(a, 8, true))
    {
        return false;
    }
    for (a, x) in at.into_iter().zip(v) {
        let bits = x.to_bits();
        cpu.bus.guest_write_32(a, bits as u32);
        cpu.bus.guest_write_32(a + 4, (bits >> 32) as u32);
    }
    true
}

/// gluProject's or gluUnProject's arithmetic: a point, the modelview
/// and projection matrices and the viewport to the point mapped.
type MapFn = fn([f64; 3], &M, &M, [i32; 4]) -> Option<[f64; 3]>;

/// gluProject and gluUnProject: three doubles, the modelview and
/// projection matrices and the viewport, and where the three results
/// go. GL_TRUE when they were written.
fn map_point(cpu: &mut Cpu, args: &[u32], f: MapFn) -> Option<u32> {
    let p: [f64; 3] = doubles(args, 0);
    let (Some(model), Some(proj), Some(vp)) = (
        matrix_at(cpu, args[6]),
        matrix_at(cpu, args[7]),
        viewport(cpu, args[8]),
    ) else {
        return Some(0);
    };
    let Some(out) = f(p, &model, &proj, vp) else {
        return Some(0);
    };
    Some(write_doubles(cpu, [args[9], args[10], args[11]], out) as u32)
}

/// gluProject(objx, objy, objz, modelMatrix, projMatrix, viewport,
/// winx, winy, winz).
pub fn project_api(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    map_point(cpu, args, project)
}

/// gluUnProject(winx, winy, winz, modelMatrix, projMatrix, viewport,
/// objx, objy, objz).
pub fn un_project_api(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    map_point(cpu, args, un_project)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &[f64], b: &[f64]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
    }

    #[test]
    fn perspective_puts_the_near_plane_at_minus_one_and_the_far_at_one() {
        let m = perspective_matrix(90.0, 2.0, 1.0, 10.0).unwrap();
        let near = apply(&m, [0.0, 0.0, -1.0, 1.0]);
        let far = apply(&m, [0.0, 0.0, -10.0, 1.0]);
        assert!((near[2] / near[3] + 1.0).abs() < 1e-12);
        assert!((far[2] / far[3] - 1.0).abs() < 1e-12);
        // 90 degrees: the top edge at the near plane is y = 1, and the
        // aspect of 2 puts the right edge at x = 2.
        let corner = apply(&m, [2.0, 1.0, -1.0, 1.0]);
        assert!(close(
            &[corner[0] / corner[3], corner[1] / corner[3]],
            &[1.0, 1.0]
        ));
        assert!(perspective_matrix(90.0, 0.0, 1.0, 10.0).is_none());
        assert!(perspective_matrix(0.0, 1.0, 1.0, 10.0).is_none());
        assert!(perspective_matrix(60.0, 1.0, 5.0, 5.0).is_none());
    }

    #[test]
    fn look_at_takes_the_eye_to_the_origin_looking_down_minus_z() {
        let m = look_at_matrix([1.0, 2.0, 3.0], [1.0, 2.0, -7.0], [0.0, 1.0, 0.0]);
        assert!(close(
            &apply(&m, [1.0, 2.0, 3.0, 1.0]),
            &[0.0, 0.0, 0.0, 1.0]
        ));
        assert!(close(
            &apply(&m, [1.0, 2.0, -7.0, 1.0]),
            &[0.0, 0.0, -10.0, 1.0]
        ));
        // For an eye at the origin looking down -x with y up, +z is on
        // its left: the point (0, 0, 1) is at x = -1.
        let m = look_at_matrix([0.0; 3], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!(close(
            &apply(&m, [0.0, 0.0, 1.0, 1.0]),
            &[-1.0, 0.0, 0.0, 1.0]
        ));
    }

    #[test]
    fn pick_matrix_blows_the_region_up_to_the_whole_viewport() {
        let vp = [0, 0, 100, 50];
        let m = pick_matrix(25.0, 10.0, 10.0, 10.0, vp).unwrap();
        // The region's center goes to the center, its corners to the
        // edges of clip space.
        let at = |x: f64, y: f64| {
            // A window point as normalized device coordinates.
            let ndc = [x / 50.0 - 1.0, y / 25.0 - 1.0, 0.0, 1.0];
            apply(&m, ndc)
        };
        assert!(close(&at(25.0, 10.0)[..2], &[0.0, 0.0]));
        assert!(close(&at(30.0, 15.0)[..2], &[1.0, 1.0]));
        assert!(close(&at(20.0, 5.0)[..2], &[-1.0, -1.0]));
        assert!(pick_matrix(25.0, 10.0, 0.0, 10.0, vp).is_none());
    }

    #[test]
    fn un_project_undoes_project() {
        let model = look_at_matrix([3.0, 4.0, 5.0], [0.0; 3], [0.0, 1.0, 0.0]);
        let proj = perspective_matrix(60.0, 4.0 / 3.0, 0.5, 100.0).unwrap();
        let vp = [10, 20, 640, 480];
        let obj = [0.25, -0.5, 1.0];
        let win = project(obj, &model, &proj, vp).unwrap();
        assert!(win[0] > 10.0 && win[0] < 650.0 && win[2] > 0.0 && win[2] < 1.0);
        let back = un_project(win, &model, &proj, vp).unwrap();
        assert!(close(&back, &obj), "{back:?}");
        // The center of the viewport at depth 0 is on the line of sight.
        let ortho = IDENTITY;
        let p = project([0.0, 0.0, 0.0], &ortho, &ortho, vp).unwrap();
        assert!(close(&p, &[330.0, 260.0, 0.5]));
    }

    #[test]
    fn a_singular_matrix_has_no_inverse() {
        let mut m = IDENTITY;
        m[10] = 0.0;
        assert!(invert(&m).is_none());
        assert!(close(&invert(&IDENTITY).unwrap(), &IDENTITY));
    }
}

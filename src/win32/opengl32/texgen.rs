//! Texture coordinate generation: glTexGen and glGetTexGen (OpenGL
//! 1.1, section 2.10.4), for each texture unit.
//!
//! A coordinate (s, t, r or q) of a unit whose GL_TEXTURE_GEN_ enable
//! is on is made from the vertex, in place of the one the program gave,
//! before the texture matrix. GL_OBJECT_LINEAR is the dot product of
//! the object plane and the vertex's object coordinates, GL_EYE_LINEAR
//! that of the eye plane, kept in eye coordinates as it was given
//! (`matrix::eye_plane`), and the vertex's eye coordinates, and
//! GL_SPHERE_MAP (s and t only) the point a sphere map has for the
//! vertex's eye position and normal. The coordinates are made on the
//! processor when the draw has its vertices (`draw::draw`), before
//! they go to the card or to Direct3D's pipeline, which take them as
//! the program's.
//!
//! The arithmetic of the sphere map follows Mesa
//! (src/mesa/tnl/t_vb_texgen.c, under the MIT license: Copyright (C)
//! 1999-2007 Brian Paul): the eye position as a 3-vector made a unit
//! vector, and the normal in eye coordinates as lighting has it,
//! normalized only with GL_NORMALIZE.

use super::{
    CAP_NORMALIZE, Context, GL_INVALID_ENUM, ImmVertex, UNITS, ctx, d, error, f, read_doubles,
    read_floats, write_doubles,
};
use super::{matrix, shade};
use crate::cpu::Cpu;

// The coordinates.
const GL_S: u32 = 0x2000;
const GL_Q: u32 = 0x2003;

// The parameters and modes.
const GL_TEXTURE_GEN_MODE: u32 = 0x2500;
const GL_OBJECT_PLANE: u32 = 0x2501;
const GL_EYE_PLANE: u32 = 0x2502;
pub const GL_EYE_LINEAR: u32 = 0x2400;
pub const GL_OBJECT_LINEAR: u32 = 0x2401;
pub const GL_SPHERE_MAP: u32 = 0x2402;

/// The enables GL_TEXTURE_GEN_S to GL_TEXTURE_GEN_Q.
pub const GL_TEXTURE_GEN_S: u32 = 0x0C60;
pub const GL_TEXTURE_GEN_Q: u32 = 0x0C63;

/// The W32 version whose saved contexts first have each unit's
/// generation and the clip planes (`clip::Clip`). An older state's
/// contexts load with OpenGL's initial values, which are what its
/// glTexGen and glClipPlane, that did nothing, left. Their enables load
/// off as well: the older build kept a glEnable of any of them as one
/// bit of `caps`, which glIsEnabled gave back, and that bit doesn't
/// say which coordinate, unit or plane it was for.
pub const TEXGEN_SAVED: u16 = 98;

/// One texture unit's coordinate generation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TexGen {
    /// GL_TEXTURE_GEN_S to GL_TEXTURE_GEN_Q, a bit each from s.
    pub enabled: u8,
    /// Each coordinate's GL_TEXTURE_GEN_MODE.
    pub mode: [u32; 4],
    /// Each coordinate's GL_OBJECT_PLANE, and its GL_EYE_PLANE in eye
    /// coordinates.
    pub object: [[f32; 4]; 4],
    pub eye: [[f32; 4]; 4],
}

/// The initial planes: (1, 0, 0, 0) for s, (0, 1, 0, 0) for t and zero
/// for r and q, object and eye alike (table 6.14).
const PLANES: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0; 4],
    [0.0; 4],
];

impl Default for TexGen {
    fn default() -> TexGen {
        TexGen {
            enabled: 0,
            mode: [GL_EYE_LINEAR; 4],
            object: PLANES,
            eye: PLANES,
        }
    }
}

crate::state_fields!(TexGen {
    enabled,
    mode,
    object,
    eye
});

/// The coordinate `coord` names, 0 for s to 3 for q.
fn coordinate(coord: u32) -> Option<usize> {
    (GL_S..=GL_Q)
        .contains(&coord)
        .then(|| (coord - GL_S) as usize)
}

/// The bit of `TexGen::enabled` of the enable `cap`, if it is one of
/// GL_TEXTURE_GEN_S to GL_TEXTURE_GEN_Q.
pub fn enable_bit(cap: u32) -> Option<u8> {
    (GL_TEXTURE_GEN_S..=GL_TEXTURE_GEN_Q)
        .contains(&cap)
        .then(|| 1 << (cap - GL_TEXTURE_GEN_S))
}

/// Where a call's values are.
#[derive(Clone, Copy)]
enum Values {
    One(f64),
    Ints(u32),
    Floats(u32),
    Doubles(u32),
}

impl Values {
    /// The `n` values, None where they can't be read.
    fn read(self, cpu: &mut Cpu, n: usize) -> Option<Vec<f64>> {
        match self {
            Values::One(x) => Some(vec![x]),
            Values::Floats(at) => Some(
                read_floats(cpu, at, n)?
                    .into_iter()
                    .map(f64::from)
                    .collect(),
            ),
            Values::Doubles(at) => read_doubles(cpu, at, n),
            Values::Ints(at) => {
                if at == 0 || !cpu.bus.guest_probe(at, n * 4, false) {
                    return None;
                }
                Some(
                    (0..n as u32)
                        .map(|i| cpu.bus.guest_read_32(at + 4 * i) as i32 as f64)
                        .collect(),
                )
            }
        }
    }
}

/// glTexGen: set the parameter `p` of the coordinate `coord` of the
/// active texture unit. The forms without a v take the mode only; the
/// sphere map is for s and t only (section 2.10.4's errors).
fn tex_gen(cpu: &mut Cpu, coord: u32, p: u32, v: Values) {
    let Some(i) = coordinate(coord) else {
        error(cpu, GL_INVALID_ENUM);
        return;
    };
    let vector = !matches!(v, Values::One(_));
    let n = match p {
        GL_TEXTURE_GEN_MODE => 1,
        GL_OBJECT_PLANE | GL_EYE_PLANE if vector => 4,
        _ => {
            error(cpu, GL_INVALID_ENUM);
            return;
        }
    };
    let Some(x) = v.read(cpu, n) else {
        return;
    };
    if p == GL_TEXTURE_GEN_MODE {
        let mode = x[0] as i64 as u32;
        let valid = match mode {
            GL_EYE_LINEAR | GL_OBJECT_LINEAR => true,
            GL_SPHERE_MAP => i < 2,
            _ => false,
        };
        if !valid {
            error(cpu, GL_INVALID_ENUM);
            return;
        }
        if let Some(c) = ctx(cpu) {
            let unit = c.active_unit as usize;
            c.texgen[unit].mode[i] = mode;
        }
        return;
    }
    let Some(c) = ctx(cpu) else {
        return;
    };
    let plane = [x[0], x[1], x[2], x[3]].map(|x| x as f32);
    let unit = c.active_unit as usize;
    if p == GL_OBJECT_PLANE {
        c.texgen[unit].object[i] = plane;
    } else {
        let modelview = *c.modelview.last().expect("a matrix stack is never empty");
        c.texgen[unit].eye[i] = matrix::eye_plane(&modelview, plane);
    }
}

/// glTexGeni(coord, pname, param).
pub fn tex_geni(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    tex_gen(cpu, args[0], args[1], Values::One(args[2] as i32 as f64));
    Some(0)
}

/// glTexGenf(coord, pname, param).
pub fn tex_genf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    tex_gen(cpu, args[0], args[1], Values::One(f(args, 2) as f64));
    Some(0)
}

/// glTexGend(coord, pname, param).
pub fn tex_gend(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    tex_gen(cpu, args[0], args[1], Values::One(d(args, 2)));
    Some(0)
}

/// glTexGeniv(coord, pname, params).
pub fn tex_geniv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    tex_gen(cpu, args[0], args[1], Values::Ints(args[2]));
    Some(0)
}

/// glTexGenfv(coord, pname, params).
pub fn tex_genfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    tex_gen(cpu, args[0], args[1], Values::Floats(args[2]));
    Some(0)
}

/// glTexGendv(coord, pname, params).
pub fn tex_gendv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    tex_gen(cpu, args[0], args[1], Values::Doubles(args[2]));
    Some(0)
}

/// How glGetTexGen writes its values.
#[derive(Clone, Copy, PartialEq)]
enum Out {
    Ints,
    Floats,
    Doubles,
}

/// glGetTexGen: write the parameter `p` of the coordinate `coord` of
/// the active unit at `at`. Integers are rounded (section 6.1.2).
fn get_tex_gen(cpu: &mut Cpu, args: &[u32], out: Out) {
    let (coord, p, at) = (args[0], args[1], args[2]);
    let Some(c) = ctx(cpu) else {
        return;
    };
    let g = &c.texgen[c.active_unit as usize];
    let v: Vec<f64> = match (coordinate(coord), p) {
        (Some(i), GL_TEXTURE_GEN_MODE) => vec![g.mode[i] as f64],
        (Some(i), GL_OBJECT_PLANE) => g.object[i].map(f64::from).to_vec(),
        (Some(i), GL_EYE_PLANE) => g.eye[i].map(f64::from).to_vec(),
        _ => {
            error(cpu, GL_INVALID_ENUM);
            return;
        }
    };
    if out == Out::Doubles {
        write_doubles(cpu, at, &v);
        return;
    }
    if at == 0 || !cpu.bus.guest_probe(at, v.len() * 4, true) {
        return;
    }
    for (i, &x) in v.iter().enumerate() {
        let word = match out {
            Out::Ints => x.round() as i32 as u32,
            _ => (x as f32).to_bits(),
        };
        cpu.bus.guest_write_32(at + 4 * i as u32, word);
    }
}

/// glGetTexGeniv(coord, pname, params).
pub fn get_tex_geniv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_tex_gen(cpu, args, Out::Ints);
    Some(0)
}

/// glGetTexGenfv(coord, pname, params).
pub fn get_tex_genfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_tex_gen(cpu, args, Out::Floats);
    Some(0)
}

/// glGetTexGendv(coord, pname, params).
pub fn get_tex_gendv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_tex_gen(cpu, args, Out::Doubles);
    Some(0)
}

fn dot(a: [f32; 4], b: [f32; 4]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

/// The sphere map's s and t for a vertex at `eye` in eye coordinates
/// with the eye normal `n`: the reflection r of the unit vector u to
/// the vertex, r = u - 2 n (n . u), at (rx / m + 1/2, ry / m + 1/2)
/// with m = 2 sqrt(rx^2 + ry^2 + (rz + 1)^2), and (1/2, 1/2) for an m
/// of 0, as Mesa leaves it.
fn sphere(eye: [f32; 4], n: [f32; 3]) -> [f32; 2] {
    let length = (eye[0] * eye[0] + eye[1] * eye[1] + eye[2] * eye[2]).sqrt();
    let u = if length > 0.0 {
        [eye[0] / length, eye[1] / length, eye[2] / length]
    } else {
        [eye[0], eye[1], eye[2]]
    };
    let two_nu = 2.0 * (n[0] * u[0] + n[1] * u[1] + n[2] * u[2]);
    let r = [
        u[0] - n[0] * two_nu,
        u[1] - n[1] * two_nu,
        u[2] - n[2] * two_nu,
    ];
    let m = r[0] * r[0] + r[1] * r[1] + (r[2] + 1.0) * (r[2] + 1.0);
    let m = if m != 0.0 { 0.5 / m.sqrt() } else { m };
    [r[0] * m + 0.5, r[1] * m + 0.5]
}

/// The vertices `raw` with the coordinates the context `c` generates
/// in place of their own, None when it generates none.
pub fn generate(c: &Context, raw: &[ImmVertex]) -> Option<Vec<ImmVertex>> {
    let units: Vec<usize> = (0..UNITS)
        .filter(|&u| c.texgen[u].enabled & 0xF != 0)
        .collect();
    if units.is_empty() {
        return None;
    }
    let gens = |mode: u32| {
        units.iter().any(|&u| {
            let g = &c.texgen[u];
            (0..4).any(|i| g.enabled >> i & 1 != 0 && g.mode[i] == mode)
        })
    };
    let (eye_linear, sphere_map) = (gens(GL_EYE_LINEAR), gens(GL_SPHERE_MAP));
    let modelview = *c.modelview.last().expect("a matrix stack is never empty");
    let normals = sphere_map
        .then(|| shade::normal_matrix(&modelview))
        .flatten();
    let normalize = c.caps & CAP_NORMALIZE != 0;
    let mut out = raw.to_vec();
    for v in out.iter_mut() {
        let eye = (eye_linear || sphere_map).then(|| matrix::apply(&modelview, v.position));
        let mapped = sphere_map.then(|| {
            let n = shade::eye_normal(normals.as_ref(), v.normal, normalize);
            sphere(eye.unwrap_or_default(), n)
        });
        for &u in &units {
            let g = &c.texgen[u];
            for i in (0..4).filter(|i| g.enabled >> i & 1 != 0) {
                v.tex[u][i] = match g.mode[i] {
                    GL_OBJECT_LINEAR => dot(g.object[i], v.position),
                    GL_EYE_LINEAR => dot(g.eye[i], eye.unwrap_or_default()),
                    _ => mapped.unwrap_or_default()[i.min(1)],
                };
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        super::super::new_context(0, 0, 1)
    }

    fn vertex(position: [f32; 4], normal: [f32; 3]) -> ImmVertex {
        ImmVertex {
            position,
            normal,
            tex: [[0.25, 0.5, 0.75, 1.0]; UNITS],
            ..ImmVertex::default()
        }
    }

    #[test]
    fn no_generation_leaves_the_vertices_alone() {
        let c = context();
        assert!(generate(&c, &[vertex([1.0; 4], [0.0, 0.0, 1.0])]).is_none());
    }

    #[test]
    fn object_linear_is_the_object_planes_dot_product() {
        let mut c = context();
        c.texgen[1].enabled = 0b0011;
        c.texgen[1].mode = [GL_OBJECT_LINEAR; 4];
        c.texgen[1].object[0] = [2.0, 0.0, 0.0, 1.0];
        c.texgen[1].object[1] = [0.0, 0.0, 1.0, -1.0];
        // The modelview matrix doesn't come into it.
        c.modelview[0] = matrix::rotation(90.0, 0.0, 0.0, 1.0);
        let v = generate(&c, &[vertex([3.0, 4.0, 5.0, 1.0], [0.0; 3])]).unwrap()[0];
        assert_eq!(v.tex[1], [7.0, 4.0, 0.75, 1.0]);
        // Unit 0 generates nothing.
        assert_eq!(v.tex[0], [0.25, 0.5, 0.75, 1.0]);
    }

    #[test]
    fn eye_linear_takes_the_eye_coordinates() {
        let mut c = context();
        c.texgen[0].enabled = 0b0001;
        c.texgen[0].eye[0] = [1.0, 0.0, 0.0, 0.0];
        // Translated by 10 along x: the eye's x is the object's + 10.
        c.modelview[0][12] = 10.0;
        let v = generate(&c, &[vertex([1.0, 2.0, 3.0, 1.0], [0.0; 3])]).unwrap()[0];
        assert_eq!(v.tex[0][0], 11.0);
    }

    #[test]
    fn an_eye_plane_is_kept_through_the_inverse_modelview() {
        // Given under a translation of 10 along x, the plane x = 0 is the
        // plane x = 10 of the eye: a point there has a coordinate of 0.
        let mut m = super::super::IDENTITY;
        m[12] = 10.0;
        let p = matrix::eye_plane(&m, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(p, [1.0, 0.0, 0.0, -10.0]);
        // A matrix with no inverse leaves it as given.
        let zero = [0.0; 16];
        assert_eq!(
            matrix::eye_plane(&zero, [1.0, 2.0, 3.0, 4.0]),
            [1.0, 2.0, 3.0, 4.0]
        );
    }

    #[test]
    fn the_sphere_map_of_a_surface_facing_the_eye_is_its_middle() {
        let mut c = context();
        c.texgen[0].enabled = 0b0011;
        c.texgen[0].mode = [GL_SPHERE_MAP, GL_SPHERE_MAP, GL_EYE_LINEAR, GL_EYE_LINEAR];
        // Straight ahead, facing the eye: r = (0, 0, 1), m = 4.
        let v = generate(&c, &[vertex([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0])]).unwrap()[0];
        assert_eq!([v.tex[0][0], v.tex[0][1]], [0.5, 0.5]);
        // A normal tilted 45 degrees toward +x throws the line of sight
        // to the side: u = (0, 0, -1), r = (1, 0, 0), m = 2 sqrt 2.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let v = generate(&c, &[vertex([0.0, 0.0, -5.0, 1.0], [h, 0.0, h])]).unwrap()[0];
        let s = 0.5 + 1.0 / (2.0 * 2f32.sqrt());
        assert!((v.tex[0][0] - s).abs() < 1e-6, "{}", v.tex[0][0]);
        assert!((v.tex[0][1] - 0.5).abs() < 1e-6);
    }
}

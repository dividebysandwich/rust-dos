//! OpenGL's lighting state (OpenGL 1.1, section 2.13): the lights, the
//! front and back materials, the lighting model and glColorMaterial,
//! with glLight, glLightModel, glMaterial, glColorMaterial and the
//! queries that read them back.
//!
//! This is only the state as the program sets it. `shade.rs` evaluates
//! the lighting equation over it for a vertex, as a function of this
//! state and the vertex, so that a card that lights vertices itself can
//! take the same state instead.

use super::state::{GL_BACK, GL_FRONT, GL_FRONT_AND_BACK};
use super::{
    CAP_COLOR_MATERIAL, Context, GL_INVALID_ENUM, GL_INVALID_OPERATION, GL_INVALID_VALUE, ctx,
    error, matrix, signed_unit,
};
use crate::cpu::Cpu;

/// How many lights there are (GL_MAX_LIGHTS).
pub const LIGHTS: usize = 8;

const GL_LIGHT0: u32 = 0x4000;

/// The light GL_LIGHTi `light` names, if it is one.
pub fn index(light: u32) -> Option<usize> {
    let i = light.wrapping_sub(GL_LIGHT0) as usize;
    (i < LIGHTS).then_some(i)
}

// Light parameters.
const GL_AMBIENT: u32 = 0x1200;
const GL_DIFFUSE: u32 = 0x1201;
const GL_SPECULAR: u32 = 0x1202;
const GL_POSITION: u32 = 0x1203;
const GL_SPOT_DIRECTION: u32 = 0x1204;
const GL_SPOT_EXPONENT: u32 = 0x1205;
const GL_SPOT_CUTOFF: u32 = 0x1206;
const GL_CONSTANT_ATTENUATION: u32 = 0x1207;
const GL_LINEAR_ATTENUATION: u32 = 0x1208;
const GL_QUADRATIC_ATTENUATION: u32 = 0x1209;

// Material parameters, and glColorMaterial's modes.
pub const GL_EMISSION: u32 = 0x1600;
const GL_SHININESS: u32 = 0x1601;
pub const GL_AMBIENT_AND_DIFFUSE: u32 = 0x1602;
const GL_COLOR_INDEXES: u32 = 0x1603;

// The lighting model.
pub const GL_LIGHT_MODEL_LOCAL_VIEWER: u32 = 0x0B51;
pub const GL_LIGHT_MODEL_TWO_SIDE: u32 = 0x0B52;
pub const GL_LIGHT_MODEL_AMBIENT: u32 = 0x0B53;
pub const GL_COLOR_MATERIAL_FACE: u32 = 0x0B55;
pub const GL_COLOR_MATERIAL_PARAMETER: u32 = 0x0B56;

/// A light source, its position and spot direction in eye coordinates
/// as the modelview matrix took them when they were given.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub ambient: [f32; 4],
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    pub position: [f32; 4],
    pub direction: [f32; 3],
    pub exponent: f32,
    /// The spot's cutoff angle in degrees, 180 for a light that isn't a
    /// spot.
    pub cutoff: f32,
    /// The constant, linear and quadratic attenuation factors.
    pub attenuation: [f32; 3],
}

impl Light {
    /// Light `i` as a new context has it (table 2.7): light 0 is white,
    /// the others give no diffuse or specular light.
    pub fn new(i: usize) -> Light {
        let lit = if i == 0 { 1.0 } else { 0.0 };
        Light {
            ambient: [0.0, 0.0, 0.0, 1.0],
            diffuse: [lit, lit, lit, 1.0],
            specular: [lit, lit, lit, 1.0],
            position: [0.0, 0.0, 1.0, 0.0],
            direction: [0.0, 0.0, -1.0],
            exponent: 0.0,
            cutoff: 180.0,
            attenuation: [1.0, 0.0, 0.0],
        }
    }
}

impl Default for Light {
    fn default() -> Light {
        Light::new(1)
    }
}

/// A material: one for front faces, one for back faces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    pub ambient: [f32; 4],
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    pub emission: [f32; 4],
    pub shininess: f32,
    /// The ambient, diffuse and specular color indexes, which only color
    /// index lighting uses: kept for glGetMaterial.
    pub indexes: [f32; 3],
}

impl Default for Material {
    fn default() -> Material {
        Material {
            ambient: [0.2, 0.2, 0.2, 1.0],
            diffuse: [0.8, 0.8, 0.8, 1.0],
            specular: [0.0, 0.0, 0.0, 1.0],
            emission: [0.0, 0.0, 0.0, 1.0],
            shininess: 0.0,
            indexes: [0.0, 1.0, 1.0],
        }
    }
}

/// All of a context's lighting state but the GL_LIGHTING and
/// GL_COLOR_MATERIAL enables, which are among its capabilities.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lighting {
    pub lights: [Light; LIGHTS],
    /// The lights that are on, a bit each (glEnable(GL_LIGHTi)).
    pub enabled: u8,
    /// The front material, and the back one.
    pub materials: [Material; 2],
    /// The scene's ambient color, and whether the viewer is local and
    /// lighting is two-sided.
    pub ambient: [f32; 4],
    pub local_viewer: bool,
    pub two_side: bool,
    /// glColorMaterial's face and mode.
    pub color_face: u32,
    pub color_mode: u32,
}

impl Default for Lighting {
    fn default() -> Lighting {
        Lighting {
            lights: std::array::from_fn(Light::new),
            enabled: 0,
            materials: [Material::default(); 2],
            ambient: [0.2, 0.2, 0.2, 1.0],
            local_viewer: false,
            two_side: false,
            color_face: GL_FRONT_AND_BACK,
            color_mode: GL_AMBIENT_AND_DIFFUSE,
        }
    }
}

/// The materials `face` names: the front's (0), the back's (1) or both.
fn faces(face: u32) -> Option<&'static [usize]> {
    match face {
        GL_FRONT => Some(&[0]),
        GL_BACK => Some(&[1]),
        GL_FRONT_AND_BACK => Some(&[0, 1]),
        _ => None,
    }
}

/// The material properties glColorMaterial's `mode` names.
fn tracked(mode: u32) -> &'static [u32] {
    match mode {
        GL_AMBIENT_AND_DIFFUSE => &[GL_AMBIENT, GL_DIFFUSE],
        GL_EMISSION => &[GL_EMISSION],
        GL_AMBIENT => &[GL_AMBIENT],
        GL_DIFFUSE => &[GL_DIFFUSE],
        GL_SPECULAR => &[GL_SPECULAR],
        _ => &[],
    }
}

impl Lighting {
    /// Whether the material property `p` of the material `side` (0 front,
    /// 1 back) follows the current color while GL_COLOR_MATERIAL is on.
    pub fn tracks(&self, side: usize, p: u32) -> bool {
        faces(self.color_face).is_some_and(|f| f.contains(&side))
            && tracked(self.color_mode).contains(&p)
    }

    /// The material `side` with the properties glColorMaterial tracks
    /// replaced by `color`.
    pub fn with_color(&self, side: usize, color: [f32; 4]) -> Material {
        let mut m = self.materials[side];
        for &p in tracked(self.color_mode) {
            if self.tracks(side, p) {
                set_color(&mut m, p, color);
            }
        }
        m
    }

    /// Put `color` in the properties glColorMaterial tracks, as glColor
    /// does while GL_COLOR_MATERIAL is on (section 2.13.3: the
    /// replacements are permanent).
    pub fn track(&mut self, color: [f32; 4]) {
        for side in 0..2 {
            self.materials[side] = self.with_color(side, color);
        }
    }
}

/// Set the color property `p` of `m`.
fn set_color(m: &mut Material, p: u32, c: [f32; 4]) {
    match p {
        GL_AMBIENT => m.ambient = c,
        GL_DIFFUSE => m.diffuse = c,
        GL_SPECULAR => m.specular = c,
        GL_EMISSION => m.emission = c,
        _ => {}
    }
}

/// How many values the light parameter `p` has, None for one there
/// isn't.
fn light_values(p: u32) -> Option<usize> {
    Some(match p {
        GL_AMBIENT | GL_DIFFUSE | GL_SPECULAR | GL_POSITION => 4,
        GL_SPOT_DIRECTION => 3,
        GL_SPOT_EXPONENT..=GL_QUADRATIC_ATTENUATION => 1,
        _ => return None,
    })
}

/// How many values the material parameter `p` has.
fn material_values(p: u32) -> Option<usize> {
    Some(match p {
        GL_AMBIENT | GL_DIFFUSE | GL_SPECULAR | GL_EMISSION | GL_AMBIENT_AND_DIFFUSE => 4,
        GL_SHININESS => 1,
        GL_COLOR_INDEXES => 3,
        _ => return None,
    })
}

/// How many values the lighting model parameter `p` has.
fn model_values(p: u32) -> Option<usize> {
    Some(match p {
        GL_LIGHT_MODEL_AMBIENT => 4,
        GL_LIGHT_MODEL_LOCAL_VIEWER | GL_LIGHT_MODEL_TWO_SIDE => 1,
        _ => return None,
    })
}

/// Whether `p` is a color, which integers give from -1 to 1.
fn is_color(p: u32) -> bool {
    matches!(
        p,
        GL_AMBIENT
            | GL_DIFFUSE
            | GL_SPECULAR
            | GL_EMISSION
            | GL_AMBIENT_AND_DIFFUSE
            | GL_LIGHT_MODEL_AMBIENT
    )
}

/// The values of a call: `n` of them at the address `at` for the vector
/// forms, or the one in `scalar`, as floats. Colors given as integers
/// are converted as table 2.6 converts signed integers.
fn read(cpu: &mut Cpu, v: Values, p: u32, n: usize) -> Option<Vec<f32>> {
    let color = is_color(p);
    match v {
        Values::Float(x) => Some(vec![x]),
        Values::Int(x) => Some(vec![x as f32]),
        Values::Floats(at) => super::read_floats(cpu, at, n),
        Values::Ints(at) => {
            if at == 0 || !cpu.bus.guest_probe(at, n * 4, false) {
                return None;
            }
            Some(
                (0..n as u32)
                    .map(|i| {
                        let x = cpu.bus.guest_read_32(at + 4 * i) as i32;
                        if color {
                            signed_unit(x as f64, 32)
                        } else {
                            x as f32
                        }
                    })
                    .collect(),
            )
        }
    }
}

/// Where a call's values are.
#[derive(Clone, Copy)]
enum Values {
    Float(f32),
    Int(i32),
    Floats(u32),
    Ints(u32),
}

impl Values {
    fn vector(self) -> bool {
        matches!(self, Values::Floats(_) | Values::Ints(_))
    }
}

/// Whether the current context is between glBegin and glEnd, where
/// glLight, glLightModel and glColorMaterial are errors.
fn inside_begin(cpu: &mut Cpu) -> bool {
    if ctx(cpu).is_some_and(|c| c.begin.is_some()) {
        error(cpu, GL_INVALID_OPERATION);
        return true;
    }
    false
}

/// glLight: set the parameter `p` of `light`.
fn light(cpu: &mut Cpu, light: u32, p: u32, v: Values) {
    if inside_begin(cpu) {
        return;
    }
    let (Some(i), Some(n)) = (index(light), light_values(p)) else {
        error(cpu, GL_INVALID_ENUM);
        return;
    };
    // The single-valued forms take only single-valued parameters.
    if n > 1 && !v.vector() {
        error(cpu, GL_INVALID_ENUM);
        return;
    }
    let Some(x) = read(cpu, v, p, n) else {
        return;
    };
    let bad = match p {
        GL_SPOT_EXPONENT => !(0.0..=128.0).contains(&x[0]),
        GL_SPOT_CUTOFF => !(0.0..=90.0).contains(&x[0]) && x[0] != 180.0,
        GL_CONSTANT_ATTENUATION | GL_LINEAR_ATTENUATION | GL_QUADRATIC_ATTENUATION => x[0] < 0.0,
        _ => false,
    };
    if bad {
        error(cpu, GL_INVALID_VALUE);
        return;
    }
    let Some(c) = ctx(cpu) else {
        return;
    };
    let modelview = *c.modelview.last().expect("a matrix stack is never empty");
    let l = &mut c.lighting.lights[i];
    let four = || [x[0], x[1], x[2], x[3]];
    match p {
        GL_AMBIENT => l.ambient = four(),
        GL_DIFFUSE => l.diffuse = four(),
        GL_SPECULAR => l.specular = four(),
        // Section 2.13.2: the position goes through the modelview matrix,
        // the spot direction through its upper left 3x3, when given.
        GL_POSITION => l.position = matrix::apply(&modelview, four()),
        GL_SPOT_DIRECTION => {
            let d = matrix::apply(&modelview, [x[0], x[1], x[2], 0.0]);
            l.direction = [d[0], d[1], d[2]];
        }
        GL_SPOT_EXPONENT => l.exponent = x[0],
        GL_SPOT_CUTOFF => l.cutoff = x[0],
        _ => l.attenuation[(p - GL_CONSTANT_ATTENUATION) as usize] = x[0],
    }
}

/// glLightf(light, pname, param).
pub fn lightf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    light(cpu, args[0], args[1], Values::Float(super::f(args, 2)));
    Some(0)
}

/// glLighti(light, pname, param).
pub fn lighti(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    light(cpu, args[0], args[1], Values::Int(args[2] as i32));
    Some(0)
}

/// glLightfv(light, pname, params).
pub fn lightfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    light(cpu, args[0], args[1], Values::Floats(args[2]));
    Some(0)
}

/// glLightiv(light, pname, params).
pub fn lightiv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    light(cpu, args[0], args[1], Values::Ints(args[2]));
    Some(0)
}

/// glLightModel: set the lighting model's parameter `p`.
fn light_model(cpu: &mut Cpu, p: u32, v: Values) {
    if inside_begin(cpu) {
        return;
    }
    let Some(n) = model_values(p) else {
        error(cpu, GL_INVALID_ENUM);
        return;
    };
    if n > 1 && !v.vector() {
        error(cpu, GL_INVALID_ENUM);
        return;
    }
    let Some(x) = read(cpu, v, p, n) else {
        return;
    };
    let Some(c) = ctx(cpu) else {
        return;
    };
    let l = &mut c.lighting;
    match p {
        GL_LIGHT_MODEL_AMBIENT => l.ambient = [x[0], x[1], x[2], x[3]],
        GL_LIGHT_MODEL_LOCAL_VIEWER => l.local_viewer = x[0] != 0.0,
        _ => l.two_side = x[0] != 0.0,
    }
}

/// glLightModelf(pname, param).
pub fn light_modelf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    light_model(cpu, args[0], Values::Float(super::f(args, 1)));
    Some(0)
}

/// glLightModeli(pname, param).
pub fn light_modeli(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    light_model(cpu, args[0], Values::Int(args[1] as i32));
    Some(0)
}

/// glLightModelfv(pname, params).
pub fn light_modelfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    light_model(cpu, args[0], Values::Floats(args[1]));
    Some(0)
}

/// glLightModeliv(pname, params).
pub fn light_modeliv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    light_model(cpu, args[0], Values::Ints(args[1]));
    Some(0)
}

/// glMaterial: set the parameter `p` of the materials `face` names. It
/// may come between glBegin and glEnd, and lights the vertices after
/// it. A property glColorMaterial tracks keeps the current color while
/// GL_COLOR_MATERIAL is on (figure 2.9).
fn material(cpu: &mut Cpu, face: u32, p: u32, v: Values) {
    let (Some(sides), Some(n)) = (faces(face), material_values(p)) else {
        error(cpu, GL_INVALID_ENUM);
        return;
    };
    if n > 1 && !v.vector() {
        error(cpu, GL_INVALID_ENUM);
        return;
    }
    let Some(x) = read(cpu, v, p, n) else {
        return;
    };
    if p == GL_SHININESS && !(0.0..=128.0).contains(&x[0]) {
        error(cpu, GL_INVALID_VALUE);
        return;
    }
    let Some(c) = ctx(cpu) else {
        return;
    };
    let tracking = c.caps & CAP_COLOR_MATERIAL != 0;
    let l = &mut c.lighting;
    for &side in sides {
        let props: &[u32] = match p {
            GL_AMBIENT_AND_DIFFUSE => &[GL_AMBIENT, GL_DIFFUSE],
            _ => std::slice::from_ref(&p),
        };
        for &prop in props {
            if tracking && l.tracks(side, prop) {
                continue;
            }
            let m = &mut l.materials[side];
            match prop {
                GL_SHININESS => m.shininess = x[0],
                GL_COLOR_INDEXES => m.indexes = [x[0], x[1], x[2]],
                _ => set_color(m, prop, [x[0], x[1], x[2], x[3]]),
            }
        }
    }
}

/// glMaterialf(face, pname, param).
pub fn materialf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    material(cpu, args[0], args[1], Values::Float(super::f(args, 2)));
    Some(0)
}

/// glMateriali(face, pname, param).
pub fn materiali(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    material(cpu, args[0], args[1], Values::Int(args[2] as i32));
    Some(0)
}

/// glMaterialfv(face, pname, params).
pub fn materialfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    material(cpu, args[0], args[1], Values::Floats(args[2]));
    Some(0)
}

/// glMaterialiv(face, pname, params).
pub fn materialiv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    material(cpu, args[0], args[1], Values::Ints(args[2]));
    Some(0)
}

/// glColorMaterial(face, mode). With GL_COLOR_MATERIAL on, the newly
/// tracked properties take the current color at once (section 2.13.3).
pub fn color_material(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (face, mode) = (args[0], args[1]);
    if faces(face).is_none() || tracked(mode).is_empty() {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if inside_begin(cpu) {
        return Some(0);
    }
    if let Some(c) = ctx(cpu) {
        c.lighting.color_face = face;
        c.lighting.color_mode = mode;
        if c.caps & CAP_COLOR_MATERIAL != 0 {
            c.lighting.track(c.color);
        }
    }
    Some(0)
}

/// The values glGetLight gives for `p` of light `i`.
fn light_get(l: &Light, p: u32) -> Option<Vec<f32>> {
    Some(match p {
        GL_AMBIENT => l.ambient.to_vec(),
        GL_DIFFUSE => l.diffuse.to_vec(),
        GL_SPECULAR => l.specular.to_vec(),
        GL_POSITION => l.position.to_vec(),
        GL_SPOT_DIRECTION => l.direction.to_vec(),
        GL_SPOT_EXPONENT => vec![l.exponent],
        GL_SPOT_CUTOFF => vec![l.cutoff],
        GL_CONSTANT_ATTENUATION..=GL_QUADRATIC_ATTENUATION => {
            vec![l.attenuation[(p - GL_CONSTANT_ATTENUATION) as usize]]
        }
        _ => return None,
    })
}

/// The values glGetMaterial gives for `p` of the material `side`.
fn material_get(m: &Material, p: u32) -> Option<Vec<f32>> {
    Some(match p {
        GL_AMBIENT => m.ambient.to_vec(),
        GL_DIFFUSE => m.diffuse.to_vec(),
        GL_SPECULAR => m.specular.to_vec(),
        GL_EMISSION => m.emission.to_vec(),
        GL_SHININESS => vec![m.shininess],
        GL_COLOR_INDEXES => m.indexes.to_vec(),
        _ => return None,
    })
}

/// Write `v` at `at` as floats, or as integers: colors from -1 to 1 to
/// the whole range of integers, other values rounded (section 6.1.2).
fn write(cpu: &mut Cpu, at: u32, v: &[f32], ints: bool, color: bool) {
    if at == 0 || !cpu.bus.guest_probe(at, v.len() * 4, true) {
        return;
    }
    for (i, &x) in v.iter().enumerate() {
        let bits = if !ints {
            x.to_bits()
        } else if color {
            (x.clamp(-1.0, 1.0) as f64 * i32::MAX as f64) as i32 as u32
        } else {
            x.round() as i32 as u32
        };
        cpu.bus.guest_write_32(at + 4 * i as u32, bits);
    }
}

/// glGetLightfv and glGetLightiv.
fn get_light(cpu: &mut Cpu, args: &[u32], ints: bool) {
    let (light, p, at) = (args[0], args[1], args[2]);
    let Some(c) = ctx(cpu) else {
        return;
    };
    let v = index(light).and_then(|i| light_get(&c.lighting.lights[i], p));
    match v {
        Some(v) => write(cpu, at, &v, ints, is_color(p)),
        None => error(cpu, GL_INVALID_ENUM),
    }
}

/// glGetLightfv(light, pname, params).
pub fn get_lightfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_light(cpu, args, false);
    Some(0)
}

/// glGetLightiv(light, pname, params).
pub fn get_lightiv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_light(cpu, args, true);
    Some(0)
}

/// glGetMaterialfv and glGetMaterialiv: of the front or the back
/// material, not both.
fn get_material(cpu: &mut Cpu, args: &[u32], ints: bool) {
    let (face, p, at) = (args[0], args[1], args[2]);
    let Some(c) = ctx(cpu) else {
        return;
    };
    let side = match face {
        GL_FRONT => Some(0),
        GL_BACK => Some(1),
        _ => None,
    };
    let v = side.and_then(|s| material_get(&c.lighting.materials[s], p));
    match v {
        Some(v) => write(cpu, at, &v, ints, is_color(p)),
        None => error(cpu, GL_INVALID_ENUM),
    }
}

/// glGetMaterialfv(face, pname, params).
pub fn get_materialfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_material(cpu, args, false);
    Some(0)
}

/// glGetMaterialiv(face, pname, params).
pub fn get_materialiv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_material(cpu, args, true);
    Some(0)
}

/// The values glGet gives for the lighting state `p`, and whether they
/// are colors.
pub fn get(c: &Context, p: u32) -> Option<(Vec<f64>, bool)> {
    let l = &c.lighting;
    let one = |x: f64| Some((vec![x], false));
    match p {
        GL_LIGHT_MODEL_AMBIENT => Some((l.ambient.map(|x| x as f64).to_vec(), true)),
        GL_LIGHT_MODEL_LOCAL_VIEWER => one(l.local_viewer as u32 as f64),
        GL_LIGHT_MODEL_TWO_SIDE => one(l.two_side as u32 as f64),
        GL_COLOR_MATERIAL_FACE => one(l.color_face as f64),
        GL_COLOR_MATERIAL_PARAMETER => one(l.color_mode as f64),
        _ => None,
    }
}

crate::state_fields!(Light {
    ambient,
    diffuse,
    specular,
    position,
    direction,
    exponent,
    cutoff,
    attenuation
});
crate::state_fields!(Material {
    ambient,
    diffuse,
    specular,
    emission,
    shininess,
    indexes
});
crate::state_fields!(Lighting {
    lights,
    enabled,
    materials,
    ambient,
    local_viewer,
    two_side,
    color_face,
    color_mode
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_material_tracks_the_faces_and_modes_it_names() {
        let mut l = Lighting::default();
        l.color_face = GL_BACK;
        l.color_mode = GL_EMISSION;
        l.track([0.5, 0.25, 0.0, 1.0]);
        assert_eq!(l.materials[0], Material::default());
        assert_eq!(l.materials[1].emission, [0.5, 0.25, 0.0, 1.0]);
        assert_eq!(l.materials[1].ambient, Material::default().ambient);

        let mut l = Lighting::default();
        l.track([0.1, 0.2, 0.3, 0.4]);
        for m in l.materials {
            assert_eq!(m.ambient, [0.1, 0.2, 0.3, 0.4]);
            assert_eq!(m.diffuse, [0.1, 0.2, 0.3, 0.4]);
            assert_eq!(m.specular, Material::default().specular);
        }
    }

    #[test]
    fn integer_colors_convert_as_signed_integers() {
        assert_eq!(signed_unit(i32::MAX as f64, 32), 1.0);
        assert_eq!(signed_unit(i32::MIN as f64, 32), -1.0);
        assert!(signed_unit(0.0, 32).abs() < 1e-9);
        // A normal's bytes and shorts as glNormal3b and glNormal3s give
        // them.
        assert_eq!(signed_unit(127.0, 8), 1.0);
        assert_eq!(signed_unit(-128.0, 8), -1.0);
        assert_eq!(signed_unit(32767.0, 16), 1.0);
    }

    #[test]
    fn lights_start_as_table_2_7_has_them() {
        let l = Lighting::default();
        assert_eq!(l.lights[0].diffuse, [1.0; 4]);
        assert_eq!(l.lights[0].specular, [1.0; 4]);
        assert_eq!(l.lights[3].diffuse, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(l.lights[3].position, [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(l.lights[3].direction, [0.0, 0.0, -1.0]);
        assert_eq!(l.lights[3].cutoff, 180.0);
    }
}

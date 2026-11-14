//! Lighting: a vertex's diffuse and specular colours from the material
//! and the lights that are on ("Lighting" in chapter 9 of
//! `docs/toadforce-spec.md`). It is worked out in camera space.

use super::geometry::{direction, dot, length, normalize, transform};
use super::math;
use super::vertex::Fvf;
use crate::toadforce::level1::states::*;
use crate::toadforce::level1::{
    LIGHT_DIRECTIONAL, LIGHT_SPOT, LIGHTS, Level1, Light, Material, TRANSFORM_VIEW,
};

/// A light that is on, in camera space.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Lamp {
    light: Light,
    position: [f32; 3],
    /// The way a directional light or a spot points, of length 1.
    axis: [f32; 3],
    /// The cosines of half a spot's inner and outer angles.
    inner: f32,
    outer: f32,
}

/// Where a colour of the lit surface comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Material,
    Diffuse,
    Specular,
}

/// The lighting of a draw's state.
#[derive(Clone, Debug, PartialEq)]
pub struct Lighting {
    lamps: Vec<Lamp>,
    /// The material of the front, and of the back.
    materials: [Material; 2],
    /// The ambient light, red first.
    ambient: [f32; 3],
    specular: bool,
    /// `SINGLE_COLOUR`: the specular light goes into the diffuse
    /// colour.
    single: bool,
    local_viewer: bool,
    /// The sources of the diffuse, specular, ambient and emissive
    /// colours of the front, and of the back: those of a side
    /// `COLORVERTEX_FACES` leaves out are the material's.
    sources: [[Source; 4]; 2],
}

impl Lighting {
    /// The lighting of the state `l` for vertices laid out as `fvf`.
    pub fn new(l: &Level1, fvf: &Fvf) -> Lighting {
        let view = &l.transforms[TRANSFORM_VIEW as usize];
        let lamps = (0..LIGHTS)
            .filter(|i| l.lights_on >> i & 1 != 0)
            .map(|i| {
                let light = l.lights[i];
                let p = light.position;
                let position = transform([p[0], p[1], p[2], 1.0], view);
                Lamp {
                    light,
                    position: [position[0], position[1], position[2]],
                    axis: normalize(direction(light.direction, view)),
                    inner: math::cos(light.theta / 2.0),
                    outer: math::cos(light.phi / 2.0),
                }
            })
            .collect();
        // A colour the vertex hasn't got is the material's.
        let source = |id: u32| match l.state(id) {
            1 if l.state(RS_COLORVERTEX) != 0 && fvf.diffuse.is_some() => Source::Diffuse,
            2 if l.state(RS_COLORVERTEX) != 0 && fvf.specular.is_some() => Source::Specular,
            _ => Source::Material,
        };
        let sources = [
            source(RS_DIFFUSEMATERIALSOURCE),
            source(RS_SPECULARMATERIALSOURCE),
            source(RS_AMBIENTMATERIALSOURCE),
            source(RS_EMISSIVEMATERIALSOURCE),
        ];
        let faces = l.state(RS_COLORVERTEX_FACES);
        let side = |bit: u32| {
            if faces & bit != 0 {
                sources
            } else {
                [Source::Material; 4]
            }
        };
        let single = l.state(RS_SINGLE_COLOUR) != 0;
        Lighting {
            lamps,
            materials: [l.material, l.material_back],
            ambient: l.ambient,
            specular: single || l.state(RS_SPECULARENABLE) != 0,
            single,
            local_viewer: l.state(RS_LOCALVIEWER) != 0,
            sources: [side(1), side(2)],
        }
    }

    /// The diffuse and specular colours of a vertex at `eye` with the
    /// normal `n`, both in camera space, whose own colours are
    /// `diffuse` and `specular`. The specular colour's alpha is 1, for
    /// no fog. With `back` they are those of a triangle's back: `n` is
    /// then the normal turned round already.
    pub fn light(
        &self,
        eye: [f32; 3],
        n: [f32; 3],
        diffuse: Option<[f32; 4]>,
        specular: Option<[f32; 4]>,
        back: bool,
    ) -> ([f32; 4], [f32; 4]) {
        let sources = &self.sources[back as usize];
        let of = |which: usize, material: [f32; 4]| match sources[which] {
            Source::Diffuse => diffuse.unwrap_or(material),
            Source::Specular => specular.unwrap_or(material),
            Source::Material => material,
        };
        let m = &self.materials[back as usize];
        let m_diffuse = of(0, m.diffuse);
        let m_specular = of(1, m.specular);
        let m_ambient = of(2, m.ambient);
        let m_emissive = of(3, m.emissive);
        let mut ambient = self.ambient;
        let mut lit = [0.0f32; 3];
        let mut shine = [0.0f32; 3];
        // The way to the eye, which is at the origin, or straight
        // back for an eye infinitely far away.
        let to_eye = if self.local_viewer {
            normalize(eye.map(|c| -c))
        } else {
            [0.0, 0.0, -1.0]
        };
        for lamp in &self.lamps {
            let light = &lamp.light;
            // The way to the light, and what the distance leaves of
            // it.
            let (to, reach) = if light.kind == LIGHT_DIRECTIONAL {
                (lamp.axis.map(|c| -c), 1.0)
            } else {
                let to: [f32; 3] = std::array::from_fn(|i| lamp.position[i] - eye[i]);
                let d = length(to);
                if d > light.range {
                    continue;
                }
                let a = &light.attenuation;
                let under = a[0] + a[1] * d + a[2] * d * d;
                (normalize(to), if under > 0.0 { 1.0 / under } else { 1.0 })
            };
            let cone = if light.kind == LIGHT_SPOT {
                let rho = -dot(to, lamp.axis);
                if rho <= lamp.outer {
                    0.0
                } else if light.spot_model == 1 {
                    math::pow_unit(rho, light.falloff)
                } else if rho > lamp.inner || lamp.inner <= lamp.outer {
                    1.0
                } else {
                    let share = (rho - lamp.outer) / (lamp.inner - lamp.outer);
                    math::pow_unit(share, light.falloff)
                }
            } else {
                1.0
            };
            let factor = reach * cone;
            if factor.is_nan() || factor <= 0.0 || factor.is_infinite() {
                continue;
            }
            for (a, l) in ambient.iter_mut().zip(light.ambient) {
                *a += l * factor;
            }
            let facing = dot(n, to);
            if facing.is_nan() || facing <= 0.0 {
                continue;
            }
            for (d, l) in lit.iter_mut().zip(light.diffuse) {
                *d += l * facing * factor;
            }
            if self.specular {
                let half: [f32; 3] = std::array::from_fn(|i| to[i] + to_eye[i]);
                let glint = dot(n, normalize(half));
                if glint > 0.0 {
                    let p = math::pow_unit(glint, m.power);
                    for (s, l) in shine.iter_mut().zip(light.specular) {
                        *s += l * p * factor;
                    }
                }
            }
        }
        let mut out_d = [0.0; 4];
        let mut out_s = [0.0, 0.0, 0.0, 1.0];
        for c in 0..3 {
            out_d[c] = m_emissive[c] + m_ambient[c] * ambient[c] + m_diffuse[c] * lit[c];
            if self.single {
                out_d[c] += m_specular[c] * shine[c];
            } else {
                out_s[c] = m_specular[c] * shine[c];
            }
        }
        out_d[3] = m_diffuse[3];
        (
            out_d.map(|c| c.clamp(0.0, 1.0)),
            out_s.map(|c| c.clamp(0.0, 1.0)),
        )
    }
}

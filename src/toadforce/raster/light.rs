//! Lighting: a vertex's diffuse and specular colours from the material
//! and the lights that are on ("Lighting" in chapter 9 of
//! `docs/toadforce-spec.md`). It is worked out in camera space.

use super::geometry::{Matrix, direction, dot, length, normalize, transform};
use super::math;
use super::vertex::Fvf;
use crate::toadforce::level1::states::*;
use crate::toadforce::level1::{
    LIGHT_DIRECTIONAL, LIGHT_SPOT, LIGHTS, Level1, Light, Material, TRANSFORM_VIEW,
};

/// A light that is on, in camera space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lamp {
    light: Light,
    position: [f32; 3],
    /// The way a directional light or a spot points, of length 1, or
    /// zeros for a light that points no way.
    axis: [f32; 3],
    /// The cosines of half a spot's inner and outer angles.
    inner: f32,
    outer: f32,
}

impl Lamp {
    /// `light`, which is in world space, as `view` puts it in camera
    /// space.
    pub fn new(light: &Light, view: &Matrix) -> Lamp {
        let p = light.position;
        let position = transform([p[0], p[1], p[2], 1.0], view);
        Lamp {
            light: *light,
            position: [position[0], position[1], position[2]],
            axis: pointing(direction(light.direction, view)),
            inner: math::cos(light.theta / 2.0),
            outer: math::cos(light.phi / 2.0),
        }
    }

    /// `light` as a directional one that shines from the way `toward`
    /// in camera space, of length 1: Direct3D's parallel point light,
    /// which the card has no kind for and its own commands never make.
    pub fn shining_from(light: &Light, toward: [f32; 3]) -> Lamp {
        Lamp {
            light: Light {
                kind: LIGHT_DIRECTIONAL,
                ..*light
            },
            position: [0.0; 3],
            axis: toward.map(|c| -c),
            inner: 1.0,
            outer: 1.0,
        }
    }
}

/// `way` with a length of 1, or zeros where it has none a float can
/// divide by: Direct3D takes a direction of 1e-25 for none.
fn pointing(way: [f32; 3]) -> [f32; 3] {
    let len = length(way);
    if len > 0.0 && len.is_finite() {
        way.map(|c| c / len)
    } else {
        [0.0; 3]
    }
}

/// Where a colour of the lit surface comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Material,
    Diffuse,
    Specular,
}

impl Source {
    /// From a material source state's value (`D3DMCS_`), for a vertex
    /// whose colours stand for the material's (`COLORVERTEX`) where it
    /// has them: `diffuse` and `specular` say which it has.
    pub fn of(value: u32, diffuse: bool, specular: bool) -> Source {
        match value {
            1 if diffuse => Source::Diffuse,
            2 if specular => Source::Specular,
            _ => Source::Material,
        }
    }
}

/// The lighting of a draw's state.
#[derive(Clone, Debug, PartialEq)]
pub struct Lighting {
    lamps: Vec<Lamp>,
    material: Material,
    /// `AMBIENT`, red first.
    ambient: [f32; 3],
    specular: bool,
    local_viewer: bool,
    /// The sources of the diffuse, specular, ambient and emissive
    /// colours.
    sources: [Source; 4],
}

impl Lighting {
    /// The lighting of the state `l` for vertices laid out as `fvf`.
    pub fn new(l: &Level1, fvf: &Fvf) -> Lighting {
        let view = &l.transforms[TRANSFORM_VIEW as usize];
        let lamps = (0..LIGHTS)
            .filter(|i| l.lights_on >> i & 1 != 0)
            .map(|i| Lamp::new(&l.lights[i], view))
            .collect();
        // A colour the vertex hasn't got is the material's.
        let own = l.state(RS_COLORVERTEX) != 0;
        let (diffuse, specular) = (own && fvf.diffuse.is_some(), own && fvf.specular.is_some());
        let source = |id: u32| Source::of(l.state(id), diffuse, specular);
        let ambient = super::geometry::colour(l.state(RS_AMBIENT));
        Lighting {
            lamps,
            material: l.material,
            ambient: [ambient[0], ambient[1], ambient[2]],
            specular: l.state(RS_SPECULARENABLE) != 0,
            local_viewer: l.state(RS_LOCALVIEWER) != 0,
            sources: [
                source(RS_DIFFUSEMATERIALSOURCE),
                source(RS_SPECULARMATERIALSOURCE),
                source(RS_AMBIENTMATERIALSOURCE),
                source(RS_EMISSIVEMATERIALSOURCE),
            ],
        }
    }

    /// The lighting of `lamps` on a surface of `material` under the
    /// ambient light `ambient` (red, green, blue), for a renderer that
    /// has those from somewhere else than the card's state: Direct3D's
    /// pipeline, which lights the vertices it keeps with the card's
    /// arithmetic, so a device draws one picture whichever of the two
    /// lit a vertex. `sources` are those of the diffuse, specular,
    /// ambient and emissive colours.
    pub fn of(
        lamps: Vec<Lamp>,
        material: Material,
        ambient: [f32; 3],
        specular: bool,
        local_viewer: bool,
        sources: [Source; 4],
    ) -> Lighting {
        Lighting {
            lamps,
            material,
            ambient,
            specular,
            local_viewer,
            sources,
        }
    }

    /// The diffuse and specular colours of a vertex at `eye` with the
    /// normal `n`, both in camera space, whose own colours are
    /// `diffuse` and `specular`. The specular colour's alpha is 1, for
    /// no fog.
    pub fn light(
        &self,
        eye: [f32; 3],
        n: [f32; 3],
        diffuse: Option<[f32; 4]>,
        specular: Option<[f32; 4]>,
    ) -> ([f32; 4], [f32; 4]) {
        let of = |which: usize, material: [f32; 4]| match self.sources[which] {
            Source::Diffuse => diffuse.unwrap_or(material),
            Source::Specular => specular.unwrap_or(material),
            Source::Material => material,
        };
        let m = &self.material;
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
                // A light reaches what is nearer than its range. A
                // distance or a sum that is no number makes a factor
                // that is none, which the test below leaves out.
                if d >= light.range {
                    continue;
                }
                let a = &light.attenuation;
                let under = a[0] + a[1] * d + a[2] * d * d;
                if under <= 0.0 {
                    continue;
                }
                (normalize(to), 1.0 / under)
            };
            let cone = if light.kind == LIGHT_SPOT {
                let rho = -dot(to, lamp.axis);
                // A spot that points no way has a rho of 0, which is
                // outside any cone up to half a turn wide.
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
            // A falloff below 0 can make the cone's share any size:
            // what the light adds stays a number.
            let factor = reach * cone;
            if factor.is_nan() || factor <= 0.0 {
                continue;
            }
            let factor = factor.min(math::POW_MAX);
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
            out_s[c] = m_specular[c] * shine[c];
        }
        out_d[3] = m_diffuse[3];
        (
            out_d.map(|c| c.clamp(0.0, 1.0)),
            out_s.map(|c| c.clamp(0.0, 1.0)),
        )
    }
}

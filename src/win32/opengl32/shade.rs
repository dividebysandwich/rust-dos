//! OpenGL 1.1's lighting equation (section 2.13.1), evaluated on the
//! processor as the Voodoo3's OpenGL driver did: a function of the
//! lighting state (`light.rs`) and one vertex in eye coordinates, which
//! gives the vertex's front color and its back color.
//!
//! Direct3D's lighting (`ddraw::d3d::pipeline`) isn't used for it: its
//! spot light has an inner and an outer cone where OpenGL's has a cosine
//! to a power, it has no two-sided lighting, and it adds the specular
//! color after texturing where OpenGL adds it in the vertex's color.
//!
//! Where the specification leaves something open, this does what Mesa
//! does (src/mesa/main/light.c, src/mesa/tnl/t_vb_lighttmp.h, under the
//! MIT license: Copyright (C) 1999-2008 Brian Paul): a modelview matrix
//! whose upper left 3x3 has no inverse leaves normals as they are.

use super::light::{Lighting, Material};

/// A vertex as lighting takes it: its position and normal in eye
/// coordinates, the normal normalized if GL_NORMALIZE is on, and the
/// color glColorMaterial gives the tracked material properties.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Input {
    pub position: [f32; 4],
    pub normal: [f32; 3],
    /// The current color, with GL_COLOR_MATERIAL on.
    pub color: Option<[f32; 4]>,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

/// `a` as a unit vector; the zero vector stays as it is.
fn unit(a: [f32; 3]) -> [f32; 3] {
    let l = length(a);
    if l > 0.0 { scale(a, 1.0 / l) } else { a }
}

/// The point (x, y, z) of homogeneous coordinates with w not zero.
fn point(p: [f32; 4]) -> [f32; 3] {
    [p[0] / p[3], p[1] / p[3], p[2] / p[3]]
}

fn xyz(p: [f32; 4]) -> [f32; 3] {
    [p[0], p[1], p[2]]
}

/// The vector from p1 to p2, before it is made a unit vector, as
/// section 2.13.1 defines it for points that may be at infinity (w 0):
/// to a point at infinity, its direction; from one, the opposite of its
/// direction; between two, the difference.
fn toward(p1: [f32; 4], p2: [f32; 4]) -> [f32; 3] {
    match (p1[3] != 0.0, p2[3] != 0.0) {
        (true, true) => sub(point(p2), point(p1)),
        (true, false) => xyz(p2),
        (false, true) => scale(xyz(p1), -1.0),
        (false, false) => sub(xyz(p2), xyz(p1)),
    }
}

/// d1 . d2 clamped to 0 below (the specification's circled dot).
fn clamped(a: [f32; 3], b: [f32; 3]) -> f32 {
    dot(a, b).max(0.0)
}

/// The eye point, (0, 0, 0, 1) in eye coordinates.
const EYE: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// The color equation 2.1 gives one side: with the material `m` and the
/// normal `n` (the back side's reversed).
fn side(l: &Lighting, m: &Material, v: [f32; 4], n: [f32; 3]) -> [f32; 4] {
    // e_cm + a_cm * a_cs.
    let mut c: [f32; 3] = std::array::from_fn(|k| m.emission[k] + m.ambient[k] * l.ambient[k]);
    for (i, light) in l.lights.iter().enumerate() {
        if l.enabled & (1 << i) == 0 {
            continue;
        }
        let p = light.position;
        let to_light = toward(v, p);
        let vp = unit(to_light);
        // Equation 2.4: only a light at a point is attenuated, by its
        // distance from the vertex.
        let att = if p[3] != 0.0 {
            let d = length(to_light);
            let [k0, k1, k2] = light.attenuation;
            1.0 / (k0 + k1 * d + k2 * d * d)
        } else {
            1.0
        };
        // Equation 2.5: a spot lights only inside its cone, more toward
        // its middle as its exponent grows. The cone is tested with the
        // cosine before it is clamped: clamped, a vertex behind a spot of
        // 90 degrees would pass, as cos(90) rounds below 0, and take
        // 0^0 = 1 with an exponent of 0.
        let spot = if light.cutoff != 180.0 {
            let cos = dot(scale(vp, -1.0), unit(light.direction));
            if cos >= light.cutoff.to_radians().cos() {
                cos.max(0.0).powf(light.exponent)
            } else {
                0.0
            }
        } else {
            1.0
        };
        let factor = att * spot;
        if factor == 0.0 {
            continue;
        }
        let n_vp = clamped(n, vp);
        // Equation 2.3: the half vector, toward the eye at the origin for
        // a local viewer, along z for one at infinity.
        let toward_eye = if l.local_viewer {
            unit(toward(v, EYE))
        } else {
            [0.0, 0.0, 1.0]
        };
        let h = unit(add(vp, toward_eye));
        // Equation 2.2: no specular light where the light doesn't reach
        // the surface.
        let specular = if n_vp != 0.0 {
            clamped(n, h).powf(m.shininess)
        } else {
            0.0
        };
        for (k, c) in c.iter_mut().enumerate() {
            *c += factor
                * (m.ambient[k] * light.ambient[k]
                    + n_vp * m.diffuse[k] * light.diffuse[k]
                    + specular * m.specular[k] * light.specular[k]);
        }
    }
    // The alpha is the diffuse material's; section 2.13.6 clamps all
    // four to 0 to 1.
    [c[0], c[1], c[2], m.diffuse[3]].map(|x| x.clamp(0.0, 1.0))
}

/// The front and back colors of the vertex `v`. Without two-sided
/// lighting they are the same: the front material's, with the normal as
/// it is.
pub fn shade(l: &Lighting, v: &Input) -> [[f32; 4]; 2] {
    let material = |s: usize| match v.color {
        Some(c) => l.with_color(s, c),
        None => l.materials[s],
    };
    let front = side(l, &material(0), v.position, v.normal);
    let back = if l.two_side {
        side(l, &material(1), v.position, scale(v.normal, -1.0))
    } else {
        front
    };
    [front, back]
}

/// The matrix normals go through: the inverse of the upper left 3x3 of
/// the modelview matrix `m` (column-major), which a row vector
/// multiplies on its left (section 2.10.3), as the 3x3 that multiplies
/// a column vector on its right, column-major. None when there is no
/// inverse.
pub fn normal_matrix(m: &[f32; 16]) -> Option<[f32; 9]> {
    // a[row][col] of the upper left 3x3.
    let a = |r: usize, c: usize| m[c * 4 + r] as f64;
    let cof = |r: usize, c: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (c1, c2) = ((c + 1) % 3, (c + 2) % 3);
        a(r1, c1) * a(r2, c2) - a(r1, c2) * a(r2, c1)
    };
    let det = a(0, 0) * cof(0, 0) + a(0, 1) * cof(0, 1) + a(0, 2) * cof(0, 2);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    // inverse[r][c] = cof(c, r) / det; n' = n M^-1, so n'_c = sum over
    // r of n_r inverse[r][c], and the column-major matrix that takes n
    // to n' has at [c][r] (row c, column r) inverse[r][c]: cof(c, r) /
    // det at column r, row c.
    let mut out = [0.0f32; 9];
    for r in 0..3 {
        for c in 0..3 {
            out[r * 3 + c] = (cof(c, r) / det) as f32;
        }
    }
    Some(out)
}

/// The normal `n` in eye coordinates, through `nm` (`normal_matrix`'s,
/// or as it is without one), and made a unit vector when `normalize`.
pub fn eye_normal(nm: Option<&[f32; 9]>, n: [f32; 3], normalize: bool) -> [f32; 3] {
    let e = match nm {
        Some(m) => std::array::from_fn(|row| (0..3).map(|k| m[k * 3 + row] * n[k]).sum()),
        None => n,
    };
    if normalize { unit(e) } else { e }
}

#[cfg(test)]
mod tests {
    use super::super::light::{GL_AMBIENT_AND_DIFFUSE, GL_EMISSION, Light, Lighting, Material};
    use super::super::state::{GL_BACK, GL_FRONT};
    use super::*;

    fn close(a: [f32; 4], b: [f32; 4]) {
        for k in 0..4 {
            assert!((a[k] - b[k]).abs() < 1e-5, "{a:?} is not {b:?}");
        }
    }

    /// A scene with no ambient light, no emission and a white light 0
    /// that has no specular light: only the diffuse term.
    fn diffuse_only() -> Lighting {
        let mut l = Lighting {
            enabled: 1,
            ambient: [0.0, 0.0, 0.0, 1.0],
            ..Lighting::default()
        };
        l.lights[0].specular = [0.0; 4];
        l
    }

    fn input(position: [f32; 4], normal: [f32; 3]) -> Input {
        Input {
            position,
            normal,
            color: None,
        }
    }

    #[test]
    fn a_directional_light_gives_the_cosine_times_the_diffuse_colors() {
        let mut l = diffuse_only();
        l.materials[0].diffuse = [0.5, 1.0, 0.25, 0.75];
        // A light along (0, 0.6, 0.8), on a normal along z: n.L = 0.8.
        l.lights[0].position = [0.0, 0.6, 0.8, 0.0];
        let [front, back] = shade(&l, &input([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0]));
        close(front, [0.4, 0.8, 0.2, 0.75]);
        assert_eq!(front, back);
        // From behind the surface: nothing.
        l.lights[0].position = [0.0, 0.0, -1.0, 0.0];
        let [front, _] = shade(&l, &input([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0]));
        close(front, [0.0, 0.0, 0.0, 0.75]);
    }

    #[test]
    fn a_directional_light_isnt_attenuated() {
        let mut l = diffuse_only();
        l.lights[0].attenuation = [2.0, 1.0, 1.0];
        let [front, _] = shade(&l, &input([0.0, 0.0, -100.0, 1.0], [0.0, 0.0, 1.0]));
        close(front, [0.8, 0.8, 0.8, 1.0]);
    }

    #[test]
    fn a_light_at_a_point_is_attenuated_by_each_factor() {
        // The light 2 units above the vertex, straight along its normal.
        let at = |k: [f32; 3]| {
            let mut l = diffuse_only();
            l.materials[0].diffuse = [1.0; 4];
            l.lights[0].position = [0.0, 0.0, 2.0, 1.0];
            l.lights[0].attenuation = k;
            shade(&l, &input([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0]))[0]
        };
        close(at([1.0, 0.0, 0.0]), [1.0, 1.0, 1.0, 1.0]);
        // 1 / 2.
        close(at([2.0, 0.0, 0.0]), [0.5, 0.5, 0.5, 1.0]);
        // 1 / (k1 d) = 1 / 2.
        close(at([0.0, 1.0, 0.0]), [0.5, 0.5, 0.5, 1.0]);
        // 1 / (k2 d^2) = 1 / 4.
        close(at([0.0, 0.0, 1.0]), [0.25, 0.25, 0.25, 1.0]);
        // 1 / (1 + 2 + 4).
        close(at([1.0, 1.0, 1.0]), [1.0 / 7.0, 1.0 / 7.0, 1.0 / 7.0, 1.0]);
        // A light at a point given with w 2 is at (0, 0, 2) as well.
        let mut l = diffuse_only();
        l.materials[0].diffuse = [1.0; 4];
        l.lights[0].position = [0.0, 0.0, 4.0, 2.0];
        l.lights[0].attenuation = [0.0, 0.0, 1.0];
        let c = shade(&l, &input([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0]))[0];
        close(c, [0.25, 0.25, 0.25, 1.0]);
    }

    #[test]
    fn a_spot_lights_inside_its_cone_with_its_exponent() {
        // A spot at (0, 0, 1) pointing down -z, at a vertex on the plane
        // z = 0 at (x, 0, 0): the angle from the spot's axis has cos =
        // 1 / sqrt(1 + x^2), and n.L is the same.
        let at = |x: f32, cutoff: f32, exponent: f32| {
            let mut l = diffuse_only();
            l.materials[0].diffuse = [1.0; 4];
            let s = &mut l.lights[0];
            s.position = [0.0, 0.0, 1.0, 1.0];
            s.direction = [0.0, 0.0, -2.0];
            s.cutoff = cutoff;
            s.exponent = exponent;
            shade(&l, &input([x, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0]))[0][0]
        };
        let cos = 1.0 / 2f32.sqrt();
        // At 45 degrees: inside a cone of 50, outside one of 40.
        assert!((at(1.0, 50.0, 0.0) - cos).abs() < 1e-5);
        assert_eq!(at(1.0, 40.0, 0.0), 0.0);
        // The exponent: cos^2 for the spot, times cos for n.L.
        assert!((at(1.0, 50.0, 2.0) - cos * cos * cos).abs() < 1e-5);
        // 180 is no spot: n.L alone, whatever the exponent.
        assert!((at(1.0, 180.0, 64.0) - cos).abs() < 1e-5);
        // On the axis, the spot's factor is 1.
        assert!((at(0.0, 10.0, 128.0) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a_spot_of_90_degrees_leaves_what_is_behind_it_dark() {
        // A spot at the origin pointing down -z, of 90 degrees and
        // exponent 0, and a vertex behind it at (0, 0, 1) whose normal
        // faces the spot: outside the cone, so no light at all.
        let mut l = diffuse_only();
        l.materials[0].diffuse = [1.0; 4];
        let s = &mut l.lights[0];
        s.position = [0.0, 0.0, 0.0, 1.0];
        s.direction = [0.0, 0.0, -1.0];
        s.cutoff = 90.0;
        s.exponent = 0.0;
        let c = shade(&l, &input([0.0, 0.0, 1.0, 1.0], [0.0, 0.0, -1.0]))[0];
        close(c, [0.0, 0.0, 0.0, 1.0]);
        // In front of it, at (0, 0, -1), the vertex is lit.
        let c = shade(&l, &input([0.0, 0.0, -1.0, 1.0], [0.0, 0.0, 1.0]))[0];
        close(c, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn a_directional_spot_takes_the_light_direction_for_its_angle() {
        // Equation 2.5 has no exception for a light at infinity: the
        // vector from the light to the vertex is its direction reversed.
        let mut l = diffuse_only();
        l.lights[0].position = [0.0, 0.0, 1.0, 0.0];
        l.lights[0].cutoff = 30.0;
        l.lights[0].direction = [0.0, 1.0, 0.0];
        let c = shade(&l, &input([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0]))[0];
        close(c, [0.0, 0.0, 0.0, 1.0]);
        l.lights[0].direction = [0.0, 0.0, -1.0];
        let c = shade(&l, &input([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0]))[0];
        close(c, [0.8, 0.8, 0.8, 1.0]);
    }

    /// A scene with only light 0's specular light, and the material's
    /// specular color white with the shininess `s`.
    fn specular_only(s: f32) -> Lighting {
        let mut l = Lighting {
            enabled: 1,
            ambient: [0.0, 0.0, 0.0, 1.0],
            ..Lighting::default()
        };
        l.lights[0].diffuse = [0.0; 4];
        l.materials[0].specular = [1.0; 4];
        l.materials[0].shininess = s;
        l
    }

    #[test]
    fn specular_light_takes_the_half_vector_of_an_infinite_viewer() {
        // A light along (0, 1, 1) / sqrt 2: the half vector with (0, 0, 1)
        // is at 22.5 degrees from z, and n along z gives cos(22.5)^s.
        let mut l = specular_only(4.0);
        l.lights[0].position = [0.0, 1.0, 1.0, 0.0];
        let c = shade(&l, &input([0.0, 0.0, -10.0, 1.0], [0.0, 0.0, 1.0]))[0];
        let want = 22.5f32.to_radians().cos().powi(4);
        close(c, [want, want, want, 1.0]);
        // Where the vertex is doesn't matter to an infinite viewer.
        let c = shade(&l, &input([5.0, -3.0, -10.0, 1.0], [0.0, 0.0, 1.0]))[0];
        close(c, [want, want, want, 1.0]);
    }

    #[test]
    fn specular_light_takes_the_half_vector_of_a_local_viewer() {
        // The vertex at (0, 0, -1), the light along x at infinity: toward
        // the eye is +z, so the half vector is (1, 0, 1) / sqrt 2. A
        // normal of (1, 0, 1) / sqrt 2 is on it: the full specular color.
        let mut l = specular_only(10.0);
        l.local_viewer = true;
        l.lights[0].position = [1.0, 0.0, 0.0, 0.0];
        let n = [0.5f32.sqrt(), 0.0, 0.5f32.sqrt()];
        let c = shade(&l, &input([0.0, 0.0, -1.0, 1.0], n))[0];
        close(c, [1.0, 1.0, 1.0, 1.0]);
        // Moved to (1, 0, -1), toward the eye is (-1, 0, 1) / sqrt 2, and
        // the half vector (1 - 1/sqrt2, 0, 1/sqrt2) normalized.
        let c = shade(&l, &input([1.0, 0.0, -1.0, 1.0], n))[0];
        let r = 0.5f32.sqrt();
        let h = {
            let v = [1.0 - r, 0.0, r];
            let len = (v[0] * v[0] + v[2] * v[2]).sqrt();
            [v[0] / len, 0.0, v[2] / len]
        };
        let want = (n[0] * h[0] + n[2] * h[2]).powf(10.0);
        close(c, [want, want, want, 1.0]);
        // The infinite viewer gives (1, 0, 1) / sqrt 2 there too.
        l.local_viewer = false;
        let c = shade(&l, &input([1.0, 0.0, -1.0, 1.0], n))[0];
        close(c, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn no_specular_light_where_the_light_doesnt_reach() {
        // A light just behind the surface: n.L < 0, so f is 0, though the
        // half vector is near the normal.
        let mut l = specular_only(1.0);
        l.lights[0].position = [0.0, 1.0, -0.01, 0.0];
        let c = shade(&l, &input([0.0, 0.0, -1.0, 1.0], [0.0, 0.0, 1.0]))[0];
        close(c, [0.0, 0.0, 0.0, 1.0]);
        // Just in front of it, there is.
        l.lights[0].position = [0.0, 1.0, 0.01, 0.0];
        let c = shade(&l, &input([0.0, 0.0, -1.0, 1.0], [0.0, 0.0, 1.0]))[0];
        assert!(c[0] > 0.7);
    }

    #[test]
    fn emission_and_the_scene_ambient_add_without_lights() {
        let mut l = Lighting {
            ambient: [0.5, 0.25, 1.0, 1.0],
            ..Lighting::default()
        };
        l.materials[0].emission = [0.1, 0.2, 0.3, 1.0];
        l.materials[0].ambient = [0.2, 0.4, 0.6, 1.0];
        l.materials[0].diffuse[3] = 0.5;
        let c = shade(&l, &input([0.0, 0.0, -1.0, 1.0], [0.0, 0.0, 1.0]))[0];
        close(c, [0.1 + 0.1, 0.2 + 0.1, 0.3 + 0.6, 0.5]);
        // A light's ambient color adds, attenuated, wherever it is.
        l.enabled = 2;
        l.lights[1].ambient = [0.5, 0.5, 0.5, 1.0];
        l.lights[1].position = [0.0, 0.0, -10.0, 1.0];
        l.lights[1].attenuation = [2.0, 0.0, 0.0];
        let c = shade(&l, &input([0.0, 0.0, -1.0, 1.0], [0.0, 0.0, 1.0]))[0];
        close(c, [0.25, 0.4, 1.0, 0.5]);
    }

    #[test]
    fn colors_are_clamped() {
        let mut l = Lighting::default();
        l.materials[0].emission = [2.0, -1.0, 0.5, 1.0];
        l.materials[0].diffuse[3] = 3.0;
        l.ambient = [0.0; 4];
        let c = shade(&l, &input([0.0, 0.0, -1.0, 1.0], [0.0, 0.0, 1.0]))[0];
        close(c, [1.0, 0.0, 0.5, 1.0]);
    }

    #[test]
    fn color_material_replaces_the_properties_of_its_mode() {
        let color = [0.5, 0.25, 1.0, 0.5];
        let v = Input {
            color: Some(color),
            ..input([0.0, 0.0, -1.0, 1.0], [0.0, 0.0, 1.0])
        };
        // Emission only: the color itself, with the diffuse material's
        // alpha.
        let mut l = Lighting {
            ambient: [0.0; 4],
            color_mode: GL_EMISSION,
            ..Lighting::default()
        };
        close(shade(&l, &v)[0], [0.5, 0.25, 1.0, 1.0]);
        // Ambient and diffuse: the scene's ambient times it, and the
        // diffuse light times it, with its alpha.
        l.color_mode = GL_AMBIENT_AND_DIFFUSE;
        l.ambient = [0.2, 0.2, 0.2, 1.0];
        l.enabled = 1;
        l.lights[0].specular = [0.0; 4];
        close(shade(&l, &v)[0], [0.6, 0.3, 1.0, 0.5]);
        // Diffuse only: the ambient material stays 0.2.
        l.color_mode = 0x1201;
        close(shade(&l, &v)[0], [0.54, 0.29, 1.0, 0.5]);
        // Ambient only: diffuse stays 0.8.
        l.color_mode = 0x1200;
        close(shade(&l, &v)[0], [0.9, 0.85, 1.0, 1.0]);
        // Specular, with light 0's white specular light along z.
        l.color_mode = 0x1202;
        l.lights[0].specular = [1.0; 4];
        l.lights[0].diffuse = [0.0; 4];
        close(shade(&l, &v)[0], [0.54, 0.29, 1.0, 1.0]);
        // A face it doesn't name keeps its material.
        l.color_face = GL_BACK;
        l.two_side = true;
        let [front, _] = shade(&l, &v);
        close(front, [0.04, 0.04, 0.04, 1.0]);
        l.color_face = GL_FRONT;
        let [_, back] = shade(&l, &v);
        // The back is lit from behind its reversed normal: only ambient.
        close(back, [0.04, 0.04, 0.04, 1.0]);
    }

    #[test]
    fn two_sided_lighting_lights_the_back_with_its_material_and_reversed_normal() {
        let mut l = diffuse_only();
        l.two_side = true;
        l.materials[1].diffuse = [0.0, 1.0, 0.0, 0.25];
        // The light behind the surface lights its back.
        l.lights[0].position = [0.0, 0.0, -1.0, 0.0];
        let [front, back] = shade(&l, &input([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0]));
        close(front, [0.0, 0.0, 0.0, 1.0]);
        close(back, [0.0, 1.0, 0.0, 0.25]);
    }

    #[test]
    fn lights_that_are_off_add_nothing() {
        let mut l = diffuse_only();
        l.enabled = 0b10;
        l.lights[1] = Light::new(0);
        l.lights[0].diffuse = [100.0; 4];
        let c = shade(&l, &input([0.0, 0.0, -5.0, 1.0], [0.0, 0.0, 1.0]))[0];
        close(c, [0.8, 0.8, 0.8, 1.0]);
        assert_eq!(Material::default().diffuse, [0.8, 0.8, 0.8, 1.0]);
    }

    /// The column-major matrix that scales by `s`.
    fn scaling(s: [f32; 3]) -> [f32; 16] {
        let mut m = [0.0; 16];
        m[0] = s[0];
        m[5] = s[1];
        m[10] = s[2];
        m[15] = 1.0;
        m
    }

    #[test]
    fn normals_take_the_inverse_transpose_of_a_non_uniform_scale() {
        // Scaled by 2 along x and 1 along y, a surface at 45 degrees
        // leans more toward x, and its normal less: (1, 1) becomes
        // (1/2, 1).
        let nm = normal_matrix(&scaling([2.0, 1.0, 1.0])).unwrap();
        let n = eye_normal(Some(&nm), [1.0, 1.0, 0.0], false);
        assert!((n[0] - 0.5).abs() < 1e-6 && (n[1] - 1.0).abs() < 1e-6);
        let n = eye_normal(Some(&nm), [1.0, 1.0, 0.0], true);
        let len = (1.25f32).sqrt();
        assert!((n[0] - 0.5 / len).abs() < 1e-6 && (n[1] - 1.0 / len).abs() < 1e-6);
    }

    #[test]
    fn a_uniform_scale_scales_the_lighting_without_normalize() {
        // A modelview scaled by 2 halves the normals: n.L halves, unless
        // GL_NORMALIZE makes them unit again.
        let nm = normal_matrix(&scaling([2.0, 2.0, 2.0])).unwrap();
        let mut l = diffuse_only();
        l.materials[0].diffuse = [1.0; 4];
        let lit = |normalize: bool| {
            let n = eye_normal(Some(&nm), [0.0, 0.0, 1.0], normalize);
            shade(&l, &input([0.0, 0.0, -5.0, 1.0], n))[0]
        };
        close(lit(false), [0.5, 0.5, 0.5, 1.0]);
        close(lit(true), [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn rotations_turn_normals_as_they_turn_positions() {
        // 90 degrees about z: x goes to y.
        let mut m = [0.0f32; 16];
        m[1] = 1.0;
        m[4] = -1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        let nm = normal_matrix(&m).unwrap();
        let n = eye_normal(Some(&nm), [1.0, 0.0, 0.0], false);
        assert!(n[0].abs() < 1e-6 && (n[1] - 1.0).abs() < 1e-6);
        // A matrix without an inverse leaves normals as they are.
        assert!(normal_matrix(&scaling([1.0, 0.0, 1.0])).is_none());
        assert_eq!(eye_normal(None, [0.0, 2.0, 0.0], false), [0.0, 2.0, 0.0]);
    }
}

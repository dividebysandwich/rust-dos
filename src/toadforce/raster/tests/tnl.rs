//! Draws of vertices the card transforms and lights, against pixels
//! worked out by hand. The render target is 64 pixels square with the
//! viewport all over it, so clip x of -1 to 1 is screen x of 0 to 64
//! and clip y of 1 to -1 is screen y of 0 to 64.

use super::*;

const XYZ: u32 = 0x002;
const NORMAL: u32 = 0x010;
const DIFFUSE: u32 = 0x040;
const TEX1: u32 = 0x100;

const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

fn words(numbers: &[f32]) -> Vec<u32> {
    numbers.iter().map(|n| n.to_bits()).collect()
}

/// A matrix that moves by (`x`, `y`, `z`).
fn moved(x: f32, y: f32, z: f32) -> [f32; 16] {
    let mut m = IDENTITY;
    (m[12], m[13], m[14]) = (x, y, z);
    m
}

/// The pixels a clockwise triangle of screen-space vertices covers.
fn covered(points: [(f32, f32); 3]) -> Vec<(u32, u32)> {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    let vertices: Vec<u32> = points
        .iter()
        .flat_map(|&(x, y)| vertex(x, y, 0xFFFF_FFFF, 0.0, 0.0))
        .collect();
    assert_eq!(rig.draw(4, &vertices), 0);
    rig.drawn(0)
}

impl Rig {
    /// A rig that draws unlit.
    fn unlit() -> Rig {
        let mut rig = Rig::new(Format::X8R8G8B8, None);
        rig.states(&[(RS_LIGHTING, 0)]);
        rig
    }

    fn transform(&mut self, which: u32, matrix: &[f32; 16]) {
        assert_eq!(self.run(|w| encode::transform(w, which, matrix)), 0);
    }

    /// Draw triangles of `vertices`, laid out as `fvf`.
    fn triangles(&mut self, fvf: u32, vertices: &[u32]) {
        let size = crate::toadforce::raster::vertex::Fvf::parse(fvf)
            .unwrap()
            .words;
        let count = (vertices.len() / size) as u32;
        assert_eq!(self.run(|w| encode::draw(w, 4, fvf, count, vertices)), 0);
    }

    /// Draw the triangle of the positions `p` in white.
    fn white(&mut self, p: [[f32; 3]; 3]) {
        self.triangles(XYZ, &words(&p.concat()));
    }

    /// Light a triangle that fills the target's top left half, all of
    /// whose vertices have the normal `n`, and give its colour.
    fn lit(&mut self, n: [f32; 3]) -> u32 {
        let p = [[-1.0, 1.0, 0.5], [1.0, 1.0, 0.5], [-1.0, -1.0, 0.5]];
        let vertices: Vec<f32> = p
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], n[0], n[1], n[2]])
            .collect();
        self.triangles(XYZ | NORMAL, &words(&vertices));
        self.px(8, 8)
    }

    fn light(&mut self, slot: u32, light: &Light) {
        let error = self.run(|w| {
            encode::light(w, slot, light);
            encode::enable_light(w, slot, true);
        });
        assert_eq!(error, 0);
    }

    fn material(&mut self, material: &Material) {
        assert_eq!(self.run(|w| encode::material(w, material)), 0);
    }
}

/// A white light that shines along +z, into the picture.
fn sun() -> Light {
    Light {
        kind: LIGHT_DIRECTIONAL,
        diffuse: [1.0; 4],
        specular: [1.0; 4],
        direction: [0.0, 0.0, 1.0],
        ..Light::default()
    }
}

fn matte(diffuse: [f32; 4]) -> Material {
    Material {
        diffuse,
        ..Material::default()
    }
}

#[test]
fn a_vertex_goes_through_world_view_and_projection_to_the_viewport() {
    // With every matrix the identity, clip space is the vertex's own.
    let mut rig = Rig::unlit();
    rig.white([[-1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]]);
    let corner = covered([(0.0, 0.0), (32.0, 0.0), (0.0, 32.0)]);
    assert_eq!(rig.drawn(0), corner);
    assert_eq!(rig.px(0, 0), 0x00FF_FFFF);

    // The world matrix moves it, the view matrix after it, and the
    // projection last: a quarter right, then up, then all doubled.
    let mut rig = Rig::unlit();
    rig.transform(TRANSFORM_WORLD, &moved(0.25, 0.0, 0.0));
    rig.transform(TRANSFORM_VIEW, &moved(0.0, 0.25, 0.0));
    let mut twice = IDENTITY;
    (twice[0], twice[5]) = (2.0, 2.0);
    rig.transform(TRANSFORM_PROJECTION, &twice);
    rig.white([[-0.75, 0.25, 0.0], [-0.25, 0.25, 0.0], [-0.75, -0.25, 0.0]]);
    assert_eq!(rig.drawn(0), corner);

    // A projection that makes w of z divides by it.
    let mut rig = Rig::unlit();
    let mut perspective = IDENTITY;
    (perspective[11], perspective[15]) = (1.0, 0.0);
    rig.transform(TRANSFORM_PROJECTION, &perspective);
    rig.white([[-2.0, 2.0, 2.0], [0.0, 2.0, 2.0], [-4.0, 0.0, 4.0]]);
    assert_eq!(rig.drawn(0), corner);

    // A viewport in the target's lower right quarter.
    let mut rig = Rig::unlit();
    let viewport = Viewport {
        x: 32,
        y: 32,
        width: 32,
        height: 32,
        min_z: 0.0,
        max_z: 1.0,
    };
    assert_eq!(rig.run(|w| encode::viewport(w, &viewport)), 0);
    rig.white([[-1.0, 1.0, 0.0], [1.0, 1.0, 0.0], [-1.0, -1.0, 0.0]]);
    assert_eq!(
        rig.drawn(0),
        covered([(32.0, 32.0), (64.0, 32.0), (32.0, 64.0)])
    );
}

#[test]
fn an_indexed_draw_transforms_the_vertices_its_indices_name() {
    // Vertex 0 is never named, and the two triangles share an edge.
    let mut rig = Rig::unlit();
    let p = [
        [f32::NAN, f32::NAN, f32::NAN],
        [-1.0, 1.0, 0.5],
        [1.0, 1.0, 0.5],
        [-1.0, -1.0, 0.5],
        [1.0, -1.0, 0.5],
    ];
    let vertices = words(&p.concat());
    let index = [1, 2, 3, 3, 2, 4];
    let error = rig.run(|w| encode::draw_indexed(w, 4, XYZ, 5, &vertices, &index));
    assert_eq!(error, 0);
    assert_eq!(rig.drawn(0).len(), (SIZE * SIZE) as usize);
}

#[test]
fn depth_runs_from_the_viewports_near_to_its_far() {
    let mut rig = Rig::new(Format::X8R8G8B8, Some(Format::D16));
    rig.states(&[(RS_LIGHTING, 0), (RS_ZENABLE, 1), (RS_ZFUNC, 8)]);
    let viewport = Viewport {
        x: 0,
        y: 0,
        width: SIZE,
        height: SIZE,
        min_z: 0.25,
        max_z: 0.75,
    };
    assert_eq!(rig.run(|w| encode::viewport(w, &viewport)), 0);
    rig.white([[-1.0, 1.0, 0.5], [1.0, 1.0, 0.5], [-1.0, -1.0, 0.5]]);
    assert_eq!(rig.depth(4, 4), depth_value(Format::D16, 0.5));
    rig.white([[-1.0, 1.0, 1.0], [1.0, 1.0, 1.0], [-1.0, -1.0, 1.0]]);
    assert_eq!(rig.depth(4, 4), depth_value(Format::D16, 0.75));
}

#[test]
fn a_triangle_is_cut_at_the_near_plane_the_band_and_the_clip_planes() {
    // Depth goes from 1 at the top to -1 at the bottom, and is 0 at
    // row 16, where the near plane cuts.
    let dipping = [[-1.0, 1.0, 1.0], [0.0, 1.0, 1.0], [-1.0, 0.0, -1.0]];
    let mut rig = Rig::unlit();
    rig.white(dipping);
    let upper: Vec<(u32, u32)> = covered([(0.0, 0.0), (32.0, 0.0), (0.0, 32.0)])
        .into_iter()
        .filter(|&(_, y)| y < 16)
        .collect();
    assert_eq!(rig.drawn(0), upper);
    // With clipping off the near and the far plane cut as with it on:
    // the state changes no pixel of a card that transforms.
    for clipping in [0, 1] {
        rig.clear(1, 0, 1.0, 0);
        rig.states(&[(RS_CLIPPING, clipping)]);
        rig.white(dipping);
        assert_eq!(rig.drawn(0), upper);
        rig.clear(1, 0, 1.0, 0);
        rig.white([[-1.0, 1.0, 2.0], [0.0, 1.0, 2.0], [-1.0, 0.0, 2.0]]);
        assert_eq!(rig.drawn(0), []);
    }

    // A triangle far larger than the band covers the whole target.
    let mut rig = Rig::unlit();
    rig.white([
        [-3000.0, 2000.0, 0.5],
        [5000.0, 2000.0, 0.5],
        [-3000.0, -6000.0, 0.5],
    ]);
    assert_eq!(rig.drawn(0).len(), (SIZE * SIZE) as usize);

    // A clip plane that keeps x of 0 and above in the world, which
    // the world matrix moves half the target to the left.
    let mut rig = Rig::unlit();
    rig.transform(TRANSFORM_WORLD, &moved(-1.0, 0.0, 0.0));
    assert_eq!(
        rig.run(|w| encode::clip_plane(w, 2, &[1.0, 0.0, 0.0, 0.5])),
        0
    );
    rig.states(&[(RS_CLIPPLANEENABLE, 4)]);
    rig.white([[0.0, 1.0, 0.0], [2.0, 1.0, 0.0], [0.0, -1.0, 0.0]]);
    let right: Vec<(u32, u32)> = covered([(0.0, 0.0), (64.0, 0.0), (0.0, 64.0)])
        .into_iter()
        .filter(|&(x, _)| x >= 16)
        .collect();
    assert_eq!(rig.drawn(0), right);
    // A plane that isn't enabled cuts nothing.
    rig.clear(1, 0, 1.0, 0);
    rig.states(&[(RS_CLIPPLANEENABLE, 1)]);
    rig.white([[0.0, 1.0, 0.0], [2.0, 1.0, 0.0], [0.0, -1.0, 0.0]]);
    assert_eq!(rig.drawn(0).len(), 2080);
}

#[test]
fn a_cut_triangles_outline_is_of_its_own_edges_and_corners() {
    // The triangle the near plane cuts at row 16: its corners on the
    // screen are (0, 0), (32, 0) and, behind the plane, (0, 32).
    let dipping = [[-1.0, 1.0, 1.0], [0.0, 1.0, 1.0], [-1.0, 0.0, -1.0]];
    let mut rig = Rig::unlit();
    rig.states(&[(RS_FILLMODE, 2), (RS_CULLMODE, 1)]);
    rig.white(dipping);
    let lines = rig.drawn(0);
    // The top edge and what is left of the other two are drawn, and
    // nothing along the plane or past it.
    assert!(lines.contains(&(16, 0)), "{lines:?}");
    assert!(lines.contains(&(0, 8)), "{lines:?}");
    assert!(lines.contains(&(24, 8)), "{lines:?}");
    assert!(lines.iter().all(|&(_, y)| y <= 16), "{lines:?}");
    for x in 2..14 {
        assert!(!lines.contains(&(x, 15)) && !lines.contains(&(x, 16)));
    }
    // As points, the two corners in front of the plane.
    rig.clear(1, 0, 1.0, 0);
    rig.states(&[(RS_FILLMODE, 1)]);
    rig.white(dipping);
    assert_eq!(rig.drawn(0), [(0, 0), (32, 0)]);
}

#[test]
fn lines_and_points_are_cut_too() {
    let mut rig = Rig::unlit();
    // A line from the middle of the top row to far outside on the
    // right is drawn to the target's edge.
    let line = words(&[0.0, 1.0, 0.0, 100.0, 1.0, 0.0]);
    assert_eq!(rig.run(|w| encode::draw(w, 2, XYZ, 2, &line)), 0);
    assert_eq!(rig.drawn(0), (32..64).map(|x| (x, 0)).collect::<Vec<_>>());
    // A point behind the near plane isn't drawn, and one inside is.
    rig.clear(1, 0, 1.0, 0);
    let points = words(&[0.0, 0.0, -0.5, -0.5, 0.5, 0.5]);
    assert_eq!(rig.run(|w| encode::draw(w, 1, XYZ, 2, &points)), 0);
    assert_eq!(rig.drawn(0), [(16, 16)]);
}

#[test]
fn a_light_gives_a_surface_its_colour_by_the_angle_it_falls_at() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    rig.material(&matte([1.0, 0.5, 0.25, 1.0]));
    // No light is on: the surface is black.
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0);
    rig.light(0, &sun());
    // Facing the light, the material's colour times the light's.
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x00FF_8040);
    // At 60 degrees to it, half of that.
    assert_eq!(rig.lit([0.0, 0.75f32.sqrt(), -0.5]), 0x0080_4020);
    // Turned away, nothing.
    assert_eq!(rig.lit([0.0, 0.0, 1.0]), 0);
    // A normal of another length scales the light, unless normals are
    // brought to a length of 1.
    assert_eq!(rig.lit([0.0, 0.0, -0.5]), 0x0080_4020);
    rig.states(&[(RS_NORMALIZENORMALS, 1)]);
    assert_eq!(rig.lit([0.0, 0.0, -0.5]), 0x00FF_8040);
    // The ambient light and the material's ambient colour, and its
    // emissive colour, come on top.
    rig.states(&[(RS_AMBIENT, 0x0040_4040)]);
    rig.material(&Material {
        ambient: [1.0, 1.0, 1.0, 1.0],
        emissive: [0.0, 0.0, 0.25, 0.0],
        ..matte([0.0; 4])
    });
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x0040_4080);
    // The view matrix turns the light as it turns the surface: after
    // a quarter turn about x the light still falls straight on it.
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    rig.material(&matte([1.0; 4]));
    rig.light(0, &sun());
    let mut turn = IDENTITY;
    (turn[5], turn[6], turn[9], turn[10]) = (0.0, -1.0, 1.0, 0.0);
    rig.transform(TRANSFORM_VIEW, &turn);
    rig.transform(TRANSFORM_PROJECTION, &{
        // The turn undone, so the triangle is where it was.
        let mut back = IDENTITY;
        (back[5], back[6], back[9], back[10]) = (0.0, 1.0, -1.0, 0.0);
        back
    });
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x00FF_FFFF);
}

#[test]
fn a_point_light_reaches_as_far_as_its_range_and_fades_with_distance() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    rig.material(&matte([1.0; 4]));
    // Two in front of the surface's middle, which is at depth 0.5.
    let lamp = Light {
        kind: LIGHT_POINT,
        diffuse: [1.0; 4],
        position: [0.0, 0.0, -1.5],
        range: 10.0,
        attenuation: [0.0, 1.0, 0.0],
        ..Light::default()
    };
    rig.light(0, &lamp);
    // A small triangle around the middle: the light falls all but
    // straight on it from a little over 2 away, and the distance
    // leaves a little under half of it (2 / 4.02).
    let small = |rig: &mut Rig| {
        let p = [[-0.1f32, 0.1, 0.5], [0.1, 0.1, 0.5], [-0.1, -0.1, 0.5]];
        let vertices: Vec<f32> = p
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 0.0, 0.0, -1.0])
            .collect();
        rig.triangles(XYZ | NORMAL, &words(&vertices));
        rig.px(30, 30)
    };
    assert_eq!(small(&mut rig), 0x007F_7F7F);
    rig.light(0, &Light { range: 1.5, ..lamp });
    rig.clear(1, 0, 1.0, 0);
    assert_eq!(small(&mut rig), 0);
    // A spot that points at the surface lights it, and one that
    // points past it doesn't.
    let spot = Light {
        kind: LIGHT_SPOT,
        direction: [0.0, 0.0, 1.0],
        attenuation: [1.0, 0.0, 0.0],
        theta: 0.5,
        phi: 1.0,
        falloff: 1.0,
        ..lamp
    };
    rig.light(0, &spot);
    assert_eq!(small(&mut rig), 0x00FE_FEFE);
    rig.light(
        0,
        &Light {
            direction: [1.0, 0.0, 0.0],
            ..spot
        },
    );
    rig.clear(1, 0, 1.0, 0);
    assert_eq!(small(&mut rig), 0);
}

#[test]
fn a_lights_numbers_outside_their_usual_ranges_light_as_the_formulas_say() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    rig.material(&matte([1.0; 4]));
    let lamp = Light {
        kind: LIGHT_POINT,
        diffuse: [1.0; 4],
        position: [0.0, 0.0, -1.5],
        range: 10.0,
        attenuation: [1.0, 0.0, 0.0],
        ..Light::default()
    };
    // The small triangle around the middle the light falls all but
    // straight on, lit by `light`.
    let mut small = |light: Light| {
        rig.clear(1, 0, 1.0, 0);
        rig.light(0, &light);
        let p = [[-0.1f32, 0.1, 0.5], [0.1, 0.1, 0.5], [-0.1, -0.1, 0.5]];
        let vertices: Vec<f32> = p
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 0.0, 0.0, -1.0])
            .collect();
        rig.triangles(XYZ | NORMAL, &words(&vertices));
        rig.px(30, 30)
    };
    assert_eq!(small(lamp), 0x00FE_FEFE);
    // Every vertex is further than a range below 0.
    let range = -1.0;
    assert_eq!(small(Light { range, ..lamp }), 0);
    // With an attenuation that isn't above 0 the light adds nothing.
    for attenuation in [[0.0; 3], [-1.0, 0.0, 0.0], [1.0, -1.0, 0.0]] {
        let light = Light {
            attenuation,
            ..lamp
        };
        assert_eq!(small(light), 0, "{attenuation:?}");
    }
    // An inner cone wider than the outer one: all of the outer is lit.
    let spot = Light {
        kind: LIGHT_SPOT,
        direction: [0.0, 0.0, 1.0],
        theta: 2.0,
        phi: 1.0,
        falloff: 1.0,
        ..lamp
    };
    assert_eq!(small(spot), 0x00FE_FEFE);
    // A spot that points no way lights nothing, with a cone up to
    // half a turn wide, and a directional light comes from nowhere.
    let direction = [0.0; 3];
    assert_eq!(small(Light { direction, ..spot }), 0);
    let phi = 3.0;
    let wide = Light {
        direction,
        phi,
        ..spot
    };
    assert_eq!(small(wide), 0);
    let kind = LIGHT_DIRECTIONAL;
    assert_eq!(small(Light { kind, ..wide }), 0);
}

#[test]
fn a_highlight_is_added_where_the_light_is_thrown_back_at_the_eye() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    rig.states(&[(RS_SPECULARENABLE, 1), (RS_LOCALVIEWER, 0)]);
    rig.material(&Material {
        specular: [0.5, 0.25, 0.0, 1.0],
        power: 10.0,
        ..matte([0.0; 4])
    });
    rig.light(0, &sun());
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x0080_4000);
    // Off the mirror's direction it falls by the power.
    let dim = rig.lit([0.0, 0.6, -0.8]);
    assert_eq!(dim, 0x000E_0700);
    // Without the state there is none.
    rig.states(&[(RS_SPECULARENABLE, 0)]);
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0);
}

#[test]
fn a_vertexs_colours_stand_for_the_materials_where_the_state_says() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    rig.material(&matte([1.0, 0.0, 0.0, 1.0]));
    rig.light(0, &sun());
    let draw = |rig: &mut Rig| {
        let p = [[-1.0f32, 1.0, 0.5], [1.0, 1.0, 0.5], [-1.0, -1.0, 0.5]];
        let mut vertices = Vec::new();
        for p in p {
            vertices.extend(words(&[p[0], p[1], p[2], 0.0, 0.0, -1.0]));
            vertices.push(0xFF00_00FF);
        }
        rig.triangles(XYZ | NORMAL | DIFFUSE, &vertices);
        rig.px(8, 8)
    };
    // The diffuse colour comes from the vertex after a reset.
    assert_eq!(draw(&mut rig), 0x0000_00FF);
    rig.states(&[(RS_DIFFUSEMATERIALSOURCE, 0)]);
    assert_eq!(draw(&mut rig), 0x00FF_0000);
    rig.states(&[(RS_DIFFUSEMATERIALSOURCE, 1), (RS_COLORVERTEX, 0)]);
    assert_eq!(draw(&mut rig), 0x00FF_0000);
    // A vertex without the colour leaves the material's.
    rig.states(&[(RS_COLORVERTEX, 1)]);
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x00FF_0000);
    // Unlit, the vertex's colour is the fragment's.
    rig.states(&[(RS_LIGHTING, 0)]);
    assert_eq!(draw(&mut rig), 0x0000_00FF);
}

#[test]
fn the_back_of_a_triangle_is_lit_by_its_normal_turned_round() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    rig.states(&[(RS_CULLMODE, 1)]);
    rig.material(&matte([1.0; 4]));
    rig.light(0, &sun());
    // Counterclockwise, with a normal that faces away from the light.
    let back = |rig: &mut Rig| {
        let p = [[-1.0f32, 1.0, 0.5], [-1.0, -1.0, 0.5], [1.0, 1.0, 0.5]];
        let vertices: Vec<f32> = p
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 0.0, 0.0, 1.0])
            .collect();
        rig.triangles(XYZ | NORMAL, &words(&vertices));
        rig.px(8, 8)
    };
    assert_eq!(back(&mut rig), 0);
    rig.states(&[(RS_TWO_SIDED_LIGHTING, 1)]);
    assert_eq!(back(&mut rig), 0x00FF_FFFF);
    // With 2 the clockwise side is the back, and this one the front.
    rig.clear(1, 0, 1.0, 0);
    rig.states(&[(RS_TWO_SIDED_LIGHTING, 2)]);
    assert_eq!(back(&mut rig), 0);
}

#[test]
fn fog_at_the_vertices_goes_by_their_distance_from_the_eye() {
    let mut rig = Rig::unlit();
    rig.states(&[
        (RS_FOGENABLE, 1),
        (RS_FOGVERTEXMODE, 3),
        (RS_FOGSTART, 0.0f32.to_bits()),
        (RS_FOGEND, 1.0f32.to_bits()),
        (RS_FOGCOLOR, 0),
    ]);
    // Half way to the fog's end, white is half fog.
    rig.white([[-1.0, 1.0, 0.5], [1.0, 1.0, 0.5], [-1.0, -1.0, 0.5]]);
    assert_eq!(rig.px(8, 8), 0x0080_8080);
    // Unfogged vertices keep their colour: no fog mode, no fog.
    rig.states(&[(RS_FOGVERTEXMODE, 0)]);
    rig.white([[-1.0, 1.0, 0.5], [1.0, 1.0, 0.5], [-1.0, -1.0, 0.5]]);
    assert_eq!(rig.px(8, 8), 0x00FF_FFFF);
}

#[test]
fn texture_coordinates_go_through_the_stages_matrix_or_come_from_the_vertex_in_camera_space() {
    // Four texels: black and red over green and blue.
    let texels: [&[u32]; 2] = [&[0xFF00_0000, 0xFFFF_0000], &[0xFF00_FF00, 0xFF00_00FF]];
    let square = |rig: &mut Rig, fvf: u32, coords: &[[f32; 2]; 4]| {
        let p = [[-1.0f32, 1.0], [1.0, 1.0], [-1.0, -1.0], [1.0, -1.0]];
        let mut vertices = Vec::new();
        for (p, c) in p.iter().zip(coords) {
            vertices.extend([p[0], p[1], 0.5]);
            if fvf & TEX1 != 0 {
                vertices.extend(c);
            }
        }
        let vertices = words(&vertices);
        assert_eq!(rig.run(|w| encode::draw(w, 5, fvf, 4, &vertices)), 0);
        [
            rig.px(16, 16),
            rig.px(48, 16),
            rig.px(16, 48),
            rig.px(48, 48),
        ]
    };
    let once = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
    let mut rig = Rig::unlit();
    rig.texture(Format::A8R8G8B8, &texels, None);
    assert_eq!(
        square(&mut rig, XYZ | TEX1, &once),
        [0, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF]
    );
    // The stage's matrix moves the coordinates half the texture to
    // the right, which wraps: a set of two numbers has 1 for a third.
    let mut shift = IDENTITY;
    shift[8] = 0.5;
    rig.transform(TRANSFORM_TEXTURE, &shift);
    rig.stage(0, &[(TSS_TEXTURETRANSFORMFLAGS, 2)]);
    assert_eq!(
        square(&mut rig, XYZ | TEX1, &once),
        [0x00FF_0000, 0, 0x0000_00FF, 0x0000_FF00]
    );
    // The position in camera space as the coordinates: x and y of -1
    // to 1, which the matrix brings to 0 to 1 with y turned round.
    let mut fit = IDENTITY;
    (fit[0], fit[5], fit[12], fit[13]) = (0.5, -0.5, 0.5, 0.5);
    rig.transform(TRANSFORM_TEXTURE, &fit);
    rig.stage(0, &[(TSS_TEXCOORDINDEX, 2 << 16)]);
    assert_eq!(
        square(&mut rig, XYZ, &once),
        [0, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF]
    );
}

#[test]
fn a_vertexs_weights_blend_the_world_matrices() {
    let corner = covered([(0.0, 0.0), (32.0, 0.0), (0.0, 32.0)]);
    let mut rig = Rig::unlit();
    rig.transform(TRANSFORM_WORLD + 1, &moved(2.0, 0.0, 0.0));
    rig.states(&[(RS_VERTEXBLEND, 1)]);
    // A quarter of the first matrix and three quarters of the second,
    // which moves by 2: 1.5 to the right.
    let p = [[-2.5f32, 1.0, 0.0], [-1.5, 1.0, 0.0], [-2.5, 0.0, 0.0]];
    let vertices: Vec<f32> = p.iter().flat_map(|p| [p[0], p[1], p[2], 0.25]).collect();
    rig.triangles(0x006, &words(&vertices));
    assert_eq!(rig.drawn(0), corner);
    // With the state off the weights are passed over.
    rig.clear(1, 0, 1.0, 0);
    rig.states(&[(RS_VERTEXBLEND, 0)]);
    let p = [[-1.0f32, 1.0, 0.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]];
    let vertices: Vec<f32> = p.iter().flat_map(|p| [p[0], p[1], p[2], 0.25]).collect();
    rig.triangles(0x006, &words(&vertices));
    assert_eq!(rig.drawn(0), corner);
}

/// A triangle over the target's top left half whose first vertex is
/// red and the others blue, at depths 0 on the left and 1 on the
/// right, drawn flat with linear vertex fog from 0 to 1 in black.
fn flat_and_fogged(rig: &mut Rig) {
    rig.states(&[
        (RS_SHADEMODE, 1),
        (RS_FOGENABLE, 1),
        (RS_FOGVERTEXMODE, 3),
        (RS_FOGSTART, 0.0f32.to_bits()),
        (RS_FOGEND, 1.0f32.to_bits()),
        (RS_FOGCOLOR, 0),
    ]);
    let (red, blue) = (0xFFFF_0000u32, 0xFF00_00FFu32);
    let vertex = |p: [f32; 3], colour: u32| {
        let mut v = words(&p);
        v.push(colour);
        v
    };
    let vertices = [
        vertex([-1.0, 1.0, 0.0], red),
        vertex([1.0, 1.0, 1.0], blue),
        vertex([-1.0, -1.0, 0.0], blue),
    ]
    .concat();
    rig.triangles(XYZ | DIFFUSE, &vertices);
}

#[test]
fn a_flat_triangle_has_one_colour_and_each_vertexs_fog() {
    // The first vertex's red all over, and the fog of each pixel's own
    // distance: an eighth of the way across, an eighth of it is fog.
    let mut rig = Rig::unlit();
    flat_and_fogged(&mut rig);
    assert_eq!(rig.px(8, 8), 0x00DF_0000);
    assert_eq!(rig.px(40, 8), 0x0060_0000);
    assert_eq!(rig.px(0, 60), 0x00FF_0000);
    // With the last vertex giving the colour, the same fog on blue.
    let mut rig = Rig::unlit();
    rig.states(&[(RS_FLAT_VERTEX, 1)]);
    flat_and_fogged(&mut rig);
    assert_eq!(rig.px(8, 8), 0x0000_00DF);
    assert_eq!(rig.px(40, 8), 0x0000_0060);
}

/// Four texels in a row: red, green, blue, white.
const ROW: [u32; 4] = [0xFFFF_0000, 0xFF00_FF00, 0xFF00_00FF, 0xFFFF_FFFF];

/// The colours a quarter and three quarters of the way across a
/// square whose texture coordinate u is 0.75 on the left and 0.25 on
/// the right, drawn with `wrap` in `WRAP0`: of vertices the card
/// transforms when `lit`, and of vertices in screen space otherwise.
fn across_the_seam(wrap: u32, lit: bool) -> [u32; 2] {
    let mut rig = Rig::unlit();
    rig.texture(Format::A8R8G8B8, &[&ROW], None);
    rig.states(&[(RS_WRAP0, wrap)]);
    if lit {
        let p = [[-1.0f32, 1.0], [1.0, 1.0], [-1.0, -1.0], [1.0, -1.0]];
        let u = [0.75f32, 0.25, 0.75, 0.25];
        let vertices: Vec<f32> = p
            .iter()
            .zip(u)
            .flat_map(|(p, u)| [p[0], p[1], 0.5, u, 0.5])
            .collect();
        let vertices = words(&vertices);
        assert_eq!(rig.run(|w| encode::draw(w, 5, XYZ | TEX1, 4, &vertices)), 0);
    } else {
        let white = 0xFFFF_FFFF;
        let vertices = [
            vertex(0.0, 0.0, white, 0.75, 0.5),
            vertex(64.0, 0.0, white, 0.25, 0.5),
            vertex(0.0, 64.0, white, 0.75, 0.5),
            vertex(64.0, 64.0, white, 0.25, 0.5),
        ]
        .concat();
        assert_eq!(rig.draw(5, &vertices), 0);
    }
    [rig.px(16, 32), rig.px(48, 32)]
}

#[test]
fn a_wrapped_coordinate_goes_the_shorter_way_round() {
    let (green, blue, white, red) = (0x0000_FF00, 0x0000_00FF, 0x00FF_FFFF, 0x00FF_0000);
    for lit in [false, true] {
        // Unwrapped, u runs back from 0.75 to 0.25 through the middle
        // of the texture: blue, then green.
        assert_eq!(across_the_seam(0, lit), [blue, green], "lit {lit}");
        // Wrapped, it runs on from 0.75 through 1 to 1.25: white, then
        // red, over the seam.
        assert_eq!(across_the_seam(1, lit), [white, red], "lit {lit}");
        // The bit of another coordinate leaves u as it is.
        assert_eq!(across_the_seam(2, lit), [blue, green], "lit {lit}");
    }
}

#[test]
fn a_wrapped_coordinate_half_a_turn_or_more_from_the_bases_moves_one_turn() {
    use crate::toadforce::raster::{wrap_base, wrapped};
    assert_eq!(wrapped(0.9, 0.1), 1.1);
    assert_eq!(wrapped(0.1, 0.9), 0.9 - 1.0);
    assert_eq!(wrapped(0.25, 0.625), 0.625);
    assert_eq!(wrapped(0.625, 0.25), 0.25);
    // Exactly half a turn moves, either way.
    assert_eq!(wrapped(0.25, 0.75), -0.25);
    assert_eq!(wrapped(0.75, 0.25), 1.25);
    // One turn and no more: further apart than a turn and a half,
    // they stay more than half a turn apart.
    assert_eq!(wrapped(0.0, 2.25), 1.25);
    assert_eq!(wrapped(0.0, 1.375), 0.375);
    assert_eq!(wrapped(3.0, 0.5), 1.5);

    // The base is the highest vertex, and the one further left of two
    // that high, wherever in the primitive it comes.
    let at = |x: f32, y: f32| Some((x, y));
    assert_eq!(wrap_base(&[at(5.0, 9.0), at(7.0, 2.0), at(1.0, 4.0)]), 1);
    assert_eq!(wrap_base(&[at(5.0, 2.0), at(7.0, 2.0), at(1.0, 2.0)]), 2);
    assert_eq!(wrap_base(&[at(5.0, 2.0), at(5.0, 2.0), at(9.0, 2.0)]), 0);
    // A vertex that isn't in front of the eye is the base only when
    // none is.
    assert_eq!(wrap_base(&[None, at(7.0, 2.0), at(1.0, 1.0)]), 2);
    assert_eq!(wrap_base(&[None, None, None]), 0);
    assert_eq!(wrap_base(&[at(f32::NAN, 0.0), at(3.0, 8.0)]), 1);
}

#[test]
fn the_highest_vertex_is_the_one_the_others_wrap_to() {
    // One triangle over the target's top left half, with u of 0 at
    // its top left, 0.375 at its top right and 0.75 at its bottom
    // left, which comes first in the draw. The base is the top left:
    // the top right is less than half a turn from it and stays, and
    // the bottom left, more than that above it, comes down to -0.25.
    // (From the first vertex, the top left would go up to 1 and the
    // top edge run back from there.)
    let mut rig = Rig::unlit();
    rig.texture(Format::A8R8G8B8, &[&ROW], None);
    rig.states(&[(RS_WRAP0, 1)]);
    let white = 0xFFFF_FFFF;
    let vertices = [
        vertex(0.0, 64.0, white, 0.75, 0.5),
        vertex(0.0, 0.0, white, 0.0, 0.5),
        vertex(64.0, 0.0, white, 0.375, 0.5),
    ]
    .concat();
    assert_eq!(rig.draw(4, &vertices), 0);
    // Along the top u goes from 0 to 0.375: red, then green.
    assert_eq!(rig.px(32, 1), 0x00FF_0000);
    assert_eq!(rig.px(60, 1), 0x0000_FF00);
    // Down the left edge it goes from 0 to -0.25, the texture's last
    // quarter: white.
    assert_eq!(rig.px(1, 8), 0x00FF_FFFF);
    assert_eq!(rig.px(1, 56), 0x00FF_FFFF);
}


#[test]
fn linear_fog_leaves_what_is_nearer_than_its_start_and_fogs_what_is_at_its_end() {
    let (clear, fogged) = (0x00FF_FFFF, 0x00FF_0000);
    let at = |z: f32| [[-1.0, 1.0, z], [1.0, 1.0, z], [-1.0, -1.0, z]];
    // Fog from `start` to `end` at the vertices of a triangle at the
    // depth `z`.
    let vertex = |start: f32, end: f32, z: f32| {
        let mut rig = Rig::unlit();
        rig.states(&[
            (RS_FOGENABLE, 1),
            (RS_FOGVERTEXMODE, 3),
            (RS_FOGSTART, start.to_bits()),
            (RS_FOGEND, end.to_bits()),
            (RS_FOGCOLOR, fogged),
        ]);
        rig.white(at(z));
        rig.px(8, 8)
    };
    // And for each pixel of a square at the depth `z`.
    let table = |start: f32, end: f32, z: f32| {
        let mut rig = Rig::new(Format::X8R8G8B8, None);
        rig.states(&[
            (RS_FOGENABLE, 1),
            (RS_FOGTABLEMODE, 3),
            (RS_FOGSTART, start.to_bits()),
            (RS_FOGEND, end.to_bits()),
            (RS_FOGCOLOR, fogged),
        ]);
        assert_eq!(rig.draw(4, &corner(0.0, 0.0, 8.0, z, 0xFFFF_FFFF)), 0);
        rig.px(1, 1)
    };
    for fog in [&vertex as &dyn Fn(f32, f32, f32) -> u32, &table] {
        // Starting where it ends: none nearer, all from there on.
        assert_eq!(fog(0.5, 0.5, 0.25), clear);
        assert_eq!(fog(0.5, 0.5, 0.5), fogged);
        assert_eq!(fog(0.5, 0.5, 0.75), fogged);
        // Ending before it starts: the start is asked first.
        assert_eq!(fog(0.625, 0.375, 0.25), clear);
        assert_eq!(fog(0.625, 0.375, 0.5), clear);
        assert_eq!(fog(0.625, 0.375, 0.625), fogged);
        assert_eq!(fog(0.625, 0.375, 0.75), fogged);
        // Between an ordinary start and end, the share: a quarter of
        // the way, a quarter fog.
        assert_eq!(fog(0.25, 0.75, 0.375), 0x00FF_BFBF);
        assert_eq!(fog(0.25, 0.75, 0.75), fogged);
    }
}


#[test]
fn exponential_and_range_fog_follow_their_formulas() {
    let p = [[-1.0, 1.0, 0.5], [1.0, 1.0, 0.5], [-1.0, -1.0, 0.5]];
    let fogged = |states: &[(u32, u32)]| {
        let mut rig = Rig::unlit();
        rig.states(&[(RS_FOGENABLE, 1), (RS_FOGCOLOR, 0)]);
        rig.states(states);
        rig.white(p);
        rig.px(8, 8)
    };
    // e^-0.5 is 0.6065, 155 of 255.
    let exp = [(RS_FOGVERTEXMODE, 1), (RS_FOGDENSITY, 1.0f32.to_bits())];
    assert_eq!(fogged(&exp), 0x009B_9B9B);
    // e^-(2 * 0.5)^2 is 0.3679, 94 of 255.
    let exp2 = [(RS_FOGVERTEXMODE, 2), (RS_FOGDENSITY, 2.0f32.to_bits())];
    assert_eq!(fogged(&exp2), 0x005E_5E5E);
    // By depth the vertices are 0.5 away, a quarter of the way to the
    // fog's end. By range each is 1.5 from the eye, three quarters.
    let linear = [
        (RS_FOGVERTEXMODE, 3),
        (RS_FOGSTART, 0.0f32.to_bits()),
        (RS_FOGEND, 2.0f32.to_bits()),
    ];
    assert_eq!(fogged(&linear), 0x00BF_BFBF);
    let mut range = linear.to_vec();
    range.push((RS_RANGEFOGENABLE, 1));
    assert_eq!(fogged(&range), 0x0040_4040);
}

#[test]
fn generated_coordinates_follow_their_formulas() {
    use crate::toadforce::raster::geometry::coordinates;
    let close = |got: [f32; 4], want: &[f32]| {
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 1e-5, "{got:?} against {want:?}");
        }
    };
    let none = ([0.0; 4], 0);
    let (eye, normal) = ([3.0, 0.0, 4.0], [0.0, 0.6, -0.8]);
    // The normal and the position, as they are.
    close(
        coordinates(1, none, eye, normal, true, None),
        &[0.0, 0.6, -0.8],
    );
    close(
        coordinates(2, none, eye, normal, true, None),
        &[3.0, 0.0, 4.0],
    );
    // The line of sight I is (0.6, 0, 0.8), and N . I is -0.64: the
    // reflection is I + 1.28 * N.
    close(
        coordinates(3, none, eye, normal, true, None),
        &[0.6, 0.768, -0.224],
    );
    // A sphere map of that: m is twice the length of (0.6, 0.768,
    // 1.224), which is 1.5649.
    let m = 2.0 * (0.6f32 * 0.6 + 0.768 * 0.768 + 1.224 * 1.224).sqrt();
    close(
        coordinates(4, none, eye, normal, true, None),
        &[0.6 / m + 0.5, 0.768 / m + 0.5],
    );
    // Without a local viewer the line of sight is (0, 0, 1) wherever
    // the vertex is: N . I is -0.8, and the reflection (0, 0, 1) +
    // 1.6 * N.
    close(
        coordinates(3, none, eye, normal, false, None),
        &[0.0, 0.96, -0.28],
    );
    // The normal counts with the length it has: (0, 0, -3) at (0, 5,
    // 12), whose line of sight is (0, 5/13, 12/13), has N . I of
    // -36/13 and throws it back as I + 72/13 * N.
    let (eye, normal) = ([0.0, 5.0, 12.0], [0.0, 0.0, -3.0]);
    close(
        coordinates(3, none, eye, normal, true, None),
        &[0.0, 5.0 / 13.0, 12.0 / 13.0 - 216.0 / 13.0],
    );
    close(
        coordinates(3, none, eye, normal, false, None),
        &[0.0, 0.0, -17.0],
    );
    // A normal of no length throws nothing back: the line of sight
    // goes on, and without a local viewer that is the sphere map's
    // own axis, where m is 0 and the map's middle is taken.
    close(
        coordinates(3, none, eye, [0.0; 3], true, None),
        &[0.0, 5.0 / 13.0, 12.0 / 13.0],
    );
    close(
        coordinates(4, none, eye, [0.0; 3], false, None),
        &[0.5, 0.5],
    );
    // Straight on, the reflection comes straight back and the sphere
    // map's middle is its.
    let (eye, normal) = ([0.0, 0.0, 2.0], [0.0, 0.0, -1.0]);
    close(
        coordinates(3, none, eye, normal, true, None),
        &[0.0, 0.0, -1.0],
    );
    close(coordinates(4, none, eye, normal, true, None), &[0.5, 0.5]);
    // A sphere map through a matrix is its two numbers and a 1.
    close(
        coordinates(4, none, eye, normal, true, Some((&IDENTITY, 3))),
        &[0.5, 0.5, 1.0],
    );
}


#[test]
fn a_stages_coordinates_can_be_its_vertices_normals() {
    // Black and red over green and blue, picked by the normal's x and
    // y, which the vertices of each triangle share.
    let texels: [&[u32]; 2] = [&[0xFF00_0000, 0xFFFF_0000], &[0xFF00_FF00, 0xFF00_00FF]];
    let mut rig = Rig::unlit();
    rig.texture(Format::A8R8G8B8, &texels, None);
    rig.stage(0, &[(TSS_TEXCOORDINDEX, 1 << 16)]);
    assert_eq!(rig.lit([0.25, 0.25, 0.0]), 0);
    assert_eq!(rig.lit([0.75, 0.25, 0.0]), 0x00FF_0000);
    assert_eq!(rig.lit([0.25, 0.75, 0.0]), 0x0000_FF00);
    assert_eq!(rig.lit([0.75, 0.75, 0.0]), 0x0000_00FF);
}

#[test]
fn a_projected_stage_divides_by_its_last_coordinate() {
    let texels: [&[u32]; 2] = [&[0xFF00_0000, 0xFFFF_0000], &[0xFF00_FF00, 0xFF00_00FF]];
    let once = [[0.0f32, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
    let p = [[-1.0f32, 1.0], [1.0, 1.0], [-1.0, -1.0], [1.0, -1.0]];
    let quarters = |rig: &Rig| {
        [
            rig.px(16, 16),
            rig.px(48, 16),
            rig.px(16, 48),
            rig.px(48, 48),
        ]
    };
    let picture = [0, 0x00FF_0000, 0x0000_FF00, 0x0000_00FF];
    // Three coordinates, all twice what they were with a third of 2,
    // and the stage's matrix the identity with a count of 3: divided
    // by the third they are the texture once over.
    let mut rig = Rig::unlit();
    rig.texture(Format::A8R8G8B8, &texels, None);
    rig.stage(0, &[(TSS_TEXTURETRANSFORMFLAGS, 256 | 3)]);
    let vertices: Vec<f32> = p
        .iter()
        .zip(once)
        .flat_map(|(p, c)| [p[0], p[1], 0.5, c[0] * 2.0, c[1] * 2.0, 2.0])
        .collect();
    let vertices = words(&vertices);
    // A set of three numbers: size code 1 for set 0.
    let fvf = XYZ | TEX1 | 1 << 16;
    assert_eq!(rig.run(|w| encode::draw(w, 5, fvf, 4, &vertices)), 0);
    assert_eq!(quarters(&rig), picture);
    assert_eq!(rig.px(24, 24), 0);
    // Undivided, the coordinates run twice over the texture: three
    // eighths of the way across and down is three quarters of it.
    rig.stage(0, &[(TSS_TEXTURETRANSFORMFLAGS, 3)]);
    assert_eq!(rig.run(|w| encode::draw(w, 5, fvf, 4, &vertices)), 0);
    assert_eq!(rig.px(24, 24), 0x0000_00FF);
    // With a count of 0 the fourth coordinate divides: a set of four
    // numbers, passed on as they are.
    let mut rig = Rig::unlit();
    rig.texture(Format::A8R8G8B8, &texels, None);
    rig.stage(0, &[(TSS_TEXTURETRANSFORMFLAGS, 256)]);
    let vertices: Vec<f32> = p
        .iter()
        .zip(once)
        .flat_map(|(p, c)| [p[0], p[1], 0.5, c[0] * 2.0, c[1] * 2.0, 0.0, 2.0])
        .collect();
    let vertices = words(&vertices);
    let fvf = XYZ | TEX1 | 2 << 16;
    assert_eq!(rig.run(|w| encode::draw(w, 5, fvf, 4, &vertices)), 0);
    assert_eq!(quarters(&rig), picture);
    // A set of fewer numbers is divided by its last: two numbers, u
    // twice what it was and v of 2, come to a row of two texels once
    // across.
    let mut rig = Rig::unlit();
    rig.texture(Format::A8R8G8B8, &[&[0xFF00_0000, 0xFFFF_0000]], None);
    rig.stage(0, &[(TSS_TEXTURETRANSFORMFLAGS, 256)]);
    let set = |scale: f32, more: f32, v: f32| {
        let vertices: Vec<f32> = p
            .iter()
            .zip(once)
            .flat_map(|(p, c)| [p[0], p[1], 0.5, c[0] * scale + more, v])
            .collect();
        words(&vertices)
    };
    let vertices = set(2.0, 0.0, 2.0);
    assert_eq!(rig.run(|w| encode::draw(w, 5, XYZ | TEX1, 4, &vertices)), 0);
    assert_eq!(quarters(&rig), [0, 0x00FF_0000, 0, 0x00FF_0000]);
}

#[test]
fn a_points_size_is_scaled_by_its_distance_from_the_eye() {
    // The projection brings a depth of 4 inside the far plane.
    let mut shallow = IDENTITY;
    shallow[10] = 0.1;
    let point = |states: &[(u32, u32)]| {
        let mut rig = Rig::unlit();
        rig.transform(TRANSFORM_PROJECTION, &shallow);
        rig.states(states);
        let vertices = words(&[0.0, 0.0, 4.0]);
        assert_eq!(rig.run(|w| encode::draw(w, 1, XYZ, 1, &vertices)), 0);
        rig.drawn(0)
    };
    // Half a pixel is no less than the smallest size, 1.
    let size = (RS_POINTSIZE, 0.5f32.to_bits());
    assert_eq!(point(&[size]), [(32, 32)]);
    // Scaled by the viewport's 64 lines over the distance, 4, with C
    // alone: 8 pixels, the square from 28 to 35.
    let scaled = point(&[
        size,
        (RS_POINTSCALEENABLE, 1),
        (RS_POINTSCALE_A, 0.0f32.to_bits()),
        (RS_POINTSCALE_C, 1.0f32.to_bits()),
    ]);
    assert_eq!(scaled.len(), 64);
    assert!(scaled.iter().all(|&(x, y)| (28..36).contains(&x) && (28..36).contains(&y)));
    // With B alone the distance counts once where C has its square:
    // 0.5 * 64 * sqrt(1 / 4) is 16 pixels.
    let scaled = point(&[
        size,
        (RS_POINTSCALEENABLE, 1),
        (RS_POINTSCALE_A, 0.0f32.to_bits()),
        (RS_POINTSCALE_B, 1.0f32.to_bits()),
    ]);
    assert_eq!(scaled.len(), 256);
    // A sum that isn't above 0 leaves the size: one pixel.
    let unscaled = point(&[
        size,
        (RS_POINTSCALEENABLE, 1),
        (RS_POINTSCALE_A, 0.0f32.to_bits()),
    ]);
    assert_eq!(unscaled, [(32, 32)]);
}

#[test]
fn a_primitive_with_a_position_that_is_no_number_is_not_drawn() {
    for bad in [f32::NAN, f32::INFINITY] {
        for corner in 0..3 {
            let mut p = [[-1.0, 1.0, 0.5], [1.0, 1.0, 0.5], [-1.0, -1.0, 0.5]];
            p[corner][0] = bad;
            let mut rig = Rig::unlit();
            rig.white(p);
            assert!(rig.drawn(0).is_empty(), "{bad} at corner {corner}");
        }
    }
}

#[test]
fn a_matrix_with_no_inverse_gives_normals_of_no_length() {
    use crate::toadforce::raster::geometry::{normal_matrix, times};
    // A world matrix that flattens z: its three by three has no
    // inverse, and every normal comes out as zeros.
    let mut flat = IDENTITY;
    (flat[0], flat[10]) = (2.0, 0.0);
    let m = normal_matrix(&flat);
    assert_eq!(times([1.0, 1.0, 1.0], &m), [0.0; 3]);
    // One that has an inverse takes it by the inverse, transposed,
    // however small one of its scales is.
    let mut wide = IDENTITY;
    wide[0] = 2.0;
    let m = normal_matrix(&wide);
    assert_eq!(times([1.0, 1.0, 1.0], &m), [0.5, 1.0, 1.0]);
    let mut thin = IDENTITY;
    thin[5] = 1e-6;
    let m = normal_matrix(&thin);
    let n = times([1.0, 1.0, 1.0], &m);
    assert!((n[1] - 1e6).abs() < 1.0 && n[0] == 1.0 && n[2] == 1.0, "{n:?}");
    // A surface under the flattening matrix is lit by nothing.
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    rig.material(&matte([1.0; 4]));
    rig.light(0, &sun());
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x00FF_FFFF);
    let mut squashed = IDENTITY;
    squashed[10] = 0.0;
    rig.transform(TRANSFORM_WORLD, &squashed);
    rig.clear(1, 0, 1.0, 0);
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0);
}


/// The diffuse and specular colours the card gives a vertex at `eye`
/// in camera space with the normal `n`, lit by `light` alone on a
/// surface that is white in every colour but the emissive, with the
/// specular `power` and no ambient light of the scene's.
fn shone(
    light: &Light,
    eye: [f32; 3],
    n: [f32; 3],
    power: f32,
    local_viewer: bool,
) -> ([f32; 4], [f32; 4]) {
    use crate::toadforce::raster::light::{Lamp, Lighting, Source};
    let white = [1.0; 4];
    let material = Material {
        diffuse: white,
        ambient: white,
        specular: white,
        emissive: [0.0; 4],
        power,
    };
    Lighting::of(
        vec![Lamp::new(light, &IDENTITY)],
        material,
        [0.0; 3],
        true,
        local_viewer,
        [Source::Material; 4],
    )
    .light(eye, n, None, None)
}

/// Whether `got` is within a thousandth part of `want`, the accuracy
/// of the card's powers.
fn near(got: f32, want: f32) -> bool {
    (got - want).abs() <= want.abs() * 1e-3 + 1e-6
}

#[test]
fn a_spots_cone_is_its_share_to_the_falloff_whatever_the_falloff_is() {
    // A spot at the eye that shines along z with an inner cone of no
    // width and an outer one of half a turn: the share between the two
    // is rho itself, which is 0.8 for a vertex at (3, 0, 4). The
    // surface faces the light, whose colour is a quarter.
    let spot = |falloff: f32| Light {
        kind: LIGHT_SPOT,
        diffuse: [0.25; 4],
        position: [0.0; 3],
        direction: [0.0, 0.0, 1.0],
        range: 100.0,
        falloff,
        attenuation: [1.0, 0.0, 0.0],
        theta: 0.0,
        phi: std::f32::consts::PI,
        ..Light::default()
    };
    let lit = |light: &Light| shone(light, [3.0, 0.0, 4.0], [-0.6, 0.0, -0.8], 0.0, false).0[0];
    assert!(near(lit(&spot(1.0)), 0.25 * 0.8));
    assert!(near(lit(&spot(2.0)), 0.25 * 0.64));
    // A falloff of 0 gives the whole light between the cones.
    assert!(near(lit(&spot(0.0)), 0.25));
    assert!(near(lit(&spot(-0.0)), 0.25));
    // One below 0 is raised to as it is, and brightens: 0.8 to the
    // power -1 is 1.25, and to -2 it is 1.5625.
    assert!(near(lit(&spot(-1.0)), 0.25 * 1.25));
    assert!(near(lit(&spot(-2.0)), 0.25 * 1.5625));
    // The colour stops at 1: 0.8 to the power -20 is 86.7.
    assert_eq!(lit(&spot(-20.0)), 1.0);
    assert_eq!(lit(&spot(-1.0e9)), 1.0);
    // The edges: nothing at the outer cone or outside it, everything
    // inside the inner one, and with the two alike a hard edge.
    let cones = |theta: f32, phi: f32| Light {
        theta,
        phi,
        ..spot(1.0)
    };
    // rho of 0.8 is the cosine of half of 1.2870022 radians.
    let edge = 2.0 * 0.8f32.acos();
    assert_eq!(lit(&cones(0.0, edge - 0.001)), 0.0);
    assert!(near(lit(&cones(edge + 0.001, 3.0)), 0.25));
    assert_eq!(lit(&cones(edge - 0.001, edge - 0.001)), 0.0);
    assert!(near(lit(&cones(edge + 0.001, edge + 0.001)), 0.25));
    // An inner cone wider than the outer one: all inside the outer.
    assert!(near(lit(&cones(3.0, edge + 0.001)), 0.25));
    assert_eq!(lit(&cones(3.0, edge - 0.001)), 0.0);
    // Angles below 0 are cones of their size.
    assert!(near(lit(&cones(-(edge + 0.001), -3.0)), 0.25));
}

#[test]
fn a_highlight_needs_the_light_and_the_half_vector_in_front_and_takes_any_power() {
    let sun = Light {
        specular: [0.25; 4],
        ..sun()
    };
    // The light comes along z and the eye is taken to be straight
    // back, so H is (0, 0, -1) and N . H is 0.8 with this normal.
    let shine = |power: f32| shone(&sun, [0.0, 0.0, 2.0], [0.0, 0.6, -0.8], power, false).1[0];
    assert!(near(shine(2.0), 0.25 * 0.64));
    // A power of 0 is the whole highlight.
    assert_eq!(shine(0.0), 0.25);
    assert_eq!(shine(-0.0), 0.25);
    assert_eq!(shine(1e-30), 0.25);
    // One below 0 is raised to as it is.
    assert!(near(shine(-1.0), 0.25 * 1.25));
    assert_eq!(shine(-40.0), 1.0);
    // The specular colour's alpha is 1, for no fog.
    assert_eq!(shone(&sun, [0.0, 0.0, 2.0], [0.0, 0.6, -0.8], 2.0, false).1[3], 1.0);
    // N . H above 1, from a long normal, counts as 1.
    let long = shone(&sun, [0.0, 0.0, 2.0], [0.0, 0.0, -3.0], 5.0, false);
    assert_eq!(long.1[0], 0.25);
    // Side on to the light, N . L is 0 and there is no highlight,
    // though the eye, off to that side, makes N . H 0.707.
    let side = shone(&sun, [-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], 1.0, true);
    assert_eq!(side.1[0], 0.0);
    // With N . H of exactly 0 there is none either: the normal is
    // (0.8, 0, -0.6), the light in front of it, and H, between the
    // light's (0, 0, -1) and the eye's (-0.96, 0, -0.28), is along
    // (-0.6, 0, -0.8).
    let flat = shone(&sun, [0.96, 0.0, 0.28], [0.8, 0.0, -0.6], 1.0, true);
    assert!(flat.0[0] > 0.0);
    assert!(flat.1[0] < 1e-6, "{flat:?}");
}

#[test]
fn a_light_adds_nothing_at_its_range_at_its_own_place_or_with_no_attenuation() {
    let lamp = Light {
        kind: LIGHT_POINT,
        diffuse: [0.5; 4],
        ambient: [0.25; 4],
        position: [0.0, 0.0, 1.0],
        range: 10.0,
        attenuation: [1.0, 0.0, 0.0],
        ..Light::default()
    };
    // Two away along z, facing the light.
    let lit = |light: &Light| shone(light, [0.0, 0.0, 3.0], [0.0, 0.0, -1.0], 0.0, true).0[0];
    assert_eq!(lit(&lamp), 0.75);
    // At its range exactly it adds nothing, its ambient light
    // included; a little nearer it does.
    assert_eq!(lit(&Light { range: 2.0, ..lamp }), 0.0);
    assert_eq!(
        lit(&Light {
            range: 2.000_000_5,
            ..lamp
        }),
        0.75
    );
    assert_eq!(
        lit(&Light {
            range: 0.0,
            ..lamp
        }),
        0.0
    );
    // The distance weakens the ambient light as it does the rest: by
    // 1 / d here, a half.
    let fading = Light {
        attenuation: [0.0, 1.0, 0.0],
        ..lamp
    };
    assert_eq!(lit(&fading), 0.375);
    // An attenuation that comes to 0 or less: nothing at all.
    for attenuation in [[0.0; 3], [-1.0, 0.0, 0.0], [-2.0, 1.0, 0.0]] {
        let light = Light {
            attenuation,
            ..lamp
        };
        assert_eq!(lit(&light), 0.0, "{attenuation:?}");
    }
    // A vertex where the light is has no way to it: the light's
    // ambient colour alone, where Direct3D adds nothing.
    let there = shone(&lamp, [0.0, 0.0, 1.0], [0.0, 0.0, -1.0], 0.0, true);
    assert_eq!(there.0[0], 0.25);
    // A normal of no length takes the ambient light alone.
    let bare = shone(&lamp, [0.0, 0.0, 3.0], [0.0; 3], 0.0, true);
    assert_eq!((bare.0[0], bare.1[0]), (0.25, 0.0));
    // A spot that points no way adds nothing, with a cone up to half
    // a turn wide, and a directional light that points no way its
    // ambient light alone. A direction too short for a float to
    // measure is none.
    for direction in [[0.0; 3], [0.0, 0.0, 1e-25]] {
        let spot = Light {
            kind: LIGHT_SPOT,
            direction,
            theta: 1.0,
            phi: 3.0,
            ..lamp
        };
        assert_eq!(lit(&spot), 0.0, "{direction:?}");
        let kind = LIGHT_DIRECTIONAL;
        assert_eq!(lit(&Light { kind, ..spot }), 0.25, "{direction:?}");
    }
    // One that points along z, however long its direction is, lights.
    let direction = [0.0, 0.0, 3.0];
    let kind = LIGHT_DIRECTIONAL;
    assert_eq!(
        lit(&Light {
            kind,
            direction,
            ..lamp
        }),
        0.75
    );
}

#[test]
fn a_flat_triangles_points_have_its_colour() {
    let mut rig = Rig::unlit();
    rig.states(&[(RS_FILLMODE, 1), (RS_SHADEMODE, 1), (RS_CULLMODE, 1)]);
    let (red, blue) = (0xFFFF_0000u32, 0xFF00_00FFu32);
    let corner = |p: [f32; 3], colour: u32| {
        let mut v = words(&p);
        v.push(colour);
        v
    };
    let vertices = [
        corner([-1.0, 1.0, 0.5], red),
        corner([0.0, 1.0, 0.5], blue),
        corner([-1.0, 0.0, 0.5], blue),
    ]
    .concat();
    rig.triangles(XYZ | DIFFUSE, &vertices);
    assert_eq!(rig.drawn(0), [(0, 0), (32, 0), (0, 32)]);
    for (x, y) in [(0, 0), (32, 0), (0, 32)] {
        assert_eq!(rig.px(x, y), 0x00FF_0000);
    }
    // Culled, it has no points and no lines.
    for fill in [1, 2] {
        rig.clear(1, 0, 1.0, 0);
        rig.states(&[(RS_FILLMODE, fill), (RS_CULLMODE, 2)]);
        rig.triangles(XYZ | DIFFUSE, &vertices);
        assert_eq!(rig.drawn(0), [], "fill mode {fill}");
    }
}

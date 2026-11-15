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
    // With clipping off the near plane still cuts, as a card with
    // transform and lighting does, and what is past the far plane is
    // drawn.
    rig.clear(1, 0, 1.0, 0);
    rig.states(&[(RS_CLIPPING, 0)]);
    rig.white(dipping);
    assert_eq!(rig.drawn(0), upper);
    rig.white([[-1.0, 1.0, 2.0], [0.0, 1.0, 2.0], [-1.0, 0.0, 2.0]]);
    assert_eq!(rig.drawn(0).len(), 528);
    rig.clear(1, 0, 1.0, 0);
    rig.states(&[(RS_CLIPPING, 1)]);
    rig.white([[-1.0, 1.0, 2.0], [0.0, 1.0, 2.0], [-1.0, 0.0, 2.0]]);
    assert_eq!(rig.drawn(0), []);

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
    // An attenuation that isn't above 0 leaves the light as it is.
    for attenuation in [[0.0; 3], [-1.0, 0.0, 0.0], [1.0, -1.0, 0.0]] {
        let light = Light {
            attenuation,
            ..lamp
        };
        assert_eq!(small(light), 0x00FE_FEFE, "{attenuation:?}");
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
    // A direction of no length gives a rho of 0: the spot's cone
    // holds nothing, unless it is wider than half a turn (0.435 of
    // the way from the outer cone to the inner one here), and a
    // directional light comes from nowhere.
    let direction = [0.0; 3];
    assert_eq!(small(Light { direction, ..spot }), 0);
    let phi = 4.0;
    let wide = Light {
        direction,
        phi,
        ..spot
    };
    assert_eq!(small(wide), 0x006F_6F6F);
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

/// A counterclockwise triangle over the target's top left half whose
/// normals face away from the sun, with the colour `colour` at its
/// vertices: its back, turned round, is lit.
fn back_of(rig: &mut Rig, colour: u32) -> u32 {
    let p = [[-1.0f32, 1.0, 0.5], [-1.0, -1.0, 0.5], [1.0, 1.0, 0.5]];
    let mut vertices = Vec::new();
    for p in p {
        vertices.extend(words(&[p[0], p[1], p[2], 0.0, 0.0, 1.0]));
        vertices.push(colour);
    }
    rig.triangles(XYZ | NORMAL | DIFFUSE, &vertices);
    rig.px(8, 8)
}

#[test]
fn the_back_of_a_triangle_has_a_material_of_its_own() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    rig.states(&[
        (RS_CULLMODE, 1),
        (RS_TWO_SIDED_LIGHTING, 1),
        (RS_COLORVERTEX, 0),
    ]);
    rig.light(0, &sun());
    // `SET_MATERIAL` sets the material of both sides.
    rig.material(&matte([1.0; 4]));
    assert_eq!(back_of(&mut rig, 0), 0x00FF_FFFF);
    let red = matte([1.0, 0.0, 0.0, 1.0]);
    assert_eq!(rig.run(|w| encode::back_material(w, &red)), 0);
    assert_eq!(back_of(&mut rig, 0), 0x00FF_0000);
    // The front keeps its own.
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x00FF_FFFF);
    rig.material(&matte([0.0, 1.0, 0.0, 1.0]));
    assert_eq!(back_of(&mut rig, 0), 0x0000_FF00);
}

#[test]
fn the_ambient_light_is_the_states_colour_or_set_ambients_numbers() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    rig.material(&Material {
        ambient: [1.0; 4],
        ..matte([0.0; 4])
    });
    rig.states(&[(RS_AMBIENT, 0x0080_4020)]);
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x0080_4020);
    // Numbers, which may be past what a byte holds.
    assert_eq!(rig.run(|w| encode::ambient(w, &[0.25, 0.5, 2.0])), 0);
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x0040_80FF);
    // The state written again holds, with the value it had.
    rig.states(&[(RS_AMBIENT, 0x0080_4020)]);
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x0080_4020);
}

#[test]
fn single_colour_adds_the_highlight_to_the_diffuse_colour() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    rig.states(&[
        (RS_SPECULARENABLE, 0),
        (RS_LOCALVIEWER, 0),
        (RS_SINGLE_COLOUR, 1),
    ]);
    rig.material(&Material {
        specular: [0.5, 0.25, 0.0, 1.0],
        power: 10.0,
        ..matte([0.0, 0.0, 0.25, 1.0])
    });
    rig.light(0, &sun());
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x0080_4040);
    // The specular colour is black, so the rasteriser adds nothing
    // more where it adds that.
    rig.states(&[(RS_SPECULARENABLE, 1)]);
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x0080_4040);
    rig.states(&[(RS_SINGLE_COLOUR, 0)]);
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x0080_4040);
    rig.states(&[(RS_SPECULARENABLE, 0)]);
    assert_eq!(rig.lit([0.0, 0.0, -1.0]), 0x0000_0040);
}

#[test]
fn colorvertex_faces_names_the_sides_that_take_the_vertexs_colour() {
    let mut rig = Rig::new(Format::X8R8G8B8, None);
    rig.states(&[(RS_CULLMODE, 1), (RS_TWO_SIDED_LIGHTING, 1)]);
    rig.material(&matte([1.0, 0.0, 0.0, 1.0]));
    rig.light(0, &sun());
    // A clockwise triangle that faces the sun, with blue vertices.
    let front = |rig: &mut Rig| {
        let p = [[-1.0f32, 1.0, 0.5], [1.0, 1.0, 0.5], [-1.0, -1.0, 0.5]];
        let mut vertices = Vec::new();
        for p in p {
            vertices.extend(words(&[p[0], p[1], p[2], 0.0, 0.0, -1.0]));
            vertices.push(0xFF00_00FF);
        }
        rig.triangles(XYZ | NORMAL | DIFFUSE, &vertices);
        rig.px(8, 8)
    };
    // Both sides after a reset.
    assert_eq!(front(&mut rig), 0x0000_00FF);
    assert_eq!(back_of(&mut rig, 0xFF00_00FF), 0x0000_00FF);
    rig.states(&[(RS_COLORVERTEX_FACES, 2)]);
    assert_eq!(front(&mut rig), 0x00FF_0000);
    assert_eq!(back_of(&mut rig, 0xFF00_00FF), 0x0000_00FF);
    rig.states(&[(RS_COLORVERTEX_FACES, 1)]);
    assert_eq!(front(&mut rig), 0x0000_00FF);
    assert_eq!(back_of(&mut rig, 0xFF00_00FF), 0x00FF_0000);
}

//! Which of OpenGL's draws the card transforms and lights, and the
//! card's state for them.
//!
//! A ToadForce transforms and lights vertices itself, as a GeForce
//! does, and its OpenGL driver hands it the vertices as the program
//! gave them, with the matrices, the lights and the materials as the
//! card's state. `state_of` and `checks` are the one place that says
//! whether a draw goes that way: they give the card's lighting state
//! for the draw (`hal::Gl`), or why the processor transforms and
//! lights it (`Why`), as it does every draw on a 3dfx card
//! (`shade.rs`, and Direct3D's pipeline).
//!
//! The card's lighting is Direct3D's with what OpenGL adds to it: a
//! material for the back, the ambient light as numbers, the specular
//! light in the vertex's one colour, and the faces glColorMaterial
//! names. Lights go to the card in eye coordinates, as OpenGL keeps
//! them: the modelview matrix is the card's world matrix, and its view
//! matrix turns z round (`VIEW`), since the card's eye looks along z
//! and OpenGL's against it. The projection undoes that turn first.
//!
//! A draw stays on the processor where the card's result could differ
//! from `shade.rs` by more than rounding: each reason is a `Why`, and
//! `/api/win32/ddraw` counts the draws of each.

use super::super::ddraw::d3d::hal::toadforce::{Gl, GlLighting, can_light};
use super::super::ddraw::d3d::pipeline::{IDENTITY, Matrix};
use super::super::ddraw::stamp::Stamped;
use super::light::{
    self, GL_AMBIENT, GL_DIFFUSE, GL_EMISSION, GL_SPECULAR, LIGHTS, Light, Lighting, Material,
};
use super::shade;
use super::state::{GL_BACK, GL_CW, GL_FILL, GL_FLAT, GL_FRONT};
use super::{CAP_COLOR_MATERIAL, CAP_LIGHTING, CAP_NORMALIZE, Context, ImmVertex};
use crate::toadforce::level1::{self, states};

/// The card's view matrix for OpenGL's draws: eye coordinates with z
/// turned round.
pub const VIEW: Matrix = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, -1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

/// The projection that follows `VIEW`: `projection` after z is turned
/// back.
pub fn projection(projection: &Matrix) -> Matrix {
    let mut out = *projection;
    for n in &mut out[8..12] {
        *n = -*n;
    }
    out
}

/// Why the processor transforms and lights a draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The card doesn't transform: a 3dfx card.
    NoCard,
    /// `RUST_DOS_TOADFORCE_TNL=pipeline` asks for it.
    Asked,
    /// A matrix or a light's or material's number that is no number,
    /// or a depth range the card's viewport doesn't take.
    Numbers,
    /// Lit through a modelview matrix that has no inverse, where
    /// OpenGL leaves normals as they are and the card has none, or
    /// one that gives eye coordinates a w other than 1.
    Modelview,
    /// A light the card has no like of: a spot at infinity, or one at
    /// a point with no attenuation factor above 0.
    Light,
    /// A specular material with a shininess of 0, which shines where
    /// the half vector is behind the surface: 0 to the power 0 is 1.
    Shininess,
    /// glMaterial between glBegin and glEnd: the vertices before it
    /// were lit with the material it replaced.
    Material,
    /// A flat primitive whose colour comes from a vertex that isn't
    /// one of its triangle's: a quad's first triangle.
    Flat,
    /// Two-sided lighting of quads or polygons, whose side OpenGL
    /// chooses for the whole polygon and the card for each triangle.
    Sides,
    /// Quads or polygons drawn as lines or points, without the
    /// diagonals that cut them into triangles.
    Edges,
}

impl Why {
    pub const ALL: [Why; 10] = [
        Why::NoCard,
        Why::Asked,
        Why::Numbers,
        Why::Modelview,
        Why::Light,
        Why::Shininess,
        Why::Material,
        Why::Flat,
        Why::Sides,
        Why::Edges,
    ];

    /// The reason's name in `/api/win32/ddraw`.
    pub fn name(self) -> &'static str {
        match self {
            Why::NoCard => "no_card",
            Why::Asked => "asked",
            Why::Numbers => "numbers",
            Why::Modelview => "modelview",
            Why::Light => "light",
            Why::Shininess => "shininess",
            Why::Material => "material",
            Why::Flat => "flat",
            Why::Sides => "sides",
            Why::Edges => "edges",
        }
    }
}

/// The counts of OpenGL's draws by where they were transformed: those
/// the card took and their vertices, the batches of them that weren't
/// drawn at the flush (`draw::flush_waiting`: none, unless `checks`
/// and the HAL disagree or the target isn't the card's to draw in;
/// their draws are counted as the card's too), and then the
/// processor's by `Why::ALL`.
pub const COUNTS: usize = 3 + Why::ALL.len();
pub const COUNT_CARD: usize = 0;
pub const COUNT_VERTICES: usize = 1;
pub const COUNT_REFUSED: usize = 2;

/// The names of the counts, in their order.
pub fn count_names() -> Vec<&'static str> {
    let mut names = vec!["card", "card_vertices", "refused"];
    names.extend(Why::ALL.iter().map(|why| why.name()));
    names
}

/// The place of the count of the draws the processor took for `why`.
pub fn count_of(why: Why) -> usize {
    3 + Why::ALL.iter().position(|w| *w == why).unwrap_or(0)
}

fn finite(numbers: &[f32]) -> bool {
    numbers.iter().all(|n| n.is_finite())
}

/// Whether the card turns normals as OpenGL does through `m`, and
/// takes the eye coordinates OpenGL lights by: its upper left three by
/// three has an inverse, by the card's arithmetic and by `shade`'s,
/// and a vertex with a w of 1 keeps it.
fn lights_through(m: &Matrix) -> bool {
    let row = |r: usize| [m[r * 4], m[r * 4 + 1], m[r * 4 + 2]];
    let (r0, r1, r2) = (row(0), row(1), row(2));
    let cross = [
        r1[1] * r2[2] - r1[2] * r2[1],
        r1[2] * r2[0] - r1[0] * r2[2],
        r1[0] * r2[1] - r1[1] * r2[0],
    ];
    let det = r0[0] * cross[0] + r0[1] * cross[1] + r0[2] * cross[2];
    det != 0.0
        && det.is_finite()
        && shade::normal_matrix(m).is_some()
        && [m[3], m[7], m[11], m[15]] == [0.0, 0.0, 0.0, 1.0]
}

/// The light `l` as the card has one, in eye coordinates.
fn card_light(l: &Light) -> Result<level1::Light, Why> {
    let p = l.position;
    let spot = l.cutoff != 180.0;
    let base = level1::Light {
        diffuse: l.diffuse,
        specular: l.specular,
        ambient: l.ambient,
        range: f32::MAX,
        attenuation: [1.0, 0.0, 0.0],
        spot_model: 1,
        ..level1::Light::default()
    };
    if p[3] == 0.0 {
        // A light at infinity shines from where its position points,
        // and isn't attenuated.
        if spot {
            return Err(Why::Light);
        }
        return Ok(level1::Light {
            kind: level1::LIGHT_DIRECTIONAL,
            direction: [-p[0], -p[1], -p[2]],
            ..base
        });
    }
    // The card takes a sum of 0 for no attenuation, where OpenGL
    // divides by it.
    if !l.attenuation.iter().any(|&k| k > 0.0) {
        return Err(Why::Light);
    }
    let at = level1::Light {
        kind: level1::LIGHT_POINT,
        position: [p[0] / p[3], p[1] / p[3], p[2] / p[3]],
        attenuation: l.attenuation,
        ..base
    };
    if !spot {
        return Ok(at);
    }
    Ok(level1::Light {
        kind: level1::LIGHT_SPOT,
        direction: l.direction,
        falloff: l.exponent,
        // The whole angle of the cone, in radians.
        phi: 2.0 * l.cutoff.to_radians(),
        ..at
    })
}

/// The material of `side` as the card has it. A colour the vertices
/// give (`tracked`) is left out: the card doesn't read it, and the
/// material then stays the same from one glColor to the next.
fn card_material(l: &Lighting, side: usize, tracked: bool) -> level1::Material {
    let m: &Material = &l.materials[side];
    let of = |p: u32, colour: [f32; 4]| {
        if tracked && l.tracks(side, p) {
            [0.0; 4]
        } else {
            colour
        }
    };
    level1::Material {
        diffuse: of(GL_DIFFUSE, m.diffuse),
        ambient: of(GL_AMBIENT, m.ambient),
        specular: of(GL_SPECULAR, m.specular),
        emissive: of(GL_EMISSION, m.emission),
        power: m.shininess,
    }
}

/// The card's lighting state for the context `c`, or why the card has
/// no state that lights as `shade` does.
fn lighting(c: &Context) -> Result<Gl, Why> {
    let l = &c.lighting;
    let modelview = c.modelview.last().copied().unwrap_or(IDENTITY);
    if !lights_through(&modelview) {
        return Err(Why::Modelview);
    }
    let tracked = c.caps & CAP_COLOR_MATERIAL != 0;
    let sides = if l.two_side { 2 } else { 1 };
    for side in 0..sides {
        let m = &l.materials[side];
        let specular = (tracked && l.tracks(side, GL_SPECULAR)) || m.specular[..3] != [0.0; 3];
        if specular && m.shininess == 0.0 {
            return Err(Why::Shininess);
        }
    }
    let mut lights = Vec::new();
    for i in (0..LIGHTS).filter(|i| l.enabled >> i & 1 != 0) {
        lights.push(card_light(&l.lights[i])?);
    }
    if lights.len() > level1::LIGHTS {
        return Err(Why::Light);
    }
    // The colours the vertices give: Direct3D's material sources, 1
    // for the vertex's colour.
    let source = |p: u32| (tracked && light::tracked(l.color_mode).contains(&p)) as u32;
    let faces = match l.color_face {
        GL_FRONT => 1,
        GL_BACK => 2,
        _ => 3,
    };
    // The back of a triangle is the side that is clockwise with
    // counterclockwise fronts (2), and the other way round (1).
    let two_sided = match (l.two_side, c.front_face == GL_CW) {
        (false, _) => 0,
        (true, false) => 2,
        (true, true) => 1,
    };
    let states = vec![
        (states::RS_LIGHTING, 1),
        (states::RS_COLORVERTEX, tracked as u32),
        (states::RS_LOCALVIEWER, l.local_viewer as u32),
        (
            states::RS_NORMALIZENORMALS,
            (c.caps & CAP_NORMALIZE != 0) as u32,
        ),
        (states::RS_DIFFUSEMATERIALSOURCE, source(GL_DIFFUSE)),
        (states::RS_SPECULARMATERIALSOURCE, source(GL_SPECULAR)),
        (states::RS_AMBIENTMATERIALSOURCE, source(GL_AMBIENT)),
        (states::RS_EMISSIVEMATERIALSOURCE, source(GL_EMISSION)),
        (states::RS_FLAT_VERTEX, 1),
        (states::RS_TWO_SIDED_LIGHTING, two_sided),
        (states::RS_SINGLE_COLOUR, 1),
        (states::RS_COLORVERTEX_FACES, faces),
    ];
    let gl = Gl {
        states,
        lighting: Some(GlLighting {
            material: card_material(l, 0, tracked),
            back: l.two_side.then(|| card_material(l, 1, tracked)),
            ambient: [l.ambient[0], l.ambient[1], l.ambient[2]],
            lights,
        }),
    };
    Ok(gl)
}

/// What a context's state gives each of its draws: the card's state,
/// or why the processor transforms and lights them.
pub type State = Result<Gl, Why>;

/// The card's state for the draws of the context `c` as it is, kept
/// for as long as its settings and its lighting have their stamps: a
/// draw that follows another with nothing set between them costs the
/// two comparisons.
pub fn state_of(c: &mut Context) -> &State {
    let stamps = (c.stamp(), Stamped::stamp(&c.lighting));
    if c.card.as_ref().is_none_or(|(a, b, _)| (*a, *b) != stamps) {
        let made = state(c);
        c.card = Some((stamps.0, stamps.1, made));
    }
    &c.card.as_ref().unwrap().2
}

/// The card's state for the context `c`, made anew (`state_of`).
fn state(c: &Context) -> State {
    let modelview = c.modelview.last().copied().unwrap_or(IDENTITY);
    let projection = c.projection.last().copied().unwrap_or(IDENTITY);
    // The card's depths are 0 to 1, as its viewport takes them
    // (`hal::toadforce::can_transform`).
    let (near, depth) = (c.depth_range[0], c.depth_range[1] - c.depth_range[0]);
    let depths = (0.0..=1.0).contains(&near) && (0.0..=1.0).contains(&(near + depth));
    if !finite(&modelview) || !finite(&projection) || !depths || depth < 0.0 {
        return Err(Why::Numbers);
    }
    if c.caps & CAP_LIGHTING == 0 {
        // With the colours they have, and a w of 0 taken for 1 as the
        // pipeline takes it.
        return Ok(Gl {
            states: vec![(states::RS_FLAT_VERTEX, 1)],
            lighting: None,
        });
    }
    let gl = lighting(c)?;
    if !can_light(&gl) {
        return Err(Why::Numbers);
    }
    Ok(gl)
}

/// Whether the card draws as the processor would the `prims`
/// primitives (`draw::primitives`) of a draw, which come `per`
/// triangles to a polygon, of the vertices `raw`, in a context `c`
/// whose state the card takes (`state_of`). An error is why the
/// processor transforms and lights the draw.
///
/// It looks at no primitive and at one vertex: what it asks follows
/// from the kind of primitive. A quad's two triangles (`per` 2) and a
/// polygon's (`per` usize::MAX, more than one) have inner edges, and
/// OpenGL chooses one side for all of them. Flat, a quad takes the
/// colour of its last vertex, which its first triangle doesn't have,
/// and a polygon that of its first, which all of its triangles have.
///
/// What the card does otherwise than OpenGL without a check here,
/// since each would cost every lit draw a pass over its vertices and
/// no game was seen to draw them: a colour glColorMaterial takes is
/// rounded to a byte, a normal longer than 1 has a highlight's cosine
/// above 1 taken for 1, and a lit vertex with a w of 0 is at its x, y
/// and z.
pub fn checks(c: &Context, per: usize, prims: usize, raw: &[ImmVertex]) -> Result<(), Why> {
    let quads = per == 2;
    let polygons = quads || (per == usize::MAX && prims > 1);
    if quads && c.shade_model == GL_FLAT {
        return Err(Why::Flat);
    }
    if polygons && c.polygon_mode != GL_FILL {
        return Err(Why::Edges);
    }
    if c.caps & CAP_LIGHTING == 0 {
        return Ok(());
    }
    if polygons && c.lighting.two_side {
        return Err(Why::Sides);
    }
    // A vertex lit already was given before a glMaterial of its
    // glBegin. That lights every vertex given so far
    // (`draw::light_gathered`), so the first is one of them.
    if raw.first().is_some_and(|v| v.lit.is_some()) {
        return Err(Why::Material);
    }
    Ok(())
}

/// The card's state for a draw of the context `c` (`state_of` and
/// `checks` in one, without the context's copy), or why the processor
/// transforms and lights it.
#[cfg(test)]
pub fn decide(
    c: &Context,
    per: usize,
    prims: &[super::super::ddraw::d3d::pipeline::Prim],
    raw: &[ImmVertex],
) -> Result<Gl, Why> {
    let gl = state(c)?;
    checks(c, per, prims.len(), raw)?;
    Ok(gl)
}

#[cfg(test)]
mod tests {
    use super::super::super::ddraw::d3d::pipeline::Prim;
    use super::*;

    fn context() -> Context {
        let mut c = super::super::new_context(0, 0, 1);
        c.caps |= CAP_LIGHTING;
        c.lighting.enabled = 1;
        c
    }

    fn vertex(color: [f32; 4]) -> ImmVertex {
        ImmVertex {
            position: [0.0, 0.0, 0.0, 1.0],
            normal: [0.0, 0.0, 1.0],
            color,
            ..ImmVertex::default()
        }
    }

    fn triangle(c: &Context) -> Result<Gl, Why> {
        let raw = [vertex([1.0; 4]); 3];
        decide(c, 1, &[Prim::Triangle([0, 1, 2], 2, 7)], &raw)
    }

    #[test]
    fn a_new_contexts_light_goes_to_the_card_from_where_it_points() {
        let gl = triangle(&context()).expect("the card lights it");
        let l = gl.lighting.expect("lit");
        assert_eq!(l.lights.len(), 1);
        assert_eq!(l.lights[0].kind, level1::LIGHT_DIRECTIONAL);
        // Light 0 is at (0, 0, 1, 0): it shines along -z.
        assert_eq!(l.lights[0].direction, [0.0, 0.0, -1.0]);
        assert_eq!(l.ambient, [0.2; 3]);
        assert_eq!(l.material.diffuse, [0.8, 0.8, 0.8, 1.0]);
        assert_eq!(l.back, None);
        assert!(gl.states.contains(&(states::RS_SINGLE_COLOUR, 1)));
        assert!(gl.states.contains(&(states::RS_TWO_SIDED_LIGHTING, 0)));
    }

    #[test]
    fn an_unlit_draw_has_only_the_flat_vertex() {
        let mut c = context();
        c.caps &= !CAP_LIGHTING;
        // A modelview matrix lighting couldn't go through.
        c.modelview = vec![[0.0; 16]];
        let gl = triangle(&c).expect("the card transforms it");
        assert_eq!(gl.states, [(states::RS_FLAT_VERTEX, 1)]);
        assert_eq!(gl.lighting, None);
    }

    #[test]
    fn lights_at_points_and_spots_are_the_cards() {
        let mut c = context();
        let l = &mut c.lighting.lights[0];
        l.position = [2.0, 4.0, 6.0, 2.0];
        l.attenuation = [0.0, 0.5, 0.0];
        let point = triangle(&c).unwrap().lighting.unwrap().lights[0];
        assert_eq!(point.kind, level1::LIGHT_POINT);
        assert_eq!(point.position, [1.0, 2.0, 3.0]);
        assert_eq!(point.attenuation, [0.0, 0.5, 0.0]);
        let l = &mut c.lighting.lights[0];
        l.cutoff = 45.0;
        l.exponent = 3.0;
        l.direction = [0.0, 1.0, 0.0];
        let spot = triangle(&c).unwrap().lighting.unwrap().lights[0];
        assert_eq!(spot.kind, level1::LIGHT_SPOT);
        assert_eq!((spot.spot_model, spot.falloff), (1, 3.0));
        assert!((spot.phi - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(spot.direction, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn what_the_card_lights_otherwise_stays_on_the_processor() {
        let why = |change: fn(&mut Context)| {
            let mut c = context();
            change(&mut c);
            triangle(&c).expect_err("the processor's")
        };
        assert_eq!(why(|c| c.modelview = vec![[0.0; 16]]), Why::Modelview);
        assert_eq!(
            why(|c| {
                let mut m = IDENTITY;
                m[15] = 2.0;
                c.modelview = vec![m];
            }),
            Why::Modelview
        );
        assert_eq!(why(|c| c.lighting.lights[0].cutoff = 30.0), Why::Light);
        assert_eq!(
            why(|c| {
                c.lighting.lights[0].position = [0.0, 0.0, 1.0, 1.0];
                c.lighting.lights[0].attenuation = [0.0; 3];
            }),
            Why::Light
        );
        assert_eq!(
            why(|c| c.lighting.materials[0].specular = [1.0; 4]),
            Why::Shininess
        );
        assert_eq!(why(|c| c.depth_range = [1.0, 0.0]), Why::Numbers);
        assert_eq!(why(|c| c.projection = vec![[f32::NAN; 16]]), Why::Numbers);
        assert_eq!(
            why(|c| c.lighting.materials[0].diffuse[0] = f32::INFINITY),
            Why::Numbers
        );
    }

    #[test]
    fn a_back_material_matters_only_with_two_sides() {
        let mut c = context();
        c.lighting.materials[1].specular = [1.0; 4];
        assert!(triangle(&c).is_ok());
        c.lighting.two_side = true;
        assert_eq!(triangle(&c), Err(Why::Shininess));
        c.lighting.materials[1].shininess = 4.0;
        let gl = triangle(&c).unwrap();
        assert!(gl.states.contains(&(states::RS_TWO_SIDED_LIGHTING, 2)));
        assert_eq!(gl.lighting.unwrap().back.unwrap().power, 4.0);
        c.front_face = GL_CW;
        let gl = triangle(&c).unwrap();
        assert!(gl.states.contains(&(states::RS_TWO_SIDED_LIGHTING, 1)));
        // A quad's two triangles show one side in OpenGL.
        let raw = [vertex([1.0; 4]); 4];
        let quad = [
            Prim::Triangle([0, 1, 2], 3, 3),
            Prim::Triangle([0, 2, 3], 3, 6),
        ];
        assert_eq!(decide(&c, 2, &quad, &raw), Err(Why::Sides));
    }

    #[test]
    fn a_tracked_colour_goes_with_the_vertices_when_bytes_hold_it() {
        let mut c = context();
        c.caps |= CAP_COLOR_MATERIAL;
        c.lighting.materials[0].ambient = [0.3; 4];
        c.lighting.materials[0].emission = [0.1; 4];
        let prims = [Prim::Triangle([0, 1, 2], 2, 7)];
        let bytes = [vertex([1.0, 128.0 / 255.0, 0.0, 7.0 / 255.0]); 3];
        let gl = decide(&c, 1, &prims, &bytes).unwrap();
        assert!(gl.states.contains(&(states::RS_COLORVERTEX, 1)));
        assert!(gl.states.contains(&(states::RS_DIFFUSEMATERIALSOURCE, 1)));
        assert!(gl.states.contains(&(states::RS_AMBIENTMATERIALSOURCE, 1)));
        assert!(gl.states.contains(&(states::RS_EMISSIVEMATERIALSOURCE, 0)));
        assert!(gl.states.contains(&(states::RS_COLORVERTEX_FACES, 3)));
        // The tracked colours are the vertices', the others the
        // material's.
        let m = gl.lighting.unwrap().material;
        assert_eq!((m.diffuse, m.ambient), ([0.0; 4], [0.0; 4]));
        assert_eq!(m.emissive, [0.1; 4]);
        // A colour between two bytes goes to the card too, which
        // rounds it.
        let floats = [vertex([0.5, 0.5, 0.5, 1.0]); 3];
        assert!(decide(&c, 1, &prims, &floats).is_ok());
    }

    #[test]
    fn primitives_the_card_has_no_like_of_stay_on_the_processor() {
        let mut c = context();
        c.caps &= !CAP_LIGHTING;
        let raw = [vertex([1.0; 4]); 4];
        let quad = [
            Prim::Triangle([0, 1, 2], 3, 3),
            Prim::Triangle([0, 2, 3], 3, 6),
        ];
        assert!(decide(&c, 2, &quad, &raw).is_ok());
        c.shade_model = GL_FLAT;
        assert_eq!(decide(&c, 2, &quad, &raw), Err(Why::Flat));
        // A polygon's colour is its first vertex's, one of each
        // triangle's.
        let polygon = [
            Prim::Triangle([0, 1, 2], 0, 3),
            Prim::Triangle([0, 2, 3], 0, 6),
        ];
        assert!(decide(&c, usize::MAX, &polygon, &raw).is_ok());
        c.polygon_mode = super::super::state::GL_LINE;
        assert_eq!(decide(&c, usize::MAX, &polygon, &raw), Err(Why::Edges));
        assert!(decide(&c, 1, &[Prim::Triangle([0, 1, 2], 2, 7)], &raw).is_ok());
    }

    /// `checks` goes by the kind of primitive alone. For every kind and
    /// count of vertices, what it says is what the primitives
    /// `draw::primitives` makes of them give, looked at one by one.
    #[test]
    fn the_kind_of_primitive_says_what_its_triangles_would() {
        use super::super::draw::{polygon_triangles, primitives};
        for kind in 0..=9 {
            for n in 1..=10u32 {
                let order: Vec<u32> = (0..n).collect();
                let prims = primitives(kind, &order);
                if prims.is_empty() {
                    continue;
                }
                let raw = vec![vertex([1.0; 4]); n as usize];
                let per = polygon_triangles(kind);
                let flat = prims.iter().any(|p| match *p {
                    Prim::Triangle(t, provoking, _) => !t.contains(&provoking),
                    Prim::Line(_, b, provoking) => provoking != b,
                    Prim::Point(_) => false,
                });
                let inner = prims
                    .iter()
                    .any(|p| matches!(*p, Prim::Triangle(_, _, edges) if edges != 7));
                let polygons = per != 1 && prims.len() != 1;
                let mut c = context();
                c.shade_model = GL_FLAT;
                let why = decide(&c, per, &prims, &raw).err();
                assert_eq!(why == Some(Why::Flat), flat, "kind {kind}, {n}");
                c.shade_model = super::super::state::GL_SMOOTH;
                c.polygon_mode = super::super::state::GL_LINE;
                let why = decide(&c, per, &prims, &raw).err();
                assert_eq!(why == Some(Why::Edges), inner, "kind {kind}, {n}");
                c.polygon_mode = GL_FILL;
                c.lighting.two_side = true;
                let why = decide(&c, per, &prims, &raw).err();
                assert_eq!(why == Some(Why::Sides), polygons, "kind {kind}, {n}");
            }
        }
    }

    #[test]
    fn vertices_lit_already_stay_on_the_processor() {
        let c = context();
        let prims = [Prim::Triangle([0, 1, 2], 2, 7)];
        let mut raw = [vertex([1.0; 4]); 3];
        assert!(decide(&c, 1, &prims, &raw).is_ok());
        // A glMaterial after the first two vertices lit them
        // (`draw::light_gathered`).
        raw[0].lit = Some([0, 0]);
        raw[1].lit = Some([0, 0]);
        assert_eq!(decide(&c, 1, &prims, &raw), Err(Why::Material));
    }

    #[test]
    fn the_view_and_the_projection_undo_each_other() {
        let p: Matrix = std::array::from_fn(|i| i as f32 + 1.0);
        let turned = projection(&p);
        // Row vectors: a vertex times the view, times the projection.
        let both: Matrix = std::array::from_fn(|i| {
            let (row, col) = (i / 4, i % 4);
            (0..4)
                .map(|k| VIEW[row * 4 + k] * turned[k * 4 + col])
                .sum()
        });
        assert_eq!(both, p);
    }

    #[test]
    fn every_reason_has_a_count_of_its_own() {
        let names = count_names();
        assert_eq!(names.len(), COUNTS);
        for why in Why::ALL {
            assert_eq!(names[count_of(why)], why.name());
        }
    }

    /// Numbers from 0 to 1 that come out the same on every run.
    struct Dice(u64);

    impl Dice {
        fn roll(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }

        fn between(&mut self, from: f32, to: f32) -> f32 {
            from + (to - from) * self.roll()
        }

        fn chance(&mut self, of: f32) -> bool {
            self.roll() < of
        }

        fn colour(&mut self) -> [f32; 4] {
            [self.roll(), self.roll(), self.roll(), self.roll()]
        }

        /// A colour a byte holds for each channel.
        fn bytes(&mut self) -> [f32; 4] {
            self.colour().map(|x| (x * 255.0).round() / 255.0)
        }

        fn vector(&mut self, size: f32) -> [f32; 3] {
            [
                self.between(-size, size),
                self.between(-size, size),
                self.between(-size, size),
            ]
        }
    }

    /// A context with lights, materials and a modelview matrix the dice
    /// give, of the kinds `decide` hands the card.
    fn rolled(d: &mut Dice) -> Context {
        use super::super::matrix::mul;
        let mut c = context();
        // A turn about each axis, a scale and a move.
        let turn = |a: usize, b: usize, angle: f32| {
            let mut m = IDENTITY;
            let (s, co) = angle.sin_cos();
            (m[a * 4 + a], m[b * 4 + b]) = (co, co);
            (m[a * 4 + b], m[b * 4 + a]) = (s, -s);
            m
        };
        let mut m = mul(
            &turn(0, 1, d.between(0.0, 6.0)),
            &turn(1, 2, d.between(0.0, 6.0)),
        );
        m = mul(&m, &turn(0, 2, d.between(0.0, 6.0)));
        let mut scale = IDENTITY;
        (scale[0], scale[5], scale[10]) = (
            d.between(0.5, 2.0),
            d.between(0.5, 2.0),
            d.between(0.5, 2.0),
        );
        m = mul(&m, &scale);
        let by = d.vector(3.0);
        (m[12], m[13], m[14]) = (by[0], by[1], by[2] - 6.0);
        c.modelview = vec![m];
        if d.chance(0.5) {
            c.caps |= CAP_NORMALIZE;
        }
        let tracked = d.chance(0.5);
        if tracked {
            c.caps |= CAP_COLOR_MATERIAL;
        }
        let l = &mut c.lighting;
        l.enabled = 0;
        for i in 0..1 + (d.roll() * 3.0) as usize {
            l.enabled |= 1 << i;
            let light = &mut l.lights[i];
            light.ambient = d.colour().map(|x| x * 0.2);
            light.diffuse = d.colour();
            light.specular = d.colour();
            let p = d.vector(8.0);
            if d.chance(0.4) {
                light.position = [p[0], p[1], p[2], 0.0];
                continue;
            }
            let w = d.between(0.5, 2.0);
            light.position = [p[0] * w, p[1] * w, p[2] * w, w];
            light.attenuation = [d.roll(), d.roll() * 0.2, d.roll() * 0.05];
            if d.chance(0.5) {
                // Half of space now and then: the widest cone.
                light.cutoff = if d.chance(0.15) {
                    90.0
                } else {
                    d.between(10.0, 85.0)
                };
                light.exponent = if d.chance(0.3) {
                    0.0
                } else {
                    d.between(0.0, 8.0)
                };
                light.direction = d.vector(1.0);
            }
        }
        l.ambient = d.colour();
        l.local_viewer = d.chance(0.5);
        l.two_side = d.chance(0.5);
        for m in &mut l.materials {
            m.ambient = d.colour();
            m.diffuse = d.colour();
            m.emission = d.colour().map(|x| x * 0.2);
            if d.chance(0.6) {
                m.specular = d.colour();
                m.shininess = d.between(1.0, 64.0);
            }
        }
        if tracked {
            l.color_face = [GL_FRONT, GL_BACK, 0x0408][(d.roll() * 3.0) as usize];
            let modes = [
                light::GL_AMBIENT_AND_DIFFUSE,
                GL_AMBIENT,
                GL_DIFFUSE,
                GL_SPECULAR,
                GL_EMISSION,
            ];
            l.color_mode = modes[(d.roll() * 5.0) as usize];
            for side in 0..2 {
                if l.tracks(side, GL_SPECULAR) && l.materials[side].shininess == 0.0 {
                    l.materials[side].shininess = 8.0;
                }
            }
        }
        if d.chance(0.3) {
            c.front_face = GL_CW;
        }
        c
    }

    /// The front and back colours the card gives the vertex `v` with
    /// the state `gl` of the context `c`, as OpenGL's layer puts both
    /// on the card.
    fn card_colours(c: &Context, gl: &Gl, v: &ImmVertex) -> [[f32; 4]; 2] {
        use crate::toadforce::raster::geometry::Setup;
        use crate::toadforce::raster::vertex::Fvf;
        let mut l = level1::Level1::default();
        l.transforms[level1::TRANSFORM_WORLD as usize] = *c.modelview.last().unwrap();
        l.transforms[level1::TRANSFORM_VIEW as usize] = VIEW;
        l.transforms[level1::TRANSFORM_PROJECTION as usize] = projection(&IDENTITY);
        for &(id, value) in &gl.states {
            l.render[id as usize] = value;
        }
        let lighting = gl.lighting.as_ref().unwrap();
        l.material = lighting.material;
        l.material_back = lighting.back.unwrap_or(lighting.material);
        l.ambient = lighting.ambient;
        for (slot, light) in lighting.lights.iter().enumerate() {
            l.lights[slot] = *light;
            l.lights_on |= 1 << slot;
        }
        // The position, the normal and the colour.
        let fvf = Fvf::parse(0x002 | 0x010 | 0x040).unwrap();
        let mut words: Vec<u32> = v.position[..3].iter().map(|n| n.to_bits()).collect();
        words.extend(v.normal.map(f32::to_bits));
        words.push(super::super::super::ddraw::d3d::pipeline::color(v.color));
        let out = Setup::new(&l, &[], &fvf).vertex(|at| words[at]);
        let back = if l.render[states::RS_TWO_SIDED_LIGHTING as usize] != 0 {
            out.back[0]
        } else {
            out.diffuse
        };
        [out.diffuse, back]
    }

    /// Whether the vertex at `eye` is so near the edge of a spot's
    /// cone that the card and `shade` may put it on different sides.
    fn on_a_cones_edge(l: &Lighting, eye: [f32; 4]) -> bool {
        (0..LIGHTS).any(|i| {
            let light = &l.lights[i];
            if l.enabled >> i & 1 == 0 || light.cutoff == 180.0 {
                return false;
            }
            let p = light.position;
            let to: [f32; 3] = std::array::from_fn(|k| eye[k] - p[k] / p[3]);
            let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            let d = light.direction;
            let cos = (to[0] * d[0] + to[1] * d[1] + to[2] * d[2]) / (len(to) * len(d));
            (cos - light.cutoff.to_radians().cos()).abs() < 1e-3
        })
    }

    /// The card's colours are `shade`'s for the states `decide` hands
    /// it, to a step of a byte: the card raises to a power and takes
    /// cosines within a thousandth and adds a vertex's lights in
    /// another order.
    #[test]
    fn the_card_lights_as_the_processor_does() {
        use super::super::matrix;
        use super::super::shade::Input;
        let mut d = Dice(1078);
        let (mut compared, mut worst) = (0, 0);
        for _ in 0..3000 {
            let c = rolled(&mut d);
            let tracked = c.caps & CAP_COLOR_MATERIAL != 0;
            let v = ImmVertex {
                position: [
                    d.between(-2.0, 2.0),
                    d.between(-2.0, 2.0),
                    d.between(-2.0, 2.0),
                    1.0,
                ],
                normal: d.vector(1.0),
                color: d.bytes(),
                ..ImmVertex::default()
            };
            let prims = [Prim::Triangle([0, 1, 2], 2, 7)];
            let gl = match decide(&c, 1, &prims, &[v; 3]) {
                Ok(gl) => gl,
                Err(why) => panic!("the dice gave a state the card doesn't light: {why:?}"),
            };
            let modelview = c.modelview[0];
            let eye = matrix::apply(&modelview, v.position);
            // Left out, as the card's rule is another there
            // (`checks`): a vertex within a thousandth of a cone's
            // edge, which the two may put on different sides.
            if on_a_cones_edge(&c.lighting, eye) {
                continue;
            }
            let normals = shade::normal_matrix(&modelview);
            // And a normal longer than 1 under a specular material
            // without GL_NORMALIZE, whose highlight's cosine the card
            // takes for 1 where OpenGL raises it to the shininess.
            let long = shade::eye_normal(normals.as_ref(), v.normal, false);
            let shines = (0..2).any(|side| {
                (tracked && c.lighting.tracks(side, GL_SPECULAR))
                    || c.lighting.materials[side].specular[..3] != [0.0; 3]
            });
            if shines
                && c.caps & CAP_NORMALIZE == 0
                && long.iter().map(|n| n * n).sum::<f32>() > 1.000_02
            {
                continue;
            }
            let input = Input {
                position: eye,
                normal: shade::eye_normal(normals.as_ref(), v.normal, c.caps & CAP_NORMALIZE != 0),
                color: tracked.then_some(v.color),
            };
            let processor = shade::shade(&c.lighting, &input);
            let card = card_colours(&c, &gl, &v);
            let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as i32;
            for side in 0..2 {
                for k in 0..4 {
                    let step = (byte(processor[side][k]) - byte(card[side][k])).abs();
                    worst = worst.max(step);
                    assert!(
                        step <= 1,
                        "side {side}: the processor's {:?}, the card's {:?}, with {:?}",
                        processor[side],
                        card[side],
                        c.lighting
                    );
                }
            }
            compared += 1;
        }
        assert!(compared > 1500, "{compared} compared");
        // Not all of them to the bit: the tolerance is in use.
        assert!(worst <= 1);
    }

    /// A vertex at a light's position gets nothing from the light on
    /// the card, as from the layer's shading: the direction to the
    /// light has no length there.
    #[test]
    fn a_vertex_at_a_lights_position_gets_nothing_from_it_either_way() {
        use super::super::shade::Input;
        let mut c = context();
        c.modelview = vec![IDENTITY];
        let l = &mut c.lighting;
        l.enabled = 1;
        l.lights[0].position = [0.0, 0.0, -5.0, 1.0];
        l.lights[0].diffuse = [1.0; 4];
        l.lights[0].ambient = [0.0; 4];
        l.lights[0].attenuation = [1.0, 0.0, 0.0];
        l.ambient = [0.0; 4];
        l.materials[0].diffuse = [0.5; 4];
        let v = ImmVertex {
            position: [0.0, 0.0, -5.0, 1.0],
            normal: [0.0, 0.0, 1.0],
            color: [1.0; 4],
            ..ImmVertex::default()
        };
        let prims = [Prim::Triangle([0, 1, 2], 2, 7)];
        let gl = decide(&c, 1, &prims, &[v; 3]).unwrap();
        let input = Input {
            position: v.position,
            normal: v.normal,
            color: None,
        };
        let processor = shade::shade(&c.lighting, &input);
        let card = card_colours(&c, &gl, &v);
        assert_eq!(processor[0], [0.0, 0.0, 0.0, 0.5]);
        assert_eq!(card, processor);
    }

    /// Where the card puts a vertex on the screen against where the
    /// pipeline does, from the same clip coordinates (the product of
    /// the three matrices is the same to the bit either way). The
    /// card works out left + (x / w + 1) * width / 2, as its
    /// specification says, and the pipeline centre + x * (1 / w) *
    /// width / 2. OpenGL asks for neither order, only for x / w scaled
    /// and moved to the viewport, and both are within a 2000th of a
    /// pixel of that worked out in double precision, in viewports up
    /// to 2048 pixels: a 125th of the 16th of a pixel the card's
    /// rasteriser snaps a position to. A position near the middle of
    /// two such steps may be snapped to either, which moves an edge by
    /// a 16th of a pixel and gives the few pixels on it to the
    /// triangle on its other side.
    #[test]
    fn the_card_and_the_pipeline_put_a_vertex_within_a_2000th_of_a_pixel() {
        use super::super::super::ddraw::d3d::pipeline::ViewMap;
        use crate::toadforce::raster::geometry::Setup;
        use crate::toadforce::raster::vertex::Fvf;
        let mut d = Dice(4);
        let fvf = Fvf::parse(0x002).unwrap();
        let (mut worst, mut differ) = (0.0f64, 0);
        for _ in 0..20000 {
            let (x, y) = (d.between(0.0, 64.0) as u32, d.between(0.0, 64.0) as u32);
            let (w, h) = (
                d.between(16.0, 1984.0) as u32,
                d.between(16.0, 1984.0) as u32,
            );
            let l = level1::Level1 {
                viewport: level1::Viewport {
                    x,
                    y,
                    width: w,
                    height: h,
                    min_z: 0.0,
                    max_z: 1.0,
                },
                ..Default::default()
            };
            let setup = Setup::new(&l, &[], &fvf);
            let cw = d.between(0.05, 4000.0);
            let clip = [
                d.between(-cw, cw),
                d.between(-cw, cw),
                d.between(0.0, cw),
                cw,
            ];
            let mut v = setup.vertex(|_| 0);
            v.clip = clip;
            let card = setup.project(&v);
            let pipeline = ViewMap::dx7(x, y, w, h, 0.0, 1.0).screen(clip);
            let exact = [
                x as f64 + (clip[0] as f64 / cw as f64 + 1.0) * w as f64 / 2.0,
                y as f64 + (1.0 - clip[1] as f64 / cw as f64) * h as f64 / 2.0,
            ];
            for (k, of) in [(0, card.x), (1, card.y), (0, pipeline[0]), (1, pipeline[1])] {
                worst = worst.max((of as f64 - exact[k]).abs());
            }
            differ += (card.x != pipeline[0] || card.y != pipeline[1]) as u32;
        }
        assert!(worst <= 1.0 / 2000.0, "{worst}");
        // The two orders do round differently: the bound is in use.
        assert!(differ > 0);
    }
}
